'use strict';
// Launcher accounts, friends and chat over Nostr — no server of our own.
// An account is a keypair generated here. Messages are NIP-04 encrypted DMs kept by public relays,
// so they reach a friend even if they were offline. Profiles carry a "ps5-launcher" tag so players
// can find each other by nickname; the friends list is stored as a contact list (kind 3) so it
// follows the account to another PC.

const N = window.NostrTools;
const RELAYS = ['wss://relay.damus.io', 'wss://nos.lol', 'wss://relay.primal.net'];
const APP_TAG = 'ps5-launcher';
const ONLINE_MS = 6 * 60e3;
const CTRL = 'ps5l';

let pool = null;
let me = null;               // { sk: Uint8Array, pk: hex }
let subs = [];
let friendSub = null;
let presenceTimer = null;
let friendsFeedTimer = null;
const social = { open: false, zone: 'list', idx: 0, chat: null, chatFocus: 0, presence: {}, profiles: {}, seen: new Set() };

const now = () => Math.floor(Date.now() / 1000);
const acct = () => data.social;
const friendPks = () => (acct()?.friends || []).map((f) => f.pk);
const isFriend = (pk) => friendPks().includes(pk);
const nameOf = (pk) => acct()?.friends.find((f) => f.pk === pk)?.name || social.profiles[pk]?.name || acct()?.requests.find((r) => r.pk === pk)?.name || 'Игрок ' + pk.slice(0, 6);
const npub = (pk) => N.nip19.npubEncode(pk);
const shortCode = (pk) => npub(pk).slice(-6).toUpperCase();

function socialOpen() { return social.open; }

// ---------- connection ----------
function socialStart() {
  const a = acct();
  if (!a?.sk) return;
  const sk = N.utils.hexToBytes(a.sk);
  me = { sk, pk: N.getPublicKey(sk) };
  pool ||= new N.SimplePool();
  a.history ||= {};
  for (const msgs of Object.values(a.history)) msgs.forEach((m) => social.seen.add(m.id));
  subs.forEach((s) => s.close());
  const since = now() - 30 * 86400;
  subs = [
    pool.subscribe(RELAYS, { kinds: [4], '#p': [me.pk], since }, { onevent: onDM }),
    pool.subscribe(RELAYS, { kinds: [4], authors: [me.pk], since }, { onevent: onDM })
  ];
  subscribeFriends();
  publishPresence();
  publishLibrarySoon();
  clearInterval(presenceTimer);
  presenceTimer = setInterval(publishPresence, 2 * 60e3);
  updateDot();
}

function subscribeFriends() {
  friendSub?.close();
  const pks = [...friendPks(), ...(acct()?.requests || []).map((r) => r.pk)];
  if (!pks.length) return;
  friendSub = pool.subscribe(RELAYS, { kinds: [0, 30315], authors: pks }, {
    onevent: (ev) => {
      if (ev.kind === 0) {
        try {
          const p = JSON.parse(ev.content);
          const cur = social.profiles[ev.pubkey];
          if (!cur || cur.at < ev.created_at) social.profiles[ev.pubkey] = { name: p.name || p.display_name, at: ev.created_at };
          const f = acct().friends.find((x) => x.pk === ev.pubkey);
          if (f && p.name && f.name !== p.name) { f.name = p.name; saveSoon(); }
        } catch {}
      } else {
        const cur = social.presence[ev.pubkey];
        if (!cur || cur.at < ev.created_at * 1000) social.presence[ev.pubkey] = { text: ev.content, at: ev.created_at * 1000 };
        // "what friends are playing" lives in the feed too
        clearTimeout(friendsFeedTimer);
        friendsFeedTimer = setTimeout(refreshFriends, 1500);
      }
      renderSocialSoon();
    }
  });
}

async function publish(template) {
  if (!me) return false;
  const ev = N.finalizeEvent({ created_at: now(), tags: [], ...template }, me.sk);
  try {
    await Promise.any(pool.publish(RELAYS, ev));
    return ev;
  } catch {
    return false;
  }
}

function publishProfile() {
  return publish({ kind: 0, tags: [['t', APP_TAG]], content: JSON.stringify({ name: acct().name, display_name: acct().name, about: 'OTIUM' }) });
}
function publishContacts() {
  return publish({ kind: 3, tags: acct().friends.map((f) => ['p', f.pk, '', f.name]), content: '' });
}
function myStatus() {
  const g = data.games.find((x) => running.has(x.id));
  return g ? `В игре: ${g.name}` : 'В сети';
}
function publishPresence() {
  if (!me) return;
  publish({ kind: 30315, tags: [['d', 'general'], ['expiration', String(now() + 600)]], content: myStatus() });
}

