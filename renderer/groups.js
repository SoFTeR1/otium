'use strict';
// Groups: a shared chat plus a voice room any member can drop into (Discord-server style), for 3–5 friends.
// Everything travels as encrypted control messages in the existing chat channel, one copy per member.
// The voice room is a mesh: every two participants hold their own direct WebRTC connection. To avoid both
// sides calling each other at once, the participant with the smaller key always makes the offer.

const GROUP_CTRL = ['ginv', 'gmsg', 'gleave', 'vjoin', 'vhere', 'voff', 'vans', 'vleave', 'vping', 'vring', 'vdecl'];
const MAX_GROUP = 5;
const room = { gid: null, peers: new Map(), mic: null, muted: false, deafened: false, mutedBeforeDeafen: false, joinedAt: 0, pingTimer: null, levelTimer: null };
const roster = new Map(); // gid -> Map(pk -> last seen) — who is in each voice room, as far as we know
const ringing = { gid: null, from: null, timer: null, ring: null };

const groups = () => {
  const a = acct();
  if (!a) return [];
  return (a.groups ||= []);
};
const groupById = (gid) => groups().find((g) => g.id === gid);
const isMember = (g, pk) => !!g?.members.some((m) => m.pk === pk);
const memberName = (g, pk) => (pk === me?.pk ? acct().name : g?.members.find((m) => m.pk === pk)?.name || nameOf(pk));
const groupTitle = (g) => g.name || g.members.filter((m) => m.pk !== me.pk).map((m) => m.name).join(', ');

function sendGroup(g, t, extra = {}, onlyPk) {
  const body = JSON.stringify({ pl: CTRL, t, gid: g.id, n: acct().name, ...extra });
  for (const m of g.members) if (m.pk !== me.pk && (!onlyPk || m.pk === onlyPk)) sendDM(m.pk, body);
}

// ---------- groups ----------
function createGroup(name, friendPks) {
  const g = {
    id: crypto.randomUUID().replace(/-/g, '').slice(0, 16),
    name: name.trim(),
    owner: me.pk,
    created: Date.now(),
    members: [{ pk: me.pk, name: acct().name }, ...friendPks.map((pk) => ({ pk, name: nameOf(pk) }))],
    history: []
  };
  groups().push(g);
  saveSoon();
  sendGroup(g, 'ginv', { name: g.name, owner: g.owner, members: g.members });
  renderSocialSoon();
  return g;
}

function inviteToGroup(g, pks) {
  for (const pk of pks) if (!isMember(g, pk) && g.members.length < MAX_GROUP) g.members.push({ pk, name: nameOf(pk) });
  saveSoon();
  sendGroup(g, 'ginv', { name: g.name, owner: g.owner, members: g.members });
  renderSocialSoon();
}

function leaveGroup(g) {
  if (room.gid === g.id) leaveRoom();
  sendGroup(g, 'gleave');
  acct().groups = groups().filter((x) => x.id !== g.id);
  if (social.chat === 'g:' + g.id) { social.chat = null; social.zone = 'list'; }
  saveSoon();
  renderSocialSoon();
}

function sendGroupMessage(g, text) {
  const mid = crypto.randomUUID().slice(0, 12);
  g.history.push({ id: mid, from: me.pk, text, at: Date.now() });
  trimHistory(g);
  sendGroup(g, 'gmsg', { text, mid });
  markGroupRead(g);
  saveSoon();
  renderSocialSoon();
}
const trimHistory = (g) => g.history.length > 400 && g.history.splice(0, g.history.length - 400);

function markGroupRead(g) {
  acct().lastRead['g:' + g.id] = Date.now();
  saveSoon();
  updateDot();
}
function groupUnread(g) {
  const last = acct()?.lastRead?.['g:' + g.id] || 0;
  return g.history.filter((m) => m.from !== me.pk && m.at > last).length;
}

