//! Settings screens, the power menu, tray / close handling and the Discord status.

use super::App;
use crate::input::{Nav, PadKind};
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
        if menu == SetMenu::Main {
            self.tv_refresh();
        }
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
                let pad = match self.input.snapshot() {
                    None => "Не подключён",
                    Some(st) if self.input.calibration(&st.key).is_some() => "Откалиброван",
                    Some(_) => "Подключён",
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
                        item(SetKey::PadTest, "Геймпад: проверка и калибровка", pad),
                        item(SetKey::VpnKino, "VPN для Кинопоиска", self.vpn_setting_value()),
                        item(SetKey::PadApps, "Геймпад в «Медиа»", if s.setting_bool("padApps", true) { "Мышь и кнопки" } else { "Выкл (мышь даёт Steam)" }),
                        item(SetKey::TvDisplay, "Телевизор", self.tv_setting_value()),
                        item(SetKey::Backgrounds, "Фоны", if s.setting_str("bgMode") == "global" { "Общий фон" } else { "Арт игры" }),
                        item(SetKey::Sounds, "Звуки интерфейса", onoff(self.sounds_on())),
                        item(SetKey::Ambient, "Фоновая музыка", if crate::ambient::available() { self.volume_text(SetKey::Ambient) } else { "нет в этой сборке".to_string() }),
                        item(SetKey::Splash, "Заставка при запуске", onoff(s.setting_bool("splash", true))),
                        item(SetKey::SplashSound, "Звук заставки", self.volume_text(SetKey::SplashSound)),
                        item(SetKey::Fullscreen, "Полноэкранный режим", onoff(self.fullscreen)),
                        item(SetKey::Autostart, "Запускать вместе с Windows", onoff(s.setting_bool("autostart", false))),
                        item(SetKey::Tray, "При закрытии окна", if s.setting_bool("closeToTray", true) { "Свернуть в трей" } else { "Выйти" }),
                        danger(SetKey::Power, "Питание", "сон, перезагрузка, выключение"),
                        item(SetKey::Version, "Версия", self.version_value()),
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
            SetMenu::Profile | SetMenu::Friend | SetMenu::Call | SetMenu::Group => return,
            SetMenu::Edit => return self.open_editor(focus),
            SetMenu::Timer => self.timer_items(),
            // built by search_text
            SetMenu::Search | SetMenu::Vpn => return,
            SetMenu::Backgrounds => self.bg_menu(),
            SetMenu::Power => (
                "Питание".to_string(),
                vec![
                    item(SetKey::TimerOpen, "Таймер выключения", self.timer.left().map(super::control::fmt_left).unwrap_or_default()),
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

    pub(super) fn keyboard(&mut self, title: &str, text: &str, target: Target, max: usize, secret: bool) {
        self.open(Modal::Keyboard(Box::new(Keyboard::new(title, text, target, max, secret))));
    }

    /// The volume of the splash chord or the background music, in percent.
    pub(super) fn volume_percent(&self, key: SetKey) -> u32 {
        let stored = |k: &str| self.lib.settings.get(k).and_then(Value::as_u64).map(|v| v.min(100) as u32);
        match key {
            SetKey::SplashSound => stored("splashVolume").unwrap_or(30),
            // the words earlier versions stored
            _ => stored("ambientVolume").unwrap_or(match self.lib.setting_str("ambient") {
                "off" => 0,
                "low" => 30,
                "high" => 80,
                _ => 50,
            }),
        }
    }

    fn volume_text(&self, key: SetKey) -> String {
        match self.volume_percent(key) {
            0 => "Выкл".to_string(),
            p => format!("{p}%"),
        }
    }

    fn volume_change(&mut self, key: SetKey, percent: u32) {
        match key {
            SetKey::SplashSound => {
                // let the new level be heard
                if percent > 0 {
                    crate::sound::play_gain(crate::sound::Sfx::Splash, crate::sound::splash_gain(percent));
                }
                self.set("splashVolume", percent);
            }
            _ if crate::ambient::available() => self.set("ambientVolume", percent),
            _ => {}
        }
    }

    /// Left / Right on a volume row of the main menu. True when that's what happened.
    pub(super) fn volume_nav(&mut self, nav: Nav) -> bool {
        let Some(Modal::Settings { menu: SetMenu::Main, items, focus, .. }) = &self.modal else { return false };
        let (focus, key) = (*focus, items[*focus].key);
        if !matches!(key, SetKey::SplashSound | SetKey::Ambient) {
            return false;
        }
        let step = if nav == Nav::Right { 10 } else { -10 };
        let (cur, next) = (self.volume_percent(key), next_percent(self.volume_percent(key), step, false));
        if next != cur {
            self.volume_change(key, next);
            self.open_settings(SetMenu::Main, focus);
        }
        true
    }

    pub(super) fn apply_setting(&mut self, menu: SetMenu, key: SetKey, focus: usize) {
        if matches!(menu, SetMenu::Profile | SetMenu::Friend) {
            return self.social_setting(key);
        }
        if menu == SetMenu::Call {
            return self.call_setting(key, focus);
        }
        if menu == SetMenu::Group {
            return self.group_setting(key);
        }
        if menu == SetMenu::Edit {
            return self.edit_setting(key, focus);
        }
        if menu == SetMenu::Timer {
            return self.timer_setting(key, focus);
        }
        if menu == SetMenu::Search {
            return self.search_pick(focus);
        }
        if menu == SetMenu::Vpn {
            return self.vpn_choice(key);
        }
        if menu == SetMenu::Backgrounds {
            return self.bg_setting(key, focus);
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
            SetKey::Backgrounds => return self.open_settings(SetMenu::Backgrounds, 0),
            SetKey::BgMode | SetKey::BgTrailers | SetKey::BgSlide | SetKey::BgAdd | SetKey::BgRemove => {}
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
            SetKey::PadTest => {
                self.open(Modal::PadTest(Box::new(crate::padtest::PadTest::new())));
                return;
            }
            SetKey::Layout => {
                let next = cycle(self.lib.setting_str("padLayout"), &["auto", "ps", "xbox"]);
                self.set("padLayout", next);
            }
            SetKey::SplashSound | SetKey::Ambient => self.volume_change(key, next_percent(self.volume_percent(key), 10, true)),
            SetKey::TvDisplay => self.tv_setting_next(),
            SetKey::TvKeep => {}
            SetKey::VpnKino => self.vpn_setting_next(),
            SetKey::KinoVpnOff | SetKey::KinoVpnKeep | SetKey::KinoVpnAlways => self.vpn_choice(key),
            SetKey::PadApps => {
                let v = !self.lib.setting_bool("padApps", true);
                self.padmouse.on.store(v, std::sync::atomic::Ordering::Relaxed);
                self.set("padApps", v);
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
            SetKey::Version => {
                if self.updates.avail.is_some() {
                    return self.start_update();
                }
                self.check_updates(true);
            }
            SetKey::ProfileCode
            | SetKey::ProfileRename
            | SetKey::ProfileKey
            | SetKey::ProfileLogout
            | SetKey::FriendChat
            | SetKey::FriendRemove
            | SetKey::FriendCall
            | SetKey::FriendProfile
            | SetKey::SearchHit
            | SetKey::CallMute
            | SetKey::CallDeafen
            | SetKey::CallVolume
            | SetKey::CallNoise
            | SetKey::CallMic
            | SetKey::CallVad
            | SetKey::Timer30
            | SetKey::Timer60
            | SetKey::Timer90
            | SetKey::Timer120
            | SetKey::TimerCustom
            | SetKey::TimerAction
            | SetKey::TimerCancel
            | SetKey::CallAdd
            | SetKey::CallHangup
            | SetKey::GroupVoice
            | SetKey::GroupInvite
            | SetKey::GroupRename
            | SetKey::GroupLeave
            | SetKey::EditName
            | SetKey::EditPath
            | SetKey::EditBrowse
            | SetKey::EditArgs
            | SetKey::EditCategory
            | SetKey::EditCover
            | SetKey::EditIcon
            | SetKey::EditCoverReset
            | SetKey::EditBackground
            | SetKey::EditBackgroundReset => {}
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
            SetKey::TimerOpen => return self.open_settings(SetMenu::Timer, 0),
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
        if key == SetKey::GroupLeave {
            return self.group_confirmed();
        }
        if key == SetKey::TvKeep {
            return self.tv_keep();
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
            if self.last_text_target == Some(Target::Search) {
                self.modal = None;
                return;
            }
            if let Some(t @ (Target::GameName | Target::GamePath | Target::GameArgs)) = self.last_text_target {
                return self.edit_text_cancelled(t);
            }
            return self.reopen_after_text(None);
        };
        if matches!(target, Target::GroupName | Target::GroupRename) {
            self.modal = None;
            return self.group_text(target, text);
        }
        if target.is_social() {
            return self.social_text(target, text);
        }
        if target == Target::Timer {
            return self.timer_text(&text);
        }
        if target == Target::Search {
            return self.search_text(&text);
        }
        if matches!(target, Target::GameName | Target::GamePath | Target::GameArgs) {
            return self.edit_text(target, text);
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
            Some(Target::Timer) => self.open_settings(SetMenu::Timer, 4),
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
        self.ensure_public_art();
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

pub(super) fn power_sleep() {
    use windows::Win32::System::Power::SetSuspendState;
    // sleep, not hibernate
    unsafe {
        let _ = SetSuspendState(false, false, false);
    }
}

impl App {
    /// One frame of the pad test: feed it the pad, store what it measured.
    pub(super) fn pad_test_tick(&mut self) {
        let st = self.input.snapshot();
        let cal = st.as_ref().and_then(|st| self.input.calibration(&st.key).copied());
        let Some(Modal::PadTest(t)) = self.modal.as_mut() else { return };
        let Some(out) = t.tick(st, cal) else { return };
        match out {
            crate::padtest::Out::Close => {
                self.modal = None;
                return self.open_settings(SetMenu::Main, 0);
            }
            crate::padtest::Out::Save(key, axes) => {
                let mut all = self.lib.settings.get("padCal").cloned().filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
                let axes: Vec<serde_json::Value> = axes.iter().map(|a| a.to_json()).collect();
                all[key.as_str()] = serde_json::json!({ "axes": axes, "at": super::now_ms() as i64 });
                self.lib.settings.insert("padCal".into(), all);
                self.toast("Калибровка сохранена", false);
                crate::sound::play(crate::sound::Sfx::Ok);
            }
            crate::padtest::Out::Reset(key) => {
                if let Some(all) = self.lib.settings.get_mut("padCal").and_then(|v| v.as_object_mut()) {
                    all.remove(&key);
                }
                self.toast("Калибровка сброшена", false);
                crate::sound::play(crate::sound::Sfx::Back);
            }
        }
        self.save();
        self.input.set_cal(self.lib.settings.get("padCal"));
        self.padmouse.set_cal(&crate::input::parse_cal(self.lib.settings.get("padCal")));
    }
}

/// A volume after one step: 10% at a time, on the 10s. `wrap`: past the top it starts over from "off".
fn next_percent(cur: u32, step: i32, wrap: bool) -> u32 {
    let next = (cur as i32 / 10) * 10 + step;
    match (next > 100, wrap) {
        (true, true) => 0,
        _ => next.clamp(0, 100) as u32,
    }
}

#[test]
fn volume_steps() {
    assert_eq!(next_percent(30, 10, true), 40);
    assert_eq!(next_percent(100, 10, true), 0);
    assert_eq!(next_percent(100, 10, false), 100);
    assert_eq!(next_percent(0, -10, false), 0);
    assert_eq!(next_percent(35, 10, false), 40);
}
