// Renders the app icon and installer artwork from HTML and writes them in the formats NSIS wants.
// Run with: npx electron build/make-assets.js
const { app, BrowserWindow } = require('electron');
const fs = require('fs');
const path = require('path');

const OUT = __dirname;
const version = require('../package.json').version;

const ICON_SVG = (size) => `
<svg width="${size}" height="${size}" viewBox="0 0 256 256" xmlns="http://www.w3.org/2000/svg">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#2f7bff"/><stop offset="0.55" stop-color="#0b3aa8"/><stop offset="1" stop-color="#050f33"/>
    </linearGradient>
    <radialGradient id="glow" cx="0.3" cy="0.2" r="0.8">
      <stop offset="0" stop-color="#9cc3ff" stop-opacity="0.55"/><stop offset="1" stop-color="#9cc3ff" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="tri" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#ffffff"/><stop offset="1" stop-color="#cfe0ff"/>
    </linearGradient>
    <filter id="sh" x="-30%" y="-30%" width="160%" height="160%"><feDropShadow dx="0" dy="6" stdDeviation="8" flood-color="#000a2a" flood-opacity="0.55"/></filter>
  </defs>
  <rect x="12" y="12" width="232" height="232" rx="56" fill="url(#bg)"/>
  <rect x="12" y="12" width="232" height="232" rx="56" fill="url(#glow)"/>
  <rect x="13.5" y="13.5" width="229" height="229" rx="55" fill="none" stroke="#ffffff" stroke-opacity="0.18" stroke-width="3"/>
  <path d="M100 74 Q92 70 92 80 V176 Q92 186 100 182 L184 136 Q192 128 184 120 Z" fill="url(#tri)" filter="url(#sh)"/>
</svg>`;

const PAGE = (w, h, body) => `<!doctype html><html><head><style>
  html,body{margin:0;width:${w}px;height:${h}px;overflow:hidden;background:transparent;font-family:'Segoe UI Variable Display','Segoe UI',sans-serif;color:#fff}
</style></head><body>${body}</body></html>`;

const SIDEBAR = PAGE(164, 314, `
  <div style="position:absolute;inset:0;background:radial-gradient(ellipse at 30% 10%,#2c6dff 0%,#0b2a7a 38%,#040b26 75%,#02050f 100%)"></div>
  <div style="position:absolute;left:-40px;top:150px;width:260px;height:2px;background:linear-gradient(90deg,transparent,rgba(140,190,255,.55),transparent);transform:rotate(-24deg)"></div>
  <div style="position:absolute;left:-40px;top:190px;width:260px;height:1px;background:linear-gradient(90deg,transparent,rgba(140,190,255,.35),transparent);transform:rotate(-24deg)"></div>
  <div style="position:absolute;left:22px;top:30px">${ICON_SVG(64)}</div>
  <div style="position:absolute;left:24px;top:112px;font-size:21px;font-weight:600;letter-spacing:.2px">PS5 Launcher</div>
  <div style="position:absolute;left:24px;top:142px;font-size:12px;line-height:17px;color:rgba(255,255,255,.72)">Игровой лаунчер<br>для Windows</div>
  <div style="position:absolute;left:24px;bottom:22px;font-size:11px;color:rgba(255,255,255,.5)">Версия ${version}</div>`);

const HEADER = PAGE(150, 57, `
  <div style="position:absolute;inset:0;background:linear-gradient(100deg,#ffffff 0%,#ffffff 35%,#eaf1ff 100%)"></div>
  <div style="position:absolute;right:10px;top:8px">${ICON_SVG(40)}</div>`);

async function render(html, w, h) {
  const win = new BrowserWindow({ width: w, height: h, show: false, frame: false, transparent: true, useContentSize: true, webPreferences: { offscreen: true } });
  const tmp = path.join(app.getPath('temp'), `ps5l-asset-${w}x${h}.html`);
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
  console.log('assets written to', OUT);
  app.quit();
});
