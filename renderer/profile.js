'use strict';
// Profile pages (own + friends), the power menu, Discord Rich Presence and trophy tracking.

const STEAM_CDN = 'https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps';
const APP_ICON_URL = 'https://raw.githubusercontent.com/SoFTeR1/otium/main/build/icon.png';
const pf = { open: false, idx: 0, stack: [] };   // stack of views: { kind: 'me' | 'friend' | 'steam', id }

// ---------- generic spatial navigation for card layouts ----------
function spatialNext(els, cur, dir) {
  if (!cur) return els[0];
  const a = cur.getBoundingClientRect();
  const ax = a.left + a.width / 2;
  const ay = a.top + a.height / 2;
  let best = null;
  let bestScore = Infinity;
  for (const el of els) {
    if (el === cur) continue;
    const b = el.getBoundingClientRect();
    const bx = b.left + b.width / 2;
    const by = b.top + b.height / 2;
    const dx = bx - ax;
    const dy = by - ay;
    const ok = dir === 'left' ? dx < -4 : dir === 'right' ? dx > 4 : dir === 'up' ? dy < -4 : dy > 4;
    if (!ok) continue;
    // prefer elements straight along the direction
    const along = dir === 'left' || dir === 'right' ? Math.abs(dx) : Math.abs(dy);
    const across = dir === 'left' || dir === 'right' ? Math.abs(dy) : Math.abs(dx);
    const score = along + across * 2.5;
    if (score < bestScore) { bestScore = score; best = el; }
  }
  return best;
}

// ---------- profile panel ----------
function profileOpen() { return pf.open; }

function openProfile(view = { kind: 'me' }) {
  pf.open = true;
  pf.stack = [view];
  pf.idx = 0;
  $('#profile').classList.remove('hidden');
  requestAnimationFrame(() => $('#profile').classList.add('show'));
  renderProfile();
}
function closeProfile() {
  pf.open = false;
  $('#profile').classList.remove('show');
  setTimeout(() => !pf.open && $('#profile').classList.add('hidden'), 300);
}
function pushProfile(view) {
  pf.stack.push(view);
  pf.idx = 0;
  renderProfile();
}

const SANE_TS = 1.5e12; // anything earlier than 2017 is a placeholder, not a real date
function since() {
  if (data.settings.since > SANE_TS) return data.settings.since;
  return Math.min(Date.now(), ...data.games.map((g) => g.added).filter((t) => t > SANE_TS));
}
function coverOf(steamId) {
  return steamId ? `${STEAM_CDN}/${steamId}/library_600x900.jpg` : '';
}
function pfAvatar(name, key, img) {
  if (img) return `<div class="pf-av" style="background-image:${esc(cssUrl(img))}"></div>`;
  const h = hue(key || name);
  return `<div class="pf-av" style="background:linear-gradient(135deg,hsl(${h} 65% 45%),hsl(${(h + 60) % 360} 65% 30%))">${esc((name || '?')[0].toUpperCase())}</div>`;
}
function trophyCard(t) {
  if (!t) return '<div class="pf-empty">Трофеев пока нет — они появятся, когда вы получите достижение в игре Steam</div>';
  return `<div class="pfi pf-trophy">${t.icon ? `<div class="ti" style="background-image:${esc(cssUrl(t.icon))}"></div>` : '<div class="ti">🏆</div>'}
    <div><div class="d">${esc(t.game || '')}</div><div class="t">${esc(t.name)}</div><div class="d">${t.time ? fmtDate(t.time) : ''}</div></div></div>`;
}
// cover: the art the launcher already shows for the game; otherwise Steam's portrait art (with fallbacks)
function gameCard(name, steamId, sub, attrs = '', cover = '') {
  const c = (cover && !isVideo(cover) ? cover : '') || coverOf(steamId);
  const h = hue(name);
  return `<div class="pfi pf-game" ${attrs}>
    <div class="gc" data-sid="${esc(steamId || '')}" data-name="${esc(name)}" style="${c ? `background-image:${esc(cssUrl(c))}` : `background:linear-gradient(135deg,hsl(${h} 70% 38%),hsl(${(h + 50) % 360} 70% 26%))`}">${c ? '' : esc(initials(name))}</div>
    <div class="t">${esc(name)}</div><div class="d">${esc(sub || '')}</div></div>`;
}
function statChip(n, label) {
  return `<div class="pf-stat"><b>${n}</b><span>${label}</span></div>`;
}

