use crate::tui::{
    reader::{
        follow::MAX_INITIAL_MESSAGES,
        local_time::{self},
        verbosity::Level,
    },
    style::theme,
};
use crate::{
    session::transcript::{Block, Message, Role},
    tui::reader::markdown,
};
use ratatui::text::Line;
use render::{is_blank, render_turn};
use std::collections::VecDeque;
use turn::{Turn, Unit};

mod render;
mod turn;

const NOTHING_STALE: usize = usize::MAX;
const TRIM_SLACK: usize = 500;
pub const MAX_RETAINED_TURNS: usize = MAX_INITIAL_MESSAGES + TRIM_SLACK;

#[derive(Default)]
struct Cache {
    width: usize,
    level: Level,
    lines: Vec<Line<'static>>,
    turn_starts: Vec<usize>,
    day_before_turn: Vec<Option<String>>,
    shown_day: Option<String>,
}

pub struct Conversation {
    markdown_theme: markdown::Theme,
    turns: Vec<Turn>,
    hidden_earlier: usize,
    open_calls: VecDeque<(usize, usize)>,
    first_stale_turn: usize,
    cache: Cache,
}

impl Default for Conversation {
    fn default() -> Self {
        Self {
            markdown_theme: theme::markdown(),
            turns: Vec::new(),
            hidden_earlier: 0,
            open_calls: VecDeque::new(),
            first_stale_turn: NOTHING_STALE,
            cache: Cache::default(),
        }
    }
}

impl Conversation {
    pub fn note_hidden_earlier(&mut self, count: usize) {
        if count > 0 {
            self.hidden_earlier = count;
            self.first_stale_turn = 0;
            self.cache.width = 0;
        }
    }

    pub fn replace_messages(&mut self, messages: &[Message], hidden_earlier: usize) {
        self.turns.clear();
        self.open_calls.clear();
        self.hidden_earlier = 0;
        self.first_stale_turn = 0;
        self.cache = Cache::default();
        self.append_messages(messages);
        self.note_hidden_earlier(hidden_earlier);
    }

    pub fn push_messages(&mut self, messages: &[Message]) {
        self.append_messages(messages);
        self.trim_oldest();
    }

    fn trim_oldest(&mut self) {
        if self.turns.len() <= MAX_RETAINED_TURNS {
            return;
        }
        let dropped = self.turns.len() - MAX_INITIAL_MESSAGES;
        self.turns.drain(..dropped);
        self.open_calls = self
            .open_calls
            .drain(..)
            .filter_map(|(turn, unit)| turn.checked_sub(dropped).map(|turn| (turn, unit)))
            .collect();
        self.hidden_earlier += dropped;
        self.first_stale_turn = 0;
        self.cache = Cache::default();
    }

    fn append_messages(&mut self, messages: &[Message]) {
        for message in messages {
            if message.role == Role::User
                && !message.is_meta
                && message.blocks.iter().any(|b| matches!(b, Block::Text(_)))
            {
                self.open_calls.clear();
            }
            let units = self.units_of(message);
            if units.is_empty() {
                continue;
            }
            let tool_calls = units
                .iter()
                .filter(|u| matches!(u, Unit::Tool { .. }))
                .count();
            let has_text = units.iter().any(|u| matches!(u, Unit::Text(_)));
            self.turns.push(Turn {
                role: message.role,
                meta: message.is_meta,
                stamp: message
                    .timestamp
                    .as_deref()
                    .and_then(local_time::local_stamp),
                units,
                tool_calls,
                quiet: message.is_meta || !has_text,
            });
        }
    }

    pub fn lines(&mut self, width: usize, level: Level) -> &[Line<'static>] {
        let width = width.max(1);
        if self.cache.width != width || self.cache.level != level {
            self.cache = Cache {
                width,
                level,
                ..Cache::default()
            };
            self.first_stale_turn = 0;
        }
        let rendered = self.cache.turn_starts.len();
        if rendered == self.turns.len() && self.first_stale_turn == NOTHING_STALE {
            return &self.cache.lines;
        }
        let restart = self.restart_turn();
        let mut shown_day = match self.cache.day_before_turn.get(restart) {
            Some(day) => day.clone(),
            None => self.cache.shown_day.clone(),
        };
        if restart < rendered {
            self.cache.lines.truncate(self.cache.turn_starts[restart]);
            self.cache.turn_starts.truncate(restart);
            self.cache.day_before_turn.truncate(restart);
        }
        if restart == 0 {
            self.cache.lines.clear();
            if self.hidden_earlier > 0 {
                self.cache.lines.push(Line::styled(
                    "… earlier messages not shown",
                    theme::secondary(),
                ));
                self.cache.lines.push(Line::default());
            }
        }
        let mut hidden_tool_calls = 0;
        for turn in &self.turns[restart..] {
            self.cache.turn_starts.push(self.cache.lines.len());
            self.cache.day_before_turn.push(shown_day.clone());
            if level == Level::Chat {
                if turn.quiet {
                    hidden_tool_calls += turn.tool_calls;
                    continue;
                }
                push_tool_run_summary(&mut self.cache.lines, &mut hidden_tool_calls);
            }
            render_turn(
                &mut self.cache.lines,
                turn,
                level,
                width,
                &self.markdown_theme,
                &mut shown_day,
            );
        }
        push_tool_run_summary(&mut self.cache.lines, &mut hidden_tool_calls);
        self.cache.shown_day = shown_day;
        self.first_stale_turn = NOTHING_STALE;
        &self.cache.lines
    }

    fn restart_turn(&self) -> usize {
        let mut restart = self.first_stale_turn.min(self.cache.turn_starts.len());
        while restart > 0 && self.turns[restart - 1].quiet {
            restart -= 1;
        }
        restart
    }

