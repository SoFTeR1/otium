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

    fn store_root(&self) -> PathBuf {
        match self.lib.setting_str("storeDir") {
            "" => store::default_root(),
            d => PathBuf::from(d),
        }
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
        let root = self.store_root();
        let mut text = format!("Скачается с GitHub автора ({}) в папку {}.", item.repo, root.display());
        if let Some(n) = &item.needs {
            text = format!("Нужно: {n}. OTIUM этого не скачивает: программа попросит указать файлы сама.\n{text}");
        }
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
        let root = self.store_dir(&item.id).and_then(|d| d.parent().map(PathBuf::from)).unwrap_or_else(|| self.store_root());
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

    /// Настройки → Папка для игр магазина.
    pub(super) fn store_dir_value(&self) -> String {
        self.store_root().display().to_string()
    }

    pub(super) fn store_pick_dir(&mut self) {
        let Some(d) = self.dialog("Папка для игр магазина").pick_folder() else { return };
        self.lib.settings.insert("storeDir".into(), d.to_string_lossy().to_string().into());
        self.save();
    }
}
