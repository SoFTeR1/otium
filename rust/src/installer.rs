//! OTIUM is its own installer: the release asset `OTIUM-Setup-X.exe` is the same exe.
//!
//! * started as `OTIUM-Setup-*.exe`, or by the Electron build's updater (`--updated /S
//!   --force-run`): installs itself as `OTIUM.exe` where OTIUM already lives (the Electron
//!   install, found through its uninstall entry) or in `%LOCALAPPDATA%\Programs\OTIUM`, clears the
//!   Electron files out of that folder, keeps shortcuts and autostart pointing at it, then starts it;
//! * `--self-update <exe> <pid>`: an update OTIUM downloaded — waits for that OTIUM to exit,
//!   replaces its exe and starts it again;
//! * `--uninstall`: what "Удалить" in Windows' app list runs.
//!
//! Settings and the library stay in %APPDATA%\OTIUM throughout.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
use winreg::RegKey;

const UNINSTALL: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
const OUR_KEY: &str = "OTIUM";
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

/// Runs the installer modes; true when this process was one of them and should exit.
pub fn handle_args() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |a: &str| args.iter().any(|x| x.eq_ignore_ascii_case(a));
    if let Some(i) = args.iter().position(|a| a == "--self-update") {
        let target = args.get(i + 1).map(PathBuf::from);
        let pid = args.get(i + 2).and_then(|p| p.parse::<u32>().ok());
        if let Some(target) = target {
            self_update(&target, pid);
        }
        return true;
    }
    if has("--uninstall") {
        uninstall();
        return true;
    }
    let me = std::env::current_exe().ok();
    let named_setup = me.as_ref().and_then(|m| m.file_name()).is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("otium-setup"));
    if has("--updated") || named_setup {
        let silent = has("/S");
        // the Electron updater asks for --force-run; a double-clicked setup starts OTIUM too
        let run = has("--force-run") || !silent;
        if let Err(e) = install(run) {
            message(&format!("Не удалось установить OTIUM: {e}"));
        }
        return true;
    }
    // a finished update leaves the previous exe behind for a moment
    if let Some(m) = &me {
        let _ = std::fs::remove_file(m.with_extension("old.exe"));
    }
    false
}

/// The Electron build's own key (electron-builder derives it from the app id "local.ps5launcher").
const ELECTRON_KEY: &str = "Software\\ab98e456-26bd-5f85-9fce-95c7943e0e2f";

