//! The Windows console reader: VT input mode, so `ConPTY` passes bracketed pastes through.

use std::io::{self, Write};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crossterm_winapi::{Console, ConsoleMode, Handle, InputRecord};
use ratatui::crossterm::Command;
use ratatui::crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};

use super::vt::VtInput;

use windows_sys::Win32::System::Console::ENABLE_VIRTUAL_TERMINAL_INPUT;

/// The console and parser state; `None` while released, so an editor's leftovers never parse.
static READER: Mutex<Option<Reader>> = Mutex::new(None);

struct Reader {
    handle: Handle,
    console: Console,
    vt: VtInput,
}

/// VT input, plus SGR mouse reports and disambiguated keys, after crossterm's own claims.
pub(crate) fn claim() {
    set_vt_input(true);
    let mut sequence = String::new();
    let _ = EnableMouseCapture.write_ansi(&mut sequence);
    let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
    let _ = PushKeyboardEnhancementFlags(flags).write_ansi(&mut sequence);
    write_out(&sequence);
}

/// Undo [`claim`], before crossterm's mouse release restores the console mode it found.
pub(crate) fn release() {
    let mut sequence = String::new();
    let _ = PopKeyboardEnhancementFlags.write_ansi(&mut sequence);
    let _ = DisableMouseCapture.write_ansi(&mut sequence);
    write_out(&sequence);
    set_vt_input(false);
    *READER.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Whether an event is ready within `timeout`, as `crossterm::event::poll` answers it.
pub(crate) fn poll(timeout: Duration) -> io::Result<bool> {
    with_reader(|reader| reader.poll(Some(Instant::now() + timeout)))
}

/// The next event, blocking until there is one, as `crossterm::event::read` answers it.
pub(crate) fn read() -> io::Result<Event> {
    with_reader(|reader| {
        loop {
            if let Some(event) = reader.vt.pop() {
                return Ok(event);
            }
            reader.poll(None)?;
        }
    })
}

fn with_reader<T>(f: impl FnOnce(&mut Reader) -> io::Result<T>) -> io::Result<T> {
    let mut guard = READER.lock().unwrap_or_else(PoisonError::into_inner);
    if guard.is_none() {
        let handle = Handle::current_in_handle()?;
        let console = Console::from(handle.clone());
        *guard = Some(Reader { handle, console, vt: VtInput::default() });
    }
    f(guard.as_mut().expect("opened above"))
}

impl Reader {
    /// Read the console until an event parses or `deadline` passes.
    fn poll(&mut self, deadline: Option<Instant>) -> io::Result<bool> {
        loop {
            // Queued input is read before an open paste's bound can close it.
            while wait_for_input(&self.handle, Some(Duration::ZERO))? {
                self.read_records()?;
            }
            self.vt.expire();
            if self.vt.has_events() {
                return Ok(true);
            }
            // Only the wait counts toward an open paste's bound, never a frame's draw.
            let started = Instant::now();
            let left = deadline.map(|deadline| deadline.saturating_duration_since(started));
            let input =
                wait_for_input(&self.handle, left.into_iter().chain(self.vt.paste_left()).min())?;
            self.vt.waited(started.elapsed());
            if input {
                self.read_records()?;
            } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                self.vt.expire();
                return Ok(self.vt.has_events());
            }
        }
    }

    fn read_records(&mut self) -> io::Result<()> {
        for record in self.console.read_console_input()? {
            match record {
                // A zero unit is a key the terminal sent no byte for, such as a bare modifier.
                InputRecord::KeyEvent(key) if key.key_down && key.u_char != 0 => {
                    self.vt.feed(key.u_char);
                }
                // The buffer size counts from zero, and crossterm adds one to match unix.
                InputRecord::WindowBufferSizeEvent(size) => self.vt.push(Event::Resize(
                    (i32::from(size.size.x) + 1) as u16,
                    (i32::from(size.size.y) + 1) as u16,
                )),
                // Mouse input arrives as SGR bytes in VT mode. reviewr never asks for focus.
                _ => {}
            }
        }
        self.vt.settle();
        Ok(())
    }
}

/// Wait until the console has input or `timeout` passes; crossterm keeps its own wait private.
#[allow(unsafe_code)]
fn wait_for_input(handle: &Handle, timeout: Option<Duration>) -> io::Result<bool> {
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

    // Rounded up, so a wait just short of its deadline sleeps instead of spinning.
    let millis = timeout.map_or(INFINITE, |timeout| {
        u32::try_from(timeout.as_nanos().div_ceil(1_000_000)).unwrap_or(INFINITE - 1)
    });
    // SAFETY: `Handle` owns the open console handle for this call, which takes no pointers.
    match unsafe { WaitForSingleObject((**handle).cast(), millis) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

fn set_vt_input(on: bool) {
    let Ok(handle) = Handle::current_in_handle() else { return };
    let mode = ConsoleMode::from(handle);
    if let Ok(before) = mode.mode() {
        let after = if on {
            before | ENABLE_VIRTUAL_TERMINAL_INPUT
        } else {
            before & !ENABLE_VIRTUAL_TERMINAL_INPUT
        };
        let set = mode.set_mode(after);
        crate::logln!("console input mode {before:#x} -> {after:#x} {set:?}");
    }
}

/// Write escape sequences crossterm would route to the console API on Windows.
fn write_out(sequence: &str) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(sequence.as_bytes());
    let _ = stdout.flush();
}
