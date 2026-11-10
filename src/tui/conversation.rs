use super::{
    theme,
    timefmt::{self, Stamp},
    tool_block,
    verbosity::Level,
};
use crate::{
    markdown,
    parse::{Block, Message, Role},
};
use ratatui::{
    text::{Line, Span},
};
use serde_json::Value;
use std::collections::VecDeque;

const NOTHING_STALE: usize = usize::MAX;

enum Unit {
    Text(String),
    Thinking(String),
    Tool { name: String, input: Value, result: Option<String> },
    OrphanResult(String),
}

struct Turn {
    role: Role,
    meta: bool,
    stamp: Option<Stamp>,
    units: Vec<Unit>,
    tool_calls: usize,
    quiet: bool,
}

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
        self.push_messages(messages);
        self.note_hidden_earlier(hidden_earlier);
    }

    pub fn push_messages(&mut self, messages: &[Message]) {
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
            let tool_calls = units.iter().filter(|u| matches!(u, Unit::Tool { .. })).count();
            let has_text = units.iter().any(|u| matches!(u, Unit::Text(_)));
            self.turns.push(Turn {
                role: message.role,
                meta: message.is_meta,
                stamp: message.timestamp.as_deref().and_then(timefmt::local_stamp),
                units,
                tool_calls,
                quiet: message.is_meta || !has_text,
            });
        }
    }

    fn units_of(&mut self, message: &Message) -> Vec<Unit> {
        let turn_index = self.turns.len();
        let mut units = Vec::new();
        for block in &message.blocks {
            match block {
                Block::Text(text) => units.push(Unit::Text(text.clone())),
                Block::Thinking(text) if !text.trim().is_empty() => units.push(Unit::Thinking(text.clone())),
                Block::Thinking(_) => {}
                Block::ToolUse { name, input } => {
                    self.open_calls.push_back((turn_index, units.len()));
                    units.push(Unit::Tool { name: name.clone(), input: input.clone(), result: None });
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

    pub fn lines(&mut self, width: usize, level: Level) -> &[Line<'static>] {
        let width = width.max(1);
        if self.cache.width != width || self.cache.level != level {
            self.cache = Cache { width, level, ..Cache::default() };
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
                    format!("… {} earlier messages not shown", self.hidden_earlier),
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
            render_turn(&mut self.cache.lines, turn, level, width, &self.markdown_theme, &mut shown_day);
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
        let noun = if *count == 1 { "tool call" } else { "tool calls" };
        if lines.last().is_some_and(|l| !is_blank(l)) {
            lines.push(Line::default());
        }
        lines.push(Line::styled(format!("  · {count} {noun}"), theme::tool_connector()));
        *count = 0;
    }
}

fn is_blank(line: &Line) -> bool {
    line.spans.iter().all(|s| s.content.is_empty())
}

fn render_turn(
    out: &mut Vec<Line<'static>>,
    turn: &Turn,
    level: Level,
    width: usize,
    markdown_theme: &markdown::Theme,
    shown_day: &mut Option<String>,
) {
    let mut body: Vec<Line<'static>> = Vec::new();
    let show_meta = level == Level::Everything;
    let mut headed = false;
    for unit in &turn.units {
        match unit {
            Unit::Text(_) if turn.meta && !show_meta => {}
            Unit::Text(text) => {
                headed = true;
                push_text(&mut body, turn, text, width, markdown_theme);
            }
            Unit::Thinking(text) if level == Level::Everything => {
                headed = true;
                push_thinking(&mut body, text, width);
            }
            Unit::Thinking(_) => {}
            Unit::Tool { name, input, result } if level != Level::Chat => {
                tool_block::render_call(&mut body, name, input, result.as_deref(), level, width)
            }
            Unit::OrphanResult(result) if level != Level::Chat => {
                tool_block::render_orphan_result(&mut body, result, level, width)
            }
            Unit::Tool { .. } | Unit::OrphanResult(_) => {}
        }
    }
    if body.is_empty() {
        return;
    }
    if headed {
        push_header(out, turn, shown_day);
        out.extend(body);
        out.push(Line::default());
    } else {
        out.extend(body);
    }
}

fn push_header(out: &mut Vec<Line<'static>>, turn: &Turn, shown_day: &mut Option<String>) {
    if let Some(stamp) = &turn.stamp {
        if shown_day.as_deref() != Some(&stamp.day) {
            if out.last().is_some_and(|l| !is_blank(l)) {
                out.push(Line::default());
            }
            out.push(Line::styled(format!("── {} ──", stamp.day), theme::day_divider()));
            *shown_day = Some(stamp.day.clone());
        }
    }
    if out.last().is_some_and(|l| !is_blank(l)) {
        out.push(Line::default());
    }
    let (label, style) = match (turn.role, turn.meta) {
        (_, true) => ("SYSTEM", theme::system_label()),
        (Role::User, false) => ("USER", theme::user_label()),
        (Role::Assistant, false) => ("ASSISTANT", theme::assistant_label()),
    };
    let clock = turn.stamp.as_ref().map_or("", |s| s.clock.as_str());
    out.push(Line::styled(format!("{label}  {clock}"), style));
}

fn push_text(body: &mut Vec<Line<'static>>, turn: &Turn, text: &str, width: usize, md: &markdown::Theme) {
    if turn.meta {
        for line in text.lines() {
            body.extend(wrap(&format!("  {}", tool_block::clean(line)), width).into_iter().map(|p| Line::styled(p, theme::secondary())));
        }
        return;
    }
    match turn.role {
        Role::Assistant => {
            let gutter = "  ";
            let body_width = width.saturating_sub(gutter.len()).max(1);
            for mut line in markdown::render(text, body_width as u16, md) {
                line.spans.insert(0, Span::styled(gutter, theme::assistant_text()));
                body.push(line);
            }
        }
        Role::User => {
            for line in text.lines() {
                body.extend(
                    wrap(&format!("▌ {line}"), width)
                        .into_iter()
                        .map(|piece| Line::styled(piece, theme::user_text())),
                );
            }
        }
    }
}

fn push_thinking(body: &mut Vec<Line<'static>>, text: &str, width: usize) {
    body.push(Line::styled("  ∴ thinking", theme::thinking()));
    for line in text.lines() {
        body.extend(
            wrap(&format!("    {}", tool_block::clean(line)), width)
                .into_iter()
                .map(|piece| Line::styled(piece, theme::thinking())),
        );
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let indent: String = text.chars().take_while(|c| *c == ' ').collect();
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let mut word = word.to_owned();
        let mut word_len = word.chars().count();
        if current_len > 0 && current_len + 1 + word_len > width {
            lines.push(std::mem::take(&mut current));
            current_len = 0;
        }
        while word_len > width {
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            word_len -= width;
            if current_len > 0 {
                lines.push(std::mem::take(&mut current));
                current_len = 0;
            }
            lines.push(head);
        }
        if current_len > 0 {
            current.push(' ');
            current_len += 1;
        } else {
            current.push_str(&indent);
            current_len = indent.chars().count();
        }
        current.push_str(&word);
        current_len += word_len;
    }
    lines.push(current);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(role: Role, meta: bool, blocks: Vec<Block>) -> Message {
        Message { role, timestamp: None, is_meta: meta, blocks }
    }

    fn call(name: &str, input: Value) -> Block {
        Block::ToolUse { name: name.into(), input }
    }

    fn result(text: &str) -> Block {
        Block::ToolResult(text.into())
    }

    fn rendered(conversation: &mut Conversation, level: Level) -> Vec<String> {
        conversation
            .lines(60, level)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect()
    }

    fn session() -> Vec<Message> {
        vec![
            message(Role::User, false, vec![Block::Text("hello".into())]),
            message(Role::User, true, vec![Block::Text("injected context".into())]),
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
            message(Role::Assistant, false, vec![call("Bash", json!({"command": "pwd"}))]),
            message(Role::User, false, vec![result("out-pwd")]),
            message(Role::Assistant, false, vec![Block::Text("done".into())]),
        ]
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
        assert!(text[ls + 1].contains("out-a"), "result sits right under its call");
    }

    #[test]
    fn redacted_empty_thinking_is_dropped_entirely() {
        let mut conversation = Conversation::default();
        conversation.push_messages(&[message(Role::Assistant, false, vec![Block::Thinking("  ".into())])]);
        assert!(rendered(&mut conversation, Level::Everything).is_empty());
    }

    #[test]
    fn everything_adds_thinking_and_meta_messages() {
        let mut conversation = Conversation::default();
        conversation.push_messages(&session());
        let text = rendered(&mut conversation, Level::Everything).join("\n");
        assert!(text.contains("pondering") && text.contains("injected context") && text.contains("SYSTEM"));
    }

    #[test]
    fn call_without_result_shows_a_running_marker_that_resolves_when_the_result_arrives() {
        let mut conversation = Conversation::default();
        conversation.push_messages(&[message(Role::Assistant, false, vec![call("Bash", json!({"command": "sleep"}))])]);
        let before = rendered(&mut conversation, Level::Tools);
        assert_eq!(before.last().unwrap(), "  ⎿  …");
        conversation.push_messages(&[message(Role::User, false, vec![result("woke")])]);
        let after = rendered(&mut conversation, Level::Tools);
        assert_eq!(after, vec!["● Bash(sleep)".to_owned(), "  ⎿  woke".to_owned()]);
    }

    #[test]
    fn result_without_a_preceding_call_is_shown_on_its_own_and_prompt_resets_pairing() {
        let mut conversation = Conversation::default();
        conversation.push_messages(&[
            message(Role::User, false, vec![result("orphan")]),
            message(Role::Assistant, false, vec![call("Bash", json!({"command": "aborted"}))]),
            message(Role::User, false, vec![Block::Text("new prompt".into())]),
            message(Role::Assistant, false, vec![call("Bash", json!({"command": "second"}))]),
            message(Role::User, false, vec![result("second-out")]),
        ]);
        let text = rendered(&mut conversation, Level::Tools);
        assert_eq!(text[0], "  ⎿  orphan");
        let aborted = text.iter().position(|l| l.contains("aborted")).unwrap();
        assert_eq!(text[aborted + 1], "  ⎿  …");
        let second = text.iter().position(|l| l.contains("Bash(second)")).unwrap();
        assert_eq!(text[second + 1], "  ⎿  second-out");
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

    #[test]
    fn wrapped_thinking_keeps_its_indent_on_every_row() {
        let mut conversation = Conversation::default();
        conversation.push_messages(&[message(
            Role::Assistant,
            false,
            vec![Block::Thinking("word ".repeat(40))],
        )]);
        let text = rendered(&mut conversation, Level::Everything);
        let body: Vec<&String> = text.iter().filter(|l| l.contains("word")).collect();
        assert!(body.len() > 2 && body.iter().all(|l| l.starts_with("    word")), "{body:?}");
    }

    #[test]
    fn switching_level_and_back_restores_the_original_lines() {
        let mut conversation = Conversation::default();
        conversation.push_messages(&session());
        let chat = rendered(&mut conversation, Level::Chat);
        rendered(&mut conversation, Level::Everything);
        assert_eq!(rendered(&mut conversation, Level::Chat), chat);
    }

    #[test]
    fn day_divider_appears_when_the_local_day_changes() {
        let mut conversation = Conversation::default();
        let stamped = |ts: &str, text: &str| Message {
            role: Role::User,
            timestamp: Some(ts.into()),
            is_meta: false,
            blocks: vec![Block::Text(text.into())],
        };
        conversation.push_messages(&[
            stamped("2026-11-10T10:00:00Z", "one"),
            stamped("2026-11-10T11:00:00Z", "two"),
            stamped("2026-11-12T11:00:00Z", "three"),
        ]);
        let text = rendered(&mut conversation, Level::Chat);
        assert_eq!(text.iter().filter(|l| l.starts_with("── ")).count(), 2, "{text:?}");
        assert!(text.iter().any(|l| l.starts_with("USER  ") && l.len() == "USER  HH:MM".len()));
    }
}
