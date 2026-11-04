//! Changing a game: the "…" menu, the editor (name, path, arguments, section, cover, background),
//! "Переставить" and "Показать в папке". Every change is saved right away, like the settings screens.

use super::{App, Zone};
use crate::data::{Game, Library};
use crate::input::Nav;
use crate::keyboard::Target;
use crate::modal::{Modal, Opt, SetItem, SetKey, SetMenu};

const IMAGES: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// Where a game's cover comes from, as the editor shows it.
fn cover_source(g: &Game) -> &'static str {
    let c = g.cover.as_deref().unwrap_or("").trim();
    if c.is_empty() {
        return if g.steam_app_id().is_some() { "Steam" } else { "нет" };
    }
    let lower = c.to_lowercase();
    if lower.contains("/media/icon_") {
        "иконка из exe"
    } else if c.starts_with(crate::steam::CDN) || lower.contains("/appcache/librarycache/") {
        "Steam"
    } else {
        "своя"
    }
}

fn short(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max {
        path.to_string()
    } else {
        format!("…{}", path.chars().skip(n - max + 1).collect::<String>())
    }
}

impl App {
    fn game_mut(&mut self, id: &str) -> Option<&mut Game> {
        self.lib.games.iter_mut().find(|g| g.id == id)
    }

    fn edited(&self) -> Option<Game> {
        let id = self.edit_id.as_deref()?;
        self.lib.games.iter().find(|g| g.id == id).cloned()
    }

    // ---------- "…" menu ----------

    pub(super) fn open_options(&mut self) {
        let Some(g) = self.games.get(self.sel).cloned() else { return };
        let app = g.is_app();
        let mut items = vec![
            if self.is_running(&g) {
                (Opt::CloseGame, if app { "Закрыть" } else { "Закрыть игру" }.to_string())
            } else {
                (Opt::Play, if app { "Открыть" } else { "Играть" }.to_string())
            },
            (Opt::Edit, "Изменить".into()),
            (Opt::MakeFirst, "Сделать первой в списке".into()),
        ];
        if self.games.len() > 1 {
            items.push((Opt::Reorder, "Переставить".into()));
        }
        items.push((Opt::SetBackground, "Установить свой фон".into()));
        if g.custom_background().is_some() {
            items.push((Opt::ResetBackground, "Убрать свой фон".into()));
        }
        if g.is_file() {
            items.push((Opt::ShowInFolder, "Показать в папке".into()));
        }
        items.push((Opt::Delete, "Удалить из библиотеки".into()));
        self.open(Modal::Options { focus: 0, game: g.name.clone(), items });
    }

    pub(super) fn option(&mut self, o: Opt) {
        let Some(g) = self.games.get(self.sel).cloned() else {
            self.modal = None;
            return;
        };
        match o {
            Opt::Play => {
                self.modal = None;
                self.launch_selected();
            }
            Opt::CloseGame => {
                self.modal = None;
                self.close_game(Some(g));
            }
            Opt::Edit => {
                self.edit_id = Some(g.id.clone());
                self.open_editor(0);
            }
            Opt::MakeFirst => {
                self.modal = None;
                self.make_first();
                self.toast(&format!("«{}» теперь первая", g.name), false);
            }
            Opt::Reorder => {
                self.modal = None;
                self.start_reorder();
            }
            Opt::SetBackground => {
                self.modal = None;
                self.edit_id = Some(g.id.clone());
                if self.pick_background() {
                    self.toast("Фон установлен", false);
                }
            }
            Opt::ResetBackground => {
                self.modal = None;
                self.set_background(&g.id, "");
                self.toast("Свой фон убран", false);
            }
            Opt::ShowInFolder => {
                self.modal = None;
                crate::win::show_in_folder(std::path::Path::new(&g.path));
            }
            Opt::Delete => {
                self.open(Modal::ConfirmDelete { focus: 0, game: g.name.clone() });
            }
        }
    }

    // ---------- editor ----------

