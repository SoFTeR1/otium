'use strict';

// Outside Electron (plain browser preview) fall back to a stub where every call is a no-op
const ipc = window.api || new Proxy({
  load: async () => ({
    games: ['Cyberpunk 2077', 'Elden Ring', 'Hades', 'Forza Horizon 5', 'Baldur’s Gate 3'].map((name, i) => ({ id: 'd' + i, name, category: 'game', path: 'C:\\x.exe', added: i })),
    sessions: [],
    settings: { user: 'Player', sounds: false }
  }),
  launch: async () => ({ ok: true }),
  importMedia: async (p) => p
}, { get: (t, k) => t[k] || (async () => null) });
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
const cssUrl = (u) => `url("${String(u).replace(/["\\]/g, '\\$&')}")`;

const TABS = ['game', 'activity'];
const DIRS = new Set(['up', 'down', 'left', 'right']);

let data = { games: [], settings: {} };
let tab = 'game';
let list = [];
let sel = 0;
let zone = 'row';
let tabFocus = 0;
let actFocus = 0;
let busy = false;
const running = new Set();          // game ids currently running
const exitedEarly = new Set();      // exe games that exited before the launch animation finished
const steamSince = {};              // steam appid -> timestamp it started running
let steamLocal = { steamId: null, apps: {} };   // playtime / last played as recorded by Steam
let launching = null;               // { cancel } while the launch overlay is up
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const isVideo = (u) => /\.(mp4|webm)(\?|$)/i.test(u || '') || /\.m3u8(\?|$)/i.test(u || '');

// Steam knows playtime even for sessions started outside the launcher, so take whichever is larger
function totalPlaytime(g) {
  const s = steamLocal.apps[steamIdOf(g)];
  return Math.max(g.playtime || 0, (s?.playtime || 0) * 60);
}
function lastPlayedOf(g) {
  const s = steamLocal.apps[steamIdOf(g)];
  return Math.max(g.lastPlayed || 0, (s?.lastPlayed || 0) * 1000);
}
function steamIdOf(g) {
  return g?.steamId || (g?.path || '').match(/^steam:\/\/(?:rungameid|run)\/(\d+)/i)?.[1] || null;
}

// ---------- Sounds ----------
let actx;
function tone(freq, dur, vol, type = 'sine', delay = 0) {
  if (!data.settings.sounds) return;
  actx ??= new AudioContext();
  const t = actx.currentTime + delay;
  const o = actx.createOscillator();
  const g = actx.createGain();
  o.type = type;
  o.frequency.setValueAtTime(freq, t);
  g.gain.setValueAtTime(vol, t);
  g.gain.exponentialRampToValueAtTime(0.0001, t + dur);
  o.connect(g).connect(actx.destination);
  o.start(t);
  o.stop(t + dur);
}
const sfx = {
  move: () => tone(1400, 0.035, 0.025),
  ok: () => { tone(880, 0.06, 0.04); tone(1320, 0.09, 0.035, 'sine', 0.05); },
  back: () => tone(520, 0.08, 0.035),
  launch: () => { tone(440, 0.25, 0.05, 'triangle'); tone(660, 0.3, 0.04, 'triangle', 0.12); tone(990, 0.5, 0.03, 'sine', 0.24); },
  err: () => tone(200, 0.18, 0.05, 'square')
};

// ---------- Helpers ----------
function hue(str) {
  let h = 0;
  for (const c of str || '?') h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}
function initials(name) {
  const w = (name || '?').trim().split(/\s+/).filter(Boolean);
  return (w.length > 1 ? w[0][0] + w[1][0] : (w[0] || '?').slice(0, 2)).toUpperCase();
}
function fmtPlaytime(sec) {
  if (!sec) return '';
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  return h ? `${h} ч ${m} мин` : `${Math.max(1, m)} мин`;
}
function fmtDate(ts) {
  if (!ts) return '';
  const d = new Date(ts);
  const days = Math.floor((Date.now() - ts) / 86400000);
  if (days === 0) return 'сегодня, ' + d.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
  if (days === 1) return 'вчера';
  if (days < 7) return `${days} дн. назад`;
  return d.toLocaleDateString('ru-RU', { day: 'numeric', month: 'long', year: 'numeric' });
}
function isFilePath(p) {
  return /^[a-z]:[\\/]/i.test(p || '') || /^\\\\/.test(p || '');
}
function save() {
  return ipc.save(data).catch((e) => toast('Не удалось сохранить настройки: ' + (e?.message || e), true));
}
// Steam-style notification cards, bottom-left, up to three stacked.
// opts: { title, img (picture url), icon (emoji/text), avatar: { name, key } }
function toast(msg, err = false, opts = {}) {
  // launcher not in front (a game or another app is) → show it in the always-on-top overlay instead
  if (window.api && !document.hasFocus()) {
    ipc.overlaySend({ type: 'toast', text: msg, err, title: opts.title, img: opts.img, icon: opts.icon, avatar: opts.avatar, ms: opts.ms });
    return;
  }
  const box = $('#toasts');
  const el = document.createElement('div');
  el.className = 'note' + (err ? ' err' : '');
  let pic;
  if (opts.img) pic = `<div class="note-pic" style="background-image:${esc(cssUrl(opts.img))}"></div>`;
  else if (opts.avatar) {
    const h = hue(opts.avatar.key || opts.avatar.name);
    pic = `<div class="note-pic letter" style="background:linear-gradient(135deg,hsl(${h} 65% 45%),hsl(${(h + 60) % 360} 65% 30%))">${esc((opts.avatar.name || '?')[0].toUpperCase())}</div>`;
  } else pic = opts.icon ? `<div class="note-pic emoji">${esc(opts.icon)}</div>` : '<div class="note-pic" style="background-image:url(icon.png)"></div>';
  el.innerHTML = `${pic}<div class="note-body"><div class="note-title">${esc(opts.title || (err ? 'Ошибка' : 'PS5 Launcher'))}</div><div class="note-text">${esc(msg)}</div></div>`;
  box.appendChild(el);
  while (box.children.length > 3) box.firstElementChild.remove();
  requestAnimationFrame(() => requestAnimationFrame(() => el.classList.add('show')));
  setTimeout(() => {
    el.classList.remove('show');
    setTimeout(() => el.remove(), 450);
  }, opts.ms || (err ? 6000 : 5000));
}

// ---------- Background ----------
let bgFront = 'bgA';
let bgCurrent = null;
let bgTimer;
function setBackground(url, blur) {
  const key = url ? url + (blur ? '#b' : '') : null;
  if (key === bgCurrent) return;
  bgCurrent = key;
  clearTimeout(bgTimer);
  bgTimer = setTimeout(() => {
    const swap = () => {
      if (bgCurrent !== key) return;
      const back = bgFront === 'bgA' ? 'bgB' : 'bgA';
      const nb = $('#' + back);
      const ob = $('#' + bgFront);
      nb.style.backgroundImage = url ? cssUrl(url) : 'none';
      nb.classList.toggle('blur', !!blur);
      nb.classList.add('show');
      ob.classList.remove('show');
      bgFront = back;
    };
    if (!url) return swap();
    const img = new Image();
    img.onload = swap;
    img.onerror = () => {
      // e.g. a game without Steam hero art — fall back to the next candidate
      const next = bgCandidates(list[sel]).find((c) => c.url !== url && !bgFailed.has(c.url));
      bgFailed.add(url);
      bgCurrent = null;
      if (next) setBackground(next.url, next.blur);
      else setBackground(null);
    };
    img.src = url;
  }, 140);
}

