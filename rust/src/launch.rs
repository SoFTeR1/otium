//! Starting and closing games. steam:// links go through Steam (tracked by its RunningAppID);
//! exe files and shortcuts to exe files are started by us and tracked until their folder is quiet.

use crate::data::Game;
use crate::watch::{Bus, Event};
use crate::win;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub struct Started {
    /// true when we can tell when it exits (exe games); Steam games are tracked through Steam
    pub tracked: bool,
}

/// game id -> pid of the exe we started
pub type Pids = Arc<Mutex<HashMap<String, u32>>>;

/// Store launchers stay running after the game, which would look like playing forever.
const STORE_LAUNCHERS: &[&str] = &[
    "steam.exe", "epicgameslauncher.exe", "galaxyclient.exe", "ubisoftconnect.exe", "upc.exe", "eadesktop.exe",
    "origin.exe", "battle.net.exe", "riotclientservices.exe", "amazongames.exe",
];

struct ExeTarget {
    exe: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
}

fn clean(path: &str) -> &str {
    path.trim().trim_matches('"')
}

fn is_url(target: &str) -> bool {
    let t = target.to_ascii_lowercase();
    let scheme_like = t.split_once(':').is_some_and(|(s, _)| s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)));
    scheme_like && !t.starts_with("file:")
}

/// The exe behind a game entry: the path itself, or what a .lnk points to (with its arguments and
/// start folder), so shortcut games are tracked like exe games.
fn exe_target(game: &Game) -> Option<ExeTarget> {
    let p = clean(&game.path);
    let extra = split_args(game.args.as_deref().unwrap_or(""));
    let lower = p.to_ascii_lowercase();
    let t = if lower.ends_with(".exe") {
        let exe = PathBuf::from(p);
        let cwd = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        ExeTarget { exe, args: extra, cwd }
    } else if lower.ends_with(".lnk") {
        let l = win::read_shortcut(Path::new(p))?;
        if !l.target.to_string_lossy().to_ascii_lowercase().ends_with(".exe") || !l.target.exists() {
            return None;
        }
        let mut args = split_args(&l.args);
        args.extend(extra);
        let cwd = l.cwd.filter(|c| c.exists()).or_else(|| l.target.parent().map(Path::to_path_buf)).unwrap_or_default();
        ExeTarget { exe: l.target, args, cwd }
    } else {
        return None;
    };
    let name = t.exe.file_name()?.to_string_lossy().to_lowercase();
    (!STORE_LAUNCHERS.contains(&name.as_str())).then_some(t)
}

/// Folder of an exe game, only when it's the game's own (not a drive root, Windows or Program Files itself).
fn game_dir(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let low = dir.to_string_lossy().to_lowercase().trim_end_matches('\\').to_string();
    let env = |k: &str| std::env::var(k).ok().map(|v| v.to_lowercase().trim_end_matches('\\').to_string());
    let sys = env("SystemRoot").unwrap_or_else(|| "c:\\windows".into());
    let shared: Vec<String> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"].iter().filter_map(|k| env(k)).collect();
    if low.len() <= 2 || low == sys || low.starts_with(&(sys.clone() + "\\")) || shared.contains(&low) {
        return None;
    }
    Some(dir.to_path_buf())
}

pub fn launch(game: &Game, bus: &Bus, pids: &Pids) -> Result<Started, String> {
    let target = clean(&game.path);
    if target.is_empty() {
        return Err("Не указан путь к игре".into());
    }
    if is_url(target) && !target.chars().nth(1).is_some_and(|c| c == ':') {
        open::that_detached(target).map_err(|e| format!("Не удалось открыть: {e}"))?;
        return Ok(Started { tracked: false });
    }
    if !Path::new(target).exists() {
        return Err(format!("Файл не найден: {target}"));
    }
    let Some(exe) = exe_target(game) else {
        open::that_detached(target).map_err(|e| format!("Не удалось открыть: {e}"))?;
        return Ok(Started { tracked: false });
    };

    let started = SystemTime::now();
    let mut child = match Command::new(&exe.exe).args(&exe.args).current_dir(&exe.cwd).spawn() {
        Ok(c) => c,
        // e.g. the exe needs elevation: let the shell handle it
        Err(_) => {
            open::that_detached(target).map_err(|e| format!("Не удалось запустить: {e}"))?;
            return Ok(Started { tracked: false });
        }
    };
    let pid = child.id();
    pids.lock().unwrap().insert(game.id.clone(), pid);

    let (bus, pids, id, dir) = (bus.clone(), pids.clone(), game.id.clone(), game_dir(&exe.exe));
    std::thread::spawn(move || {
        let _ = child.wait();
        // a launcher-style exe may hand off to other processes of the game: wait until its folder is quiet
        if let Some(dir) = &dir {
            while !win::processes_under(dir).is_empty() {
                std::thread::sleep(Duration::from_secs(3));
            }
        }
        let mut map = pids.lock().unwrap();
        if map.get(&id) != Some(&pid) {
            return; // started again meanwhile
        }
        map.remove(&id);
        drop(map);
        bus.send(Event::Exited { id, start: started, end: SystemTime::now() });
    });
    Ok(Started { tracked: true })
}

/// Closes a running game: the exe we started, everything in its folder, or for Steam games
/// everything in the game's install folder. Returns an error text when nothing was found.
pub fn close(game: &Game, pids: &Pids) -> Result<(), String> {
    let mut list: Vec<u32> = pids.lock().unwrap().get(&game.id).copied().into_iter().collect();
    if let Some(id) = game.steam_app_id() {
        if let Some(dir) = crate::steam::install_dir(&id) {
            list.extend(win::processes_under(&dir));
        }
    } else if let Some(t) = exe_target(game) {
        if let Some(dir) = game_dir(&t.exe) {
            list.extend(win::processes_under(&dir));
        }
    }
    list.sort_unstable();
    list.dedup();
    list.retain(|p| *p != std::process::id());
    if list.is_empty() {
        return Err("Не нашёл процесс игры".into());
    }
    win::close_processes(&list);
    Ok(())
}

/// Closes whatever owns a window (the game OTIUM was summoned over).
pub fn close_window_owner(hwnd: isize) -> Result<(), String> {
    let pid = win::window_pid(hwnd);
    if pid == 0 || pid == std::process::id() {
        return Err("Не нашёл процесс игры".into());
    }
    win::close_processes(&[pid]);
    Ok(())
}

/// Splits a command line the way the old launcher did: spaces separate, quotes group.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' | '\t' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args() {
        assert_eq!(split_args(r#"-a "b c"  d"#), vec!["-a", "b c", "d"]);
        assert!(split_args("").is_empty());
    }

    #[test]
    fn urls() {
        assert!(is_url("steam://rungameid/730"));
        assert!(is_url("com.epicgames.launcher://apps/x"));
        assert!(!is_url("file:///C:/x.exe"));
    }

    #[test]
    fn dirs() {
        assert!(game_dir(Path::new("C:\\Program Files\\game.exe")).is_none());
        assert!(game_dir(Path::new("D:\\game.exe")).is_none());
        assert!(game_dir(Path::new("C:\\Windows\\System32\\notepad.exe")).is_none());
        assert_eq!(game_dir(Path::new("D:\\Games\\UFC4\\UFC4.exe")).unwrap(), PathBuf::from("D:\\Games\\UFC4"));
    }
}
