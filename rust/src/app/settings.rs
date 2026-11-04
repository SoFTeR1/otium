//! Settings screens, the power menu, tray / close handling and the Discord status.

use super::App;
use crate::input::PadKind;
use crate::keyboard::{Keyboard, Outcome, Target};
use crate::modal::{Modal, SetItem, SetKey, SetMenu};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

fn item(key: SetKey, label: &str, value: impl Into<String>) -> SetItem {
    SetItem { key, label: label.into(), value: value.into(), danger: false }
}

fn danger(key: SetKey, label: &str, value: impl Into<String>) -> SetItem {
    SetItem { key, label: label.into(), value: value.into(), danger: true }
}

fn onoff(v: bool) -> &'static str {
    if v {
        "Вкл"
    } else {
        "Выкл"
    }
}

impl App {
    fn set(&mut self, key: &str, v: impl Into<Value>) {
        self.lib.settings.insert(key.into(), v.into());
        self.save();
    }

    /// Glyphs to show: the setting wins over the detected controller.
    pub(super) fn pad_kind(&self) -> PadKind {
        match self.lib.setting_str("padLayout") {
            "ps" => PadKind::PlayStation,
            "xbox" => PadKind::Xbox,
            _ => self.input.pad_kind,
        }
    }

    pub(super) fn sounds_on(&self) -> bool {
        self.lib.setting_bool("sounds", true)
    }

    pub(super) fn open_settings(&mut self, menu: SetMenu, focus: usize) {
        let s = &self.lib;
        let (title, items) = match menu {
            SetMenu::Main => {
                let sort = match s.setting_str("sortMode") {
                    "alpha" | "name" => "По алфавиту",
                    "custom" => "Свой порядок",
                    _ => "Недавние",
                };
                let layout = match s.setting_str("padLayout") {
                    "ps" => "PlayStation",
                    "xbox" => "Xbox",
                    _ => "Автоматически",
                };
                let user = s.setting_str("user");
                (
                    "Настройки".to_string(),
                    vec![
                        item(SetKey::User, "Имя пользователя", if user.is_empty() { "Игрок" } else { user }),
                        item(SetKey::ImportSteam, "Импорт игр из Steam", ""),
                        item(SetKey::Account, "Аккаунт Steam (трофеи)", if s.setting_str("steamApiKey").is_empty() { "Не подключён" } else { "Подключён" }),
                        item(SetKey::Discord, "Статус в Discord", if s.setting_str("discordId").is_empty() { "Выкл" } else { "Вкл" }),
                        item(SetKey::Sort, "Порядок игр", sort),
                        item(SetKey::Layout, "Кнопки геймпада", layout),
                        item(SetKey::Sounds, "Звуки интерфейса", onoff(self.sounds_on())),
                        item(SetKey::Splash, "Заставка при запуске", onoff(s.setting_bool("splash", true))),
                        item(SetKey::Fullscreen, "Полноэкранный режим", onoff(self.fullscreen)),
                        item(SetKey::Autostart, "Запускать вместе с Windows", onoff(s.setting_bool("autostart", false))),
                        item(SetKey::Tray, "При закрытии окна", if s.setting_bool("closeToTray", true) { "Свернуть в трей" } else { "Выйти" }),
                        danger(SetKey::Power, "Питание", "сон, перезагрузка, выключение"),
                        item(SetKey::Version, "Версия", env!("CARGO_PKG_VERSION")),
                    ],
                )
            }
            SetMenu::Account => {
                let key = s.setting_str("steamApiKey");
                let key_v = if key.is_empty() {
                    "не указан".to_string()
                } else {
                    format!("••••{}", key.chars().skip(key.chars().count().saturating_sub(4)).collect::<String>())
                };
                let sid = s.setting_str("steamId");
                let sid_v = if !sid.is_empty() {
                    sid.to_string()
                } else {
                    match &self.steam_user_local {
                        Some(id) => format!("авто ({id})"),
                        None => "авто".into(),
                    }
                };
                (
                    "Аккаунт Steam".to_string(),
                    vec![
                        item(SetKey::ApiKey, "Steam Web API-ключ", key_v),
                        item(SetKey::SteamId, "SteamID64", sid_v),
                        item(SetKey::GetKey, "Получить ключ на сайте Steam", "откроется браузер"),
                    ],
                )
            }
            SetMenu::Discord => {
                let on = !s.setting_str("discordId").is_empty();
                let state = s.setting_str("discordState");
                let title = match (on, crate::discord::connected()) {
                    (false, _) => "Статус в Discord",
                    (true, true) => "Статус в Discord · подключён",
                    (true, false) => "Статус в Discord · Discord не запущен",
                };
                (
                    title.to_string(),
                    vec![
                        item(SetKey::DiscordOn, "Показывать статус", onoff(on)),
                        item(SetKey::DiscordGame, "Название и обложка игры", if s.setting_bool("discordHideGame", false) { "Скрыты" } else { "Видны" }),
                        item(SetKey::DiscordTimer, "Время в игре", if s.setting_bool("discordNoTimer", false) { "Скрыто" } else { "Видно" }),
                        item(SetKey::DiscordState, "Вторая строка", if state.trim().is_empty() { "через OTIUM" } else { state }),
                    ],
                )
            }
            // the friends panel builds these itself
            SetMenu::Profile | SetMenu::Friend => return,
            SetMenu::Power => (
                "Питание".to_string(),
                vec![
                    item(SetKey::Sleep, "Спящий режим", ""),
                    danger(SetKey::Restart, "Перезагрузка", ""),
                    danger(SetKey::Shutdown, "Выключение", ""),
                    item(SetKey::Quit, "Выйти из OTIUM", ""),
                ],
            ),
        };
        let focus = focus.min(items.len().saturating_sub(1));
        let scroll = match &self.modal {
            Some(Modal::Settings { menu: m, scroll, .. }) if *m == menu => *scroll,
            _ => 0.0,
        };
        let same = matches!(&self.modal, Some(Modal::Settings { menu: m, .. }) if *m == menu);
        self.modal = Some(Modal::Settings { menu, title, items, focus, scroll });
        if !same {
            self.modal_at = std::time::Instant::now();
        }
    }