// Priority: game's own custom background > game art (unless "global" mode) > shared wallpaper > blurred cover
const bgFailed = new Set();
function bgCandidates(g) {
  const s = data.settings;
  const out = [];
  if (g?.background) out.push({ url: g.background });
  if (g?.hero && s.bgMode !== 'global') out.push({ url: g.hero });
  const wall = currentWallpaper();
  if (wall) out.push({ url: wall });
  if (g?.cover) out.push({ url: g.cover, blur: true });
  return out.filter((c) => !bgFailed.has(c.url));
}
// ---------- Shared wallpapers: several, shown as a random slideshow ----------
const SLIDE_NAMES = { 0: 'Выкл', 30: 'каждые 30 сек', 60: 'каждую минуту', 300: 'каждые 5 мин', 900: 'каждые 15 мин' };
let slideIdx = 0;
let slideAt = Date.now();
function currentWallpaper() {
  const w = data.settings.wallpapers || [];
  return w.length ? w[slideIdx % w.length] : '';
}
setInterval(() => {
  const s = data.settings;
  const w = s.wallpapers || [];
  if (!s.slideshow || w.length < 2 || document.hidden || Date.now() - slideAt < s.slideshow * 1000) return;
  const prev = currentWallpaper();
  let next = slideIdx;
  while (next === slideIdx) next = Math.floor(Math.random() * w.length);
  slideIdx = next;
  slideAt = Date.now();
  // only swap if the wallpaper is what's on screen right now (not a game's own art)
  if ((bgCurrent && bgCurrent.startsWith(prev)) || videoUrl === prev) applyBackground(list[sel]);
}, 1000);

function openWallpapers(focus = 0) {
  const s = data.settings;
  const w = (s.wallpapers ||= []);
  openModal(`<h2>Общие фоны</h2>
    <div class="meta" style="margin-bottom:14px"><span>${w.length > 1 ? `Фоны меняются в случайном порядке: ${SLIDE_NAMES[s.slideshow || 0].toLowerCase()}. Нажмите на фон, чтобы убрать его.` : 'Добавьте несколько картинок или видео — их можно показывать слайдшоу.'}</span></div>
    <div class="wp-grid">${w.map((u, i) => `<button class="wp focusable" data-i="${i}" ${isVideo(u) ? '' : `style="background-image:${esc(cssUrl(u))}"`}>${isVideo(u) ? '🎬' : ''}<b>✕</b></button>`).join('')}
      <button class="wp add focusable" id="wpAdd">+<span>Добавить</span></button></div>
    <div class="buttons"><button class="btn focusable" id="wpSlide">Смена: ${SLIDE_NAMES[s.slideshow || 0]}</button><button class="btn primary focusable" id="wpDone">Готово</button></div>`,
  { onBack: () => openSettings(5), focus });
  $$('#modal .wp[data-i]').forEach((b) => (b.onclick = async () => {
    w.splice(+b.dataset.i, 1);
    slideIdx = 0;
    await save();
    update(true);
    openWallpapers(Math.max(0, +b.dataset.i - 1));
  }));
  $('#wpAdd').onclick = async () => {
    const files = await ipc.openFile('background', true);
    if (!files?.length) return;
    for (const f of files) {
      const u = await ipc.importMedia(f);
      bgFailed.delete(u);
      w.push(u);
    }
    if (!s.slideshow && w.length > 1) s.slideshow = 60;
    await save();
    update(true);
    openWallpapers(w.length);
  };
  $('#wpSlide').onclick = async () => {
    const steps = [0, 30, 60, 300, 900];
    s.slideshow = steps[(steps.indexOf(s.slideshow || 0) + 1) % steps.length];
    await save();
    openWallpapers(w.length + 1);
  };
  $('#wpDone').onclick = () => openSettings(5);
}

function applyBackground(g) {
  const all = bgCandidates(g);
  const top = all[0];
  // a video (file or trailer) plays on its own layer; the best still image stays underneath as a poster
  const still = all.find((c) => !isVideo(c.url));
  setBackground(still?.url || null, still?.blur);
  clearTimeout(trailerTimer);
  if (top && isVideo(top.url)) return setVideoBg(top.url);
  setVideoBg(null);
  const sid = steamIdOf(g);
  if (sid && data.settings.liveTrailers && !g.background && data.settings.bgMode !== 'global') {
    // like PS5: after resting on a game for a moment its trailer fades in
    const id = g.id;
    trailerTimer = setTimeout(async () => {
      const url = await ipc.trailer(sid);
      if (url && list[sel]?.id === id && zone !== 'feed') setVideoBg(url);
    }, 2500);
  }
}

let trailerTimer;
let hls = null;
let videoUrl = null;
function setVideoBg(url) {
  if (url === videoUrl) return;
  videoUrl = url;
  const v = $('#bgVideo');
  v.classList.remove('show');
  if (hls) { hls.destroy(); hls = null; }
  if (!url) {
    setTimeout(() => { if (!videoUrl) { v.removeAttribute('src'); v.load(); } }, 900);
    return;
  }
  v.onplaying = () => videoUrl === url && v.classList.add('show');
  const play = () => v.play().catch(() => {});
  if (/\.m3u8/i.test(url) && window.Hls?.isSupported()) {
    hls = new Hls({ enableWorker: false, capLevelToPlayerSize: true, maxBufferLength: 20 });
    hls.loadSource(url);
    hls.attachMedia(v);
    hls.on(Hls.Events.MANIFEST_PARSED, play);
    hls.on(Hls.Events.ERROR, (_e, d) => d.fatal && setVideoBg(null));
  } else {
    v.src = url;
    v.onerror = () => setVideoBg(null);
    play();
  }
}
// don't burn the GPU on a trailer while a game is running
// The launcher now stays under the game instead of minimising, so "hidden" no longer means "in a game":
// a game is running and we don't have focus → idle (pause the trailer, skip polling)
function idleBehindGame() {
  return document.hidden || (running.size > 0 && !document.hasFocus());
}
function syncIdle() {
  const v = $('#bgVideo');
  document.body.classList.toggle('idle', idleBehindGame());
  if (idleBehindGame()) v.pause();
  else if (videoUrl) v.play().catch(() => {});
}
document.addEventListener('visibilitychange', syncIdle);
window.addEventListener('blur', () => setTimeout(syncIdle, 300));
window.addEventListener('focus', syncIdle);

// ---------- Rendering ----------
const SORT_NAMES = { recent: 'Недавние', alpha: 'По алфавиту', custom: 'Свой порядок' };
function sortedItems() {
  const mode = data.settings.sortMode || 'recent';
  if (tab === 'activity') return activityItems();
  // apps (the old "Media" tab) live alongside games
  const items = [...data.games];
  if (mode === 'alpha') return items.sort((a, b) => a.name.localeCompare(b.name, 'ru'));
  if (mode === 'custom') return items.sort((a, b) => (a.order ?? 1e9) - (b.order ?? 1e9) || (b.added || 0) - (a.added || 0));
  return items.sort((a, b) => lastPlayedOf(b) - lastPlayedOf(a) || (b.added || 0) - (a.added || 0));
}

// ---------- Activity tab: friends as tiles, players in a game first ----------
const STEAM_ART = 'https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps';
function activityItems() {
  const items = [];
  for (const f of (typeof acct === 'function' && acct()?.friends) || []) {
    const p = presenceOf(f.pk);
    items.push({ id: 'lf:' + f.pk, kind: 'friend', src: 'launcher', pk: f.pk, name: f.name, online: p.online, game: p.game ? p.text.replace(/^В игре: /, '') : null, status: p.text });
  }
  for (const f of steamFriendsCache || []) {
    const online = f.state > 0 || !!f.game;
    items.push({
      id: 'sf:' + f.id, kind: 'friend', src: 'steam', sid: f.id, name: f.name, cover: f.avatar, online, game: f.game, gameId: f.gameId,
      hero: f.gameId ? `${STEAM_ART}/${f.gameId}/library_hero.jpg` : '',
      status: f.game ? `В игре: ${f.game}` : f.state === 1 ? 'В сети' : f.state ? 'Нет на месте' : 'Не в сети'
    });
  }
  return items.sort((a, b) => !!b.game - !!a.game || b.online - a.online || a.name.localeCompare(b.name, 'ru'));
}
function friendView(f) {
  return f.src === 'steam' ? { kind: 'steam', id: f.sid } : { kind: 'friend', id: f.pk };
}
function chatWith(f) {
  if (f.src === 'steam') return ipc.openUrl(`steam://friends/message/${f.sid}`);
  openSocial();
  social.idx = listItems().findIndex((x) => x.pk === f.pk);
  activateItem();
}

