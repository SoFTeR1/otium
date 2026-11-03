// Everything that talks to the local Steam install or the Steam Web API
const fs = require('fs');
const path = require('path');
const { execFile } = require('child_process');
const { pathToFileURL } = require('url');

const CDN = 'https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps';
const CLAN_IMG = 'https://clan.cloudflare.steamstatic.com/images';
const STEAMID_BASE = 76561197960265728n;
const EXCLUDE = /redistributable|proton|steam linux runtime|steamvr|steamworks|soundtrack|dedicated server|\bsdk\b/i;

function regQuery(key, value) {
  return new Promise((resolve) => {
    execFile('reg', ['query', key, '/v', value], { windowsHide: true }, (err, stdout) => {
      if (err) return resolve(null);
      const m = stdout.match(new RegExp(value + '\\s+REG_\\w+\\s+(.+)'));
      resolve(m ? m[1].trim() : null);
    });
  });
}

// Minimal parser for Valve's KeyValues (.vdf/.acf) text format
function parseVdf(text) {
  let i = 0;
  const n = text.length;
  const skip = () => {
    while (i < n) {
      const c = text[i];
      if (c === ' ' || c === '\t' || c === '\r' || c === '\n') i++;
      else if (c === '/' && text[i + 1] === '/') while (i < n && text[i] !== '\n') i++;
      else break;
    }
  };
  const str = () => {
    i++;
    let s = '';
    while (i < n && text[i] !== '"') {
      if (text[i] === '\\' && i + 1 < n) { s += text[i + 1]; i += 2; } else s += text[i++];
    }
    i++;
    return s;
  };
  const bare = () => {
    let s = '';
    while (i < n && !/[\s{}"]/.test(text[i])) s += text[i++];
    return s;
  };
  const obj = () => {
    const o = {};
    for (;;) {
      skip();
      if (i >= n) return o;
      if (text[i] === '}') { i++; return o; }
      const k = text[i] === '"' ? str() : bare();
      skip();
      if (text[i] === '{') { i++; o[k] = obj(); } else o[k] = text[i] === '"' ? str() : bare();
    }
  };
  return obj();
}

// Case-insensitive path lookup: get(o, 'Software', 'Valve', 'Steam')
function get(o, ...keys) {
  for (const k of keys) {
    if (!o || typeof o !== 'object') return undefined;
    const real = Object.keys(o).find((x) => x.toLowerCase() === k.toLowerCase());
    o = real === undefined ? undefined : o[real];
  }
  return o;
}

let rootCache;
async function steamRoot() {
  if (rootCache !== undefined) return rootCache;
  const r =
    (await regQuery('HKCU\\Software\\Valve\\Steam', 'SteamPath')) ||
    (await regQuery('HKLM\\SOFTWARE\\WOW6432Node\\Valve\\Steam', 'InstallPath'));
  rootCache = r ? path.normalize(r) : null;
  return rootCache;
}

function art(root, id, file) {
  const cache = path.join(root, 'appcache', 'librarycache');
  for (const c of [path.join(cache, `${id}_${file}`), path.join(cache, String(id), file)]) {
    if (fs.existsSync(c)) return pathToFileURL(c).href;
  }
  return `${CDN}/${id}/${file}`;
}

async function scan() {
  const root = await steamRoot();
  if (!root) return { error: 'Steam не найден на этом компьютере' };

  // Windows paths are case-insensitive; key by lowercase to avoid scanning a library twice
  const libs = new Map([[root.toLowerCase(), root]]);
  try {
    const vdf = parseVdf(fs.readFileSync(path.join(root, 'steamapps', 'libraryfolders.vdf'), 'utf8'));
    for (const lib of Object.values(get(vdf, 'libraryfolders') || {})) {
      const p = lib?.path && path.normalize(lib.path);
      if (p) libs.set(p.toLowerCase(), p);
    }
  } catch {}

  const games = [];
  const seen = new Set();
  for (const lib of libs.values()) {
    const dir = path.join(lib, 'steamapps');
    let files = [];
    try {
      files = fs.readdirSync(dir).filter((f) => /^appmanifest_\d+\.acf$/.test(f));
    } catch {
      continue;
    }
    for (const f of files) {
      const st = get(parseVdf(fs.readFileSync(path.join(dir, f), 'utf8')), 'AppState') || {};
      const id = st.appid;
      const name = st.name;
      if (!id || !name || id === '228980' || EXCLUDE.test(name) || seen.has(id)) continue;
      seen.add(id);
      games.push({
        name,
        steamId: id,
        path: `steam://rungameid/${id}`,
        cover: art(root, id, 'library_600x900.jpg'),
        hero: art(root, id, 'library_hero.jpg'),
        logo: `${CDN}/${id}/logo.png`
      });
    }
  }
  return { games };
}

// The Steam account that was used last = the userdata folder with the freshest localconfig.vdf
async function currentUser() {
  const root = await steamRoot();
  if (!root) return null;
  let best = null;
  try {
    for (const d of fs.readdirSync(path.join(root, 'userdata'))) {
      if (!/^\d+$/.test(d) || d === '0') continue;
      const f = path.join(root, 'userdata', d, 'config', 'localconfig.vdf');
      try {
        const t = fs.statSync(f).mtimeMs;
        if (!best || t > best.t) best = { t, accountId: d, file: f };
      } catch {}
    }
  } catch {}
  if (!best) return null;
  return { ...best, steamId: String(BigInt(best.accountId) + STEAMID_BASE) };
}

// Per-app playtime (minutes) and last-played (unix seconds) as recorded by the Steam client
async function localStats() {
  const u = await currentUser();
  if (!u) return { steamId: null, apps: {} };
  const apps = {};
  try {
    const vdf = parseVdf(fs.readFileSync(u.file, 'utf8'));
    const list = get(vdf, 'UserLocalConfigStore', 'Software', 'Valve', 'Steam', 'apps') || {};
    for (const [id, a] of Object.entries(list)) {
      if (!a || typeof a !== 'object') continue;
      const playtime = +get(a, 'Playtime') || 0;
      const lastPlayed = +get(a, 'LastPlayed') || 0;
      if (playtime || lastPlayed) apps[id] = { playtime, lastPlayed };
    }
  } catch {}
  return { steamId: u.steamId, apps };
}

// ---------- running game watcher ----------
let runningApp = 0;
function watchRunning(onChange) {
  const poll = async () => {
    const v = await regQuery('HKCU\\Software\\Valve\\Steam', 'RunningAppID');
    const id = v ? parseInt(v, 16) || parseInt(v, 10) || 0 : 0;
    if (id !== runningApp) {
      const prev = runningApp;
      runningApp = id;
      onChange(id, prev);
    }
  };
  poll();
  setInterval(poll, 2500);
}
const running = () => runningApp;

// ---------- Web API helpers ----------
const cache = new Map();
async function cached(key, ttl, fn) {
  const c = cache.get(key);
  if (c && Date.now() - c.t < ttl) return c.v;
  const v = await fn();
  if (v != null) cache.set(key, { t: Date.now(), v });
  return v;
}
async function json(url) {
  const r = await fetch(url, { headers: { 'Accept-Language': 'ru' } });
  if (!r.ok) throw new Error('HTTP ' + r.status);
  return r.json();
}
// Like json() but never throws: { status, data } where data is null for non-JSON bodies
async function probe(url) {
  try {
    const r = await fetch(url);
    let data = null;
    try { data = await r.json(); } catch {}
    return { status: r.status, data };
  } catch {
    return { status: 0, data: null };
  }
}
// Steam answers 403 with an HTML page for a bad key, but 403 + JSON for a private profile
const keyRejected = (p) => (p.status === 401 || p.status === 403) && !p.data;

function newsImage(contents) {
  const m = contents.match(/\{STEAM_CLAN_IMAGE\}[^\s\[\]"'<>]+|https?:\/\/[^\s\[\]"'<>]+\.(?:png|jpe?g|gif|webp)/i);
  return m ? m[0].replace('{STEAM_CLAN_IMAGE}', CLAN_IMG) : null;
}
function newsText(contents) {
  return contents
    .replace(/\[img[^\]]*\][^\[]*\[\/img\]|\[img[^\]]*\]/gi, '')
    .replace(/\[previewyoutube[^\]]*\][^\[]*\[\/previewyoutube\]/gi, '')
    .replace(/\[\*\]/g, '\n• ')
    .replace(/\[(?:\/?p|br|\/h\d|\/list|hr\/?)\]/gi, '\n')
    .replace(/\[h\d\]/gi, '\n\n')
    .replace(/\[[^\]]+\]/g, '')
    .replace(/<br\s*\/?>|<\/p>/gi, '\n')
    .replace(/<[^>]+>/g, '')
    .replace(/\{STEAM_CLAN_IMAGE\}\S*/g, '')
    .replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&').replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&lt;/g, '<').replace(/&gt;/g, '>')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
    .slice(0, 6000);
}
async function fetchNews(appid, feeds) {
  const j = await json(`https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid=${appid}&count=8&maxlength=0&format=json${feeds ? '&feeds=' + feeds : ''}`);
  return (j.appnews?.newsitems || []).map((n) => ({
    id: n.gid, title: n.title, url: n.url, date: n.date * 1000, feed: n.feedlabel || '',
    image: newsImage(n.contents || ''), text: newsText(n.contents || '')
  }));
}
function news(appid) {
  return cached('news' + appid, 30 * 60e3, async () => {
    // official announcements first (they carry images); fall back to all feeds
    let items = await fetchNews(appid, 'steam_community_announcements');
    if (!items.length) items = await fetchNews(appid);
    return items;
  });
}

function achievements(appid, key, steamId) {
  return cached(`ach${appid}:${steamId}:${key.slice(-6)}`, 5 * 60e3, async () => {
    const [p, schema, global] = await Promise.all([
      probe(`https://api.steampowered.com/ISteamUserStats/GetPlayerAchievements/v1/?appid=${appid}&key=${key}&steamid=${steamId}&l=russian`),
      json(`https://api.steampowered.com/ISteamUserStats/GetSchemaForGame/v2/?appid=${appid}&key=${key}&l=russian`).catch(() => ({})),
      json(`https://api.steampowered.com/ISteamUserStats/GetGlobalAchievementPercentagesForApp/v2/?gameid=${appid}`).catch(() => ({}))
    ]);
    if (keyRejected(p)) return { error: 'badkey' };
    if (!p.status) return { error: 'network' };
    const list = p.data?.playerstats?.achievements;
    if (!list) {
      const msg = p.data?.playerstats?.error || '';
      return { error: /not public|private/i.test(msg) ? 'private' : /no stats|has no stats|not found/i.test(msg) || p.status === 400 ? 'none' : 'error', message: msg };
    }
    const meta = Object.fromEntries((schema.game?.availableGameStats?.achievements || []).map((a) => [a.name, a]));
    const pct = Object.fromEntries((global.achievementpercentages?.achievements || []).map((a) => [a.name, +a.percent]));
    const items = list.map((a) => ({
      id: a.apiname,
      name: meta[a.apiname]?.displayName || a.name || a.apiname,
      desc: meta[a.apiname]?.description || a.description || '',
      icon: a.achieved ? meta[a.apiname]?.icon : meta[a.apiname]?.icongray,
      achieved: !!a.achieved,
      time: a.unlocktime * 1000,
      percent: pct[a.apiname] ?? null
    }));
    return { items, total: items.length, done: items.filter((a) => a.achieved).length };
  });
}

function friends(key, steamId) {
  return cached('friends' + steamId + key.slice(-6), 45e3, async () => {
    const fl = await probe(`https://api.steampowered.com/ISteamUser/GetFriendList/v1/?key=${key}&steamid=${steamId}&relationship=friend`);
    if (keyRejected(fl) && fl.status === 403) return { error: 'badkey' };
    if (fl.status === 401) return { error: 'private' };
    if (!fl.status) return { error: 'network' };
    const ids = (fl.data?.friendslist?.friends || []).map((f) => f.steamid).slice(0, 100);
    if (!ids.length) return { list: [] };
    const s = await json(`https://api.steampowered.com/ISteamUser/GetPlayerSummaries/v2/?key=${key}&steamids=${ids.join(',')}`);
    return {
      list: (s.response?.players || []).map((p) => ({
        id: p.steamid, name: p.personaname, avatar: p.avatarfull || p.avatarmedium,
        state: p.personastate, game: p.gameextrainfo || null, gameId: p.gameid || null
      }))
    };
  });
}

// What the account dialog shows: is the key accepted, which profile is it, is it public
async function checkAccount(key, steamId) {
  if (!key) return { error: 'nokey' };
  const s = await probe(`https://api.steampowered.com/ISteamUser/GetPlayerSummaries/v2/?key=${key}&steamids=${steamId}`);
  if (!s.status) return { error: 'network' };
  if (s.status === 401 || s.status === 403) return { error: 'badkey' };
  const p = s.data?.response?.players?.[0];
  if (!p) return { error: 'noprofile' };
  const fl = await probe(`https://api.steampowered.com/ISteamUser/GetFriendList/v1/?key=${key}&steamid=${steamId}&relationship=friend`);
  return {
    name: p.personaname,
    avatar: p.avatarfull || p.avatarmedium,
    public: p.communityvisibilitystate === 3,
    friendsVisible: fl.status === 200
  };
}

function sales(steamId) {
  return cached('sales' + (steamId || ''), 30 * 60e3, async () => {
    const feat = await json('https://store.steampowered.com/api/featuredcategories?cc=ru&l=russian');
    const specials = (feat.specials?.items || []).map((i) => ({
      id: String(i.id), name: i.name, discount: i.discount_percent,
      price: i.final_price, old: i.original_price, currency: i.currency,
      image: i.header_image || i.large_capsule_image, wish: false
    }));
    // discounted games from the user's public wishlist go first
    let wish = [];
    if (steamId) {
      try {
        const w = await json(`https://api.steampowered.com/IWishlistService/GetWishlist/v1/?steamid=${steamId}`);
        const ids = (w.response?.items || []).map((i) => i.appid).slice(0, 80);
        if (ids.length) {
          const pr = await json(`https://store.steampowered.com/api/appdetails?appids=${ids.join(',')}&filters=price_overview&cc=ru`);
          const disc = ids.filter((id) => pr[id]?.data?.price_overview?.discount_percent > 0).slice(0, 8);
          wish = await Promise.all(disc.map(async (id) => {
            const p = pr[id].data.price_overview;
            const d = await json(`https://store.steampowered.com/api/appdetails?appids=${id}&filters=basic&l=russian`).catch(() => ({}));
            return {
              id: String(id), name: d[id]?.data?.name || `App ${id}`, discount: p.discount_percent,
              price: p.final, old: p.initial, currency: p.currency,
              image: d[id]?.data?.header_image || `${CDN}/${id}/header.jpg`, wish: true
            };
          }));
        }
      } catch {}
    }
    const wishIds = new Set(wish.map((w) => w.id));
    return [...wish, ...specials.filter((s) => !wishIds.has(s.id))];
  });
}

function trailer(appid) {
  return cached('trailer' + appid, 24 * 3600e3, async () => {
    const d = await json(`https://store.steampowered.com/api/appdetails?appids=${appid}&filters=movies`).catch(() => ({}));
    const movies = d[appid]?.data?.movies || [];
    const m = movies.find((x) => x.highlight) || movies[0];
    if (!m) return '';
    return m.hls_h264 || m.mp4?.max || m.mp4?.['480'] || m.webm?.max || '';
  });
}

module.exports = { scan, localStats, currentUser, watchRunning, running, news, achievements, friends, checkAccount, sales, trailer };
