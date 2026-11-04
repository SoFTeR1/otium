'use strict';
// On-screen keyboard (gamepad text entry, PS5-style) and the Control Center (Guide button menu).

// ---------- On-screen keyboard ----------
const OSK_LAYOUTS = {
  ru: ['1234567890', 'йцукенгшщзхъ', 'фывапролджэ', 'ячсмитьбю.'],
  en: ['1234567890', 'qwertyuiop', 'asdfghjkl', 'zxcvbnm,.'],
  sym: ['1234567890', '-/:;()?!@"', '#%+=_*&\'<>', '~$€₽[]{}|\\']
};
// shift = one capital letter (⇧ key), caps = Caps Lock (LB on the gamepad)
const osk = { open: false, input: null, layout: 'ru', shift: false, caps: false, row: 1, el: null, onDone: null };

function openOSK(input, { onDone } = {}) {
  osk.open = true;
  osk.input = input;
  osk.onDone = onDone || null;
  osk.row = 1;
  osk.el = null;
  input.focus();
  $('#osk').classList.remove('hidden');
  renderOSK();
}

function closeOSK(submit) {
  osk.open = false;
  $('#osk').classList.add('hidden');
  const cb = osk.onDone;
  if (submit && cb) cb();
}

function renderOSK() {
  const upper = osk.shift !== osk.caps;
  const rows = OSK_LAYOUTS[osk.layout].map((r) => [...r].map((ch) => (upper ? ch.toUpperCase() : ch)));
  const keyHTML = (label, act, cls = '') => `<button class="key ${cls}" data-act="${esc(act)}">${esc(label)}</button>`;
  $('#osk').innerHTML = `
    <div class="osk-field">${esc(osk.input.value)}<i class="caret"></i></div>
    <div class="osk-keys">
      ${rows.map((r, i) => `<div class="krow">${i === 3 ? keyHTML(osk.caps ? '⇪' : '⇧', 'shift', 'wide' + (osk.shift || osk.caps ? ' on' : '')) : ''}${r.map((ch) => keyHTML(ch, 'c:' + ch)).join('')}${i === 3 ? keyHTML('⌫', 'back', 'wide') : ''}</div>`).join('')}
      <div class="krow">
        ${keyHTML(osk.layout === 'sym' ? 'АБВ' : '?123', 'sym', 'wide')}
        ${keyHTML(osk.layout === 'en' ? 'RU' : 'EN', 'lang', 'wide')}
        ${keyHTML('Пробел', 'space', 'space')}
        ${keyHTML('⌫', 'back', 'wide')}
        ${keyHTML('Готово', 'done', 'wide done')}
      </div>
    </div>
    <div class="osk-hints"><span><b class="kb">LB</b> ${osk.caps ? 'строчные' : 'заглавные'}</span>${oskHint('add', 'стереть')}${oskHint('menu', 'пробел')}<span><b class="kb">Start</b> готово</span>${oskHint('back', 'закрыть')}</div>`;
  const keys = $$('#osk .key');
  // keep the cursor on the "same" key after re-render
  const prev = osk.el;
  osk.el = (prev && keys.find((k) => k.dataset.act === prev)) || keys.find((k) => k.closest('.krow') === $$('#osk .krow')[osk.row]) || keys[0];
  osk.el = osk.el.dataset.act;
  keys.forEach((k) => {
    k.onmousedown = (e) => e.preventDefault();
    k.onclick = () => { osk.el = k.dataset.act; pressKey(k.dataset.act); };
  });
  focusKey();
}

function oskHint(k, label) {
  const g = GLYPHS[currentLayout()][k];
  return `<span><i class="g ${g[1]}">${g[0]}</i> ${label}</span>`;
}

function focusKey() {
  $$('#osk .key').forEach((k) => k.classList.toggle('focus', k.dataset.act === osk.el));
}

function typeText(s) {
  const i = osk.input;
  i.value += s;
  i.dispatchEvent(new Event('input', { bubbles: true }));
}