// ---------- incoming group control messages ----------
function onGroupCtrl(from, ctrl, createdAt) {
  const age = Date.now() - createdAt * 1000;
  let g = groupById(ctrl.gid);

  if (ctrl.t === 'ginv') {
    const mine = ctrl.members?.some((m) => m.pk === me.pk);
    if (!ctrl.members?.some((m) => m.pk === from)) return;
    if (!g) {
      if (!mine) return;
      g = { id: ctrl.gid, name: ctrl.name, owner: ctrl.owner, created: createdAt * 1000, members: ctrl.members, history: [] };
      groups().push(g);
      if (age < 10 * 60e3) toast(`${ctrl.n || 'Друг'} добавил(а) вас в группу «${groupTitle(g)}»`, false, { title: 'Новая группа', icon: '👥' });
    } else if (isMember(g, from)) {
      if (!mine) return leaveGroupLocal(g);
      g.name = ctrl.name;
      g.members = ctrl.members;
    }
    saveSoon();
    renderSocialSoon();
    return;
  }
  if (!g || !isMember(g, from)) return;

  if (ctrl.t === 'gmsg') {
    if (g.history.some((m) => m.id === ctrl.mid)) return;
    g.history.push({ id: ctrl.mid, from, text: ctrl.text, at: createdAt * 1000 });
    g.history.sort((a, b) => a.at - b.at);
    trimHistory(g);
    saveSoon();
    const viewing = socialOpen() && social.chat === 'g:' + g.id && !document.hidden;
    if (viewing) markGroupRead(g);
    else if (age < 5 * 60e3) {
      sfx.ok();
      toast(ctrl.text.slice(0, 160), false, { title: `${memberName(g, from)} · ${groupTitle(g)}`, avatar: { name: memberName(g, from), key: from } });
    }
    updateDot();
    renderSocialSoon();
    return;
  }
  if (ctrl.t === 'gleave') {
    g.members = g.members.filter((m) => m.pk !== from);
    closePeer(from);
    rosterDel(g.id, from);
    saveSoon();
    renderSocialSoon();
    return;
  }

  // voice room signals are only meaningful while fresh
  if (age > 90e3) return;
  if (ctrl.t === 'vjoin' || ctrl.t === 'vhere' || ctrl.t === 'vping') {
    rosterSet(g.id, from);
    if (room.gid === g.id) {
      if (ctrl.t === 'vjoin') sendGroup(g, 'vhere', {}, from);
      if (ctrl.t !== 'vping' && me.pk < from && !room.peers.has(from)) connectPeer(g, from);
    }
  } else if (ctrl.t === 'vleave') {
    rosterDel(g.id, from);
    closePeer(from);
  } else if (ctrl.t === 'voff' && room.gid === g.id) {
    answerPeer(g, from, ctrl.sdp);
  } else if (ctrl.t === 'vans' && room.gid === g.id) {
    room.peers.get(from)?.pc.setRemoteDescription({ type: 'answer', sdp: ctrl.sdp }).catch(() => closePeer(from));
  } else if (ctrl.t === 'vring') {
    rosterSet(g.id, from);
    if (room.gid !== g.id && !inCall() && !callCardOpen()) startGroupRing(g, from);
  }
  roomUI();
  renderSocialSoon();
}
function leaveGroupLocal(g) {
  if (room.gid === g.id) leaveRoom();
  acct().groups = groups().filter((x) => x.id !== g.id);
  saveSoon();
  renderSocialSoon();
}

// ---------- roster (who's in a room) ----------
function rosterSet(gid, pk) {
  if (!roster.has(gid)) roster.set(gid, new Map());
  roster.get(gid).set(pk, Date.now());
}
function rosterDel(gid, pk) { roster.get(gid)?.delete(pk); }
function inRoom(gid) {
  const r = roster.get(gid);
  if (!r) return [];
  // participants ping every minute; anyone silent for 3 minutes is gone
  return [...r.entries()].filter(([pk, t]) => pk === me?.pk || Date.now() - t < 180e3).map(([pk]) => pk);
}

