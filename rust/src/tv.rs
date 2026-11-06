//! Moving the picture and sound to the TV and back. The configuration in use (mirrored screens,
//! the default speakers) is remembered first, so going back restores exactly that.

use std::sync::Mutex;
use windows::core::{Interface, GUID, HRESULT, IUnknown, PCWSTR};
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED, STGM_READ};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;

const ACTIVE: u32 = 1; // DISPLAYCONFIG_PATH_ACTIVE
const MODE_INVALID: u32 = 0xFFFF_FFFF;
const HDMI: i32 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    pub name: String,
    pub id: u32,
    pub hdmi: bool,
    pub active: bool,
}

struct Saved {
    paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    modes: Vec<DISPLAYCONFIG_MODE_INFO>,
    audio: Option<String>,
}

static SAVED: Mutex<Option<Saved>> = Mutex::new(None);

fn query(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>), String> {
    let (mut np, mut nm) = (0u32, 0u32);
    unsafe {
        check(GetDisplayConfigBufferSizes(flags, &mut np, &mut nm))?;
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        check(QueryDisplayConfig(flags, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None))?;
        paths.truncate(np as usize);
        modes.truncate(nm as usize);
        Ok((paths, modes))
    }
}

fn check(e: WIN32_ERROR) -> Result<(), String> {
    if e == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("Windows отказала: код {}", e.0))
    }
}

fn target_name(p: &DISPLAYCONFIG_PATH_INFO) -> String {
    let mut n = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
    n.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
    n.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
    n.header.adapterId = p.targetInfo.adapterId;
    n.header.id = p.targetInfo.id;
    unsafe { DisplayConfigGetDeviceInfo(&mut n.header) };
    let w = &n.monitorFriendlyDeviceName;
    String::from_utf16_lossy(&w[..w.iter().position(|c| *c == 0).unwrap_or(w.len())])
}

/// Every screen that's connected, whether shown on now or not.
pub fn displays() -> Vec<Display> {
    let Ok((paths, _)) = query(QDC_ALL_PATHS) else { return Vec::new() };
    let mut out: Vec<Display> = Vec::new();
    for p in paths.iter().filter(|p| p.targetInfo.targetAvailable.as_bool()) {
        let id = p.targetInfo.id;
        let active = p.flags & ACTIVE != 0;
        match out.iter_mut().find(|d| d.id == id) {
            Some(d) => d.active |= active,
            None => {
                let name = target_name(p);
                if !name.is_empty() {
                    out.push(Display { name, id, hdmi: p.targetInfo.outputTechnology.0 == HDMI, active });
                }
            }
        }
    }
    out
}

/// The screen to treat as the TV: the one chosen in the settings, else the one on HDMI.
pub fn pick<'a>(all: &'a [Display], chosen: &str) -> Option<&'a Display> {
    if all.len() < 2 {
        return None;
    }
    all.iter().find(|d| d.name == chosen).or_else(|| all.iter().find(|d| d.hdmi))
}

/// The picture is on this screen alone.
pub fn only_on(all: &[Display], name: &str) -> bool {
    let active: Vec<_> = all.iter().filter(|d| d.active).collect();
    active.len() == 1 && active[0].name == name
}

/// The screen alone, keeping the mode it's in right now: left to choose, Windows takes whatever it
/// last used for this screen alone, which may be a much lower resolution.
fn keep_mode(name: &str, flags: SET_DISPLAY_CONFIG_FLAGS) -> Option<Result<(), String>> {
    let (paths, modes) = query(QDC_ONLY_ACTIVE_PATHS).ok()?;
    let mut p = *paths.iter().find(|p| target_name(p) == name)?;
    let (src, tgt) = unsafe { (p.sourceInfo.Anonymous.modeInfoIdx, p.targetInfo.Anonymous.modeInfoIdx) };
    let picked = vec![*modes.get(src as usize)?, *modes.get(tgt as usize)?];
    p.sourceInfo.Anonymous.modeInfoIdx = 0;
    p.targetInfo.Anonymous.modeInfoIdx = 1;
    let r = unsafe { SetDisplayConfig(Some(&[p]), Some(&picked), flags | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES) };
    Some(check(WIN32_ERROR(r as u32)))
}

fn apply_only(name: &str, flags: SET_DISPLAY_CONFIG_FLAGS) -> Result<(), String> {
    if let Some(Ok(())) = keep_mode(name, flags) {
        return Ok(());
    }
    let (paths, _) = query(QDC_ALL_PATHS)?;
    let mut p = *paths.iter().find(|p| p.targetInfo.targetAvailable.as_bool() && target_name(p) == name).ok_or_else(|| format!("{name} не подключён"))?;
    p.flags |= ACTIVE;
    p.sourceInfo.Anonymous.modeInfoIdx = MODE_INVALID;
    p.targetInfo.Anonymous.modeInfoIdx = MODE_INVALID;
    let r = unsafe { SetDisplayConfig(Some(&[p]), None, flags | SDC_TOPOLOGY_SUPPLIED | SDC_ALLOW_PATH_ORDER_CHANGES) };
    check(WIN32_ERROR(r as u32))
}

