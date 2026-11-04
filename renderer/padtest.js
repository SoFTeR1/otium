// ---------- Gamepad: live button test and stick calibration ----------
// Settings → "Геймпад: проверка и калибровка". Every button lights up on a drawing of the pad, sticks
// move inside their rings, triggers fill up. Calibration measures each stick's resting centre and
// jitter (→ dead zone), then its reach while you roll it around (→ full range). The result is stored
// per controller model in settings.padCal and applied to menu navigation (padAxes below).
// Inside the test every button is "being tested", so leaving and calibrating are holds:
// hold ○/B to leave, hold Start/≡ to calibrate. Keyboard and mouse work as usual.

const padTest = { open: false, raf: 0, phase: 'idle', t0: 0, samples: null, hold: {}, gpId: '' };
const HOLD_MS = 1200;

function padTestOpen() { return padTest.open && modal.open; }

// ---- calibrated sticks, used by the launcher's own pad polling ----
function calAxis(v, k) {
  if (!k) return v;
  const d = v - k.c;
  return Math.max(-1, Math.min(1, d >= 0 ? d / Math.max(0.2, k.hi - k.c) : d / Math.max(0.2, k.c - k.lo)));
}
function padAxes(gp, stick) {
  const cal = data.settings.padCal?.[gp.id];
  const kx = cal?.axes?.[stick * 2], ky = cal?.axes?.[stick * 2 + 1];
  let x = calAxis(gp.axes[stick * 2] ?? 0, kx);
  let y = calAxis(gp.axes[stick * 2 + 1] ?? 0, ky);
  const dz = Math.max(kx?.dz || 0, ky?.dz || 0);
  const m = Math.hypot(x, y);
  if (m < dz) return [0, 0];
  if (dz && m) { const s = Math.min(1, (m - dz) / (1 - dz)) / m; x *= s; y *= s; }
  return [x, y];
}

// the most recently used pad
function activePad() {
  let best = null;
  for (const gp of navigator.getGamepads()) if (gp && (!best || gp.timestamp > best.timestamp)) best = gp;
  return best;
}
const padName = (gp) => (gp?.id || '').replace(/\s*\(.*?\)\s*/g, ' ').replace(/\s+/g, ' ').trim() || 'Геймпад';

// ---- drawing of a generic pad; every part carries the standard-mapping button index ----
function padSvg() {
  const g = GLYPHS[currentLayout()];
  const face = [[0, 0, 34, g.ok[0]], [1, 34, 0, g.back[0]], [2, -34, 0, g.add[0]], [3, 0, -34, g.menu[0]]];
  return `
  <svg class="pt-pad" viewBox="-30 -14 580 360">
    <defs>
      <linearGradient id="ptBody" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#3a4766"/><stop offset="1" stop-color="#1a2238"/></linearGradient>
      <radialGradient id="ptGlow"><stop offset="0" stop-color="#bfe0ff"/><stop offset="1" stop-color="#4f8dff"/></radialGradient>
    </defs>
    <g class="pt-trig" data-b="6"><rect x="96" y="-8" width="92" height="22" rx="9"/><rect class="pt-fill" x="96" y="-8" width="0" height="22" rx="9"/><text x="142" y="8">LT</text></g>
    <g class="pt-trig" data-b="7"><rect x="332" y="-8" width="92" height="22" rx="9"/><rect class="pt-fill" x="332" y="-8" width="0" height="22" rx="9"/><text x="378" y="8">RT</text></g>
    <g class="pt-btn" data-b="4"><rect x="88" y="24" width="108" height="20" rx="10"/><text x="142" y="39">LB</text></g>
    <g class="pt-btn" data-b="5"><rect x="324" y="24" width="108" height="20" rx="10"/><text x="378" y="39">RB</text></g>
    <path class="pt-body" d="M130 52 H390 C452 52 478 84 494 140 L522 246 C534 296 492 324 456 298 L396 244 H124 L64 298 C28 324 -14 296 -2 246 L26 140 C42 84 68 52 130 52 Z"/>
    <g class="pt-btn" data-b="8"><rect x="208" y="112" width="30" height="16" rx="8"/></g>
    <g class="pt-btn" data-b="9"><rect x="282" y="112" width="30" height="16" rx="8"/></g>
    <g class="pt-btn" data-b="16"><circle cx="260" cy="86" r="13"/><circle class="pt-dot" cx="260" cy="86" r="4"/></g>
    <g class="pt-stick" data-s="0" data-b="10" transform="translate(150 132)"><circle class="pt-well" r="40"/><circle class="pt-dz" r="0"/><circle class="pt-cap" r="17"/></g>
    <g class="pt-stick" data-s="1" data-b="11" transform="translate(320 214)"><circle class="pt-well" r="40"/><circle class="pt-dz" r="0"/><circle class="pt-cap" r="17"/></g>
    <g transform="translate(200 214)">
      <g class="pt-btn" data-b="12"><rect x="-11" y="-40" width="22" height="28" rx="5"/></g>
      <g class="pt-btn" data-b="13"><rect x="-11" y="12" width="22" height="28" rx="5"/></g>
      <g class="pt-btn" data-b="14"><rect x="-40" y="-11" width="28" height="22" rx="5"/></g>
      <g class="pt-btn" data-b="15"><rect x="12" y="-11" width="28" height="22" rx="5"/></g>
    </g>
    <g transform="translate(372 132)">
      ${face.map(([b, x, y, t]) => `<g class="pt-btn pt-face" data-b="${b}"><circle cx="${x}" cy="${y}" r="16"/><text x="${x}" y="${y + 5}">${esc(t)}</text></g>`).join('')}
    </g>
  </svg>`;
}

