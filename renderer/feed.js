'use strict';
// Home feed below the game hub (scroll down from the Play button, like the PS5 game hub):
// news, trophies, continue playing, stats, friends, music, Steam sales, PC status.

const FEED = [
  { key: 'news', title: 'Новости' },
  { key: 'trophies', title: 'Трофеи' },
  { key: 'continue', title: 'Продолжить' },
  { key: 'stats', title: 'Статистика' },
  { key: 'friends', title: 'Друзья' },
  { key: 'music', title: 'Сейчас играет' },
  { key: 'sales', title: 'Скидки Steam' },
  { key: 'pc', title: 'Состояние ПК' }
];
const feed = { cards: {}, idx: {}, sec: 0, titles: {} };

const visibleSecs = () => FEED.filter((s) => feed.cards[s.key]?.length);
const apiKey = () => (data.settings.steamApiKey || '').trim();
const steamUser = () => (data.settings.steamId || '').trim() || steamLocal.steamId || '';

function feedInit() {
  $('#feed').innerHTML = FEED.map((s) => `
    <section class="fsec hidden" data-key="${s.key}">
      <div class="fhead">${s.title}</div>
      <div class="ftrack"></div>
    </section>`).join('');
  refreshSales();
  setInterval(refreshSales, 30 * 60e3);
  refreshFriends();
  setInterval(() => !document.hidden && refreshFriends(), 60e3);
  refreshPc();
  setInterval(() => !document.hidden && refreshPc(), 3000);
  pollMusic();
  setInterval(() => !document.hidden && pollMusic(), 2000);
}

// cards: [{ html, cls, ok }]. Same number of cards → update in place so entry animations don't replay.
function setSection(key, cards, { title, after } = {}) {
  const sec = $(`.fsec[data-key="${key}"]`);
  if (!sec) return;
  feed.cards[key] = cards;
  if (title !== undefined) $('.fhead', sec).textContent = title;
  const track = $('.ftrack', sec);
  const els = [...track.children];
  if (els.length === cards.length && els.length) {
    cards.forEach((c, i) => {
      const cls = 'fcard ' + (c.cls || '');
      if (els[i].className.replace(/ ?focus/, '') !== cls.trim()) els[i].className = cls;
      if (els[i].innerHTML !== c.html) els[i].innerHTML = c.html;
    });
  } else {
    track.innerHTML = cards.map((c, i) => `<div class="fcard ${c.cls || ''}" data-i="${i}" style="animation-delay:${Math.min(i, 8) * 50}ms">${c.html}</div>`).join('');
    [...track.children].forEach((el, i) => (el.onclick = () => {
      const s = visibleSecs().findIndex((x) => x.key === key);
      zone = 'feed';
      feed.sec = s;
      feed.idx[key] = i;
      update();
      sfx.ok();
      feed.cards[key][i]?.ok?.();
    }));
  }
  sec.classList.toggle('hidden', !cards.length);
  feed.idx[key] = Math.min(feed.idx[key] || 0, Math.max(0, cards.length - 1));
  after?.([...track.children]);
  if (zone === 'feed') {
    const n = visibleSecs().length;
    if (!n) zone = 'actions';
    else feed.sec = Math.min(feed.sec, n - 1);
  }
  feedUpdateFocus();
}

function feedUpdateFocus() {
  const secs = visibleSecs();
  const cur = zone === 'feed' ? secs[feed.sec] : null;
  for (const s of FEED) {
    const el = $(`.fsec[data-key="${s.key}"]`);
    if (!el) continue;
    const cards = [...$('.ftrack', el).children];
    cards.forEach((c, i) => c.classList.toggle('focus', cur?.key === s.key && i === (feed.idx[s.key] || 0)));
    const f = cards[feed.idx[s.key] || 0];
    if (cur?.key === s.key && f) {
      const track = $('.ftrack', el);
      track.scrollTo({ left: Math.max(0, f.offsetLeft - track.offsetLeft - f.offsetWidth * 0.6), behavior: 'smooth' });
    }
  }
  // scroll the whole screen so the focused section sits under the top bar
  let shift = 0;
  if (cur && feed.sec > 0) {
    const secEl = $(`.fsec[data-key="${cur.key}"]`);
    shift = $('#feed').offsetTop + secEl.offsetTop - innerHeight * 0.15;
  }
  $('main').style.transform = shift > 0 ? `translateY(${-shift}px)` : '';
  $('main').classList.toggle('scrolled', shift > 0);
  $('#hints').classList.toggle('away', shift > 0);
  $('.topbar').classList.toggle('scrolled', shift > 0);
  // sections above the focused one fade out instead of sliding under the top bar
  const curIdx = cur ? FEED.findIndex((s) => s.key === cur.key) : -1;
  FEED.forEach((s, i) => $(`.fsec[data-key="${s.key}"]`)?.classList.toggle('above', shift > 0 && i < curIdx));
}

