//! The VPN in the shell: the Control Center tile, and switching it off for Кинопоиск (which refuses
//! to work through it) — asked, or done on its own (Настройки → VPN для Кинопоиска), and switched
//! back on once its window is closed.

use super::App;
use crate::modal::{Modal, SetItem, SetKey, SetMenu};
use crate::services::SERVICES;
use crate::watch::Event;
use std::path::PathBuf;

#[derive(Default)]
pub struct Vpn {
    /// v2RayTun.exe, when it is installed
    pub exe: Option<PathBuf>,
    pub on: bool,
    /// a switch was asked for and hasn't finished
    pub busy: bool,
    /// OTIUM turned it off for a service: turn it back on when that service closes
    restore: bool,
    /// the service waiting for the VPN to go off (or for the answer to the question)
    then_open: Option<usize>,
}

impl Vpn {
    pub fn new() -> Self {
        let exe = crate::vpn::find();
        let on = exe.is_some() && crate::vpn::is_on();
        Self { exe, on, ..Default::default() }
    }
}

impl App {
    pub(super) fn vpn_refresh(&mut self) {
        if self.vpn.exe.is_some() && !self.vpn.busy {
            self.vpn.on = crate::vpn::is_on();
        }
    }

    pub(super) fn vpn_switch(&mut self, on: bool) {
        let Some(exe) = self.vpn.exe.clone() else { return };
        if self.vpn.busy {
            return;
        }
        self.vpn.busy = true;
        self.toast(if on { "Включаю VPN…" } else { "Отключаю VPN…" }, false);
        let bus = self.bus.clone();
        std::thread::spawn(move || {
            let res = crate::vpn::set(&exe, on);
            bus.send(Event::VpnDone { wanted: on, res });
        });
    }

    pub(super) fn on_vpn_done(&mut self, wanted: bool, res: Result<bool, String>) {
        self.vpn.busy = false;
        match res {
            Ok(now) => {
                self.vpn.on = now;
                if now == wanted {
                    self.toast(if now { "VPN включён" } else { "VPN отключён" }, false);
                } else {
                    self.vpn.restore = false;
                    let still = if now { "всё ещё включён" } else { "всё ещё выключен" };
                    self.toast(&format!("v2RayTun не ответил: VPN {still}"), true);
                }
            }
            Err(e) => {
                self.vpn.restore = false;
                self.toast(&e, true);
            }
        }
        // the service opens either way: with the VPN still on it says so itself
        if let Some(i) = self.vpn.then_open.take() {
            self.open_service_now(i);
        }
    }

    /// Before opening service `i`: when it doesn't work through the VPN and the VPN is on, asks or
    /// switches it off first. Returns true when the opening waits for that.
    pub(super) fn vpn_before(&mut self, i: usize) -> bool {
        let Some(s) = SERVICES.get(i) else { return false };
        if !s.no_vpn || self.vpn.exe.is_none() || self.vpn.busy {
            return false;
        }
        self.vpn_refresh();
        if !self.vpn.on {
            return false;
        }
        match self.lib.setting_str("vpnKino") {
            "off" => false,
            "auto" => {
                self.vpn.restore = true;
                self.vpn.then_open = Some(i);
                self.vpn_switch(false);
                true
            }
            _ => {
                self.vpn.then_open = Some(i);
                let row = |key, label: &str, value: &str| SetItem { key, label: label.into(), value: value.into(), danger: false };
                let items = vec![
                    row(SetKey::KinoVpnOff, "Отключить VPN и открыть", "включится, когда закроете"),
                    row(SetKey::KinoVpnKeep, "Открыть с VPN", ""),
                    row(SetKey::KinoVpnAlways, "Всегда отключать автоматически", "можно поменять в Настройках"),
                ];
                self.open(Modal::Settings { menu: SetMenu::Vpn, title: format!("{} не работает через VPN", s.name), items, focus: 0, scroll: 0.0 });
                true
            }
        }
    }

    /// An answer to the question above.
    pub(super) fn vpn_choice(&mut self, key: SetKey) {
        self.modal = None;
        let Some(i) = self.vpn.then_open else { return };
        match key {
            SetKey::KinoVpnKeep => {
                self.vpn.then_open = None;
                self.open_service_now(i);
            }
            SetKey::KinoVpnAlways | SetKey::KinoVpnOff => {
                if key == SetKey::KinoVpnAlways {
                    self.lib.settings.insert("vpnKino".into(), "auto".into());
                    self.save();
                }
                self.vpn.restore = true;
                self.vpn_switch(false);
            }
            _ => {}
        }
    }

    pub(super) fn vpn_cancel_pending(&mut self) {
        self.vpn.then_open = None;
    }

    /// A service window closed: the VPN comes back if OTIUM turned it off for it.
    pub(super) fn vpn_service_closed(&mut self, id: &str) {
        if self.vpn.restore && SERVICES.iter().any(|s| s.id == id && s.no_vpn) {
            self.vpn.restore = false;
            self.vpn.then_open = None;
            self.vpn_switch(true);
        }
    }

    /// Настройки → VPN для Кинопоиска.
    pub(super) fn vpn_setting_value(&self) -> &'static str {
        if self.vpn.exe.is_none() {
            return "v2RayTun не найден";
        }
        match self.lib.setting_str("vpnKino") {
            "auto" => "Отключать автоматически",
            "off" => "Не трогать",
            _ => "Спрашивать",
        }
    }

    pub(super) fn vpn_setting_next(&mut self) {
        if self.vpn.exe.is_none() {
            return;
        }
        let next = match self.lib.setting_str("vpnKino") {
            "auto" => "off",
            "off" => "ask",
            _ => "auto",
        };
        self.lib.settings.insert("vpnKino".into(), next.into());
        self.save();
    }
}
