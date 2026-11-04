const { app, BrowserWindow, ipcMain, dialog, shell, session, screen, globalShortcut, Tray, Menu, nativeImage } = require('electron');
const path = require('path');
const fs = require('fs');
const { spawn, execFile } = require('child_process');
const { pathToFileURL } = require('url');
const steam = require('./steam');
const system = require('./system');
const gamepad = require('./gamepad');
const discord = require('./discord');
const audio = require('./audio');

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
    bgMode: 'game', wallpaper: '', liveTrailers: true, steamApiKey: '', steamId: '',
    // the shared "PS5 Launcher" Discord application — an app ID is public, everyone can use the same one
    discordId: '1567308312947728414',
    closeToTray: true
  }
};

let win;
// set while the launcher is minimised because a game is running; cleared when the game exits
let hiddenForGame = false;
let pad = null;        // background gamepad watcher
let gameHwnd = null;   // window that was in front when the Guide button summoned us
let ownHwnd = null;
let gameStartedAt = 0; // when the current game was launched (to find its window)
let quitting = false;
let userMinimized = false; // minimised on purpose from the menu
let coverTimer = null; // keeps us topmost while a game is starting

if (!app.requestSingleInstanceLock()) app.quit();
// the window can already be gone while we're quitting (e.g. the updater relaunching us)
const alive = () => !!win && !win.isDestroyed();
app.on('second-instance', () => { if (!quitting) restoreWindow(); });

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
    icon: path.join(__dirname, 'renderer', 'icon.png'),
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      backgroundThrottling: false
    }
  });
  win.setMenu(null);
  // the close button / Alt+F4 hides to the tray instead of quitting (unless turned off in settings)
  win.on('close', (e) => {
    if (quitting || !loadData().settings.closeToTray) return;
    e.preventDefault();
    hideToTray();
  });
  win.on('focus', () => toOverlay({ type: 'focus', focused: true }));
  win.on('blur', () => toOverlay({ type: 'focus', focused: false }));
  // never let the desktop show: if anything minimises us (Win+D, a game), come straight back
  win.on('minimize', () => {
    // "Свернуть лаунчер" is the one minimise we keep; anything else (Win+D, a game) is undone
    if (!quitting && !userMinimized) setTimeout(() => alive() && win.restore(), 120);
  });
  win.on('restore', () => (userMinimized = false));
  win.on('show', () => (userMinimized = false));
  win.loadFile(path.join(__dirname, 'renderer', 'index.html'));
  win.webContents.on('before-input-event', (e, input) => {
    if (input.type === 'keyDown' && input.key === 'F11') {
      win.setFullScreen(!win.isFullScreen());
      e.preventDefault();
    }
    if (input.type === 'keyDown' && input.key === 'F12') win.webContents.toggleDevTools();
  });
}

// ---------- Tray: closing the window keeps the launcher running (chat, calls, Guide button) ----------
let tray = null;
let trayHinted = false;
function createTray() {
  const img = nativeImage.createFromPath(path.join(__dirname, 'renderer', 'icon.png')).resize({ width: 32, height: 32 });
  tray = new Tray(img);
  tray.setToolTip('PS5 Launcher');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: 'Открыть PS5 Launcher', click: () => forceFront() },
    { label: 'Центр управления', click: () => { forceFront(); send('pad:guide', { from: 'background', canReturn: !!gameHwnd }); } },
    { type: 'separator' },
    { label: 'Выйти', click: () => { quitting = true; app.quit(); } }
  ]));
  tray.on('click', () => forceFront());
}
function hideToTray() {
  clearInterval(coverTimer);
  coverTimer = null;
  win.setAlwaysOnTop(false);
  win.hide();
  if (!trayHinted) {
    trayHinted = true;
    tray?.displayBalloon({
      iconType: 'info',
      title: 'PS5 Launcher работает в трее',
      content: 'Откройте его значком в трее, кнопкой Xbox на геймпаде или Ctrl+Alt+Home. Выйти — правой кнопкой по значку.'
    });
  }
}