async function renderProfile() {
  const view = pf.stack[pf.stack.length - 1];
  const el = $('#profile');
  const back = pf.stack.length > 1 ? 'назад' : 'закрыть';
  let body = '';
  let title = 'Профиль';

  if (view.kind === 'me') {
    const a = acct();
    const name = a?.name || data.settings.user || 'Player';
    const games = data.games.filter((g) => g.category !== 'app');
    const total = games.reduce((s, g) => s + totalPlaytime(g), 0);
    const recent = [...games].filter((g) => lastPlayedOf(g)).sort((x, y) => lastPlayedOf(y) - lastPlayedOf(x)).slice(0, 8);
    const nowPlaying = data.games.find((g) => running.has(g.id));
    const lf = a?.friends || [];
    const sf = (steamFriendsCache || []).filter((f) => f.state > 0 || f.game);
    body = `
      <div class="pf-head">${pfAvatar(name, me?.pk || name)}
        <div><div class="pf-name">${esc(name)}</div>
        <div class="d">В OTIUM с ${fmtDate(since()).replace(/^сегодня.*/, 'сегодня')}${a?.sk ? ` · код ${shortCode(me.pk)}` : ''}</div>
        <div class="d ${nowPlaying ? 'game' : ''}">${nowPlaying ? `● Играет в ${esc(nowPlaying.name)}` : '● В сети'}</div></div>
        <div class="pf-stats">${statChip(games.length, 'игр')}${statChip(hours(total), 'наиграно')}${statChip(lf.length, 'друзей')}</div>
      </div>
      <div class="pf-sec"><div class="fhead">Последний трофей</div>${trophyCard(data.lastTrophy)}</div>
      <div class="pf-sec"><div class="fhead">Недавние игры</div><div class="pf-row">${recent.length ? recent.map((g) => gameCard(g.name, steamIdOf(g), [hours(totalPlaytime(g)), fmtDate(lastPlayedOf(g))].join(' · '), `data-game="${g.id}"`, g.cover)).join('') : '<div class="pf-empty">Вы ещё ничего не запускали</div>'}</div></div>
      <div class="pf-sec"><div class="fhead">Друзья</div><div class="pf-row">
        ${lf.map((f) => friendCard(f.name, f.pk, presenceOf(f.pk), `data-friend="${f.pk}"`)).join('')}
        ${sf.map((f) => friendCard(f.name, f.id, { online: true, game: !!f.game, text: f.game ? `В игре: ${f.game}` : 'В сети · Steam' }, `data-steam="${f.id}"`, f.avatar)).join('')}
        <div class="pfi pf-friend" data-action="social"><div class="pf-av plus">+</div><div class="t">${a?.sk ? 'Друзья и чат' : 'Создать аккаунт'}</div><div class="d">открыть</div></div>
      </div></div>`;
  } else if (view.kind === 'friend') {
    const pk = view.id;
    title = nameOf(pk);
    el.innerHTML = profileShell(title, back, '<div class="pf-empty">Загружаем профиль…</div>');
    const lib = await fetchLibrary(pk);
    if (pf.stack[pf.stack.length - 1] !== view) return;
    const p = presenceOf(pk);
    const games = lib?.games || [];
    body = `
      <div class="pf-head">${pfAvatar(nameOf(pk), pk)}
        <div><div class="pf-name">${esc(nameOf(pk))}</div>
        <div class="d">${lib?.since ? `В OTIUM с ${fmtDate(lib.since)}` : 'Игрок OTIUM'} · код ${shortCode(pk)}</div>
        <div class="d ${p.game ? 'game' : ''}">● ${esc(p.text)}</div></div>
        <div class="pf-stats">${statChip(games.length, 'игр')}${statChip(hours(games.reduce((s, g) => s + (g.p || 0), 0)), 'наиграно')}</div>
      </div>
      <div class="pf-sec"><div class="pf-row"><div class="pfi pf-btn" data-chat="${pk}">💬 Написать</div><div class="pfi pf-btn" data-call="${pk}">📞 Позвонить</div></div></div>
      <div class="pf-sec"><div class="fhead">Последний трофей</div>${trophyCard(lib?.trophy)}</div>
      <div class="pf-sec"><div class="fhead">Игры</div><div class="pf-row wrap">${games.length ? games.map((g) => gameCard(g.n, g.s, [g.p ? hours(g.p) : '', g.l ? fmtDate(g.l) : ''].filter(Boolean).join(' · '))).join('') : '<div class="pf-empty">Друг ещё не открыл свою библиотеку — она появится, когда он обновит лаунчер</div>'}</div></div>`;
  } else if (view.kind === 'steam') {
    title = 'Профиль Steam';
    el.innerHTML = profileShell(title, back, '<div class="pf-empty">Загружаем профиль Steam…</div>');
    const r = await ipc.playerGames({ key: apiKey(), steamId: view.id });
    if (pf.stack[pf.stack.length - 1] !== view) return;
    if (!r || r.error) body = `<div class="pf-empty">Не удалось загрузить профиль Steam (${esc(r?.error || 'ошибка')})</div>`;
    else {
      title = r.name;
      body = `
        <div class="pf-head">${pfAvatar(r.name, view.id, r.avatar)}
          <div><div class="pf-name">${esc(r.name)}</div>
          <div class="d">${r.since ? `В Steam с ${fmtDate(r.since)}` : 'Steam'}</div>
          <div class="d ${r.game ? 'game' : ''}">● ${esc(r.game ? `В игре: ${r.game}` : r.state ? 'В сети' : 'Не в сети')}</div></div>
          <div class="pf-stats">${statChip(r.total || 0, 'игр')}${statChip(hours(r.games.reduce((s, g) => s + g.playtime, 0)), 'наиграно')}</div>
        </div>
        <div class="pf-sec"><div class="pf-row"><div class="pfi pf-btn" data-url="steam://friends/message/${view.id}">💬 Написать в Steam</div><div class="pfi pf-btn" data-url="https://steamcommunity.com/profiles/${view.id}">Открыть профиль</div></div></div>
        ${r.recent.length ? `<div class="pf-sec"><div class="fhead">Недавно играл</div><div class="pf-row">${r.recent.map((g) => gameCard(g.name, g.id, `${hours(g.recent)} за 2 недели`)).join('')}</div></div>` : ''}
        <div class="pf-sec"><div class="fhead">Игры</div><div class="pf-row wrap">${r.hidden ? '<div class="pf-empty">Сведения об играх у этого игрока скрыты</div>' : r.games.map((g) => gameCard(g.name, g.id, g.playtime ? hours(g.playtime) : 'не запускал')).join('')}</div></div>`;
    }
  }

  el.innerHTML = profileShell(title, back, body);
  $$('#profile .pfi').forEach((n, i) => (n.onclick = () => { pf.idx = i; focusProfile(); activateProfileItem(n); }));
  // newer Steam games often lack the portrait art: fall back to the wide header, then to initials
  $$('#profile .gc').forEach((g) => {
    const m = g.style.backgroundImage.match(/url\("(.+)"\)/);
    if (!m) return;
    const tries = [m[1]];
    if (g.dataset.sid) tries.push(`https://cdn.cloudflare.steamstatic.com/steam/apps/${g.dataset.sid}/header.jpg`);
    const next = () => {
      const url = tries.shift();
      if (!url) {
        const h = hue(g.dataset.name);
        g.style.backgroundImage = '';
        g.style.background = `linear-gradient(135deg,hsl(${h} 70% 38%),hsl(${(h + 50) % 360} 70% 26%))`;
        g.textContent = initials(g.dataset.name);
        return;
      }
      const im = new Image();
      im.onload = () => (g.style.backgroundImage = cssUrl(url));
      im.onerror = next;
      im.src = url;
    };
    next();
  });
  focusProfile();
}