// Friends can open our profile: share the game list (names, Steam ids, playtime) and the latest trophy
function publishLibrary() {
  if (!me) return;
  const games = data.games
    .filter((g) => g.category !== 'app')
    .sort((a, b) => lastPlayedOf(b) - lastPlayedOf(a))
    .slice(0, 40)
    .map((g) => ({ n: g.name, s: steamIdOf(g) || undefined, p: Math.round(totalPlaytime(g)) || undefined, l: lastPlayedOf(g) || undefined }));
  publish({ kind: 30078, tags: [['d', 'ps5l-library']], content: JSON.stringify({ games, since: data.settings.since, trophy: data.lastTrophy || null }) });
}
let libTimer;
function publishLibrarySoon() {
  clearTimeout(libTimer);
  libTimer = setTimeout(publishLibrary, 4000);
}
async function fetchLibrary(pk) {
  if (!pool) return null;
  try {
    const ev = await pool.get(RELAYS, { kinds: [30078], authors: [pk], '#d': ['ps5l-library'] });
    return ev ? JSON.parse(ev.content) : null;
  } catch {
    return null;
  }
}

function presenceOf(pk) {
  const p = social.presence[pk];
  if (p && Date.now() - p.at < ONLINE_MS) return { online: true, text: p.text || 'В сети', game: /^В игре/.test(p.text || '') };
  return { online: false, text: 'Не в сети' };
}

// ---------- direct messages ----------
async function sendDM(pk, text) {
  const content = await N.nip04.encrypt(me.sk, pk, text);
  const ev = N.finalizeEvent({ kind: 4, created_at: now(), tags: [['p', pk]], content }, me.sk);
  await onDM(ev);
  try {
    await Promise.any(pool.publish(RELAYS, ev));
    return true;
  } catch {
    toast('Сообщение не отправлено — нет связи', true);
    return false;
  }
}
const sendCtrl = (pk, t) => sendDM(pk, JSON.stringify({ pl: CTRL, t, n: acct().name }));

async function onDM(ev) {
  if (social.seen.has(ev.id)) return;
  social.seen.add(ev.id);
  const mine = ev.pubkey === me.pk;
  const other = mine ? ev.tags.find((t) => t[0] === 'p')?.[1] : ev.pubkey;
  if (!other || other === me.pk) return;
  let text;
  try { text = await N.nip04.decrypt(me.sk, other, ev.content); } catch { return; }
  const a = acct();

  let ctrl = null;
  if (text.startsWith('{"pl":"' + CTRL)) { try { ctrl = JSON.parse(text); } catch {} }
  if (ctrl) {
    // voice call signaling
    if (['call', 'ans', 'decl', 'busy', 'end', 'move'].includes(ctrl.t)) {
      if (!mine) onCallSignal(other, ctrl, ev.created_at);
      return;
    }
    // groups: chat, membership and the voice room
    if (GROUP_CTRL.includes(ctrl.t)) {
      if (!mine) onGroupCtrl(other, ctrl, ev.created_at);
      return;
    }
    if (ctrl.t === 'fr' && !mine) {
      if (isFriend(other)) return;
      // both sides asked — that's a yes
      if (a.outgoing.includes(other)) return addFriend(other, ctrl.n, true);
      if (!a.requests.some((r) => r.pk === other)) {
        a.requests.push({ pk: other, name: ctrl.n || nameOf(other), at: ev.created_at * 1000 });
        if (Date.now() - ev.created_at * 1000 < 10 * 60e3) toast('хочет добавить вас в друзья', false, { title: ctrl.n || 'Игрок', avatar: { name: ctrl.n, key: other } });
        saveSoon();
        subscribeFriends();
      }
    } else if (ctrl.t === 'fr' && mine) {
      if (!a.outgoing.includes(other) && !isFriend(other)) { a.outgoing.push(other); saveSoon(); }
    } else if (ctrl.t === 'fa') {
      if (!mine && a.outgoing.includes(other)) {
        addFriend(other, ctrl.n, false);
        if (Date.now() - ev.created_at * 1000 < 10 * 60e3) toast('теперь у вас в друзьях', false, { title: ctrl.n || 'Игрок', avatar: { name: ctrl.n, key: other } });
      } else if (mine && !isFriend(other)) addFriend(other, a.requests.find((r) => r.pk === other)?.name, false);
    } else if (ctrl.t === 'rm' && !mine && isFriend(other)) {
      removeFriend(other, false);
    }
    renderSocialSoon();
    return;
  }

  if (!isFriend(other)) return;
  const list = (a.history[other] ||= []);
  list.push({ id: ev.id, from: ev.pubkey, text, at: ev.created_at * 1000 });
  list.sort((x, y) => x.at - y.at);
  if (list.length > 300) list.splice(0, list.length - 300);
  saveSoon();
  const viewing = social.open && social.chat === other && !document.hidden;
  if (viewing) markRead(other);
  else if (!mine && Date.now() - ev.created_at * 1000 < 5 * 60e3) {
    sfx.ok();
    toast(text.slice(0, 160), false, { title: nameOf(other), avatar: { name: nameOf(other), key: other } });
  }
  updateDot();
  renderSocialSoon();
}

