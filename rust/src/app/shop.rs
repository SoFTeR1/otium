//! The Store tab's state: what the catalog offers, what's installed (settings.store), the install in
//! progress, and the menu of an installed game.

use super::App;
use crate::data::{Game, Library};
use crate::modal::{Modal, SetItem, SetKey, SetMenu};
use crate::store::{self, Installed, Item, Stage};
use crate::watch::Event;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Default)]
pub struct Shop {
    pub items: Vec<Item>,
    /// the one install running now, and how far it got
    pub busy: Option<(String, Stage, u8)>,
    /// id -> the version in the latest release, for installed games that OTIUM updates
    pub latest: HashMap<String, String>,
    /// the catalog and versions were asked for this session
    checked: bool,
    /// the game the open menu or question is about
    current: Option<String>,
    /// the folder chooser: where it is (None: the list of disks), and what its rows do
    folder: Option<PathBuf>,
    rows: Vec<FolderRow>,
    /// the folder picked for the install about to start
    chosen: Option<PathBuf>,
}

enum FolderRow {
    Here,
    New,
    Up,
    Go(PathBuf),
}

/// A long path for a title: its end is what tells where one is.
fn short_path(p: &str) -> String {
    const MAX: usize = 44;
    let n = p.chars().count();
    if n <= MAX {
        return p.to_string();
    }
    format!("…{}", p.chars().skip(n - MAX).collect::<String>())
}

#[test]
fn long_paths_keep_their_end() {
    assert_eq!(short_path(r"D:\Игры"), r"D:\Игры");
    let long = format!(r"D:\{}\Игры", "a".repeat(60));
    let s = short_path(&long);
    assert!(s.starts_with('…') && s.ends_with("Игры") && s.chars().count() == 45);
}

fn gigabytes(bytes: u64) -> String {
    format!("свободно {} ГБ", bytes / (1024 * 1024 * 1024))
}

impl App {
    pub(super) fn shop_items(&mut self) -> &[Item] {
        if self.shop.items.is_empty() {
            self.shop.items = store::catalog();
        }
        &self.shop.items
    }

    fn store_record(&self, id: &str) -> Option<&Value> {
        self.lib.settings.get("store")?.get(id)
    }

    pub(super) fn store_version(&self, id: &str) -> Option<String> {
        self.store_record(id)?.get("version")?.as_str().map(str::to_string)
    }

    fn store_dir(&self, id: &str) -> Option<PathBuf> {
        self.store_record(id)?.get("dir")?.as_str().map(PathBuf::from)
    }

    /// The newer version available for an installed game that OTIUM updates.
    pub(super) fn store_update(&self, item: &Item) -> Option<String> {
        if item.self_updates {
            return None;
        }
        let have = self.store_version(&item.id)?;
        self.shop.latest.get(&item.id).filter(|v| **v != have).cloned()
    }

    /// The first look at the Store this session: fresh catalog, and versions of what's installed.
    pub(super) fn shop_opened(&mut self) {
        let _ = self.shop_items();
        if std::mem::replace(&mut self.shop.checked, true) {
            return;
        }
        let bus = self.bus.clone();
        let installed: Vec<String> = self.lib.settings.get("store").and_then(Value::as_object).map(|m| m.keys().cloned().collect()).unwrap_or_default();
        let have = self.shop.items.clone();
        std::thread::spawn(move || {
            let items = store::fetch_catalog().unwrap_or(have);
            for i in items.iter().filter(|i| !i.self_updates && installed.contains(&i.id)) {
                if let Ok(r) = store::latest(i) {
                    bus.send(Event::StoreLatest { id: i.id.clone(), version: r.version });
                }
            }
            bus.send(Event::StoreCatalog(items));
        });
    }

    /// A on a Store tile.
    pub(super) fn shop_activate(&mut self, i: usize) {
        let Some(item) = self.shop_items().get(i).cloned() else { return };
        if self.shop.busy.as_ref().is_some_and(|b| b.0 == item.id) {
            return;
        }
        if self.store_version(&item.id).is_some() {
            return self.store_play(&item);
        }
        if self.shop.busy.is_some() {
            return self.toast("Сначала дождитесь конца текущей установки", false);
        }
        self.shop.current = Some(item.id.clone());
        // from the last place chosen, if it's still there
        self.shop.folder = Some(PathBuf::from(self.lib.setting_str("storeDir"))).filter(|d| d.is_dir());
        self.modal_at = std::time::Instant::now();
        self.folder_show();
    }