    fn keyboard(&mut self, title: &str, text: &str, target: Target, max: usize, secret: bool) {
        self.open(Modal::Keyboard(Box::new(Keyboard::new(title, text, target, max, secret))));
    }

    pub(super) fn apply_setting(&mut self, menu: SetMenu, key: SetKey, focus: usize) {
        if matches!(menu, SetMenu::Profile | SetMenu::Friend) {
            return self.social_setting(key);
        }
        let cycle = |cur: &str, order: &[&str]| -> String {
            let i = order.iter().position(|x| *x == cur).unwrap_or(0);
            order[(i + 1) % order.len()].to_string()
        };
        match key {
            SetKey::User => {
                let u = self.lib.setting_str("user").to_string();
                self.keyboard("Имя пользователя", &u, Target::User, 24, false);
                return;
            }
            SetKey::ImportSteam => {
                self.modal = None;
                self.open_import();
                return;
            }
            SetKey::Account => return self.open_settings(SetMenu::Account, 0),
            SetKey::Discord => return self.open_settings(SetMenu::Discord, 0),
            SetKey::Power => return self.open_settings(SetMenu::Power, 0),
            SetKey::Sort => {
                let cur = match self.lib.setting_str("sortMode") {
                    "" => "recent",
                    "name" => "alpha",
                    x => x,
                }
                .to_string();
                let next = cycle(&cur, &["recent", "alpha", "custom"]);
                self.set("sortMode", next);
                self.refresh(None);
            }
            SetKey::Layout => {
                let next = cycle(self.lib.setting_str("padLayout"), &["auto", "ps", "xbox"]);
                self.set("padLayout", next);
            }
            SetKey::Sounds => {
                let v = !self.sounds_on();
                self.set("sounds", v);
            }
            SetKey::Splash => {
                let v = !self.lib.setting_bool("splash", true);
                self.set("splash", v);
            }
            SetKey::Fullscreen => {
                self.fullscreen = !self.fullscreen;
                self.fs_tries = 0;
                self.pending_cmd.push(egui::ViewportCommand::Fullscreen(self.fullscreen));
                let v = self.fullscreen;
                self.set("fullscreen", v);
            }
            SetKey::Autostart => {
                let v = !self.lib.setting_bool("autostart", false);
                match crate::tray::set_autostart(v) {
                    Ok(()) => self.set("autostart", v),
                    Err(e) => self.toast(&format!("Не удалось изменить автозапуск: {e}"), true),
                }
            }
            SetKey::Tray => {
                let v = !self.lib.setting_bool("closeToTray", true);
                self.set("closeToTray", v);
            }
            SetKey::Version => {}
            SetKey::ProfileCode
            | SetKey::ProfileRename
            | SetKey::ProfileKey
            | SetKey::ProfileLogout
            | SetKey::FriendChat
            | SetKey::FriendRemove => {}
            SetKey::ApiKey => {
                let k = self.lib.setting_str("steamApiKey").to_string();
                self.keyboard("Steam Web API-ключ", &k, Target::ApiKey, 64, true);
                return;
            }
            SetKey::SteamId => {
                let k = self.lib.setting_str("steamId").to_string();
                self.keyboard("SteamID64 (пусто = аккаунт, который последним входил в Steam)", &k, Target::SteamId, 20, false);
                return;
            }
            SetKey::GetKey => {
                let _ = open::that_detached("https://steamcommunity.com/dev/apikey");
                self.toast("Страница ключа открыта в браузере. В поле Domain можно написать localhost", false);
            }
            SetKey::DiscordOn => {
                let on = self.lib.setting_str("discordId").is_empty();
                self.set("discordId", if on { crate::discord::APP_ID } else { "" });
                crate::discord::configure(on.then_some(crate::discord::APP_ID));
                self.update_discord();
                self.toast(if on { "Статус в Discord включён — появится, когда Discord запущен" } else { "Статус в Discord выключен" }, false);
            }
            SetKey::DiscordGame => {
                let v = !self.lib.setting_bool("discordHideGame", false);
                self.set("discordHideGame", v);
                self.update_discord();
            }
            SetKey::DiscordTimer => {
                let v = !self.lib.setting_bool("discordNoTimer", false);
                self.set("discordNoTimer", v);
                self.update_discord();
            }
            SetKey::DiscordState => {
                let st = self.lib.setting_str("discordState").to_string();
                self.keyboard("Вторая строка статуса", &st, Target::DiscordState, 64, false);
                return;
            }
            SetKey::Sleep => {
                self.modal = None;
                power_sleep();
                return;
            }
            SetKey::Restart => {
                self.open(Modal::Confirm {
                    title: "Перезагрузить компьютер?".into(),
                    text: "Несохранённый прогресс в играх может пропасть".into(),
                    yes: SetKey::Restart,
                    yes_label: "Перезагрузить",
                    focus: 0,
                });
                return;
            }
            SetKey::Shutdown => {
                self.open(Modal::Confirm {
                    title: "Выключить компьютер?".into(),
                    text: "Несохранённый прогресс в играх может пропасть".into(),
                    yes: SetKey::Shutdown,
                    yes_label: "Выключить",
                    focus: 0,
                });
                return;
            }
            SetKey::Quit => {
                self.quit();
                return;
            }
        }
        self.open_settings(menu, focus);
    }

