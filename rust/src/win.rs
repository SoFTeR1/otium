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
        } else if !IsWindowVisible(w).as_bool() {
            // hidden in the tray
            let _ = ShowWindow(w, windows::Win32::UI::WindowsAndMessaging::SW_SHOW);
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

/// Shows a window hidden in the tray and brings it to the front.
pub fn show(h: isize) {
    activate(h);
}

pub fn hide(h: isize) {
    if h != 0 {
        unsafe {
            let _ = ShowWindow(hwnd(h), windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
        }
    }
}

/// Whether a window can be treated as a game: not a known shell / launcher / chat app, and
/// covering its whole monitor unless `game_running` says a game is known to be up.
pub fn looks_like_game(h: isize, game_running: bool) -> bool {
    let pid = window_pid(h);
    let Some(exe) = exe_path(pid) else { return false };
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if NOT_GAMES.contains(&stem.as_str()) || EXTRA_NOT_GAMES.contains(&stem.as_str()) {
        return false;
    }
    game_running || covers_monitor(h)
}

const EXTRA_NOT_GAMES: &[&str] = &[
    "claude", "code", "devenv", "chrome", "msedge", "firefox", "browser", "opera", "telegram", "taskmgr",
    "searchhost", "startmenuexperiencehost", "shellexperiencehost", "applicationframehost", "textinputhost",
];

fn covers_monitor(h: isize) -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(hwnd(h), &mut r).is_err() {
            return false;
        }
        let m = MonitorFromWindow(hwnd(h), MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(m, &mut mi).as_bool() {
            return false;
        }
        let mr = mi.rcMonitor;
        r.left <= mr.left && r.top <= mr.top && r.right >= mr.right && r.bottom >= mr.bottom
    }
}

/// The exe's own icon at 256×256 (Windows scales smaller ones up), saved as PNG.
pub fn exe_icon_png(exe: &Path, out: &Path) -> bool {
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, PrivateExtractIconsW, HICON, ICONINFO};
    const SIZE: i32 = 256;
    let mut name = [0u16; 260];
    let wide: Vec<u16> = exe.as_os_str().encode_wide_vec();
    if wide.len() >= name.len() {
        return false;
    }
    name[..wide.len()].copy_from_slice(&wide);
    unsafe {
        let mut icons = [HICON::default()];
        let n = PrivateExtractIconsW(&name, 0, SIZE, SIZE, Some(&mut icons), None, 0);
        if n == 0 || n == u32::MAX || icons[0].is_invalid() {
            return false;
        }
        let icon = icons[0];
        let mut info = ICONINFO::default();
        let ok = GetIconInfo(icon, &mut info).is_ok();
        let mut px = vec![0u8; (SIZE * SIZE * 4) as usize];
        let mut got = false;
        if ok && !info.hbmColor.is_invalid() {
            let dc = GetDC(None);
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: SIZE,
                    biHeight: -SIZE, // top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            got = GetDIBits(dc, info.hbmColor, 0, SIZE as u32, Some(px.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS) == SIZE;
            ReleaseDC(None, dc);
        }
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor);
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask);
        }
        let _ = DestroyIcon(icon);
        if !got {
            return false;
        }
        // BGRA -> RGBA; very old icons carry no alpha at all: treat them as opaque
        let no_alpha = px.chunks(4).all(|p| p[3] == 0);
        for p in px.chunks_mut(4) {
            p.swap(0, 2);
            if no_alpha {
                p[3] = 255;
            }
        }
        let Some(img) = image::RgbaImage::from_raw(SIZE as u32, SIZE as u32, px) else { return false };
        if let Some(dir) = out.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        img.save_with_format(out, image::ImageFormat::Png).is_ok()
    }
}

trait WideVec {
    fn encode_wide_vec(&self) -> Vec<u16>;
}
impl WideVec for std::ffi::OsStr {
    fn encode_wide_vec(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().collect()
    }
}

/// Opens Explorer with the file selected.
pub fn show_in_folder(path: &Path) {
    use std::os::windows::process::CommandExt;
    if std::env::var_os("OTIUM_NO_OPEN").is_some() {
        eprintln!("show in folder (suppressed): {}", path.display());
        return;
    }
    let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}

