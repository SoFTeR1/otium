//! Mouse support: what's clickable is registered while painting (rect + what it is), and the next
//! frame turns the pointer into the same moves and presses the gamepad makes — hovering focuses,
//! clicking chooses, the wheel scrolls, the right button goes back.

use egui::Rect;
use std::cell::RefCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// a game tile in the row
    Game(usize),
    Play,
    More,
    /// an item of the top bar (update, search, settings, profile)
    Top(usize),
    /// a row of the open menu
    Row(usize),
    /// a Control Center tile
    Cc(usize),
    /// a row of the volume mixer
    Mixer(usize),
    /// a tile of the Media tab
    Media(usize),
    /// an entry of the friends list
    Social(usize),
}

thread_local! {
    static HITS: RefCell<Vec<(Rect, Hit)>> = const { RefCell::new(Vec::new()) };
}

pub fn add(r: Rect, h: Hit) {
    HITS.with(|v| v.borrow_mut().push((r, h)));
}

/// What was registered while painting the last frame (and starts the list over).
pub fn take() -> Vec<(Rect, Hit)> {
    HITS.with(|v| std::mem::take(&mut *v.borrow_mut()))
}
