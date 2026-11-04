// Renders the app icon and installer artwork from HTML and writes them in the formats NSIS wants.
// Run with: npx electron build/make-assets.js
const { app, BrowserWindow } = require('electron');
const fs = require('fs');
const path = require('path');

const OUT = __dirname;
const version = require('../package.json').version;

// OTIUM mark: a platinum ring broken at the top right, a spark of light in the gap
const RING = (id) => `
    <linearGradient id="${id}ring" x1="0.15" y1="0.1" x2="0.85" y2="0.95">
      <stop offset="0" stop-color="#ffffff"/><stop offset="0.45" stop-color="#d9e8ff"/><stop offset="1" stop-color="#6fa3ff"/>
    </linearGradient>
    <radialGradient id="${id}spark" cx="0.5" cy="0.5" r="0.5">
      <stop offset="0" stop-color="#ffffff"/><stop offset="0.35" stop-color="#cfe3ff" stop-opacity="0.9"/><stop offset="1" stop-color="#5b9bff" stop-opacity="0"/>
    </radialGradient>`;
const MARK = (id, cx, cy, r, w) => {
  const a = -Math.PI / 4;
  const sx = cx + r * Math.cos(a), sy = cy + r * Math.sin(a);
  return `
  <circle cx="${cx}" cy="${cy}" r="${r}" fill="none" stroke="url(#${id}ring)" stroke-width="${w}" stroke-linecap="round"
          pathLength="100" stroke-dasharray="84 16" transform="rotate(-${(16 / 2 + 84) * 3.6 - 315} ${cx} ${cy})"/>
  <circle cx="${sx}" cy="${sy}" r="${w * 1.6}" fill="url(#${id}spark)"/>
  <circle cx="${sx}" cy="${sy}" r="${w * 0.42}" fill="#ffffff"/>`;
};

const ICON_SVG = (size) => `
<svg width="${size}" height="${size}" viewBox="0 0 256 256" xmlns="http://www.w3.org/2000/svg">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#14306f"/><stop offset="0.5" stop-color="#081a45"/><stop offset="1" stop-color="#02060f"/>
    </linearGradient>
    <radialGradient id="glow" cx="0.32" cy="0.22" r="0.85">
      <stop offset="0" stop-color="#7fb0ff" stop-opacity="0.42"/><stop offset="1" stop-color="#7fb0ff" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="edge" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.35"/><stop offset="0.5" stop-color="#ffffff" stop-opacity="0.06"/><stop offset="1" stop-color="#ffffff" stop-opacity="0.16"/>
    </linearGradient>
    ${RING('i')}
    <filter id="sh" x="-30%" y="-30%" width="160%" height="160%"><feDropShadow dx="0" dy="5" stdDeviation="7" flood-color="#000614" flood-opacity="0.6"/></filter>
  </defs>
  <rect x="12" y="12" width="232" height="232" rx="58" fill="url(#bg)"/>
  <rect x="12" y="12" width="232" height="232" rx="58" fill="url(#glow)"/>
  <rect x="13.5" y="13.5" width="229" height="229" rx="56.5" fill="none" stroke="url(#edge)" stroke-width="3"/>
  <g filter="url(#sh)">${MARK('i', 128, 130, 66, size <= 32 ? 22 : 17)}</g>
</svg>`;

// wordmark: the ring stands in for the O
const WORDMARK = (h, color = '#eef4ff') => `
<div style="display:flex;align-items:center;gap:${h * 0.16}px;height:${h}px">
  <svg width="${h}" height="${h}" viewBox="0 0 120 120"><defs>${RING('w')}</defs>${MARK('w', 60, 60, 46, 9)}</svg>
  <div style="font-family:'Segoe UI Variable Display','Segoe UI';font-weight:300;font-size:${h * 0.78}px;line-height:1;letter-spacing:${h * 0.2}px;color:${color};margin-top:${-h * 0.04}px">TIUM</div>
</div>`;

const PAGE = (w, h, body) => `<!doctype html><html><head><style>
  html,body{margin:0;width:${w}px;height:${h}px;overflow:hidden;background:transparent;font-family:'Segoe UI Variable Display','Segoe UI',sans-serif;color:#fff}
</style></head><body>${body}</body></html>`;

const NIGHT = 'radial-gradient(ellipse at 28% 12%,#1b4bb5 0%,#0a2363 30%,#040c28 62%,#01030b 100%)';

const SIDEBAR = PAGE(164, 314, `
  <div style="position:absolute;inset:0;background:${NIGHT}"></div>
  <div style="position:absolute;left:-30px;top:-60px;width:220px;height:220px;border-radius:50%;background:radial-gradient(circle,rgba(120,170,255,.35),rgba(120,170,255,0) 65%)"></div>
  <div style="position:absolute;left:-40px;top:236px;width:260px;height:1px;background:linear-gradient(90deg,transparent,rgba(170,205,255,.55),transparent);transform:rotate(-16deg)"></div>
  <div style="position:absolute;left:28px;top:44px">${ICON_SVG(108)}</div>
  <div style="position:absolute;left:0;right:0;top:172px;text-align:center;font-weight:300;font-size:25px;letter-spacing:7px;padding-left:7px">OTIUM</div>
  <div style="position:absolute;left:0;right:0;top:206px;text-align:center;font-size:10.5px;letter-spacing:2.4px;color:rgba(200,220,255,.62)">ИГРОВОЙ ЛАУНЧЕР</div>
  <div style="position:absolute;left:0;right:0;bottom:20px;text-align:center;font-size:10px;letter-spacing:1.5px;color:rgba(200,220,255,.42)">ВЕРСИЯ ${version}</div>`);