function feedEnter() {
  if (!visibleSecs().length) return false;
  zone = 'feed';
  feed.sec = 0;
  return true;
}

function feedHandle(act) {
  const secs = visibleSecs();
  const s = secs[feed.sec];
  if (!s) { zone = 'actions'; return; }
  const n = feed.cards[s.key].length;
  const i = feed.idx[s.key] || 0;
  if (act === 'left' && i > 0) { feed.idx[s.key] = i - 1; sfx.move(); }
  else if (act === 'right' && i < n - 1) { feed.idx[s.key] = i + 1; sfx.move(); }
  else if (act === 'up') { sfx.move(); if (feed.sec > 0) feed.sec--; else zone = 'actions'; }
  else if (act === 'down' && feed.sec < secs.length - 1) { feed.sec++; sfx.move(); }
  else if (act === 'back') zone = 'actions';
  else if (act === 'ok') { const c = feed.cards[s.key][i]; if (c?.ok) { sfx.ok(); c.ok(); } }
}

// ---------- per-game sections ----------
let feedGame = null;
let feedTimer;
function feedOnSelect(g) {
  const key = g ? g.id + (steamIdOf(g) || '') : null;
  if (key === feedGame) return;
  feedGame = key;
  clearTimeout(feedTimer);
  setSection('news', []);
  setSection('trophies', []);
  const sid = steamIdOf(g);
  if (!sid) return;
  feedTimer = setTimeout(() => {
    loadNews(g, sid, key);
    loadTrophies(g, sid, key);
  }, 350);
}

async function loadNews(g, sid, key) {
  const items = await ipc.news(sid);
  if (feedGame !== key || !items?.length) return;
  const fallback = g.hero || g.cover || '';
  setSection('news', items.map((n) => ({
    cls: 'news',
    html: `<div class="img"></div><div class="t">${esc(n.title)}</div><div class="d">${esc([n.feed, fmtDate(n.date)].filter(Boolean).join(' · '))}</div>`,
    ok: () => openNews(n, g)
  })), {
    after: (els) => els.forEach((el, i) => {
      const img = $('.img', el);
      const src = items[i].image;
      // show the game art immediately, swap to the article image once it has loaded
      if (fallback) img.style.backgroundImage = cssUrl(fallback);
      if (!src) return;
      const im = new Image();
      im.onload = () => (img.style.backgroundImage = cssUrl(src));
      im.src = src;
    })
  });
}

function openNews(n, g) {
  const img = n.image || g?.hero || g?.cover;
  openModal(`
    <div class="nm-hero" ${img ? `style="background-image:${esc(cssUrl(img))}"` : ''}></div>
    <div class="nm-body">
      <div class="nm-meta">${esc([g?.name, n.feed, fmtDate(n.date)].filter(Boolean).join(' · '))}</div>
      <h2>${esc(n.title)}</h2>
      <div class="nm-text" id="nmText">${esc(n.text || 'Текст новости недоступен — откройте её в браузере.')}</div>
      <div class="buttons">
        <button class="btn focusable" id="bClose">Закрыть</button>
        <button class="btn primary focusable" id="bWeb">Открыть в браузере</button>
      </div>
    </div>`, { cls: 'news-modal', scroll: '#nmText', focus: 1 });
  $('#bClose').onclick = closeModal;
  $('#bWeb').onclick = () => { ipc.openUrl(n.url); closeModal(); };
}

const keyCard = (text) => ({
  cls: 'wide prompt',
  html: `<div class="pr-ic">🔑</div><div><div class="t">${text}</div><div class="d">Нажмите, чтобы указать Steam API-ключ</div></div>`,
  ok: openSteamAccount
});

