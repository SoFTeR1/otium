// Background gamepad watcher: lets the Guide button (or View+Start) bring the launcher up from the couch.
// The renderer only sees the gamepad while the window is focused, so a small XInput poller runs here.
const { spawn } = require('child_process');
const readline = require('readline');

// XInputGetStateEx (ordinal 100) is the variant that also reports the Guide button.
// When the launcher isn't in front, the helper raises it itself: a process that just "pressed" a key
// is allowed to take the foreground, which is the usual way around Windows' focus-stealing lock.
const SOURCE = String.raw`
using System;
using System.Runtime.InteropServices;
using System.Threading;
public static class PadWatch {
  [StructLayout(LayoutKind.Sequential)]
  public struct State { public uint packet; public ushort buttons; public byte lt; public byte rt; public short lx; public short ly; public short rx; public short ry; }
  [DllImport("xinput1_4.dll", EntryPoint = "#100")] static extern int GetStateEx(int i, out State s);
  [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  const ushort GUIDE = 0x0400, BACK = 0x0020, START = 0x0010;
  public static void Run(long hwnd) {
    IntPtr h = new IntPtr(hwnd);
    ushort[] prev = new ushort[4];
    while (true) {
      for (int i = 0; i < 4; i++) {
        State s;
        if (GetStateEx(i, out s) != 0) { prev[i] = 0; continue; }
        ushort b = s.buttons, p = prev[i];
        bool guide = (b & GUIDE) != 0 && (p & GUIDE) == 0;
        bool combo = (b & (BACK | START)) == (BACK | START) && (p & (BACK | START)) != (BACK | START);
        if (guide || combo) {
          if (GetForegroundWindow() == h && !IsIconic(h)) Say("toggle");
          else {
            if (IsIconic(h)) ShowWindow(h, 9);
            keybd_event(0x12, 0, 0, UIntPtr.Zero);
            keybd_event(0x12, 0, 2, UIntPtr.Zero);
            SetForegroundWindow(h);
            Say("front");
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
  return () => { stopped = true; proc?.kill(); };
}

module.exports = { watch };
