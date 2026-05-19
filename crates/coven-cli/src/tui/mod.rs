//! TUI surfaces for the coven CLI.

use crossterm::event::KeyEventKind;

pub(crate) mod cast;
pub mod chat;
pub(crate) mod sessions;
pub(crate) mod shell;

pub(crate) fn is_key_press(kind: KeyEventKind) -> bool {
    kind == KeyEventKind::Press
}