function pressKey(act) {
  sfx.move();
  if (act.startsWith('c:')) {
    typeText(act.slice(2));
    osk.shift = false;
  } else if (act === 'space') typeText(' ');
  else if (act === 'back') {
    osk.input.value = osk.input.value.slice(0, -1);
    osk.input.dispatchEvent(new Event('input', { bubbles: true }));
  } else if (act === 'shift') osk.shift = !osk.shift;
  else if (act === 'lang') osk.layout = osk.layout === 'en' ? 'ru' : 'en';
  else if (act === 'sym') osk.layout = osk.layout === 'sym' ? 'ru' : 'sym';
  else if (act === 'done') return closeOSK(true);
  renderOSK();
}

// Move to the key in the neighbouring row whose centre is closest horizontally
function moveKey(dir) {
  const rows = $$('#osk .krow').map((r) => [...r.querySelectorAll('.key')]);
  const cur = $(`#osk .key[data-act="${CSS.escape(osk.el)}"]`);
  let ri = rows.findIndex((r) => r.includes(cur));
  let ci = rows[ri].indexOf(cur);
  if (dir === 'left') ci = (ci - 1 + rows[ri].length) % rows[ri].length;
  else if (dir === 'right') ci = (ci + 1) % rows[ri].length;
  else {
    const x = cur.getBoundingClientRect();
    const cx = x.left + x.width / 2;
    ri = Math.max(0, Math.min(rows.length - 1, ri + (dir === 'down' ? 1 : -1)));
    ci = rows[ri].reduce((best, k, i) => {
      const r = k.getBoundingClientRect();
      const d = Math.abs(r.left + r.width / 2 - cx);
      return d < best.d ? { d, i } : best;
    }, { d: Infinity, i: 0 }).i;
  }
  osk.row = ri;
  osk.el = rows[ri][ci].dataset.act;
  sfx.move();
  focusKey();
}

function oskHandle(act) {
  if (DIRS.has(act)) return moveKey(act);
  if (act === 'ok') return pressKey(osk.el);
  if (act === 'add') return pressKey('back');
  if (act === 'menu') return pressKey('space');
  if (act === 'start') return closeOSK(true);
  if (act === 'lb') { osk.caps = !osk.caps; osk.shift = false; sfx.move(); return renderOSK(); }
  if (act === 'back') { sfx.back(); return closeOSK(false); }
}

// keep the preview line in sync when typing on a physical keyboard
document.addEventListener('input', (e) => {
  if (osk.open && e.target === osk.input) {
    const f = $('#osk .osk-field');
    if (f) f.innerHTML = `${esc(osk.input.value)}<i class="caret"></i>`;
  }
});

// ---------- Control Center (Guide button) ----------
const cc = { open: false, idx: 0, items: [] };

function runningGame() {
  return data.games.find((g) => running.has(g.id)) || null;
}

function openCC() {
  cc.open = true;
  cc.idx = 0;
  renderCC();
  document.body.classList.add('cc-open');
  $('#cc').classList.remove('hidden');
  requestAnimationFrame(() => $('#cc').classList.add('show'));
  sfx.ok();
}

function closeCC() {
  closeMixer();
  document.body.classList.remove('cc-open');
  cc.open = false;
  $('#cc').classList.remove('show');
  setTimeout(() => !cc.open && $('#cc').classList.add('hidden'), 300);
}

function backToGame() {
  closeCC();
  ipc.returnToGame();
}

