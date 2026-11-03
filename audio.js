// Per-application volume (the Windows volume mixer) via Core Audio, through a long-lived PowerShell helper.
// Commands (one per line): list | master <0-100> | mastermute <0|1> | set <pid> <0-100> | mute <pid> <0|1>
// Every command answers with one JSON line: { master, masterMuted, apps: [{ pid, name, path, volume, muted }] }
const { spawn } = require('child_process');
const readline = require('readline');

const SOURCE = String.raw`
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

[ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")] class MMDeviceEnumeratorCom {}

[Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDeviceEnumerator {
  int EnumAudioEndpoints();
  [PreserveSig] int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice device);
}
[Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDevice {
  [PreserveSig] int Activate(ref Guid iid, int clsCtx, IntPtr activationParams, [MarshalAs(UnmanagedType.IUnknown)] out object iface);
}
[Guid("77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioSessionManager2 {
  int GetAudioSessionControl();
  int GetSimpleAudioVolume();
  [PreserveSig] int GetSessionEnumerator(out IAudioSessionEnumerator sessions);
}
[Guid("E2F5BB11-0570-40CA-ACDD-3AA01277DEE8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioSessionEnumerator {
  [PreserveSig] int GetCount(out int count);
  [PreserveSig] int GetSession(int index, out IAudioSessionControl2 session);
}
[Guid("bfb7ff88-7239-4fc9-8fa2-07c950be9c6d"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioSessionControl2 {
  [PreserveSig] int GetState(out int state);
  int GetDisplayName(); int SetDisplayName(); int GetIconPath(); int SetIconPath();
  int GetGroupingParam(); int SetGroupingParam(); int RegisterNotification(); int UnregisterNotification();
  int GetSessionIdentifier(); int GetSessionInstanceIdentifier();
  [PreserveSig] int GetProcessId(out int pid);
  [PreserveSig] int IsSystemSoundsSession();
}
[Guid("87CE5498-68D6-44E5-9215-6DA47EF883D8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface ISimpleAudioVolume {
  [PreserveSig] int SetMasterVolume(float level, ref Guid ctx);
  [PreserveSig] int GetMasterVolume(out float level);
  [PreserveSig] int SetMute(bool mute, ref Guid ctx);
  [PreserveSig] int GetMute(out bool mute);
}
[Guid("5CDF2C82-841E-4546-9722-0CF74078229A"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioEndpointVolume {
  int RegisterControlChangeNotify(); int UnregisterControlChangeNotify(); int GetChannelCount(); int SetMasterVolumeLevel();
  [PreserveSig] int SetMasterVolumeLevelScalar(float level, ref Guid ctx);
  int GetMasterVolumeLevel();
  [PreserveSig] int GetMasterVolumeLevelScalar(out float level);
  int SetChannelVolumeLevel(); int SetChannelVolumeLevelScalar(); int GetChannelVolumeLevel(); int GetChannelVolumeLevelScalar();
  [PreserveSig] int SetMute(bool mute, ref Guid ctx);
  [PreserveSig] int GetMute(out bool mute);
}

public static class Mixer {
  static Guid ctx = Guid.Empty;
  static IMMDevice Device() {
    var en = (IMMDeviceEnumerator)new MMDeviceEnumeratorCom();
    IMMDevice dev; en.GetDefaultAudioEndpoint(0, 1, out dev); // render, multimedia
    return dev;
  }
  static IAudioEndpointVolume Endpoint() {
    object o; var iid = typeof(IAudioEndpointVolume).GUID;
    Device().Activate(ref iid, 23, IntPtr.Zero, out o);
    return (IAudioEndpointVolume)o;
  }
  // sessions grouped by process (Chrome etc. open several)
  static Dictionary<int, List<ISimpleAudioVolume>> Sessions() {
    var map = new Dictionary<int, List<ISimpleAudioVolume>>();
    object o; var iid = typeof(IAudioSessionManager2).GUID;
    Device().Activate(ref iid, 23, IntPtr.Zero, out o);
    IAudioSessionEnumerator en; ((IAudioSessionManager2)o).GetSessionEnumerator(out en);
    int n; en.GetCount(out n);
    for (int i = 0; i < n; i++) {
      IAudioSessionControl2 s; en.GetSession(i, out s);
      int state; s.GetState(out state);
      int pid; s.GetProcessId(out pid);
      if (pid == 0 || state == 2 || s.IsSystemSoundsSession() == 0) continue; // 2 = expired
      List<ISimpleAudioVolume> l;
      if (!map.TryGetValue(pid, out l)) map[pid] = l = new List<ISimpleAudioVolume>();
      l.Add((ISimpleAudioVolume)s);
    }
    return map;
  }
  static string Esc(string s) {
    var b = new StringBuilder();
    foreach (var c in s ?? "") {
      if (c == '"' || c == '\\') b.Append('\\').Append(c);
      else if (c < 32) b.Append(' ');
      else b.Append(c);
    }
    return b.ToString();
  }
  public static string Run(string line) {
    var p = line.Trim().Split(' ');
    var ep = Endpoint();
    var all = Sessions();
    if (p[0] == "master") ep.SetMasterVolumeLevelScalar(int.Parse(p[1]) / 100f, ref ctx);
    else if (p[0] == "mastermute") ep.SetMute(p[1] == "1", ref ctx);
    else if ((p[0] == "set" || p[0] == "mute") && all.ContainsKey(int.Parse(p[1]))) {
      foreach (var v in all[int.Parse(p[1])]) {
        if (p[0] == "set") v.SetMasterVolume(int.Parse(p[2]) / 100f, ref ctx);
        else v.SetMute(p[2] == "1", ref ctx);
      }
    }
    float mv; ep.GetMasterVolumeLevelScalar(out mv);
    bool mm; ep.GetMute(out mm);
    var sb = new StringBuilder();
    sb.Append("{\"master\":").Append((int)Math.Round(mv * 100)).Append(",\"masterMuted\":").Append(mm ? "true" : "false").Append(",\"apps\":[");
    bool first = true;
    foreach (var kv in all) {
      string name = null, path = "";
      try {
        var proc = Process.GetProcessById(kv.Key);
        name = proc.ProcessName;
        try { path = proc.MainModule.FileName; var d = proc.MainModule.FileVersionInfo.FileDescription; if (!string.IsNullOrWhiteSpace(d)) name = d; } catch { }
      } catch { continue; }
      float vol; kv.Value[0].GetMasterVolume(out vol);
      bool mute; kv.Value[0].GetMute(out mute);
      if (!first) sb.Append(',');
      first = false;
      sb.Append("{\"pid\":").Append(kv.Key).Append(",\"name\":\"").Append(Esc(name)).Append("\",\"path\":\"").Append(Esc(path))
        .Append("\",\"volume\":").Append((int)Math.Round(vol * 100)).Append(",\"muted\":").Append(mute ? "true" : "false").Append('}');
    }
    return sb.Append("]}").ToString();
  }
}
`;