// A card explaining why Steam data is missing, instead of silently hiding the section
function steamErrorCard(error, what) {
  const privacy = () => ipc.openUrl('https://steamcommunity.com/my/edit/settings');
  const c = {
    badkey: ['🔑', 'Steam не принял API-ключ', 'Проверьте ключ в Настройках → Аккаунт Steam', openSteamAccount],
    private: ['🔒', what === 'friends' ? 'Список друзей Steam скрыт' : 'Сведения об играх скрыты',
      what === 'friends' ? 'Откройте «Список друзей» в настройках приватности Steam' : 'Откройте «Сведения об играх» в настройках приватности Steam', privacy],
    network: ['📡', 'Нет связи со Steam', 'Попробуем ещё раз позже', null],
    error: ['⚠️', 'Steam вернул ошибку', 'Проверьте аккаунт в Настройках → Аккаунт Steam', openSteamAccount]
  }[error] || null;
  if (!c) return null;
  return {
    cls: 'wide prompt',
    html: `<div class="pr-ic">${c[0]}</div><div><div class="t">${c[1]}</div><div class="d">${c[2]}</div></div>`,
    ok: c[3]
  };
}

async function loadTrophies(g, sid, key) {
  if (!apiKey()) return setSection('trophies', [keyCard('Трофеи и друзья Steam')]);
  if (!steamUser()) return;
  const r = await ipc.achievements({ appid: sid, key: apiKey(), steamId: steamUser() });
  if (feedGame !== key || !r) return;
  if (r.error === 'none' || (!r.error && !r.total)) return setSection('trophies', []);
  if (r.error) return setSection('trophies', [steamErrorCard(r.error, 'trophies') || steamErrorCard('error')]);
  noteTrophy(g.name, sid, r.items, false);

  const pct = Math.round((100 * r.done) / r.total);
  const C = 2 * Math.PI * 34;
  const summary = {
    cls: 'wide trophy-sum',
    html: `<svg class="ring" viewBox="0 0 80 80"><circle cx="40" cy="40" r="34" class="bgc"/><circle cx="40" cy="40" r="34" class="fg" stroke-dasharray="${(C * pct) / 100} ${C}"/></svg>
      <div class="ring-n">${pct}%</div>
      <div><div class="big">${r.done} <span>/ ${r.total}</span></div><div class="d">трофеев получено</div>
      <div class="d">${r.done === r.total ? '🏆 Платина!' : `Осталось ${r.total - r.done}`}</div></div>`,
    ok: () => openTrophyList(g, r)
  };
  const got = r.items.filter((a) => a.achieved);
  const recent = [...got].sort((a, b) => b.time - a.time).slice(0, 5);
  const rare = [...got].filter((a) => a.percent != null && !recent.includes(a)).sort((a, b) => a.percent - b.percent).slice(0, 3);
  const card = (a, tag) => ({
    cls: 'trophy',
    html: `<div class="ti" style="background-image:${a.icon ? esc(cssUrl(a.icon)) : 'none'}"></div>
      <div class="t">${esc(a.name)}</div>
      <div class="d">${tag || (a.percent != null ? `${a.percent.toFixed(1)}% игроков` : '')}</div>`,
    ok: () => openTrophy(a)
  });
  setSection('trophies', [
    summary,
    ...recent.map((a) => card(a, fmtDate(a.time))),
    ...rare.map((a) => card(a, `Редкий · ${a.percent.toFixed(1)}%`))
  ]);
}

function openTrophy(a) {
  openModal(`<div class="trophy-modal">
      <div class="ti big" style="background-image:${a.icon ? esc(cssUrl(a.icon)) : 'none'}"></div>
      <h2>${esc(a.name)}</h2>
      <div class="meta"><span>${esc(a.desc || 'Скрытый трофей')}</span></div>
      <div class="meta"><span>${a.achieved ? 'Получен ' + fmtDate(a.time) : 'Не получен'}</span>${a.percent != null ? `<span>Есть у ${a.percent.toFixed(1)}% игроков</span>` : ''}</div>
    </div>
    <div class="buttons"><button class="btn focusable" id="bOk">Закрыть</button></div>`);
  $('#bOk').onclick = closeModal;
}

