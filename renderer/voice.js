'use strict';
// Voice calls between launcher friends: WebRTC audio straight from PC to PC.
// Signaling (offer / answer) travels as encrypted control messages in the existing chat, so no server is
// involved. Only public STUN servers are used to discover addresses; there is no TURN relay, so a call
// can fail when both sides sit behind a carrier-grade NAT.
// Once connected, a WebRTC data channel carries live state (mic muted / sound off) between the two sides.

const ICE = [{ urls: 'stun:stun.l.google.com:19302' }, { urls: 'stun:stun.cloudflare.com:3478' }];
const RING_TIMEOUT = 40e3;
const call = {
  state: 'idle', pk: null, id: null, pc: null, dc: null, mic: null, audio: null, gain: null, offer: null,
  startedAt: 0, muted: false, deafened: false, mutedBeforeDeafen: false, remote: { muted: false, deafened: false },
  timers: [], ring: null
};
const voiceSettings = () => (data.settings.voice ||= { volume: 100, micId: '', noise: true });

function inCall() { return call.state !== 'idle'; }

// ---------- UI ----------
function callUI() {
  const el = $('#call');
  el.className = 'call ' + call.state;
  pushOverlay();
  if (socialOpen()) renderSocialSoon();
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
    return;
  }
  // active: a compact pill at the top (hidden while the chat with this friend shows the full call stage)
  const stageVisible = socialOpen() && social.chat === call.pk;
  el.classList.toggle('stage-visible', stageVisible);
  el.innerHTML = `<div class="call-pill">
    <div class="call-av small ${speaking.remote ? 'speaking' : ''}" id="cRemoteAv">${avatar(call.pk, name)}${remoteBadge()}</div>
    <div><div class="t">${esc(name)}</div><div class="d" id="cTimer">00:00</div></div>
    ${callButtons('p')}
  </div>`;
  bindCallButtons(el);
  tickTimer();
}

function remoteBadge() {
  const ic = call.remote.deafened ? CALL_ICONS.headOff : call.remote.muted ? CALL_ICONS.micOff : '';
  return ic ? `<span class="rbadge">${ic}</span>` : '';
}
// the same control set is used in the pill, the chat call stage and (mirrored) in the overlay
function callButtons(cls) {
  return `<button class="${cls}btn ${call.muted ? 'off' : ''}" data-ca="mute" title="Микрофон">${call.muted ? CALL_ICONS.micOff : CALL_ICONS.mic}</button>
    <button class="${cls}btn ${call.deafened ? 'off' : ''}" data-ca="deafen" title="Выключить звук">${call.deafened ? CALL_ICONS.headOff : CALL_ICONS.head}</button>
    <button class="${cls}btn" data-ca="menu" title="Настройки звонка">${CALL_ICONS.gear}</button>
    <button class="${cls}btn hang" data-ca="hangup" title="Завершить">${CALL_ICONS.hang}</button>`;
}
function bindCallButtons(root) {
  root.querySelectorAll('[data-ca]').forEach((b) => (b.onclick = () => callAction(b.dataset.ca)));
}
function callAction(a) {
  if (a === 'mute') toggleMic();
  else if (a === 'deafen') toggleDeafen();
  else if (a === 'menu') openCallMenu();
  else if (a === 'hangup') hangUp();
  else if (a === 'accept') acceptCall();
  else if (a === 'decline') declineCall();
}