let proc = null;
const waiting = [];
function helper() {
  if (proc) return proc;
  const script = `
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Add-Type -TypeDefinition @'
${SOURCE}
'@
while ($true) {
  $line = [Console]::In.ReadLine()
  if ($line -eq $null) { break }
  try { $out = [Mixer]::Run($line) } catch { $out = '{"error":true}' }
  [Console]::Out.WriteLine($out)
  [Console]::Out.Flush()
}
`;
  proc = spawn('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', Buffer.from(script, 'utf16le').toString('base64')], { windowsHide: true });
  readline.createInterface({ input: proc.stdout }).on('line', (l) => {
    const cb = waiting.shift();
    if (!cb) return;
    try { cb(JSON.parse(l)); } catch { cb(null); }
  });
  proc.on('exit', () => { proc = null; while (waiting.length) waiting.shift()(null); });
  return proc;
}

function command(cmd) {
  if (!/^(list|master \d+|mastermute [01]|set \d+ \d+|mute \d+ [01])$/.test(cmd)) return Promise.resolve(null);
  return new Promise((resolve) => {
    const p = helper();
    // first call compiles the C# (a few seconds); a stuck helper is restarted
    const timer = setTimeout(() => p.kill(), 15000);
    waiting.push((v) => { clearTimeout(timer); resolve(v); });
    p.stdin.write(cmd + '\n');
  });
}

module.exports = { command, warmUp: () => helper(), shutdown: () => proc?.kill() };