/// A window handle for dialogs that need a parent (rfd) when only the HWND is at hand.
pub struct Owner(pub isize);

impl raw_window_handle::HasWindowHandle for Owner {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let h = std::num::NonZeroIsize::new(self.0).ok_or(raw_window_handle::HandleError::Unavailable)?;
        let raw = raw_window_handle::RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(h));
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

impl raw_window_handle::HasDisplayHandle for Owner {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(raw_window_handle::DisplayHandle::windows())
    }
}

/// The monitor a window is on, in physical pixels: (x, y, width, height).
pub fn monitor_rect(h: isize) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
    unsafe {
        let m = MonitorFromWindow(hwnd(h), MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(m, &mut info).as_bool() {
            return None;
        }
        let r = info.rcMonitor;
        Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
    }
}

/// Mouse position on screen, physical pixels.
pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = windows::Win32::Foundation::POINT::default();
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p).ok().map(|_| (p.x, p.y)) }
}

/// A top-level window of ours by its exact title.
pub fn find_own_window(title: &str) -> isize {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let Ok(w) = (unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) }) else { return 0 };
    if window_pid(w.0 as isize) != std::process::id() {
        return 0;
    }
    w.0 as isize
}

/// Makes the overlay a window that never takes focus (clicking it leaves the game in front),
/// keeps it above everything and stretches it over `rect`. `last_ex` remembers the extended style
/// we last saw: winit rewrites it when click-through toggles, and then it needs fixing again.
pub fn pin_overlay(h: isize, rect: (i32, i32, i32, i32), restack: bool, last_ex: &mut isize) {
    use windows::Win32::Graphics::Dwm::{DwmEnableBlurBehindWindow, DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND};
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    let w = hwnd(h);
    unsafe {
        let ex = GetWindowLongPtrW(w, GWL_EXSTYLE);
        if ex != *last_ex {
            let want = ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize;
            if ex != want {
                SetWindowLongPtrW(w, GWL_EXSTYLE, want);
            }
            if want & WS_EX_LAYERED.0 as isize != 0 {
                // click-through makes it layered: fully opaque as a layer, the per-pixel alpha
                // comes from the blur-behind below
                let _ = SetLayeredWindowAttributes(w, windows::Win32::Foundation::COLORREF(0), 255, LWA_ALPHA);
            }
            // winit turns on blur-behind (DWM composing with our alpha) when it creates the window,
            // but by the time the overlay is shown it has no effect and the clear colour comes out
            // black: turn it on again after every style change
            let region = CreateRectRgn(0, 0, -1, -1);
            let bb = DWM_BLURBEHIND { dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION, fEnable: true.into(), hRgnBlur: region, fTransitionOnMaximized: false.into() };
            let _ = DwmEnableBlurBehindWindow(w, &bb);
            let _ = DeleteObject(region);
            *last_ex = GetWindowLongPtrW(w, GWL_EXSTYLE);
        }
        let mut r = windows::Win32::Foundation::RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(w, &mut r);
        let placed = (r.left, r.top, r.right - r.left, r.bottom - r.top) == rect;
        if !placed {
            let _ = SetWindowPos(w, HWND_TOPMOST, rect.0, rect.1, rect.2, rect.3, SWP_NOACTIVATE);
        } else if restack {
            let _ = SetWindowPos(w, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// Creates (or overwrites) a .lnk at `path` that starts `target` in its own folder.
pub fn write_shortcut(path: &Path, target: &Path) -> bool {
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(link) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER) else { return false };
        let t = target.as_os_str().encode_wide_null();
        let _ = link.SetPath(PCWSTR(t.as_ptr()));
        if let Some(dir) = target.parent() {
            let d = dir.as_os_str().encode_wide_null();
            let _ = link.SetWorkingDirectory(PCWSTR(d.as_ptr()));
        }
        let _ = link.SetIconLocation(PCWSTR(t.as_ptr()), 0);
        let Ok(file) = link.cast::<IPersistFile>() else { return false };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let p = path.as_os_str().encode_wide_null();
        file.Save(PCWSTR(p.as_ptr()), true).is_ok()
    }
}