function addFriend(pk, name, sendAccept) {
  const a = acct();
  if (!isFriend(pk)) a.friends.push({ pk, name: name || nameOf(pk) });
  a.requests = a.requests.filter((r) => r.pk !== pk);
  a.outgoing = a.outgoing.filter((x) => x !== pk);
  if (sendAccept) sendCtrl(pk, 'fa');
  saveSoon();
  publishContacts();
  subscribeFriends();
  renderSocialSoon();
}

function removeFriend(pk, notify = true) {
  const a = acct();
  a.friends = a.friends.filter((f) => f.pk !== pk);
  delete a.history[pk];
  if (notify) sendCtrl(pk, 'rm');
  if (social.chat === pk) { social.chat = null; social.zone = 'list'; }
  saveSoon();
  publishContacts();
  subscribeFriends();
  renderSocial();
}

function markRead(pk) {
  acct().lastRead[pk] = Date.now();
  saveSoon();
  updateDot();
}
function unreadOf(pk) {
  const last = acct()?.lastRead?.[pk] || 0;
  return (acct()?.history?.[pk] || []).filter((m) => m.from !== me?.pk && m.at > last).length;
}
function socialUnread() {
  if (!acct()?.sk) return 0;
  return friendPks().reduce((n, pk) => n + unreadOf(pk), 0) + acct().requests.length + groups().reduce((n, g) => n + groupUnread(g), 0);
}
function updateDot() {
  $('#friendsDot')?.classList.toggle('hidden', !socialUnread());
}

let saveTimer;
function saveSoon() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(save, 800);
}

// ---------- account ----------
async function createAccount(name) {
  const sk = N.generateSecretKey();
  data.social = { sk: N.utils.bytesToHex(sk), name, friends: [], requests: [], outgoing: [], lastRead: {}, history: {} };
  await save();
  socialStart();
  publishProfile();
  publishContacts();
  toast(`Аккаунт «${name}» создан`);
  renderSocial();
}

async function loginWithKey(str) {
  let hex = str.trim();
  try {
    if (hex.startsWith('nsec')) hex = N.utils.bytesToHex(N.nip19.decode(hex).data);
    if (!/^[0-9a-f]{64}$/i.test(hex)) throw new Error();
  } catch {
    return toast('Это не похоже на ключ аккаунта', true);
  }
  const pk = N.getPublicKey(N.utils.hexToBytes(hex));
  pool ||= new N.SimplePool();
  toast('Входим…');
  const [prof, contacts] = await Promise.all([
    pool.get(RELAYS, { kinds: [0], authors: [pk] }).catch(() => null),
    pool.get(RELAYS, { kinds: [3], authors: [pk] }).catch(() => null)
  ]);
  let name = 'Игрок';
  try { name = JSON.parse(prof?.content || '{}').name || name; } catch {}
  const friends = (contacts?.tags || []).filter((t) => t[0] === 'p').map((t) => ({ pk: t[1], name: t[3] || 'Игрок ' + t[1].slice(0, 6) }));
  data.social = { sk: hex.toLowerCase(), name, friends, requests: [], outgoing: [], lastRead: {}, history: {} };
  await save();
  socialStart();
  toast(`С возвращением, ${name}`);
  renderSocial();
}

// ---------- panel ----------
function openSocial() {
  social.open = true;
  social.zone = 'list';
  social.idx = 0;
  $('#social').classList.remove('hidden');
  requestAnimationFrame(() => $('#social').classList.add('show'));
  renderSocial();
}
function closeSocial() {
  social.open = false;
  setTimeout(() => { roomUI(); if (inCall()) callUI(); }, 0);
  $('#social').classList.remove('show');
  setTimeout(() => !social.open && $('#social').classList.add('hidden'), 300);
  document.activeElement?.blur();
}

let renderQueued = false;
function renderSocialSoon() {
  updateDot();
  if (!social.open || renderQueued) return;
  renderQueued = true;
  requestAnimationFrame(() => { renderQueued = false; renderSocial(); });
}

function avatar(pk, name, cls = '') {
  const h = hue(pk || name);
  return `<div class="sav ${cls}" style="--c1:hsl(${h} 65% 45%);--c2:hsl(${(h + 60) % 360} 65% 32%)">${esc((name || '?')[0].toUpperCase())}</div>`;
}

