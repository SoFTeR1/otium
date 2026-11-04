//! On-screen keyboard for typing with a gamepad: RU / EN / symbols, shift, space, erase, done.
//! A physical keyboard types straight into it too.

use crate::draw::{self, font, semibold, white};
use crate::input::Nav;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    User,
    ApiKey,
    SteamId,
    DiscordState,
    Chat,
    AccountName,
    LoginKey,
    FindFriends,
    Rename,
}

impl Target {
    /// Text for the friends panel, not for the settings screens.
    pub fn is_social(self) -> bool {
        matches!(self, Target::Chat | Target::AccountName | Target::LoginKey | Target::FindFriends | Target::Rename)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Ru,
    En,
    Sym,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    Char(char),
    Shift,
    Lang,
    Sym,
    Space,
    Erase,
    Done,
}

impl Key {
    /// width in key units
    fn width(self) -> f32 {
        match self {
            Key::Char(_) => 1.0,
            Key::Space => 4.0,
            Key::Done => 2.0,
            _ => 1.5,
        }
    }
}

pub enum Outcome {
    Done(Target, String),
    Cancel,
}

pub struct Keyboard {
    pub title: String,
    pub text: String,
    pub target: Target,
    max: usize,
    secret: bool,
    layout: Layout,
    shift: bool,
    row: usize,
    col: usize,
}

const RU: [&str; 4] = ["1234567890", "йцукенгшщзхъ", "фывапролджэ", "ячсмитьбю.,"];
const EN: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm.,-"];
const SYM: [&str; 4] = ["1234567890", "-_@#$%&*()", "!?:;/+=\"'", "<>[]{}~^|\\"];

impl Keyboard {
    pub fn new(title: &str, text: &str, target: Target, max: usize, secret: bool) -> Self {
        // codes and keys are typed in latin letters; names usually in Russian
        let layout = match target {
            Target::ApiKey | Target::SteamId | Target::LoginKey => Layout::En,
            _ => Layout::Ru,
        };
        Self { title: title.into(), text: text.into(), target, max, secret, layout, shift: false, row: 1, col: 0 }
    }

    fn rows(&self) -> Vec<Vec<Key>> {
        let src = match self.layout {
            Layout::Ru => RU,
            Layout::En => EN,
            Layout::Sym => SYM,
        };
        let mut rows: Vec<Vec<Key>> = src
            .iter()
            .map(|r| {
                r.chars()
                    .map(|c| Key::Char(if self.shift { c.to_uppercase().next().unwrap_or(c) } else { c }))
                    .collect()
            })
            .collect();
        rows.push(vec![Key::Shift, Key::Lang, Key::Sym, Key::Space, Key::Erase, Key::Done]);
        rows
    }

    /// Centre of a key along the row, in key units.
    fn center(row: &[Key], col: usize) -> f32 {
        let gaps = 0.12;
        let before: f32 = row.iter().take(col).map(|k| k.width() + gaps).sum();
        let total: f32 = row.iter().map(|k| k.width() + gaps).sum::<f32>() - gaps;
        before + row[col].width() / 2.0 - total / 2.0
    }

    fn move_row(&mut self, to: usize) {
        let rows = self.rows();
        let x = Self::center(&rows[self.row], self.col.min(rows[self.row].len() - 1));
        self.row = to;
        let r = &rows[to];
        self.col = (0..r.len()).min_by(|a, b| (Self::center(r, *a) - x).abs().total_cmp(&(Self::center(r, *b) - x).abs())).unwrap_or(0);
    }

    pub fn type_text(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            if self.text.chars().count() < self.max {
                self.text.push(c);
            }
        }
    }

    pub fn erase(&mut self) {
        self.text.pop();
    }

    pub fn finish(&self) -> Outcome {
        Outcome::Done(self.target, self.text.trim().to_string())
    }

    pub fn handle(&mut self, nav: Nav) -> Option<Outcome> {
        let rows = self.rows();
        let n = rows[self.row].len();
        match nav {
            Nav::Left => self.col = if self.col == 0 { n - 1 } else { self.col - 1 },
            Nav::Right => self.col = (self.col + 1) % n,
            Nav::Up if self.row > 0 => self.move_row(self.row - 1),
            Nav::Down if self.row + 1 < rows.len() => self.move_row(self.row + 1),
            Nav::Back => return Some(Outcome::Cancel),
            Nav::Add => self.erase(),
            Nav::Options => self.type_text(" "),
            Nav::Ok => match rows[self.row][self.col.min(n - 1)] {
                Key::Char(c) => {
                    self.type_text(&c.to_string());
                    self.shift = false;
                }
                Key::Shift => self.shift = !self.shift,
                Key::Lang => self.layout = if self.layout == Layout::Ru { Layout::En } else { Layout::Ru },
                Key::Sym => self.layout = if self.layout == Layout::Sym { Layout::Ru } else { Layout::Sym },
                Key::Space => self.type_text(" "),
                Key::Erase => self.erase(),
                Key::Done => return Some(self.finish()),
            },
            _ => {}
        }
        let rows = self.rows();
        self.col = self.col.min(rows[self.row].len() - 1);
        None
    }

