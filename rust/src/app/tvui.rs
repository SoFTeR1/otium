//! The "На ТВ / На монитор" tile: moves the picture and sound, and, like the screen settings of
//! Windows, undoes it by itself unless kept: the TV may be off or on another input, and then
//! nothing on screen can be pressed.

use super::App;
use crate::modal::{Modal, SetKey};
use crate::tv::{self, Display};
use crate::watch::Event;
use std::time::{Duration, Instant};

const KEEP_FOR: Duration = Duration::from_secs(15);

#[derive(Default)]
pub struct Tv {
    pub displays: Vec<Display>,
    pub busy: bool,
    /// moved to the TV and not yet kept
    pending: Option<Instant>,
    /// whether OTIUM was fullscreen before it went to the TV
    was_fullscreen: Option<bool>,
}

impl App {
    pub(super) fn tv_refresh(&mut self) {
        // a move changes what's active, and the question isn't over until it's answered
        if !self.tv.busy {
            self.tv.displays = tv::displays();
        }
    }

    /// The TV's name, when there are two screens to switch between.
    pub(super) fn tv_name(&self) -> Option<String> {
        tv::pick(&self.tv.displays, self.lib.setting_str("tvDisplay")).map(|d| d.name.clone())
    }

    /// (label, hint) of the Control Center tile, when it should be there.
    pub(super) fn tv_tile(&self) -> Option<(&'static str, String)> {
        let name = self.tv_name()?;
        Some(if self.tv.busy {
            ("Экран…", "Переключаю экран".to_string())
        } else if tv::only_on(&self.tv.displays, &name) {
            ("На монитор", "Вернуть картинку и звук как было".to_string())
        } else {
            ("На ТВ", format!("Картинка и звук на {name}"))
        })
    }

    pub(super) fn tv_toggle(&mut self) {
        let Some(name) = self.tv_name() else { return };
        let to_tv = !tv::only_on(&self.tv.displays, &name);
        self.modal = None;
        self.tv_switch(to_tv, name);
    }

    fn tv_switch(&mut self, to_tv: bool, name: String) {
        if self.tv.busy {
            return;
        }
        self.tv.busy = true;
        let bus = self.bus.clone();
        std::thread::spawn(move || {
            let res = if to_tv { tv::to_tv(&name) } else { tv::back(&name) };
            bus.send(Event::TvDone { to_tv, res });
        });
    }

    pub(super) fn on_tv_done(&mut self, to_tv: bool, res: Result<(), String>) {
        self.tv.busy = false;
        self.tv_refresh();
        if let Err(e) = res {
            return self.toast(&e, true);
        }
        if to_tv {
            self.tv.was_fullscreen.get_or_insert(self.fullscreen);
            self.tv_fullscreen(true);
            self.tv.pending = Some(Instant::now());
            self.open(Modal::Confirm {
                title: "Оставить на телевизоре?".into(),
                text: tv_question(KEEP_FOR.as_secs()),
                yes: SetKey::TvKeep,
                yes_label: "Оставить",
                focus: 1,
            });
        } else if let Some(was) = self.tv.was_fullscreen.take() {
            self.tv_fullscreen(was);
        }
    }

    fn tv_fullscreen(&mut self, on: bool) {
        self.fullscreen = on;
        self.fs_tries = 0;
        self.pending_cmd.push(egui::ViewportCommand::Fullscreen(on));
        self.refocus = true;
    }

    pub(super) fn tv_asking(&self) -> bool {
        self.tv.pending.is_some()
    }

    /// "Оставить": the move stays.
    pub(super) fn tv_keep(&mut self) {
        self.tv.pending = None;
    }

    /// Counts down the question; time up, or answered with anything else, goes back.
    pub(super) fn tick_tv(&mut self) {
        let Some(at) = self.tv.pending else { return };
        let left = KEEP_FOR.saturating_sub(at.elapsed());
        let asked = match &mut self.modal {
            Some(Modal::Confirm { yes: SetKey::TvKeep, text, .. }) => {
                *text = tv_question(left.as_secs() + 1);
                true
            }
            _ => false,
        };
        if asked && !left.is_zero() {
            return;
        }
        self.tv.pending = None;
        if asked {
            self.modal = None;
        }
        if let Some(name) = self.tv_name() {
            self.tv_switch(false, name);
        }
    }

    /// Настройки → Телевизор.
    pub(super) fn tv_setting_value(&self) -> String {
        self.tv_name().unwrap_or_else(|| "нужно два экрана".to_string())
    }

    pub(super) fn tv_setting_next(&mut self) {
        let names: Vec<String> = self.tv.displays.iter().map(|d| d.name.clone()).collect();
        if names.len() < 2 {
            return;
        }
        let cur = self.tv_name().and_then(|n| names.iter().position(|x| *x == n)).unwrap_or(0);
        self.lib.settings.insert("tvDisplay".into(), names[(cur + 1) % names.len()].clone().into());
        self.save();
    }
}

fn tv_question(secs: u64) -> String {
    format!("Если картинки нет, всё само вернётся назад через {secs} сек.")
}