// ---------- voice room ----------
async function joinRoom(gid) {
  const g = groupById(gid);
  if (!g || room.gid === gid) return;
  if (room.gid) leaveRoom();
  if (inCall()) hangUp();
  stopGroupRing();
  const mic = await getMic();
  if (!mic) return;
  const wasEmpty = inRoom(gid).filter((pk) => pk !== me.pk).length === 0;
  Object.assign(room, { gid, mic, muted: false, deafened: false, joinedAt: Date.now() });
  room.peers = new Map();
  rosterSet(gid, me.pk);
  sendGroup(g, 'vjoin');
  if (wasEmpty) sendGroup(g, 'vring');
  callSound.connect();
  clearInterval(room.pingTimer);
  room.pingTimer = setInterval(() => groupById(gid) && sendGroup(groupById(gid), 'vping'), 60e3);
  watchRoomLevels();
  roomUI();
  renderSocialSoon();
}

function leaveRoom() {
  const g = groupById(room.gid);
  if (!room.gid) return;
  if (g) sendGroup(g, 'vleave');
  rosterDel(room.gid, me.pk);
  for (const pk of [...room.peers.keys()]) closePeer(pk);
  room.mic?.getTracks().forEach((t) => t.stop());
  clearInterval(room.pingTimer);
  clearInterval(room.levelTimer);
  Object.assign(room, { gid: null, mic: null, muted: false, deafened: false });
  callSound.end();
  roomUI();
  renderSocialSoon();
}

function newRoomPeer(g, pk, offerer) {
  const pc = new RTCPeerConnection({ iceServers: ICE });
  const peer = { pc, dc: null, gain: null, stream: null, state: { muted: false, deafened: false }, speaking: false, connected: false };
  room.peers.set(pk, peer);
  room.mic.getTracks().forEach((t) => pc.addTrack(t, room.mic));
  const setupDc = (dc) => {
    peer.dc = dc;
    dc.onopen = () => sendRoomState(peer);
    dc.onmessage = (e) => { try { peer.state = JSON.parse(e.data); } catch {} roomUI(); renderSocialSoon(); };
  };
  if (offerer) setupDc(pc.createDataChannel('state'));
  else pc.ondatachannel = (e) => setupDc(e.channel);
  pc.ontrack = (e) => {
    peer.stream = e.streams[0];
    // Chrome only feeds remote WebRTC audio into Web Audio while it's also on a media element
    peer.audio = new Audio();
    peer.audio.srcObject = peer.stream;
    peer.audio.muted = true;
    peer.audio.play().catch(() => {});
    actx ??= new AudioContext();
    actx.resume();
    peer.gain = actx.createGain();
    applyRoomGain(peer);
    actx.createMediaStreamSource(peer.stream).connect(peer.gain).connect(actx.destination);
    watchRoomLevels();
  };
  pc.onconnectionstatechange = () => {
    if (pc.connectionState === 'connected' && !peer.connected) {
      peer.connected = true;
      callSound.click(true);
      roomUI();
      renderSocialSoon();
    } else if (pc.connectionState === 'failed') {
      toast(`Не удалось соединиться с ${memberName(g, pk)} напрямую`, true, { title: groupTitle(g) });
      closePeer(pk);
    }
  };
  return peer;
}

async function connectPeer(g, pk) {
  const peer = newRoomPeer(g, pk, true);
  await peer.pc.setLocalDescription(await peer.pc.createOffer());
  await gathered(peer.pc);
  if (room.peers.get(pk) === peer) sendGroup(g, 'voff', { sdp: peer.pc.localDescription.sdp }, pk);
}

async function answerPeer(g, pk, sdp) {
  closePeer(pk);
  const peer = newRoomPeer(g, pk, false);
  try {
    await peer.pc.setRemoteDescription({ type: 'offer', sdp });
    await peer.pc.setLocalDescription(await peer.pc.createAnswer());
    await gathered(peer.pc);
    sendGroup(g, 'vans', { sdp: peer.pc.localDescription.sdp }, pk);
  } catch {
    closePeer(pk);
  }
}

function closePeer(pk) {
  const p = room.peers.get(pk);
  if (!p) return;
  room.peers.delete(pk);
  p.dc?.close();
  p.pc.close();
  p.gain?.disconnect();
  if (p.audio) p.audio.srcObject = null;
  if (p.connected) callSound.click(false);
  roomUI();
  renderSocialSoon();
}

function sendRoomState(peer) {
  if (peer.dc?.readyState === 'open') peer.dc.send(JSON.stringify({ muted: room.muted, deafened: room.deafened }));
}
const broadcastRoomState = () => room.peers.forEach(sendRoomState);
function applyRoomGain(peer) {
  if (peer.gain) peer.gain.gain.value = room.deafened ? 0 : voiceSettings().volume / 100;
}