    /// Fills the chooser with what's at its current place.
    pub(super) fn folder_show(&mut self) {
        let row = |label: String, value: String| SetItem { key: SetKey::FolderRow, label, value, danger: false };
        let (mut rows, mut items) = (Vec::new(), Vec::new());
        let title = match self.shop.folder.clone() {
            None => {
                for (root, name) in crate::win::drives() {
                    let label = if name.is_empty() { root.display().to_string() } else { format!("{} {name}", root.display()) };
                    items.push(row(label, crate::win::disk_free(&root).map(gigabytes).unwrap_or_default()));
                    rows.push(FolderRow::Go(root));
                }
                "Куда установить?".to_string()
            }
            Some(dir) => {
                items.push(row("Установить сюда".into(), crate::win::disk_free(&dir).map(gigabytes).unwrap_or_default()));
                rows.push(FolderRow::Here);
                items.push(row("Новая папка…".into(), String::new()));
                rows.push(FolderRow::New);
                items.push(row("‹ Наверх".into(), String::new()));
                rows.push(FolderRow::Up);
                for sub in store::subfolders(&dir) {
                    items.push(row(sub.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), String::new()));
                    rows.push(FolderRow::Go(sub));
                }
                short_path(&dir.display().to_string())
            }
        };
        self.shop.rows = rows;
        self.modal = Some(Modal::Settings { menu: SetMenu::Folder, title, items, focus: 0, scroll: 0.0 });
    }

    pub(super) fn folder_row(&mut self, i: usize) {
        match self.shop.rows.get(i) {
            Some(FolderRow::Here) => self.folder_chosen(),
            Some(FolderRow::New) => self.keyboard("Новая папка", "", crate::keyboard::Target::NewFolder, 60, false),
            Some(FolderRow::Up) => self.folder_up(),
            Some(FolderRow::Go(p)) => {
                self.shop.folder = Some(p.clone());
                self.folder_show();
            }
            None => {}
        }
    }

    /// B: one level up; from the list of disks, out.
    pub(super) fn folder_up(&mut self) {
        match self.shop.folder.take() {
            None => self.modal = None,
            Some(d) => {
                self.shop.folder = d.parent().map(PathBuf::from);
                self.folder_show();
            }
        }
    }

    pub(super) fn folder_create(&mut self, name: &str) {
        let Some(dir) = self.shop.folder.clone() else { return self.folder_show() };
        if !store::folder_name_ok(name) {
            self.toast("Такое имя папки Windows не примет", true);
            return self.folder_show();
        }
        let new = dir.join(name.trim());
        match std::fs::create_dir_all(&new) {
            Ok(()) => self.shop.folder = Some(new),
            Err(e) => self.toast(&format!("Не удалось создать папку: {e}"), true),
        }
        self.folder_show();
    }

    fn folder_chosen(&mut self) {
        let (Some(root), Some(item)) = (self.shop.folder.clone(), self.current_item()) else { return };
        let mut text = format!("Скачается с GitHub автора ({}) в {}.", item.repo, root.join(&item.id).display());
        if let Some(n) = &item.needs {
            text = format!("Нужно: {n}. OTIUM этого не скачивает: программа попросит указать файлы сама.\n{text}");
        }
        self.shop.chosen = Some(root);
        self.open(Modal::Confirm { title: format!("Установить {}?", item.name), text, yes: SetKey::StoreInstall, yes_label: "Установить", focus: 1 });
    }

    /// Y on an installed game's tile.
    pub(super) fn shop_menu(&mut self, i: usize) {
        let Some(item) = self.shop_items().get(i).cloned() else { return };
        if self.store_version(&item.id).is_none() || self.shop.busy.as_ref().is_some_and(|b| b.0 == item.id) {
            return;
        }
        let row = |key, label: String, value: String, danger| SetItem { key, label, value, danger };
        let mut items = vec![row(SetKey::StorePlay, "Запустить".into(), String::new(), false)];
        if let Some(v) = self.store_update(&item) {
            items.push(row(SetKey::StoreUpdate, "Обновить".into(), format!("до версии {v}"), false));
        }
        items.push(row(SetKey::StoreFolder, "Показать в папке".into(), String::new(), false));
        items.push(row(SetKey::StoreRemove, "Удалить".into(), String::new(), true));
        self.shop.current = Some(item.id.clone());
        self.open(Modal::Settings { menu: SetMenu::Store, title: item.name.clone(), items, focus: 0, scroll: 0.0 });
    }

    fn current_item(&self) -> Option<Item> {
        let id = self.shop.current.as_ref()?;
        self.shop.items.iter().find(|i| i.id == *id).cloned()
    }

    /// A row of that menu, or the answer to one of the questions.
    pub(super) fn shop_setting(&mut self, key: SetKey) {
        let Some(item) = self.current_item() else {
            self.modal = None;
            return;
        };
        match key {
            SetKey::StorePlay => {
                self.modal = None;
                self.store_play(&item);
            }
            SetKey::StoreInstall | SetKey::StoreUpdate => {
                self.modal = None;
                self.store_install(&item);
            }
            SetKey::StoreFolder => {
                self.modal = None;
                if let Some(d) = self.store_dir(&item.id) {
                    crate::win::show_in_folder(&d.join(&item.exe));
                }
            }
            SetKey::StoreRemove => self.open(Modal::Confirm {
                title: format!("Удалить {}?", item.name),
                text: "Папка игры удалится вместе со всем, что игра хранит внутри неё. Ваши исходные файлы игры, если они лежат отдельно, останутся.".into(),
                yes: SetKey::StoreRemoved,
                yes_label: "Удалить",
                focus: 0,
            }),
            SetKey::StoreRemoved => {
                self.modal = None;
                self.store_remove(&item);
            }
            _ => self.modal = None,
        }
    }

    fn store_install(&mut self, item: &Item) {
        if self.shop.busy.is_some() {
            return self.toast("Сначала дождитесь конца текущей установки", false);
        }
        self.shop.busy = Some((item.id.clone(), Stage::Download, 0));
        // an update goes where the game already is
        let chosen = self.shop.chosen.take();
        if let Some(c) = &chosen {
            self.lib.settings.insert("storeDir".into(), c.to_string_lossy().to_string().into());
            self.save();
        }
        let root = self.store_dir(&item.id).and_then(|d| d.parent().map(PathBuf::from)).or(chosen).unwrap_or_else(store::default_root);
        let (bus, item) = (self.bus.clone(), item.clone());
        std::thread::spawn(move || {
            let b = bus.clone();
            let id = item.id.clone();
            let res = store::install(&item, &root, move |stage, pct| b.send(Event::StoreProgress { id: id.clone(), stage, pct }));
            bus.send(Event::StoreDone { id: item.id.clone(), res });
        });
    }

    pub(super) fn on_store_event(&mut self, e: Event) {
        match e {
            Event::StoreCatalog(items) => self.shop.items = items,
            Event::StoreLatest { id, version } => {
                self.shop.latest.insert(id, version);
            }
            Event::StoreProgress { id, stage, pct } => {
                if self.shop.busy.as_ref().is_some_and(|b| b.0 == id) {
                    self.shop.busy = Some((id, stage, pct));
                }
            }
            Event::StoreDone { id, res } => {
                self.shop.busy = None;
                let name = self.shop.items.iter().find(|i| i.id == id).map_or(id.clone(), |i| i.name.clone());
                match res {
                    Ok(done) => {
                        self.store_installed(&id, &name, done);
                        self.toast(&format!("«{name}» установлена — она теперь и в «Играх»"), false);
                    }
                    Err(e) => self.toast(&format!("{name}: {e}"), true),
                }
            }
            _ => {}
        }
    }

    fn store_installed(&mut self, id: &str, name: &str, done: Installed) {
        let stored = json!({ "version": done.version, "dir": done.dir.to_string_lossy() });
        let store = self.lib.settings.entry("store").or_insert_with(|| json!({}));
        if let Some(m) = store.as_object_mut() {
            m.insert(id.to_string(), stored);
        }
        self.shop.latest.remove(id);
        let args = self.shop.items.iter().find(|i| i.id == id).map(|i| i.args.clone()).unwrap_or_default();
        self.ensure_store_game(id, name, &done.exe.to_string_lossy(), &args);
        self.save();
        self.refresh(None);
    }

    /// The game's tile in the library, for the given program (added, or pointed at it again).
    fn ensure_store_game(&mut self, id: &str, name: &str, exe: &str, args: &str) -> Game {
        let args = (!args.is_empty()).then(|| args.to_string());
        if let Some(g) = self.lib.games.iter_mut().find(|g| g.store_id() == Some(id)) {
            g.path = exe.to_string();
            g.args = args;
            return g.clone();
        }
        let mut g = Game::new(name, exe);
        g.cover = Library::exe_icon(exe);
        g.args = args;
        g.set_store_id(id);
        self.lib.games.push(g.clone());
        g
    }

    fn store_play(&mut self, item: &Item) {
        let Some(dir) = self.store_dir(&item.id) else { return };
        let exe = dir.join(&item.exe);
        if !exe.is_file() {
            return self.toast(&format!("Не нашёл {}: возможно, папку удалили или перенесли", exe.display()), true);
        }
        let g = self.ensure_store_game(&item.id, &item.name, &exe.to_string_lossy(), &item.args);
        self.save();
        self.refresh(Some(&g.id));
        self.start_launch(g);
    }

    fn store_remove(&mut self, item: &Item) {
        let running = self.lib.games.iter().any(|g| g.store_id() == Some(item.id.as_str()) && self.running.contains(&g.id));
        if running {
            return self.toast("Сначала закройте игру", false);
        }
        let Some(dir) = self.store_dir(&item.id) else { return };
        match store::remove(&item.id, &dir) {
            Ok(()) => {
                self.lib.games.retain(|g| g.store_id() != Some(item.id.as_str()));
                if let Some(m) = self.lib.settings.get_mut("store").and_then(Value::as_object_mut) {
                    m.remove(&item.id);
                }
                self.shop.latest.remove(&item.id);
                self.save();
                self.refresh(None);
                self.toast(&format!("«{}» удалена", item.name), false);
            }
            Err(e) => self.toast(&e, true),
        }
    }

}
