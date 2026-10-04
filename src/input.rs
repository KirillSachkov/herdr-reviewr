//! The event loop's input: crossterm's own on unix, the console in VT mode on Windows.

#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod vt;
#[cfg(windows)]
mod windows;

use std::io;

use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;

#[cfg(not(windows))]
pub(crate) use ratatui::crossterm::event::{poll, read};
#[cfg(windows)]
pub(crate) use windows::{poll, read};

/// Mouse capture, bracketed paste, and the kitty protocol where the terminal has it.
pub(crate) fn claim(kbd: bool) {
    let _ = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    if kbd {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        let _ = execute!(io::stdout(), PushKeyboardEnhancementFlags(flags));
    }
    // Last, since the mouse capture above rewrites the console mode it builds on.
    #[cfg(windows)]
    windows::claim();
}

/// Release what [`claim`] claimed, in reverse.
pub(crate) fn release(kbd: bool) {
    #[cfg(windows)]
    windows::release();
    if kbd {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
}