function listItems() {
  const a = acct();
  if (!a?.sk) return [];
  const items = [{ k: 'me' }, { k: 'find' }, { k: 'newgroup' }];
  a.requests.forEach((r) => items.push({ k: 'req', pk: r.pk }));
  // groups with someone in voice or unread messages first
  const lastAt = (g) => Math.max(g.created || 0, g.history.at(-1)?.at || 0);
  [...groups()].sort((x, y) => inRoom(y.id).length - inRoom(x.id).length || groupUnread(y) - groupUnread(x) || lastAt(y) - lastAt(x))
    .forEach((g) => items.push({ k: 'group', gid: g.id }));
  const fr = [...a.friends].sort((x, y) => presenceOf(y.pk).online - presenceOf(x.pk).online || unreadOf(y.pk) - unreadOf(x.pk) || x.name.localeCompare(y.name));
  fr.forEach((f) => items.push({ k: 'friend', pk: f.pk }));
  return items;
}

function renderSocial() {
  if (!social.open) return;
  const a = acct();
  const el = $('#social');
  if (!a?.sk) return renderWelcome(el);

  const items = listItems();
  social.idx = Math.min(social.idx, items.length - 1);
  const itemHTML = (it, i) => {
    const f = social.zone === 'list' && i === social.idx ? ' focus' : '';
    const active = it.pk && it.pk === social.chat ? ' active' : '';
    if (it.k === 'me') return `<div class="sitem me${f}" data-i="${i}">${avatar(me.pk, a.name, 'big')}<div><div class="t">${esc(a.name)}</div><div class="d">Мой профиль · код ${shortCode(me.pk)}</div></div></div>`;
    if (it.k === 'find') return `<div class="sitem find${f}" data-i="${i}"><div class="sav plus">+</div><div><div class="t">Найти друзей</div><div class="d">по нику или коду</div></div></div>`;
    if (it.k === 'newgroup') return `<div class="sitem find${f}" data-i="${i}"><div class="sav plus">👥</div><div><div class="t">Создать группу</div><div class="d">чат и голосовой для 3–5 друзей</div></div></div>`;
    if (it.k === 'group') {
      const g = groupById(it.gid);
      const live = inRoom(g.id);
      const u = groupUnread(g);
      const act = social.chat === 'g:' + g.id ? ' active' : '';
      return `<div class="sitem${f}${act}" data-i="${i}">${groupAvatar(g)}
        <div class="sbody"><div class="t">${esc(groupTitle(g))}</div><div class="d ${live.length ? 'game' : ''}">${live.length ? `🔊 в голосовом: ${live.length}` : `${g.members.length} ${plural(g.members.length, 'участник', 'участника', 'участников')}`}</div></div>${u ? `<b class="badge">${u}</b>` : ''}</div>`;
    }
    if (it.k === 'req') {
      const r = a.requests.find((x) => x.pk === it.pk);
      return `<div class="sitem req${f}" data-i="${i}">${avatar(it.pk, r.name)}<div><div class="t">${esc(r.name)}</div><div class="d accent">Заявка в друзья</div></div></div>`;
    }
    const p = presenceOf(it.pk);
    const u = unreadOf(it.pk);
    return `<div class="sitem${f}${active}" data-i="${i}">${avatar(it.pk, nameOf(it.pk))}<i class="pres ${p.online ? (p.game ? 'game' : 'on') : ''}"></i>
      <div class="sbody"><div class="t">${esc(nameOf(it.pk))}</div><div class="d ${p.game ? 'game' : ''}">${esc(p.text)}</div></div>${u ? `<b class="badge">${u}</b>` : ''}</div>`;
  };
  const firstFriend = items.findIndex((x) => x.k === 'friend');
  const firstReq = items.findIndex((x) => x.k === 'req');
  const firstGroup = items.findIndex((x) => x.k === 'group');
  const listHTML = items.map((it, i) => (i === firstReq ? '<div class="shead">Заявки</div>' : '') + (i === firstGroup ? '<div class="shead">Группы</div>' : '') + (i === firstFriend ? `<div class="shead">Друзья · ${a.friends.filter((f) => presenceOf(f.pk).online).length} в сети</div>` : '') + itemHTML(it, i)).join('')
    + (a.friends.length ? '' : '<div class="sempty">Пока нет друзей — найдите их по нику</div>');

  let right = `<div class="schat-empty">${CC_ICONS.chat}<div>Выберите друга, чтобы начать чат</div></div>`;
  const chatGroup = social.chat?.startsWith('g:') ? groupById(social.chat.slice(2)) : null;
  if (social.chat?.startsWith('g:') && !chatGroup) social.chat = null;
  if (chatGroup) {
    right = groupChatHTML(chatGroup) + `
      <div class="emoji-tray hidden" id="emojiTray">${QUICK_EMOJI.map((e) => `<button>${e}</button>`).join('')}</div>
      <div class="sinput">
        <div class="sfield ${social.zone === 'chat' && social.chatFocus === 0 ? 'focus' : ''}">
          <button class="emo-btn" id="sEmoji" title="Эмодзи">😊</button>
          <input id="sInput" placeholder="Сообщение в «${esc(groupTitle(chatGroup))}»" maxlength="1000" value="${esc(social.draft || '')}"/>
        </div>
        <button class="sendbtn ${social.zone === 'chat' && social.chatFocus === 1 ? 'focus' : ''}" id="sSend" title="Отправить">${SEND_ICON}</button>
      </div>`;
  } else if (social.chat) {
    const pk = social.chat;
    const p = presenceOf(pk);
    const msgs = a.history[pk] || [];
    const inThisCall = inCall() && call.pk === pk;
    right = `
      <div class="schat-head">
        <div class="hav">${avatar(pk, nameOf(pk))}<i class="pres ${p.online ? (p.game ? 'game' : 'on') : ''}"></i></div>
        <div class="hname"><div class="t">${esc(nameOf(pk))}</div><div class="d ${p.game ? 'game' : ''}">${esc(p.text)}</div></div>
        ${inThisCall ? '' : `<button class="callpill" id="sCall">${CALL_ICONS.phone}<span>Позвонить</span><i class="g ${GLYPHS[currentLayout()].menu[1]}">${GLYPHS[currentLayout()].menu[0]}</i></button>`}
      </div>
      ${callStageHTML(pk)}
      <div class="smsgs" id="smsgs">${msgs.length ? messageGroups(msgs) : `<div class="schat-hello">${avatar(pk, nameOf(pk), 'big')}<div class="t">${esc(nameOf(pk))}</div><div class="d">Это начало вашей переписки. Скажите привет 👋</div></div>`}</div>
      <div class="emoji-tray hidden" id="emojiTray">${QUICK_EMOJI.map((e) => `<button>${e}</button>`).join('')}</div>
      <div class="sinput">
        <div class="sfield ${social.zone === 'chat' && social.chatFocus === 0 ? 'focus' : ''}">
          <button class="emo-btn" id="sEmoji" title="Эмодзи">😊</button>
          <input id="sInput" placeholder="Сообщение для ${esc(nameOf(pk))}" maxlength="1000" value="${esc(social.draft || '')}"/>
        </div>
        <button class="sendbtn ${social.zone === 'chat' && social.chatFocus === 1 ? 'focus' : ''}" id="sSend" title="Отправить">${SEND_ICON}</button>
      </div>`;
  }

  el.innerHTML = `
    <div class="soc-top"><div class="soc-title">Друзья и сообщения</div><div class="d">${oskHint('back', 'назад')}</div></div>
    <div class="soc-body"><div class="soc-left" id="socLeft">${listHTML}</div><div class="soc-right">${right}</div></div>`;

  $$('#socLeft .sitem').forEach((n) => (n.onclick = () => { social.zone = 'list'; social.idx = +n.dataset.i; activateItem(); }));
  const m = $('#smsgs');
  if (m) m.scrollTop = m.scrollHeight;
  const inp = $('#sInput');
  if (inp) {
    inp.oninput = () => (social.draft = inp.value);
    inp.onfocus = () => { social.zone = 'chat'; social.chatFocus = 0; };
    $('#sSend').onclick = sendCurrent;
    if (chatGroup) bindGroupChat(chatGroup);
    else {
      if ($('#sCall')) $('#sCall').onclick = () => startCall(social.chat);
      if ($('.vstage')) bindCallButtons($('.vstage'));
    }
    $('#sEmoji').onclick = () => $('#emojiTray').classList.toggle('hidden');
    $$('#emojiTray button').forEach((b) => (b.onclick = () => {
      inp.value += b.textContent;
      social.draft = inp.value;
      inp.focus();
    }));
    tickTimer();
    if (social.zone === 'chat' && social.chatFocus === 0 && document.activeElement !== inp && !osk.open) inp.focus();
  }
  $('#socLeft .sitem.focus')?.scrollIntoView({ block: 'nearest' });
}