    pub fn turn_at_line(&self, line: usize) -> usize {
        self.cache
            .turn_starts
            .partition_point(|&start| start <= line)
            .saturating_sub(1)
    }

    pub fn first_line_of_turn(&self, turn: usize) -> usize {
        self.cache.turn_starts.get(turn).copied().unwrap_or(0)
    }
}

fn push_tool_run_summary(lines: &mut Vec<Line<'static>>, count: &mut usize) {
    if *count > 0 {
        let noun = if *count == 1 {
            "tool call"
        } else {
            "tool calls"
        };
        if lines.last().is_some_and(|l| !is_blank(l)) {
            lines.push(Line::default());
        }
        lines.push(Line::styled(
            format!("  · {count} {noun}"),
            theme::tool_connector(),
        ));
        *count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn message(role: Role, meta: bool, blocks: Vec<Block>) -> Message {
        Message {
            role,
            timestamp: None,
            is_meta: meta,
            blocks,
        }
    }

    fn call(name: &str, input: Value) -> Block {
        Block::ToolUse {
            name: name.into(),
            input,
        }
    }

    fn result(text: &str) -> Block {
        Block::ToolResult(text.into())
    }

    fn rendered(conversation: &mut Conversation, level: Level) -> Vec<String> {
        conversation
            .lines(60, level)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn session() -> Vec<Message> {
        vec![
            message(Role::User, false, vec![Block::Text("hello".into())]),
            message(
                Role::User,
                true,
                vec![Block::Text("injected context".into())],
            ),
            message(
                Role::Assistant,
                false,
                vec![
                    Block::Thinking("pondering".into()),
                    call("Bash", json!({"command": "ls"})),
                    call("Read", json!({"file_path": "/x"})),
                ],
            ),
            message(Role::User, false, vec![result("out-a"), result("a\nb")]),
            message(
                Role::Assistant,
                false,
                vec![call("Bash", json!({"command": "pwd"}))],
            ),
            message(Role::User, false, vec![result("out-pwd")]),
            message(Role::Assistant, false, vec![Block::Text("done".into())]),
        ]
    }

    #[test]
    fn user_text_wraps_by_display_width_for_wide_characters() {
        for text in [
            "漢字漢字漢字漢字漢字漢字漢字漢字",
            "🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂",
            "ab 漢字漢字漢字漢字漢字漢字 cd",
        ] {
            let mut conversation = Conversation::default();
            conversation.push_messages(&[message(
                Role::User,
                false,
                vec![Block::Text(text.into())],
            )]);
            let widest = conversation
                .lines(20, Level::Chat)
                .iter()
                .map(|l| {
                    unicode_width::UnicodeWidthStr::width(
                        l.spans
                            .iter()
                            .map(|s| s.content.as_ref())
                            .collect::<String>()
                            .as_str(),
                    )
                })
                .max()
                .unwrap();
            assert!(widest <= 20, "{text}: {widest}");
        }
    }

    #[test]
    fn appended_messages_beyond_the_retained_bound_drop_the_oldest_and_say_so() {
        let mut conversation = Conversation::default();
        let batch: Vec<Message> = (0..MAX_RETAINED_TURNS + 10)
            .map(|index| {
                message(
                    Role::Assistant,
                    false,
                    vec![Block::Text(format!("msg {index}"))],
                )
            })
            .collect();
        conversation.push_messages(&batch);
        let text = rendered(&mut conversation, Level::Chat).join("\n");
        assert!(text.contains("earlier messages not shown"));
        assert!(!text.contains("msg 0\n"));
        assert!(text.contains(&format!("msg {}", MAX_RETAINED_TURNS + 9)));
    }

    #[test]
    fn chat_hides_tools_thinking_and_meta_and_counts_each_run_of_calls_once() {
        let mut conversation = Conversation::default();
        conversation.push_messages(&session());
        let text = rendered(&mut conversation, Level::Chat).join("\n");
        assert!(text.contains("hello") && text.contains("done"));
        assert!(text.contains("· 3 tool calls"), "{text}");
        for hidden in ["injected", "pondering", "Bash(", "out-a"] {
            assert!(!text.contains(hidden), "{hidden} leaked at chat level");
        }
    }

    #[test]
    fn tools_shows_calls_with_their_results_but_not_thinking_or_meta() {
        let mut conversation = Conversation::default();
        conversation.push_messages(&session());
        let text = rendered(&mut conversation, Level::Tools);
        let joined = text.join("\n");
        assert!(joined.contains("● Bash(ls)") && joined.contains("⎿  out-a"));
        assert!(joined.contains("● Bash(pwd)") && joined.contains("⎿  out-pwd"));
        assert!(joined.contains("● Read(/x)") && joined.contains("⎿  2 lines"));
        assert!(!joined.contains("pondering") && !joined.contains("injected"));
        let ls = text.iter().position(|l| l.contains("Bash(ls)")).unwrap();
        assert!(
            text[ls + 1].contains("out-a"),
            "result sits right under its call"
        );
    }

    #[test]
    fn appending_renders_the_same_lines_as_loading_everything_at_once() {
        for level in [Level::Chat, Level::Tools, Level::Everything] {
            let messages = session();
            let mut whole = Conversation::default();
            whole.push_messages(&messages);
            let expected = rendered(&mut whole, level);
            let mut incremental = Conversation::default();
            for message in &messages {
                incremental.push_messages(std::slice::from_ref(message));
                rendered(&mut incremental, level);
            }
            assert_eq!(rendered(&mut incremental, level), expected, "{level:?}");
        }
    }
}