function openTrophyList(g, r) {
  const items = [...r.items].sort((a, b) => b.achieved - a.achieved || b.time - a.time || (b.percent ?? 0) - (a.percent ?? 0));
  openModal(`<h2>${esc(g.name)} · ${r.done}/${r.total}</h2>
    <div class="steam-list">${items.map((a) => `
      <button class="steam-item focusable ${a.achieved ? '' : 'locked'}">
        <img src="${esc(a.icon || '')}"/><span><b>${esc(a.name)}</b><br><small>${esc(a.desc || '')}</small></span>
        <span class="val" style="margin-left:auto">${a.achieved ? fmtDate(a.time) : a.percent != null ? a.percent.toFixed(1) + '%' : ''}</span>
      </button>`).join('')}</div>
    <div class="buttons"><button class="btn focusable" id="bOk">Закрыть</button></div>`);
  $$('#modal .steam-item img').forEach((img) => (img.onerror = () => (img.style.visibility = 'hidden')));
  $('#bOk').onclick = closeModal;
}

// ---------- global sections ----------
function refreshContinue() {
  const recent = data.games
    .filter((g) => lastPlayedOf(g) > 0)
    .sort((a, b) => lastPlayedOf(b) - lastPlayedOf(a))
    .slice(0, 6);
  setSection('continue', recent.map((g) => {
    const bg = g.background && !isVideo(g.background) ? g.background : g.hero || g.cover;
    const sub = [running.has(g.id) ? '● Запущено' : fmtDate(lastPlayedOf(g)), totalPlaytime(g) ? fmtPlaytime(totalPlaytime(g)) : ''].filter(Boolean).join(' · ');
    return {
      cls: 'wide cont' + (running.has(g.id) ? ' live' : ''),
      html: `<div class="cimg" style="background-image:${bg ? esc(cssUrl(bg)) : 'none'}"></div>
        <div class="cbody">${g.logo ? `<img class="clogo" src="${esc(g.logo)}">` : ''}
        <div class="t">${esc(g.name)}</div><div class="d">${esc(sub)}</div></div>`,
      ok: () => { tab = 'game'; zone = 'actions'; render(g.id); }
    };
  }), {
    // no logo art → fall back to the plain title
    after: (els) => els.forEach((el) => {
      const logo = $('.clogo', el);
      if (!logo) return;
      const ok = () => el.classList.add('has-logo');
      logo.onerror = () => { logo.remove(); el.classList.remove('has-logo'); };
      if (logo.complete && logo.naturalWidth) ok(); else logo.onload = ok;
    })
  });
}

function refreshStats() {
  const DAY = 86400e3;
  const today = new Date(); today.setHours(0, 0, 0, 0);
  const days = Array.from({ length: 7 }, (_, i) => ({ t: today.getTime() - (6 - i) * DAY, s: 0 }));
  let month = 0;
  let monthSessions = 0;
  for (const s of data.sessions || []) {
    const dur = Math.max(0, (s.end - s.start) / 1000);
    if (s.start >= today.getTime() - 29 * DAY) { month += dur; monthSessions++; }
    const d = days.find((x) => s.start >= x.t && s.start < x.t + DAY);
    if (d) d.s += dur;
  }
  const week = days.reduce((a, d) => a + d.s, 0);
  const max = Math.max(...days.map((d) => d.s), 1);
  const wd = ['вс', 'пн', 'вт', 'ср', 'чт', 'пт', 'сб'];
  const bars = days.map((d, i) => {
    const h = Math.max(3, (d.s / max) * 46);
    return `<rect x="${i * 20 + 3}" y="${50 - h}" width="12" height="${h}" rx="3" class="${i === 6 ? 'today' : ''}"/><text x="${i * 20 + 9}" y="62">${wd[new Date(d.t).getDay()]}</text>`;
  }).join('');
  const fav = [...data.games].sort((a, b) => totalPlaytime(b) - totalPlaytime(a))[0];
  const total = data.games.reduce((a, g) => a + totalPlaytime(g), 0);
  const cards = [
    { cls: 'wide stat', html: `<div><div class="d">Последние 7 дней</div><div class="big">${hours(week)}</div><div class="d">по данным лаунчера</div></div><svg class="bars" viewBox="0 0 140 66">${bars}</svg>` },
    { cls: 'stat', html: `<div class="d">За 30 дней</div><div class="big">${hours(month)}</div><div class="d">${monthSessions} ${plural(monthSessions, 'сессия', 'сессии', 'сессий')}</div>` },
    { cls: 'stat', html: `<div class="d">Всего наиграно</div><div class="big">${hours(total)}</div><div class="d">${data.games.length} ${plural(data.games.length, 'игра', 'игры', 'игр')} в библиотеке</div>` }
  ];
  if (fav && totalPlaytime(fav)) {
    cards.splice(1, 0, {
      cls: 'wide stat fav',
      html: `<div class="fimg" style="background-image:${fav.cover ? esc(cssUrl(fav.cover)) : 'none'}"></div><div><div class="d">Любимая игра</div><div class="t">${esc(fav.name)}</div><div class="big">${hours(totalPlaytime(fav))}</div></div>`,
      ok: () => { tab = 'game'; zone = 'actions'; render(fav.id); }
    });
  }
  setSection('stats', data.games.length ? cards : []);
}

