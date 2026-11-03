const { contextBridge, ipcRenderer } = require('electron');

const on = (ch) => (cb) => ipcRenderer.on(ch, (_e, d) => cb(d));

contextBridge.exposeInMainWorld('api', {
  load: () => ipcRenderer.invoke('lib:load'),
  save: (data) => ipcRenderer.invoke('lib:save', data),
  openFile: (kind) => ipcRenderer.invoke('dlg:open', kind),
  importMedia: (src) => ipcRenderer.invoke('media:import', src),
  exeIcon: (exe) => ipcRenderer.invoke('exe:icon', exe),

  launch: (game) => ipcRenderer.invoke('game:launch', game),
  cover: (on) => ipcRenderer.invoke('win:cover', on),
  stepAside: () => ipcRenderer.invoke('win:stepAside'),
  onGameExited: on('game:exited'),

  scanSteam: () => ipcRenderer.invoke('steam:scan'),
  steamLocal: () => ipcRenderer.invoke('steam:local'),
  steamRunning: () => ipcRenderer.invoke('steam:running'),
  onSteamRunning: on('steam:running'),
  news: (appid) => ipcRenderer.invoke('steam:news', appid),
  trailer: (appid) => ipcRenderer.invoke('steam:trailer', appid),
  sales: (steamId) => ipcRenderer.invoke('steam:sales', steamId),
  achievements: (q) => ipcRenderer.invoke('steam:achievements', q),
  friends: (q) => ipcRenderer.invoke('steam:friends', q),

  sysStats: () => ipcRenderer.invoke('sys:stats'),
  media: (cmd) => ipcRenderer.invoke('media:cmd', cmd),

  showItem: (p) => ipcRenderer.invoke('shell:showItem', p),
  openUrl: (url) => ipcRenderer.invoke('shell:openUrl', url),
  setFullscreen: (on) => ipcRenderer.invoke('win:fullscreen', on),
  setAutostart: (on) => ipcRenderer.invoke('app:autostart', on),
  version: () => ipcRenderer.invoke('app:version'),
  quit: () => ipcRenderer.invoke('win:quit'),

  checkUpdate: () => ipcRenderer.invoke('upd:check'),
  downloadUpdate: () => ipcRenderer.invoke('upd:download'),
  installUpdate: () => ipcRenderer.invoke('upd:install'),
  onUpdateProgress: on('upd:progress'),
  onUpdateReady: on('upd:ready'),
  onUpdateError: on('upd:error')
});