function profileShell(title, back, body) {
  return `<div class="soc-top"><div class="soc-title">${esc(title)}</div><div class="d">${oskHint('back', back)}</div></div>
    <div class="pf-body" id="pfBody">${body}</div>`;
}
function friendCard(name, key, p, attrs, img) {
  return `<div class="pfi pf-friend" ${attrs}>${pfAvatar(name, key, img)}<i class="pres ${p.online ? (p.game ? 'game' : 'on') : ''}"></i>
    <div class="t">${esc(name)}</div><div class="d ${p.game ? 'game' : ''}">${esc(p.text)}</div></div>`;
}

function focusProfile() {
  const els = $$('#profile .pfi');
  pf.idx = Math.max(0, Math.min(pf.idx, els.length - 1));
  els.forEach((e, i) => e.classList.toggle('focus', i === pf.idx));
  els[pf.idx]?.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'smooth' });
}

function activateProfileItem(n) {
  if (n.dataset.friend) return pushProfile({ kind: 'friend', id: n.dataset.friend });
  if (n.dataset.steam) return pushProfile({ kind: 'steam', id: n.dataset.steam });
  if (n.dataset.url) return ipc.openUrl(n.dataset.url);
  if (n.dataset.call) { closeProfile(); return startCall(n.dataset.call); }
  if (n.dataset.action === 'social') { closeProfile(); return openSocial(); }
  if (n.dataset.chat) {
    closeProfile();
    openSocial();
    social.idx = listItems().findIndex((x) => x.pk === n.dataset.chat);
    return activateItem();
  }
  if (n.dataset.game) {
    const g = data.games.find((x) => x.id === n.dataset.game);
    closeProfile();
    tab = 'game';
    zone = 'actions';
    render(g.id);
  }
}