let steamFriendsCache = null;
// Launcher friends (who's online / playing what) first, then Steam friends; cards open the friend's profile
async function refreshFriends() {
  const lf = (typeof acct === 'function' && acct()?.friends) || [];
  const lfOnline = lf.map((f) => ({ f, p: presenceOf(f.pk) })).filter((x) => x.p.online).sort((a, b) => b.p.game - a.p.game);
  const lfCards = lfOnline.map(({ f, p }) => {
    const h = hue(f.pk);
    return {
      cls: 'friend',
      html: `<div class="av letter" style="background:linear-gradient(135deg,hsl(${h} 65% 45%),hsl(${(h + 60) % 360} 65% 30%))">${esc(f.name[0].toUpperCase())}<i class="${p.game ? 'game' : 'on'}"></i></div>
        <div class="t">${esc(f.name)}</div><div class="d"><span class="st ${p.game ? 'game' : 'on'}">${esc(p.game ? p.text.replace(/^В игре: /, '') : 'В сети')}</span></div>`,
      ok: () => openProfile({ kind: 'friend', id: f.pk })
    };
  });

  let steamCards = [];
  let steamOnline = 0;
  let err = null;
  if (apiKey() && steamUser()) {
    const r = await ipc.friends({ key: apiKey(), steamId: steamUser() });
    if (r?.error) {
      // keep showing the last good list on a network hiccup
      if (!(r.error === 'network' && steamFriendsCache)) err = steamErrorCard(r.error, 'friends');
    } else if (r?.list) steamFriendsCache = r.list;
    const list = steamFriendsCache || [];
    const online = list.filter((f) => f.state > 0 || f.game).sort((a, b) => !!b.game - !!a.game || (a.state === 1 ? -1 : 1) - (b.state === 1 ? -1 : 1));
    steamOnline = online.length;
    const offline = list.length - online.length;
    const status = (f) => f.game ? `<span class="st game">${esc(f.game)}</span>` : f.state === 1 ? '<span class="st on">В сети</span>' : '<span class="st away">Нет на месте</span>';
    steamCards = online.map((f) => ({
      cls: 'friend',
      html: `<div class="av" style="background-image:${esc(cssUrl(f.avatar))}"><i class="${f.game ? 'game' : f.state === 1 ? 'on' : 'away'}"></i></div><div class="t">${esc(f.name)}</div><div class="d">${status(f)}</div>`,
      ok: () => openProfile({ kind: 'steam', id: f.id })
    }));
    if (offline) steamCards.push({ cls: 'friend more', html: `<div class="av plus">+${offline}</div><div class="t">Не в сети</div><div class="d">открыть список друзей</div>`, ok: () => ipc.openUrl('steam://open/friends') });
  }
  const playing = lfOnline.filter((x) => x.p.game).length + (steamFriendsCache || []).filter((f) => f.game).length;
  const total = lfOnline.length + steamOnline;
  // the Activity tab is built from the same data
  if (tab === 'activity' && !busy) render(list[sel]?.id);
  setSection('friends', [...lfCards, ...(err ? [err] : []), ...steamCards], {
    title: total ? `Друзья · ${total} в сети${playing ? ` · ${playing} ${plural(playing, 'играет', 'играют', 'играют')}` : ''}` : 'Друзья'
  });
}

async function refreshSales() {
  const items = await ipc.sales(steamUser() || null);
  if (!items?.length) return;
  const money = (v, cur) => {
    try { return new Intl.NumberFormat('ru-RU', { style: 'currency', currency: cur || 'RUB', maximumFractionDigits: 0 }).format(v / 100); } catch { return (v / 100).toFixed(0); }
  };
  setSection('sales', items.slice(0, 14).map((s) => ({
    cls: 'sale',
    html: `<div class="simg" style="background-image:${esc(cssUrl(s.image))}"><b class="disc">−${s.discount}%</b>${s.wish ? '<b class="wish">♥ Желаемое</b>' : ''}</div>
      <div class="t">${esc(s.name)}</div>
      <div class="d"><span class="price">${money(s.price, s.currency)}</span> <s>${money(s.old, s.currency)}</s></div>`,
    ok: () => ipc.openUrl(`steam://store/${s.id}`)
  })));
}