// ---------- Custom order ----------
// Switching to the custom order freezes whatever order is on screen right now, then tiles can be moved
function freezeOrder() {
  if (data.settings.sortMode !== 'custom') {
    for (const t of ['game']) {
      const prevTab = tab;
      tab = t;
      sortedItems().forEach((g, i) => (g.order = i));
      tab = prevTab;
    }
    data.settings.sortMode = 'custom';
  }
  list.forEach((g, i) => (g.order = i));
}
function moveToFront(g) {
  freezeOrder();
  list.forEach((x) => { if (x !== g) x.order += 1; });
  g.order = 0;
  save();
  render(g.id);
  toast(`«${g.name}» теперь первая`);
}
let reorder = false;
function startReorder() {
  if (!list[sel]) return;
  freezeOrder();
  reorder = true;
  zone = 'row';
  update();
  toast('Двигайте плитку влево и вправо, ✕ / Enter — готово');
}
function reorderHandle(act) {
  const g = list[sel];
  if ((act === 'left' && sel > 0) || (act === 'right' && sel < list.length - 1)) {
    const j = act === 'left' ? sel - 1 : sel + 1;
    [list[sel].order, list[j].order] = [list[j].order, list[sel].order];
    sfx.move();
    render(g.id);
  } else if (act === 'ok' || act === 'back') {
    reorder = false;
    sfx.ok();
    save();
    update();
  }
}

function tileHTML(g, i) {
  if (!g) return `<div class="tile add" data-i="${i}">+<span class="name">${tab === 'game' ? 'Добавить игру' : 'Найти друзей'}</span></div>`;
  if (g.kind === 'friend') {
    const fh = hue(g.pk || g.sid);
    return `<div class="tile friend${g.online ? '' : ' offline'}" data-i="${i}">${g.cover ? '' : `<div class="ph" style="--c1:hsl(${fh} 65% 45%);--c2:hsl(${(fh + 60) % 360} 65% 30%)">${esc(g.name[0].toUpperCase())}</div>`}<i class="pres ${g.game ? 'game' : g.online ? 'on' : ''}"></i><span class="name">${esc(g.name)}</span></div>`;
  }
  const h = hue(g.name);
  const ph = `<div class="ph" style="--c1:hsl(${h} 70% 38%);--c2:hsl(${(h + 50) % 360} 70% 26%)">${esc(initials(g.name))}</div>`;
  return `<div class="tile" data-i="${i}" data-id="${g.id}">${g.cover ? '' : ph}<span class="name">${esc(g.name)}</span></div>`;
}

function render(keepId) {
  list = sortedItems();
  const track = $('#track');
  track.innerHTML = list.map(tileHTML).join('') + tileHTML(null, list.length);
  // covers are set right away (cached images show instantly, no flicker); placeholder only on error
  list.forEach((g, i) => {
    if (!g.cover) return;
    const el = track.children[i];
    el.style.backgroundImage = cssUrl(g.cover);
    const img = new Image();
    img.onerror = () => {
      el.style.backgroundImage = '';
      const h = hue(g.name);
      el.insertAdjacentHTML('afterbegin', `<div class="ph" style="--c1:hsl(${h} 70% 38%);--c2:hsl(${(h + 50) % 360} 70% 26%)">${esc(initials(g.name))}</div>`);
    };
    img.src = g.cover;
  });
  if (keepId) {
    const i = list.findIndex((g) => g.id === keepId);
    if (i >= 0) sel = i;
  }
  sel = Math.max(0, Math.min(sel, list.length));
  update(true);
  refreshContinue();
  refreshStats();
}

function tileMetrics() {
  const m = $('#measure');
  return { w: m.offsetWidth, gap: m.offsetHeight };
}

let infoKey = null;
function update(force) {
  const track = $('#track');
  const tiles = [...track.children];
  // like PS5: one tile stays visible to the left of the selected one
  const first = Math.max(0, sel - 1);
  tiles.forEach((t, i) => {
    t.classList.toggle('sel', i === sel);
    t.classList.toggle('before', i < first);
    t.classList.toggle('moving', reorder && i === sel);
  });
  const { w, gap } = tileMetrics();
  track.style.transform = `translateX(${-first * (w + gap)}px)`;

  // focus rings
  $('#row').classList.toggle('focused', zone === 'row');
  $$('.tabs .tab').forEach((t) => t.classList.toggle('active', t.dataset.tab === tab));
  const tabEls = [...$$('.tabs .tab'), $('[data-action="search"]'), $('[data-action="friends"]'), $('[data-action="settings"]'), $('[data-action="profile"]')];
  tabEls.forEach((t, i) => t.classList.toggle('focus', zone === 'tabs' && i === tabFocus));
  const g = list[sel];
  const actEls = g ? [$('#btnPlay'), $('#btnMore')] : [$('#btnPlay')];
  if (actFocus >= actEls.length) actFocus = 0;
  [$('#btnPlay'), $('#btnMore')].forEach((b) => b.classList.toggle('focus', zone === 'actions' && b === actEls[actFocus]));
  feedUpdateFocus();

  // "Играет в …" chip in the top bar while a game runs
  const playing = data.games.find((x) => running.has(x.id));
  $('#playing').classList.toggle('hidden', !playing);
  if (playing) $('#playing span').textContent = `Играет в ${playing.name}`;

  // info panel
  const key = g ? [g.id, lastPlayedOf(g), totalPlaytime(g), g.name, g.logo, running.has(g.id)].join(':') : 'add:' + tab;
  if (key !== infoKey || force) {
    infoKey = key;
    const info = $('#info');
    info.classList.add('swap');
    requestAnimationFrame(() => {
      fillInfo(g);
      requestAnimationFrame(() => info.classList.remove('swap'));
    });
  }

  applyBackground(g);
  feedOnSelect(g);
}

function fillInfo(g) {
  const info = $('#info');
  const title = $('#title');
  const logo = $('#logo');
  info.classList.toggle('empty', !g);
  logo.classList.remove('show');
  title.classList.remove('hidden');
  if (!g && tab === 'activity') {
    title.textContent = 'Найти друзей';
    $('#meta').innerHTML = `<span>${list.length ? 'Добавьте друзей по нику или коду' : 'Здесь будут друзья из PS5 Launcher и Steam — и во что они играют'}</span>`;
    $('#btnPlay').textContent = 'Найти';
    return;
  }
  if (!g) {
    title.textContent = 'Добавить игру';
    $('#meta').innerHTML = `<span>${list.length ? 'Укажите exe, ярлык или ссылку' : 'Библиотека пуста — добавьте первую игру или импортируйте из Steam в настройках'}</span>`;
    $('#btnPlay').textContent = 'Добавить';
    return;
  }
  title.textContent = g.name;
  if (g.kind === 'friend') {
    $('#meta').innerHTML = `<span class="${g.game ? 'live' : ''}">● ${esc(g.status)}</span><span>${g.src === 'steam' ? 'Друг в Steam' : 'Друг в PS5 Launcher'}</span>`;
    $('#btnPlay').textContent = 'Профиль';
    return;
  }
  if (g.logo) {
    logo.onload = () => { logo.classList.add('show'); title.classList.add('hidden'); };
    logo.onerror = () => { logo.classList.remove('show'); title.classList.remove('hidden'); };
    logo.src = g.logo;
  }
  const meta = [];
  if (running.has(g.id)) meta.push('<span class="live">● Запущено</span>');
  if (steamIdOf(g)) meta.push('Steam');
  meta.push(lastPlayedOf(g) ? 'Последний запуск: ' + fmtDate(lastPlayedOf(g)) : 'Ещё не запускалась');
  if (totalPlaytime(g)) meta.push('Сыграно: ' + fmtPlaytime(totalPlaytime(g)));
  $('#meta').innerHTML = meta.map((m) => (m.startsWith('<span') ? m : `<span>${esc(m)}</span>`)).join('');
  const isRunning = running.has(g.id);
  $('#btnPlay').textContent = isRunning ? (g.category === 'app' ? 'Закрыть' : 'Закрыть игру') : g.category === 'app' ? 'Открыть' : 'Играть';
  $('#btnPlay').classList.toggle('stop', isRunning);
}

