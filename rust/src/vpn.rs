//! The VPN switch (v2RayTun): on / off from the Control Center, and off for the services that don't
//! work through it (Кинопоиск). v2RayTun takes commands as a link passed to its exe — the running
//! copy acts on it — and its tunnel adapter (sing-tun) shows whether the VPN is up.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// v2RayTun.exe: the Start menu or desktop shortcut, else the usual install folders.
pub fn find() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let links = [
        env("APPDATA").map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs\v2RayTun.lnk")),
        env("ProgramData").map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs\v2RayTun.lnk")),
        env("USERPROFILE").map(|p| p.join(r"Desktop\v2RayTun.lnk")),
        env("PUBLIC").map(|p| p.join(r"Desktop\v2RayTun.lnk")),
    ];
    for l in links.into_iter().flatten().filter(|l| l.exists()) {
        if let Some(s) = crate::win::read_shortcut(&l) {
            if is_v2raytun(&s.target) {
                return Some(s.target);
            }
        }
    }
    [
        env("LOCALAPPDATA").map(|p| p.join(r"Programs\v2RayTun\v2RayTun.exe")),
        env("ProgramFiles").map(|p| p.join(r"v2RayTun\v2RayTun.exe")),
        Some(PathBuf::from(r"C:\v2RayTun\v2RayTun.exe")),
        Some(PathBuf::from(r"D:\v2RayTun\v2RayTun.exe")),
    ]
    .into_iter()
    .flatten()
    .find(|p| is_v2raytun(p))
}

fn is_v2raytun(p: &Path) -> bool {
    p.exists() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("v2RayTun.exe"))
}

/// The VPN tunnel is up: v2RayTun's adapter (or any sing-tun one) is connected.
pub fn is_on() -> bool {
    use windows::Win32::NetworkManagement::IpHelper::{GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::AF_UNSPEC;
    let mut size = 16 * 1024u32;
    for _ in 0..3 {
        let mut buf = vec![0u8; size as usize];
        let head = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        let r = unsafe { GetAdaptersAddresses(AF_UNSPEC.0 as u32, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST, None, Some(head), &mut size) };
        if r == 111 {
            continue; // ERROR_BUFFER_OVERFLOW: `size` is now what it needs
        }
        if r != 0 {
            return false;
        }
        let mut p = head as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = unsafe { &*p };
            let name = unsafe { a.FriendlyName.to_string() }.unwrap_or_default();
            let desc = unsafe { a.Description.to_string() }.unwrap_or_default();
            if a.OperStatus == IfOperStatusUp && (name.eq_ignore_ascii_case("v2RayTun") || desc.to_lowercase().contains("sing-tun")) {
                return true;
            }
            p = a.Next;
        }
        return false;
    }
    false
}

/// Asks v2RayTun to connect or disconnect and waits (up to a few seconds) until the tunnel says so.
/// Blocking: run it off the UI thread. Returns the state the tunnel ended up in.
pub fn set(exe: &Path, on: bool) -> Result<bool, String> {
    let cmd = if on { "v2raytun://control/start" } else { "v2raytun://control/stop" };
    std::process::Command::new(exe).arg(cmd).spawn().map_err(|e| format!("Не удалось запустить v2RayTun: {e}"))?;
    let at = Instant::now();
    while at.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(400));
        if is_on() == on {
            return Ok(on);
        }
    }
    Ok(is_on())
}

#[cfg(test)]
mod live {
    /// cargo test --release vpn_state -- --ignored --nocapture  (reads only, switches nothing)
    #[test]
    #[ignore]
    fn vpn_state() {
        println!("v2RayTun: {:?}, tunnel up: {}", super::find(), super::is_on());
    }
}
