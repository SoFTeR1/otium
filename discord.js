// Discord Rich Presence over the local IPC pipe — no Discord SDK needed.
// Frames are: int32 opcode, int32 length, JSON payload. Opcode 0 = handshake, 1 = command.
const net = require('net');

let sock = null;
let clientId = null;
let ready = false;
let pending = null;   // last activity requested while not connected
let retry = null;

function frame(op, payload) {
  const json = Buffer.from(JSON.stringify(payload));
  const head = Buffer.alloc(8);
  head.writeInt32LE(op, 0);
  head.writeInt32LE(json.length, 4);
  return Buffer.concat([head, json]);
}

function connect(i = 0) {
  if (!clientId || sock) return;
  if (i > 9) { scheduleRetry(); return; }
  const s = net.createConnection(`\\\\?\\pipe\\discord-ipc-${i}`);
  s.once('error', () => { s.destroy(); if (sock === s) sock = null; connect(i + 1); });
  s.once('connect', () => {
    sock = s;
    s.removeAllListeners('error');
    s.on('error', () => {});
    s.on('close', () => { sock = null; ready = false; scheduleRetry(); });
    s.on('data', (buf) => {
      // the first DISPATCH (READY) after the handshake means we can set activity
      try {
        const msg = JSON.parse(buf.slice(8).toString());
        if (msg.evt === 'READY') { ready = true; if (pending !== undefined) send(pending); }
      } catch {}
    });
    s.write(frame(0, { v: 1, client_id: clientId }));
  });
}

function scheduleRetry() {
  clearTimeout(retry);
  if (clientId) retry = setTimeout(() => connect(), 20000);
}

function send(activity) {
  if (!sock || !ready) return;
  sock.write(frame(1, { cmd: 'SET_ACTIVITY', args: { pid: process.pid, activity }, nonce: String(Date.now()) }));
}

// activity: null clears the status
function setActivity(activity) {
  pending = activity;
  if (ready) send(activity);
  else connect();
}

function configure(id) {
  id = (id || '').replace(/\D/g, '');
  if (id === clientId) return;
  clientId = id || null;
  ready = false;
  sock?.destroy();
  sock = null;
  if (clientId) connect();
}

module.exports = { configure, setActivity, isConnected: () => ready };