// ---------- Overlay: a transparent, click-through, always-on-top window over every app ----------
// Shows the call pill, incoming calls and notifications while the launcher itself isn't in focus.
let overlay = null;
function createOverlay() {
  const d = screen.getDisplayMatching(win.getBounds()).bounds;
  overlay = new BrowserWindow({
    x: d.x, y: d.y, width: d.width, height: d.height,
    transparent: true, frame: false, resizable: false, movable: false, focusable: false,
    skipTaskbar: true, hasShadow: false, show: false, backgroundColor: '#00000000',
    webPreferences: { preload: path.join(__dirname, 'overlay-preload.js'), contextIsolation: true, backgroundThrottling: false }
  });
  overlay.setAlwaysOnTop(true, 'screen-saver');
  overlay.setIgnoreMouseEvents(true, { forward: true });
  overlay.loadFile(path.join(__dirname, 'renderer', 'overlay.html'));
  overlay.webContents.once('did-finish-load', () => overlay.webContents.send('overlay:msg', { type: 'focus', focused: win.isFocused() }));
  overlay.on('closed', () => (overlay = null));
}
const toOverlay = (m) => overlay && !overlay.isDestroyed() && overlay.webContents.send('overlay:msg', m);
ipcMain.on('overlay:send', (_e, m) => toOverlay(m));
ipcMain.on('overlay:action', (_e, a) => {
  // clicking the voice widget brings the launcher forward to the call
  if (a === 'opencall') forceFront();
  send('overlay:action', a);
});
ipcMain.on('overlay:interactive', (_e, on) => overlay?.setIgnoreMouseEvents(!on, { forward: true }));
// clickable areas of the overlay (in overlay coordinates); the cursor is polled against them
let overlayRegions = [];
let overlayClickable = false;
ipcMain.on('overlay:regions', (_e, rects) => (overlayRegions = Array.isArray(rects) ? rects : []));
setInterval(() => {
  if (!overlay || overlay.isDestroyed() || !overlay.isVisible() || !overlayRegions.length) {
    if (overlayClickable && overlay && !overlay.isDestroyed()) { overlayClickable = false; overlay.setIgnoreMouseEvents(true, { forward: true }); }
    return;
  }
  const p = screen.getCursorScreenPoint();
  const b = overlay.getBounds();
  const x = p.x - b.x;
  const y = p.y - b.y;
  const inside = overlayRegions.some((r) => x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h);
  if (inside !== overlayClickable) {
    overlayClickable = inside;
    overlay.setIgnoreMouseEvents(!inside, { forward: true });
  }
}, 80);
// only show the window while it has something on it, so it never sits over a game for nothing
ipcMain.on('overlay:visible', (_e, on) => {
  if (!overlay) return;
  if (on && !overlay.isVisible()) { overlay.showInactive(); overlay.setAlwaysOnTop(true, 'screen-saver'); }
  else if (!on && overlay.isVisible()) overlay.hide();
});

function restoreWindow() {
  if (!alive()) return;
  if (win.isMinimized()) win.restore();
  win.show();
  win.focus();
}

// Windows won't let a background app take focus on its own; briefly going topmost and letting the
// gamepad helper (which can) activate our window gets us reliably in front of everything
function forceFront(holdMs = 1500) {
  if (!alive()) return;
  if (win.isMinimized()) win.restore();
  win.setAlwaysOnTop(true, 'screen-saver');
  win.show();
  win.moveTop();
  win.focus();
  if (ownHwnd) pad?.focus(ownHwnd);
  if (holdMs) setTimeout(() => { if (!coverTimer && alive()) win.setAlwaysOnTop(false); }, holdMs);
}

