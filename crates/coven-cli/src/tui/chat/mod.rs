//! Ratatui-based chat TUI. State lives in `app`, view in `render`, event loop
//! in `events`. The entry point `run_chat` manages the raw-terminal lifecycle.

mod app;
mod events;
mod render;

// Re-export the public types so callers see them at `tui::chat::*` instead of
// having to reach into `tui::chat::app::*`. Matches the surface of the old
// `chat::*` module from before the carve-out. The allow is necessary because
// no callsite outside this module imports these types today; they're kept
// `pub` per spec AC8 ("preserve visibility") so future phases can consume them.
#[allow(unused_imports)]
pub use app::{AgentInfo, ChatMessage, MessageRole};

use std::io::stdout;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::App;
use events::run_event_loop;

pub fn run_chat() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let result = run_event_loop(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}