function openPadTest() {
  const back = GLYPHS[currentLayout()].back[0];
  openModal(`
    <div class="pt-wrap">
      <div class="pt-left">
        ${padSvg()}
        <div class="pt-holds">
          <span class="pt-hold" id="ptHoldBack"><i></i>Удерживайте <b>${esc(back)}</b> — выйти</span>
          <span class="pt-hold" id="ptHoldStart"><i></i>Удерживайте <b>Start</b> — калибровка</span>
        </div>
      </div>
      <div class="pt-right">
        <div class="pt-kicker">ПРОВЕРКА ГЕЙМПАДА</div>
        <h2 id="ptName">Нажмите любую кнопку</h2>
        <div class="pt-status" id="ptStatus"></div>
        <div class="pt-stats" id="ptStats"></div>
        <div class="buttons pt-actions">
          <button class="btn primary focusable" id="ptCal">Калибровать</button>
          <button class="btn focusable" id="ptReset">Сбросить</button>
          <button class="btn focusable" id="ptClose">Закрыть</button>
        </div>
      </div>
    </div>`, { cls: 'padtest', onBack: closePadTest });
  $('#ptCal').onclick = startCalibration;
  $('#ptReset').onclick = resetCalibration;
  $('#ptClose').onclick = closePadTest;
  padTest.open = true;
  padTest.phase = 'idle';
  padTest.hold = {};
  padTest.layout = currentLayout();
  cancelAnimationFrame(padTest.raf);
  padTest.raf = requestAnimationFrame(padTestFrame);
}

function closePadTest() {
  padTest.open = false;
  cancelAnimationFrame(padTest.raf);
  closeModal();
}

function startCalibration() {
  const gp = activePad();
  if (!gp) return toast('Подключите геймпад', true, { title: 'Калибровка' });
  sfx.ok();
  padTest.gpId = gp.id;
  padTest.phase = 'rest';
  padTest.t0 = performance.now();
  padTest.samples = Array.from({ length: 4 }, () => ({ sum: 0, n: 0, vals: [], lo: 1, hi: -1 }));
}

function resetCalibration() {
  const gp = activePad();
  if (gp && data.settings.padCal?.[gp.id]) {
    delete data.settings.padCal[gp.id];
    save();
  }
  padTest.phase = 'idle';
  sfx.back();
  toast('Калибровка сброшена', false, { title: 'Геймпад', icon: '🎮' });
}

// rest → centre and jitter, circle → reach in every direction
function calibrationStep(gp, now) {
  const el = now - padTest.t0;
  const S = padTest.samples;
  if (padTest.phase === 'rest') {
    for (let i = 0; i < 4; i++) { const v = gp.axes[i] ?? 0; S[i].sum += v; S[i].n++; S[i].vals.push(v); }
    if (el > 2500) { padTest.phase = 'circle'; padTest.t0 = now; sfx.move(); }
    return;
  }
  if (padTest.phase === 'circle') {
    for (let i = 0; i < 4; i++) { const v = gp.axes[i] ?? 0; S[i].lo = Math.min(S[i].lo, v); S[i].hi = Math.max(S[i].hi, v); }
    if (el < 5000) return;
    const axes = S.map((s) => {
      const c = s.n ? s.sum / s.n : 0;
      const noise = s.vals.reduce((m, v) => Math.max(m, Math.abs(v - c)), 0);
      const moved = s.hi - s.lo > 1;
      return {
        c: +c.toFixed(4),
        dz: +Math.min(0.3, Math.max(0.06, noise * 1.6 + 0.04)).toFixed(3),
        lo: moved ? +Math.min(-0.5, s.lo).toFixed(3) : -1,
        hi: moved ? +Math.max(0.5, s.hi).toFixed(3) : 1
      };
    });
    data.settings.padCal = { ...(data.settings.padCal || {}), [padTest.gpId]: { axes, at: Date.now() } };
    save();
    padTest.phase = 'done';
    padTest.t0 = now;
    sfx.ok();
    toast('Калибровка сохранена', false, { title: padName(gp), icon: '🎮' });
  }
}

