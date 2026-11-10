#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    ListRows,
    TabBar,
    Transcript,
    Embedded,
    Prefix,
    Global,
    Overlay,
}

impl Zone {
    pub const HELP_ORDER: [Zone; 7] = [
        Zone::ListRows,
        Zone::TabBar,
        Zone::Transcript,
        Zone::Embedded,
        Zone::Prefix,
        Zone::Global,
        Zone::Overlay,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Zone::ListRows => "List rows",
            Zone::TabBar => "Tab bar",
            Zone::Transcript => "Transcript",
            Zone::Embedded => "Embedded session",
            Zone::Prefix => "After ctrl+space (embedded session)",
            Zone::Global => "Everywhere",
            Zone::Overlay => "Settings and help",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct KeyHint {
    pub zone: Zone,
    pub keys: &'static str,
    pub label: &'static str,
    pub note: &'static str,
    pub essential: bool,
}

const fn key(zone: Zone, keys: &'static str, label: &'static str, note: &'static str, essential: bool) -> KeyHint {
    KeyHint { zone, keys, label, note, essential }
}

pub const SETTINGS_KEY: &str = ",";
pub const HELP_KEY: &str = "?";

use Zone::{Embedded, Global, ListRows, Overlay, Prefix, TabBar, Transcript};

pub const KEYS: &[KeyHint] = &[
    key(ListRows, "↑/↓", "move", "also j/k", true),
    key(ListRows, "→", "open", "also l, Enter, Tab: read or focus the right pane", true),
    key(ListRows, "↑", "tabs", "from the first row: go to the tab bar", false),
    key(ListRows, "r", "resume", "resume the selected session in an embedded pane", true),
    key(ListRows, "n", "new", "start a new session in the selected directory", true),
    key(ListRows, "s", "star", "also space: star or unstar the session", true),
    key(ListRows, "f", "fork", "fork the selected session", false),
    key(ListRows, "c", "continue", "continue the latest session in the directory", false),
    key(ListRows, "a", "attach", "attach to a Claude background session", false),
    key(ListRows, "o/O", "terminal", "o resumes, O starts new, in an external terminal", false),
    key(ListRows, "x", "hide", "hide the selected session (see Settings)", false),
    key(ListRows, "d/D", "dismiss", "d: drop an active session from Active until it works again; D: undo. ccview only, never touches the process", false),
    key(ListRows, "H", "show hidden", "show or hide hidden sessions in the list", false),
    key(ListRows, "E", "copies", "show or collapse older copies of the same conversation", false),
    key(ListRows, "S", "star dir", "star or unstar the selected row's directory", false),
    key(ListRows, "Shift+←/→", "dir", "switch directory tab without leaving the rows", false),
    key(ListRows, "v", "view", "cycle view level: chat, tools, everything", false),
    key(ListRows, "C", "colour", "cycle the selected session's colour", false),
    key(ListRows, "< > =", "resize", "narrower, wider, reset the list pane", false),
    key(ListRows, "PgUp/PgDn", "scroll", "scroll the transcript", false),
    key(ListRows, "g/G", "top/end", "transcript to top or follow the end", false),
    key(ListRows, SETTINGS_KEY, "settings", "colours, view, mouse, split, terminal, hidden", true),
    key(ListRows, HELP_KEY, "help", "this screen", true),
    key(ListRows, "q", "quit", "also ctrl+c; asks while sessions are running", true),
    key(TabBar, "←/→", "tab", "also h/l, [ ]", true),
    key(TabBar, "↓", "rows", "also Enter, Esc, Tab", true),
    key(TabBar, "s", "star dir", "star or unstar the focused directory tab", false),
    key(TabBar, "Shift+←/→", "tab", "also switches tabs", false),
    key(TabBar, "r n s ...", "list keys", "every list-row action also works here", false),
    key(TabBar, SETTINGS_KEY, "settings", "open settings", true),
    key(TabBar, HELP_KEY, "help", "this screen", true),
    key(TabBar, "q", "quit", "also ctrl+c", true),
    key(Transcript, "←/esc", "list", "also h, Tab: back to the list", true),
    key(Transcript, "↑/↓", "scroll", "also j/k", true),
    key(Transcript, "PgUp/PgDn", "page", "scroll a page", false),
    key(Transcript, "g/G", "top/end", "jump to the top, follow the end", false),
    key(Transcript, "r", "resume", "resume this session", true),
    key(Transcript, "f", "fork", "fork this session", false),
    key(Transcript, "n", "new", "new session in this directory", false),
    key(Transcript, "c", "continue", "continue the latest session in the directory", false),
    key(Transcript, "o/O", "terminal", "external terminal: o resume, O new", false),
    key(Transcript, "d/D", "dismiss", "dismiss from Active, undo (ccview only)", false),
    key(Transcript, "s", "star", "also space", false),
    key(Transcript, "v", "view", "cycle view level", false),
    key(Transcript, "C", "colour", "cycle colour", false),
    key(Transcript, "< > =", "resize", "narrower, wider, reset the list pane", false),
    key(Transcript, "Shift+←/→", "dir", "switch directory tab", false),
    key(Transcript, SETTINGS_KEY, "settings", "open settings", true),
    key(Transcript, HELP_KEY, "help", "this screen", true),
    key(Transcript, "q", "quit", "also ctrl+c", true),
    key(Embedded, "←", "back to list", "only at the start of the prompt", true),
    key(Embedded, "any other key", "session", "sent to the session, including ctrl+c and Esc", false),
    key(Prefix, "l", "list", "also Esc: back to the list", true),
    key(Prefix, "x", "close", "close the session (asks first)", true),
    key(Prefix, SETTINGS_KEY, "settings", "open settings", true),
    key(Prefix, HELP_KEY, "help", "open this screen", true),
    key(Prefix, "[ ]", "switch", "also ←/→: previous or next session", false),
    key(Prefix, "C", "colour", "cycle this session's colour", false),
    key(Prefix, "PgUp/PgDn", "scroll", "also ↑/↓: scroll the session", false),
    key(Prefix, "< > =", "resize/reset", "resize the list pane", false),
    key(Prefix, "ctrl+space", "send literal", "send ctrl+space to the session", false),
    key(Global, "ctrl+c", "quit", "forwarded to a focused session", false),
    key(Global, "y / n", "answer", "answers a confirmation in the footer", false),
    key(Overlay, "↑/↓", "move", "settings: select a row", false),
    key(Overlay, "←/→", "change", "settings: change the value (Enter or space too)", false),
    key(Overlay, "PgUp/PgDn", "page", "help: scroll", false),
    key(Overlay, "Esc", "close", "also leaves the hidden-sessions list", false),
];

pub fn zone_keys(zone: Zone) -> impl Iterator<Item = &'static KeyHint> {
    KEYS.iter().filter(move |hint| hint.zone == zone)
}

pub fn essential(zone: Zone) -> impl Iterator<Item = &'static KeyHint> {
    zone_keys(zone).filter(|hint| hint.essential)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::prefix::{Action, PREFIX, Prefix};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn documented(zone: Zone, text: &str) -> bool {
        zone_keys(zone).any(|hint| {
            let separators = [' ', '/', ',', ':'];
            hint.keys == text || hint.keys.split(separators).chain(hint.note.split(separators)).any(|part| part == text)
        })
    }