function gameEnded() {
  gameHwnd = null;
  gameStartedAt = 0;
  if (!hiddenForGame) return;
  hiddenForGame = false;
  forceFront();
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
ipcMain.handle('dlg:open', async (_e, kind, multi) => {
  const r = await dialog.showOpenDialog(win, { properties: multi ? ['openFile', 'multiSelections'] : ['openFile'], filters: FILTERS[kind] || FILTERS.exe });
  if (r.canceled) return null;
  return multi ? r.filePaths : r.filePaths[0];
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
// While a game is starting, keep re-asserting topmost: Steam's dialogs and some game launchers make
// themselves topmost too, and the last one to do so wins
ipcMain.handle('win:cover', (_e, on) => {
  if (!alive()) return;
  clearInterval(coverTimer);
  coverTimer = null;
  if (on) {
    forceFront(0);
    coverTimer = setInterval(() => { if (alive()) { win.setAlwaysOnTop(true, 'screen-saver'); win.moveTop(); } }, 600);
  } else {
    win.setAlwaysOnTop(false);
  }
});
// The launcher is the base layer of the screen, like the PS5 shell: it is never minimised.
// Once the game is up we stop being topmost and put the game's window above us; the desktop never shows.
ipcMain.handle('win:stepAside', (_e, launchedAt) => {
  if (!alive()) return;
  clearInterval(coverTimer);
  coverTimer = null;
  win.setAlwaysOnTop(false);
  hiddenForGame = true;
  gameStartedAt = launchedAt || Date.now();
  pad?.focusNew(gameStartedAt);
});
ipcMain.handle('win:minimize', () => {
  if (!alive()) return;
  clearInterval(coverTimer);
  coverTimer = null;
  userMinimized = true;
  win.setAlwaysOnTop(false);
  win.minimize();
});
// "Back to game": activate the game's window above us
ipcMain.handle('win:returnToGame', () => {
  if (!alive()) return;
  clearInterval(coverTimer);
  coverTimer = null;
  win.setAlwaysOnTop(false);
  hiddenForGame = true;
  if (gameHwnd) pad?.focus(gameHwnd);
  else if (gameStartedAt) pad?.focusNew(gameStartedAt);
});

// ---------- Closing a running game ----------
const gamePids = new Map(); // game id -> pid of the exe we started

// processes whose executable lives in `dir` (or is exactly `exe`)
function processesUnder({ dir, exe }) {
  return new Promise((resolve) => {
    const script = `Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and ((($env:PS5L_DIR) -and $_.ExecutablePath.StartsWith($env:PS5L_DIR + '\\', 'OrdinalIgnoreCase')) -or (($env:PS5L_EXE) -and $_.ExecutablePath -ieq $env:PS5L_EXE)) } | ForEach-Object { $_.ProcessId }`;
    execFile('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script],
      { windowsHide: true, env: { ...process.env, PS5L_DIR: dir || '', PS5L_EXE: exe || '' } },
      (err, out) => resolve(err ? [] : out.split(/\s+/).map(Number).filter((n) => n > 0)));
  });
}
const pidAlive = (pid) => { try { process.kill(pid, 0); return true; } catch { return false; } };
const taskkill = (pid, force) => new Promise((r) => execFile('taskkill', ['/PID', String(pid), '/T', ...(force ? ['/F'] : [])], { windowsHide: true }, () => r()));

ipcMain.handle('game:close', async (_e, game) => {
  const pids = new Set();
  if (gamePids.has(game.id)) pids.add(gamePids.get(game.id));
  const sid = game.steamId || (game.path || '').match(/^steam:\/\/(?:rungameid|run)\/(\d+)/i)?.[1];
  if (sid) {
    const dir = await steam.installDir(sid);
    if (dir) (await processesUnder({ dir })).forEach((p) => pids.add(p));
  } else if (/\.exe$/i.test(game.path || '')) {
    (await processesUnder({ exe: path.normalize(game.path.replace(/^"|"$/g, '')) })).forEach((p) => pids.add(p));
  }
  if (!pids.size) return { ok: false, error: 'Не нашёл процесс игры' };
  // ask nicely first (like the window's close button), then force whatever is still running
  for (const pid of pids) await taskkill(pid, false);
  for (let i = 0; i < 10 && [...pids].some(pidAlive); i++) await new Promise((r) => setTimeout(r, 500));
  const stubborn = [...pids].filter(pidAlive);
  for (const pid of stubborn) await taskkill(pid, true);
  return { ok: true, count: pids.size, forced: stubborn.length > 0 };
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
      gamePids.set(game.id, child.pid);
      child.once('exit', () => {
        gamePids.delete(game.id);
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
ipcMain.handle('steam:playerGames', (_e, { key, steamId }) => steam.playerGames(key, steamId).catch(() => ({ error: 'network' })));
// fresh=true skips the cache — used right after a game closes to catch newly unlocked trophies
ipcMain.handle('steam:achievementsFresh', (_e, { appid, key, steamId }) => {
  steam.clearCache(`ach${String(appid).replace(/\D/g, '')}:`);
  return steam.achievements(String(appid).replace(/\D/g, ''), key, steamId).catch(() => ({ error: 'error' }));
});

// ---------- Volume mixer ----------
ipcMain.handle('audio:cmd', (_e, cmd) => audio.command(cmd));
const iconCache = new Map();
ipcMain.handle('audio:icon', async (_e, p) => {
  if (!p) return null;
  if (!iconCache.has(p)) iconCache.set(p, await app.getFileIcon(p, { size: 'normal' }).then((i) => i.toDataURL()).catch(() => null));
  return iconCache.get(p);
});

// ---------- Discord / power ----------
ipcMain.handle('discord:config', (_e, id) => discord.configure(id));
ipcMain.handle('discord:activity', (_e, a) => discord.setActivity(a));
ipcMain.handle('discord:status', () => discord.isConnected());
ipcMain.handle('power:do', (_e, action) => {
  const run = (cmd, args) => spawn(cmd, args, { detached: true, stdio: 'ignore', windowsHide: true }).unref();
  if (action === 'shutdown') run('shutdown', ['/s', '/t', '0']);
  else if (action === 'restart') run('shutdown', ['/r', '/t', '0']);
  else if (action === 'sleep') {
    run('powershell', ['-NoProfile', '-Command', "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Application]::SetSuspendState('Suspend', $false, $false)"]);
  } else if (action === 'quit') app.quit();
});

ipcMain.handle('steam:check',(_e, { key, steamId }) => steam.checkAccount(key, steamId).catch(() => ({ error: 'network' })));

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
  // microphone for voice calls, system notifications for incoming calls
  session.defaultSession.setPermissionRequestHandler((_wc, perm, cb) => cb(['media', 'notifications', 'fullscreen'].includes(perm)));
  session.defaultSession.setPermissionCheckHandler((_wc, perm) => ['media', 'notifications', 'fullscreen'].includes(perm));
  createWindow();
  createOverlay();
  createTray();
  // Guide / View+Start on the gamepad: bring the launcher up from anywhere (or hand control back to the game)
  const hwnd = win.getNativeWindowHandle().readBigUInt64LE(0).toString();
  ownHwnd = hwnd;
  pad = gamepad.watch(hwnd, (ev) => {
    if (ev.startsWith('game ')) gameHwnd = ev.split(' ')[1];
    else if (ev.startsWith('front')) {
      // the window that was in front when Guide was pressed — normally the game
      const prev = ev.split(' ')[1];
      if (prev && prev !== '0' && prev !== hwnd) gameHwnd = prev;
      // minimised or in the tray: Home just brings the launcher back; over a game it also opens the menu
      const wasAway = userMinimized || !alive() || win.isMinimized() || !win.isVisible();
      restoreWindow();
      if (!wasAway) send('pad:guide', { from: 'background', canReturn: !!gameHwnd });
    } else if (ev === 'toggle') send('pad:guide', { from: 'front', canReturn: !!gameHwnd });
  });
  app.on('will-quit', () => pad.stop());
  // keyboard equivalent of the Guide button, from anywhere
  globalShortcut.register('Control+Alt+Home', () => {
    if (!alive() || quitting) return;
    if (win.isFocused()) return send('pad:guide', { from: 'front', canReturn: !!gameHwnd });
    forceFront();
    send('pad:guide', { from: 'background', canReturn: !!gameHwnd });
  });
  app.on('will-quit', () => globalShortcut.unregisterAll());
  steam.watchRunning((id, prev) => {
    send('steam:running', { id, prev, at: Date.now() });
    if (!id && prev) gameEnded();
  });
});
app.on('window-all-closed', () => app.quit());
app.on('before-quit', () => { quitting = true; });
app.on('will-quit', () => { system.shutdown(); audio.shutdown(); });