function profileHandle(act) {
  const els = $$('#profile .pfi');
  if (act === 'back') {
    sfx.back();
    if (pf.stack.length > 1) { pf.stack.pop(); pf.idx = 0; return renderProfile(); }
    return closeProfile();
  }
  if (DIRS.has(act)) {
    const next = spatialNext(els, els[pf.idx], act);
    if (next) { pf.idx = els.indexOf(next); sfx.move(); focusProfile(); }
    else if (act === 'up' || act === 'down') $('#pfBody')?.scrollBy({ top: act === 'down' ? 200 : -200, behavior: 'smooth' });
    return;
  }
  if (act === 'ok' && els[pf.idx]) { sfx.ok(); activateProfileItem(els[pf.idx]); }
}

// ---------- trophies: remember the latest one, announce new ones after a game closes ----------
function noteTrophy(game, appid, items, announce) {
  const got = (items || []).filter((a) => a.achieved && a.time);
  if (!got.length) return;
  const newest = got.reduce((x, y) => (y.time > x.time ? y : x));
  if (newest.time <= (data.lastTrophy?.time || 0)) return;
  data.lastTrophy = { game, appid, name: newest.name, icon: newest.icon, time: newest.time };
  save();
  publishLibrarySoon();
  if (announce) {
    sfx.launch();
    toast(`${newest.name} — ${game}`, false, newest.icon ? { title: 'Трофей получен', img: newest.icon, ms: 7000 } : { title: 'Трофей получен', icon: '🏆', ms: 7000 });
  }
}

