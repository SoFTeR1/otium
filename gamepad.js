// Background gamepad watcher: lets the Guide button (or View+Start) bring the launcher up from the couch.
// The renderer only sees the gamepad while the window is focused, so a small XInput poller runs here.
const { spawn } = require('child_process');
const readline = require('readline');

// XInputGetStateEx (ordinal 100) is the variant that also reports the Guide button.
// When the launcher isn't in front, the helper raises it itself: a process that just "pressed" a key
// is allowed to take the foreground, which is the usual way around Windows' focus-stealing lock.
const SOURCE = String.raw`
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
public static class PadWatch {
  delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern int GetWindowTextLength(IntPtr h);
  [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  static readonly string[] NotGames = { "steam", "steamwebhelper", "steamerrorreporter", "gameoverlayui", "explorer", "ps5 launcher", "otium",
    "powershell", "conhost", "discord", "epicgameslauncher", "eadesktop", "ubisoftconnect", "upc", "galaxyclient", "battle.net", "crashpad_handler" };
  // The game is the top-most visible window whose process started after we launched it.
  // It may take a while to appear, so keep looking for up to 25 seconds.
  static void FocusNew(long sinceMs, IntPtr own) {
    var since = new DateTime(1970, 1, 1, 0, 0, 0, DateTimeKind.Utc).AddMilliseconds(sinceMs).AddSeconds(-2);
    for (int attempt = 0; attempt < 50; attempt++) {
      IntPtr found = IntPtr.Zero;
      EnumWindows((w, l) => {
        if (w == own || !IsWindowVisible(w) || GetWindowTextLength(w) == 0 || GetWindow(w, 4) != IntPtr.Zero) return true;
        uint pid; GetWindowThreadProcessId(w, out pid);
        try {
          var p = Process.GetProcessById((int)pid);
          if (Array.IndexOf(NotGames, p.ProcessName.ToLowerInvariant()) >= 0) return true;
          if (p.StartTime.ToUniversalTime() < since) return true;
          found = w;
          return false;
        } catch { return true; }
      }, IntPtr.Zero);
      if (found != IntPtr.Zero) { Activate(found); Say("game " + found.ToInt64()); return; }
      Thread.Sleep(500);
    }
  }
  [StructLayout(LayoutKind.Sequential)]
  public struct State { public uint packet; public ushort buttons; public byte lt; public byte rt; public short lx; public short ly; public short rx; public short ry; }
  [DllImport("xinput1_4.dll", EntryPoint = "#100")] static extern int GetStateEx(int i, out State s);
  [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  const ushort GUIDE = 0x0400, BACK = 0x0020, START = 0x0010;
  // stdin: "focus <hwnd>" activates that window (used to hand control back to the game)
  public static void Run(long hwnd) {
    Thread t = new Thread(() => Poll(new IntPtr(hwnd)));
    t.IsBackground = true;
    t.Start();
    string line;
    while ((line = Console.In.ReadLine()) != null) {
      long v;
      if (line.StartsWith("focus ") && long.TryParse(line.Substring(6), out v) && v != 0) Activate(new IntPtr(v));
      else if (line.StartsWith("focusnew ") && long.TryParse(line.Substring(9), out v)) {
        long since = v;
        new Thread(() => FocusNew(since, new IntPtr(hwnd))) { IsBackground = true }.Start();
      }
    }
  }
  static void Activate(IntPtr w) {
    if (IsIconic(w)) ShowWindow(w, 9);
    keybd_event(0x12, 0, 0, UIntPtr.Zero);
    keybd_event(0x12, 0, 2, UIntPtr.Zero);
    SetForegroundWindow(w);
  }
  static void Poll(IntPtr h) {
    ushort[] prev = new ushort[4];
    while (true) {
      for (int i = 0; i < 4; i++) {
        State s;
        if (GetStateEx(i, out s) != 0) { prev[i] = 0; continue; }
        ushort b = s.buttons, p = prev[i];
        bool guide = (b & GUIDE) != 0 && (p & GUIDE) == 0;
        bool combo = (b & (BACK | START)) == (BACK | START) && (p & (BACK | START)) != (BACK | START);
        if (guide || combo) {
          IntPtr fg = GetForegroundWindow();
          if (fg == h && !IsIconic(h)) Say("toggle");
          else {
            // report what was in front (normally the game) so we can return to it later
            Activate(h);
            Say("front " + fg.ToInt64());
          }
        }
        prev[i] = b;
      }
      Thread.Sleep(40);
    }
  }
  static void Say(string s) { Console.Out.WriteLine(s); Console.Out.Flush(); }
}
`;

function watch(hwnd, onEvent) {
  const script = `
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
${SOURCE}
'@
[PadWatch]::Run(${hwnd})
`;
  let stopped = false;
  let proc;
  const start = () => {
    proc = spawn('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', Buffer.from(script, 'utf16le').toString('base64')], { windowsHide: true });
    readline.createInterface({ input: proc.stdout }).on('line', (l) => onEvent(l.trim()));
    proc.on('exit', () => { if (!stopped) setTimeout(start, 5000); });
  };
  start();
  return {
    stop: () => { stopped = true; proc?.kill(); },
    focus: (hwnd) => proc?.stdin.writable && proc.stdin.write(`focus ${hwnd}\n`),
    // bring the window of a game started at `sinceMs` above the launcher
    focusNew: (sinceMs) => proc?.stdin.writable && proc.stdin.write(`focusnew ${Math.round(sinceMs)}\n`)
  };
}

module.exports = { watch };
