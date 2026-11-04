'use strict';
// Always-on-top overlay: renders what the launcher sends it, only while the launcher isn't focused.
const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
function hue(str) { let h = 0; for (const c of str || '?') h = (h * 31 + c.charCodeAt(0)) >>> 0; return h % 360; }
const grad = (key) => { const h = hue(key); return `background:linear-gradient(135deg,hsl(${h} 65% 45%),hsl(${(h + 60) % 360} 65% 30%))`; };

const ICON = {
  phone: '<svg viewBox="0 0 24 24"><path d="M6.6 10.8a15.1 15.1 0 0 0 6.6 6.6l2.2-2.2c.3-.3.7-.4 1-.2 1.1.4 2.3.6 3.6.6.6 0 1 .4 1 1V20c0 .6-.4 1-1 1A17 17 0 0 1 3 4c0-.6.4-1 1-1h3.5c.6 0 1 .4 1 1 0 1.3.2 2.5.6 3.6.1.3 0 .7-.2 1z"/></svg>',
  hang: '<svg viewBox="0 0 24 24"><path d="M12 9c-1.6 0-3.1.3-4.6.7v3.1c0 .4-.2.7-.6.9-1 .5-1.9 1.1-2.7 1.8-.2.2-.4.3-.7.3s-.5-.1-.7-.3L.3 13.1A1 1 0 0 1 0 12.4c0-.3.1-.5.3-.7A16.9 16.9 0 0 1 12 7c4.6 0 8.7 1.8 11.7 4.7.2.2.3.4.3.7s-.1.5-.3.7l-2.4 2.4c-.2.2-.4.3-.7.3s-.5-.1-.7-.3c-.8-.7-1.7-1.3-2.7-1.8-.3-.2-.6-.5-.6-.9V9.7C15.1 9.3 13.6 9 12 9z"/></svg>',
  mic: '<svg viewBox="0 0 24 24"><path d="M12 14a3 3 0 0 0 3-3V5a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.9V21h2v-3.1a7 7 0 0 0 6-6.9z"/></svg>',
  micOff: '<svg viewBox="0 0 24 24"><path d="M19 11h-2c0 .7-.2 1.4-.4 2l1.5 1.5c.6-1 .9-2.2.9-3.5zm-4 .2V5a3 3 0 0 0-5.9-.8zM4.3 3 3 4.3l6 6V11a3 3 0 0 0 4.3 2.7l1.6 1.6A5 5 0 0 1 7 11H5a7 7 0 0 0 6 6.9V21h2v-3.1c1-.1 1.9-.5 2.7-1L19.7 21l1.3-1.3z"/></svg>',
  head: '<svg viewBox="0 0 24 24"><path d="M12 3a9 9 0 0 0-9 9v7a2 2 0 0 0 2 2h3v-8H5v-1a7 7 0 0 1 14 0v1h-3v8h3a2 2 0 0 0 2-2v-7a9 9 0 0 0-9-9z"/></svg>',
  headOff: '<svg viewBox="0 0 24 24"><path d="M3.3 2 2 3.3l2.6 2.6A9 9 0 0 0 3 12v7a2 2 0 0 0 2 2h3v-8H5v-1c0-1.2.3-2.4.9-3.4L19 21.7l1.3-1.3zM12 3c-1.7 0-3.3.5-4.7 1.3l1.5 1.5A7 7 0 0 1 19 12v1h-3v.2l5 5V12a9 9 0 0 0-9-9z"/></svg>'
};

const st = { focused: true, call: null, speakingMap: {}, toasts: [] };

// ---------- click-through except over buttons ----------
// Windows doesn't reliably forward mouse moves to a click-through window, so instead we tell the main
// process where our clickable areas are; it watches the cursor and switches click-through off over them.
function reportRegions() {
  const rects = [...document.querySelectorAll('#voice.show .vwrap, #incoming.show button')].map((b) => {
    const r = b.getBoundingClientRect();
    return { x: r.left - 6, y: r.top - 6, w: r.width + 12, h: r.height + 40 };
  });
  window.overlay.regions(rects);
}
setInterval(reportRegions, 500);

function syncVisible() {
  const c = st.call;
  const show = !st.focused && ((c && c.state !== 'idle') || st.toasts.length > 0);
  window.overlay.visible(show);
}