    pub(super) fn open_editor(&mut self, focus: usize) {
        let Some(g) = self.edited() else {
            self.modal = None;
            return;
        };
        let item = |key, label: &str, value: String| SetItem { key, label: label.into(), value, danger: false };
        let args = g.args.clone().unwrap_or_default();
        let mut items = vec![
            item(SetKey::EditName, "Название", g.name.clone()),
            item(SetKey::EditPath, "Путь", short(&g.path, 38)),
            item(SetKey::EditBrowse, "Выбрать exe или ярлык…", String::new()),
            item(SetKey::EditArgs, "Аргументы запуска", if args.is_empty() { "нет".into() } else { short(&args, 30) }),
            item(SetKey::EditCategory, "Раздел", if g.is_app() { "Приложение (Медиа)" } else { "Игра" }.into()),
            item(SetKey::EditCover, "Обложка", format!("{}  · выбрать файл", cover_source(&g))),
        ];
        if g.path.to_lowercase().ends_with(".exe") && g.is_file() {
            items.push(item(SetKey::EditIcon, "Обложка из иконки exe", String::new()));
        }
        if matches!(cover_source(&g), "своя" | "иконка из exe") {
            items.push(item(SetKey::EditCoverReset, "Убрать свою обложку", String::new()));
        }
        let bg = if g.custom_background().is_some() { "свой" } else { "арт игры" };
        items.push(item(SetKey::EditBackground, "Фон", format!("{bg}  · выбрать файл")));
        if g.custom_background().is_some() {
            items.push(item(SetKey::EditBackgroundReset, "Убрать свой фон", String::new()));
        }
        let focus = focus.min(items.len() - 1);
        self.open(Modal::Settings { menu: SetMenu::Edit, title: format!("Изменить · {}", g.name), items, focus, scroll: 0.0 });
    }

    pub(super) fn edit_setting(&mut self, key: SetKey, focus: usize) {
        let Some(g) = self.edited() else {
            self.modal = None;
            return;
        };
        match key {
            SetKey::EditName => return self.keyboard("Название", &g.name, Target::GameName, 80, false),
            SetKey::EditPath => return self.keyboard("Путь к exe, ярлыку или ссылке", &g.path, Target::GamePath, 400, false),
            SetKey::EditArgs => return self.keyboard("Аргументы запуска", g.args.as_deref().unwrap_or(""), Target::GameArgs, 300, false),
            SetKey::EditBrowse => {
                let picked = self.dialog("Выберите игру или приложение").add_filter("Игры и ярлыки", &["exe", "lnk", "url", "bat", "cmd"]).pick_file();
                if let Some(p) = picked {
                    self.set_path(&g.id, &p.to_string_lossy());
                }
            }
            SetKey::EditCategory => {
                let next = if g.is_app() { "game" } else { "app" };
                if let Some(x) = self.game_mut(&g.id) {
                    x.extra.insert("category".into(), next.into());
                }
                self.save_edit(&g.id);
            }
            SetKey::EditCover => {
                let picked = self.dialog("Обложка (квадратная)").add_filter("Картинки", &IMAGES).pick_file();
                if let Some(p) = picked {
                    match Library::import_media(&p) {
                        Ok(url) => self.set_cover(&g.id, Some(url)),
                        Err(e) => self.toast(&format!("Не удалось скопировать картинку: {e}"), true),
                    }
                }
            }
            SetKey::EditIcon => match Library::exe_icon(&g.path) {
                Some(url) => self.set_cover(&g.id, Some(url)),
                None => self.toast("В этом exe нет иконки", true),
            },
            SetKey::EditCoverReset => {
                // back to Steam's art for Steam games, to the exe's icon otherwise
                let cover = match g.steam_app_id() {
                    Some(id) => Some(format!("{}/{id}/library_600x900.jpg", crate::steam::CDN)),
                    None => Library::exe_icon(&g.path),
                };
                self.set_cover(&g.id, cover);
            }
            SetKey::EditBackground => {
                self.pick_background();
            }
            SetKey::EditBackgroundReset => self.set_background(&g.id, ""),
            _ => {}
        }
        self.open_editor(focus);
    }

    /// Text typed for the editor; empty names and paths are not allowed.
    pub(super) fn edit_text(&mut self, target: Target, text: String) {
        let Some(g) = self.edited() else {
            self.modal = None;
            return;
        };
        let text = text.trim().to_string();
        let focus = match target {
            Target::GameName => {
                if text.is_empty() {
                    self.toast("Укажите название", true);
                } else if let Some(x) = self.game_mut(&g.id) {
                    x.name = text;
                    self.save_edit(&g.id);
                }
                0
            }
            Target::GamePath => {
                let path = text.trim_matches('"').to_string();
                if path.is_empty() {
                    self.toast("Укажите путь к игре", true);
                } else {
                    self.set_path(&g.id, &path);
                }
                1
            }
            Target::GameArgs => {
                if let Some(x) = self.game_mut(&g.id) {
                    x.args = Some(text);
                    self.save_edit(&g.id);
                }
                3
            }
            _ => 0,
        };
        self.open_editor(focus);
    }

    /// Where the keyboard goes back to when it's cancelled.
    pub(super) fn edit_text_cancelled(&mut self, target: Target) {
        let focus = match target {
            Target::GamePath => 1,
            Target::GameArgs => 3,
            _ => 0,
        };
        self.open_editor(focus);
    }