    pub(super) fn confirmed(&mut self, key: SetKey) {
        self.modal = None;
        if key == SetKey::ProfileLogout {
            return self.social_logout();
        }
        use std::os::windows::process::CommandExt;
        let arg = match key {
            SetKey::Restart => "/r",
            SetKey::Shutdown => "/s",
            _ => return,
        };
        let _ = std::process::Command::new("shutdown").args([arg, "/t", "0"]).creation_flags(0x0800_0000).spawn();
    }

    pub(super) fn text_done(&mut self, o: Outcome) {
        let Outcome::Done(target, text) = o else {
            // cancelled: back to where the keyboard was opened from
            if self.last_text_target.is_some_and(|t| t.is_social()) {
                self.modal = None;
                return;
            }
            return self.reopen_after_text(None);
        };
        if target.is_social() {
            return self.social_text(target, text);
        }
        match target {
            Target::User => {
                let name: String = if text.is_empty() { "Игрок".into() } else { text };
                self.set("user", name);
            }
            Target::ApiKey => {
                let key: String = text.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                self.set("steamApiKey", key);
                self.pages_forget_trophies();
            }
            Target::SteamId => {
                let id: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
                self.set("steamId", id);
                self.pages_forget_trophies();
            }
            Target::DiscordState => {
                self.set("discordState", text);
                self.update_discord();
            }
            _ => {}
        }
        self.reopen_after_text(Some(target));
    }

