// ---------- Startup splash: glow, drifting lights, the play mark and a soft synth swell ----------
// Plays once per launcher start (the renderer never reloads while the app lives in the tray).
// index.html shows #splash from the very first frame; app.js calls splashPlay() once the home screen
// is laid out underneath, or splashSkip(true) when the splash is turned off.
const splash = { active: !!document.getElementById('splash'), timers: [], raf: 0, ignoreUntil: 0 };

// timeline, ms from splashPlay()
const SPLASH_T = { mark: 700, word: 1100, sheen: 2050, out: 3000, gone: 3900 };

function splashPlay({ sound = true } = {}) {
  const el = document.getElementById('splash');
  if (!el) return splashDone();
  splashLights(el.querySelector('canvas'));
  el.classList.add('play');
  if (sound) splashSound();
  splash.timers.push(setTimeout(() => splashOut(), SPLASH_T.out));
}

// leave: the splash dissolves forward while the home screen settles in from behind it
function splashOut(fast) {
  const el = document.getElementById('splash');
  if (!el || el.classList.contains('out')) return;
  splash.timers.forEach(clearTimeout);
  el.classList.toggle('fast', !!fast);
  el.classList.add('out');
  splashReveal();
  splash.active = false;
  setTimeout(() => { cancelAnimationFrame(splash.raf); el.remove(); }, fast ? 450 : SPLASH_T.gone - SPLASH_T.out);
}

// any key, click or pad button skips; the press itself must not reach the launcher
function splashSkip(instant) {
  if (!splash.active) return false;
  splash.ignoreUntil = performance.now() + 450;
  if (splashMaster) splashMaster.gain.setTargetAtTime(0.0001, actx.currentTime, 0.25);
  if (instant) {
    cancelAnimationFrame(splash.raf);
    document.getElementById('splash')?.remove();
    document.body.classList.remove('intro');
    splash.active = false;
    return true;
  }
  splashOut(true);
  return true;
}
const splashBusy = () => splash.active || performance.now() < splash.ignoreUntil;
function splashDone() { splash.active = false; document.body.classList.remove('intro'); }
function splashReveal() {
  document.body.classList.add('revealing');
  document.body.classList.remove('intro');
  setTimeout(() => document.body.classList.remove('revealing'), 1300);
}

window.addEventListener('keydown', (e) => { if (splash.active) { e.stopImmediatePropagation(); e.preventDefault(); splashSkip(); } }, true);
window.addEventListener('mousedown', (e) => { if (splash.active) { e.stopPropagation(); e.preventDefault(); splashSkip(); } }, true);

// Soft lights drifting upwards, additive, a slow twinkle each
function splashLights(cv) {
  if (!cv) return;
  const dpr = Math.min(devicePixelRatio || 1, 2);
  const ctx = cv.getContext('2d');
  const size = () => { cv.width = innerWidth * dpr; cv.height = innerHeight * dpr; };
  size();
  const W = () => cv.width, H = () => cv.height;
  const dots = Array.from({ length: 34 }, () => ({
    x: Math.random(), y: Math.random() * 1.1,
    r: (6 + Math.random() ** 2 * 46) * dpr,
    v: 0.012 + Math.random() * 0.03,
    sway: Math.random() * Math.PI * 2,
    tw: 0.4 + Math.random() * 1.2,
    a: 0.12 + Math.random() * 0.35,
    hue: 200 + Math.random() * 30
  }));
  const t0 = performance.now();
  const frame = (now) => {
    const t = (now - t0) / 1000;
    ctx.clearRect(0, 0, W(), H());
    ctx.globalCompositeOperation = 'lighter';
    const fadeIn = Math.min(1, t / 1.4);
    for (const d of dots) {
      const y = ((d.y - t * d.v) % 1.1 + 1.1) % 1.1 - 0.05;
      const x = d.x + Math.sin(t * 0.35 + d.sway) * 0.012;
      const a = d.a * fadeIn * (0.55 + 0.45 * Math.sin(t * d.tw + d.sway));
      const px = x * W(), py = y * H();
      const g = ctx.createRadialGradient(px, py, 0, px, py, d.r);
      g.addColorStop(0, `hsla(${d.hue},100%,80%,${a})`);
      g.addColorStop(0.35, `hsla(${d.hue},100%,65%,${a * 0.35})`);
      g.addColorStop(1, `hsla(${d.hue},100%,50%,0)`);
      ctx.fillStyle = g;
      ctx.beginPath();
      ctx.arc(px, py, d.r, 0, Math.PI * 2);
      ctx.fill();
    }
    splash.raf = requestAnimationFrame(frame);
  };
  splash.raf = requestAnimationFrame(frame);
  addEventListener('resize', size);
}

