//! The console's VT byte stream parsed into crossterm's unix events, via `terminput`.

use std::collections::VecDeque;
use std::time::Duration;

use ratatui::crossterm::event::Event;

/// How long an open paste may wait for a byte before it closes as a paste of what arrived.
pub(super) const PASTE_IDLE: Duration = Duration::from_millis(500);

const PASTE_START: &[u8] = crate::herdr::PASTE_START.as_bytes();

/// The parser state between console reads.
#[derive(Debug, Default)]
pub(super) struct VtInput {
    /// The bytes of a sequence that has started but not finished.
    pending: Vec<u8>,
    /// A high surrogate whose low half is still to come.
    surrogate: Option<u16>,
    /// Parsed events, oldest first.
    events: VecDeque<Event>,
    /// Console wait since the last input, which an open paste's bound counts.
    waited: Duration,
}

impl VtInput {
    /// Take one UTF-16 unit from a key record.
    pub(super) fn feed(&mut self, unit: u16) {
        let ch = match unit {
            0xD800..=0xDBFF => {
                self.surrogate = Some(unit);
                return;
            }
            0xDC00..=0xDFFF => {
                let Some(high) = self.surrogate.take() else { return };
                char::decode_utf16([high, unit]).next().and_then(Result::ok)
            }
            _ => {
                self.surrogate = None;
                char::from_u32(u32::from(unit))
            }
        };
        let mut utf8 = [0; 4];
        for &byte in ch.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut utf8).as_bytes() {
            self.pending.push(byte);
            self.parse(true);
        }
    }

    /// End of one console read: a lone ESC with nothing queued is the Esc key.
    pub(super) fn settle(&mut self) {
        self.parse(false);
        self.waited = Duration::ZERO;
    }

    /// Count `time` spent waiting on the console.
    pub(super) fn waited(&mut self, time: Duration) {
        self.waited += time;
    }

    /// The wait an open paste has left before it counts as complete, if one is open.
    pub(super) fn paste_left(&self) -> Option<Duration> {
        self.pending.starts_with(PASTE_START).then(|| PASTE_IDLE.saturating_sub(self.waited))
    }

    /// Close a paste past its bound as a paste, never as keys.
    pub(super) fn expire(&mut self) {
        if self.paste_left() == Some(Duration::ZERO) {
            let text = String::from_utf8_lossy(&self.pending[PASTE_START.len()..]).into_owned();
            self.pending.clear();
            self.events.push_back(Event::Paste(text));
        }
    }

    pub(super) fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    pub(super) fn pop(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Queue an event that did not come through the byte stream: a resize.
    pub(super) fn push(&mut self, event: Event) {
        self.events.push_back(event);
    }

    /// Try the pending bytes as one event, the step crossterm's unix reader runs per byte.
    fn parse(&mut self, more: bool) {
        // `parse_from` would read a lone ESC as Esc, so wait while more is queued.
        if more && self.pending == b"\x1b" {
            return;
        }
        match terminput::Event::parse_from(&self.pending) {
            Ok(Some(event)) => {
                // crossterm drops what it has no type for, and so does this.
                if let Ok(event) = terminput_crossterm::to_crossterm(event) {
                    self.events.push_back(event);
                }
                self.pending.clear();
            }
            Ok(None) => {}
            Err(_) => self.pending.clear(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    /// Feed each batch as one console read and collect every event, in order.
    fn events(vt: &mut VtInput, batches: &[&str]) -> Vec<Event> {
        for batch in batches {
            for unit in batch.encode_utf16() {
                vt.feed(unit);
            }
            vt.settle();
        }
        std::iter::from_fn(|| vt.pop()).collect()
    }

    fn parse(batches: &[&str]) -> Vec<Event> {
        events(&mut VtInput::default(), batches)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
    const CONTROL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;

    #[test]
    fn keys_map_to_the_events_crossterm_reads_on_unix() {
        // The bytes herdr's ConPTY delivered per key: legacy, then disambiguated.
        let rows: &[(&str, Event)] = &[
            ("a", key(KeyCode::Char('a'), NONE)),
            ("A", key(KeyCode::Char('A'), SHIFT)),
            ("é", key(KeyCode::Char('é'), NONE)),
            ("日", key(KeyCode::Char('日'), NONE)),
            ("🙂", key(KeyCode::Char('🙂'), NONE)),
            ("\r", key(KeyCode::Enter, NONE)),
            ("\n", key(KeyCode::Char('j'), CONTROL)),
            ("\x7f", key(KeyCode::Backspace, NONE)),
            ("\x1b", key(KeyCode::Esc, NONE)),
            ("\x1b\r", key(KeyCode::Enter, ALT)),
            ("\x1bx", key(KeyCode::Char('x'), ALT)),
            ("\x1b[A", key(KeyCode::Up, NONE)),
            ("\x1b[D", key(KeyCode::Left, NONE)),
            ("\x1b[1;5D", key(KeyCode::Left, CONTROL)),
            ("\x1b[Z", key(KeyCode::BackTab, SHIFT)),
            ("\x1b[13;2u", key(KeyCode::Enter, SHIFT)),
            ("\x1b[13;3u", key(KeyCode::Enter, ALT)),
            ("\x1b[106;5u", key(KeyCode::Char('j'), CONTROL)),
            ("\x1b[27u", key(KeyCode::Esc, NONE)),
            ("\x1b[120;3u", key(KeyCode::Char('x'), ALT)),
            ("\x1b[9;2u", key(KeyCode::BackTab, SHIFT)),
        ];
        for (bytes, expected) in rows {
            assert_eq!(parse(&[bytes]), vec![expected.clone()], "{bytes:?}");
        }
    }

    #[test]
    fn sgr_mouse_reports_map_to_zero_based_mouse_events() {
        let rows: &[(&str, Event)] = &[
            ("\x1b[<0;10;5M", mouse(MouseEventKind::Down(MouseButton::Left), 9, 4)),
            ("\x1b[<0;10;5m", mouse(MouseEventKind::Up(MouseButton::Left), 9, 4)),
            ("\x1b[<32;11;5M", mouse(MouseEventKind::Drag(MouseButton::Left), 10, 4)),
            ("\x1b[<64;10;5M", mouse(MouseEventKind::ScrollUp, 9, 4)),
            ("\x1b[<65;10;5M", mouse(MouseEventKind::ScrollDown, 9, 4)),
        ];
        for (bytes, expected) in rows {
            assert_eq!(parse(&[bytes]), vec![expected.clone()], "{bytes:?}");
        }
    }

    #[test]
    fn a_bracketed_paste_is_one_event_with_its_text_verbatim() {
        // Exactly what herdr's paste handler writes on Windows: markers, CRLF line breaks.
        let pasted = "\x1b[200~line one\r\nline two é 日本 🙂\x1b[201~";
        assert_eq!(parse(&[pasted]), vec![Event::Paste("line one\r\nline two é 日本 🙂".into())]);
        // Split across console reads, it is still one paste.
        assert_eq!(
            parse(&["\x1b[200~line one\r", "\nline two\x1b[2", "01~"]),
            vec![Event::Paste("line one\r\nline two".into())]
        );
    }

    #[test]
    fn a_sequence_split_across_reads_waits_for_its_end() {
        assert_eq!(parse(&["\x1b[", "A"]), vec![key(KeyCode::Up, NONE)]);
        // A surrogate pair split across reads is one character.
        let smile: Vec<u16> = "🙂".encode_utf16().collect();
        let mut vt = VtInput::default();
        vt.feed(smile[0]);
        vt.settle();
        vt.feed(smile[1]);
        vt.settle();
        assert_eq!(vt.pop(), Some(key(KeyCode::Char('🙂'), NONE)));
    }

    #[test]
    fn a_lone_esc_at_the_end_of_a_read_is_the_esc_key() {
        assert_eq!(
            parse(&["\x1b", "j"]),
            vec![key(KeyCode::Esc, NONE), key(KeyCode::Char('j'), NONE)]
        );
    }

    #[test]
    fn an_unterminated_paste_closes_as_a_paste_after_its_bound() {
        let mut vt = VtInput::default();
        assert_eq!(events(&mut vt, &["\x1b[200~abc\rx"]), vec![]);
        vt.waited(PASTE_IDLE.saturating_sub(Duration::from_millis(1)));
        vt.expire();
        assert_eq!(vt.pop(), None, "still inside the bound");
        // More input restarts the bound.
        assert_eq!(events(&mut vt, &["y"]), vec![]);
        vt.waited(PASTE_IDLE.saturating_sub(Duration::from_millis(1)));
        vt.expire();
        assert_eq!(vt.pop(), None, "the bound counts from the last input");
        vt.waited(Duration::from_millis(1));
        vt.expire();
        assert_eq!(vt.pop(), Some(Event::Paste("abc\rxy".into())));
        // Input reads as keys again.
        assert_eq!(events(&mut vt, &["x"]), vec![key(KeyCode::Char('x'), NONE)]);
    }
}