async function refreshPc() {
  const s = await ipc.sysStats();
  if (!s) return;
  const gb = (b) => (b / 1024 ** 3).toFixed(b > 100 * 1024 ** 3 ? 0 : 1);
  const bar = (p, warn) => `<div class="meter"><i style="width:${Math.min(100, Math.max(0, p))}%" class="${p >= (warn || 85) ? 'hot' : ''}"></i></div>`;
  const cards = [];
  if (s.cpu.load != null) {
    cards.push({ cls: 'pc', html: `<div class="d">Процессор</div><div class="big">${s.cpu.load}%</div>${bar(s.cpu.load)}<div class="d ell">${esc(s.cpu.name)}</div>` });
  }
  const ramP = Math.round((100 * s.ram.used) / s.ram.total);
  cards.push({ cls: 'pc', html: `<div class="d">Память</div><div class="big">${gb(s.ram.used)} <span>/ ${gb(s.ram.total)} ГБ</span></div>${bar(ramP)}<div class="d">занято ${ramP}%</div>` });
  if (s.gpu) {
    const vram = Math.round((100 * s.gpu.memUsed) / s.gpu.memTotal);
    cards.push({ cls: 'pc', html: `<div class="d">Видеокарта · ${s.gpu.temp}°C</div><div class="big">${s.gpu.load}%</div>${bar(s.gpu.load)}<div class="d ell">${esc(s.gpu.name)} · VRAM ${vram}%</div>` });
  }
  for (const d of s.disks) {
    const used = Math.round((100 * (d.total - d.free)) / d.total);
    cards.push({ cls: 'pc', html: `<div class="d">Диск ${d.drive}</div><div class="big">${gb(d.free)} <span>ГБ свободно</span></div>${bar(used, 90)}<div class="d">из ${gb(d.total)} ГБ</div>` });
  }
  setSection('pc', cards);
}

// ---------- music ----------
const ICONS = {
  prev: '<svg class="ci" viewBox="0 0 24 24"><path d="M6 5h2v14H6zM20 5v14L9 12z"/></svg>',
  next: '<svg class="ci" viewBox="0 0 24 24"><path d="M16 5h2v14h-2zM4 5v14l11-7z"/></svg>',
  play: '<svg class="ci" viewBox="0 0 24 24"><path d="M7 4v16l13-8z"/></svg>',
  pause: '<svg class="ci" viewBox="0 0 24 24"><path d="M6 4h4v16H6zM14 4h4v16h-4z"/></svg>'
};
let music = { key: '', art: null };
let currentMusic = null;
const APP_NAMES = [[/spotify/i, 'Spotify'], [/yandex/i, 'Яндекс Музыка'], [/chrome/i, 'Chrome'], [/msedge|edge/i, 'Edge'], [/firefox/i, 'Firefox'], [/vk/i, 'VK Музыка'], [/zune|groove|media/i, 'Медиаплеер']];
function appName(id) {
  return APP_NAMES.find(([re]) => re.test(id || ''))?.[1] || (id || '').split(/[\\!.]/).filter(Boolean).pop() || 'Плеер';
}
async function musicCmd(cmd) {
  const m = await ipc.media(cmd);
  if (m) showMusic(m);
}
async function pollMusic() {
  showMusic(await ipc.media('poll'));
}
function showMusic(m) {
  currentMusic = m;
  const np = $('#np');
  if (!m || !m.title) {
    np.classList.add('hidden');
    return setSection('music', []);
  }
  if (m.key !== music.key) music = { key: m.key, art: null };
  if (m.art) music.art = 'data:image/png;base64,' + m.art;
  np.classList.remove('hidden');
  np.classList.toggle('paused', !m.playing);
  $('span', np).textContent = [m.artist, m.title].filter(Boolean).join(' — ');
  const art = music.art ? `style="background-image:url('${music.art}')"` : '';
  setSection('music', [
    {
      cls: 'wide music',
      html: `<div class="mart ${music.art ? '' : 'noart'}" ${art}>${music.art ? '' : '♪'}</div>
        <div class="mbody"><div class="d">${esc(appName(m.app))} · ${m.playing ? 'играет' : 'пауза'}</div>
        <div class="t">${esc(m.title)}</div><div class="d ell">${esc(m.artist || '')}</div>
        <div class="eq ${m.playing ? '' : 'paused'}"><i></i><i></i><i></i><i></i></div></div>`,
      ok: () => musicCmd('toggle')
    },
    { cls: 'ctl', html: `${ICONS.prev}<div class="d">Назад</div>`, ok: () => musicCmd('prev') },
    { cls: 'ctl', html: `${m.playing ? ICONS.pause : ICONS.play}<div class="d">${m.playing ? 'Пауза' : 'Играть'}</div>`, ok: () => musicCmd('toggle') },
    { cls: 'ctl', html: `${ICONS.next}<div class="d">Далее</div>`, ok: () => musicCmd('next') }
  ]);
}