// ---------- Sound: a warm Dmaj9 pad opening up through a filter, a big room, then three soft bells ----------
let splashMaster = null;
function splashSound() {
  try {
    actx ??= new AudioContext();
    const c = actx;
    if (c.state === 'suspended') c.resume();
    const t0 = c.currentTime + 0.05;

    const master = (splashMaster = c.createGain());
    master.gain.value = 0.8;
    master.connect(c.destination);
    // generated hall: stereo noise with an exponential tail
    const rev = c.createConvolver();
    rev.buffer = splashImpulse(c, 3.6, 2.4);
    const wet = c.createGain();
    wet.gain.value = 0.6;
    rev.connect(wet).connect(master);
    const bus = c.createGain();
    bus.connect(master);
    bus.connect(rev);

    // pad: two slightly detuned voices per note, through a lowpass that opens as the mark appears
    const lp = c.createBiquadFilter();
    lp.type = 'lowpass';
    lp.Q.value = 0.8;
    lp.frequency.setValueAtTime(220, t0);
    lp.frequency.exponentialRampToValueAtTime(4200, t0 + 2.1);
    lp.frequency.exponentialRampToValueAtTime(1400, t0 + 5);
    const pad = c.createGain();
    pad.gain.setValueAtTime(0.0001, t0);
    pad.gain.exponentialRampToValueAtTime(0.085, t0 + 1.5);
    pad.gain.setTargetAtTime(0.0001, t0 + 2.9, 0.8);
    lp.connect(pad).connect(bus);
    const notes = [73.42, 146.83, 220, 369.99, 554.37, 659.25]; // D2 D3 A3 F#4 C#5 E5
    notes.forEach((f, i) => {
      for (const [type, det, lvl] of [['sine', -7, 0.6], ['triangle', 6, 0.32]]) {
        const o = c.createOscillator();
        o.type = type;
        o.frequency.value = f;
        o.detune.value = det;
        const g = c.createGain();
        g.gain.value = lvl * (i === 0 ? 0.9 : f > 500 ? 0.45 : 1);
        o.connect(g).connect(lp);
        o.start(t0 + i * 0.06);
        o.stop(t0 + 7.5);
      }
    });

    // bells on the sheen: sine + an inharmonic partial, quick attack, long ring
    const bell = (f, at, vol) => {
      const t = t0 + at;
      const g = c.createGain();
      g.gain.setValueAtTime(0.0001, t);
      g.gain.exponentialRampToValueAtTime(vol, t + 0.008);
      g.gain.exponentialRampToValueAtTime(0.0001, t + 2.8);
      g.connect(bus);
      for (const [mul, lvl] of [[1, 1], [2.76, 0.18], [5.4, 0.05]]) {
        const o = c.createOscillator();
        o.frequency.value = f * mul;
        const og = c.createGain();
        og.gain.value = lvl;
        o.connect(og).connect(g);
        o.start(t);
        o.stop(t + 3);
      }
    };
    const s = SPLASH_T.sheen / 1000;
    bell(1318.5, s, 0.05);        // E6
    bell(1760, s + 0.13, 0.035);   // A6
    bell(2217.5, s + 0.26, 0.022); // C#7
  } catch {}
}
function splashImpulse(c, secs, decay) {
  const n = Math.floor(c.sampleRate * secs);
  const buf = c.createBuffer(2, n, c.sampleRate);
  for (let ch = 0; ch < 2; ch++) {
    const d = buf.getChannelData(ch);
    for (let i = 0; i < n; i++) d[i] = (Math.random() * 2 - 1) * (1 - i / n) ** decay;
  }
  return buf;
}
