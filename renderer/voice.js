'use strict';
// Voice calls between launcher friends: WebRTC audio straight from PC to PC.
// Signaling (offer / answer) travels as encrypted control messages in the existing chat, so no server is
// involved. Only public STUN servers are used to discover addresses; there is no TURN relay, so a call
// can fail when both sides sit behind a carrier-grade NAT.

const ICE = [{ urls: 'stun:stun.l.google.com:19302' }, { urls: 'stun:stun.cloudflare.com:3478' }];
const RING_TIMEOUT = 40e3;
const call = { state: 'idle', pk: null, id: null, pc: null, mic: null, audio: null, offer: null, startedAt: 0, muted: false, timers: [], ring: null };

function inCall() { return call.state !== 'idle'; }

function callUI() {
  const el = $('#call');
  el.className = 'call ' + call.state;
  if (call.state === 'idle') { el.innerHTML = ''; return; }
  const name = nameOf(call.pk);
  if (call.state === 'incoming' || call.state === 'outgoing' || call.state === 'connecting') {
    const sub = { incoming: 'Входящий звонок', outgoing: 'Звоним…', connecting: 'Соединяем…' }[call.state];
    el.innerHTML = `<div class="call-card">
      <div class="call-av">${avatar(call.pk, name, 'big')}<i class="ring1"></i><i class="ring2"></i></div>
      <div class="call-sub">${sub}</div><div class="call-name">${esc(name)}</div>
      <div class="call-btns">
        ${call.state === 'incoming' ? `<button class="cbtn accept" id="cAccept">${CALL_ICONS.phone}<span>Принять</span></button>` : ''}
        <button class="cbtn hang" id="cHang">${CALL_ICONS.hang}<span>${call.state === 'incoming' ? 'Отклонить' : 'Отменить'}</span></button>
      </div>
      <div class="call-hint">${call.state === 'incoming' ? `${oskHint('ok', 'принять')} ${oskHint('back', 'отклонить')}` : oskHint('back', 'отменить')}</div>
    </div>`;
    $('#cAccept') && ($('#cAccept').onclick = acceptCall);
    $('#cHang').onclick = () => (call.state === 'incoming' ? declineCall() : hangUp());
  } else {
    // active: a compact pill at the top of the screen
    el.innerHTML = `<div class="call-pill">
      <div class="call-av small" id="cRemoteAv">${avatar(call.pk, name)}</div>
      <div><div class="t">${esc(name)}</div><div class="d" id="cTimer">00:00</div></div>
      <button class="pbtn ${call.muted ? 'off' : ''}" id="cMute" title="Микрофон">${call.muted ? CALL_ICONS.micOff : CALL_ICONS.mic}</button>
      <button class="pbtn hang" id="cHang2" title="Завершить">${CALL_ICONS.hang}</button>
    </div>`;
    $('#cMute').onclick = toggleMic;
    $('#cHang2').onclick = hangUp;
    tickTimer();
  }
}

