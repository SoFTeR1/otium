// Bridge for the always-on-top overlay window (call pill, incoming call, notifications)
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('overlay', {
  onMessage: (cb) => ipcRenderer.on('overlay:msg', (_e, m) => cb(m)),
  action: (a) => ipcRenderer.send('overlay:action', a),
  // the overlay is click-through except over its buttons
  interactive: (on) => ipcRenderer.send('overlay:interactive', on),
  visible: (on) => ipcRenderer.send('overlay:visible', on)
});
