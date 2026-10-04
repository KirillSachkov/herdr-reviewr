//! Where the event loop's input comes from.
//!
//! On macOS and Linux it is crossterm's own `poll` and `read`, untouched. On Windows crossterm
//! reads the classic console API, where `ConPTY` drops the bracketed-paste markers: a multi-line
//! paste arrives as keystrokes, its first newline submits the comment, and the rest runs as
//! normal-mode keys. There the console is read in VT input mode instead (`windows.rs`), and the
//! bytes parse into the same crossterm events (`vt.rs`). Both go once crossterm reads Windows
//! input that way itself (crossterm PR #1030).

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

/// Claim the input modes the reader reads. Mouse capture and bracketed paste on every OS, so a
/// multi-line paste arrives as one event, not raw keystrokes whose embedded newlines would
/// submit the comment early. The kitty keyboard protocol where the terminal reports it
/// (`kbd`), for the modifiers the legacy encoding drops, most notably Ctrl/Alt+arrows. Then
/// what the Windows reader needs beyond those.
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
