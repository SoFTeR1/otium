// PC status (CPU / RAM / GPU / disks) and the Windows "now playing" media session
const os = require('os');
const fs = require('fs');
const { execFile, spawn } = require('child_process');
const readline = require('readline');

// ---------- PC status ----------
let lastCpu = null;
function cpuLoad() {
  const t = os.cpus().reduce((a, c) => {
    for (const [k, v] of Object.entries(c.times)) a[k] = (a[k] || 0) + v;
    return a;
  }, {});
  const total = Object.values(t).reduce((a, b) => a + b, 0);
  const prev = lastCpu;
  lastCpu = { idle: t.idle, total };
  if (!prev) return null;
  const dt = total - prev.total;
  return dt > 0 ? Math.round(100 * (1 - (t.idle - prev.idle) / dt)) : null;
}

let hasNvidia = true;
function gpu() {
  if (!hasNvidia) return Promise.resolve(null);
  return new Promise((resolve) => {
    execFile('nvidia-smi', ['--query-gpu=name,utilization.gpu,temperature.gpu,memory.used,memory.total', '--format=csv,noheader,nounits'],
      { windowsHide: true, timeout: 4000 }, (err, out) => {
        if (err) { hasNvidia = false; return resolve(null); }
        const [name, load, temp, used, total] = out.split('\n')[0].split(',').map((s) => s.trim());
        resolve({ name, load: +load, temp: +temp, memUsed: +used, memTotal: +total });
      });
  });
}

function disks() {
  const out = [];
  for (const L of 'CDEFGHIJKLMNOPQRSTUVWXYZ') {
    try {
      const s = fs.statfsSync(`${L}:\\`);
      const total = s.blocks * s.bsize;
      if (total > 0) out.push({ drive: L + ':', total, free: s.bavail * s.bsize });
    } catch {}
  }
  return out;
}

async function stats() {
  return {
    cpu: { load: cpuLoad(), name: os.cpus()[0]?.model.trim(), cores: os.cpus().length },
    ram: { total: os.totalmem(), used: os.totalmem() - os.freemem() },
    gpu: await gpu(),
    disks: disks(),
    uptime: os.uptime()
  };
}

// ---------- Media session (Spotify, Яндекс Музыка, browsers…) ----------
// A long-lived PowerShell process talks to GlobalSystemMediaTransportControls.
// We write one command per line (poll | toggle | next | prev) and read back one JSON line.
const MEDIA_PS = String.raw`
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation${'`'}1' } | Select-Object -First 1
function Await($op, [Type]$t) { $task = $asTask.MakeGenericMethod($t).Invoke($null, @($op)); $task.Wait(-1) | Out-Null; $task.Result }
$null = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$null = [Windows.Storage.Streams.IRandomAccessStreamWithContentType, Windows.Storage.Streams, ContentType = WindowsRuntime]
$mgr = Await ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]::RequestAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager])
$lastKey = ''
while ($true) {
  $cmd = [Console]::In.ReadLine()
  if ($cmd -eq $null) { break }
  $s = $mgr.GetCurrentSession()
  if (-not $s) { [Console]::Out.WriteLine('{}'); [Console]::Out.Flush(); continue }
  switch ($cmd) {
    'toggle' { $null = Await ($s.TryTogglePlayPauseAsync()) ([bool]) }
    'next'   { $null = Await ($s.TrySkipNextAsync()) ([bool]) }
    'prev'   { $null = Await ($s.TrySkipPreviousAsync()) ([bool]) }
  }
  if ($cmd -ne 'poll') { Start-Sleep -Milliseconds 350 }
  $p = Await ($s.TryGetMediaPropertiesAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties])
  $key = "$($p.Title)|$($p.Artist)"
  $art = $null
  if ($key -ne $lastKey -and $p.Thumbnail) {
    $st = Await ($p.Thumbnail.OpenReadAsync()) ([Windows.Storage.Streams.IRandomAccessStreamWithContentType])
    $ns = [System.IO.WindowsRuntimeStreamExtensions]::AsStreamForRead($st)
    $ms = New-Object System.IO.MemoryStream
    $ns.CopyTo($ms)
    $art = [Convert]::ToBase64String($ms.ToArray())
  }
  $newTrack = $key -ne $lastKey
  $lastKey = $key
  $o = @{ title = $p.Title; artist = $p.Artist; album = $p.AlbumTitle; app = $s.SourceAppUserModelId; playing = ([int]$s.GetPlaybackInfo().PlaybackStatus -eq 4); key = $key; newTrack = $newTrack; art = $art }
  [Console]::Out.WriteLine(($o | ConvertTo-Json -Compress))
  [Console]::Out.Flush()
}
`;

let media = null;
const waiting = [];
function mediaProc() {
  if (media) return media;
  const encoded = Buffer.from(MEDIA_PS, 'utf16le').toString('base64');
  media = spawn('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', encoded], { windowsHide: true });
  readline.createInterface({ input: media.stdout }).on('line', (line) => {
    const cb = waiting.shift();
    if (!cb) return;
    try { cb(JSON.parse(line)); } catch { cb(null); }
  });
  media.on('exit', () => {
    media = null;
    while (waiting.length) waiting.shift()(null);
  });
  return media;
}

function mediaCommand(cmd) {
  if (!['poll', 'toggle', 'next', 'prev'].includes(cmd)) return Promise.resolve(null);
  return new Promise((resolve) => {
    const p = mediaProc();
    // a stuck helper would desync request/response order, so restart it instead
    const timer = setTimeout(() => p.kill(), 8000);
    function done(v) { clearTimeout(timer); resolve(v); }
    waiting.push(done);
    p.stdin.write(cmd + '\n');
  });
}

function shutdown() {
  if (media) media.kill();
}

module.exports = { stats, mediaCommand, shutdown };
