// ---------- Discord status: what we send, and a preview of how friends see it ----------
// buildActivity() is the single source for both the Rich Presence payload (profile.js → updateDiscord)
// and the Discord-style card drawn in Settings → "Статус в Discord" and in the Control Center.
// The app's name and icon come from Discord's public application info, so the preview matches what
// Discord shows even after the application is renamed in the developer portal.

const presence = { app: null, tick: 0 };

async function discordAppInfo() {
  if (presence.app) return presence.app;
  const cached = data.settings.discordApp;
  try {
    const r = await fetch(`https://discord.com/api/v10/applications/${DISCORD_APP_ID}/rpc`);
    if (r.ok) {
      const j = await r.json();
      presence.app = { name: j.name || 'OTIUM', icon: j.icon ? `https://cdn.discordapp.com/app-icons/${j.id}/${j.icon}.png?size=128` : APP_ICON_URL };
      if (JSON.stringify(cached) !== JSON.stringify(presence.app)) { data.settings.discordApp = presence.app; save(); }
      return presence.app;
    }
  } catch {}
  return (presence.app = cached || { name: 'OTIUM', icon: APP_ICON_URL });
}

// ---------- game art Discord can show ----------
// Discord only displays images by public URL. Steam games have their store art; for everything else
// (exe games with a cover picked from disk) we look the game up once — Steam store first, then
// Wikipedia's article image (usually the box art) — and remember the result on the game.
// Numbers in the name must match ("UFC 6" never borrows "UFC 5"'s cover).
const nums = (s) => (String(s).match(/\d+/g) || []).join(' ');
const plainName = (s) => String(s).toLowerCase().replace(/[™®©]/g, '').replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
function sameGame(name, title) {
  if (nums(name) !== nums(title.replace(/\((?:\d{4} )?video game\)|\b(19|20)\d{2}\b/gi, ''))) return false;
  // the whole name, in order, spaces ignored: "Drive Club" fits "Driveclub" but not "Club Drive"
  return plainName(title).replace(/ /g, '').includes(plainName(name).replace(/ /g, ''));
}
async function findPublicArt(name) {
  try {
    const r = await (await fetch(`https://store.steampowered.com/api/storesearch/?term=${encodeURIComponent(name)}&cc=US&l=english`)).json();
    const hit = (r.items || []).find((it) => sameGame(name, it.name));
    if (hit) return `${STEAM_CDN}/${hit.id}/header.jpg`;
  } catch {}
  // as typed, then written together ("Drive Club" → "Driveclub")
  const terms = [...new Set([name, plainName(name).replace(/ /g, '')])];
  for (const term of terms) {
    try {
      const q = `https://en.wikipedia.org/w/api.php?action=query&format=json&origin=*&generator=search&gsrlimit=5&prop=pageimages&piprop=original&pilicense=any&gsrsearch=${encodeURIComponent(term + ' video game')}`;
      const pages = Object.values((await (await fetch(q)).json()).query?.pages || {}).sort((a, b) => a.index - b.index);
      const hit = pages.find((p) => p.original?.source && sameGame(name, p.title));
      if (hit) return hit.original.source.replace(/\?.*$/, '');
    } catch {}
  }
  return '';
}
// returns a public URL for the game's art (cached), '' when nothing fits
async function ensureGameArt(g) {
  if (!g || steamIdOf(g)) return '';
  if (/^https:\/\//i.test(g.cover || '')) return g.cover;
  if (g.publicArt?.for === g.name) return g.publicArt.url;
  const url = await findPublicArt(g.name);
  g.publicArt = { for: g.name, url };
  save();
  return url;
}
function gameArt(g) {
  const sid = steamIdOf(g);
  if (sid) return `${STEAM_CDN}/${sid}/header.jpg`;
  if (/^https:\/\//i.test(g.cover || '')) return g.cover;
  return g.publicArt?.for === g.name ? g.publicArt.url : '';
}

// what Discord gets right now (null = nothing / turned off)
function buildActivity() {
  const s = data.settings;
  const g = data.games.find((x) => running.has(x.id));
  if (g) {
    const sid = steamIdOf(g);
    const hide = !!s.discordHideGame;
    const act = {
      details: hide ? 'Играет в игру' : `Играет в ${g.name}`,
      state: (s.discordState || '').trim() || 'через OTIUM',
      assets: {
        large_image: (!hide && gameArt(g)) || APP_ICON_URL,
        large_text: hide ? 'OTIUM' : g.name,
        small_image: APP_ICON_URL,
        small_text: 'OTIUM'
      }
    };
    if (!s.discordNoTimer) act.timestamps = { start: steamSince[sid] || launchStarted[g.id] || Date.now() };
    return act;
  }
  return {
    details: 'В главном меню',
    state: `${data.games.length} ${plural(data.games.length, 'игра', 'игры', 'игр')} в библиотеке`,
    assets: { large_image: APP_ICON_URL, large_text: 'OTIUM' }
  };
}

function fmtElapsed(ms) {
  const t = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(t / 3600), m = Math.floor((t % 3600) / 60), sec = t % 60;
  const p = (n) => String(n).padStart(2, '0');
  return h ? `${p(h)}:${p(m)}:${p(sec)}` : `${p(m)}:${p(sec)}`;
}

// a Discord profile card with the activity block; `mini` = just the activity, for the Control Center
function presenceCard(act, { mini = false } = {}) {
  const app = presence.app || data.settings.discordApp || { name: 'OTIUM', icon: APP_ICON_URL };
  const name = data.settings.user || 'Player';
  const h = hue(name);
  const big = act.assets?.large_image;
  const small = act.assets?.small_image;
  const timer = act.timestamps?.start ? `<div class="rp-timer" data-start="${act.timestamps.start}">${fmtElapsed(Date.now() - act.timestamps.start)} прошло</div>` : '';
  const activity = `
    <div class="rp-act">
      <div class="rp-art">
        <div class="rp-big" style="background-image:${big ? esc(cssUrl(big)) : 'none'}"></div>
        ${small ? `<div class="rp-small" style="background-image:${esc(cssUrl(small))}"></div>` : ''}
      </div>
      <div class="rp-lines">
        <div class="rp-app">${esc(app.name)}</div>
        ${act.details ? `<div class="rp-l">${esc(act.details)}</div>` : ''}
        ${act.state ? `<div class="rp-l dim">${esc(act.state)}</div>` : ''}
        ${timer}
      </div>
    </div>`;
  if (mini) return `<div class="rp-card mini"><div class="rp-kick"><span class="rp-dlogo">${DISCORD_LOGO}</span>Так вас видят в Discord</div>${activity}</div>`;
  return `
    <div class="rp-card">
      <div class="rp-banner" style="--h:${h}"></div>
      <div class="rp-avatar" style="--h:${h}">${esc(initials(name))}<i></i></div>
      <div class="rp-body">
        <div class="rp-name">${esc(name)}</div>
        <div class="rp-tag">в сети</div>
        <div class="rp-sep"></div>
        <div class="rp-sec">${running.size ? 'Играет' : 'Активность'}</div>
        ${activity}
      </div>
    </div>`;
}

const DISCORD_LOGO = '<svg viewBox="0 0 24 24"><path d="M19.6 5.3A17 17 0 0 0 15.4 4l-.5 1a15.6 15.6 0 0 0-5.8 0L8.6 4a17 17 0 0 0-4.2 1.3C1.7 9.3 1 13.2 1.3 17a17 17 0 0 0 5.2 2.6l1.1-1.8a11 11 0 0 1-1.7-.8l.4-.3a12.2 12.2 0 0 0 11.4 0l.4.3a11 11 0 0 1-1.7.8l1.1 1.8a17 17 0 0 0 5.2-2.6c.4-4.4-.7-8.3-3.1-11.7zM8.7 14.7c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1zm6.6 0c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1z"/></svg>';

// keep the "прошло" timers running wherever a card is on screen
setInterval(() => {
  for (const el of document.querySelectorAll('.rp-timer[data-start]')) el.textContent = `${fmtElapsed(Date.now() - +el.dataset.start)} прошло`;
}, 1000);

// ---------- Settings → "Статус в Discord" ----------
async function openDiscordScreen(focus = 0) {
  const s = data.settings;
  const on = !!s.discordId;
  const connected = on && (await ipc.discordStatus().catch(() => false));
  await discordAppInfo();
  await ensureGameArt(data.games.find((x) => running.has(x.id)));
  const row = (id, label, val) => `<button class="menu-item focusable" data-id="${id}">${esc(label)}<span class="val">${esc(val)}</span></button>`;
  openModal(`
    <div class="rp-wrap">
      <div class="rp-left">
        <div class="pt-kicker">ПРЕДПРОСМОТР</div>
        <div id="rpPreview">${on ? presenceCard(buildActivity()) : '<div class="rp-off">Статус выключен — друзья в Discord не видят, во что вы играете</div>'}</div>
        <div class="rp-conn ${connected ? 'ok' : on ? 'warn' : ''}">${connected ? '● Discord подключён — друзья видят этот статус' : on ? '● Discord не запущен — статус появится, когда вы его откроете' : '● Выключено'}</div>
      </div>
      <div class="rp-right">
        <h2>Статус в Discord</h2>
        <div class="rp-hint">Как в Discord: «Играет в …» с обложкой игры и временем. Работает сам, если Discord запущен на этом ПК.</div>
        ${row('on', 'Показывать статус', on ? 'Вкл' : 'Выкл')}
        ${row('game', 'Название и обложка игры', s.discordHideGame ? 'Скрыты' : 'Видны')}
        ${row('timer', 'Время в игре', s.discordNoTimer ? 'Скрыто' : 'Видно')}
        <div class="rp-field"><span>Вторая строка</span><input class="focusable" id="rpState" maxlength="64" placeholder="через OTIUM" value="${esc(s.discordState || '')}"></div>
      </div>
    </div>`, { cls: 'presence', focus });
  const items = $$('#modal .rp-right .menu-item');
  items.forEach((b) => (b.onclick = async () => {
    const id = b.dataset.id;
    const at = modalEls().indexOf(b);
    if (id === 'on') { await toggleDiscord(); return openDiscordScreen(at); }
    if (id === 'game') s.discordHideGame = !s.discordHideGame;
    if (id === 'timer') s.discordNoTimer = !s.discordNoTimer;
    await save();
    updateDiscord();
    openDiscordScreen(at);
  }));
  const inp = $('#rpState');
  let t;
  inp.oninput = () => {
    s.discordState = inp.value;
    const p = $('#rpPreview .rp-lines .rp-l.dim');
    if (p && running.size) p.textContent = inp.value.trim() || 'через OTIUM';
    clearTimeout(t);
    t = setTimeout(() => { save(); updateDiscord(); }, 700);
  };
}

// small card for the Control Center while a game runs
function presenceMini() {
  if (!data.settings.discordId || !running.size) return '';
  return presenceCard(buildActivity(), { mini: true });
}