function tickTimer() {
  const t = $('#cTimer');
  if (!t || call.state !== 'active') return;
  const s = Math.floor((Date.now() - call.startedAt) / 1000);
  t.textContent = `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}
setInterval(tickTimer, 1000);

// ---------- media ----------
async function getMic() {
  try {
    return await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true } });
  } catch {
    toast('Нет доступа к микрофону — проверьте, что он подключён и разрешён в Windows', true, { title: 'Звонок' });
    return null;
  }
}

function newPeer() {
  const pc = new RTCPeerConnection({ iceServers: ICE });
  pc.ontrack = (e) => {
    call.audio ||= new Audio();
    call.audio.srcObject = e.streams[0];
    call.audio.play().catch(() => {});
    watchSpeaking(e.streams[0]);
  };
  pc.onconnectionstatechange = () => {
    if (pc !== call.pc) return;
    if (pc.connectionState === 'connected' && call.state !== 'active') {
      call.state = 'active';
      call.startedAt = Date.now();
      stopRing();
      tone(660, 0.12, 0.05); tone(990, 0.15, 0.04, 'sine', 0.1);
      callUI();
    } else if (pc.connectionState === 'failed') {
      toast('Не удалось соединиться напрямую. Чаще всего так бывает, когда у обоих провайдер раздаёт адреса «за общим NAT».', true, { title: 'Звонок не соединился', ms: 8000 });
      hangUp();
    } else if (pc.connectionState === 'disconnected') {
      // brief network hiccups recover by themselves; give it a few seconds
      call.timers.push(setTimeout(() => pc === call.pc && pc.connectionState !== 'connected' && hangUp(), 8000));
    }
  };
  return pc;
}

// gather all candidates up front so one message carries the whole description (no trickle needed)
function gathered(pc) {
  return new Promise((resolve) => {
    if (pc.iceGatheringState === 'complete') return resolve();
    const done = () => pc.iceGatheringState === 'complete' && resolve();
    pc.addEventListener('icegatheringstatechange', done);
    setTimeout(resolve, 3000);
  });
}

let speakTimer;
function watchSpeaking(stream) {
  try {
    actx ??= new AudioContext();
    const an = actx.createAnalyser();
    an.fftSize = 256;
    actx.createMediaStreamSource(stream).connect(an);
    const buf = new Uint8Array(an.frequencyBinCount);
    clearInterval(speakTimer);
    speakTimer = setInterval(() => {
      an.getByteFrequencyData(buf);
      const level = buf.reduce((a, b) => a + b, 0) / buf.length;
      $('#cRemoteAv')?.classList.toggle('speaking', level > 18);
    }, 120);
  } catch {}
}

const sendCall = (pk, t, extra = {}) => sendDM(pk, JSON.stringify({ pl: CTRL, t, n: acct().name, ...extra }));

// ---------- outgoing ----------
async function startCall(pk) {
  if (!me) return toast('Сначала создайте аккаунт в «Друзья и сообщения»', true);
  if (inCall()) return toast('Вы уже в звонке', true);
  const mic = await getMic();
  if (!mic) return;
  Object.assign(call, { state: 'outgoing', pk, id: crypto.randomUUID(), mic, muted: false });
  callUI();
  startRing(false);
  const pc = (call.pc = newPeer());
  mic.getTracks().forEach((t) => pc.addTrack(t, mic));
  await pc.setLocalDescription(await pc.createOffer());
  await gathered(pc);
  if (call.pc !== pc) return;
  sendCall(pk, 'call', { id: call.id, sdp: pc.localDescription.sdp });
  call.timers.push(setTimeout(() => {
    if (call.state === 'outgoing') { toast(`${nameOf(pk)} не отвечает`, false, { title: 'Звонок', avatar: { name: nameOf(pk), key: pk } }); hangUp(); }
  }, RING_TIMEOUT));
}

// ---------- incoming ----------
function onCallSignal(from, ctrl, createdAt) {
  // ignore stale signals replayed from history
  if (Date.now() - createdAt * 1000 > 60e3) return;
  if (ctrl.t === 'call') {
    if (!isFriend(from)) return;
    if (inCall()) return sendCall(from, 'busy', { id: ctrl.id });
    Object.assign(call, { state: 'incoming', pk: from, id: ctrl.id, offer: ctrl.sdp, muted: false });
    callUI();
    startRing(true);
    if (!document.hasFocus()) {
      try { new Notification(`${nameOf(from)} звонит`, { body: 'Нажмите кнопку Xbox на геймпаде, чтобы ответить', silent: true }); } catch {}
    }
    call.timers.push(setTimeout(() => call.state === 'incoming' && call.id === ctrl.id && endLocal(`Пропущенный звонок от ${nameOf(from)}`), RING_TIMEOUT));
    return;
  }
  if (from !== call.pk || ctrl.id !== call.id) return;
  if (ctrl.t === 'ans' && call.state === 'outgoing') {
    call.state = 'connecting';
    callUI();
    call.pc.setRemoteDescription({ type: 'answer', sdp: ctrl.sdp }).catch(() => hangUp());
    const pc = call.pc;
    call.timers.push(setTimeout(() => connectTimeout(pc), 25000));
  } else if (ctrl.t === 'decl') endLocal(`${nameOf(from)} отклонил(а) звонок`);
  else if (ctrl.t === 'busy') endLocal(`${nameOf(from)} сейчас в другом звонке`);
  else if (ctrl.t === 'end') endLocal(call.state === 'incoming' ? `Пропущенный звонок от ${nameOf(from)}` : 'Звонок завершён');
}

async function acceptCall() {
  if (call.state !== 'incoming') return;
  stopRing();
  const mic = await getMic();
  if (!mic) return declineCall();
  call.mic = mic;
  call.state = 'connecting';
  callUI();
  const pc = (call.pc = newPeer());
  mic.getTracks().forEach((t) => pc.addTrack(t, mic));
  try {
    await pc.setRemoteDescription({ type: 'offer', sdp: call.offer });
    await pc.setLocalDescription(await pc.createAnswer());
    await gathered(pc);
    sendCall(call.pk, 'ans', { id: call.id, sdp: pc.localDescription.sdp });
    call.timers.push(setTimeout(() => connectTimeout(pc), 25000));
  } catch {
    hangUp();
  }
}

function connectTimeout(pc) {
  if (pc !== call.pc || call.state !== 'connecting') return;
  toast('Не удалось соединиться напрямую. Чаще всего так бывает, когда у обоих провайдер раздаёт адреса «за общим NAT».', true, { title: 'Звонок не соединился', ms: 8000 });
  hangUp();
}

function declineCall() {
  if (call.state !== 'incoming') return;
  sendCall(call.pk, 'decl', { id: call.id });
  endLocal();
}

function hangUp() {
  if (!inCall()) return;
  if (call.pk) sendCall(call.pk, 'end', { id: call.id });
  endLocal('Звонок завершён');
}

function endLocal(msg) {
  const pk = call.pk;
  stopRing();
  call.timers.forEach(clearTimeout);
  clearInterval(speakTimer);
  call.mic?.getTracks().forEach((t) => t.stop());
  call.pc?.close();
  if (call.audio) call.audio.srcObject = null;
  Object.assign(call, { state: 'idle', pk: null, id: null, pc: null, mic: null, offer: null, timers: [] });
  callUI();
  if (msg && pk) toast(msg, false, { title: 'Звонок', avatar: { name: nameOf(pk), key: pk } });
  tone(520, 0.12, 0.04); tone(390, 0.18, 0.04, 'sine', 0.1);
}

function toggleMic() {
  if (!call.mic) return;
  call.muted = !call.muted;
  call.mic.getAudioTracks().forEach((t) => (t.enabled = !call.muted));
  callUI();
}

// ---------- ringtone ----------
function startRing(incoming) {
  stopRing();
  const once = () => {
    const was = data.settings.sounds;
    data.settings.sounds = true; // a ringtone should ring even with UI sounds off
    if (incoming) { tone(880, 0.25, 0.06); tone(1100, 0.25, 0.06, 'sine', 0.3); }
    else tone(440, 0.6, 0.035);
    data.settings.sounds = was;
  };
  once();
  call.ring = setInterval(once, incoming ? 1600 : 2600);
}
function stopRing() {
  clearInterval(call.ring);
  call.ring = null;
}

// gamepad / keyboard while the call card is up
function callCardOpen() { return ['incoming', 'outgoing', 'connecting'].includes(call.state); }
function callHandle(act) {
  if (call.state === 'incoming') {
    if (act === 'ok') return acceptCall();
    if (act === 'back') return declineCall();
  } else if (act === 'back') return hangUp();
}

const CALL_ICONS = {
  phone: '<svg viewBox="0 0 24 24"><path d="M6.6 10.8a15.1 15.1 0 0 0 6.6 6.6l2.2-2.2c.3-.3.7-.4 1-.2 1.1.4 2.3.6 3.6.6.6 0 1 .4 1 1V20c0 .6-.4 1-1 1A17 17 0 0 1 3 4c0-.6.4-1 1-1h3.5c.6 0 1 .4 1 1 0 1.3.2 2.5.6 3.6.1.3 0 .7-.2 1z"/></svg>',
  hang: '<svg viewBox="0 0 24 24"><path d="M12 9c-1.6 0-3.1.3-4.6.7v3.1c0 .4-.2.7-.6.9-1 .5-1.9 1.1-2.7 1.8-.2.2-.4.3-.7.3s-.5-.1-.7-.3L.3 13.1A1 1 0 0 1 0 12.4c0-.3.1-.5.3-.7A16.9 16.9 0 0 1 12 7c4.6 0 8.7 1.8 11.7 4.7.2.2.3.4.3.7s-.1.5-.3.7l-2.4 2.4c-.2.2-.4.3-.7.3s-.5-.1-.7-.3c-.8-.7-1.7-1.3-2.7-1.8-.3-.2-.6-.5-.6-.9V9.7C15.1 9.3 13.6 9 12 9z"/></svg>',
  mic: '<svg viewBox="0 0 24 24"><path d="M12 14a3 3 0 0 0 3-3V5a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.9V21h2v-3.1a7 7 0 0 0 6-6.9z"/></svg>',
  micOff: '<svg viewBox="0 0 24 24"><path d="M19 11h-2c0 .7-.2 1.4-.4 2l1.5 1.5c.6-1 .9-2.2.9-3.5zm-4 .2V5a3 3 0 0 0-5.9-.8zM4.3 3 3 4.3l6 6V11a3 3 0 0 0 4.3 2.7l1.6 1.6A5 5 0 0 1 7 11H5a7 7 0 0 0 6 6.9V21h2v-3.1c1-.1 1.9-.5 2.7-1L19.7 21l1.3-1.3z"/></svg>'
};