// "Play" turns into "Close game" while the game runs
function confirmCloseGame(g) {
  openModal(`<h2>Закрыть «${esc(g.name)}»?</h2>
    <div class="meta"><span>Игра получит команду закрыться, а если не ответит за 5 секунд — будет завершена. Несохранённый прогресс может потеряться.</span></div>
    <div class="buttons"><button class="btn focusable" id="cgNo">Отмена</button><button class="btn danger focusable" id="cgYes">Закрыть игру</button></div>`, { focus: 0 });
  $('#cgNo').onclick = closeModal;
  $('#cgYes').onclick = async () => {
    closeModal();
    toast(`Закрываем ${g.name}…`, false, { title: 'Игра', icon: '⏹' });
    const r = await ipc.closeGame(g);
    if (!r?.ok) return toast(r?.error || 'Не получилось закрыть игру', true, { title: g.name });
    // exe games report their exit themselves; Steam updates its running-app flag within a few seconds
    setTimeout(() => {
      if (running.has(g.id) && !steamIdOf(g)) { running.delete(g.id); render(g.id); }
    }, 1500);
    toast(r.forced ? 'Игра не отвечала и была завершена принудительно' : 'Игра закрыта', false, { title: g.name, icon: '✅' });
  };
}

let tabSwitchTimer;
function switchTab(t) {
  if (t === tab) return;
  tab = t;
  sel = 0;
  // fade the old list out, swap content while invisible, fade the new one in
  document.body.classList.add('fade');
  clearTimeout(tabSwitchTimer);
  tabSwitchTimer = setTimeout(() => {
    render();
    requestAnimationFrame(() => document.body.classList.remove('fade'));
  }, 200);
}

// ---------- Launch ----------
async function launch(g) {
  if (busy) return;
  busy = true;
  sfx.launch();

  const launchedAt = Date.now();
  const tile = $('#track').children[list.indexOf(g)];
  const from = tile?.getBoundingClientRect() || { left: innerWidth / 2, top: innerHeight / 2, width: 0, height: 0 };
  const size = Math.min(innerHeight * 0.26, 300);
  const cy = innerHeight * 0.4;
  const h = hue(g.name);

  const ov = document.createElement('div');
  ov.className = 'launch';
  ov.innerHTML = `
    <div class="launch-bg"></div>
    <div class="launch-icon">${g.cover ? '' : `<div class="ph" style="--c1:hsl(${h} 70% 38%);--c2:hsl(${(h + 50) % 360} 70% 26%)">${esc(initials(g.name))}</div>`}</div>
    <div class="launch-text" style="top:${cy + size / 2 + 34}px">
      <div class="launch-sub">${g.category === 'app' ? 'Открытие' : 'Запуск игры'}</div>
      <div class="launch-name">${esc(g.name)}</div>
      <div class="launch-err"></div>
      <div class="launch-bar"><i></i></div>
    </div>
    <div class="launch-hint">Нажмите <b>${GLYPHS[currentLayout()].back[0]}</b> или Esc, чтобы скрыть заставку</div>`;
  const icon = $('.launch-icon', ov);
  if (g.cover) icon.style.backgroundImage = cssUrl(g.cover);
  Object.assign(icon.style, { left: from.left + 'px', top: from.top + 'px', width: from.width + 'px', height: from.height + 'px' });
  document.body.appendChild(ov);
  if (tile) tile.style.visibility = 'hidden';

  // FLIP: fly the tile from the row to the centre of the screen
  requestAnimationFrame(() => requestAnimationFrame(() => {
    ov.classList.add('in');
    Object.assign(icon.style, { left: (innerWidth - size) / 2 + 'px', top: cy - size / 2 + 'px', width: size + 'px', height: size + 'px' });
  }));

  // stay above every other window (Steam's "launching…" dialog included) until the game is up
  ipc.cover(true);
  let cancel;
  const cancelled = new Promise((r) => (cancel = r));
  launching = { cancel: () => cancel('cancel') };

  const [res] = await Promise.all([ipc.launch(g), sleep(1500)]);
  if (!res.ok) {
    sfx.err();
    ov.classList.add('fail');
    $('.launch-sub', ov).textContent = 'Не удалось запустить';
    $('.launch-err', ov).textContent = res.error || '';
    await sleep(2600);
    ipc.cover(false);
    return finishLaunch(g, ov, false);
  }

  g.lastPlayed = Date.now();
  save();
  const sid = steamIdOf(g);
  $('.launch-hint', ov).classList.add('show');
  let ready;
  if (sid) {
    // Steam reports the running app in the registry once the game process has started
    $('.launch-sub', ov).textContent = 'Steam запускает игру';
    ready = waitSteam(sid, 120e3).then(async (ok) => { if (ok) await sleep(2500); return ok ? 'ready' : 'timeout'; });
  } else {
    if (res.tracked && !exitedEarly.delete(g.id)) running.add(g.id);
    ready = sleep(res.tracked ? 4000 : 2500).then(() => 'ready');
  }
  const outcome = await Promise.race([ready, cancelled]);
  // the exe already closed during the wait — nothing to step aside for
  if (res.tracked && !sid && !running.has(g.id)) {
    ipc.cover(false);
    return finishLaunch(g, ov, false);
  }
  if (outcome === 'ready') {
    ov.classList.add('ok');
    $('.launch-sub', ov).textContent = g.category === 'app' ? 'Открыто' : 'Игра запущена';
    await sleep(900);
  }
  finishLaunch(g, ov, true, launchedAt);
}

async function finishLaunch(g, ov, stepAside, launchedAt) {
  launching = null;
  if (stepAside) launchStarted[g.id] = Date.now();
  publishPresence();
  updateDiscord();
  // rebuild the row underneath the overlay so the reorder is never visible
  zone = 'row';
  render(g.id);
  if (stepAside) {
    // the game window is raised above us; keep the splash a moment so there's no flash while it appears
    await ipc.stepAside(launchedAt);
    setTimeout(() => { ov.classList.add('out'); setTimeout(() => ov.remove(), 560); }, 2500);
  } else {
    ov.classList.add('out');
    await sleep(560);
    ov.remove();
  }
  busy = false;
}

const steamWaiters = [];
function waitSteam(appid, timeout) {
  return new Promise(async (resolve) => {
    if (String(await ipc.steamRunning()) === String(appid)) return resolve(true);
    const w = { appid: String(appid), resolve };
    steamWaiters.push(w);
    setTimeout(() => {
      const i = steamWaiters.indexOf(w);
      if (i >= 0) { steamWaiters.splice(i, 1); resolve(false); }
    }, timeout);
  });
}

function addSession(g, start, end) {
  const secs = Math.round((end - start) / 1000);
  if (secs < 5) return;
  g.playtime = (g.playtime || 0) + secs;
  data.sessions = [...(data.sessions || []), { id: g.id, start, end }].slice(-3000);
}

ipc.onGameExited(async ({ id, start, end }) => {
  const g = data.games.find((x) => x.id === id);
  if (!running.delete(id)) exitedEarly.add(id);
  if (!g) return;
  addSession(g, start, end);
  await save();
  render(list[sel]?.id);
});

// Steam game started / stopped (by us or from anywhere else)
ipc.onSteamRunning(async ({ id, prev, at }) => {
  if (prev) {
    for (const g of data.games.filter((x) => steamIdOf(x) === String(prev))) {
      running.delete(g.id);
      if (steamSince[prev]) addSession(g, steamSince[prev], at);
    }
    delete steamSince[prev];
    await save();
    // Steam writes its own playtime a moment after the game closes
    setTimeout(loadSteamLocal, 6000);
  }
  if (id) {
    steamSince[id] = at;
    for (const g of data.games.filter((x) => steamIdOf(x) === String(id))) running.add(g.id);
    for (let i = steamWaiters.length - 1; i >= 0; i--) {
      if (steamWaiters[i].appid === String(id)) steamWaiters.splice(i, 1)[0].resolve(true);
    }
  }
  render(list[sel]?.id);
});

async function loadSteamLocal() {
  const s = await ipc.steamLocal();
  if (s) steamLocal = s;
  render(list[sel]?.id);
}

function activate() {
  const g = list[sel];
  if (g?.kind === 'friend') return openProfile(friendView(g));
  if (g && running.has(g.id)) return confirmCloseGame(g);
  if (!g && tab === 'activity') { openSocial(); social.idx = 1; return activateItem(); }
  if (g) launch(g);
  else openEditor();
}

// ---------- Modal system ----------
const modal = { open: false, idx: 0, onBack: null, scroll: null };