function padTestFrame(now) {
  if (!padTestOpen() || !$('#modal.padtest')) { padTest.open = false; return; }
  const gp = activePad();
  const root = $('#modal');
  if (gp) {
    $('#ptName').textContent = padName(gp);
    // A/B/X/Y or ✕/○/□/△ depending on the pad in hand
    detectLayout(gp);
    if (padTest.layout !== currentLayout()) {
      padTest.layout = currentLayout();
      $('.pt-pad', root).outerHTML = padSvg();
      $('#ptHoldBack b').textContent = GLYPHS[padTest.layout].back[0];
    }
    // buttons
    $$('.pt-btn, .pt-stick', root).forEach((el) => el.classList.toggle('on', !!gp.buttons[+el.dataset.b]?.pressed));
    $$('.pt-trig', root).forEach((el) => {
      const v = gp.buttons[+el.dataset.b]?.value || 0;
      el.querySelector('.pt-fill').setAttribute('width', (92 * v).toFixed(1));
      el.classList.toggle('on', v > 0.05);
    });
    // sticks: raw position, dead zone ring from the calibration
    const cal = data.settings.padCal?.[gp.id];
    $$('.pt-stick', root).forEach((el) => {
      const s = +el.dataset.s;
      const [x, y] = [gp.axes[s * 2] ?? 0, gp.axes[s * 2 + 1] ?? 0];
      el.querySelector('.pt-cap').setAttribute('transform', `translate(${(x * 23).toFixed(1)} ${(y * 23).toFixed(1)})`);
      const dz = cal ? Math.max(cal.axes[s * 2].dz, cal.axes[s * 2 + 1].dz) : 0;
      el.querySelector('.pt-dz').setAttribute('r', (dz * 40).toFixed(1));
    });
    // holds: ○/B leaves, Start calibrates
    for (const [b, id, fn] of [[1, 'ptHoldBack', closePadTest], [9, 'ptHoldStart', startCalibration]]) {
      const down = !!gp.buttons[b]?.pressed;
      if (down && !padTest.hold[b]) padTest.hold[b] = now;
      if (!down) padTest.hold[b] = 0;
      const p = padTest.hold[b] ? Math.min(1, (now - padTest.hold[b]) / HOLD_MS) : 0;
      $('#' + id).style.setProperty('--p', p);
      if (p >= 1 && padTest.hold[b] !== -1) {
        padTest.hold[b] = -1; // fire once per hold
        if (b === 9 && padTest.phase !== 'idle' && padTest.phase !== 'done') continue;
        fn();
        if (!padTest.open) return;
      }
    }
    if (padTest.phase === 'rest' || padTest.phase === 'circle') calibrationStep(gp, now);
    // numbers
    const pct1 = (v) => `${v >= 0 ? '+' : '−'}${Math.abs(v * 100).toFixed(0)}`;
    const pct = (a, b) => `${pct1(a)} / ${pct1(b)}`;
    const [lx, ly] = padAxes(gp, 0), [rx, ry] = padAxes(gp, 1);
    $('#ptStats').innerHTML = `
      <div><span>Левый стик</span><b>${pct(gp.axes[0] ?? 0, gp.axes[1] ?? 0)}</b><em>после калибровки ${pct(lx, ly)}</em></div>
      <div><span>Правый стик</span><b>${pct(gp.axes[2] ?? 0, gp.axes[3] ?? 0)}</b><em>после калибровки ${pct(rx, ry)}</em></div>
      <div><span>Курки</span><b>${Math.round((gp.buttons[6]?.value || 0) * 100)}% · ${Math.round((gp.buttons[7]?.value || 0) * 100)}%</b></div>
      <div><span>Нажато</span><b>${gp.buttons.map((b, i) => (b.pressed ? i : null)).filter((i) => i !== null).join(' · ') || '—'}</b></div>`;
  } else {
    $('#ptName').textContent = 'Геймпад не найден';
  }
  const st = $('#ptStatus');
  const el = now - padTest.t0;
  const cal = gp && data.settings.padCal?.[gp.id];
  st.className = 'pt-status ' + padTest.phase;
  st.innerHTML =
    padTest.phase === 'rest' ? `<b>Шаг 1 из 2.</b> Отпустите стики и не трогайте геймпад… <i style="--p:${Math.min(1, el / 2500)}"></i>`
    : padTest.phase === 'circle' ? `<b>Шаг 2 из 2.</b> Медленно покрутите оба стика по кругу до упора… <i style="--p:${Math.min(1, el / 5000)}"></i>`
    : cal ? `Калибровка сохранена ${fmtDate(cal.at)}. Мёртвая зона: ${Math.round(Math.max(cal.axes[0].dz, cal.axes[1].dz) * 100)}% / ${Math.round(Math.max(cal.axes[2].dz, cal.axes[3].dz) * 100)}%`
    : gp ? 'Нажимайте кнопки и двигайте стики — всё подсветится на схеме. Если меню «уплывает» само, откалибруйте стики.'
    : 'Подключите геймпад по USB или Bluetooth и нажмите любую кнопку.';
  padTest.raf = requestAnimationFrame(padTestFrame);
}