    #[test]
    fn every_prefix_command_the_router_accepts_is_documented() {
        let candidates = ('!'..='~').map(KeyCode::Char).chain([
            KeyCode::Esc,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
        ]);
        for code in candidates {
            let mut prefix = Prefix::default();
            prefix.feed(PREFIX);
            if prefix.feed(KeyEvent::new(code, KeyModifiers::NONE)) == Action::Cancelled {
                continue;
            }
            let text = match code {
                KeyCode::Char(c) => c.to_string(),
                KeyCode::Esc => "Esc".to_owned(),
                KeyCode::Left => "←".to_owned(),
                KeyCode::Right => "→".to_owned(),
                KeyCode::Up => "↑".to_owned(),
                KeyCode::Down => "↓".to_owned(),
                KeyCode::PageUp => "PgUp".to_owned(),
                _ => "PgDn".to_owned(),
            };
            assert!(documented(Zone::Prefix, &text), "prefix key {code:?} is handled but not in the help table");
        }
    }

    #[test]
    fn list_and_transcript_keys_handled_by_the_router_are_documented() {
        let list = ['q', 'r', 'f', 'n', 'c', 'a', 'o', 'O', 's', 'x', 'H', 'C', 'v', 'g', 'G', 'j', 'k', 'l', '<', '>', '=', ',', '?'];
        for c in list {
            assert!(documented(Zone::ListRows, &c.to_string()), "{c} missing from list rows");
        }
        let transcript = ['q', 'r', 'f', 'n', 'c', 'o', 'O', 's', 'v', 'C', 'g', 'G', 'j', 'k', 'h', '<', '>', '=', ',', '?'];
        for c in transcript {
            assert!(documented(Zone::Transcript, &c.to_string()), "{c} missing from transcript");
        }
    }

    #[test]
    fn essential_hints_end_with_help_then_quit_on_zones_that_pin_them() {
        for zone in [Zone::ListRows, Zone::TabBar, Zone::Transcript] {
            let labels: Vec<_> = essential(zone).map(|hint| hint.label).collect();
            assert_eq!(labels[labels.len() - 2..], ["help", "quit"]);
        }
    }
}
