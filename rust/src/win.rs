//! Thin wrappers over the Win32 calls the shell needs: finding and raising windows, listing and
//! closing processes, reading .lnk shortcuts. Window handles travel as `isize` so they can cross threads.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, FILETIME, HANDLE, HWND, LPARAM, MAX_PATH};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindow, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SetForegroundWindow, ShowWindow, GW_OWNER, SW_RESTORE,
};

pub fn hwnd(h: isize) -> HWND {
    HWND(h as *mut _)
}

pub fn foreground() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

pub fn is_minimized(h: isize) -> bool {
    unsafe { IsIconic(hwnd(h)).as_bool() }
}

/// Brings a window to the front. Windows only lets the process that last received input take the
/// foreground; tapping Alt makes us that process (the usual way around the focus-stealing lock).
pub fn activate(h: isize) {
    if h == 0 {
        return;
    }
    unsafe {
        let w = hwnd(h);
        if IsIconic(w).as_bool() {
            let _ = ShowWindow(w, SW_RESTORE);
        }
        keybd_event(0x12, 0, Default::default(), 0);
        keybd_event(0x12, 0, KEYEVENTF_KEYUP, 0);
        let _ = SetForegroundWindow(w);
        // focus alone doesn't always change the stacking order of a fullscreen window: raise it too
        let _ = windows::Win32::UI::WindowsAndMessaging::BringWindowToTop(w);
        set_top(w);
    }
}

fn set_top(w: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE};
    unsafe {
        let _ = SetWindowPos(w, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
    }
}

pub fn window_pid(h: isize) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd(h), Some(&mut pid)) };
    pid
}

pub struct Proc {
    pub pid: u32,
}

pub fn processes() -> Vec<Proc> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                out.push(Proc { pid: e.th32ProcessID });
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn open(pid: u32) -> Option<HANDLE> {
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok() }
}

pub fn exe_path(pid: u32) -> Option<PathBuf> {
    let h = open(pid)?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) };
    unsafe {
        let _ = CloseHandle(h);
    }
    ok.ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

pub fn start_time(pid: u32) -> Option<SystemTime> {
    let h = open(pid)?;
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let ok = unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) };
    unsafe {
        let _ = CloseHandle(h);
    }
    ok.ok()?;
    // FILETIME: 100 ns ticks since 1601
    let ticks = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64;
    let unix_ticks = ticks.checked_sub(116_444_736_000_000_000)?;
    Some(UNIX_EPOCH + Duration::from_nanos(unix_ticks * 100))
}

pub fn alive(pid: u32) -> bool {
    match open(pid) {
        Some(h) => {
            let mut code = 0u32;
            let ok = unsafe { windows::Win32::System::Threading::GetExitCodeProcess(h, &mut code) };
            unsafe {
                let _ = CloseHandle(h);
            }
            ok.is_ok() && code == 259 // STILL_ACTIVE
        }
        None => false,
    }
}

/// Processes whose executable lives inside `dir`.
pub fn processes_under(dir: &Path) -> Vec<u32> {
    let prefix = dir.to_string_lossy().to_lowercase().trim_end_matches('\\').to_string() + "\\";
    processes()
        .into_iter()
        .filter(|p| p.pid > 4)
        .filter(|p| exe_path(p.pid).is_some_and(|e| e.to_string_lossy().to_lowercase().starts_with(&prefix)))
        .map(|p| p.pid)
        .collect()
}

const NOT_GAMES: &[&str] = &[
    "steam", "steamwebhelper", "steamerrorreporter", "gameoverlayui", "explorer", "ps5 launcher", "otium",
    "powershell", "conhost", "discord", "epicgameslauncher", "eadesktop", "ubisoftconnect", "upc", "galaxyclient",
    "battle.net", "crashpad_handler", "windowsterminal", "openconsole",
];

/// The game's window: the top-most visible, titled, unowned window whose process started after
/// `since` and isn't a launcher, shell or the OTIUM window itself.
pub fn find_new_window(own: isize, since: SystemTime) -> Option<isize> {
    struct Ctx {
        own: isize,
        since: SystemTime,
        found: isize,
    }
    unsafe extern "system" fn cb(w: HWND, l: LPARAM) -> BOOL {
        let ctx = &mut *(l.0 as *mut Ctx);
        let h = w.0 as isize;
        if h == ctx.own || !IsWindowVisible(w).as_bool() || GetWindowTextLengthW(w) == 0 {
            return true.into();
        }
        if GetWindow(w, GW_OWNER).is_ok_and(|o| !o.0.is_null()) {
            return true.into();
        }
        let pid = window_pid(h);
        let Some(exe) = exe_path(pid) else { return true.into() };
        let stem = exe.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        if NOT_GAMES.contains(&stem.as_str()) {
            return true.into();
        }
        if start_time(pid).is_none_or(|t| t < ctx.since) {
            return true.into();
        }
        ctx.found = h;
        false.into()
    }
    let mut ctx = Ctx { own, since: since - Duration::from_secs(2), found: 0 };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut Ctx as isize));
    }
    (ctx.found != 0).then_some(ctx.found)
}

/// Runs a console tool without flashing a console window.
fn quiet(cmd: &str, args: &[&str]) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new(cmd).args(args).creation_flags(0x0800_0000).status();
}

/// Closes processes the way the window's close button would, then forces what's still running.
pub fn close_processes(pids: &[u32]) -> bool {
    for pid in pids {
        quiet("taskkill", &["/PID", &pid.to_string(), "/T"]);
    }
    for _ in 0..10 {
        if !pids.iter().any(|p| alive(*p)) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let stubborn: Vec<u32> = pids.iter().copied().filter(|p| alive(*p)).collect();
    for pid in &stubborn {
        quiet("taskkill", &["/PID", &pid.to_string(), "/T", "/F"]);
    }
    !stubborn.is_empty()
}

pub struct Shortcut {
    pub target: PathBuf,
    pub args: String,
    pub cwd: Option<PathBuf>,
}

/// Reads a .lnk shortcut through the shell's own IShellLink.
pub fn read_shortcut(path: &Path) -> Option<Shortcut> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let wide: Vec<u16> = path.as_os_str().encode_wide_null();
        file.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        let mut buf = [0u16; MAX_PATH as usize * 2];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let target = from_wide(&buf);
        let mut abuf = [0u16; 2048];
        let _ = link.GetArguments(&mut abuf);
        let mut cbuf = [0u16; MAX_PATH as usize * 2];
        let _ = link.GetWorkingDirectory(&mut cbuf);
        let cwd = from_wide(&cbuf);
        if target.is_empty() {
            return None;
        }
        Some(Shortcut {
            target: PathBuf::from(target),
            args: from_wide(&abuf),
            cwd: (!cwd.is_empty()).then(|| PathBuf::from(cwd)),
        })
    }
}

fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

trait EncodeWideNull {
    fn encode_wide_null(&self) -> Vec<u16>;
}
impl EncodeWideNull for std::ffi::OsStr {
    fn encode_wide_null(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}

/// Keeps a window above all others (or releases it) without activating it.
pub fn set_topmost(h: isize, on: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE};
    if h == 0 {
        return;
    }
    unsafe {
        let _ = SetWindowPos(hwnd(h), if on { HWND_TOPMOST } else { HWND_NOTOPMOST }, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

pub fn is_window(h: isize) -> bool {
    h != 0 && unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(hwnd(h)).as_bool() }
}