    fn set_path(&mut self, id: &str, path: &str) {
        let Some(x) = self.game_mut(id) else { return };
        x.path = path.to_string();
        // a game added without a picture gets the new exe's icon, like the Electron editor does on save
        let no_cover = x.cover.as_deref().is_none_or(|c| c.trim().is_empty());
        if no_cover && x.steam_app_id().is_none() {
            x.cover = Library::exe_icon(path);
        }
        self.save_edit(id);
    }

    fn set_cover(&mut self, id: &str, cover: Option<String>) {
        if let Some(x) = self.game_mut(id) {
            x.cover = cover;
        }
        self.save_edit(id);
    }

    fn set_background(&mut self, id: &str, url: &str) {
        if let Some(x) = self.game_mut(id) {
            x.set_background(url);
        }
        self.save_edit(id);
        // show the new background straight away
        self.bg_prev.clear();
        self.bg_changed = std::time::Instant::now();
    }

    /// File dialog for the edited game's background; true when one was set.
    fn pick_background(&mut self) -> bool {
        let Some(id) = self.edit_id.clone() else { return false };
        let picked = self.dialog("Свой фон (широкий)").add_filter("Картинки", &IMAGES).pick_file();
        let Some(p) = picked else { return false };
        match Library::import_media(&p) {
            Ok(url) => {
                self.set_background(&id, &url);
                true
            }
            Err(e) => {
                self.toast(&format!("Не удалось скопировать картинку: {e}"), true);
                false
            }
        }
    }

    fn save_edit(&mut self, id: &str) {
        self.save();
        self.refresh(Some(id));
        self.update_discord();
    }

    /// A file dialog over OTIUM's window; the window gets the focus back afterwards.
    pub(super) fn dialog(&mut self, title: &str) -> rfd::FileDialog {
        self.refocus = true;
        let own = crate::win::Owner(self.own.load(std::sync::atomic::Ordering::Relaxed));
        let d = rfd::FileDialog::new().set_title(title);
        if own.0 != 0 {
            d.set_parent(&own)
        } else {
            d
        }
    }

    // ---------- custom order ----------

    /// Writes the order on screen into `order` and switches the sort to "custom".
    fn freeze_order(&mut self) {
        for (i, g) in self.games.iter().enumerate() {
            if let Some(lg) = self.lib.games.iter_mut().find(|x| x.id == g.id) {
                lg.order = Some(i as f64);
            }
        }
        self.lib.settings.insert("sortMode".into(), "custom".into());
    }

    fn start_reorder(&mut self) {
        if self.games.len() < 2 {
            return;
        }
        self.freeze_order();
        self.save();
        self.refresh(None);
        self.reorder = true;
        self.zone = Zone::Row;
        self.toast("Двигайте плитку влево и вправо, затем нажмите «Готово»", false);
    }

    /// Left / Right move the selected tile; OK or Back ends.
    pub(super) fn reorder_nav(&mut self, nav: Nav) {
        let n = self.games.len();
        let j = match nav {
            Nav::Left if self.sel > 0 => self.sel - 1,
            Nav::Right if self.sel + 1 < n => self.sel + 1,
            Nav::Ok | Nav::Back => {
                self.reorder = false;
                self.save();
                return;
            }
            _ => return,
        };
        let (a, b) = (self.games[self.sel].id.clone(), self.games[j].id.clone());
        let oa = self.lib.games.iter().find(|g| g.id == a).and_then(|g| g.order);
        let ob = self.lib.games.iter().find(|g| g.id == b).and_then(|g| g.order);
        if let Some(g) = self.game_mut(&a) {
            g.order = ob;
        }
        if let Some(g) = self.game_mut(&b) {
            g.order = oa;
        }
        // the tile moves, the selection follows it without the background fading
        self.games.swap(self.sel, j);
        self.sel = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_sources() {
        let mut g = Game::new("x", "C:\\x.exe");
        assert_eq!(cover_source(&g), "нет");
        g.cover = Some("file:///C:/Users/u/AppData/Roaming/OTIUM/media/icon_1.png".into());
        assert_eq!(cover_source(&g), "иконка из exe");
        g.cover = Some("file:///C:/Users/u/AppData/Roaming/OTIUM/media/1_ab.jpg".into());
        assert_eq!(cover_source(&g), "своя");
        g.cover = Some(format!("{}/730/library_600x900.jpg", crate::steam::CDN));
        assert_eq!(cover_source(&g), "Steam");
        g.cover = Some("file:///C:/Program%20Files%20(x86)/Steam/appcache/librarycache/730/library_600x900.jpg".into());
        assert_eq!(cover_source(&g), "Steam");
    }

    #[test]
    fn shortens_paths() {
        assert_eq!(short("C:\\a.exe", 20), "C:\\a.exe");
        assert_eq!(short("C:\\Games\\Some Long Folder\\game.exe", 12), "…er\\game.exe");
    }
}
