//! Ratatui-based chat TUI. State lives in `app`, view in `render`, event loop
//! in `events`. The entry point `run_chat` manages the raw-terminal lifecycle.

mod app;
pub(crate) mod client;
mod events;
mod highlight;
mod persistence;
mod render;
mod settings;

// Re-export the public types so callers see them at `tui::chat::*` instead of
// having to reach into `tui::chat::app::*`. Matches the surface of the old
// `chat::*` module from before the carve-out. The allow is necessary because
// no callsite outside this module imports these types today; they're kept
// `pub` per spec AC8 ("preserve visibility") so future phases can consume them.
#[allow(unused_imports)]
pub use app::{AgentInfo, ChatMessage, MessageRole};

use std::io::{stdout, Stdout};

use anyhow::Result;
use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::App;
use events::run_event_loop;

pub fn run_chat() -> Result<()> {
    let mut terminal = TerminalSession::enter()?;
    let mut app = App::new()?;
    let result = run_event_loop(&mut terminal, &mut app);
    // Tear down long-lived stream sessions (claude --stream-json) before
    // we release the terminal. Covers every normal exit path — /exit,
    // double Ctrl+C, Ctrl+D, plus errors from the event loop — so a
    // closed chat doesn't leave a claude process running in the daemon.
    app.shutdown();
    terminal.restore()?;
    result
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

impl TerminalSession {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut terminal_stdout = stdout();
        if let Err(error) = execute!(
            terminal_stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        ) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        let backend = CrosstermBackend::new(terminal_stdout);
        let terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                let mut stdout = stdout();
                let _ = execute!(
                    stdout,
                    LeaveAlternateScreen,
                    DisableMouseCapture,
                    DisableBracketedPaste
                );
                return Err(error.into());
            }
        };
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste
        )?;
        self.terminal.show_cursor()?;
        self.restored = true;
        Ok(())
    }
}

impl std::ops::Deref for TerminalSession {
    type Target = Terminal<CrosstermBackend<Stdout>>;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl std::ops::DerefMut for TerminalSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