function roomToggleMic() {
  if (!room.mic) return;
  room.muted = !room.muted;
  if (!room.muted && room.deafened) { room.deafened = false; room.peers.forEach(applyRoomGain); }
  room.mic.getAudioTracks().forEach((t) => (t.enabled = !room.muted));
  broadcastRoomState();
  callSound.click(!room.muted);
  roomUI();
  renderSocialSoon();
}
function roomToggleDeafen() {
  if (!room.gid) return;
  room.deafened = !room.deafened;
  if (room.deafened) { room.mutedBeforeDeafen = room.muted; room.muted = true; }
  else room.muted = room.mutedBeforeDeafen;
  room.mic?.getAudioTracks().forEach((t) => (t.enabled = !room.muted));
  room.peers.forEach(applyRoomGain);
  broadcastRoomState();
  callSound.click(!room.deafened);
  roomUI();
  renderSocialSoon();
}

// who's talking: our mic + every connected peer
function watchRoomLevels() {
  clearInterval(room.levelTimer);
  if (!room.gid) return;
  actx ??= new AudioContext();
  const meter = (stream) => {
    const an = actx.createAnalyser();
    an.fftSize = 256;
    actx.createMediaStreamSource(stream).connect(an);
    const buf = new Uint8Array(an.frequencyBinCount);
    return () => { an.getByteFrequencyData(buf); return buf.reduce((a, b) => a + b, 0) / buf.length; };
  };
  const self = room.mic ? meter(room.mic) : () => 0;
  const peers = [...room.peers.entries()].filter(([, p]) => p.stream).map(([pk, p]) => [pk, meter(p.stream), p]);
  let selfSpeaking = false;
  room.levelTimer = setInterval(() => {
    const map = {};
    const s = !room.muted && self() > 16;
    map[me.pk] = s;
    let changed = s !== selfSpeaking;
    selfSpeaking = s;
    for (const [pk, m, p] of peers) {
      const sp = !room.deafened && m() > 16;
      if (sp !== p.speaking) changed = true;
      p.speaking = sp;
      map[pk] = sp;
    }
    if (!changed) return;
    for (const [pk, on] of Object.entries(map)) $$(`[data-vkey="${pk}"]`).forEach((el) => el.classList.toggle('speaking', on));
    ipc.overlaySend({ type: 'speaking', map });
  }, 120);
}

// ---------- group ring (someone opened an empty room) ----------
function startGroupRing(g, from) {
  stopGroupRing();
  Object.assign(ringing, { gid: g.id, from });
  callSound.ring();
  ringing.ring = setInterval(() => callSound.ring(), 2400);
  ringing.timer = setTimeout(stopGroupRing, 35e3);
  groupRingUI();
}
function stopGroupRing() {
  clearInterval(ringing.ring);
  clearTimeout(ringing.timer);
  ringing.gid = null;
  groupRingUI();
}
function groupRingOpen() { return !!ringing.gid; }
function acceptGroupRing() { const gid = ringing.gid; stopGroupRing(); if (gid) joinRoom(gid); }
function groupRingHandle(act) {
  if (act === 'ok') return acceptGroupRing();
  if (act === 'back') return stopGroupRing();
}
function groupRingUI() {
  const el = $('#gring');
  const g = groupById(ringing.gid);
  if (!g) {
    el.className = 'call idle';
    el.innerHTML = '';
    if (room.gid || !inCall()) ipc.overlaySend({ type: 'call', state: room.gid ? 'active' : 'idle', ...roomOverlayInfo() });
    return;
  }
  const glyph = (k) => { const x = GLYPHS[currentLayout()][k]; return `<i class="g ${x[1]}">${x[0]}</i>`; };
  const others = inRoom(g.id).filter((pk) => pk !== me.pk);
  el.className = 'call incoming';
  el.style.setProperty('--h', hue(g.id));
  el.innerHTML = `<div class="cs-glow"></div><div class="cs">
    <div class="cs-kicker"><span class="cs-live"></span>Групповой звонок · OTIUM</div>
    <div class="cs-av"><i class="w w1"></i><i class="w w2"></i><i class="w w3"></i>${groupAvatar(g, 'big')}</div>
    <div class="cs-name">${esc(groupTitle(g))}</div>
    <div class="cs-status">${esc(memberName(g, ringing.from))} зовёт в голосовой</div>
    <div class="cs-presence">В голосовом: ${others.map((pk) => esc(memberName(g, pk))).join(', ') || '—'}</div>
    <div class="cs-eq">${'<i></i>'.repeat(16)}</div>
    <div class="cs-actions">
      <button class="cs-btn yes" id="grYes">${CALL_ICONS.phone}<span>Войти</span>${glyph('ok')}</button>
      <button class="cs-btn no" id="grNo">${CALL_ICONS.hang}<span>Не сейчас</span>${glyph('back')}</button>
    </div></div>`;
  $('#grYes').onclick = acceptGroupRing;
  $('#grNo').onclick = stopGroupRing;
  ipc.overlaySend({ type: 'call', state: 'incoming', key: g.id, name: groupTitle(g), kicker: 'Групповой звонок' });
}