// big "voice stage" shown at the top of the chat with the friend we're talking to
function callStageHTML(pk) {
  if (!inCall() || call.pk !== pk || call.state === 'incoming') return '';
  const a = acct();
  const tile = (key, name, who, speakingNow, mutedIc) => `
    <div class="vtile ${speakingNow ? 'speaking' : ''}" id="vt-${who}">
      <div class="vav">${avatar(key, name, 'big')}</div>
      <div class="vname">${esc(name)}${mutedIc ? `<span class="vic">${mutedIc}</span>` : ''}</div>
    </div>`;
  const remoteIc = call.remote.deafened ? CALL_ICONS.headOff : call.remote.muted ? CALL_ICONS.micOff : '';
  const selfIc = call.deafened ? CALL_ICONS.headOff : call.muted ? CALL_ICONS.micOff : '';
  const status = { outgoing: 'Звоним…', connecting: 'Соединяем…' }[call.state];
  return `<div class="vstage">
    <div class="vstage-top"><span class="vdot"></span>${status || `Голосовой звонок · <b id="cTimer2">00:00</b>`}</div>
    <div class="vtiles">${tile(me.pk, a.name, 'self', speaking.self, selfIc)}${tile(pk, nameOf(pk), 'remote', speaking.remote, remoteIc)}</div>
    <div class="vdock">${callButtons('v')}<div class="vvol">${CALL_ICONS.vol}<span>${voiceSettings().volume}%</span></div></div>
  </div>`;
}