const HEADER = PAGE(150, 57, `
  <div style="position:absolute;inset:0;background:linear-gradient(100deg,#ffffff 0%,#ffffff 30%,#e8efff 100%)"></div>
  <div style="position:absolute;right:12px;top:9px">${ICON_SVG(39)}</div>`);

// README / GitHub banner
const BANNER = PAGE(1280, 400, `
  <div style="position:absolute;inset:0;background:radial-gradient(ellipse at 50% 42%,#123a8f 0%,#071a4a 34%,#020a1f 66%,#000510 100%)"></div>
  <div style="position:absolute;left:50%;top:44%;width:900px;height:900px;margin:-450px 0 0 -450px;border-radius:50%;background:radial-gradient(circle,rgba(110,165,255,.32),rgba(110,165,255,0) 52%)"></div>
  ${[[180, 80, 3], [330, 300, 2], [1010, 90, 2.5], [1120, 280, 3], [610, 350, 2], [860, 330, 1.6], [90, 250, 1.8], [740, 60, 1.5]].map(([x, y, r]) =>
    `<div style="position:absolute;left:${x}px;top:${y}px;width:${r * 8}px;height:${r * 8}px;border-radius:50%;background:radial-gradient(circle,rgba(210,230,255,.9),rgba(120,170,255,0) 60%)"></div>`).join('')}
  <div style="position:absolute;left:0;right:0;top:118px;display:flex;justify-content:center">${WORDMARK(118)}</div>
  <div style="position:absolute;left:0;right:0;top:282px;text-align:center;font-size:19px;letter-spacing:9px;color:rgba(205,222,255,.7)">ИГРЫ · ДРУЗЬЯ · ДИВАН</div>`);

async function render(html, w, h) {
  const win = new BrowserWindow({ width: w, height: h, show: false, frame: false, transparent: true, useContentSize: true, webPreferences: { offscreen: true } });
  const tmp = path.join(app.getPath('temp'), `otium-asset-${w}x${h}.html`);
  fs.writeFileSync(tmp, html);
  await win.loadFile(tmp);
  await new Promise((r) => setTimeout(r, 400));
  const img = await win.webContents.capturePage({ x: 0, y: 0, width: w, height: h });
  win.destroy();
  return img.resize({ width: w, height: h, quality: 'best' });
}

// 24-bit bottom-up BMP (what NSIS expects for sidebar/header images)
function toBmp(img) {
  const { width: w, height: h } = img.getSize();
  const bgra = img.toBitmap();
  const rowSize = Math.ceil((w * 3) / 4) * 4;
  const buf = Buffer.alloc(54 + rowSize * h);
  buf.write('BM', 0);
  buf.writeUInt32LE(buf.length, 2);
  buf.writeUInt32LE(54, 10);
  buf.writeUInt32LE(40, 14);
  buf.writeInt32LE(w, 18);
  buf.writeInt32LE(h, 22);
  buf.writeUInt16LE(1, 26);
  buf.writeUInt16LE(24, 28);
  buf.writeUInt32LE(rowSize * h, 34);
  for (let y = 0; y < h; y++) {
    const dst = 54 + (h - 1 - y) * rowSize;
    for (let x = 0; x < w; x++) {
      const s = (y * w + x) * 4;
      buf[dst + x * 3] = bgra[s];
      buf[dst + x * 3 + 1] = bgra[s + 1];
      buf[dst + x * 3 + 2] = bgra[s + 2];
    }
  }
  return buf;
}

// ICO with PNG-compressed entries (supported since Windows Vista)
function toIco(pngs) {
  const header = Buffer.alloc(6 + 16 * pngs.length);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(pngs.length, 4);
  let offset = header.length;
  pngs.forEach(({ size, data }, i) => {
    const e = 6 + i * 16;
    header[e] = size >= 256 ? 0 : size;
    header[e + 1] = size >= 256 ? 0 : size;
    header.writeUInt16LE(1, e + 4);
    header.writeUInt16LE(32, e + 6);
    header.writeUInt32LE(data.length, e + 8);
    header.writeUInt32LE(offset, e + 12);
    offset += data.length;
  });
  return Buffer.concat([header, ...pngs.map((p) => p.data)]);
}

// each render closes its window; don't let that end the script
app.on('window-all-closed', () => {});

app.whenReady().then(async () => {
  const sizes = [256, 64, 48, 32, 24, 16];
  const pngs = [];
  for (const s of sizes) {
    const img = await render(PAGE(s, s, ICON_SVG(s)), s, s);
    pngs.push({ size: s, data: img.toPNG() });
  }
  fs.writeFileSync(path.join(OUT, 'icon.ico'), toIco(pngs));
  fs.writeFileSync(path.join(OUT, 'icon.png'), pngs[0].data);
  fs.copyFileSync(path.join(OUT, 'icon.png'), path.join(OUT, '..', 'renderer', 'icon.png'));

  const side = await render(SIDEBAR, 164, 314);
  fs.writeFileSync(path.join(OUT, 'installerSidebar.bmp'), toBmp(side));
  fs.writeFileSync(path.join(OUT, 'preview-sidebar.png'), side.toPNG());
  const head = await render(HEADER, 150, 57);
  fs.writeFileSync(path.join(OUT, 'installerHeader.bmp'), toBmp(head));
  fs.writeFileSync(path.join(OUT, 'banner.png'), (await render(BANNER, 1280, 400)).toPNG());
  fs.writeFileSync(path.join(OUT, 'icon-preview.png'), (await render(PAGE(512, 512, ICON_SVG(512)), 512, 512)).toPNG());
  console.log('assets written to', OUT);
  app.quit();
});
