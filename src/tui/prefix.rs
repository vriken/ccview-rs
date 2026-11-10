use super::split::Resize;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub const PREFIX: KeyEvent = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
pub const PREFIX_LABEL: &str = "ctrl+space";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Forward(KeyEvent),
    Armed,
    Cancelled,
    ReturnToList,
    Detach,
    Previous,
    Next,
    SendPrefix,
    CycleColor,
    ScrollUp,
    ScrollDown,
    Resize(Resize),
}

#[derive(Default)]
pub struct Prefix {
    armed: bool,
}

impl Prefix {
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    pub fn reset(&mut self) {
        self.armed = false;
    }

    pub fn feed(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::Cancelled;
        }
        if !self.armed {
            if is_prefix(key) {
                self.armed = true;
                return Action::Armed;
            }
            return Action::Forward(key);
        }
        self.armed = false;
        if is_prefix(key) {
            return Action::SendPrefix;
        }
        match key.code {
            KeyCode::Char('l') | KeyCode::Esc => Action::ReturnToList,
            KeyCode::Char('x' | 'd') => Action::Detach,
            KeyCode::Char('C') => Action::CycleColor,
            KeyCode::Char('[') | KeyCode::Left => Action::Previous,
            KeyCode::Char(']') | KeyCode::Right => Action::Next,
            KeyCode::PageUp | KeyCode::Up => Action::ScrollUp,
            KeyCode::PageDown | KeyCode::Down => Action::ScrollDown,
            _ => Resize::from_key(key).map_or(Action::Cancelled, Action::Resize),
        }
    }
}

fn is_prefix(key: KeyEvent) -> bool {
    key.code == PREFIX.code && key.modifiers == PREFIX.modifiers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn feed_all(prefix: &mut Prefix, keys: &[KeyEvent]) -> Vec<Action> {
        keys.iter().map(|&key| prefix.feed(key)).collect()
    }

    #[test]
    fn ordinary_keys_pass_through_and_the_prefix_is_swallowed() {
        let mut prefix = Prefix::default();
        let typed = press(KeyCode::Char('l'));
        assert_eq!(prefix.feed(typed), Action::Forward(typed));
        assert_eq!(prefix.feed(PREFIX), Action::Armed);
        assert!(prefix.is_armed());
    }

    #[test]
    fn prefix_then_command_key_selects_the_ui_action() {
        let commands = [
            (press(KeyCode::Char('l')), Action::ReturnToList),
            (press(KeyCode::Esc), Action::ReturnToList),
            (press(KeyCode::Char('x')), Action::Detach),
            (press(KeyCode::Char('d')), Action::Detach),
            (press(KeyCode::Char('C')), Action::CycleColor),
            (press(KeyCode::Char('[')), Action::Previous),
            (press(KeyCode::Left), Action::Previous),
            (press(KeyCode::Char(']')), Action::Next),
            (press(KeyCode::Right), Action::Next),
            (press(KeyCode::PageUp), Action::ScrollUp),
            (press(KeyCode::PageDown), Action::ScrollDown),
            (PREFIX, Action::SendPrefix),
            (press(KeyCode::Char('<')), Action::Resize(Resize::Delta(-2))),
            (press(KeyCode::Char('>')), Action::Resize(Resize::Delta(2))),
            (press(KeyCode::Char('=')), Action::Resize(Resize::Reset)),
        ];
        for (key, expected) in commands {
            let mut prefix = Prefix::default();
            assert_eq!(feed_all(&mut prefix, &[PREFIX, key]), [Action::Armed, expected]);
            assert!(!prefix.is_armed());
        }
    }

    #[test]
    fn unknown_key_after_prefix_is_dropped_and_disarms() {
        let mut prefix = Prefix::default();
        let stray = press(KeyCode::Char('z'));
        assert_eq!(feed_all(&mut prefix, &[PREFIX, stray]), [Action::Armed, Action::Cancelled]);
        assert_eq!(prefix.feed(stray), Action::Forward(stray));
    }
}