// Discord-style: consecutive messages from one person within a few minutes share one header
const QUICK_EMOJI = ['😂', '🔥', '👍', '❤️', '😎', '🎮', '🏆', '😭', '🤝', '💀', '🙏', '👀'];
const SEND_ICON = '<svg viewBox="0 0 24 24"><path d="M3.4 20.4 20.9 12 3.4 3.6 3.4 10l12.5 2-12.5 2z"/></svg>';
function messageGroups(msgs, nameFn) {
  const out = [];
  let g = null;
  for (const m of msgs) {
    const day = new Date(m.at).toDateString();
    if (!g || g.from !== m.from || g.day !== day || m.at - g.last > 5 * 60e3) {
      if (!g || g.day !== day) out.push(`<div class="sday"><span>${fmtDate(m.at).replace(/, \d\d:\d\d$/, '')}</span></div>`);
      g = { from: m.from, day, last: m.at, lines: [] };
      out.push(g);
    }
    g.lines.push(m);
    g.last = m.at;
  }
  const time = (t) => new Date(t).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
  const jumbo = (t) => /^(\p{Extended_Pictographic}|\uFE0F|\u200D|\s){1,12}$/u.test(t) && t.trim().length <= 12;
  return out.map((g) => {
    if (typeof g === 'string') return g;
    const mine = g.from === me.pk;
    const name = mine ? acct().name : nameFn ? nameFn(g.from) : nameOf(g.from);
    return `<div class="mgroup ${mine ? 'mine' : ''}">
      <div class="mav">${avatar(g.from, name)}</div>
      <div class="mcol"><div class="mhead"><b>${esc(name)}</b><span>${time(g.lines[0].at)}</span></div>
        ${g.lines.map((l) => `<div class="mline ${jumbo(l.text) ? 'jumbo' : ''}"><span class="mt">${time(l.at)}</span>${esc(l.text)}</div>`).join('')}
      </div></div>`;
  }).join('');
}

