use crate::tui::{
    reader::{links::LinkSpan, local_time, verbosity::Level},
    style::theme,
};
use crate::{
    session::{
        cursor::Cursor,
        transcript::{Block, Message, Role},
    },
    tui::reader::markdown,
};
use ratatui::text::Line;
use render::{Output, is_blank, render_turn};
use std::collections::VecDeque;
use std::sync::Arc;
use turn::{Turn, Unit};

mod render;
mod turn;

const NOTHING_STALE: usize = usize::MAX;
const TRIM_SLACK: usize = 500;
const HEADER_LINES: usize = 2;
pub const MAX_LOADED_TURNS: usize = 20_000;

#[derive(Default)]
struct Cache {
    width: usize,
    level: Level,
    lines: Vec<Line<'static>>,
    links: Vec<Vec<LinkSpan>>,
    turn_starts: Vec<usize>,
    day_before_turn: Vec<Option<String>>,
    shown_day: Option<String>,
}

impl Cache {
    fn sync_links(&mut self) {
        self.links.resize_with(self.lines.len(), Vec::new);
    }
}

enum Earlier {
    Start,
    Available(Cursor),
    Trimmed,
}

struct Prepend {
    turns: usize,
    rerender_until: usize,
}

pub struct Conversation {
    markdown_theme: markdown::Theme,
    cwd: String,
    turns: Vec<Turn>,
    earlier: Earlier,
    open_calls: VecDeque<(usize, usize)>,
    first_stale_turn: usize,
    cache: Cache,
    prepend: Option<Prepend>,
    prepended_turns: usize,
    prepend_shift: isize,
}

impl Default for Conversation {
    fn default() -> Self {
        Self {
            markdown_theme: theme::markdown(),
            cwd: String::new(),
            turns: Vec::new(),
            earlier: Earlier::Start,
            open_calls: VecDeque::new(),
            first_stale_turn: NOTHING_STALE,
            cache: Cache::default(),
            prepend: None,
            prepended_turns: 0,
            prepend_shift: 0,
        }
    }
}

impl Conversation {
    pub fn set_cwd(&mut self, cwd: &str) {
        if self.cwd != cwd {
            self.cwd = cwd.to_owned();
            self.first_stale_turn = 0;
            self.cache = Cache::default();
            self.prepend = None;
        }
    }