function renderCC() {
  const g = runningGame();
  const m = currentMusic;
  const items = [];
  if (g) items.push({ cls: 'wide game', html: `<div class="ccimg" style="background-image:${g.cover ? esc(cssUrl(g.cover)) : 'none'}"></div><div><div class="d">Сейчас запущено</div><div class="t">${esc(g.name)}</div><div class="d">Вернуться в игру</div></div>`, ok: backToGame });
  else if (cc.canReturn) items.push({ cls: 'home', html: `${CC_ICONS.back}<div class="d">Вернуться</div>`, ok: backToGame });
  if (inCall() && call.state === 'active') {
    items.push({ cls: 'wide game', html: `<div class="ccimg call-ic">${CALL_ICONS.phone}</div><div><div class="d">Звонок</div><div class="t">${esc(nameOf(call.pk))}</div><div class="d">${call.muted ? 'микрофон выключен' : 'микрофон включён'}</div></div>`, ok: () => { toggleMic(); renderCC(); } });
    items.push({ cls: 'ctl', html: call.muted ? CALL_ICONS.micOff.replace('<svg', '<svg class="ci"') : CALL_ICONS.mic.replace('<svg', '<svg class="ci"'), ok: () => { toggleMic(); renderCC(); } });
    items.push({ cls: 'ctl hangup', html: CALL_ICONS.hang.replace('<svg', '<svg class="ci"'), ok: () => { hangUp(); renderCC(); } });
  }
  items.push({ cls: 'home', html: `${CC_ICONS.home}<div class="d">Главная</div>`, ok: () => { closeCC(); zone = 'row'; update(); } });
  if (m?.title) {
    items.push({ cls: 'wide music', html: `<div class="ccimg ${music.art ? '' : 'noart'}" ${music.art ? `style="background-image:url('${music.art}')"` : ''}>${music.art ? '' : '♪'}</div><div><div class="d">${esc(appName(m.app))}</div><div class="t">${esc(m.title)}</div><div class="d">${esc(m.artist || '')}</div></div>`, ok: () => musicCmd('toggle').then(renderCC) });
    items.push({ cls: 'ctl', html: ICONS.prev, ok: () => musicCmd('prev').then(renderCC) });
    items.push({ cls: 'ctl', html: m.playing ? ICONS.pause : ICONS.play, ok: () => musicCmd('toggle').then(renderCC) });
    items.push({ cls: 'ctl', html: ICONS.next, ok: () => musicCmd('next').then(renderCC) });
  }
  const unread = typeof socialUnread === 'function' ? socialUnread() : 0;
  items.push({ cls: 'chat', html: `${CC_ICONS.chat}<div class="d">Друзья${unread ? ` · <b class="badge">${unread}</b>` : ''}</div>`, ok: () => { closeCC(); openSocial(); } });
  items.push({ cls: 'vol', html: `${CC_ICONS.vol}<div class="d">Звук</div>`, ok: openMixer });
  items.push({ cls: 'set', html: `${CC_ICONS.settings}<div class="d">Настройки</div>`, ok: () => { closeCC(); openSettings(); } });
  items.push({ cls: 'set', html: `${CC_ICONS.power}<div class="d">Питание</div>`, ok: () => { closeCC(); openPower(); } });
  cc.items = items;
  cc.idx = Math.min(cc.idx, items.length - 1);
  $('#cc').innerHTML = `<div class="cc-bar">${items.map((it, i) => `<div class="cci ${it.cls}" data-i="${i}">${it.html}</div>`).join('')}</div>
    <div class="cc-time">${new Date().toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' })}</div>`;
  $$('#cc .cci').forEach((el) => (el.onclick = () => { cc.idx = +el.dataset.i; focusCC(); cc.items[cc.idx].ok(); }));
  focusCC();
}

function focusCC() {
  $$('#cc .cci').forEach((el, i) => el.classList.toggle('focus', i === cc.idx));
}

// ---------- Volume mixer (per-app volume, like the Windows mixer) ----------
const mixer = { open: false, idx: 0, rows: [] };