function openModal(html, { cls = '', onBack = null, focus = 0, scroll = null } = {}) {
  const m = $('#modal');
  m.className = 'modal ' + cls;
  m.innerHTML = html;
  modal.open = true;
  modal.onBack = onBack;
  modal.scroll = scroll;
  $('#overlay').classList.remove('hidden');
  setModalFocus(focus, false);
}

function closeModal() {
  modal.open = false;
  $('#overlay').classList.add('hidden');
  document.activeElement?.blur();
}

function modalEls() {
  return $$('.focusable', $('#modal')).filter((e) => !e.disabled && e.offsetParent !== null);
}

function setModalFocus(i, sound = true) {
  const els = modalEls();
  if (!els.length) return;
  i = Math.max(0, Math.min(i, els.length - 1));
  if (sound && i !== modal.idx) sfx.move();
  modal.idx = i;
  els.forEach((e, j) => e.classList.toggle('focus', j === i));
  const el = els[i];
  if (el.tagName === 'INPUT') el.focus();
  else if (document.activeElement?.tagName === 'INPUT') document.activeElement.blur();
  el.scrollIntoView({ block: 'nearest' });
}

function modalHandle(act, src) {
  if (callMenuKey(act)) return;
  const els = modalEls();
  const el = els[modal.idx];
  if (act === 'back') {
    sfx.back();
    return modal.onBack ? modal.onBack() : closeModal();
  }
  // modals with long text: up/down scroll it, left/right move between buttons
  if (modal.scroll && (act === 'up' || act === 'down')) {
    $(modal.scroll)?.scrollBy({ top: act === 'down' ? 160 : -160, behavior: 'smooth' });
    return;
  }
  if (act === 'up' || act === 'left') return setModalFocus(modal.idx - 1);
  if (act === 'down' || act === 'right') return setModalFocus(modal.idx + 1);
  if (act === 'ok' && el) {
    if (el.tagName === 'INPUT') {
      if (src === 'key') setModalFocus(modal.idx + 1);
      // from the couch: type with the on-screen keyboard
      else openOSK(el, { onDone: () => setModalFocus(modal.idx + 1) });
      return;
    }
    sfx.ok();
    el.click();
  }
}

$('#modal').addEventListener('mousedown', (e) => {
  const f = e.target.closest('.focusable');
  if (!f) return;
  const i = modalEls().indexOf(f);
  if (i >= 0) setModalFocus(i, false);
});
$('#overlay').addEventListener('mousedown', (e) => {
  if (e.target.id === 'overlay') modal.onBack ? modal.onBack() : closeModal();
});

// ---------- Editor ----------
function openEditor(game) {
  const g = game ? { ...game } : { name: '', category: tab, path: '', args: '', cover: '', background: '' };
  const catLabel = (c) => (c === 'app' ? 'Приложение (Медиа)' : 'Игра');
  openModal(`
    <h2>${game ? 'Изменить' : tab === 'game' ? 'Добавить игру' : 'Добавить приложение'}</h2>
    <div class="field"><label>Путь к exe / ярлыку / ссылке</label>
      <div class="line"><input class="focusable" id="fPath" placeholder="C:\\Games\\Game\\game.exe или steam://rungameid/730" value="${esc(g.path)}"/>
      <button class="btn focusable" id="bPath">Обзор</button></div></div>
    <div class="field"><label>Название</label><input class="focusable" id="fName" value="${esc(g.name)}"/></div>
    <div class="field"><label>Раздел</label><button class="btn focusable" id="fCat">${catLabel(g.category)}</button></div>
    <div class="field"><label>Аргументы запуска</label><input class="focusable" id="fArgs" placeholder="необязательно" value="${esc(g.args)}"/></div>
    <div class="field"><label>Обложка (квадратная)</label>
      <div class="line"><input class="focusable" id="fCover" placeholder="файл или https://…" value="${esc(g.cover)}"/>
      <button class="btn focusable" id="bCover">Обзор</button></div>
      <div class="hint">Если не указать — возьмём иконку из exe</div></div>
    <div class="field"><label>Свой фон (широкий)</label>
      <div class="line"><input class="focusable" id="fHero" placeholder="файл или https://…" value="${esc(g.background)}"/>
      <button class="btn focusable" id="bHero">Обзор</button></div>
      <div class="hint">Если не указать — арт игры из Steam или общий фон из настроек</div></div>
    <div class="preview"><div class="pc" id="pvCover"></div><div class="ph2" id="pvHero"></div></div>
    <div class="buttons">
      <button class="btn focusable" id="bCancel">Отмена</button>
      <button class="btn primary focusable" id="bSave">Сохранить</button>
    </div>`, { focus: game ? 2 : 1 });

  const pv = () => {
    $('#pvCover').style.backgroundImage = $('#fCover').value ? cssUrl($('#fCover').value) : 'none';
    $('#pvHero').style.backgroundImage = $('#fHero').value ? cssUrl($('#fHero').value) : 'none';
  };
  pv();
  $('#fCover').addEventListener('input', pv);
  $('#fHero').addEventListener('input', pv);

  $('#bPath').onclick = async () => {
    const p = await ipc.openFile('exe');
    if (!p) return;
    $('#fPath').value = p;
    if (!$('#fName').value.trim()) {
      $('#fName').value = p.split(/[\\/]/).pop().replace(/\.(exe|lnk|url|bat|cmd)$/i, '').replace(/[_.-]+/g, ' ');
    }
  };
  const pick = (input, kind) => async () => {
    const p = await ipc.openFile(kind);
    if (!p) return;
    $(input).value = await ipc.importMedia(p);
    pv();
  };
  $('#bCover').onclick = pick('#fCover', 'image');
  $('#bHero').onclick = pick('#fHero', 'background');
  $('#fCat').onclick = () => {
    g.category = g.category === 'app' ? 'game' : 'app';
    $('#fCat').textContent = catLabel(g.category);
  };
  $('#bCancel').onclick = closeModal;
  $('#bSave').onclick = async () => {
    const path = $('#fPath').value.trim().replace(/^"|"$/g, '');
    const name = $('#fName').value.trim();
    if (!path) return toast('Укажите путь к игре', true);
    if (!name) return toast('Укажите название', true);
    let cover = $('#fCover').value.trim();
    if (cover && isFilePath(cover)) cover = await ipc.importMedia(cover).catch(() => cover);
    let background = $('#fHero').value.trim();
    if (background && isFilePath(background)) background = await ipc.importMedia(background).catch(() => background);
    if (!cover && /\.exe$/i.test(path)) cover = (await ipc.exeIcon(path)) || '';

    const rec = { ...g, name, path, args: $('#fArgs').value.trim(), cover, background };
    bgFailed.delete(background);
    if (game) {
      Object.assign(data.games.find((x) => x.id === game.id), rec);
    } else {
      rec.id = crypto.randomUUID();
      rec.added = Date.now();
      data.games.push(rec);
    }
    await save();
    closeModal();
    tab = 'game';
    zone = 'row';
    render(rec.id);
    toast(game ? 'Сохранено' : `«${name}» добавлена в библиотеку`);
  };
}

