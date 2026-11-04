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
    Options { focus: usize, game: String },
    ConfirmDelete { focus: usize, game: String },
    Message { title: String, text: String },
}

pub enum Action {
    None,
    Close,
    PickFile,
    OpenImport,
    Import(Vec<SteamGame>),
    Play,
    MakeFirst,
    AskDelete,
    Delete,
}

const ADD_ITEMS: [&str; 2] = ["Импорт игр из Steam", "Выбрать exe или ярлык…"];
const OPTION_ITEMS: [&str; 3] = ["Играть", "Сделать первой в списке", "Удалить из библиотеки"];
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
            Modal::Options { .. } => OPTION_ITEMS.len(),
            Modal::ConfirmDelete { .. } => CONFIRM_ITEMS.len(),
            Modal::Import { list, .. } => list.len() + 1,
            Modal::Message { .. } => 1,
        }
    }

    fn focus_mut(&mut self) -> Option<&mut usize> {
        match self {
            Modal::Add { focus } | Modal::Options { focus, .. } | Modal::ConfirmDelete { focus, .. } | Modal::Import { focus, .. } => {
                Some(focus)
            }
            Modal::Message { .. } => None,
        }
    }

    pub fn handle(&mut self, nav: Nav) -> Action {
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
        match self {
            Modal::Add { focus } if nav == Nav::Ok => match focus {
                0 => Action::OpenImport,
                _ => Action::PickFile,
            },
            Modal::Options { focus, .. } if nav == Nav::Ok => match focus {
                0 => Action::Play,
                1 => Action::MakeFirst,
                _ => Action::AskDelete,
            },
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
            Modal::Options { focus, game } => {
                let r = panel(ui, screen, s, 620.0, game, OPTION_ITEMS.len(), appear);
                rows(ui, r, s, &OPTION_ITEMS, *focus, appear);
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
        let danger = label.starts_with("Удалить");
        let color = match (f, danger) {
            (true, _) => Color32::from_rgb(10, 12, 18),
            (false, true) => Color32::from_rgb(255, 110, 110).gamma_multiply(a),
            (false, false) => white(a),
        };
        p.text(pos2(row.left() + 20.0 * s, row.center().y), Align2::LEFT_CENTER, *label, semibold(22.0 * s), color);
    }
}