function renderWelcome(el) {
  el.innerHTML = `
    <div class="soc-top"><div class="soc-title">Друзья и сообщения</div><div class="d">${oskHint('back', 'назад')}</div></div>
    <div class="soc-welcome">
      ${CC_ICONS.chat}
      <h2>Аккаунт OTIUM</h2>
      <div class="d">Создайте аккаунт, чтобы добавлять друзей и переписываться прямо в лаунчере.<br>Регистрация не нужна — аккаунт создаётся на этом компьютере.</div>
      <input id="wName" class="${social.idx === 0 ? 'focus' : ''}" placeholder="Ваш ник" maxlength="24" value="${esc(social.draft || data.settings.user || '')}"/>
      <button class="btn primary ${social.idx === 1 ? 'focus' : ''}" id="wCreate">Создать аккаунт</button>
      <button class="btn ${social.idx === 2 ? 'focus' : ''}" id="wLogin">У меня есть ключ входа</button>
    </div>`;
  const inp = $('#wName');
  inp.oninput = () => (social.draft = inp.value);
  $('#wCreate').onclick = () => {
    const name = inp.value.trim();
    if (name.length < 2) return toast('Ник должен быть не короче 2 символов', true);
    social.draft = '';
    createAccount(name);
  };
  $('#wLogin').onclick = openLoginModal;
  if (social.idx === 0 && !osk.open) inp.focus();
}

function activateItem() {
  const it = listItems()[social.idx];
  if (!it) return;
  if (it.k === 'me') return openProfileModal();
  if (it.k === 'find') return openFindModal();
  if (it.k === 'req') return openRequestModal(it.pk);
  if (it.k === 'newgroup') return openCreateGroup();
  social.chat = it.k === 'group' ? 'g:' + it.gid : it.pk;
  social.zone = 'chat';
  social.chatFocus = 0;
  social.draft = '';
  if (it.k === 'group') markGroupRead(groupById(it.gid));
  else markRead(it.pk);
  renderSocial();
  roomUI();
}

async function sendCurrent() {
  const inp = $('#sInput');
  const text = (inp?.value || '').trim();
  if (!text || !social.chat) return;
  social.draft = '';
  inp.value = '';
  sfx.ok();
  if (social.chat.startsWith('g:')) return sendGroupMessage(groupById(social.chat.slice(2)), text);
  await sendDM(social.chat, text);
  markRead(social.chat);
  renderSocial();
}

function socialHandle(act, src) {
  const a = acct();
  if (!a?.sk) {
    if (act === 'back') { sfx.back(); return closeSocial(); }
    if (act === 'up' && social.idx > 0) { social.idx--; sfx.move(); }
    else if (act === 'down' && social.idx < 2) { social.idx++; sfx.move(); }
    else if (act === 'ok') {
      if (social.idx === 0) {
        if (src === 'pad') return openOSK($('#wName'), { onDone: () => { social.idx = 1; renderSocial(); } });
        social.idx = 1;
      } else return (social.idx === 1 ? $('#wCreate') : $('#wLogin')).click();
    }
    if (social.idx !== 0) document.activeElement?.blur();
    return renderSocial();
  }

  if (social.zone === 'list') {
    const n = listItems().length;
    if (act === 'back') { sfx.back(); return closeSocial(); }
    if (act === 'up' && social.idx > 0) { social.idx--; sfx.move(); }
    else if (act === 'down' && social.idx < n - 1) { social.idx++; sfx.move(); }
    else if (act === 'right' && social.chat) { social.zone = 'chat'; social.chatFocus = 0; sfx.move(); }
    else if (act === 'ok') { sfx.ok(); return activateItem(); }
    else if (act === 'menu') {
      const it = listItems()[social.idx];
      if (it?.k === 'friend') return openFriendMenu(it.pk);
      if (it?.k === 'group') return openGroupMenu(groupById(it.gid));
    }
    return renderSocial();
  }

  // chat
  const m = $('#smsgs');
  if (act === 'back') { sfx.back(); social.zone = 'list'; document.activeElement?.blur(); return renderSocial(); }
  if (act === 'menu') {
    if (social.chat.startsWith('g:')) {
      const gid = social.chat.slice(2);
      return room.gid === gid ? openGroupMenu(groupById(gid)) : joinRoom(gid);
    }
    return inCall() && call.pk === social.chat ? openCallMenu() : startCall(social.chat);
  }
  if (act === 'up' || act === 'down') { m?.scrollBy({ top: act === 'down' ? 140 : -140, behavior: 'smooth' }); return; }
  if (act === 'left') {
    if (social.chatFocus === 1) social.chatFocus = 0;
    else { social.zone = 'list'; document.activeElement?.blur(); }
    sfx.move();
    return renderSocial();
  }
  if (act === 'right' && social.chatFocus === 0) { social.chatFocus = 1; sfx.move(); document.activeElement?.blur(); return renderSocial(); }
  if (act === 'ok') {
    if (social.chatFocus === 1 || src === 'key') return sendCurrent();
    return openOSK($('#sInput'), { onDone: sendCurrent });
  }
}