// ---------- Options menu ----------
function openOptions() {
  const g = list[sel];
  if (!g) return tab === 'activity' ? activate() : openEditor();
  if (g.kind === 'friend') {
    openModal(`<button class="menu-item focusable" id="fProf">Открыть профиль</button>
      <button class="menu-item focusable" id="fChat">Написать</button>
      ${g.src === 'launcher' ? '<button class="menu-item focusable" id="fCall">Позвонить</button>' : ''}`, { cls: 'menu' });
    if ($('#fCall')) $('#fCall').onclick = () => { closeModal(); startCall(g.pk); };
    $('#fProf').onclick = () => { closeModal(); openProfile(friendView(g)); };
    $('#fChat').onclick = () => { closeModal(); chatWith(g); };
    return;
  }
  const items = [
    running.has(g.id) ? ['close', g.category === 'app' ? 'Закрыть' : 'Закрыть игру', 'danger'] : ['play', g.category === 'app' ? 'Открыть' : 'Играть'],
    ['edit', 'Изменить'],
    ['first', 'Сделать первой в списке'],
    ['reorder', 'Переставить'],
    ['bg', 'Установить свой фон'],
    ...(g.background ? [['bgReset', 'Убрать свой фон']] : []),
    ...(isFilePath(g.path) ? [['folder', 'Показать в папке']] : []),
    ['sep'],
    ['del', 'Удалить из библиотеки', 'danger']
  ];
  openModal(items.map(([id, label, cls]) => (id === 'sep' ? '<div class="menu-sep"></div>' : `<button class="menu-item focusable ${cls || ''}" data-id="${id}">${esc(label)}</button>`)).join(''), { cls: 'menu' });
  $$('#modal .menu-item').forEach((b) => (b.onclick = async () => {
    const id = b.dataset.id;
    if (id === 'play') { closeModal(); launch(g); }
    if (id === 'edit') openEditor(g);
    if (id === 'folder') { closeModal(); ipc.showItem(g.path); }
    if (id === 'first') { closeModal(); moveToFront(g); }
    if (id === 'close') { closeModal(); confirmCloseGame(g); }
    if (id === 'reorder') { closeModal(); startReorder(); }
    if (id === 'bg' || id === 'bgReset') {
      let url = '';
      if (id === 'bg') {
        const p = await ipc.openFile('background');
        if (!p) return;
        url = await ipc.importMedia(p);
        bgFailed.delete(url);
      }
      g.background = url;
      await save();
      closeModal();
      update(true);
      toast(url ? 'Фон установлен' : 'Свой фон убран');
    }
    if (id === 'move') {
      g.category = g.category === 'app' ? 'game' : 'app';
      await save();
      closeModal();
      render();
      toast(`«${g.name}» перемещена`);
    }
    if (id === 'del') confirmDelete(g);
  }));
}

function confirmDelete(g) {
  openModal(`<h2>Удалить «${esc(g.name)}»?</h2>
    <div class="meta"><span>Игра будет удалена только из лаунчера, файлы на диске останутся.</span></div>
    <div class="buttons"><button class="btn focusable" id="bNo">Отмена</button><button class="btn danger focusable" id="bYes">Удалить</button></div>`,
  { onBack: openOptions });
  $('#bNo').onclick = closeModal;
  $('#bYes').onclick = async () => {
    data.games = data.games.filter((x) => x.id !== g.id);
    await save();
    closeModal();
    render();
    toast('Удалено');
  };
}

// ---------- Search ----------
function openSearch() {
  openModal(`<h2>Поиск</h2>
    <div class="field"><input class="focusable" id="fSearch" placeholder="Название игры или приложения"/></div>
    <div class="steam-list" id="results"></div>`);
  const show = () => {
    const q = $('#fSearch').value.trim().toLowerCase();
    const found = data.games.filter((g) => !q || g.name.toLowerCase().includes(q)).sort((a, b) => a.name.localeCompare(b.name));
    $('#results').innerHTML = found.length
      ? found.map((g) => `<button class="steam-item focusable" data-id="${g.id}">${g.cover ? `<img src="${esc(g.cover)}"/>` : ''}${esc(g.name)}<span class="val" style="margin-left:auto">${g.category === 'app' ? 'Приложение' : 'Игра'}</span></button>`).join('')
      : '<div class="meta"><span>Ничего не найдено</span></div>';
    $$('#results img').forEach((img) => (img.onerror = () => (img.style.visibility = 'hidden')));
    $$('#results .steam-item').forEach((b) => (b.onclick = () => {
      const g = data.games.find((x) => x.id === b.dataset.id);
      closeModal();
      tab = 'game';
      zone = 'row';
      render(g.id);
    }));
  };
  $('#fSearch').addEventListener('input', show);
  show();
}

// ---------- Settings ----------
function openSettings(focus = 0) {
  const s = data.settings;
  const onoff = (v) => (v ? 'Вкл' : 'Выкл');
  const items = [
    ['user', 'Имя пользователя', s.user],
    ['steam', 'Импорт игр из Steam', ''],
    ['account', 'Аккаунт Steam (трофеи, друзья)', s.steamApiKey ? 'Подключён' : 'Не подключён'],
    ['discord', 'Статус в Discord «Играет в…»', onoff(!!s.discordId)],
    ['sort', 'Порядок игр', SORT_NAMES[s.sortMode || 'recent']],
    ['sep'],
    ['wallpaper', 'Общие фоны (картинки или видео)', s.wallpapers?.length ? `${s.wallpapers.length} шт.` : 'Стандартный'],
    ['slideshow', 'Смена общего фона', SLIDE_NAMES[s.slideshow || 0]],
    ['bgMode', 'Фон на играх', s.bgMode === 'global' ? 'Общий фон' : 'Арт игры'],
    ['trailers', 'Живые фоны (трейлеры игр)', onoff(s.liveTrailers)],
    ['sep'],
    ['layout', 'Кнопки геймпада', LAYOUT_NAMES[s.padLayout || 'auto']],
    ['sounds', 'Звуки интерфейса', onoff(s.sounds)],
    ['fullscreen', 'Полноэкранный режим', onoff(s.fullscreen)],
    ['autostart', 'Запускать вместе с Windows', onoff(s.autostart)],
    ['tray', 'При закрытии окна', s.closeToTray === false ? 'Выйти' : 'Свернуть в трей'],
    ['sep'],
    ['update', 'Проверить обновления', updateState.latest && updateState.status === 'available' ? `Доступна ${updateState.latest}` : `Версия ${appVersion}`],
    ['power', 'Питание', 'сон, перезагрузка, выключение', 'danger']
  ];
  openModal(`<h2 style="padding:10px 18px 0">Настройки</h2>` + items.map(([id, label, val, cls]) => (id === 'sep'
    ? '<div class="menu-sep"></div>'
    : `<button class="menu-item focusable ${cls || ''}" data-id="${id}">${esc(label)}<span class="val">${esc(val)}</span></button>`)).join(''),
  { cls: 'menu', focus });
  $$('#modal .menu-item').forEach((b, i) => (b.onclick = async () => {
    const id = b.dataset.id;
    const toggle = async (k, after) => { s[k] = !s[k]; await save(); after?.(s[k]); openSettings(i); };
    if (id === 'user') return renameUser();
    if (id === 'steam') return importSteam();
    if (id === 'layout') {
      const order = ['auto', 'ps', 'xbox'];
      s.padLayout = order[(order.indexOf(s.padLayout || 'auto') + 1) % order.length];
      await save();
      renderHints();
      return openSettings(i);
    }
    if (id === 'sounds') return toggle('sounds');
    if (id === 'fullscreen') return toggle('fullscreen', ipc.setFullscreen);
    if (id === 'account') return openSteamAccount();
    if (id === 'tray') {
      s.closeToTray = s.closeToTray === false;
      await save();
      return openSettings(i);
    }
    if (id === 'discord') {
      await toggleDiscord();
      return openSettings(i);
    }
    if (id === 'power') return openPower();
    if (id === 'sort') {
      const order = ['recent', 'alpha', 'custom'];
      s.sortMode = order[(order.indexOf(s.sortMode || 'recent') + 1) % order.length];
      await save();
      render(list[sel]?.id);
      return openSettings(i);
    }
    if (id === 'update') return openUpdate();
    if (id === 'trailers') {
      s.liveTrailers = !s.liveTrailers;
      await save();
      update(true);
      return openSettings(i);
    }
    if (id === 'wallpaper') return openWallpapers();
    if (id === 'slideshow') {
      const steps = [0, 30, 60, 300, 900];
      s.slideshow = steps[(steps.indexOf(s.slideshow || 0) + 1) % steps.length];
      await save();
      return openSettings(i);
    }
    if (id === 'bgMode') {
      s.bgMode = s.bgMode === 'global' ? 'game' : 'global';
      await save();
      update(true);
      return openSettings(i);
    }
    if (id === 'autostart') return toggle('autostart', ipc.setAutostart);
    if (id === 'quit') return ipc.quit();
  }));
}