    pub fn link_at(
        &mut self,
        width: usize,
        level: Level,
        line: usize,
        column: usize,
    ) -> Option<Arc<str>> {
        self.lines(width, level);
        self.cache
            .links
            .get(line)?
            .iter()
            .find(|link| link.columns.contains(&column))
            .map(|link| link.url.clone())
    }

    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }

    pub fn earlier_cursor(&self) -> Option<&Cursor> {
        match &self.earlier {
            Earlier::Available(cursor) if self.turns.len() < MAX_LOADED_TURNS => Some(cursor),
            _ => None,
        }
    }

    pub fn take_prepend_shift(&mut self) -> isize {
        std::mem::take(&mut self.prepend_shift)
    }

    pub fn replace_messages(&mut self, messages: &[Message], earlier: Option<Cursor>) {
        self.turns.clear();
        self.open_calls.clear();
        self.earlier = earlier.map_or(Earlier::Start, Earlier::Available);
        self.first_stale_turn = 0;
        self.cache = Cache::default();
        self.prepend = None;
        self.prepended_turns = 0;
        self.prepend_shift = 0;
        self.append_messages(messages);
    }

    pub fn push_messages(&mut self, messages: &[Message]) {
        self.append_messages(messages);
        self.trim_oldest();
        self.refresh_marker();
    }

    pub fn prepend_messages(&mut self, messages: &[Message], earlier: Option<Cursor>) {
        self.earlier = earlier.map_or(Earlier::Start, Earlier::Available);
        let reusable = self.first_stale_turn == NOTHING_STALE
            && self.prepend.is_none()
            && !self.turns.is_empty()
            && self.cache.turn_starts.len() == self.turns.len();
        let mut old_turns = std::mem::take(&mut self.turns);
        let old_open_calls = std::mem::take(&mut self.open_calls);
        self.append_messages(messages);
        let page_turns = self.turns.len();
        if page_turns == 0 {
            self.turns = old_turns;
            self.open_calls = old_open_calls;
            self.refresh_marker();
            return;
        }
        let touched = self.relink_head_orphans(&mut old_turns);
        self.turns.append(&mut old_turns);
        self.open_calls = old_open_calls
            .into_iter()
            .map(|(turn, unit)| (turn + page_turns, unit))
            .collect();
        if reusable {
            self.prepend = Some(Prepend {
                turns: page_turns,
                rerender_until: page_turns + 1 + touched.unwrap_or(0),
            });
        } else {
            self.cache = Cache::default();
            self.first_stale_turn = 0;
            self.prepend = None;
            self.prepended_turns += page_turns;
        }
    }

    fn relink_head_orphans(&mut self, old_turns: &mut [Turn]) -> Option<usize> {
        let mut unmatched_calls = std::mem::take(&mut self.open_calls);
        let mut touched = None;
        for (index, turn) in old_turns.iter_mut().enumerate() {
            if turn.starts_exchange() {
                break;
            }
            let before = turn.units.len();
            turn.units.retain_mut(|unit| {
                let Unit::OrphanResult(content) = unit else {
                    return true;
                };
                let Some((call_turn, call_unit)) = unmatched_calls.pop_front() else {
                    return true;
                };
                if let Unit::Tool { result, .. } = &mut self.turns[call_turn].units[call_unit] {
                    *result = Some(std::mem::take(content));
                }
                false
            });
            if turn.units.len() != before {
                touched = Some(index);
            }
        }
        touched
    }

    fn trim_oldest(&mut self) {
        if self.turns.len() <= MAX_LOADED_TURNS + TRIM_SLACK {
            return;
        }
        let dropped = self.turns.len() - MAX_LOADED_TURNS;
        self.turns.drain(..dropped);
        self.open_calls = self
            .open_calls
            .drain(..)
            .filter_map(|(turn, unit)| turn.checked_sub(dropped).map(|turn| (turn, unit)))
            .collect();
        self.earlier = Earlier::Trimmed;
        self.first_stale_turn = 0;
        self.cache = Cache::default();
        self.prepend = None;
        self.prepended_turns = 0;
        self.prepend_shift = 0;
    }

    fn earlier_line(&self) -> Line<'static> {
        let text = match &self.earlier {
            Earlier::Start => "start of session",
            Earlier::Available(_) if self.turns.len() < MAX_LOADED_TURNS => "loading earlier…",
            Earlier::Available(_) | Earlier::Trimmed => "earlier messages not loaded",
        };
        Line::styled(text, theme::secondary())
    }

    fn refresh_marker(&mut self) {
        let marker = self.earlier_line();
        if let Some(first) = self.cache.lines.first_mut() {
            *first = marker;
        }
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
            self.prepend = None;
        }
        if let Some(prepend) = self.prepend.take() {
            self.splice_prepended(&prepend);
        }
        self.render_pending();
        self.settle_prepend_shift();
        &self.cache.lines
    }

    fn render_pending(&mut self) {
        let rendered = self.cache.turn_starts.len();
        if rendered == self.turns.len() && self.first_stale_turn == NOTHING_STALE {
            return;
        }
        let restart = self.restart_turn();
        let mut shown_day = match self.cache.day_before_turn.get(restart) {
            Some(day) => day.clone(),
            None => self.cache.shown_day.clone(),
        };
        if restart < rendered {
            self.cache.lines.truncate(self.cache.turn_starts[restart]);
            self.cache.sync_links();
            self.cache.turn_starts.truncate(restart);
            self.cache.day_before_turn.truncate(restart);
        }
        if restart == 0 {
            self.cache.lines.clear();
            self.cache.lines.push(self.earlier_line());
            self.cache.lines.push(Line::default());
            self.cache.links.clear();
            self.cache.sync_links();
        }
        let mut hidden_tool_calls = 0;
        for turn in &self.turns[restart..] {
            render_cached_turn(
                &mut self.cache,
                turn,
                &self.markdown_theme,
                &self.cwd,
                &mut shown_day,
                &mut hidden_tool_calls,
            );
        }
        push_tool_run_summary(&mut self.cache.lines, &mut hidden_tool_calls);
        self.cache.sync_links();
        self.cache.shown_day = shown_day;
        self.first_stale_turn = NOTHING_STALE;
    }

    fn splice_prepended(&mut self, prepend: &Prepend) {
        let old = std::mem::take(&mut self.cache);
        let old_end = prepend.turns + old.turn_starts.len();
        let old_line_count = old.lines.len();
        self.cache = Cache {
            width: old.width,
            level: old.level,
            lines: vec![self.earlier_line(), Line::default()],
            links: vec![Vec::new(), Vec::new()],
            ..Cache::default()
        };
        let mut shown_day = None;
        let mut hidden_tool_calls = 0;
        let mut turn = 0;
        while turn < old_end {
            if turn >= prepend.rerender_until
                && self.state_matches_old(&old, turn, prepend.turns, &shown_day)
            {
                break;
            }
            render_cached_turn(
                &mut self.cache,
                &self.turns[turn],
                &self.markdown_theme,
                &self.cwd,
                &mut shown_day,
                &mut hidden_tool_calls,
            );
            turn += 1;
        }
        if turn == old_end {
            push_tool_run_summary(&mut self.cache.lines, &mut hidden_tool_calls);
            self.cache.sync_links();
            self.cache.shown_day = shown_day;
            self.prepend_shift += self.cache.lines.len() as isize - old_line_count as isize;
            return;
        }
        let reused = turn - prepend.turns;
        let old_start = old.turn_starts[reused];
        let delta = self.cache.lines.len() as isize - old_start as isize;
        let mut old_lines = old.lines;
        let mut old_links = old.links;
        self.cache.sync_links();
        self.cache.lines.extend(old_lines.drain(old_start..));
        self.cache.links.extend(old_links.drain(old_start..));
        self.cache.turn_starts.extend(
            old.turn_starts[reused..]
                .iter()
                .map(|start| start.wrapping_add_signed(delta)),
        );
        self.cache
            .day_before_turn
            .extend_from_slice(&old.day_before_turn[reused..]);
        self.cache.shown_day = old.shown_day;
        self.prepend_shift += self.cache.lines.len() as isize - old_line_count as isize;
    }

    fn state_matches_old(
        &self,
        old: &Cache,
        turn: usize,
        prepended: usize,
        shown_day: &Option<String>,
    ) -> bool {
        let reused = turn - prepended;
        let no_hidden_calls = self.cache.level != Level::Chat || !self.turns[turn - 1].quiet;
        let last_line_blank = |lines: &[Line<'static>]| lines.last().is_none_or(is_blank);
        no_hidden_calls
            && *shown_day == old.day_before_turn[reused]
            && last_line_blank(&self.cache.lines)
                == last_line_blank(&old.lines[..old.turn_starts[reused]])
    }

    fn settle_prepend_shift(&mut self) {
        let turns = std::mem::take(&mut self.prepended_turns);
        if turns == 0 {
            return;
        }
        let start = self
            .cache
            .turn_starts
            .get(turns)
            .copied()
            .unwrap_or(self.cache.lines.len());
        self.prepend_shift += start.saturating_sub(HEADER_LINES) as isize;
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

fn render_cached_turn(
    cache: &mut Cache,
    turn: &Turn,
    markdown_theme: &markdown::Theme,
    cwd: &str,
    shown_day: &mut Option<String>,
    hidden_tool_calls: &mut usize,
) {
    cache.turn_starts.push(cache.lines.len());
    cache.day_before_turn.push(shown_day.clone());
    if cache.level == Level::Chat {
        if turn.quiet {
            *hidden_tool_calls += turn.tool_calls;
            return;
        }
        push_tool_run_summary(&mut cache.lines, hidden_tool_calls);
    }
    render_turn(
        Output {
            lines: &mut cache.lines,
            links: &mut cache.links,
        },
        turn,
        cache.level,
        cache.width,
        markdown_theme,
        cwd,
        shown_day,
    );
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
    fn markdown_links_and_bare_urls_get_exact_ranges_and_urls() {
        let text = "see [the docs](https://a.example/x_y) and https://b.example/p?q=1, then (https://c.example/f_(1)). end";
        let mut conversation = Conversation::default();
        conversation.replace_messages(
            &[message(
                Role::Assistant,
                false,
                vec![Block::Text(text.into())],
            )],
            None,
        );
        let lines: Vec<String> = conversation
            .lines(200, Level::Chat)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let row = lines.iter().position(|l| l.contains("the docs")).unwrap();
        let column = |needle: &str| {
            let byte = lines[row].find(needle).unwrap();
            lines[row][..byte].chars().count()
        };
        let found: Vec<(std::ops::Range<usize>, &str)> = conversation.cache.links[row]
            .iter()
            .map(|link| (link.columns.clone(), &*link.url))
            .collect();
        let range = |needle: &str| column(needle)..column(needle) + needle.len();
        assert_eq!(
            found,
            [
                (range("the docs"), "https://a.example/x_y"),
                (range("https://b.example/p?q=1"), "https://b.example/p?q=1"),
                (range("https://c.example/f_(1)"), "https://c.example/f_(1)"),
            ]
        );
    }

    #[test]
    fn urls_inside_fenced_code_blocks_and_tool_results_are_links() {
        let in_code = "https://code.example/a_b";
        let in_result = "https://tool.example/r?x=1";
        let mut conversation = Conversation::default();
        conversation.replace_messages(
            &[
                message(
                    Role::Assistant,
                    false,
                    vec![
                        Block::Text(format!("```\nrun {in_code} now\n```")),
                        call("Bash", json!({"command": "ls"})),
                    ],
                ),
                message(
                    Role::User,
                    false,
                    vec![result(&format!("see {in_result} here"))],
                ),
            ],
            None,
        );
        let lines = rendered(&mut conversation, Level::Tools);
        for url in [in_code, in_result] {
            let row = lines.iter().position(|l| l.contains(url)).unwrap();
            let column = lines[row][..lines[row].find(url).unwrap()].chars().count();
            let found: Vec<(std::ops::Range<usize>, &str)> = conversation.cache.links[row]
                .iter()
                .map(|link| (link.columns.clone(), &*link.url))
                .collect();
            assert_eq!(found, [(column..column + url.len(), url)], "{url}");
        }
    }

    #[test]
    fn no_line_is_wider_than_the_pane_at_any_level() {
        let wide = "漢字漢字漢字漢字漢字漢字漢字漢字 🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂";
        let long_result = format!("{wide}\n{}", "x".repeat(200));
        let messages = vec![
            message(Role::User, false, vec![Block::Text(wide.into())]),
            message(Role::User, true, vec![Block::Text(wide.into())]),
            message(
                Role::Assistant,
                false,
                vec![
                    Block::Thinking(wide.into()),
                    Block::Text(format!(
                        "{wide}\n\n- {wide}\n\n```\n{}\n```",
                        "y".repeat(200)
                    )),
                    call("Bash", json!({"command": format!("echo {wide}\n{wide}")})),
                    call(
                        "Edit",
                        json!({"file_path": "/w/a.rs", "old_string": wide, "new_string": long_result}),
                    ),
                ],
            ),
            message(
                Role::User,
                false,
                vec![result(&long_result), result(&long_result)],
            ),
        ];
        for level in [Level::Chat, Level::Tools, Level::Everything] {
            let mut conversation = Conversation::default();
            conversation.push_messages(&messages);
            let widest = conversation
                .lines(24, level)
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
            assert!(widest <= 24, "{level:?}: {widest}");
        }
    }

    #[test]
    fn a_long_result_collapses_to_a_few_lines_and_never_reaches_chat() {
        let output: Vec<String> = (0..30).map(|i| format!("row-{i}")).collect();
        let mut conversation = Conversation::default();
        conversation.push_messages(&[
            message(Role::User, false, vec![Block::Text("go".into())]),
            message(
                Role::Assistant,
                false,
                vec![
                    Block::Text("running it".into()),
                    call("Bash", json!({"command": "seq"})),
                ],
            ),
            message(Role::User, false, vec![result(&output.join("\n"))]),
        ]);
        let tools = rendered(&mut conversation, Level::Tools);
        assert!(tools.iter().any(|l| l.contains("row-2")));
        assert!(!tools.iter().any(|l| l.contains("row-3")));
        assert!(tools.iter().any(|l| l.contains("… +27 lines")));
        let everything = rendered(&mut conversation, Level::Everything).join("\n");
        assert!(everything.contains("row-29") && !everything.contains("… +"));
        let chat = rendered(&mut conversation, Level::Chat).join("\n");
        assert!(chat.contains("running it") && !chat.contains("row-0") && !chat.contains("Bash"));
    }

    #[test]
    fn a_window_that_outgrows_the_cap_drops_its_oldest_turns_and_says_so() {
        let mut conversation = Conversation::default();
        conversation.replace_messages(&[], Some(Cursor::Offset(10)));
        let batch: Vec<Message> = (0..MAX_LOADED_TURNS + TRIM_SLACK + 10)
            .map(|index| {
                message(
                    Role::Assistant,
                    false,
                    vec![Block::Text(format!("msg {index}"))],
                )
            })
            .collect();
        conversation.push_messages(&batch);
        assert_eq!(conversation.turn_count(), MAX_LOADED_TURNS);
        assert!(conversation.earlier_cursor().is_none());
        let text = rendered(&mut conversation, Level::Chat);
        assert_eq!(text[0], "earlier messages not loaded");
        assert!(text.iter().any(|line| line.contains("msg 20509")));
        assert!(!text.iter().any(|line| line == "msg 0"));
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
    fn tools_shows_calls_with_their_results_and_collapsed_thinking_but_not_meta() {
        let mut conversation = Conversation::default();
        conversation.push_messages(&session());
        let text = rendered(&mut conversation, Level::Tools);
        let joined = text.join("\n");
        assert!(joined.contains("● Bash(ls)") && joined.contains("⎿  out-a"));
        assert!(joined.contains("● Bash(pwd)") && joined.contains("⎿  out-pwd"));
        assert!(joined.contains("● Read(/x)") && joined.contains("⎿  Read 2 lines"));
        assert!(joined.contains("∴ Thinking…") && joined.contains("pondering"));
        assert!(!joined.contains("injected"));
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

    fn stamped(role: Role, day: u32, text: &str, blocks: Vec<Block>) -> Message {
        let mut blocks = blocks;
        if !text.is_empty() {
            blocks.push(Block::Text(text.into()));
        }
        Message {
            role,
            timestamp: Some(format!("2026-03-{day:02}T12:00:00Z")),
            is_meta: false,
            blocks,
        }
    }

    fn history(count: usize) -> Vec<Message> {
        let mut messages = Vec::new();
        for index in 0..count {
            let day = 1 + (index / 7) as u32;
            messages.push(stamped(
                Role::User,
                day,
                &format!("question {index}"),
                vec![],
            ));
            messages.push(stamped(
                Role::Assistant,
                day,
                "",
                vec![call("Bash", json!({"command": format!("echo {index}")}))],
            ));
            messages.push(stamped(
                Role::User,
                day,
                "",
                vec![result(&format!("out {index}"))],
            ));
            messages.push(stamped(
                Role::Assistant,
                day,
                &format!("answer {index}"),
                vec![],
            ));
        }
        messages
    }

    #[test]
    fn prepending_a_page_renders_the_same_lines_as_loading_everything_at_once() {
        let messages = history(30);
        for level in [Level::Chat, Level::Tools, Level::Everything] {
            let mut whole = Conversation::default();
            whole.replace_messages(&messages, None);
            let expected = rendered(&mut whole, level);
            for split in [1, 2, 3, 6, 7, 50, 60, 118] {
                let mut paged = Conversation::default();
                paged.replace_messages(&messages[split..], Some(Cursor::Offset(1)));
                rendered(&mut paged, level);
                paged.prepend_messages(&messages[..split], None);
                assert_eq!(rendered(&mut paged, level), expected, "{level:?} {split}");
            }
        }
    }

    #[test]
    fn prepending_keeps_the_viewport_on_the_same_line_by_reporting_the_lines_added() {
        let messages = history(30);
        let split = 60;
        let mut conversation = Conversation::default();
        conversation.replace_messages(&messages[split..], Some(Cursor::Offset(1)));
        let before = rendered(&mut conversation, Level::Chat);
        for viewport_top in [4, 6, before.len() / 2, before.len() - 1] {
            let mut conversation = Conversation::default();
            conversation.replace_messages(&messages[split..], Some(Cursor::Offset(1)));
            rendered(&mut conversation, Level::Chat);
            conversation.prepend_messages(&messages[..split], None);
            let after = rendered(&mut conversation, Level::Chat);
            let shift = conversation.take_prepend_shift();
            assert_eq!(
                after[viewport_top.checked_add_signed(shift).unwrap()],
                before[viewport_top]
            );
            assert_eq!(conversation.take_prepend_shift(), 0);
        }
    }

    #[test]
    fn a_tool_result_whose_call_sits_on_the_previous_page_is_attached_to_it() {
        let messages = history(2);
        let mut whole = Conversation::default();
        whole.replace_messages(&messages, None);
        let expected = rendered(&mut whole, Level::Tools);

        let mut paged = Conversation::default();
        paged.replace_messages(&messages[2..], Some(Cursor::Offset(1)));
        rendered(&mut paged, Level::Tools);
        paged.prepend_messages(&messages[..2], None);
        let text = rendered(&mut paged, Level::Tools);
        assert_eq!(text, expected);
        assert!(text.join("\n").contains("⎿  out 0"));
    }
}
