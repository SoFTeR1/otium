//! Per-app volume, like the Windows volume mixer: the default speakers' master volume and every
//! app that has an audio session on them (Core Audio, the same calls `audio.js` makes through
//! PowerShell in the Electron build). Apps with several processes (browsers, Discord) are one row.

use std::path::PathBuf;
use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Foundation::{BOOL, S_OK};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};

#[derive(Debug, Clone)]
pub struct AppVolume {
    pub pids: Vec<u32>,
    pub name: String,
    pub exe: Option<PathBuf>,
    /// 0..=100
    pub volume: i32,
    pub muted: bool,
}

#[derive(Debug, Clone)]
pub struct Mixer {
    pub master: i32,
    pub master_muted: bool,
    pub apps: Vec<AppVolume>,
}

fn device() -> windows::core::Result<IMMDevice> {
    unsafe {
        // the UI thread may already be in an apartment of either kind; either works for these
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        en.GetDefaultAudioEndpoint(eRender, eMultimedia)
    }
}

fn endpoint() -> windows::core::Result<IAudioEndpointVolume> {
    unsafe { device()?.Activate(CLSCTX_ALL, None) }
}

/// Live sessions of real processes on the default speakers: (pid, volume control).
fn sessions() -> windows::core::Result<Vec<(u32, ISimpleAudioVolume)>> {
    let mut out = Vec::new();
    unsafe {
        let mgr: IAudioSessionManager2 = device()?.Activate(CLSCTX_ALL, None)?;
        let list = mgr.GetSessionEnumerator()?;
        for i in 0..list.GetCount()? {
            let Ok(s) = list.GetSession(i) else { continue };
            let Ok(s2) = s.cast::<IAudioSessionControl2>() else { continue };
            let pid = s2.GetProcessId().unwrap_or(0);
            if pid == 0 || s2.IsSystemSoundsSession() == S_OK || s.GetState().is_ok_and(|st| st == AudioSessionStateExpired) {
                continue;
            }
            if let Ok(v) = s.cast::<ISimpleAudioVolume>() {
                out.push((pid, v));
            }
        }
    }
    Ok(out)
}

pub fn list() -> Result<Mixer, String> {
    let ep = endpoint().map_err(|e| e.to_string())?;
    let (master, master_muted) = unsafe { (ep.GetMasterVolumeLevelScalar().unwrap_or(1.0), ep.GetMute().map(|b| b.as_bool()).unwrap_or(false)) };
    let mut apps: Vec<AppVolume> = Vec::new();
    for (pid, v) in sessions().map_err(|e| e.to_string())? {
        let exe = crate::win::exe_path(pid);
        let key = exe.as_ref().map(|p| p.to_string_lossy().to_lowercase());
        if let Some(a) = apps.iter_mut().find(|a| key.is_some() && a.exe.as_ref().map(|p| p.to_string_lossy().to_lowercase()) == key) {
            if !a.pids.contains(&pid) {
                a.pids.push(pid);
            }
            continue;
        }
        let (volume, muted) = unsafe { (v.GetMasterVolume().unwrap_or(1.0), v.GetMute().map(|b| b.as_bool()).unwrap_or(false)) };
        let name = exe.as_deref().map(app_name).unwrap_or_else(|| format!("Процесс {pid}"));
        apps.push(AppVolume { pids: vec![pid], name, exe, volume: pct(volume), muted });
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    Ok(Mixer { master: pct(master), master_muted, apps })
}

fn pct(v: f32) -> i32 {
    (v * 100.0).round().clamp(0.0, 100.0) as i32
}

pub fn set_master(v: i32) {
    if let Ok(ep) = endpoint() {
        unsafe {
            let _ = ep.SetMasterVolumeLevelScalar(v.clamp(0, 100) as f32 / 100.0, &GUID::zeroed());
        }
    }
}

pub fn set_master_mute(m: bool) {
    if let Ok(ep) = endpoint() {
        unsafe {
            let _ = ep.SetMute(BOOL::from(m), &GUID::zeroed());
        }
    }
}

pub fn set_app(pids: &[u32], v: i32) {
    for (pid, s) in sessions().unwrap_or_default() {
        if pids.contains(&pid) {
            unsafe {
                let _ = s.SetMasterVolume(v.clamp(0, 100) as f32 / 100.0, &GUID::zeroed());
            }
        }
    }
}

pub fn set_app_mute(pids: &[u32], m: bool) {
    for (pid, s) in sessions().unwrap_or_default() {
        if pids.contains(&pid) {
            unsafe {
                let _ = s.SetMute(BOOL::from(m), &GUID::zeroed());
            }
        }
    }
}

/// The exe's "File description" (what Task Manager shows: "Google Chrome", "Discord"), or its
/// file name.
pub fn app_name(exe: &std::path::Path) -> String {
    file_description(exe).filter(|d| !d.trim().is_empty()).unwrap_or_else(|| exe.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())
}

fn file_description(exe: &std::path::Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let wide: Vec<u16> = exe.as_os_str().encode_wide_z();
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wide.as_ptr()), 0, size, buf.as_mut_ptr() as *mut _).ok()?;
        let query = |q: &str| -> Option<(*const u8, u32)> {
            let q: Vec<u16> = q.encode_utf16().chain(Some(0)).collect();
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            VerQueryValueW(buf.as_ptr() as *const _, PCWSTR(q.as_ptr()), &mut ptr, &mut len).as_bool().then_some((ptr as *const u8, len))
        };
        // the first language the file has, else US English / Unicode
        let lang = match query("\\VarFileInfo\\Translation") {
            Some((p, len)) if len >= 4 => {
                let l = *(p as *const u16);
                let cp = *(p as *const u16).add(1);
                format!("{l:04x}{cp:04x}")
            }
            _ => "040904b0".to_string(),
        };
        let (p, len) = query(&format!("\\StringFileInfo\\{lang}\\FileDescription"))?;
        let chars = std::slice::from_raw_parts(p as *const u16, len as usize);
        let s = String::from_utf16_lossy(chars);
        Some(s.trim_end_matches('\0').trim().to_string())
    }
}

trait WideZ {
    fn encode_wide_z(&self) -> Vec<u16>;
}

impl WideZ for std::ffi::OsStr {
    fn encode_wide_z(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(Some(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    /// Reads the real mixer (nothing is changed): `cargo test -- --ignored real_mixer --nocapture`
    #[test]
    #[ignore]
    fn real_mixer() {
        let m = super::list().expect("mixer");
        println!("master {}%{}", m.master, if m.master_muted { " (muted)" } else { "" });
        for a in &m.apps {
            println!("{:>3}%{} {} {:?}", a.volume, if a.muted { " M" } else { "  " }, a.name, a.pids);
        }
    }

    #[test]
    fn names_from_version_info() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let name = super::app_name(std::path::Path::new(&format!("{windir}\\explorer.exe")));
        assert!(!name.is_empty() && name != "explorer", "{name}");
    }
}