// ---------- Updates ----------
let appVersion = '';
let updateState = { status: null };
async function openUpdate() {
  const back = () => openSettings(0);
  openModal('<h2>Обновления</h2><div class="meta"><span>Проверяем GitHub…</span></div>', { onBack: back });
  updateState = await ipc.checkUpdate();
  showUpdate();
}
function showUpdate(progress) {
  const u = updateState;
  const back = () => openSettings(0);
  const text = {
    dev: 'Обновления работают только в установленной версии лаунчера.',
    latest: `У вас последняя версия ${u.current}.`,
    available: `Доступна версия ${u.latest} (у вас ${u.current}).`,
    downloading: `Загрузка версии ${u.latest}… ${progress ?? 0}%`,
    ready: `Версия ${u.latest} загружена. Лаунчер перезапустится и обновится.`,
    error: `Не удалось проверить обновления: ${/\b404\b/.test(u.error || '') ? 'релизы на GitHub не найдены' : /ENOTFOUND|ETIMEDOUT|net::/i.test(u.error || '') ? 'нет соединения с GitHub' : (u.error || '').split('\n')[0].slice(0, 160)}`
  }[u.status] || '';
  const notes = typeof u.notes === 'string' ? u.notes.replace(/<[^>]+>/g, '').trim() : '';
  const btn = {
    available: '<button class="btn primary focusable" id="bUpd">Скачать и установить</button>',
    ready: '<button class="btn primary focusable" id="bUpd">Перезапустить</button>',
    error: '<button class="btn focusable" id="bUpd">Повторить</button>'
  }[u.status] || '';
  openModal(`<h2>Обновления</h2>
    <div class="meta"><span>${esc(text)}</span></div>
    ${u.status === 'downloading' ? `<div class="launch-bar" style="margin:18px 0 0"><i style="animation:none;width:${progress ?? 0}%"></i></div>` : ''}
    ${notes && u.status === 'available' ? `<div class="nm-text" style="margin-top:14px;max-height:30vh">${esc(notes)}</div>` : ''}
    <div class="buttons"><button class="btn focusable" id="bBack">Назад</button>${btn}</div>`,
  { onBack: back, focus: btn ? 1 : 0 });
  $('#bBack').onclick = back;
  const b = $('#bUpd');
  if (!b) return;
  b.onclick = async () => {
    if (u.status === 'available') {
      updateState.status = 'downloading';
      showUpdate(0);
      ipc.downloadUpdate();
    } else if (u.status === 'ready') ipc.installUpdate();
    else openUpdate();
  };
}
ipc.onUpdateProgress((p) => {
  if (updateState.status !== 'downloading') return;
  if (modal.open && $('#modal h2')?.textContent === 'Обновления') showUpdate(p);
});
ipc.onUpdateReady(() => {
  updateState.status = 'ready';
  if (modal.open && $('#modal h2')?.textContent === 'Обновления') showUpdate();
  else toast('Обновление загружено — перезапустите лаунчер в настройках');
});
ipc.onUpdateError((e) => {
  if (updateState.status !== 'downloading') return;
  updateState = { ...updateState, status: 'error', error: e };
  if (modal.open) showUpdate();
});

function renameUser() {
  openModal(`<h2>Имя пользователя</h2>
    <div class="field"><input class="focusable" id="fUser" maxlength="24" value="${esc(data.settings.user)}"/></div>
    <div class="buttons"><button class="btn focusable" id="bNo">Отмена</button><button class="btn primary focusable" id="bOk">Сохранить</button></div>`,
  { onBack: () => openSettings(0) });
  $('#bNo').onclick = () => openSettings(0);
  $('#bOk').onclick = async () => {
    data.settings.user = $('#fUser').value.trim() || 'Player';
    await save();
    applyUser();
    openSettings(0);
  };
}

async function importSteam() {
  openModal('<h2>Поиск игр Steam…</h2><div class="meta"><span>Сканируем библиотеки</span></div>', { onBack: () => openSettings(1) });
  const res = await ipc.scanSteam();
  if (res.error) {
    openModal(`<h2>Steam</h2><div class="meta"><span>${esc(res.error)}</span></div><div class="buttons"><button class="btn focusable" id="bOk">Назад</button></div>`, { onBack: () => openSettings(1) });
    $('#bOk').onclick = () => openSettings(1);
    return;
  }
  const have = new Set(data.games.filter((g) => g.steamId).map((g) => g.steamId));
  const found = res.games.sort((a, b) => a.name.localeCompare(b.name));
  const fresh = found.filter((g) => !have.has(g.steamId));
  const picked = new Set(fresh.map((g) => g.steamId));
  openModal(`<h2>Найдено игр Steam: ${found.length}</h2>
    ${found.length ? '' : '<div class="meta"><span>Установленные игры не найдены</span></div>'}
    <div class="steam-list">${found.map((g) => `
      <button class="steam-item ${have.has(g.steamId) ? 'exists' : 'focusable on'}" data-id="${g.steamId}" ${have.has(g.steamId) ? 'disabled' : ''}>
        <img src="${esc(g.cover)}"/>${esc(g.name)}${have.has(g.steamId) ? '<span class="val" style="margin-left:auto">уже добавлена</span>' : '<span class="chk"></span>'}
      </button>`).join('')}</div>
    <div class="buttons"><button class="btn focusable" id="bNo">Отмена</button><button class="btn primary focusable" id="bOk">Импортировать (<span id="cnt">${picked.size}</span>)</button></div>`,
  { onBack: () => openSettings(1) });
  $$('#modal .steam-item img').forEach((img) => (img.onerror = () => (img.style.visibility = 'hidden')));
  $$('#modal .steam-item.focusable').forEach((b) => (b.onclick = () => {
    const id = b.dataset.id;
    picked.has(id) ? picked.delete(id) : picked.add(id);
    b.classList.toggle('on', picked.has(id));
    $('#cnt').textContent = picked.size;
  }));
  $('#bNo').onclick = () => openSettings(1);
  $('#bOk').onclick = async () => {
    const now = Date.now();
    fresh.filter((g) => picked.has(g.steamId)).forEach((g, i) => {
      data.games.push({ ...g, id: crypto.randomUUID(), category: 'game', args: '', added: now - i, logo: `https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/${g.steamId}/logo.png` });
    });
    await save();
    closeModal();
    tab = 'game';
    sel = 0;
    render();
    toast(`Импортировано игр: ${picked.size}`);
  };
}

// ---------- Input handling ----------
function handle(act, src) {
  if (busy) {
    if (act === 'back' && launching) { sfx.back(); launching.cancel(); }
    return;
  }
  // Home key = the Guide button: toggle the Control Center
  if (act === 'guide') {
    if (osk.open) closeOSK(false);
    if (cc.open) { sfx.back(); return closeCC(); }
    if (modal.open) closeModal();
    return pollMusic().finally(openCC);
  }
  if (osk.open) return oskHandle(act);
  if (callCardOpen()) return callHandle(act);
  if (groupRingOpen()) return groupRingHandle(act);
  if (act === 'lb') return;
  if (act === 'start') act = 'menu';
  if (modal.open) return modalHandle(act, src);
  if (cc.open) return ccHandle(act);
  if (profileOpen()) return profileHandle(act);
  if (socialOpen()) return socialHandle(act, src);
  if (reorder) return reorderHandle(act);
  const count = list.length + 1;
  const prevZone = zone;

  if (act === 'add') {
    sfx.ok();
    if (tab === 'activity') { openSocial(); social.idx = 1; return activateItem(); }
    return openEditor();
  }
  if (act === 'menu') { sfx.ok(); return openOptions(); }

  if (zone === 'row') {
    if (act === 'left' && sel > 0) { sel--; sfx.move(); }
    else if (act === 'right' && sel < count - 1) { sel++; sfx.move(); }
    else if (act === 'up') { zone = 'tabs'; tabFocus = TABS.indexOf(tab); }
    else if (act === 'down') { zone = 'actions'; actFocus = 0; }
    else if (act === 'ok') { sfx.ok(); return activate(); }
  } else if (zone === 'tabs') {
    if (act === 'left' && tabFocus > 0) { tabFocus--; sfx.move(); if (tabFocus < 2) switchTab(TABS[tabFocus]); }
    else if (act === 'right' && tabFocus < 5) { tabFocus++; sfx.move(); if (tabFocus < 2) switchTab(TABS[tabFocus]); }
    else if (act === 'down' || act === 'back') zone = 'row';
    else if (act === 'ok') {
      sfx.ok();
      if (tabFocus === 2) return openSearch();
      if (tabFocus === 3) return openSocial();
      if (tabFocus === 4) return openSettings();
      if (tabFocus === 5) return openProfile();
      switchTab(TABS[tabFocus]);
      zone = 'row';
    }
  } else if (zone === 'actions') {
    const n = list[sel] ? 2 : 1;
    if (act === 'left' && actFocus > 0) { actFocus--; sfx.move(); }
    else if (act === 'right' && actFocus < n - 1) { actFocus++; sfx.move(); }
    else if (act === 'up' || act === 'back') zone = 'row';
    else if (act === 'down') feedEnter();
    else if (act === 'ok') { sfx.ok(); return actFocus === 0 ? activate() : openOptions(); }
  } else if (zone === 'feed') {
    feedHandle(act);
    if (zone === 'feed' && act === 'ok') return;
  }
  if (zone !== prevZone) sfx.move();
  update();
}