    pub fn paint(&self, ui: &egui::Ui, screen: Rect, s: f32, a: f32) {
        let p = ui.painter();
        let unit = 70.0 * s;
        let gap = 0.12 * unit;
        let w = 14.0 * unit;
        let rows = self.rows();
        let h = 120.0 * s + 110.0 * s + rows.len() as f32 * (unit + gap) + 30.0 * s;
        let r = Rect::from_center_size(screen.center() + vec2(0.0, (1.0 - a) * 24.0 * s), vec2(w, h));
        let cr = CornerRadius::same((28.0 * s) as u8);
        draw::glow(p, r, 28.0 * s, Color32::BLACK, 40.0 * s, 0.8 * a);
        p.rect_filled(r, cr, Color32::from_rgb(22, 26, 36).gamma_multiply(a));
        p.text(r.left_top() + vec2(36.0 * s, 34.0 * s), Align2::LEFT_TOP, &self.title, semibold(28.0 * s), white(a));

        // the text field
        let field = Rect::from_min_size(r.left_top() + vec2(36.0 * s, 92.0 * s), vec2(w - 72.0 * s, 72.0 * s));
        p.rect_filled(field, CornerRadius::same((14.0 * s) as u8), Color32::from_rgb(12, 14, 22).gamma_multiply(a));
        p.rect_stroke(field, CornerRadius::same((14.0 * s) as u8), Stroke::new(2.0 * s, Color32::from_rgb(61, 123, 255).gamma_multiply(a)), StrokeKind::Inside);
        let shown = if self.secret && !self.text.is_empty() {
            // the last 4 characters stay visible so a pasted key can be checked
            let n = self.text.chars().count();
            let tail: String = self.text.chars().skip(n.saturating_sub(4)).collect();
            format!("{}{}", "•".repeat(n.saturating_sub(4)), tail)
        } else {
            self.text.clone()
        };
        let clip = p.with_clip_rect(field.shrink(4.0));
        let galley = p.layout_no_wrap(shown, font(26.0 * s), white(a));
        let tw = galley.size().x;
        // keep the end of long text in view
        let x = (field.left() + 20.0 * s).min(field.right() - 30.0 * s - tw);
        let gy = field.center().y - galley.size().y / 2.0;
        clip.galley(pos2(x, gy), galley, Color32::WHITE);
        let blink = (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0;
        if blink {
            let cx = x + tw + 3.0 * s;
            clip.line_segment([pos2(cx, field.top() + 18.0 * s), pos2(cx, field.bottom() - 18.0 * s)], Stroke::new(2.0 * s, white(a)));
        }
        let count = format!("{}/{}", self.text.chars().count(), self.max);
        p.text(pos2(field.right(), field.bottom() + 8.0 * s), Align2::RIGHT_TOP, count, font(15.0 * s), white(0.4 * a));

        // keys
        let top = field.bottom() + 40.0 * s;
        for (ri, row) in rows.iter().enumerate() {
            let y = top + ri as f32 * (unit + gap);
            for (ci, k) in row.iter().enumerate() {
                // centre() already counts the gaps between keys, in key units
                let cx = r.center().x + Self::center(row, ci) * unit;
                let kw = k.width() * unit;
                let kr = Rect::from_center_size(pos2(cx, y + unit / 2.0), vec2(kw, unit));
                let f = ri == self.row && ci == self.col;
                let active = matches!(k, Key::Shift if self.shift) || matches!(k, Key::Sym if self.layout == Layout::Sym);
                let bg = if f {
                    Color32::WHITE
                } else if matches!(k, Key::Done) {
                    Color32::from_rgb(61, 123, 255)
                } else if active {
                    white(0.3)
                } else {
                    white(0.08)
                };
                if f {
                    draw::glow(p, kr, 12.0 * s, Color32::WHITE, 12.0 * s, 0.5 * a);
                }
                p.rect_filled(kr, CornerRadius::same((12.0 * s) as u8), bg.gamma_multiply(a));
                let label = match k {
                    Key::Char(c) => c.to_string(),
                    Key::Shift => "⇧".into(),
                    Key::Lang => if self.layout == Layout::Ru { "EN".into() } else { "RU".into() },
                    Key::Sym => if self.layout == Layout::Sym { "АБВ".into() } else { "123".into() },
                    Key::Space => "Пробел".into(),
                    Key::Erase => "⌫".into(),
                    Key::Done => "Готово".into(),
                };
                let fg = if f { Color32::from_rgb(10, 12, 18) } else { Color32::WHITE };
                let size = if matches!(k, Key::Char(_)) { 26.0 } else { 20.0 } * s;
                p.text(kr.center(), Align2::CENTER_CENTER, label, semibold(size), fg.gamma_multiply(a));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_and_moves() {
        let mut k = Keyboard::new("x", "", Target::User, 5, false);
        k.handle(Nav::Ok); // "й"
        k.handle(Nav::Right);
        k.handle(Nav::Ok); // "ц"
        assert_eq!(k.text, "йц");
        k.type_text("abcdef");
        assert_eq!(k.text.chars().count(), 5, "respects the limit");
        k.handle(Nav::Add);
        assert_eq!(k.text.chars().count(), 4);
        // down to the bottom row and over to Done
        for _ in 0..4 {
            k.handle(Nav::Down);
        }
        for _ in 0..10 {
            k.handle(Nav::Right);
            if let Some(Outcome::Done(..)) = k.handle(Nav::Ok) {
                return;
            }
            k.handle(Nav::Add);
        }
        panic!("never reached Done");
    }
}