/// Where OTIUM is (or should be) installed, like the old C# installer looked for it: our own
/// registration, a running OTIUM (the Electron build is still closing when its updater starts us),
/// the Start menu / desktop shortcut, the Electron build's registry key, else
/// %LOCALAPPDATA%\Programs\OTIUM. The registry alone isn't trusted: people move the folder.
pub fn install_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("OTIUM_INSTALL_DIR") {
        return PathBuf::from(d);
    }
    // NSIS style "/D=C:\Some Folder" (the old web installer passes it): always last, spaces and all
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a.len() > 3 && a[..3].eq_ignore_ascii_case("/D=")) {
        let d = args[i..].join(" ");
        let d = d[3..].trim().trim_matches('"');
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    let found =registered_dir().or_else(running_dir).or_else(shortcut_dir).or_else(electron_dir);
    if let Some(d) = found {
        return d;
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    local.join("Programs").join("OTIUM")
}

/// Our own entry in Windows' app list.
fn registered_dir() -> Option<PathBuf> {
    let k = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(format!("{UNINSTALL}\\{OUR_KEY}"), KEY_READ).ok()?;
    let loc: String = k.get_value("InstallLocation").ok()?;
    let dir = PathBuf::from(loc.trim().trim_matches('"'));
    dir.join("OTIUM.exe").exists().then_some(dir)
}

fn is_otium_exe(p: &Path) -> bool {
    p.file_name().is_some_and(|n| {
        let n = n.to_string_lossy().to_lowercase();
        n == "otium.exe" || n == "ps5 launcher.exe"
    })
}

/// A running OTIUM that isn't this setup or a downloaded copy in a temp folder.
fn running_dir() -> Option<PathBuf> {
    let me = std::process::id();
    let temp = std::env::temp_dir().to_string_lossy().to_lowercase();
    crate::win::processes()
        .into_iter()
        .filter(|p| p.pid != me)
        .filter_map(|p| crate::win::exe_path(p.pid))
        .filter(|e| is_otium_exe(e))
        .filter(|e| {
            let l = e.to_string_lossy().to_lowercase();
            !l.starts_with(&temp) && !l.contains("-updater\\")
        })
        .find_map(|e| e.parent().map(Path::to_path_buf))
}

fn shortcut_dir() -> Option<PathBuf> {
    let programs = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Microsoft\\Windows\\Start Menu\\Programs"));
    let mut links: Vec<PathBuf> = programs.map(|p| vec![p.join("OTIUM.lnk"), p.join("PS5 Launcher.lnk")]).unwrap_or_default();
    links.extend(desktop());
    links.into_iter().filter(|l| l.exists()).filter_map(|l| crate::win::read_shortcut(&l)).map(|s| s.target).find(|t| is_otium_exe(t) && t.exists()).and_then(|t| t.parent().map(Path::to_path_buf))
}

fn electron_dir() -> Option<PathBuf> {
    let k = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(ELECTRON_KEY, KEY_READ).ok()?;
    let loc: String = k.get_value("InstallLocation").ok()?;
    let dir = PathBuf::from(loc.trim().trim_matches('"'));
    dir.is_dir().then_some(dir)
}

fn install(run: bool) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join("OTIUM.exe");
    let electron = dir.join("resources").join("app.asar").exists();
    let fresh = !target.exists() && !dir.join("PS5 Launcher.exe").exists();

    // whatever runs from there has to go first (the Electron updater has closed it already)
    stop_running(&dir, Duration::from_secs(15));
    copy_over(&me, &target)?;
    if electron {
        remove_electron_files(&dir);
    }

    // a test install (OTIUM_INSTALL_DIR) leaves Windows' app list, shortcuts and autostart alone
    if std::env::var_os("OTIUM_INSTALL_DIR").is_none() && std::env::var_os("OTIUM_INSTALL_NO_RUN").is_none() {
        register(&dir, &target, electron);
        shortcuts(&target, fresh);
        fix_autostart(&target);
    }
    if run && std::env::var_os("OTIUM_INSTALL_NO_RUN").is_none() {
        std::process::Command::new(&target).current_dir(&dir).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn self_update(target: &Path, pid: Option<u32>) {
    if let Some(pid) = pid {
        let end = Instant::now() + Duration::from_secs(30);
        while crate::win::alive(pid) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let Ok(me) = std::env::current_exe() else { return };
    // a running exe can't be overwritten, but it can be renamed: if anything still holds it
    let old = target.with_extension("old.exe");
    if copy_over(&me, target).is_err() {
        let _ = std::fs::remove_file(&old);
        if std::fs::rename(target, &old).is_err() || copy_over(&me, target).is_err() {
            message("Не удалось обновить OTIUM: файл занят. Закройте OTIUM и запустите обновление ещё раз.");
            return;
        }
    }
    // OTIUM_INSTALL_NO_RUN: a test — no registry, no start
    if std::env::var_os("OTIUM_INSTALL_NO_RUN").is_some() {
        return;
    }
    crate::win::refresh_icons();
    if let Some(dir) = target.parent() {
        if let Ok(un) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(format!("{UNINSTALL}\\{OUR_KEY}"), KEY_SET_VALUE) {
            let _ = un.set_value("DisplayVersion", &crate::update::current());
        }
        let _ = std::process::Command::new(target).current_dir(dir).spawn();
    }
}

fn copy_over(from: &Path, to: &Path) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..20 {
        match std::fs::copy(from, to) {
            Ok(_) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(last)
}

fn stop_running(dir: &Path, wait: Duration) {
    let me = std::process::id();
    let end = Instant::now() + wait;
    loop {
        let pids: Vec<u32> = crate::win::processes_under(dir).into_iter().filter(|p| *p != me).collect();
        if pids.is_empty() {
            return;
        }
        if Instant::now() >= end {
            crate::win::close_processes(&pids);
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// What electron-builder put next to the Electron exe; our OTIUM.exe and the old uninstaller stay.
fn remove_electron_files(dir: &Path) {
    for d in ["resources", "locales", "swiftshader"] {
        let _ = std::fs::remove_dir_all(dir.join(d));
    }
    for f in [
        "chrome_100_percent.pak",
        "chrome_200_percent.pak",
        "resources.pak",
        "icudtl.dat",
        "snapshot_blob.bin",
        "v8_context_snapshot.bin",
        "ffmpeg.dll",
        "libEGL.dll",
        "libGLESv2.dll",
        "vk_swiftshader.dll",
        "vulkan-1.dll",
        "d3dcompiler_47.dll",
        "vk_swiftshader_icd.json",
        "LICENSE.electron.txt",
        "LICENSES.chromium.html",
    ] {
        let _ = std::fs::remove_file(dir.join(f));
    }
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_lowercase();
        if n.starts_with("uninstall ") && n.ends_with(".exe") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Windows' app list: our own "OTIUM" entry. After the Electron build, its entries (named "OTIUM" or
/// "PS5 Launcher …", the uninstaller of which is gone now) are removed so there is one.
fn register(dir: &Path, exe: &Path, after_electron: bool) {
    let hk = RegKey::predef(HKEY_CURRENT_USER);
    if after_electron {
        if let Ok(un) = hk.open_subkey_with_flags(UNINSTALL, KEY_READ | KEY_SET_VALUE) {
            let stale: Vec<String> = un
                .enum_keys()
                .flatten()
                .filter(|name| name != OUR_KEY)
                .filter(|name| {
                    un.open_subkey_with_flags(name, KEY_READ).ok().and_then(|k| k.get_value::<String, _>("DisplayName").ok()).is_some_and(|d| d.starts_with("OTIUM") || d.starts_with("PS5 Launcher"))
                })
                .collect();
            for name in stale {
                let _ = un.delete_subkey_all(&name);
            }
        }
    }
    let Ok((k, _)) = hk.create_subkey(format!("{UNINSTALL}\\{OUR_KEY}")) else { return };
    let _ = k.set_value("DisplayName", &"OTIUM");
    let _ = k.set_value("DisplayVersion", &crate::update::current());
    let _ = k.set_value("Publisher", &"SoFTeR1");
    let _ = k.set_value("DisplayIcon", &format!("\"{}\",0", exe.display()));
    let _ = k.set_value("InstallLocation", &dir.display().to_string());
    let _ = k.set_value("UninstallString", &format!("\"{}\" --uninstall", exe.display()));
    let _ = k.set_value("NoModify", &1u32);
    let _ = k.set_value("NoRepair", &1u32);
}

fn start_menu() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Microsoft\\Windows\\Start Menu\\Programs\\OTIUM.lnk"))
}

fn desktop() -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(|s| PathBuf::from(s).join("OTIUM.lnk"))
    }
}

/// Start menu and desktop shortcuts: made on a fresh install, re-pointed on an update (a desktop
/// shortcut the user deleted is not brought back).
fn shortcuts(exe: &Path, fresh: bool) {
    for (path, always) in [(start_menu(), true), (desktop(), fresh)] {
        let Some(path) = path else { continue };
        if always || path.exists() {
            crate::win::write_shortcut(&path, exe);
        }
    }
    crate::win::refresh_icons();
}

/// The Electron build registered autostart under its own name; keep one entry, ours, pointing at
/// the installed exe.
fn fix_autostart(exe: &Path) {
    let Ok(run) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_SET_VALUE) else { return };
    let want = exe.to_string_lossy().to_lowercase();
    let mut had = false;
    for (name, _) in run.enum_values().flatten() {
        let data: String = run.get_value(&name).unwrap_or_default();
        if data.to_lowercase().contains(&want) {
            had = true;
            if name != "OTIUM" {
                let _ = run.delete_value(&name);
            }
        }
    }
    if had {
        let _ = run.set_value("OTIUM", &format!("\"{}\" --autostart", exe.display()));
    }
}

fn uninstall() {
    let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    for p in [start_menu(), desktop()].into_iter().flatten() {
        let _ = std::fs::remove_file(p);
    }
    let hk = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(run) = hk.open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE) {
        let _ = run.delete_value("OTIUM");
    }
    let _ = hk.delete_subkey_all(format!("{UNINSTALL}\\{OUR_KEY}"));
    // the running exe can't delete its own folder: a shell does it a moment after we exit
    if let Some(dir) = dir {
        use std::os::windows::process::CommandExt;
        let cmd = format!("ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display());
        let _ = std::process::Command::new("cmd").args(["/c", &cmd]).creation_flags(0x0800_0000).spawn();
    }
    message("OTIUM удалён. Ваша библиотека и настройки остались в %APPDATA%\\OTIUM.");
}

fn message(text: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
    let t: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let c: Vec<u16> = "OTIUM".encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(None, PCWSTR(t.as_ptr()), PCWSTR(c.as_ptr()), MB_OK | MB_ICONINFORMATION);
    }
}

#[cfg(test)]
mod tests {
    /// Read-only: where an install or an update from the Electron build would go on this PC.
    /// `cargo test -- --ignored install_location --nocapture`
    #[test]
    #[ignore]
    fn install_location() {
        let dir = super::install_dir();
        println!("install dir: {}", dir.display());
        println!("ours {:?} | running {:?} | shortcut {:?} | electron key {:?}", super::registered_dir(), super::running_dir(), super::shortcut_dir(), super::electron_dir());
        println!("electron there: {}", dir.join("resources").join("app.asar").exists());
    }
}
