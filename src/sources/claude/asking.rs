use serde_json::Value;

const USER_TYPE_MARKER: &[u8] = br#""type":"user""#;
const ASSISTANT_TYPE_MARKER: &[u8] = br#""type":"assistant""#;
const TOOL_RESULT_MARKER: &[u8] = b"\"tool_result\"";
const SIDECHAIN_MARKER: &[u8] = br#""isSidechain":true"#;
const META_MARKER: &[u8] = br#""isMeta":true"#;
const END_TURN_MARKER: &[u8] = br#""stop_reason":"end_turn""#;
const HANDOFF_PREFIXES: [&str; 4] = [
    "decision for you:",
    "your step:",
    "next step, yours",
    "handoff for you",
];
const FENCE: &str = "```";
const LINE_DECORATION: &[char] = &['*', '_', '>', '#', '-', ' ', '\t'];

pub(crate) fn asks_user_after(line: &[u8], current: bool) -> bool {
    if contains(line, SIDECHAIN_MARKER) {
        return current;
    }
    if contains(line, USER_TYPE_MARKER) {
        if contains(line, TOOL_RESULT_MARKER) || contains(line, META_MARKER) {
            return current;
        }
        return false;
    }
    if contains(line, ASSISTANT_TYPE_MARKER) {
        return contains(line, END_TURN_MARKER) && final_text(line).is_some_and(|t| asks(&t));
    }
    current
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    super::scan::contains_bytes(haystack, needle)
}

fn final_text(line: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let blocks = value.get("message")?.get("content")?.as_array()?;
    let last = blocks.last()?;
    if last.get("type")?.as_str()? != "text" {
        return None;
    }
    last.get("text")?.as_str().map(str::to_owned)
}

fn asks(text: &str) -> bool {
    let prose = without_fenced_code(text);
    let Some(paragraph) = prose.rsplit("\n\n").find(|p| !p.trim().is_empty()) else {
        return false;
    };
    paragraph.lines().any(starts_with_handoff) || has_question_mark(paragraph)
}

fn starts_with_handoff(line: &str) -> bool {
    let line = line.trim_start_matches(LINE_DECORATION).to_lowercase();
    HANDOFF_PREFIXES
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

fn without_fenced_code(text: &str) -> String {
    let mut inside = false;
    let mut kept = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with(FENCE) {
            inside = !inside;
            if !kept
                .last()
                .is_some_and(|last: &&str| last.trim().is_empty())
            {
                kept.push("");
            }
        } else if !inside {
            kept.push(line);
        }
    }
    kept.join("\n").replace("\n\n\n", "\n\n")
}

fn has_question_mark(paragraph: &str) -> bool {
    paragraph
        .split('`')
        .step_by(2)
        .flat_map(str::split_whitespace)
        .filter(|word| !word.contains("://"))
        .any(|word| word.contains('?'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> String {
        format!(r#"{{"type":"user","message":{{"role":"user","content":"{text}"}}}}"#)
    }

    fn tool_result() -> String {
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#.to_owned()
    }

    fn assistant(stop_reason: &str, text: &str) -> String {
        let text = text.replace('\n', "\\n");
        format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","stop_reason":"{stop_reason}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    fn asks_after(lines: &[String]) -> bool {
        lines.iter().fold(false, |current, line| {
            asks_user_after(line.as_bytes(), current)
        })
    }

    #[test]
    fn a_turn_ending_on_a_question_asks_until_the_user_replies() {
        let cases: [(&str, Vec<String>, bool); 12] = [
            (
                "question in the last paragraph",
                vec![assistant(
                    "end_turn",
                    "Done.\n\nShould I go ahead with the rename? Say so.",
                )],
                true,
            ),
            (
                "question only in an earlier paragraph",
                vec![assistant(
                    "end_turn",
                    "Why did this fail?\n\nIt was the cache.",
                )],
                false,
            ),
            (
                "question mark only inside a fenced block",
                vec![assistant(
                    "end_turn",
                    "Run this:\n\n```\nfirst\n\nselect a ? b\n```",
                )],
                false,
            ),
            (
                "question mark only inside a code span or url",
                vec![assistant(
                    "end_turn",
                    "Use `a ? b` or see https://example.test/x?y=1 for more.",
                )],
                false,
            ),
            (
                "decision prefix without a question mark",
                vec![assistant(
                    "end_turn",
                    "Checked.\n\n**Decision for you:** ship it or hold.",
                )],
                true,
            ),
            (
                "your step convention without a question mark",
                vec![assistant(
                    "end_turn",
                    "Deployed to staging.\n\n_Your step:_ press merge on the release PR.",
                )],
                true,
            ),
            (
                "next step yours convention, any case",
                vec![assistant(
                    "end_turn",
                    "All checks pass.\n\n> NEXT STEP, YOURS: approve the rollout.",
                )],
                true,
            ),
            (
                "handoff for you convention",
                vec![assistant(
                    "end_turn",
                    "Done.\n\n- Handoff for you: rotate the key.",
                )],
                true,
            ),
            (
                "offer and status report without a question mark",
                vec![assistant(
                    "end_turn",
                    "Tests pass.\n\nIf you'd rather keep the old name, I can revert. Nothing else is pending.",
                )],
                false,
            ),
            (
                "stop reason other than end_turn",
                vec![assistant("tool_use", "Want me to continue?")],
                false,
            ),
            (
                "user message after",
                vec![assistant("end_turn", "Want me to continue?"), user("yes")],
                false,
            ),
            (
                "tool result after does not count as a reply",
                vec![assistant("end_turn", "Want me to continue?"), tool_result()],
                true,
            ),
        ];
        for (name, lines, expected) in cases {
            assert_eq!(asks_after(&lines), expected, "{name}");
        }
    }
}