ipc.onSteamRunning(({ prev, at }) => {
  if (!prev || !apiKey() || !steamUser()) return;
  const g = data.games.find((x) => steamIdOf(x) === String(prev));
  // Steam needs a moment to sync achievements after the game closes
  setTimeout(async () => {
    const r = await ipc.achievementsFresh({ appid: prev, key: apiKey(), steamId: steamUser() });
    if (r?.items) noteTrophy(g?.name || 'Steam', String(prev), r.items.filter((a) => a.time >= (at - 6 * 3600e3)), true);
  }, 10000);
});

// ---------- shutdown timer ----------
const sleepTimer = { at: 0, action: 'shutdown', warned: {} };
const timerLeft = () => Math.max(0, sleepTimer.at - Date.now());
function fmtLeft(ms) {
  const s = Math.ceil(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = s % 60;
  return h ? `${h}:${String(m).padStart(2, '0')}:${String(ss).padStart(2, '0')}` : `${m}:${String(ss).padStart(2, '0')}`;
}
function setSleepTimer(minutes) {
  sleepTimer.at = Date.now() + minutes * 60e3;
  sleepTimer.warned = {};
  const what = sleepTimer.action === 'sleep' ? 'перейдёт в сон' : 'выключится';
  const at = new Date(sleepTimer.at).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
  toast(`ПК ${what} в ${at} (через ${fmtLeft(minutes * 60e3)})`, false, { title: 'Таймер выключения', icon: '⏻' });
  tickSleepTimer();
}
function cancelSleepTimer(silent) {
  sleepTimer.at = 0;
  tickSleepTimer();
  if (!silent) toast('Таймер выключения отменён', false, { title: 'Таймер', icon: '⏻' });
}
function tickSleepTimer() {
  const chip = $('#sleepChip');
  if (!sleepTimer.at) { chip.classList.add('hidden'); return; }
  const left = timerLeft();
  chip.classList.remove('hidden');
  $('span', chip).textContent = fmtLeft(left);
  const what = sleepTimer.action === 'sleep' ? 'перейдёт в сон' : 'выключится';
  for (const [min, label] of [[5, '5 минут'], [1, '1 минуту']]) {
    if (left <= min * 60e3 && !sleepTimer.warned[min]) {
      sleepTimer.warned[min] = true;
      toast(`ПК ${what} через ${label}. Отменить — Питание → Таймер выключения`, false, { title: 'Таймер выключения', icon: '⏻', ms: 9000 });
    }
  }
  if (left <= 0) {
    const action = sleepTimer.action;
    cancelSleepTimer(true);
    powerCountdown(action === 'sleep' ? 'sleep' : 'shutdown');
  }
}
setInterval(tickSleepTimer, 1000);

function openSleepTimer(focus = 0) {
  const active = !!sleepTimer.at;
  const presets = [[30, '30 минут'], [60, '1 час'], [90, '1,5 часа'], [120, '2 часа']];
  openModal(`<h2 style="padding:10px 18px 0">Таймер выключения</h2>
    ${active ? `<div class="meta" style="padding:4px 18px 8px"><span>Осталось ${fmtLeft(timerLeft())} — ПК ${sleepTimer.action === 'sleep' ? 'перейдёт в сон' : 'выключится'} в ${new Date(sleepTimer.at).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' })}</span></div>` : ''}
    ${presets.map(([m, l]) => `<button class="menu-item focusable" data-m="${m}">${POWER_ICONS.timer}Через ${l}</button>`).join('')}
    <button class="menu-item focusable" data-m="custom">${POWER_ICONS.timer}Своё время…</button>
    <div class="menu-sep"></div>
    <button class="menu-item focusable" data-m="action">Действие<span class="val">${sleepTimer.action === 'sleep' ? 'Спящий режим' : 'Выключить ПК'}</span></button>
    ${active ? '<button class="menu-item focusable danger" data-m="cancel">Отменить таймер</button>' : ''}`, { cls: 'menu', focus, onBack: openPower });
  $$('#modal .menu-item').forEach((b, i) => (b.onclick = () => {
    const m = b.dataset.m;
    if (m === 'action') { sleepTimer.action = sleepTimer.action === 'sleep' ? 'shutdown' : 'sleep'; return openSleepTimer(i); }
    if (m === 'cancel') { closeModal(); return cancelSleepTimer(); }
    if (m === 'custom') return openCustomTimer();
    closeModal();
    setSleepTimer(+m);
  }));
}

// "45" = in 45 minutes, "1:30" = in 1 h 30 min, "23:30" = at 23:30 (today or tomorrow)
function parseTimerInput(v) {
  v = v.trim().replace(/[.,\s]/g, ':');
  if (/^\d{1,4}$/.test(v)) return +v;
  const m = v.match(/^(\d{1,2}):(\d{2})$/);
  if (!m) return null;
  const hh = +m[1];
  const mm = +m[2];
  if (mm > 59) return null;
  // one-digit hours ("1:30") mean a duration; two digits ("01:30", "23:30") mean a clock time
  if (m[1].length === 1) return hh * 60 + mm;
  if (hh > 23) return null;
  const at = new Date();
  at.setHours(hh, mm, 0, 0);
  if (at <= new Date()) at.setDate(at.getDate() + 1);
  return Math.round((at - Date.now()) / 60e3);
}
function openCustomTimer() {
  openModal(`<h2>Своё время</h2>
    <div class="field"><label>Через сколько минут или во сколько</label><input class="focusable" id="fTimer" placeholder="например 45 или 23:30"/>
      <div class="hint">45 — через 45 минут · 1:30 — через полтора часа · 23:30 — в 23:30</div></div>
    <div class="buttons"><button class="btn focusable" id="bNo">Назад</button><button class="btn primary focusable" id="bYes">Запустить таймер</button></div>`,
  { onBack: () => openSleepTimer() });
  $('#bNo').onclick = () => openSleepTimer();
  $('#bYes').onclick = () => {
    const min = parseTimerInput($('#fTimer').value);
    if (!min || min < 1 || min > 24 * 60) return toast('Не понял время — введите минуты (45) или время (23:30)', true);
    closeModal();
    setSleepTimer(min);
  };
}

function minimizeLauncher() {
  closeModal();
  if (typeof closeCC === 'function' && cc.open) closeCC();
  ipc.minimize();
  // once we're no longer focused, this lands in the overlay above the desktop
  setTimeout(() => toast('Нажмите кнопку Home (Xbox) на геймпаде или Ctrl+Alt+Home, чтобы вернуть его', false, { title: 'Лаунчер свёрнут', icon: '🎮', ms: 6000 }), 600);
}

// ---------- power ----------
function openPower() {
  openModal(`<h2 style="padding:10px 18px 0">Питание</h2>
    <button class="menu-item focusable" data-p="minimize">${POWER_ICONS.min}Свернуть лаунчер<span class="val">Home — развернуть</span></button>
    <button class="menu-item focusable" data-p="timer">${POWER_ICONS.timer}Таймер выключения<span class="val">${sleepTimer.at ? fmtLeft(timerLeft()) : ''}</span></button>
    <button class="menu-item focusable" data-p="sleep">${POWER_ICONS.sleep}Спящий режим</button>
    <button class="menu-item focusable" data-p="restart">${POWER_ICONS.restart}Перезагрузить ПК</button>
    <button class="menu-item focusable danger" data-p="shutdown">${POWER_ICONS.off}Выключить ПК</button>
    <div class="menu-sep"></div>
    <button class="menu-item focusable" data-p="quit">Выйти из лаунчера</button>`, { cls: 'menu' });
  $$('#modal .menu-item').forEach((b) => (b.onclick = () => {
    const p = b.dataset.p;
    if (p === 'timer') return openSleepTimer();
    if (p === 'minimize') return minimizeLauncher();
    if (p === 'shutdown' || p === 'restart') return powerCountdown(p);
    closeModal();
    ipc.power(p);
  }));
}
function powerCountdown(action) {
  let left = 10;
  const label = { shutdown: 'Выключение', restart: 'Перезагрузка', sleep: 'Спящий режим' }[action];
  const draw = () => {
    openModal(`<h2>${label} через ${left}…</h2>
      <div class="meta"><span>Несохранённый прогресс в играх может быть потерян.</span></div>
      <div class="launch-bar" style="margin:18px 0 0;width:100%"><i style="animation:none;width:${(10 - left) * 10}%"></i></div>
      <div class="buttons"><button class="btn focusable" id="bNo">Отмена</button><button class="btn danger focusable" id="bYes">${label} сейчас</button></div>`, { onBack: cancel });
    $('#bNo').onclick = cancel;
    $('#bYes').onclick = go;
  };
  const timer = setInterval(() => { left--; if (left <= 0) go(); else if (modal.open) draw(); }, 1000);
  function cancel() { clearInterval(timer); closeModal(); }
  function go() { clearInterval(timer); closeModal(); ipc.power(action); }
  draw();
}
const POWER_ICONS = {
  min: '<svg class="mi" viewBox="0 0 24 24"><path d="M5 17h14v2H5zM12 5l6 7h-4v3h-4v-3H6z" transform="rotate(180 12 12)"/></svg>',
  timer: '<svg class="mi" viewBox="0 0 24 24"><path d="M15 1H9v2h6zm-4 13h2V8h-2zm8-6.6 1.4-1.4c-.4-.5-.9-1-1.4-1.4L17.6 6A9 9 0 1 0 19 7.4zM12 20a7 7 0 1 1 0-14 7 7 0 0 1 0 14z"/></svg>',
  sleep: '<svg class="mi" viewBox="0 0 24 24"><path d="M12 3a9 9 0 1 0 9 9 7 7 0 0 1-9-9z"/></svg>',
  restart: '<svg class="mi" viewBox="0 0 24 24"><path d="M12 4a8 8 0 1 0 8 8h-2a6 6 0 1 1-1.8-4.3L14 10h6V4l-2.3 2.3A8 8 0 0 0 12 4z"/></svg>',
  off: '<svg class="mi" viewBox="0 0 24 24"><path d="M11 2h2v10h-2zm6.4 3.2 1.4 1.4A8 8 0 1 1 5.2 6.6l1.4-1.4A6 6 0 1 0 17.4 5.2z"/></svg>'
};

// ---------- Discord Rich Presence ----------
let launchStarted = {};
async function updateDiscord() {
  if (!data.settings.discordId) return;
  ipc.discordActivity(buildActivity()); // renderer/presence.js — also drives the preview card
  // a game outside Steam: find public art for it once, then send again with the picture
  const g = data.games.find((x) => running.has(x.id));
  if (g && !gameArt(g) && g.publicArt?.for !== g.name && (await ensureGameArt(g)) && running.has(g.id)) ipc.discordActivity(buildActivity());
}
ipc.onSteamRunning(() => setTimeout(updateDiscord, 500));
ipc.onGameExited(() => setTimeout(updateDiscord, 500));

const DISCORD_APP_ID = '1567308312947728414';
async function toggleDiscord() {
  const s = data.settings;
  s.discordId = s.discordId ? '' : DISCORD_APP_ID;
  await save();
  // closing the pipe clears the status in Discord
  await ipc.discordConfig(s.discordId);
  if (s.discordId) {
    updateDiscord();
    setTimeout(async () => toast(await ipc.discordStatus() ? 'Статус в Discord включён' : 'Статус включится, когда Discord будет запущен'), 2500);
  } else toast('Статус в Discord выключен');
}

function profileInit() {
  if (!(data.settings.since > SANE_TS)) { data.settings.since = since(); save(); }
  if (data.settings.discordId) {
    ipc.discordConfig(data.settings.discordId);
    setTimeout(updateDiscord, 3000);
  }
}