// ---------- UI helpers ----------
function groupAvatar(g, cls = '') {
  const h = hue(g.id);
  return `<div class="sav gav ${cls}" style="--c1:hsl(${h} 70% 50%);--c2:hsl(${(h + 70) % 360} 70% 32%)">${esc(groupTitle(g)[0]?.toUpperCase() || '#')}</div>`;
}

function roomParticipants() {
  const g = groupById(room.gid);
  if (!g) return [];
  return [
    { key: me.pk, name: acct().name, muted: room.muted, deafened: room.deafened, self: true, connected: true },
    ...[...room.peers.entries()].map(([pk, p]) => ({ key: pk, name: memberName(g, pk), ...p.state, connected: p.connected }))
  ];
}
function roomOverlayInfo() {
  const g = groupById(room.gid);
  return g ? { key: g.id, name: groupTitle(g), title: groupTitle(g), startedAt: room.joinedAt, participants: roomParticipants() } : {};
}

// small pill at the top while in a room (hidden while that group's chat with the full stage is open)
function roomUI() {
  const el = $('#roomPill');
  const g = groupById(room.gid);
  // a 1:1 call owns the overlay while there's no room
  if (g || !inCall()) ipc.overlaySend({ type: 'call', state: g ? 'active' : 'idle', ...roomOverlayInfo() });
  if (!g || (socialOpen() && social.chat === 'g:' + g.id)) { el.className = 'roompill hidden'; el.innerHTML = ''; return; }
  const ps = roomParticipants();
  el.className = 'roompill';
  el.innerHTML = `<div class="call-pill">
    <button class="rp-open" id="rpOpen">${groupAvatar(g)}<div><div class="t">${esc(groupTitle(g))}</div><div class="d">🔊 ${ps.length} в голосовом</div></div></button>
    ${roomButtons('p')}</div>`;
  $('#rpOpen').onclick = () => openGroupChat(g.id);
  bindRoomButtons(el);
}
function roomButtons(cls) {
  return `<button class="${cls}btn ${room.muted ? 'off' : ''}" data-ra="mute" title="Микрофон">${room.muted ? CALL_ICONS.micOff : CALL_ICONS.mic}</button>
    <button class="${cls}btn ${room.deafened ? 'off' : ''}" data-ra="deafen" title="Выключить звук">${room.deafened ? CALL_ICONS.headOff : CALL_ICONS.head}</button>
    <button class="${cls}btn hang" data-ra="leave" title="Выйти из голосового">${CALL_ICONS.hang}</button>`;
}
function bindRoomButtons(root) {
  root.querySelectorAll('[data-ra]').forEach((b) => (b.onclick = () => roomAction(b.dataset.ra)));
}
function roomAction(a) {
  if (a === 'mute') roomToggleMic();
  else if (a === 'deafen') roomToggleDeafen();
  else if (a === 'leave') leaveRoom();
  else if (a === 'join' && social.chat?.startsWith('g:')) joinRoom(social.chat.slice(2));
}

