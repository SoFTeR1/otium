const { app, BrowserWindow, ipcMain, dialog, shell } = require('electron');
const path = require('path');
const fs = require('fs');
const { spawn } = require('child_process');
const { pathToFileURL } = require('url');
const steam = require('./steam');
const system = require('./system');
const gamepad = require('./gamepad');

let autoUpdater = null;
try {
  ({ autoUpdater } = require('electron-updater'));
} catch {}

const DATA_DIR = app.getPath('userData');
const DB_FILE = path.join(DATA_DIR, 'library.json');
const MEDIA_DIR = path.join(DATA_DIR, 'media');

const DEFAULT_DATA = {
  games: [],
  sessions: [],
  settings: {
    user: 'Player', sounds: true, fullscreen: true, autostart: false,
    bgMode: 'game', wallpaper: '', liveTrailers: true, steamApiKey: '', steamId: ''
  }
};

let win;
// set while the launcher is minimised because a game is running; cleared when the game exits
let hiddenForGame = false;

if (!app.requestSingleInstanceLock()) app.quit();
app.on('second-instance', () => restoreWindow());

function loadData() {
  try {
    const d = JSON.parse(fs.readFileSync(DB_FILE, 'utf8').replace(/^﻿/, ''));
    return { ...d, games: d.games || [], sessions: d.sessions || [], settings: { ...DEFAULT_DATA.settings, ...(d.settings || {}) } };
  } catch {
    return structuredClone(DEFAULT_DATA);
  }
}

function saveData(d) {
  fs.mkdirSync(DATA_DIR, { recursive: true });
  const tmp = DB_FILE + '.tmp';
  fs.writeFileSync(tmp, JSON.stringify(d, null, 2));
  fs.renameSync(tmp, DB_FILE);
}

const send = (ch, payload) => win && !win.isDestroyed() && win.webContents.send(ch, payload);

function createWindow() {
  const { settings } = loadData();
  win = new BrowserWindow({
    width: 1600,
    height: 900,
    minWidth: 1024,
    minHeight: 600,
    fullscreen: settings.fullscreen,
    backgroundColor: '#000814',
    autoHideMenuBar: true,
    title: 'PS5 Launcher',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      backgroundThrottling: false
    }
  });
  win.setMenu(null);
  win.loadFile(path.join(__dirname, 'renderer', 'index.html'));
  win.webContents.on('before-input-event', (e, input) => {
    if (input.type === 'keyDown' && input.key === 'F11') {
      win.setFullScreen(!win.isFullScreen());
      e.preventDefault();
    }
    if (input.type === 'keyDown' && input.key === 'F12') win.webContents.toggleDevTools();
  });
}

function restoreWindow() {
  if (!win) return;
  if (win.isMinimized()) win.restore();
  win.show();
  win.focus();
}

function gameEnded() {
  if (!hiddenForGame) return;
  hiddenForGame = false;
  restoreWindow();
}

// Split an argument string, respecting double quotes
function parseArgs(str) {
  const out = [];
  const re = /"([^"]*)"|(\S+)/g;
  let m;
  while ((m = re.exec(str))) out.push(m[1] ?? m[2]);
  return out;
}

ipcMain.handle('lib:load', () => loadData());
ipcMain.handle('lib:save', (_e, d) => saveData(d));

const FILTERS = {
  image: [{ name: 'Изображения', extensions: ['jpg', 'jpeg', 'png', 'webp', 'gif', 'bmp'] }],
  background: [
    { name: 'Изображения и видео', extensions: ['jpg', 'jpeg', 'png', 'webp', 'gif', 'bmp', 'mp4', 'webm'] }
  ],
  exe: [
    { name: 'Программы и ярлыки', extensions: ['exe', 'lnk', 'url', 'bat', 'cmd'] },
    { name: 'Все файлы', extensions: ['*'] }
  ]
};
ipcMain.handle('dlg:open', async (_e, kind) => {
  const r = await dialog.showOpenDialog(win, { properties: ['openFile'], filters: FILTERS[kind] || FILTERS.exe });
  return r.canceled ? null : r.filePaths[0];
});

ipcMain.handle('media:import', (_e, src) => {
  fs.mkdirSync(MEDIA_DIR, { recursive: true });
  const dest = path.join(MEDIA_DIR, `${Date.now()}_${Math.random().toString(36).slice(2, 8)}${path.extname(src)}`);
  fs.copyFileSync(src, dest);
  return pathToFileURL(dest).href;
});

ipcMain.handle('exe:icon', async (_e, exe) => {
  try {
    if (!fs.existsSync(exe)) return null;
    const icon = await app.getFileIcon(exe, { size: 'large' });
    fs.mkdirSync(MEDIA_DIR, { recursive: true });
    const dest = path.join(MEDIA_DIR, `icon_${Date.now()}.png`);
    fs.writeFileSync(dest, icon.toPNG());
    return pathToFileURL(dest).href;
  } catch {
    return null;
  }
});

// ---------- Launching ----------
// While a game is starting the launcher sits above every window (Steam's "launching…" dialog included);
// once the game is up we step aside, and come back when it exits.
ipcMain.handle('win:cover', (_e, on) => {
  if (!win) return;
  if (on) {
    win.setAlwaysOnTop(true, 'screen-saver');
    win.moveTop();
    win.focus();
  } else {
    win.setAlwaysOnTop(false);
  }
});
ipcMain.handle('win:stepAside', () => {
  if (!win) return;
  win.setAlwaysOnTop(false);
  hiddenForGame = true;
  win.minimize();
});