function tickTimer() {
  if (call.state !== 'active') return;
  const s = Math.floor((Date.now() - call.startedAt) / 1000);
  const txt = `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
  for (const id of ['cTimer', 'cTimer2']) { const t = document.getElementById(id); if (t) t.textContent = txt; }
}
setInterval(tickTimer, 1000);

function pushOverlay() {
  ipc.overlaySend({
    type: 'call', state: call.state, key: call.pk, name: call.pk ? nameOf(call.pk) : '',
    startedAt: call.startedAt, muted: call.muted, deafened: call.deafened, remote: call.remote
  });
}

// ---------- media ----------
async function getMic() {
  const v = voiceSettings();
  const audio = { echoCancellation: true, noiseSuppression: v.noise, autoGainControl: true };
  if (v.micId) audio.deviceId = { exact: v.micId };
  try {
    return await navigator.mediaDevices.getUserMedia({ audio });
  } catch {
    if (v.micId) { v.micId = ''; return getMic(); } // the chosen mic is gone — fall back to default
    toast('Нет доступа к микрофону — проверьте, что он подключён и разрешён в Windows', true, { title: 'Звонок' });
    return null;
  }
}

const speaking = { self: false, remote: false };
let speakTimer;
function watchLevels() {
  clearInterval(speakTimer);
  actx ??= new AudioContext();
  const meter = (stream) => {
    const an = actx.createAnalyser();
    an.fftSize = 256;
    actx.createMediaStreamSource(stream).connect(an);
    const buf = new Uint8Array(an.frequencyBinCount);
    return () => { an.getByteFrequencyData(buf); return buf.reduce((a, b) => a + b, 0) / buf.length; };
  };
  const self = call.mic ? meter(call.mic) : () => 0;
  const remote = call.remoteStream ? meter(call.remoteStream) : () => 0;
  speakTimer = setInterval(() => {
    const s = !call.muted && self() > 16;
    const r = !call.deafened && remote() > 16;
    if (s === speaking.self && r === speaking.remote) return;
    speaking.self = s;
    speaking.remote = r;
    $('#cRemoteAv')?.classList.toggle('speaking', r);
    $('#vt-remote')?.classList.toggle('speaking', r);
    $('#vt-self')?.classList.toggle('speaking', s);
    ipc.overlaySend({ type: 'speaking', remote: r, self: s });
  }, 120);
}

function newPeer(caller) {
  const pc = new RTCPeerConnection({ iceServers: ICE });
  if (caller) setupChannel(pc.createDataChannel('state'));
  else pc.ondatachannel = (e) => setupChannel(e.channel);
  pc.ontrack = (e) => {
    const stream = e.streams[0];
    call.remoteStream = stream;
    // Chrome only delivers remote WebRTC audio to Web Audio while it's also attached to a media element
    call.audio ||= new Audio();
    call.audio.srcObject = stream;
    call.audio.muted = true;
    call.audio.play().catch(() => {});
    actx ??= new AudioContext();
    actx.resume();
    call.gain = actx.createGain();
    applyGain();
    actx.createMediaStreamSource(stream).connect(call.gain).connect(actx.destination);
    watchLevels();
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

function setupChannel(dc) {
  call.dc = dc;
  dc.onopen = sendState;
  dc.onmessage = (e) => {
    try { call.remote = { ...call.remote, ...JSON.parse(e.data) }; } catch {}
    callUI();
  };
}
function sendState() {
  if (call.dc?.readyState === 'open') call.dc.send(JSON.stringify({ muted: call.muted, deafened: call.deafened }));
}

// gather all candidates up front so one message carries the whole description (no trickle needed)
function gathered(pc) {
  return new Promise((resolve) => {
    if (pc.iceGatheringState === 'complete') return resolve();
    pc.addEventListener('icegatheringstatechange', () => pc.iceGatheringState === 'complete' && resolve());
    setTimeout(resolve, 3000);
  });
}

const sendCall = (pk, t, extra = {}) => sendDM(pk, JSON.stringify({ pl: CTRL, t, n: acct().name, ...extra }));

// ---------- outgoing ----------
async function startCall(pk) {
  if (!me) return toast('Сначала создайте аккаунт в «Друзья и сообщения»', true);
  if (inCall()) return toast('Вы уже в звонке', true);
  const mic = await getMic();
  if (!mic) return;
  Object.assign(call, { state: 'outgoing', pk, id: crypto.randomUUID(), mic, muted: false, deafened: false, remote: { muted: false, deafened: false } });
  callUI();
  startRing(false);
  const pc = (call.pc = newPeer(true));
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
    Object.assign(call, { state: 'incoming', pk: from, id: ctrl.id, offer: ctrl.sdp, muted: false, deafened: false, remote: { muted: false, deafened: false } });
    callUI();
    startRing(true);
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
  const pc = (call.pc = newPeer(false));
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
  call.dc?.close();
  call.pc?.close();
  call.gain?.disconnect();
  if (call.audio) call.audio.srcObject = null;
  Object.assign(call, { state: 'idle', pk: null, id: null, pc: null, dc: null, mic: null, gain: null, offer: null, remoteStream: null, timers: [], muted: false, deafened: false });
  speaking.self = speaking.remote = false;
  closeCallMenu();
  callUI();
  if (msg && pk) toast(msg, false, { title: 'Звонок', avatar: { name: nameOf(pk), key: pk } });
  tone(520, 0.12, 0.04); tone(390, 0.18, 0.04, 'sine', 0.1);
}

// ---------- controls ----------
function setMicEnabled() {
  call.mic?.getAudioTracks().forEach((t) => (t.enabled = !call.muted));
}
function toggleMic() {
  if (!call.mic) return;
  call.muted = !call.muted;
  // un-muting the mic also brings the sound back, like Discord
  if (!call.muted && call.deafened) { call.deafened = false; applyGain(); }
  setMicEnabled();
  sendState();
  tone(call.muted ? 500 : 800, 0.06, 0.04);
  callUI();
}
// "Выключить звук": stop hearing the friend; also mutes the mic, like Discord's deafen
function toggleDeafen() {
  if (!inCall()) return;
  call.deafened = !call.deafened;
  if (call.deafened) { call.mutedBeforeDeafen = call.muted; call.muted = true; }
  else call.muted = call.mutedBeforeDeafen;
  setMicEnabled();
  applyGain();
  sendState();
  tone(call.deafened ? 450 : 750, 0.06, 0.04);
  callUI();
}
function applyGain() {
  if (call.gain) call.gain.gain.value = call.deafened ? 0 : voiceSettings().volume / 100;
}
function setFriendVolume(v) {
  voiceSettings().volume = Math.max(0, Math.min(200, Math.round(v / 5) * 5));
  applyGain();
  saveSoon();
}

async function switchMic(deviceId) {
  voiceSettings().micId = deviceId;
  saveSoon();
  if (!call.pc) return;
  const fresh = await getMic();
  if (!fresh) return;
  const track = fresh.getAudioTracks()[0];
  track.enabled = !call.muted;
  await call.pc.getSenders().find((s) => s.track?.kind === 'audio')?.replaceTrack(track);
  call.mic?.getTracks().forEach((t) => t.stop());
  call.mic = fresh;
  watchLevels();
}

// ---------- call menu (settings during a call) ----------
let callMenuOpen = false;
async function openCallMenu(focus = 0) {
  const v = voiceSettings();
  const mics = (await navigator.mediaDevices.enumerateDevices()).filter((d) => d.kind === 'audioinput' && d.deviceId !== 'communications');
  const micName = (mics.find((d) => d.deviceId === v.micId) || mics.find((d) => d.deviceId === 'default') || mics[0])?.label || 'По умолчанию';
  const items = [
    ['mute', call.muted ? 'Включить микрофон' : 'Выключить микрофон', ''],
    ['deafen', call.deafened ? 'Включить звук' : 'Выключить звук', 'не слышать собеседника'],
    ['vol', `Громкость ${call.pk ? nameOf(call.pk) : 'собеседника'}`, `${v.volume}%`],
    ['mic', 'Микрофон', micName.replace(/^По умолчанию - |^Default - /, '')],
    ['noise', 'Шумоподавление', v.noise ? 'Вкл' : 'Выкл'],
    ['hangup', 'Завершить звонок', '', 'danger']
  ];
  callMenuOpen = true;
  openModal(`<h2 style="padding:10px 18px 0">Настройки звонка</h2>` + items.map(([id, label, val, cls]) =>
    `<button class="menu-item focusable ${cls || ''}" data-id="${id}">${esc(label)}<span class="val">${id === 'vol' ? `<span class="volbar"><i style="width:${v.volume / 2}%"></i></span>` : ''}${esc(val)}</span></button>`).join('') +
    `<div class="meta" style="padding:8px 18px 4px"><span>Громкость: ◀ ▶ на этом пункте</span></div>`, { cls: 'menu', focus, onBack: closeCallMenu });
  $$('#modal .menu-item').forEach((b, i) => (b.onclick = async () => {
    const id = b.dataset.id;
    if (id === 'mute') toggleMic();
    if (id === 'deafen') toggleDeafen();
    if (id === 'vol') setFriendVolume(v.volume >= 200 ? 0 : v.volume + 25);
    if (id === 'noise') { v.noise = !v.noise; saveSoon(); if (call.pc) await switchMic(v.micId); }
    if (id === 'mic' && mics.length) {
      const cur = Math.max(0, mics.findIndex((d) => d.deviceId === (v.micId || 'default')));
      await switchMic(mics[(cur + 1) % mics.length].deviceId);
    }
    if (id === 'hangup') { closeCallMenu(); return hangUp(); }
    if (inCall()) openCallMenu(i);
  }));
}
function closeCallMenu() {
  if (!callMenuOpen) return;
  callMenuOpen = false;
  closeModal();
}
// left/right on the volume row nudges the volume
function callMenuKey(act) {
  if (!callMenuOpen) return false;
  const el = modalEls()[modal.idx];
  if (el?.dataset.id !== 'vol' || (act !== 'left' && act !== 'right')) return false;
  setFriendVolume(voiceSettings().volume + (act === 'right' ? 10 : -10));
  openCallMenu(modal.idx);
  return true;
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

// buttons pressed in the overlay window (over a game)
ipc.onOverlayAction((a) => callAction(a));

const CALL_ICONS = {
  phone: '<svg viewBox="0 0 24 24"><path d="M6.6 10.8a15.1 15.1 0 0 0 6.6 6.6l2.2-2.2c.3-.3.7-.4 1-.2 1.1.4 2.3.6 3.6.6.6 0 1 .4 1 1V20c0 .6-.4 1-1 1A17 17 0 0 1 3 4c0-.6.4-1 1-1h3.5c.6 0 1 .4 1 1 0 1.3.2 2.5.6 3.6.1.3 0 .7-.2 1z"/></svg>',
  hang: '<svg viewBox="0 0 24 24"><path d="M12 9c-1.6 0-3.1.3-4.6.7v3.1c0 .4-.2.7-.6.9-1 .5-1.9 1.1-2.7 1.8-.2.2-.4.3-.7.3s-.5-.1-.7-.3L.3 13.1A1 1 0 0 1 0 12.4c0-.3.1-.5.3-.7A16.9 16.9 0 0 1 12 7c4.6 0 8.7 1.8 11.7 4.7.2.2.3.4.3.7s-.1.5-.3.7l-2.4 2.4c-.2.2-.4.3-.7.3s-.5-.1-.7-.3c-.8-.7-1.7-1.3-2.7-1.8-.3-.2-.6-.5-.6-.9V9.7C15.1 9.3 13.6 9 12 9z"/></svg>',
  mic: '<svg viewBox="0 0 24 24"><path d="M12 14a3 3 0 0 0 3-3V5a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.9V21h2v-3.1a7 7 0 0 0 6-6.9z"/></svg>',
  micOff: '<svg viewBox="0 0 24 24"><path d="M19 11h-2c0 .7-.2 1.4-.4 2l1.5 1.5c.6-1 .9-2.2.9-3.5zm-4 .2V5a3 3 0 0 0-5.9-.8zM4.3 3 3 4.3l6 6V11a3 3 0 0 0 4.3 2.7l1.6 1.6A5 5 0 0 1 7 11H5a7 7 0 0 0 6 6.9V21h2v-3.1c1-.1 1.9-.5 2.7-1L19.7 21l1.3-1.3z"/></svg>',
  head: '<svg viewBox="0 0 24 24"><path d="M12 3a9 9 0 0 0-9 9v7a2 2 0 0 0 2 2h3v-8H5v-1a7 7 0 0 1 14 0v1h-3v8h3a2 2 0 0 0 2-2v-7a9 9 0 0 0-9-9z"/></svg>',
  headOff: '<svg viewBox="0 0 24 24"><path d="M3.3 2 2 3.3l2.6 2.6A9 9 0 0 0 3 12v7a2 2 0 0 0 2 2h3v-8H5v-1c0-1.2.3-2.4.9-3.4L19 21.7l1.3-1.3zM12 3c-1.7 0-3.3.5-4.7 1.3l1.5 1.5A7 7 0 0 1 19 12v1h-3v.2l5 5V12a9 9 0 0 0-9-9z"/></svg>',
  gear: '<svg viewBox="0 0 24 24"><path d="M19.4 13a7.6 7.6 0 0 0 0-2l2.1-1.6-2-3.5-2.5 1a7.4 7.4 0 0 0-1.7-1L15 3.3h-4l-.4 2.6a7.4 7.4 0 0 0-1.7 1l-2.5-1-2 3.5L6.6 11a7.6 7.6 0 0 0 0 2l-2.1 1.6 2 3.5 2.5-1a7.4 7.4 0 0 0 1.7 1l.4 2.6h4l.4-2.6a7.4 7.4 0 0 0 1.7-1l2.5 1 2-3.5zM12 15.5a3.5 3.5 0 1 1 0-7 3.5 3.5 0 0 1 0 7z"/></svg>',
  vol: '<svg viewBox="0 0 24 24"><path d="M4 9v6h4l5 5V4L8 9zm12.5 3A4.5 4.5 0 0 0 14 8v8a4.5 4.5 0 0 0 2.5-4zM14 3.2v2.1a7 7 0 0 1 0 13.4v2.1a9 9 0 0 0 0-17.6z"/></svg>'
};
