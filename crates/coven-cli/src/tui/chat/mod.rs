//! Ratatui-based chat TUI. State lives in `app`, view in `render`, event loop
//! in `events`. The entry point `run_chat` here manages the raw-terminal
//! lifecycle.

#![allow(dead_code)]

mod app;
mod events;
mod render;
