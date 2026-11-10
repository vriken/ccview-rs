use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

const KITTY_STACK_LIMIT: usize = 16;
const EVENT_QUEUE_LIMIT: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyboardFlags(u8);

impl KeyboardFlags {
    pub const NONE: Self = Self(0);
    pub const DISAMBIGUATE: Self = Self(1);
    pub const EVENT_TYPES: Self = Self(2);
    pub const ALTERNATE_KEYS: Self = Self(4);
    pub const REPORT_ALL_KEYS: Self = Self(8);
    pub const ASSOCIATED_TEXT: Self = Self(16);

    const ALL_BITS: u8 = 31;

    pub fn from_bits(bits: u8) -> Self {
        Self(bits & Self::ALL_BITS)
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for KeyboardFlags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self::from_bits(self.0 | other.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Title(String),
    Bell,
    Clipboard { selection: String, data: Vec<u8> },
    Notification { title: Option<String>, body: String },
}

#[derive(Default)]
struct KeyboardStack(Vec<KeyboardFlags>);

impl KeyboardStack {
    fn current(&self) -> KeyboardFlags {
        self.0.last().copied().unwrap_or_default()
    }

    fn push(&mut self, flags: KeyboardFlags) {
        if self.0.len() == KITTY_STACK_LIMIT {
            self.0.remove(0);
        }
        self.0.push(flags);
    }

    fn pop(&mut self, count: usize) {
        let keep = self.0.len().saturating_sub(count);
        self.0.truncate(keep);
    }

    fn set(&mut self, flags: KeyboardFlags, mode: u16) {
        let current = self.current();
        let next = match mode {
            2 => KeyboardFlags::from_bits(current.0 | flags.0),
            3 => KeyboardFlags::from_bits(current.0 & !flags.0),
            _ => flags,
        };
        match self.0.last_mut() {
            Some(top) => *top = next,
            None => self.0.push(next),
        }
    }
}

#[derive(Default)]
pub(super) struct TerminalState {
    main_keyboard: KeyboardStack,
    alternate_keyboard: KeyboardStack,
    focus_reporting: bool,
    synchronized_output: bool,
    title: Option<String>,
    events: VecDeque<TerminalEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeSnapshot {
    main_keyboard: Vec<u8>,
    alternate_keyboard: Vec<u8>,
    focus_reporting: bool,
    synchronized_output: bool,
    title: Option<String>,
}

impl KeyboardStack {
    fn bits(&self) -> Vec<u8> {
        self.0.iter().map(|flags| flags.bits()).collect()
    }

    fn from_bits(bits: &[u8]) -> Self {
        Self(bits.iter().map(|&bits| KeyboardFlags::from_bits(bits)).collect())
    }
}

impl TerminalState {
    fn keyboard(&mut self, alternate_screen: bool) -> &mut KeyboardStack {
        if alternate_screen {
            &mut self.alternate_keyboard
        } else {
            &mut self.main_keyboard
        }
    }

    pub(super) fn keyboard_flags(&self, alternate_screen: bool) -> KeyboardFlags {
        if alternate_screen {
            self.alternate_keyboard.current()
        } else {
            self.main_keyboard.current()
        }
    }

    pub(super) fn push_keyboard_flags(&mut self, alternate_screen: bool, flags: KeyboardFlags) {
        self.keyboard(alternate_screen).push(flags);
    }

    pub(super) fn pop_keyboard_flags(&mut self, alternate_screen: bool, count: usize) {
        self.keyboard(alternate_screen).pop(count);
    }

    pub(super) fn set_keyboard_flags(
        &mut self,
        alternate_screen: bool,
        flags: KeyboardFlags,
        mode: u16,
    ) {
        self.keyboard(alternate_screen).set(flags, mode);
    }

    pub(super) fn focus_reporting(&self) -> bool {
        self.focus_reporting
    }

    pub(super) fn set_focus_reporting(&mut self, enabled: bool) {
        self.focus_reporting = enabled;
    }

    pub(super) fn synchronized_output(&self) -> bool {
        self.synchronized_output
    }

    pub(super) fn set_synchronized_output(&mut self, enabled: bool) {
        self.synchronized_output = enabled;
    }

    pub(super) fn export(&self) -> ModeSnapshot {
        ModeSnapshot {
            main_keyboard: self.main_keyboard.bits(),
            alternate_keyboard: self.alternate_keyboard.bits(),
            focus_reporting: self.focus_reporting,
            synchronized_output: self.synchronized_output,
            title: self.title.clone(),
        }
    }

    pub(super) fn import(&mut self, modes: ModeSnapshot) {
        self.main_keyboard = KeyboardStack::from_bits(&modes.main_keyboard);
        self.alternate_keyboard = KeyboardStack::from_bits(&modes.alternate_keyboard);
        self.focus_reporting = modes.focus_reporting;
        self.synchronized_output = modes.synchronized_output;
        if let Some(title) = modes.title {
            self.push_event(TerminalEvent::Title(title));
        }
    }

    pub(super) fn push_event(&mut self, event: TerminalEvent) {
        if let TerminalEvent::Title(title) = &event {
            self.title = Some(title.clone());
        }
        if let (TerminalEvent::Title(_), Some(last @ TerminalEvent::Title(_))) =
            (&event, self.events.back_mut())
        {
            *last = event;
            return;
        }
        if self.events.len() == EVENT_QUEUE_LIMIT {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    pub(super) fn take_events(&mut self) -> Vec<TerminalEvent> {
        self.events.drain(..).collect()
    }
}

pub(super) fn decode_base64(encoded: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(encoded.len() / 4 * 3);
    let mut accumulator = 0u32;
    let mut bits = 0u32;
    for &byte in encoded.iter().take_while(|&&byte| byte != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    Some(output)
}