/// Picture and sound to the TV. Blocking (the screens take a moment): off the UI thread.
pub fn to_tv(name: &str) -> Result<(), String> {
    {
        let mut saved = SAVED.lock().unwrap();
        if saved.is_none() {
            let (paths, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
            *saved = Some(Saved { paths, modes, audio: default_speakers() });
        }
    }
    apply_only(name, SDC_APPLY)?;
    if let Some(id) = speakers_of(name) {
        // no sound there is no reason to undo the picture
        let _ = set_default_speakers(&id);
    }
    Ok(())
}

/// Back to what was before, or, when that isn't known (OTIUM was restarted), to the other screen.
pub fn back(tv: &str) -> Result<(), String> {
    match SAVED.lock().unwrap().take() {
        Some(s) => {
            let r = unsafe { SetDisplayConfig(Some(&s.paths), Some(&s.modes), SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES) };
            check(WIN32_ERROR(r as u32))?;
            if let Some(id) = s.audio {
                let _ = set_default_speakers(&id);
            }
            Ok(())
        }
        None => {
            let other = displays().into_iter().find(|d| d.name != tv).ok_or("Другого экрана нет")?;
            apply_only(&other.name, SDC_APPLY)?;
            if let Some(id) = speakers_of(&other.name) {
                let _ = set_default_speakers(&id);
            }
            Ok(())
        }
    }
}

// ---------- sound ----------

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
    }
}

fn default_speakers() -> Option<String> {
    unsafe { enumerator().ok()?.GetDefaultAudioEndpoint(eRender, eConsole).ok()?.GetId().ok()?.to_string().ok() }
}

/// (id, name) of every active audio output.
fn speakers() -> Vec<(String, String)> {
    // PKEY_Device_FriendlyName
    const NAME: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
    let list = || -> windows::core::Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        unsafe {
            let all = enumerator()?.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            for i in 0..all.GetCount()? {
                let d = all.Item(i)?;
                let name = d.OpenPropertyStore(STGM_READ)?.GetValue(&NAME)?.to_string();
                out.push((d.GetId()?.to_string().unwrap_or_default(), name));
            }
        }
        Ok(out)
    };
    list().unwrap_or_default()
}

/// The sound output on this screen's cable: it carries the screen's name.
fn speakers_of(display: &str) -> Option<String> {
    speakers_named(&speakers(), display)
}

fn speakers_named(all: &[(String, String)], display: &str) -> Option<String> {
    let d = display.to_lowercase();
    all.iter().find(|(_, n)| n.to_lowercase().contains(&d)).map(|(id, _)| id.clone())
}

/// Windows has no public call for this: IPolicyConfig is what its own sound settings use.
fn set_default_speakers(id: &str) -> Result<(), String> {
    const CLSID: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
    const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    const SET_DEFAULT_ENDPOINT: usize = 13;
    type SetDefault = unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, i32) -> HRESULT;
    let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let unknown: IUnknown = CoCreateInstance(&CLSID, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let mut raw = std::ptr::null_mut();
        unknown.query(&IID, &mut raw).ok().map_err(|e| e.to_string())?;
        // takes over the reference `query` gave, and releases it at the end
        let policy = IUnknown::from_raw(raw);
        let vtable = *(policy.as_raw() as *const *const usize);
        let call: SetDefault = std::mem::transmute(*vtable.add(SET_DEFAULT_ENDPOINT));
        // console, multimedia, communications
        for role in 0..3 {
            call(policy.as_raw(), PCWSTR(wide.as_ptr()), role).ok().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(name: &str, hdmi: bool, active: bool) -> Display {
        Display { name: name.into(), id: 0, hdmi, active }
    }

    #[test]
    fn which_screen_is_the_tv() {
        let all = [d("AW2521", false, true), d("SAMSUNG", true, true)];
        assert_eq!(pick(&all, "").unwrap().name, "SAMSUNG");
        assert_eq!(pick(&all, "AW2521").unwrap().name, "AW2521");
        // a chosen screen that's gone: the HDMI one
        assert_eq!(pick(&all, "LG").unwrap().name, "SAMSUNG");
        // nothing to switch between, or no way to tell
        assert!(pick(&all[..1], "").is_none());
        assert!(pick(&[d("A", false, true), d("B", false, false)], "").is_none());
    }

    #[test]
    fn mirrored_is_not_on_the_tv_alone() {
        assert!(!only_on(&[d("M", false, true), d("TV", true, true)], "TV"));
        assert!(only_on(&[d("M", false, false), d("TV", true, true)], "TV"));
        assert!(!only_on(&[d("M", false, true), d("TV", true, false)], "TV"));
    }

    #[test]
    fn sound_by_screen_name() {
        let all = [("1".to_string(), "Динамики (Realtek)".to_string()), ("2".to_string(), "SAMSUNG (NVIDIA High Definition Audio)".to_string())];
        assert_eq!(speakers_named(&all, "Samsung").as_deref(), Some("2"));
        assert_eq!(speakers_named(&all, "LG"), None);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// cargo test --release tv_look -- --ignored --nocapture
    /// Reads and validates only: Windows checks the change without making it, nothing moves.
    #[test]
    #[ignore]
    fn tv_look() {
        let all = displays();
        println!("{all:?}");
        println!("tv: {:?}", pick(&all, "").map(|d| &d.name));
        println!("speakers: {:?}", speakers().iter().map(|s| &s.1).collect::<Vec<_>>());
        let cur = default_speakers();
        let name = speakers().into_iter().find(|s| Some(&s.0) == cur.as_ref()).map(|s| s.1);
        println!("default: {name:?}");
        // the call itself, changing nothing: the current output set as the current output
        println!("setting it again: {:?}", cur.as_deref().map(set_default_speakers));
        println!("still: {}", default_speakers() == cur);
        for d in &all {
            println!("{} alone is possible: {:?}, keeping its mode: {:?}, its sound: {}", d.name, apply_only(&d.name, SDC_VALIDATE), keep_mode(&d.name, SDC_VALIDATE), speakers_of(&d.name).is_some());
        }
    }
}