function openGroupChat(gid) {
  if (modal.open) closeModal();
  if (cc.open) closeCC();
  if (!socialOpen()) openSocial();
  social.idx = listItems().findIndex((x) => x.gid === gid);
  activateItem();
}

// right-hand side of the friends panel for a group
function groupChatHTML(g) {
  const live = inRoom(g.id);
  const joined = room.gid === g.id;
  const tile = (p) => `<div class="vtile ${p.connected ? '' : 'pending'}" data-vkey="${p.key}">
      <div class="vav">${avatar(p.key, p.name, 'big')}</div>
      <div class="vname">${esc(p.name)}${p.deafened ? `<span class="vic">${CALL_ICONS.headOff}</span>` : p.muted ? `<span class="vic">${CALL_ICONS.micOff}</span>` : ''}</div>
      ${p.connected ? '' : '<div class="vpending">соединяем…</div>'}</div>`;
  const stage = joined ? `<div class="vstage">
      <div class="vstage-top"><span class="vdot"></span>Голосовой · <b id="cTimer3">${fmtLeft(Date.now() - room.joinedAt)}</b></div>
      <div class="vtiles">${roomParticipants().map(tile).join('')}</div>
      <div class="vdock">${roomButtons('v')}</div></div>`
    : live.length ? `<div class="vlive">🔊 В голосовом: ${live.map((pk) => esc(memberName(g, pk))).join(', ')}</div>` : '';
  const glyph = GLYPHS[currentLayout()].menu;
  return `
    <div class="schat-head">
      <div class="hav">${groupAvatar(g)}</div>
      <div class="hname"><div class="t">${esc(groupTitle(g))}</div><div class="d gmembers">${g.members.map((m) => esc(m.pk === me.pk ? 'вы' : m.name)).join(' · ')}</div></div>
      <button class="hbtn" id="gMenu" title="Группа">${CALL_ICONS.gear}</button>
      ${joined ? '' : `<button class="callpill" id="gJoin">${CALL_ICONS.phone}<span>${live.length ? 'Присоединиться' : 'Голосовой'}</span><i class="g ${glyph[1]}">${glyph[0]}</i></button>`}
    </div>
    ${stage}
    <div class="smsgs" id="smsgs">${g.history.length ? messageGroups(g.history, (pk) => memberName(g, pk)) : `<div class="schat-hello">${groupAvatar(g, 'big')}<div class="t">${esc(groupTitle(g))}</div><div class="d">Это начало группы. Позовите всех в голосовой 🎧</div></div>`}</div>`;
}
function bindGroupChat(g) {
  if ($('#gJoin')) $('#gJoin').onclick = () => joinRoom(g.id);
  $('#gMenu').onclick = () => openGroupMenu(g);
  if ($('.vstage')) bindRoomButtons($('.vstage'));
}
setInterval(() => {
  const t = document.getElementById('cTimer3');
  if (t && room.gid) t.textContent = fmtLeft(Date.now() - room.joinedAt);
}, 1000);

// ---------- modals ----------
function friendPicker(title, exclude, max, onDone, back) {
  const friends = acct().friends.filter((f) => !exclude.includes(f.pk));
  const picked = new Set();
  openModal(`<h2>${esc(title)}</h2>
    ${title.startsWith('Новая') ? '<div class="field"><label>Название группы</label><input class="focusable" id="gName" maxlength="32" placeholder="например: Катка"/></div>' : ''}
    <div class="steam-list" id="gPick">${friends.length ? friends.map((f) => `
      <button class="steam-item focusable" data-pk="${f.pk}">${avatar(f.pk, f.name)}<span><b>${esc(f.name)}</b><br><small>${esc(presenceOf(f.pk).text)}</small></span><span class="chk"></span></button>`).join('')
      : '<div class="meta"><span>Нет друзей, которых можно добавить</span></div>'}</div>
    <div class="buttons"><button class="btn focusable" id="gNo">Отмена</button><button class="btn primary focusable" id="gOk">Готово (<span id="gCnt">0</span>)</button></div>`, { onBack: back || closeModal });
  $$('#gPick .steam-item').forEach((b) => (b.onclick = () => {
    const pk = b.dataset.pk;
    if (picked.has(pk)) picked.delete(pk);
    else if (picked.size < max) picked.add(pk);
    else return toast(`В группе может быть до ${MAX_GROUP} человек`, true);
    b.classList.toggle('on', picked.has(pk));
    $('#gCnt').textContent = picked.size;
  }));
  $('#gNo').onclick = back || closeModal;
  $('#gOk').onclick = () => {
    if (!picked.size) return toast('Выберите хотя бы одного друга', true);
    onDone([...picked], $('#gName')?.value || '');
  };
}