function renderCall() {
  const c = st.call;
  const on = !st.focused && c && c.state !== 'idle';
  const incoming = on && c.state === 'incoming';
  const active = on && c.state !== 'incoming';
  $('#incoming').classList.toggle('show', !!incoming);
  $('#voice').classList.toggle('show', !!active);
  if (incoming) {
    $('#incoming').innerHTML = `<div class="card">
      <div class="top"><div class="av" style="${grad(c.key)}">${esc(c.name[0].toUpperCase())}</div>
        <div><div class="k">${esc(c.kicker || 'Входящий звонок')}</div><div class="n">${esc(c.name)}</div></div></div>
      <div class="row"><button class="btn yes" data-a="accept">${ICON.phone}Принять</button><button class="btn no" data-a="decline">${ICON.hang}Отклонить</button></div>
      <div class="hint">или кнопка Xbox на геймпаде</div></div>`;
  }
  if (active) {
    // Discord-style voice widget: who's in the call, who's talking, who's muted. No buttons — the call is
    // controlled from the launcher; this stays fully click-through.
    const status = { outgoing: 'Звоним…', connecting: 'Соединяем…' }[c.state];
    const icons = (p) => (p?.deafened ? ICON.headOff + ICON.micOff : p?.muted ? ICON.micOff : '');
    const row = (p) => `<div class="vrow">
      <div class="vav ${st.speakingMap[p.key] ? 'speaking' : ''}" data-key="${esc(p.key)}" style="${grad(p.key)}">${esc((p.name || '?')[0].toUpperCase())}</div>
      <div class="vname ${p.deafened ? 'dim' : ''}">${esc(p.name)}${icons(p)}</div></div>`;
    // the whole widget is one button: hovering wakes it up, clicking opens the call in the launcher
    $('#voice').innerHTML = `<button class="vwrap" data-a="opencall">
      <div class="vhead"><i></i>${status ? status : `${c.title ? esc(c.title) : 'Голосовой звонок'} <b id="timer">${timer(c.startedAt)}</b>`}</div>
      ${(c.participants || []).map(row).join('')}
      <div class="vopen">Открыть звонок в лаунчере ›</div></button>`;
  }
  document.querySelectorAll('[data-a]').forEach((b) => (b.onclick = () => window.overlay.action(b.dataset.a)));
  syncVisible();
  setTimeout(reportRegions, 450); // after the slide-in transition
}

function timer(start) {
  if (!start) return '';
  const s = Math.floor((Date.now() - start) / 1000);
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}
setInterval(() => {
  const t = $('#timer');
  if (t && st.call?.state === 'active') t.textContent = timer(st.call.startedAt);
}, 1000);

function showToast(m) {
  const el = document.createElement('div');
  el.className = 'toast' + (m.err ? ' err' : '');
  let pic;
  if (m.img) pic = `<div class="pic sq" style="background-image:url('${esc(m.img)}')"></div>`;
  else if (m.avatar) pic = `<div class="pic" style="${grad(m.avatar.key || m.avatar.name)}">${esc((m.avatar.name || '?')[0].toUpperCase())}</div>`;
  else pic = `<div class="pic emoji">${esc(m.icon || '🎮')}</div>`;
  el.innerHTML = `<div class="app"><img src="icon.png">PS5 Launcher</div>
    <div class="body">${pic}<div><div class="tt">${esc(m.title || 'PS5 Launcher')}</div><div class="tx">${esc(m.text)}</div></div></div>`;
  $('#toasts').appendChild(el);
  st.toasts.push(el);
  while (st.toasts.length > 3) st.toasts.shift().remove();
  syncVisible();
  requestAnimationFrame(() => requestAnimationFrame(() => el.classList.add('show')));
  setTimeout(() => {
    el.classList.remove('show');
    setTimeout(() => { el.remove(); st.toasts = st.toasts.filter((x) => x !== el); syncVisible(); }, 450);
  }, m.ms || 6000);
}

window.overlay.onMessage((m) => {
  if (m.type === 'focus') { st.focused = m.focused; renderCall(); }
  else if (m.type === 'call') { st.call = m; renderCall(); }
  else if (m.type === 'speaking') {
    st.speakingMap = { ...st.speakingMap, ...(m.map || {}) };
    for (const [k, on] of Object.entries(m.map || {})) document.querySelector(`#voice [data-key="${CSS.escape(k)}"]`)?.classList.toggle('speaking', on);
  } else if (m.type === 'toast') showToast(m);
});