// ---------- modals ----------
function openProfileModal() {
  const a = acct();
  openModal(`<h2>Мой профиль</h2>
    <div class="sprof">${avatar(me.pk, a.name, 'big')}<div><div class="t">${esc(a.name)}</div><div class="d">Код друга: <b>${shortCode(me.pk)}</b></div></div></div>
    <div class="field"><label>Полный ID (для поиска по коду)</label><input readonly value="${esc(npub(me.pk))}" class="focusable" id="fNpub"/></div>
    <div class="menu-sep"></div>
    <button class="menu-item focusable" id="pRename">Сменить ник</button>
    <button class="menu-item focusable" id="pKey">Ключ для входа на другом ПК</button>
    <button class="menu-item focusable danger" id="pOut">Выйти из аккаунта</button>
    <div class="buttons"><button class="btn focusable" id="bOk">Закрыть</button></div>`, { focus: 1 });
  $('#bOk').onclick = closeModal;
  $('#pRename').onclick = () => {
    openModal(`<h2>Сменить ник</h2><div class="field"><input class="focusable" id="fNick" maxlength="24" value="${esc(a.name)}"/></div>
      <div class="buttons"><button class="btn focusable" id="bNo">Отмена</button><button class="btn primary focusable" id="bYes">Сохранить</button></div>`, { onBack: openProfileModal });
    $('#bNo').onclick = openProfileModal;
    $('#bYes').onclick = async () => {
      const v = $('#fNick').value.trim();
      if (v.length < 2) return toast('Ник должен быть не короче 2 символов', true);
      a.name = v;
      await save();
      publishProfile();
      closeModal();
      renderSocial();
    };
  };
  $('#pKey').onclick = () => {
    openModal(`<h2>Ключ входа</h2>
      <div class="meta"><span>Это пароль от аккаунта. Никому его не показывайте — с ним можно писать от вашего имени.</span></div>
      <div class="field" style="margin-top:14px"><input readonly class="focusable" value="${esc(N.nip19.nsecEncode(me.sk))}"/></div>
      <div class="buttons"><button class="btn focusable" id="bOk">Готово</button></div>`, { onBack: openProfileModal });
    $('#bOk').onclick = openProfileModal;
  };
  $('#pOut').onclick = () => {
    openModal(`<h2>Выйти из аккаунта?</h2><div class="meta"><span>Без ключа входа вернуться в этот аккаунт будет нельзя. Сохраните ключ, если он понадобится.</span></div>
      <div class="buttons"><button class="btn focusable" id="bNo">Отмена</button><button class="btn danger focusable" id="bYes">Выйти</button></div>`, { onBack: openProfileModal });
    $('#bNo').onclick = openProfileModal;
    $('#bYes').onclick = async () => {
      subs.forEach((s) => s.close());
      friendSub?.close();
      clearInterval(presenceTimer);
      me = null;
      data.social = null;
      social.chat = null;
      await save();
      closeModal();
      social.idx = 0;
      renderSocial();
      updateDot();
    };
  };
}

function openLoginModal() {
  openModal(`<h2>Вход по ключу</h2>
    <div class="field"><label>Ключ входа (nsec…)</label><input class="focusable" id="fKey" placeholder="nsec1…"/></div>
    <div class="buttons"><button class="btn focusable" id="bNo">Отмена</button><button class="btn primary focusable" id="bYes">Войти</button></div>`);
  $('#bNo').onclick = closeModal;
  $('#bYes').onclick = () => { const v = $('#fKey').value; closeModal(); loginWithKey(v); };
}

