//! The event loop's input: crossterm's own on unix, the console in VT mode on Windows.

#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod vt;
#[cfg(windows)]
mod windows;

use std::io;

use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
#[cfg(not(windows))]
use ratatui::crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;

#[cfg(not(windows))]
pub(crate) use ratatui::crossterm::event::{poll, read};
#[cfg(windows)]
pub(crate) use windows::{poll, read};

/// Whether the terminal speaks the kitty protocol, asked once; it reports Ctrl/Alt+arrows.
#[cfg(not(windows))]
fn kitty() -> bool {
    static KITTY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *KITTY.get_or_init(|| {
        let kitty = ratatui::crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
        crate::logln!("keyboard enhancement supported={kitty}");
        kitty
    })
}

/// Mouse capture, bracketed paste, and the kitty protocol where the terminal has it.
pub(crate) fn claim() {
    let _ = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    #[cfg(not(windows))]
    if kitty() {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        let _ = execute!(io::stdout(), PushKeyboardEnhancementFlags(flags));
    }
    // Last, since the mouse capture above rewrites the console mode it builds on.
    #[cfg(windows)]
    windows::claim();
}

/// Release what [`claim`] claimed, in reverse.
pub(crate) fn release() {
    #[cfg(windows)]
    windows::release();
    #[cfg(not(windows))]
    if kitty() {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
}