function openCreateGroup() {
  friendPicker('Новая группа', [], MAX_GROUP - 1, (pks, name) => {
    const g = createGroup(name, pks);
    closeModal();
    toast(`Группа «${groupTitle(g)}» создана`, false, { title: 'Группы', icon: '👥' });
    openGroupChat(g.id);
  });
}

function openGroupMenu(g) {
  const items = [
    room.gid === g.id ? ['leave-voice', 'Выйти из голосового', ''] : ['join', 'Войти в голосовой', ''],
    ...(g.members.length < MAX_GROUP ? [['invite', 'Добавить друзей', `${g.members.length}/${MAX_GROUP}`]] : []),
    ['rename', 'Переименовать', ''],
    ['leave', 'Покинуть группу', '', 'danger']
  ];
  openModal(`<h2 style="padding:10px 18px 0">${esc(groupTitle(g))}</h2>` + items.map(([id, label, val, cls]) =>
    `<button class="menu-item focusable ${cls || ''}" data-id="${id}">${esc(label)}<span class="val">${esc(val)}</span></button>`).join(''), { cls: 'menu' });
  $$('#modal .menu-item').forEach((b) => (b.onclick = () => {
    const id = b.dataset.id;
    if (id === 'join') { closeModal(); joinRoom(g.id); }
    if (id === 'leave-voice') { closeModal(); leaveRoom(); }
    if (id === 'invite') friendPicker('Добавить в группу', g.members.map((m) => m.pk), MAX_GROUP - g.members.length, (pks) => { inviteToGroup(g, pks); closeModal(); toast('Друзья добавлены в группу'); }, () => openGroupMenu(g));
    if (id === 'rename') {
      openModal(`<h2>Название группы</h2><div class="field"><input class="focusable" id="gRen" maxlength="32" value="${esc(g.name || '')}"/></div>
        <div class="buttons"><button class="btn focusable" id="rNo">Отмена</button><button class="btn primary focusable" id="rYes">Сохранить</button></div>`, { onBack: () => openGroupMenu(g) });
      $('#rNo').onclick = () => openGroupMenu(g);
      $('#rYes').onclick = () => { g.name = $('#gRen').value.trim(); inviteToGroup(g, []); closeModal(); };
    }
    if (id === 'leave') {
      openModal(`<h2>Покинуть «${esc(groupTitle(g))}»?</h2><div class="meta"><span>Вернуться получится, только если вас снова добавят.</span></div>
        <div class="buttons"><button class="btn focusable" id="lNo">Отмена</button><button class="btn danger focusable" id="lYes">Покинуть</button></div>`, { onBack: () => openGroupMenu(g) });
      $('#lNo').onclick = () => openGroupMenu(g);
      $('#lYes').onclick = () => { closeModal(); leaveGroup(g); toast('Вы покинули группу'); };
    }
  }));
}

// "Add someone" during a 1:1 call: the call becomes a group voice room with all three
function openAddToCall() {
  if (!inCall() || call.state !== 'active') return;
  const other = call.pk;
  friendPicker('Добавить в звонок', [other], MAX_GROUP - 2, async (pks) => {
    closeModal();
    const g = createGroup('', [other, ...pks]);
    sendCall(other, 'move', { id: call.id, gid: g.id });
    endLocal();
    await joinRoom(g.id);
    toast('Звонок перенесён в группу — друзьям пришло приглашение', false, { title: groupTitle(g), icon: '👥' });
  }, () => openCallMenu());
}
// the other side of the move: wait for the group invite to arrive, then join
async function followMove(gid) {
  for (let i = 0; i < 20 && !groupById(gid); i++) await sleep(250);
  if (groupById(gid)) joinRoom(gid);
}