async function openMixer() {
  mixer.open = true;
  mixer.idx = 0;
  $('#mixer').innerHTML = '<div class="mx-head">Звук</div><div class="mx-empty">Загружаем приложения…</div>';
  $('#mixer').classList.remove('hidden');
  requestAnimationFrame(() => $('#mixer').classList.add('show'));
  loadMixer(await ipc.audio('list'));
}
function closeMixer() {
  if (!mixer.open) return;
  mixer.open = false;
  $('#mixer').classList.remove('show');
  setTimeout(() => !mixer.open && $('#mixer').classList.add('hidden'), 250);
}

function loadMixer(d) {
  if (!mixer.open) return;
  if (!d || d.error) {
    $('#mixer').innerHTML = '<div class="mx-head">Звук</div><div class="mx-empty">Не удалось получить громкость приложений</div>';
    return;
  }
  // one row per app even if it runs several audio processes (Chrome, Discord…)
  const byApp = new Map();
  for (const a of d.apps) {
    const k = (a.path || a.name).toLowerCase();
    const cur = byApp.get(k);
    if (cur) cur.pids.push(a.pid);
    else byApp.set(k, { ...a, pids: [a.pid] });
  }
  mixer.rows = [
    { master: true, name: 'Общая громкость', volume: d.master, muted: d.masterMuted },
    ...[...byApp.values()].sort((a, b) => a.name.localeCompare(b.name, 'ru'))
  ];
  mixer.idx = Math.min(mixer.idx, mixer.rows.length - 1);
  renderMixer();
  mixer.rows.forEach(async (r, i) => {
    if (!r.path || r.icon) return;
    r.icon = await ipc.appIcon(r.path);
    const img = $(`#mixer .mx-row[data-i="${i}"] .mx-ic`);
    if (img && r.icon) img.style.backgroundImage = `url('${r.icon}')`;
  });
}

function renderMixer() {
  $('#mixer').innerHTML = `<div class="mx-head">Звук <span>${oskHint('ok', 'выкл. звук')} ◀ ▶ громкость</span></div>` + mixer.rows.map((r, i) => `
    <div class="mx-row ${i === mixer.idx ? 'focus' : ''} ${r.muted ? 'muted' : ''} ${r.master ? 'master' : ''}" data-i="${i}">
      <div class="mx-ic" ${r.icon ? `style="background-image:url('${r.icon}')"` : ''}>${r.master ? CC_ICONS.vol : r.icon ? '' : esc(r.name[0])}</div>
      <div class="mx-body"><div class="mx-name">${esc(r.name)}</div>
        <div class="mx-bar"><i style="width:${r.muted ? 0 : r.volume}%"></i></div></div>
      <div class="mx-val">${r.muted ? CC_ICONS.mute : r.volume}</div>
    </div>`).join('');
  $$('#mixer .mx-row').forEach((el) => {
    const i = +el.dataset.i;
    el.onclick = (e) => {
      mixer.idx = i;
      const bar = $('.mx-bar', el).getBoundingClientRect();
      if (e.clientX >= bar.left && e.clientX <= bar.right) setVolume(mixer.rows[i], Math.round(((e.clientX - bar.left) / bar.width) * 100));
      else renderMixer();
    };
  });
}

async function setVolume(r, v) {
  r.volume = Math.max(0, Math.min(100, v));
  if (r.muted && v > 0) await toggleMute(r);
  renderMixer();
  if (r.master) ipc.audio(`master ${r.volume}`);
  else for (const pid of r.pids) ipc.audio(`set ${pid} ${r.volume}`);
}
async function toggleMute(r) {
  r.muted = !r.muted;
  renderMixer();
  if (r.master) await ipc.audio(`mastermute ${r.muted ? 1 : 0}`);
  else for (const pid of r.pids) await ipc.audio(`mute ${pid} ${r.muted ? 1 : 0}`);
}