ipcMain.handle('game:launch', async (_e, game) => {
  const target = (game.path || '').trim().replace(/^"|"$/g, '');
  if (!target) return { ok: false, error: 'Не указан путь к игре' };

  if (/^[a-z][a-z0-9+.-]*:/i.test(target) && !/^[a-z]:[\\/]/i.test(target) && !/^file:/i.test(target)) {
    await shell.openExternal(target);
    return { ok: true, tracked: false };
  }
  if (!fs.existsSync(target)) return { ok: false, error: 'Файл не найден: ' + target };

  if (path.extname(target).toLowerCase() !== '.exe') {
    const err = await shell.openPath(target);
    return err ? { ok: false, error: err } : { ok: true, tracked: false };
  }

  const started = Date.now();
  return new Promise((resolve) => {
    const child = spawn(target, parseArgs(game.args || ''), { cwd: path.dirname(target), detached: true, stdio: 'ignore' });
    child.once('spawn', () => {
      child.unref();
      child.once('exit', () => {
        send('game:exited', { id: game.id, start: started, end: Date.now() });
        gameEnded();
      });
      resolve({ ok: true, tracked: true });
    });
    child.once('error', async () => {
      // e.g. the exe requires elevation — let the shell handle it
      const err = await shell.openPath(target);
      resolve(err ? { ok: false, error: err } : { ok: true, tracked: false });
    });
  });
});

// ---------- Steam ----------
ipcMain.handle('steam:scan', () => steam.scan());
ipcMain.handle('steam:local', () => steam.localStats());
ipcMain.handle('steam:running', () => steam.running());
ipcMain.handle('steam:news', (_e, appid) => steam.news(String(appid).replace(/\D/g, '')).catch(() => null));
ipcMain.handle('steam:trailer', (_e, appid) => steam.trailer(String(appid).replace(/\D/g, '')).catch(() => ''));
ipcMain.handle('steam:sales', (_e, steamId) => steam.sales(steamId).catch(() => null));
ipcMain.handle('steam:achievements', (_e, { appid, key, steamId }) =>
  steam.achievements(String(appid).replace(/\D/g, ''), key, steamId).catch(() => ({ error: 'error' })));
ipcMain.handle('steam:friends', (_e, { key, steamId }) => steam.friends(key, steamId).catch(() => ({ error: 'network' })));
ipcMain.handle('steam:check', (_e, { key, steamId }) => steam.checkAccount(key, steamId).catch(() => ({ error: 'network' })));

// ---------- System ----------
ipcMain.handle('sys:stats', () => system.stats());
ipcMain.handle('media:cmd', (_e, cmd) => system.mediaCommand(cmd));

ipcMain.handle('shell:showItem', (_e, p) => {
  if (p && fs.existsSync(p)) shell.showItemInFolder(p);
});
ipcMain.handle('shell:openUrl', (_e, url) => {
  if (/^(https:\/\/|steam:\/\/)/i.test(url)) shell.openExternal(url);
});

ipcMain.handle('win:fullscreen', (_e, on) => win.setFullScreen(on));
ipcMain.handle('win:quit', () => app.quit());
ipcMain.handle('app:autostart', (_e, on) => {
  const opts = app.isPackaged
    ? { openAtLogin: on, path: process.execPath }
    : { openAtLogin: on, path: process.execPath, args: [app.getAppPath()] };
  app.setLoginItemSettings(opts);
});
ipcMain.handle('app:version', () => app.getVersion());

// ---------- Updates (GitHub releases) ----------
if (autoUpdater) {
  autoUpdater.autoDownload = false;
  autoUpdater.autoInstallOnAppQuit = true;
  autoUpdater.on('download-progress', (p) => send('upd:progress', Math.round(p.percent)));
  autoUpdater.on('update-downloaded', (i) => send('upd:ready', i.version));
  autoUpdater.on('error', (e) => send('upd:error', String(e?.message || e)));
}
ipcMain.handle('upd:check', async () => {
  if (!autoUpdater || !app.isPackaged) return { status: 'dev', current: app.getVersion() };
  try {
    const r = await autoUpdater.checkForUpdates();
    const latest = r?.updateInfo?.version;
    const newer = latest && latest !== app.getVersion() && r.isUpdateAvailable !== false;
    return { status: newer ? 'available' : 'latest', current: app.getVersion(), latest, notes: r?.updateInfo?.releaseNotes || '' };
  } catch (e) {
    return { status: 'error', current: app.getVersion(), error: String(e?.message || e) };
  }
});
ipcMain.handle('upd:download', async () => {
  try {
    await autoUpdater.downloadUpdate();
    return true;
  } catch (e) {
    send('upd:error', String(e?.message || e));
    return false;
  }
});
ipcMain.handle('upd:install', () => autoUpdater.quitAndInstall(true, true));

app.whenReady().then(() => {
  createWindow();
  // Guide / View+Start on the gamepad: bring the launcher up from anywhere (or hand control back to the game)
  const hwnd = win.getNativeWindowHandle().readBigUInt64LE(0).toString();
  const stopPad = gamepad.watch(hwnd, (ev) => {
    if (ev === 'front') {
      restoreWindow();
      send('pad:guide', { from: 'background' });
    } else if (ev === 'toggle') send('pad:guide', { from: 'front' });
  });
  app.on('will-quit', stopPad);
  steam.watchRunning((id, prev) => {
    send('steam:running', { id, prev, at: Date.now() });
    if (!id && prev) gameEnded();
  });
});
app.on('window-all-closed', () => app.quit());
app.on('will-quit', () => system.shutdown());