// ---------- Steam account (API key) ----------
function openSteamAccount() {
  openModal(`<h2>Аккаунт Steam</h2>
    <div class="meta" style="margin-bottom:16px"><span>Ключ нужен для трофеев и списка друзей. Он бесплатный и хранится только на этом компьютере.</span></div>
    <div class="field"><label>Steam Web API-ключ</label>
      <div class="line"><input class="focusable" id="fKey" placeholder="32 символа" value="${esc(data.settings.steamApiKey || '')}"/>
      <button class="btn focusable" id="bGet">Получить ключ</button></div>
      <div class="hint">На странице ключа в поле «Domain» можно написать что угодно, например localhost</div></div>
    <div class="field"><label>SteamID64</label><input class="focusable" id="fSid" placeholder="${esc(steamLocal.steamId || 'определяется автоматически')}" value="${esc(data.settings.steamId || '')}"/>
      <div class="hint">Оставьте пустым — возьмём аккаунт, который последним входил в Steam${steamLocal.steamId ? ` (${esc(steamLocal.steamId)})` : ''}</div></div>
    <div class="acc-check" id="accCheck"></div>
    <div class="buttons"><button class="btn focusable" id="bNo">Отмена</button><button class="btn focusable" id="bCheck">Проверить</button><button class="btn primary focusable" id="bOk">Сохранить</button></div>`,
  { focus: 0 });
  $('#bGet').onclick = () => ipc.openUrl('https://steamcommunity.com/dev/apikey');
  $('#bNo').onclick = closeModal;
  $('#bCheck').onclick = async () => {
    const box = $('#accCheck');
    box.innerHTML = '<span class="d">Проверяем…</span>';
    const key = $('#fKey').value.trim();
    const sid = $('#fSid').value.trim().replace(/\D/g, '') || steamLocal.steamId || '';
    const r = await ipc.steamCheck({ key, steamId: sid });
    const msg = {
      nokey: 'Введите ключ', badkey: 'Steam не принял ключ — скопируйте его заново со страницы ключа',
      noprofile: `Профиль ${sid} не найден — проверьте SteamID64`, network: 'Нет связи со Steam'
    }[r?.error];
    box.innerHTML = msg ? `<span class="bad">✕ ${esc(msg)}</span>` : `
      <div class="av" style="background-image:${esc(cssUrl(r.avatar))}"></div>
      <div><div class="t">${esc(r.name)}</div>
      <div class="${r.public ? 'good' : 'bad'}">${r.public ? '✓ Профиль открыт — трофеи будут видны' : '✕ Профиль скрыт — трофеи недоступны'}</div>
      <div class="${r.friendsVisible ? 'good' : 'bad'}">${r.friendsVisible ? '✓ Список друзей доступен' : '✕ Список друзей скрыт'}</div></div>`;
  };
  $('#bOk').onclick = async () => {
    data.settings.steamApiKey = $('#fKey').value.trim();
    data.settings.steamId = $('#fSid').value.trim().replace(/\D/g, '');
    await save();
    closeModal();
    feedGame = null;
    update(true);
    refreshFriends();
    refreshSales();
    toast('Аккаунт Steam сохранён');
  };
}

// ---------- helpers ----------
function hours(sec) {
  if (sec < 3600) return `${Math.round(sec / 60)} мин`;
  const h = sec / 3600;
  return `${h < 10 ? h.toFixed(1).replace('.', ',') : Math.round(h)} ч`;
}
function plural(n, one, few, many) {
  const m10 = n % 10;
  const m100 = n % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
}