function mixerHandle(act) {
  const r = mixer.rows[mixer.idx];
  if (act === 'back') { sfx.back(); return closeMixer(); }
  if (act === 'up' && mixer.idx > 0) { mixer.idx--; sfx.move(); renderMixer(); }
  else if (act === 'down' && mixer.idx < mixer.rows.length - 1) { mixer.idx++; sfx.move(); renderMixer(); }
  else if ((act === 'left' || act === 'right') && r) { setVolume(r, r.volume + (act === 'right' ? 5 : -5)); tone(act === 'right' ? 900 : 700, 0.03, 0.02); }
  else if (act === 'ok' && r) { sfx.ok(); toggleMute(r); }
  $('#mixer .mx-row.focus')?.scrollIntoView({ block: 'nearest' });
}

function ccHandle(act) {
  if (mixer.open) return mixerHandle(act);
  if (act === 'left' && cc.idx > 0) { cc.idx--; sfx.move(); focusCC(); }
  else if (act === 'right' && cc.idx < cc.items.length - 1) { cc.idx++; sfx.move(); focusCC(); }
  else if (act === 'ok') { sfx.ok(); cc.items[cc.idx]?.ok(); }
  else if (act === 'back' || act === 'up') { sfx.back(); closeCC(); }
}

const CC_ICONS = {
  vol: '<svg class="ci" viewBox="0 0 24 24"><path d="M4 9v6h4l5 5V4L8 9zm12.5 3A4.5 4.5 0 0 0 14 8v8a4.5 4.5 0 0 0 2.5-4zM14 3.2v2.1a7 7 0 0 1 0 13.4v2.1a9 9 0 0 0 0-17.6z"/></svg>',
  mute: '<svg class="ci" viewBox="0 0 24 24"><path d="M4 9v6h4l5 5V4L8 9zm12.6 3 2.7-2.7-1.4-1.4-2.7 2.7-2.7-2.7-1.4 1.4 2.7 2.7-2.7 2.7 1.4 1.4 2.7-2.7 2.7 2.7 1.4-1.4z"/></svg>',
  power: '<svg class="ci" viewBox="0 0 24 24"><path d="M11 2h2v10h-2zm6.4 3.2 1.4 1.4A8 8 0 1 1 5.2 6.6l1.4-1.4A6 6 0 1 0 17.4 5.2z"/></svg>',
  back: '<svg class="ci" viewBox="0 0 24 24"><path d="M10 6 4 12l6 6v-4h6a4 4 0 0 0 4-4V6h-2v4a2 2 0 0 1-2 2h-6V6z"/></svg>',
  home: '<svg class="ci" viewBox="0 0 24 24"><path d="M12 3 2 12h3v8h5v-5h4v5h5v-8h3z"/></svg>',
  chat: '<svg class="ci" viewBox="0 0 24 24"><path d="M4 4h16v11H8l-4 4zM7 8h10v2H7zm0 3h7v2H7z"/></svg>',
  settings: '<svg class="ci" viewBox="0 0 24 24"><path d="M19.4 13a7.6 7.6 0 0 0 0-2l2.1-1.6-2-3.5-2.5 1a7.4 7.4 0 0 0-1.7-1L15 3.3h-4l-.4 2.6a7.4 7.4 0 0 0-1.7 1l-2.5-1-2 3.5L6.6 11a7.6 7.6 0 0 0 0 2l-2.1 1.6 2 3.5 2.5-1a7.4 7.4 0 0 0 1.7 1l.4 2.6h4l.4-2.6a7.4 7.4 0 0 0 1.7-1l2.5 1 2-3.5zM12 15.5a3.5 3.5 0 1 1 0-7 3.5 3.5 0 0 1 0 7z"/></svg>'
};

// Guide / View+Start from the background watcher
ipc.onGuide(({ from, canReturn }) => {
  cc.canReturn = !!canReturn;
  if (osk.open) closeOSK(false);
  if (from === 'front' && cc.open) {
    // second press while a game runs = back to the game, like the PS button
    if (runningGame() || cc.canReturn) backToGame();
    else closeCC();
    return;
  }
  if (modal.open) closeModal();
  if (!cc.open) {
    pollMusic().finally(openCC);
  }
});