function openFindModal() {
  openModal(`<h2>Найти друзей</h2>
    <div class="field"><label>Ник или код друга</label><input class="focusable" id="fFind" placeholder="Например: Artem или npub1…"/></div>
    <div class="steam-list" id="findRes"><div class="meta"><span>Введите ник — покажем игроков OTIUM</span></div></div>
    <div class="buttons"><button class="btn focusable" id="bClose">Закрыть</button></div>`, { onBack: closeModal });
  $('#bClose').onclick = closeModal;
  let t;
  $('#fFind').addEventListener('input', () => { clearTimeout(t); t = setTimeout(runFind, 500); });
}

let profileCache = null;
async function runFind() {
  const q = $('#fFind')?.value.trim() || '';
  const box = $('#findRes');
  if (!box) return;
  if (q.length < 2) return (box.innerHTML = '<div class="meta"><span>Введите хотя бы 2 символа</span></div>');
  box.innerHTML = '<div class="meta"><span>Ищем…</span></div>';
  let found = [];
  if (/^npub1/.test(q)) {
    try { const pk = N.nip19.decode(q).data; found = [{ pk, name: social.profiles[pk]?.name || 'Игрок ' + pk.slice(0, 6) }]; } catch {}
  } else {
    if (!profileCache || Date.now() - profileCache.t > 60e3) {
      const evs = await pool.querySync(RELAYS, { kinds: [0], '#t': [APP_TAG], limit: 500 }).catch(() => []);
      const latest = {};
      for (const e of evs) if (!latest[e.pubkey] || latest[e.pubkey].created_at < e.created_at) latest[e.pubkey] = e;
      profileCache = { t: Date.now(), list: Object.values(latest).map((e) => { try { return { pk: e.pubkey, name: JSON.parse(e.content).name || '' }; } catch { return null; } }).filter(Boolean) };
    }
    const ql = q.toLowerCase();
    found = profileCache.list.filter((p) => p.pk !== me.pk && (p.name.toLowerCase().includes(ql) || shortCode(p.pk) === q.toUpperCase()));
  }
  if (!$('#findRes')) return;
  box.innerHTML = found.length ? found.slice(0, 30).map((p) => {
    const state = isFriend(p.pk) ? 'уже в друзьях' : acct().outgoing.includes(p.pk) ? 'заявка отправлена' : 'добавить';
    return `<button class="steam-item focusable" data-pk="${p.pk}">${avatar(p.pk, p.name)}<span><b>${esc(p.name)}</b><br><small>код ${shortCode(p.pk)}</small></span><span class="val" style="margin-left:auto">${state}</span></button>`;
  }).join('') : '<div class="meta"><span>Никого не нашли</span></div>';
  $$('#findRes .steam-item').forEach((b) => (b.onclick = async () => {
    const pk = b.dataset.pk;
    if (isFriend(pk) || acct().outgoing.includes(pk)) return;
    const p = found.find((x) => x.pk === pk);
    social.profiles[pk] = { name: p.name, at: 0 };
    // they already asked us — accept instead
    if (acct().requests.some((r) => r.pk === pk)) addFriend(pk, p.name, true);
    else {
      acct().outgoing.push(pk);
      saveSoon();
      await sendCtrl(pk, 'fr');
    }
    toast(`Заявка отправлена: ${p.name}`);
    $('.val', b).textContent = 'заявка отправлена';
  }));
}

function openRequestModal(pk) {
  const r = acct().requests.find((x) => x.pk === pk);
  openModal(`<h2>Заявка в друзья</h2>
    <div class="sprof">${avatar(pk, r.name, 'big')}<div><div class="t">${esc(r.name)}</div><div class="d">код ${shortCode(pk)} · ${fmtDate(r.at)}</div></div></div>
    <div class="buttons"><button class="btn focusable" id="bNo">Отклонить</button><button class="btn primary focusable" id="bYes">Принять</button></div>`, { focus: 1 });
  $('#bNo').onclick = () => { acct().requests = acct().requests.filter((x) => x.pk !== pk); saveSoon(); closeModal(); renderSocial(); };
  $('#bYes').onclick = () => { addFriend(pk, r.name, true); closeModal(); toast(`${r.name} теперь в друзьях`); };
}

function openFriendMenu(pk) {
  openModal(`<button class="menu-item focusable" id="mChat">Написать</button>
    <button class="menu-item focusable" id="mCall">Позвонить</button>
    <button class="menu-item focusable danger" id="mDel">Удалить из друзей</button>`, { cls: 'menu' });
  $('#mCall').onclick = () => { closeModal(); startCall(pk); };
  $('#mChat').onclick = () => { closeModal(); social.idx = listItems().findIndex((x) => x.pk === pk); activateItem(); };
  $('#mDel').onclick = () => { closeModal(); removeFriend(pk); toast('Удалено из друзей'); };
}

// keep presence fresh when a game starts / stops
ipc.onSteamRunning(() => { setTimeout(publishPresence, 1500); publishLibrarySoon(); });
ipc.onGameExited(() => { setTimeout(publishPresence, 1500); publishLibrarySoon(); });
