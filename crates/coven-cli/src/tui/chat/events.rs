//! Chat TUI event loop. Reads keyboard events via crossterm and dispatches
//! to `App` methods; calls `render_ui` between events.

use std::io::Stdout;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{backend::CrosstermBackend, Terminal};

use super::app::{App, InputMode, SlashCommandResult};
use super::render::render_ui;

pub(super) fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| render_ui(f, app))?;

        // Poll with timeout for spinner animation
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    if app.show_session_overlay {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => {
                                app.show_session_overlay = false;
                            }
                            KeyCode::Char('r') => app.refresh_sessions(),
                            _ => {}
                        }
                        continue;
                    }

                    if app.input_mode == InputMode::AgentSelect {
                        match key.code {
                            KeyCode::Up if app.agent_select_index > 0 => {
                                app.agent_select_index -= 1;
                            }
                            KeyCode::Down if app.agent_select_index + 1 < app.agents.len() => {
                                app.agent_select_index += 1;
                            }
                            KeyCode::Enter => {
                                let idx = app.agent_select_index;
                                app.switch_agent_by_index(idx);
                            }
                            KeyCode::Esc => {
                                app.input_mode = InputMode::Normal;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if app.show_help {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => {
                                app.show_help = false;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            app.insert_newline();
                        }
                        KeyCode::Enter => match app.handle_input() {
                            Some(SlashCommandResult::Quit) => return Ok(()),
                            Some(SlashCommandResult::Unknown(cmd)) => {
                                app.push_system_message(&format!("Unknown command: {cmd}"));
                            }
                            _ => {}
                        },
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
                        }
                        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.show_help = !app.show_help;
                        }
                        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.delete_word_before_cursor();
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.input.clear();
                            app.cursor_pos = 0;
                        }
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.move_cursor_home();
                        }
                        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.move_cursor_end();
                        }
                        KeyCode::Char(c) => {
                            app.insert_char(c);
                        }
                        KeyCode::Backspace => {
                            app.delete_char_before_cursor();
                        }
                        KeyCode::Delete => {
                            app.delete_char_at_cursor();
                        }
                        KeyCode::Left => {
                            app.move_cursor_left();
                        }
                        KeyCode::Right => {
                            app.move_cursor_right();
                        }
                        KeyCode::Home => {
                            app.move_cursor_home();
                        }
                        KeyCode::End => {
                            app.move_cursor_end();
                        }
                        KeyCode::Up if !app.input_history.is_empty() => {
                            app.history_previous();
                        }
                        KeyCode::Down if app.history_index.is_some() => {
                            app.history_next();
                        }
                        KeyCode::PageUp => {
                            let page = terminal.size()?.height.saturating_sub(6) as usize;
                            app.scroll_offset = app.scroll_offset.saturating_sub(page);
                        }
                        KeyCode::PageDown => {
                            let page = terminal.size()?.height.saturating_sub(6) as usize;
                            app.scroll_offset = app.scroll_offset.saturating_add(page);
                            // Will be clamped during render
                        }
                        KeyCode::Esc if app.cancel_pending_cast_confirmation() => {}
                        KeyCode::Esc if !app.input.is_empty() => {
                            app.input.clear();
                            app.cursor_pos = 0;
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    use crossterm::event::MouseEventKind;
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            app.scroll_offset = app.scroll_offset.saturating_sub(3);
                        }
                        MouseEventKind::ScrollDown => {
                            app.scroll_offset = app.scroll_offset.saturating_add(3);
                        }
                        _ => {}
                    }
                }
                Event::Resize(..) => {
                    // Terminal will redraw on next loop
                }
                Event::Paste(value) => {
                    app.insert_str(&value);
                }
                _ => {}
            }
        }

        app.tick();
    }
}
