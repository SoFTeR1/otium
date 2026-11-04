//! Pop-up menus driven by the gamepad: Add game, Steam import, game options, delete confirmation.

use crate::data::image_uri;
use crate::draw::{self, black, bold, font, semibold, white};
use crate::input::Nav;
use crate::steam::SteamGame;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind};
use std::collections::HashSet;

pub enum Modal {
    Add { focus: usize },
    Import { list: Vec<SteamGame>, picked: HashSet<String>, focus: usize, scroll: f32 },
    Options { focus: usize, game: String, items: Vec<(Opt, String)> },
    /// the Guide-button menu over a running game
    Control { focus: usize, items: Vec<(CtlAction, &'static str)> },
    ConfirmDelete { focus: usize, game: String },
    Message { title: String, text: String },
    News { news: crate::steam_api::News, game: String, fallback: Vec<String>, scroll: f32 },
    Trophies { title: String, items: Vec<crate::steam_api::Achievement>, focus: usize, scroll: f32 },
    /// a settings screen: rows of "label ... value"
    Settings { menu: SetMenu, title: String, items: Vec<SetItem>, focus: usize, scroll: f32 },
    Keyboard(Box<crate::keyboard::Keyboard>),
    PadTest(Box<crate::padtest::PadTest>),
    Confirm { title: String, text: String, yes: SetKey, yes_label: &'static str, focus: usize },
    /// "find friends" results: (public key, name, state)
    Players { items: Vec<(String, String, String)>, focus: usize, scroll: f32 },
    FriendRequest { pk: String, name: String, focus: usize },
    /// pick friends (for a group or a call): (public key, name, picked); the last row is "Готово"
    Pick { title: String, items: Vec<(String, String, bool)>, focus: usize, scroll: f32, max: usize, purpose: crate::app::PickFor },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SetMenu {
    Main,
    Account,
    Discord,
    Power,
    Profile,
    Friend,
    Call,
    Group,
    Edit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SetKey {
    User,
    ImportSteam,
    Account,
    Discord,
    Sort,
    Layout,
    Sounds,
    Splash,
    Fullscreen,
    Autostart,
    Tray,
    Power,
    Version,
    ApiKey,
    SteamId,
    GetKey,
    DiscordOn,
    DiscordGame,
    DiscordTimer,
    DiscordState,
    Sleep,
    Restart,
    Shutdown,
    Quit,
    ProfileCode,
    ProfileRename,
    ProfileKey,
    ProfileLogout,
    FriendChat,
    FriendRemove,
    FriendCall,
    CallMute,
    CallDeafen,
    CallVolume,
    CallNoise,
    CallMic,
    CallAdd,
    CallHangup,
    GroupVoice,
    GroupInvite,
    GroupRename,
    GroupLeave,
    EditName,
    EditPath,
    EditBrowse,
    EditArgs,
    EditCategory,
    EditCover,
    EditIcon,
    EditCoverReset,
    EditBackground,
    EditBackgroundReset,
    PadTest,
}

#[derive(Clone, Debug)]
pub struct SetItem {
    pub key: SetKey,
    pub label: String,
    pub value: String,
    pub danger: bool,
}

/// The game's "…" menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opt {
    Play,
    CloseGame,
    Edit,
    MakeFirst,
    Reorder,
    SetBackground,
    ResetBackground,
    ShowInFolder,
    Delete,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CtlAction {
    ReturnToGame,
    CloseGame,
    Home,
    Settings,
    Power,
    Social,
    CallMenu,
    AcceptCall,
}

pub enum Action {
    None,
    Control(CtlAction),
    Close,
    PickFile,
    OpenImport,
    Import(Vec<SteamGame>),
    Option(Opt),
    Delete,
    OpenUrl(String),
    Setting(SetMenu, SetKey, usize),
    Text(crate::keyboard::Outcome),
    Confirmed(SetKey),
    RequestFriend(String, String),
    AnswerRequest(String, bool),
    Picked(crate::app::PickFor, Vec<String>),
}

const ADD_ITEMS: [&str; 2] = ["Импорт игр из Steam", "Выбрать exe или ярлык…"];
const CONFIRM_ITEMS: [&str; 2] = ["Отмена", "Удалить"];
const IMPORT_VISIBLE: usize = 7;

impl Modal {
    pub fn import(list: Vec<SteamGame>) -> Self {
        let picked = list.iter().map(|g| g.app_id.clone()).collect();
        Modal::Import { list, picked, focus: 0, scroll: 0.0 }
    }

    fn item_count(&self) -> usize {
        match self {
            Modal::Add { .. } => ADD_ITEMS.len(),
            Modal::Options { items, .. } => items.len(),
            Modal::Control { items, .. } => items.len(),
            Modal::ConfirmDelete { .. } => CONFIRM_ITEMS.len(),
            Modal::Import { list, .. } => list.len() + 1,
            Modal::Message { .. } | Modal::News { .. } => 1,
            Modal::Trophies { items, .. } => items.len(),
            Modal::Settings { items, .. } => items.len(),
            Modal::Confirm { .. } | Modal::FriendRequest { .. } => 2,
            Modal::Players { items, .. } => items.len(),
            Modal::Pick { items, .. } => items.len() + 1,
            Modal::Keyboard(_) | Modal::PadTest(_) => 0,
        }
    }

    fn focus_mut(&mut self) -> Option<&mut usize> {
        match self {
            Modal::Add { focus }
            | Modal::Options { focus, .. }
            | Modal::ConfirmDelete { focus, .. }
            | Modal::Import { focus, .. }
            | Modal::Control { focus, .. }
            | Modal::Trophies { focus, .. }
            | Modal::Settings { focus, .. }
            | Modal::Confirm { focus, .. }
            | Modal::Players { focus, .. }
            | Modal::Pick { focus, .. }
            | Modal::FriendRequest { focus, .. } => Some(focus),
            Modal::Message { .. } | Modal::News { .. } | Modal::Keyboard(_) | Modal::PadTest(_) => None,
        }
    }

    pub fn handle(&mut self, nav: Nav) -> Action {
        if let Modal::Keyboard(k) = self {
            return k.handle(nav).map_or(Action::None, Action::Text);
        }
        // only the keyboard gets here: the pad's buttons are being tested
        if let Modal::PadTest(t) = self {
            match nav {
                Nav::Back => return Action::Close,
                Nav::Ok => t.start(),
                Nav::Options | Nav::Add => t.reset(),
                _ => {}
            }
            return Action::None;
        }
        let n = self.item_count();
        if let Some(f) = self.focus_mut() {
            match nav {
                Nav::Up if *f > 0 => {
                    *f -= 1;
                    return Action::None;
                }
                Nav::Down if *f + 1 < n => {
                    *f += 1;
                    return Action::None;
                }
                _ => {}
            }
        }
        if nav == Nav::Back {
            return Action::Close;
        }
        if let Modal::News { news, scroll, .. } = self {
            return match nav {
                Nav::Up => {
                    *scroll = (*scroll - 160.0).max(0.0);
                    Action::None
                }
                Nav::Down => {
                    *scroll += 160.0;
                    Action::None
                }
                Nav::Ok if !news.url.is_empty() => Action::OpenUrl(news.url.clone()),
                _ => Action::None,
            };
        }
        match self {
            Modal::Add { focus } if nav == Nav::Ok => match focus {
                0 => Action::OpenImport,
                _ => Action::PickFile,
            },
            Modal::Control { focus, items } if nav == Nav::Ok => Action::Control(items[*focus].0),
            Modal::Settings { menu, items, focus, .. } if nav == Nav::Ok => Action::Setting(*menu, items[*focus].key, *focus),
            Modal::Players { items, focus, .. } if nav == Nav::Ok => {
                let (pk, name, _) = items[*focus].clone();
                Action::RequestFriend(pk, name)
            }
            Modal::FriendRequest { pk, focus, .. } if nav == Nav::Ok => Action::AnswerRequest(pk.clone(), *focus == 1),
            Modal::Pick { items, focus, max, purpose, .. } if nav == Nav::Ok => {
                if *focus < items.len() {
                    let count = items.iter().filter(|i| i.2).count();
                    let it = &mut items[*focus];
                    if it.2 || count < *max {
                        it.2 = !it.2;
                    }
                    Action::None
                } else {
                    Action::Picked(purpose.clone(), items.iter().filter(|i| i.2).map(|i| i.0.clone()).collect())
                }
            }
            Modal::Confirm { yes, focus, .. } if nav == Nav::Ok => {
                if *focus == 1 {
                    Action::Confirmed(*yes)
                } else {
                    Action::Close
                }
            }
            Modal::Options { focus, items, .. } if nav == Nav::Ok => Action::Option(items[*focus].0),
            Modal::ConfirmDelete { focus, .. } if nav == Nav::Ok => {
                if *focus == 1 {
                    Action::Delete
                } else {
                    Action::Close
                }
            }
            Modal::Import { list, picked, focus, .. } => match nav {
                Nav::Ok if *focus < list.len() => {
                    let id = &list[*focus].app_id;
                    if !picked.remove(id) {
                        picked.insert(id.clone());
                    }
                    Action::None
                }
                Nav::Ok => Action::Import(list.iter().filter(|g| picked.contains(&g.app_id)).cloned().collect()),
                // Y / Start: pick all, or none if everything is already picked
                Nav::Options => {
                    if picked.len() == list.len() {
                        picked.clear();
                    } else {
                        *picked = list.iter().map(|g| g.app_id.clone()).collect();
                    }
                    Action::None
                }
                _ => Action::None,
            },
            Modal::Message { .. } if nav == Nav::Ok => Action::Close,
            _ => Action::None,
        }
    }

    /// Glyph labels shown in the hint bar while this menu is open.
    pub fn hints(&self) -> Vec<(usize, &'static str)> {
        // 0 = confirm, 1 = back, 2 = options
        match self {
            Modal::Import { focus, list, .. } if *focus < list.len() => vec![(0, "Отметить"), (2, "Выбрать все"), (1, "Назад")],
            Modal::Import { .. } => vec![(0, "Импортировать"), (2, "Выбрать все"), (1, "Назад")],
            Modal::Message { .. } => vec![(0, "OK")],
            Modal::News { .. } => vec![(0, "Открыть в браузере"), (1, "Закрыть")],
            Modal::Trophies { .. } => vec![(1, "Закрыть")],
            Modal::Keyboard(_) => vec![(0, "Клавиша"), (3, "Стереть"), (2, "Пробел"), (1, "Отмена")],
            Modal::PadTest(_) => vec![],
            _ => vec![(0, "Выбрать"), (1, "Назад")],
        }
    }

    pub fn paint(&mut self, ui: &egui::Ui, screen: Rect, s: f32, dt: f32, appear: f32) {
        let p = ui.painter();
        p.rect_filled(screen, 0.0, black(0.62 * appear));

        match self {
            Modal::Add { focus } => {
                let r = panel(ui, screen, s, 620.0, "Добавить", ADD_ITEMS.len(), appear);
                rows(ui, r, s, &ADD_ITEMS, *focus, appear);
            }
            Modal::Options { focus, game, items } => {
                let labels: Vec<&str> = items.iter().map(|(_, l)| l.as_str()).collect();
                let r = panel(ui, screen, s, 620.0, game, labels.len(), appear);
                rows(ui, r, s, &labels, *focus, appear);
            }
            Modal::Keyboard(k) => k.paint(ui, screen, s, appear),
            Modal::PadTest(t) => t.paint(ui, screen, s, appear),
            Modal::Pick { title, items, focus, scroll, max, .. } => {
                let visible = 7.min(items.len());
                let r = panel(ui, screen, s, 760.0, title, visible + 1, appear);
                let target = (*focus as f32 - (visible as f32 - 2.0)).clamp(0.0, items.len().saturating_sub(visible) as f32);
                *scroll += (target - *scroll) * (1.0 - (-dt * 18.0).exp());
                let list = Rect::from_min_size(r.min, vec2(r.width(), visible as f32 * ROW * s));
                let clip = ui.painter().with_clip_rect(list.expand(2.0));
                for (i, (pk, name, on)) in items.iter().enumerate() {
                    let y = list.top() + (i as f32 - *scroll) * ROW * s;
                    if y + ROW * s < list.top() || y > list.bottom() {
                        continue;
                    }
                    let row = Rect::from_min_size(pos2(r.left(), y), vec2(r.width(), ROW * s - 6.0 * s));
                    let f = i == *focus;
                    if f {
                        clip.rect_filled(row, CornerRadius::same((12.0 * s) as u8), white(appear));
                    }
                    let fg = if f { Color32::from_rgb(10, 12, 18) } else { white(appear) };
                    let cb = Rect::from_center_size(pos2(row.left() + 30.0 * s, row.center().y), vec2(26.0 * s, 26.0 * s));
                    if *on {
                        clip.rect_filled(cb, CornerRadius::same((6.0 * s) as u8), Color32::from_rgb(61, 123, 255));
                        clip.text(cb.center(), Align2::CENTER_CENTER, "✓", bold(19.0 * s), Color32::WHITE);
                    } else {
                        clip.rect_stroke(cb, CornerRadius::same((6.0 * s) as u8), Stroke::new(2.0 * s, fg.gamma_multiply(0.6)), StrokeKind::Inside);
                    }
                    let av = pos2(cb.right() + 36.0 * s, row.center().y);
                    clip.circle_filled(av, 18.0 * s, draw::name_color(pk));
                    clip.text(av, Align2::CENTER_CENTER, name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), semibold(16.0 * s), Color32::WHITE);
                    clip.text(pos2(av.x + 32.0 * s, row.center().y), Align2::LEFT_CENTER, name, semibold(21.0 * s), fg);
                }
                let picked = items.iter().filter(|i| i.2).count();
                let btn = Rect::from_min_size(pos2(r.left(), list.bottom() + 10.0 * s), vec2(r.width(), ROW * s - 6.0 * s));
                let f = *focus == items.len();
                let p = ui.painter();
                p.rect_filled(btn, CornerRadius::same((30.0 * s) as u8), if f { white(appear) } else { Color32::from_rgb(61, 123, 255).gamma_multiply(appear) });
                p.text(btn.center(), Align2::CENTER_CENTER, format!("Готово ({picked} из {max})"), semibold(22.0 * s), if f { Color32::from_rgb(10, 12, 18) } else { Color32::WHITE });
            }
            Modal::FriendRequest { name, focus, .. } => {
                let r = panel(ui, screen, s, 620.0, &format!("{name} хочет добавить вас в друзья"), 2, appear);
                rows(ui, r, s, &["Отклонить", "Принять"], *focus, appear);
            }
            Modal::Players { items, focus, scroll } => {
                let visible = 7.min(items.len().max(1));
                let r = panel(ui, screen, s, 760.0, "Найти друзей", visible, appear);
                let target = (*focus as f32 - (visible as f32 - 2.0)).clamp(0.0, items.len().saturating_sub(visible) as f32);
                *scroll += (target - *scroll) * (1.0 - (-dt * 18.0).exp());
                let clip = ui.painter().with_clip_rect(r.expand(2.0));
                for (i, (pk, name, state)) in items.iter().enumerate() {
                    let y = r.top() + (i as f32 - *scroll) * ROW * s;
                    if y + ROW * s < r.top() || y > r.bottom() {
                        continue;
                    }
                    let row = Rect::from_min_size(pos2(r.left(), y), vec2(r.width(), ROW * s - 6.0 * s));
                    let f = i == *focus;
                    if f {
                        clip.rect_filled(row, CornerRadius::same((12.0 * s) as u8), white(appear));
                    }
                    let fg = if f { Color32::from_rgb(10, 12, 18) } else { white(appear) };
                    let av = pos2(row.left() + 32.0 * s, row.center().y);
                    clip.circle_filled(av, 20.0 * s, draw::name_color(pk));
                    let letter = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                    clip.text(av, Align2::CENTER_CENTER, letter, semibold(18.0 * s), Color32::WHITE);
                    clip.text(pos2(row.left() + 66.0 * s, row.top() + 10.0 * s), Align2::LEFT_TOP, name, semibold(20.0 * s), fg);
                    clip.text(pos2(row.left() + 66.0 * s, row.bottom() - 8.0 * s), Align2::LEFT_BOTTOM, format!("код {}", crate::nostr::short_code(pk)), font(15.0 * s), fg.gamma_multiply(0.6));
                    clip.text(pos2(row.right() - 18.0 * s, row.center().y), Align2::RIGHT_CENTER, state, font(17.0 * s), fg.gamma_multiply(0.7));
                }
            }
            Modal::Confirm { title, text, yes_label, focus, .. } => {
                let r = panel(ui, screen, s, 620.0, title, 3, appear);
                let galley = ui.painter().layout(text.clone(), font(19.0 * s), white(0.65 * appear), r.width() - 16.0 * s);
                ui.painter().galley(r.left_top() + vec2(8.0 * s, 0.0), galley, Color32::WHITE);
                let r = Rect::from_min_max(r.min + vec2(0.0, ROW * s), r.max);
                rows(ui, r, s, &["Отмена", yes_label], *focus, appear);
            }
            Modal::Settings { title, items, focus, scroll, .. } => {
                let visible = 9.min(items.len());
                let r = panel(ui, screen, s, 860.0, title, visible, appear);
                let target = (*focus as f32 - (visible as f32 - 2.0)).clamp(0.0, items.len().saturating_sub(visible) as f32);
                *scroll += (target - *scroll) * (1.0 - (-dt * 18.0).exp());
                let clip = ui.painter().with_clip_rect(r.expand(2.0));
                for (i, it) in items.iter().enumerate() {
                    let y = r.top() + (i as f32 - *scroll) * ROW * s;
                    if y + ROW * s < r.top() || y > r.bottom() {
                        continue;
                    }
                    let row = Rect::from_min_size(pos2(r.left(), y), vec2(r.width(), ROW * s - 6.0 * s));
                    let f = i == *focus;
                    if f {
                        clip.rect_filled(row, CornerRadius::same((12.0 * s) as u8), white(appear));
                    }
                    let fg = match (f, it.danger) {
                        (true, _) => Color32::from_rgb(10, 12, 18),
                        (false, true) => Color32::from_rgb(255, 110, 110).gamma_multiply(appear),
                        (false, false) => white(appear),
                    };
                    clip.text(pos2(row.left() + 20.0 * s, row.center().y), Align2::LEFT_CENTER, &it.label, semibold(21.0 * s), fg);
                    clip.text(pos2(row.right() - 20.0 * s, row.center().y), Align2::RIGHT_CENTER, &it.value, font(19.0 * s), fg.gamma_multiply(0.65));
                }
            }
            Modal::Control { focus, items } => {
                let labels: Vec<&str> = items.iter().map(|(_, l)| *l).collect();
                let r = panel(ui, screen, s, 620.0, "Центр управления", labels.len(), appear);
                rows(ui, r, s, &labels, *focus, appear);
            }
            Modal::ConfirmDelete { focus, game } => {
                let r = panel(ui, screen, s, 620.0, &format!("Удалить «{game}»?"), CONFIRM_ITEMS.len() + 1, appear);
                let p = ui.painter();
                p.text(
                    r.left_top() + vec2(8.0 * s, 0.0),
                    Align2::LEFT_TOP,
                    "Игра пропадёт только из OTIUM, файлы останутся",
                    font(19.0 * s),
                    white(0.65 * appear),
                );
                let r = Rect::from_min_max(r.min + vec2(0.0, ROW * s), r.max);
                rows(ui, r, s, &CONFIRM_ITEMS, *focus, appear);
            }
            Modal::Message { title, text } => {
                let r = panel(ui, screen, s, 620.0, title, 2, appear);
                let galley = ui.painter().layout(text.clone(), font(20.0 * s), white(0.8 * appear), r.width() - 16.0 * s);
                ui.painter().galley(r.left_top() + vec2(8.0 * s, 0.0), galley, Color32::WHITE);
                let ok = Rect::from_min_size(pos2(r.left(), r.bottom() - ROW * s), vec2(r.width(), ROW * s));
                rows(ui, ok, s, &["OK"], 0, appear);
            }
            Modal::News { news, game, fallback, scroll } => paint_news(ui, screen, s, appear, news, game, fallback, scroll),
            Modal::Trophies { title, items, focus, scroll } => {
                let visible = 7.min(items.len().max(1));
                let r = panel(ui, screen, s, 900.0, title, visible, appear);
                let target = (*focus as f32 - (visible as f32 - 2.0)).clamp(0.0, items.len().saturating_sub(visible) as f32);
                *scroll += (target - *scroll) * (1.0 - (-dt * 18.0).exp());
                let clip = ui.painter().with_clip_rect(r);
                for (i, a) in items.iter().enumerate() {
                    let y = r.top() + (i as f32 - *scroll) * ROW * s;
                    if y + ROW * s < r.top() || y > r.bottom() {
                        continue;
                    }
                    let row = Rect::from_min_size(pos2(r.left(), y), vec2(r.width(), ROW * s - 6.0 * s));
                    let f = i == *focus;
                    if f {
                        clip.rect_filled(row, CornerRadius::same((12.0 * s) as u8), white(appear));
                    }
                    let dim = if a.achieved { 1.0 } else { 0.5 };
                    let fg = if f { Color32::from_rgb(10, 12, 18) } else { white(dim * appear) };
                    let side = row.height() - 12.0 * s;
                    let icon = Rect::from_min_size(pos2(row.left() + 10.0 * s, row.top() + 6.0 * s), vec2(side, side));
                    if r.contains_rect(icon) {
                        let uris: Vec<String> = a.icon.iter().cloned().collect();
                        draw::cover(ui, &uris, icon, 6.0 * s, white(appear), (0.5, 0.5));
                    }
                    let tx = icon.right() + 16.0 * s;
                    clip.text(pos2(tx, row.top() + 8.0 * s), Align2::LEFT_TOP, &a.name, semibold(19.0 * s), fg);
                    let desc: String = a.desc.chars().take(90).collect();
                    clip.text(pos2(tx, row.bottom() - 8.0 * s), Align2::LEFT_BOTTOM, desc, font(15.0 * s), fg.gamma_multiply(0.7));
                    let val = if a.achieved {
                        crate::app::fmt_date(a.time)
                    } else {
                        a.percent.map(|p| format!("{p:.1}%")).unwrap_or_default()
                    };
                    clip.text(pos2(row.right() - 14.0 * s, row.center().y), Align2::RIGHT_CENTER, val, font(16.0 * s), fg.gamma_multiply(0.8));
                }
            }
            Modal::Import { list, picked, focus, scroll } => {
                let visible = IMPORT_VISIBLE.min(list.len());
                let title = format!("Игры Steam · {}", list.len());
                let r = panel(ui, screen, s, 820.0, &title, visible + 1, appear);
                // keep the focused row in view
                let target = (*focus as f32 - (IMPORT_VISIBLE as f32 - 2.0)).clamp(0.0, list.len().saturating_sub(IMPORT_VISIBLE) as f32);
                *scroll += (target - *scroll) * (1.0 - (-dt * 18.0).exp());
                let list_rect = Rect::from_min_size(r.min, vec2(r.width(), visible as f32 * ROW * s));
                let clip = ui.painter().with_clip_rect(list_rect);
                for (i, g) in list.iter().enumerate() {
                    let y = list_rect.top() + (i as f32 - *scroll) * ROW * s;
                    if y + ROW * s < list_rect.top() || y > list_rect.bottom() {
                        continue;
                    }
                    let row = Rect::from_min_size(pos2(r.left(), y), vec2(r.width(), ROW * s - 6.0 * s));
                    let f = i == *focus;
                    let on = picked.contains(&g.app_id);
                    if f {
                        clip.rect_filled(row, CornerRadius::same((12.0 * s) as u8), white(appear));
                    }
                    let fg = if f { Color32::from_rgb(10, 12, 18) } else { white(appear) };
                    // checkbox
                    let cb = Rect::from_center_size(pos2(row.left() + 30.0 * s, row.center().y), vec2(26.0 * s, 26.0 * s));
                    let blue = Color32::from_rgb(61, 123, 255);
                    if on {
                        clip.rect_filled(cb, CornerRadius::same((6.0 * s) as u8), blue);
                        clip.text(cb.center(), Align2::CENTER_CENTER, "✓", bold(19.0 * s), Color32::WHITE);
                    } else {
                        clip.rect_stroke(cb, CornerRadius::same((6.0 * s) as u8), Stroke::new(2.0 * s, fg.gamma_multiply(0.6)), StrokeKind::Inside);
                    }
                    // thumbnail: clip-aware painting is done by shrinking to the visible part
                    let thumb = Rect::from_min_size(pos2(cb.right() + 16.0 * s, row.top() + 5.0 * s), vec2(row.height() - 10.0 * s, row.height() - 10.0 * s));
                    if list_rect.contains_rect(thumb) {
                        let uris: Vec<String> = image_uri(&g.cover).into_iter().collect();
                        if !draw::cover(ui, &uris, thumb, 8.0 * s, white(appear), (0.5, 0.3)) {
                            clip.rect_filled(thumb, CornerRadius::same((8.0 * s) as u8), draw::name_color(&g.name));
                        }
                    }
                    clip.text(pos2(thumb.right() + 18.0 * s, row.center().y), Align2::LEFT_CENTER, &g.name, semibold(22.0 * s), fg);
                }
                let btn = Rect::from_min_size(pos2(r.left(), list_rect.bottom() + 10.0 * s), vec2(r.width(), ROW * s - 6.0 * s));
                let f = *focus == list.len();
                let p = ui.painter();
                p.rect_filled(btn, CornerRadius::same((30.0 * s) as u8), if f { white(appear) } else { Color32::from_rgb(61, 123, 255).gamma_multiply(appear) });
                p.text(
                    btn.center(),
                    Align2::CENTER_CENTER,
                    format!("Импортировать ({})", picked.len()),
                    semibold(22.0 * s),
                    if f { Color32::from_rgb(10, 12, 18) } else { Color32::WHITE },
                );
            }
        }
    }
}

const ROW: f32 = 64.0;

/// Draws the panel with its title and returns the area left for rows.
fn panel(ui: &egui::Ui, screen: Rect, s: f32, width: f32, title: &str, rows: usize, a: f32) -> Rect {
    let pad = 28.0 * s;
    let title_h = 64.0 * s;
    let h = pad * 2.0 + title_h + rows as f32 * ROW * s + 10.0 * s;
    let r = Rect::from_center_size(screen.center() + vec2(0.0, (1.0 - a) * 24.0 * s), vec2(width * s, h));
    let p = ui.painter();
    draw::glow(p, r, 28.0 * s, Color32::BLACK, 40.0 * s, 0.8 * a);
    p.rect_filled(r, CornerRadius::same((28.0 * s) as u8), Color32::from_rgb(22, 26, 36).gamma_multiply(a));
    p.rect_stroke(r, CornerRadius::same((28.0 * s) as u8), Stroke::new(1.0_f32, white(0.08 * a)), StrokeKind::Inside);
    let title_pos = pos2(r.left() + pad + 8.0 * s, r.top() + pad + title_h / 2.0 - 6.0 * s);
    p.with_clip_rect(r.shrink(pad)).text(title_pos, Align2::LEFT_CENTER, title, semibold(30.0 * s), white(a));
    Rect::from_min_max(pos2(r.left() + pad, r.top() + pad + title_h), pos2(r.right() - pad, r.bottom() - pad))
}

fn rows(ui: &egui::Ui, r: Rect, s: f32, items: &[&str], focus: usize, a: f32) {
    let p = ui.painter();
    for (i, label) in items.iter().enumerate() {
        let row = Rect::from_min_size(pos2(r.left(), r.top() + i as f32 * ROW * s), vec2(r.width(), ROW * s - 6.0 * s));
        let f = i == focus;
        if f {
            p.rect_filled(row, CornerRadius::same((12.0 * s) as u8), white(a));
        }
        let danger = label.starts_with("Удалить") || label.starts_with("Закрыть");
        let color = match (f, danger) {
            (true, _) => Color32::from_rgb(10, 12, 18),
            (false, true) => Color32::from_rgb(255, 110, 110).gamma_multiply(a),
            (false, false) => white(a),
        };
        p.text(pos2(row.left() + 20.0 * s, row.center().y), Align2::LEFT_CENTER, *label, semibold(22.0 * s), color);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_news(ui: &egui::Ui, screen: Rect, s: f32, a: f32, n: &crate::steam_api::News, game: &str, fallback: &[String], scroll: &mut f32) {
    let w = 1100.0 * s;
    let r = Rect::from_center_size(screen.center() + vec2(0.0, (1.0 - a) * 24.0 * s), vec2(w, screen.height() - 120.0 * s));
    let p = ui.painter();
    let cr = CornerRadius::same((24.0 * s) as u8);
    draw::glow(p, r, 24.0 * s, Color32::BLACK, 40.0 * s, 0.8 * a);
    p.rect_filled(r, cr, Color32::from_rgb(22, 26, 36).gamma_multiply(a));
    let hero = Rect::from_min_size(r.min, vec2(w, 300.0 * s));
    let mut uris: Vec<String> = n.image.iter().cloned().collect();
    uris.extend(fallback.iter().cloned());
    draw::cover(ui, &uris, hero, 24.0 * s, white(a), (0.5, 0.3));
    let bg = Color32::from_rgb(22, 26, 36).gamma_multiply(a);
    draw::vgradient(p, Rect::from_min_max(pos2(hero.left(), hero.bottom() - 120.0 * s), hero.max), Color32::TRANSPARENT, bg);
    let pad = 40.0 * s;
    let meta = [game.to_string(), n.feed.clone(), crate::app::fmt_date(n.date)]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    p.text(pos2(r.left() + pad, hero.bottom() + 10.0 * s), Align2::LEFT_TOP, meta, font(18.0 * s), white(0.6 * a));
    let title = p.layout(n.title.clone(), bold(32.0 * s), white(a), w - pad * 2.0);
    let ty = hero.bottom() + 40.0 * s;
    let title_h = title.size().y;
    p.galley(pos2(r.left() + pad, ty), title, Color32::WHITE);
    let body_top = ty + title_h + 20.0 * s;
    let body = Rect::from_min_max(pos2(r.left() + pad, body_top), pos2(r.right() - pad, r.bottom() - pad));
    let text = if n.text.is_empty() { "Текст новости недоступен — откройте её в браузере.".to_string() } else { n.text.clone() };
    let galley = p.layout(text, font(20.0 * s), white(0.85 * a), body.width());
    let max = (galley.size().y - body.height()).max(0.0);
    *scroll = scroll.min(max);
    p.with_clip_rect(body).galley(body.min - vec2(0.0, *scroll), galley, Color32::WHITE);
}