const KEYMAP = {
  ArrowUp: 'up', ArrowDown: 'down', ArrowLeft: 'left', ArrowRight: 'right',
  Enter: 'ok', Escape: 'back', Backspace: 'back', ContextMenu: 'menu', Insert: 'add', Home: 'guide',
  q: 'menu', 'й': 'menu', n: 'add', 'т': 'add'
};
window.addEventListener('keydown', (e) => {
  const act = KEYMAP[e.key] || KEYMAP[e.key.toLowerCase()];
  if (!act) return;
  const inInput = document.activeElement?.tagName === 'INPUT';
  if (inInput && (act === 'left' || act === 'right' || act === 'menu' || act === 'add' || act === 'guide' || e.key === 'Backspace')) return;
  e.preventDefault();
  document.body.classList.remove('no-cursor');
  handle(act, 'key');
});

// Gamepad: A/✕=ok, B/○=back, X/□=add, Y/△ or Start=menu, D-pad / left stick = move
const PAD_BUTTONS = { 0: 'ok', 1: 'back', 2: 'add', 3: 'menu', 4: 'lb', 9: 'start', 12: 'up', 13: 'down', 14: 'left', 15: 'right' };
const PAD_ACTS = ['ok', 'back', 'add', 'menu', 'lb', 'start', 'up', 'down', 'left', 'right'];
const padState = {};
function pollPads() {
  const now = performance.now();
  for (const gp of navigator.getGamepads()) {
    if (!gp) continue;
    const st = (padState[gp.index] ??= {});
    const held = {};
    for (const [b, act] of Object.entries(PAD_BUTTONS)) if (gp.buttons[b]?.pressed) held[act] = true;
    const [ax = 0, ay = 0] = gp.axes;
    if (ax < -0.6) held.left = true;
    if (ax > 0.6) held.right = true;
    if (ay < -0.6) held.up = true;
    if (ay > 0.6) held.down = true;
    for (const act of PAD_ACTS) {
      if (!held[act]) { st[act] = 0; continue; }
      if (!st[act]) { st[act] = now + 380; padFire(act, gp); }
      else if (DIRS.has(act) && now >= st[act]) { st[act] = now + 110; padFire(act, gp); }
    }
  }
  requestAnimationFrame(pollPads);
}
// ---------- Button glyphs ----------
const LAYOUT_NAMES = { auto: 'Авто', ps: 'PlayStation', xbox: 'Xbox' };
const GLYPHS = {
  ps: { ok: ['✕', 'g-x'], back: ['○', 'g-o'], menu: ['△', 'g-t'], add: ['□', 'g-s'] },
  xbox: { ok: ['A', 'xb g-xa'], back: ['B', 'xb g-xb'], menu: ['Y', 'xb g-xy'], add: ['X', 'xb g-xx'] }
};
let detectedLayout = 'ps';
function currentLayout() {
  const l = data.settings.padLayout || 'auto';
  return l === 'auto' ? detectedLayout : l;
}
function renderHints() {
  const g = GLYPHS[currentLayout()];
  const hint = (k, label) => `<span><i class="g ${g[k][1]}">${g[k][0]}</i> ${label}</span>`;
  $('#hints').innerHTML = hint('ok', 'Выбрать') + hint('back', 'Назад') + hint('menu', 'Параметры') + hint('add', 'Добавить');
}
function detectLayout(gp) {
  const id = gp.id.toLowerCase();
  const l = /054c|dualsense|dualshock|wireless controller|playstation/.test(id) ? 'ps' : /xbox|xinput|045e/.test(id) ? 'xbox' : null;
  if (l && l !== detectedLayout) {
    detectedLayout = l;
    renderHints();
  }
}
window.addEventListener('gamepadconnected', (e) => detectLayout(e.gamepad));

function padFire(act, gp) {
  if (!document.hasFocus()) return;
  if (gp) detectLayout(gp);
  document.body.classList.add('no-cursor');
  handle(act, 'pad');
}

window.addEventListener('mousemove', () => document.body.classList.remove('no-cursor'));

// Mouse
$('#track').addEventListener('click', (e) => {
  const t = e.target.closest('.tile');
  if (!t) return;
  const i = +t.dataset.i;
  zone = 'row';
  if (i === sel) { sfx.ok(); activate(); }
  else { sel = i; sfx.move(); update(); }
});
$('#track').addEventListener('contextmenu', (e) => {
  const t = e.target.closest('.tile');
  if (!t) return;
  sel = +t.dataset.i;
  zone = 'row';
  update();
  openOptions();
});
$('#row').addEventListener('wheel', (e) => {
  if (modal.open) return;
  zone = 'row';
  handle(e.deltaY > 0 || e.deltaX > 0 ? 'right' : 'left', 'mouse');
}, { passive: true });
$$('.tabs .tab').forEach((t) => (t.onclick = () => { zone = 'row'; switchTab(t.dataset.tab); sfx.move(); update(); }));
$('[data-action="settings"]').onclick = () => { sfx.ok(); openSettings(); };
$('[data-action="search"]').onclick = () => { sfx.ok(); openSearch(); };
$('[data-action="friends"]').onclick = () => { sfx.ok(); openSocial(); };
$('[data-action="profile"]').onclick = () => { sfx.ok(); openProfile(); };
$('#btnPlay').onclick = () => { sfx.ok(); activate(); };
$('#btnMore').onclick = () => { sfx.ok(); openOptions(); };

window.addEventListener('resize', () => update());

// ---------- Clock / user ----------
function tick() {
  $('#clock').textContent = new Date().toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
}
function applyUser() {
  $('#avatar').textContent = (data.settings.user || 'P')[0].toUpperCase();
  $('#avatar').title = data.settings.user;
}

// ---------- Boot ----------
// wait for feed.js / overlays.js / social.js too — the IPC reply can otherwise beat their loading
window.addEventListener('DOMContentLoaded', async () => {
  const m = document.createElement('div');
  m.id = 'measure';
  m.style.cssText = 'position:absolute;visibility:hidden;pointer-events:none;width:var(--tile);height:var(--gap)';
  document.body.appendChild(m);

  data = await ipc.load();
  data.sessions ||= [];
  // single shared wallpaper → list of wallpapers (slideshow)
  if (data.settings.wallpaper && !data.settings.wallpapers) data.settings.wallpapers = [data.settings.wallpaper];
  // older versions stored a user-picked background in `hero`; Steam art keeps using `hero`
  for (const g of data.games) {
    if (g.hero && !steamIdOf(g) && !g.background) { g.background = g.hero; delete g.hero; }
  }
  appVersion = (await ipc.version()) || '';
  steamLocal = (await ipc.steamLocal()) || steamLocal;
  const runningNow = String((await ipc.steamRunning()) || '');
  if (runningNow !== '0' && runningNow) {
    steamSince[runningNow] = Date.now();
    for (const g of data.games.filter((x) => steamIdOf(x) === runningNow)) running.add(g.id);
  }
  applyUser();
  renderHints();
  tick();
  setInterval(tick, 10000);
  feedInit();
  render();
  socialStart();
  profileInit();
  requestAnimationFrame(pollPads);
  // fade in once the first frame is laid out
  setTimeout(() => document.body.classList.remove('booting'), 120);

  // quiet update check shortly after start
  setTimeout(async () => {
    const u = await ipc.checkUpdate();
    if (u?.status === 'available') {
      updateState = u;
      toast(`Доступно обновление ${u.latest} — Настройки → Проверить обновления`);
    }
  }, 8000);
});
