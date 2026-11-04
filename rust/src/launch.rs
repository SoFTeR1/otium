//! Starting games: steam:// links and shortcuts go through the shell, exe files run directly.

use crate::data::Game;
use std::path::Path;
use std::process::Command;

pub fn launch(game: &Game) -> Result<(), String> {
    let target = game.path.trim().trim_matches('"');
    if target.is_empty() {
        return Err("Не указан путь к игре".into());
    }
    let lower = target.to_ascii_lowercase();
    let is_exe = lower.ends_with(".exe") && !lower.contains("://");
    if !is_exe {
        return open::that_detached(target).map_err(|e| format!("Не удалось открыть: {e}"));
    }
    let path = Path::new(target);
    if !path.exists() {
        return Err(format!("Файл не найден: {target}"));
    }
    let mut cmd = Command::new(path);
    cmd.args(split_args(&game.args));
    if let Some(dir) = path.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("Не удалось запустить: {e}"))
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
    use super::split_args;

    #[test]
    fn args() {
        assert_eq!(split_args(r#"-a "b c"  d"#), vec!["-a", "b c", "d"]);
        assert!(split_args("").is_empty());
    }
}