    fn reopen_after_text(&mut self, target: Option<Target>) {
        let target = target.or(self.last_text_target);
        match target {
            Some(Target::ApiKey) => self.open_settings(SetMenu::Account, 0),
            Some(Target::SteamId) => self.open_settings(SetMenu::Account, 1),
            Some(Target::DiscordState) => self.open_settings(SetMenu::Discord, 3),
            _ => self.open_settings(SetMenu::Main, 0),
        }
    }

    /// Leaves OTIUM for real (the tray "Выйти", the power menu).
    pub(super) fn quit(&mut self) {
        self.quitting = true;
        self.pending_cmd.push(egui::ViewportCommand::Close);
    }

    /// The window's close button / Alt+F4: hide to the tray instead, unless that's turned off.
    pub(super) fn on_close_requested(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) || self.quitting {
            return;
        }
        if self.tray.is_some() && self.lib.setting_bool("closeToTray", true) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            // hidden through Win32, not egui: egui doesn't repaint a window it hid itself, so it
            // could never be told to show it again
            crate::win::hide(self.own.load(std::sync::atomic::Ordering::Relaxed));
            self.modal = None;
            if !self.tray_hinted {
                self.tray_hinted = true;
                // shown when the window comes back
                self.toast("OTIUM работает в трее — кнопка Xbox или Ctrl+Alt+Home вернёт его", false);
            }
        }
    }

    /// Brings the window back from the tray through egui, which hid it.
    pub(super) fn show_window(&mut self) {
        crate::win::show(self.own.load(std::sync::atomic::Ordering::Relaxed));
    }

    pub(super) fn poll_tray(&mut self) {
        let Some(t) = &self.tray else { return };
        match t.poll() {
            Some(crate::tray::TrayAction::Show) => self.show_window(),
            Some(crate::tray::TrayAction::Quit) => self.quit(),
            None => {}
        }
    }

    // ---------- Discord ----------

    /// Public art Discord can show for a game: Steam store art, an https cover, or the art the
    /// Electron build looked up once (stored as `publicArt`).
    fn discord_art(g: &crate::data::Game) -> Option<String> {
        if let Some(id) = g.steam_app_id() {
            return Some(format!("{}/{id}/header.jpg", crate::steam::CDN));
        }
        if let Some(c) = g.cover.as_deref().filter(|c| c.starts_with("https://")) {
            return Some(c.to_string());
        }
        let pa = g.extra.get("publicArt")?;
        (pa["for"].as_str() == Some(&g.name)).then(|| pa["url"].as_str().map(str::to_string)).flatten().filter(|u| !u.is_empty())
    }

    pub(super) fn update_discord(&mut self) {
        if self.lib.setting_str("discordId").is_empty() {
            return;
        }
        let s = &self.lib;
        let icon = crate::discord::APP_ICON;
        let running = self.games.iter().find(|g| self.running.contains(&g.id));
        let act = match running {
            Some(g) => {
                let hide = s.setting_bool("discordHideGame", false);
                let state = s.setting_str("discordState").trim().to_string();
                let mut a = json!({
                    "details": if hide { "Играет в игру".to_string() } else { format!("Играет в {}", g.name) },
                    "state": if state.is_empty() { "через OTIUM".to_string() } else { state },
                    "assets": {
                        "large_image": if hide { None } else { Self::discord_art(g) }.unwrap_or_else(|| icon.to_string()),
                        "large_text": if hide { "OTIUM".to_string() } else { g.name.clone() },
                        "small_image": icon,
                        "small_text": "OTIUM",
                    }
                });
                if !s.setting_bool("discordNoTimer", false) {
                    let start = g
                        .steam_app_id()
                        .and_then(|id| id.parse::<u32>().ok())
                        .and_then(|id| self.steam_since.get(&id).copied())
                        .or(self.last_launch)
                        .unwrap_or_else(SystemTime::now);
                    let ms = start.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
                    a["timestamps"] = json!({ "start": ms });
                }
                a
            }
            None => {
                let n = self.lib.games.len();
                json!({
                    "details": "В главном меню",
                    "state": format!("{n} {} в библиотеке", super::plural(n, "игра", "игры", "игр")),
                    "assets": { "large_image": icon, "large_text": "OTIUM" }
                })
            }
        };
        crate::discord::set_activity(Some(act));
    }
}

fn power_sleep() {
    use windows::Win32::System::Power::SetSuspendState;
    // sleep, not hibernate
    unsafe {
        let _ = SetSuspendState(false, false, false);
    }
}
