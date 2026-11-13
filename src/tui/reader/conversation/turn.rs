use super::Conversation;
use crate::session::transcript::{Block, Message, Role};
use crate::tui::reader::local_time::LocalTime;
use serde_json::Value;

pub(super) enum Unit {
    Text(String),
    Thinking(String),
    Tool {
        name: String,
        input: Value,
        result: Option<String>,
    },
    OrphanResult(String),
}

pub(super) struct Turn {
    pub(super) role: Role,
    pub(super) meta: bool,
    pub(super) offset: Option<u64>,
    pub(super) stamp: Option<LocalTime>,
    pub(super) units: Vec<Unit>,
    pub(super) tool_calls: usize,
    pub(super) quiet: bool,
}

impl Turn {
    pub(super) fn starts_exchange(&self) -> bool {
        self.role == Role::User
            && !self.meta
            && self.units.iter().any(|unit| matches!(unit, Unit::Text(_)))
    }
}

impl Conversation {
    pub(super) fn units_of(&mut self, message: &Message) -> Vec<Unit> {
        let turn_index = self.turns.len();
        let mut units = Vec::new();
        for block in &message.blocks {
            match block {
                Block::Text(text) => units.push(Unit::Text(text.clone())),
                Block::Thinking(text) if !text.trim().is_empty() => {
                    units.push(Unit::Thinking(text.clone()))
                }
                Block::Thinking(_) => {}
                Block::ToolUse { name, input } => {
                    self.open_calls.push_back((turn_index, units.len()));
                    units.push(Unit::Tool {
                        name: name.clone(),
                        input: input.clone(),
                        result: None,
                    });
                }
                Block::ToolResult(content) => match self.open_calls.pop_front() {
                    Some((turn, unit)) if turn == turn_index => {
                        if let Unit::Tool { result, .. } = &mut units[unit] {
                            *result = Some(content.clone());
                        }
                    }
                    Some((turn, unit)) => {
                        if let Unit::Tool { result, .. } = &mut self.turns[turn].units[unit] {
                            *result = Some(content.clone());
                        }
                        self.first_stale_turn = self.first_stale_turn.min(turn);
                    }
                    None => units.push(Unit::OrphanResult(content.clone())),
                },
            }
        }
        units
    }
}
