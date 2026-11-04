// Compiles the branded web installer (dist/OTIUM-Installer.exe) with the .NET Framework C# compiler.
// Run after electron-builder so the unpacked size (used for the progress bar) is known.
const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const root = path.join(__dirname, '..', '..');
const version = require(path.join(root, 'package.json')).version;
const fw = path.join(process.env.WINDIR || 'C:\\Windows', 'Microsoft.NET', 'Framework64', 'v4.0.30319');
const wpf = path.join(fw, 'WPF');

function dirSize(dir) {
  let total = 0;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    total += e.isDirectory() ? dirSize(p) : fs.statSync(p).size;
  }
  return total;
}
const unpacked = path.join(root, 'dist', 'win-unpacked');
const bytes = fs.existsSync(unpacked) ? dirSize(unpacked) : 0;

const tmp = path.join(require('os').tmpdir(), 'otium-installer');
fs.mkdirSync(tmp, { recursive: true });
const src = path.join(tmp, 'Installer.cs');
fs.writeFileSync(src, fs.readFileSync(path.join(__dirname, 'Installer.cs'), 'utf8').replace('__BYTES__', String(bytes)).replace('__VERSION__', version));

const out = path.join(root, 'dist', 'OTIUM-Installer.exe');
execFileSync(path.join(fw, 'csc.exe'), [
  '/nologo', '/target:winexe', '/optimize+', '/codepage:65001',
  `/out:${out}`,
  `/win32icon:${path.join(root, 'build', 'icon.ico')}`,
  `/resource:${path.join(root, 'build', 'icon.png')},icon.png`,
  `/resource:${path.join(__dirname, 'ui.xaml')},ui.xaml`,
  `/r:${path.join(wpf, 'PresentationFramework.dll')}`,
  `/r:${path.join(wpf, 'PresentationCore.dll')}`,
  `/r:${path.join(wpf, 'WindowsBase.dll')}`,
  `/r:${path.join(fw, 'System.Xaml.dll')}`,
  '/r:System.Windows.Forms.dll', '/r:System.Core.dll', '/r:System.dll', '/r:Microsoft.CSharp.dll',
  src
], { stdio: 'inherit' });
console.log(`installer: ${out} (${(fs.statSync(out).size / 1024).toFixed(0)} KB, unpacked ${(bytes / 1048576).toFixed(0)} MB)`);
