use crate::terminal::Snapshot;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

const PROMPT_GLYPHS: &[char] = &['>', '❯', '›', '»', '|', '$', '%', '#'];
const BOX_DRAWING: std::ops::RangeInclusive<char> = '\u{2500}'..='\u{257f}';

pub(crate) fn left_is_noop(cursor_visible: bool, left_of_cursor: &str) -> bool {
    cursor_visible
        && left_of_cursor
            .chars()
            .all(|c| c.is_whitespace() || PROMPT_GLYPHS.contains(&c) || BOX_DRAWING.contains(&c))
}

pub(crate) fn is_plain_left(key: KeyEvent) -> bool {
    key.code == KeyCode::Left && key.modifiers.is_empty() && key.kind != KeyEventKind::Release
}

pub(crate) fn snapshot_left_is_noop(snapshot: &Snapshot) -> bool {
    let Some((row, column)) = snapshot.cursor() else {
        return false;
    };
    let screen = snapshot.screen();
    let left: String = (0..column)
        .map(|col| {
            screen
                .cell(row, col)
                .map(|cell| cell.contents())
                .filter(|text| !text.is_empty())
                .unwrap_or(" ")
                .to_owned()
        })
        .collect();
    left_is_noop(true, &left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use std::num::NonZeroU16;

    #[test]
    fn left_is_a_noop_only_after_whitespace_prompt_glyphs_and_box_drawing() {
        let cases: &[(bool, &str, bool)] = &[
            (true, "", true),
            (true, "> ", true),
            (true, "│ > ", true),
            (true, "│ ❯ ", true),
            (true, "› ", true),
            (true, "┃  ", true),
            (true, "║ ", true),
            (true, "$ ", true),
            (true, "% ", true),
            (true, "> abc", false),
            (true, "│ > abc", false),
            (true, "~/proj $ ls", false),
            (true, "Do you want to proceed?", false),
            (false, "> ", false),
            (false, "", false),
        ];
        for &(visible, left, expected) in cases {
            assert_eq!(
                left_is_noop(visible, left),
                expected,
                "visible={visible} left={left:?}"
            );
        }
    }

    fn screen_after(output: &str) -> vt100::Parser {
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(10).unwrap(),
            NonZeroU16::new(40).unwrap(),
            0,
        );
        parser.process(output.as_bytes());
        parser
    }

    fn noop_on(output: &str) -> bool {
        let parser = screen_after(output);
        let screen = parser.screen();
        if screen.hide_cursor() {
            return false;
        }
        let (row, column) = screen.cursor_position();
        let left: String = (0..column)
            .map(|col| {
                screen
                    .cell(row, col)
                    .map(|c| c.contents())
                    .filter(|t| !t.is_empty())
                    .unwrap_or(" ")
                    .to_owned()
            })
            .collect();
        left_is_noop(true, &left)
    }

    #[test]
    fn real_prompt_layouts() {
        assert!(
            noop_on("\x1b[2J\x1b[H╭────────╮\r\n│ > "),
            "claude empty input in box"
        );
        assert!(
            !noop_on("\x1b[2J\x1b[H╭────────╮\r\n│ > abc"),
            "claude typed text"
        );
        assert!(noop_on("\x1b[2J\x1b[H› "), "codex empty");
        assert!(!noop_on("\x1b[2J\x1b[H› fix the bug"), "codex typed");
        assert!(noop_on("\x1b[2J\x1b[H┃  "), "opencode empty");
        assert!(!noop_on("\x1b[2J\x1b[H┃  hello"), "opencode typed");
        assert!(!noop_on("\x1b[?25l> "), "cursor hidden (menu or dialog)");
        assert!(
            noop_on("\x1b[2J\x1b[H│ > line one\r\n│   "),
            "continuation line start counts as at start"
        );
    }

    #[test]
    fn only_a_plain_left_press_qualifies() {
        let left = |modifiers, kind| KeyEvent::new_with_kind(KeyCode::Left, modifiers, kind);
        assert!(is_plain_left(left(KeyModifiers::NONE, KeyEventKind::Press)));
        assert!(!is_plain_left(left(
            KeyModifiers::SHIFT,
            KeyEventKind::Press
        )));
        assert!(!is_plain_left(left(
            KeyModifiers::NONE,
            KeyEventKind::Release
        )));
    }
}
