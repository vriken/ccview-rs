use crate::{session::pending_work::PendingWork, sources::millis_from_rfc3339};
use serde_json::Value;

const NOTIFICATION_ID_OPEN: &str = "<tool-use-id>";
const NOTIFICATION_ID_CLOSE: &str = "</tool-use-id>";
const NOTIFICATION_STATUS: &str = "<status>";
const CALL_HINTS: [&str; 5] = [
    "\"run_in_background\":true",
    "\"name\":\"Monitor\"",
    "\"name\":\"CronCreate\"",
    "\"name\":\"CronDelete\"",
    "\"name\":\"ScheduleWakeup\"",
];
const BACKGROUND_RESULTS: [&str; 4] = [
    "Async agent launched",
    "manually backgrounded",
    "running in background with ID",
    "moved to the background (ID:",
];
const RESULT_HEAD_CHARS: usize = 200;
const UNKNOWN_TIME_MS: u64 = u64::MAX;

pub(crate) fn fold_pending_work(line: &[u8], work: &mut PendingWork) {
    let Ok(text) = std::str::from_utf8(line) else {
        return;
    };
    close_notified(text, work);
    let hinted = CALL_HINTS
        .iter()
        .chain(BACKGROUND_RESULTS.iter())
        .any(|hint| text.contains(hint));
    if !hinted {
        return;
    }
    let Ok(record) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let at_ms = record
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(millis_from_rfc3339)
        .unwrap_or(UNKNOWN_TIME_MS);
    let Some(blocks) = record.pointer("/message/content").and_then(Value::as_array) else {
        return;
    };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => fold_call(block, at_ms, work),
            Some("tool_result") => fold_result(block, at_ms, work),
            _ => {}
        }
    }
}

fn close_notified(text: &str, work: &mut PendingWork) {
    if !text.contains(NOTIFICATION_STATUS) {
        return;
    }
    for (start, _) in text.match_indices(NOTIFICATION_ID_OPEN) {
        let after = &text[start + NOTIFICATION_ID_OPEN.len()..];
        if let Some(end) = after.find(NOTIFICATION_ID_CLOSE) {
            work.close_task(&after[..end]);
        }
    }
}

fn fold_call(block: &Value, at_ms: u64, work: &mut PendingWork) {
    let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
    let input = block.get("input").unwrap_or(&Value::Null);
    match block.get("name").and_then(Value::as_str) {
        Some("CronCreate") => work.cron_created(at_ms),
        Some("CronDelete") => work.cron_deleted(at_ms),
        Some("ScheduleWakeup") => fold_wakeup(input, at_ms, work),
        Some("Monitor") => work.open_task(id, at_ms),
        _ if input.get("run_in_background").and_then(Value::as_bool) == Some(true) => {
            work.open_task(id, at_ms)
        }
        _ => {}
    }
}

fn fold_result(block: &Value, at_ms: u64, work: &mut PendingWork) {
    let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
        return;
    };
    let Some(text) = result_text(block.get("content")) else {
        return;
    };
    let head: String = text.chars().take(RESULT_HEAD_CHARS).collect();
    if BACKGROUND_RESULTS
        .iter()
        .any(|phrase| head.contains(phrase))
    {
        work.open_task(id, at_ms);
    }
}

fn result_text(content: Option<&Value>) -> Option<&str> {
    match content? {
        Value::String(text) => Some(text),
        Value::Array(parts) => parts
            .iter()
            .find(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|part| part.get("text"))
            .and_then(Value::as_str),
        _ => None,
    }
}

fn fold_wakeup(input: &Value, at_ms: u64, work: &mut PendingWork) {
    if input.get("stop").and_then(Value::as_bool) == Some(true) {
        return work.set_wakeup(None);
    }
    if let Some(delay_seconds) = input.get("delaySeconds").and_then(Value::as_f64) {
        work.set_wakeup(Some(at_ms.saturating_add((delay_seconds * 1000.0) as u64)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAUNCH_MS: u64 = 4_102_444_800_000;
    const NOW_MS: u64 = LAUNCH_MS + 60_000;
    const AFTER_LAUNCH: &str = "2100-01-01T00:00:30.000Z";
    const BEFORE_LAUNCH: &str = "2099-12-31T23:00:00.000Z";

    fn call(at: &str, id: &str, name: &str, input: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
        )
    }

    fn result(at: &str, id: &str, text: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"{at}","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":"{text}"}}]}}}}"#
        )
    }

    fn notification(id: &str) -> String {
        format!(
            r#"{{"type":"queue-operation","operation":"enqueue","content":"<task-notification><task-id>t</task-id><tool-use-id>{id}</tool-use-id><status>completed</status></task-notification>"}}"#
        )
    }

    fn blocks(lines: &[String]) -> bool {
        let mut work = PendingWork::default();
        for line in lines {
            fold_pending_work(line.as_bytes(), &mut work);
        }
        work.blocks_restart(LAUNCH_MS, NOW_MS)
    }

    #[test]
    fn only_work_started_by_the_running_process_and_still_open_blocks_a_restart() {
        let cases = [
            (
                "recurring cron created after launch",
                vec![call(
                    AFTER_LAUNCH,
                    "c1",
                    "CronCreate",
                    r#"{"cron":"*/5 * * * *","prompt":"p","recurring":true}"#,
                )],
                true,
            ),
            (
                "cron created then deleted",
                vec![
                    call(
                        AFTER_LAUNCH,
                        "c1",
                        "CronCreate",
                        r#"{"cron":"*/5 * * * *","prompt":"p","recurring":true}"#,
                    ),
                    call(AFTER_LAUNCH, "c2", "CronDelete", r#"{"id":"j1"}"#),
                ],
                false,
            ),
            (
                "cron from an earlier process (copied history)",
                vec![call(
                    BEFORE_LAUNCH,
                    "c1",
                    "CronCreate",
                    r#"{"cron":"*/5 * * * *","prompt":"p","recurring":true}"#,
                )],
                false,
            ),
            (
                "wakeup due in the future",
                vec![call(
                    AFTER_LAUNCH,
                    "w1",
                    "ScheduleWakeup",
                    r#"{"delaySeconds":1200,"prompt":"p","reason":"r"}"#,
                )],
                true,
            ),
            (
                "wakeup loop stopped",
                vec![
                    call(
                        AFTER_LAUNCH,
                        "w1",
                        "ScheduleWakeup",
                        r#"{"delaySeconds":1200,"prompt":"p","reason":"r"}"#,
                    ),
                    call(AFTER_LAUNCH, "w2", "ScheduleWakeup", r#"{"stop":true}"#),
                ],
                false,
            ),
            (
                "monitor still running",
                vec![call(
                    AFTER_LAUNCH,
                    "m1",
                    "Monitor",
                    r#"{"command":"tail -f x"}"#,
                )],
                true,
            ),
            (
                "monitor finished",
                vec![
                    call(AFTER_LAUNCH, "m1", "Monitor", r#"{"command":"tail -f x"}"#),
                    notification("m1"),
                ],
                false,
            ),
            (
                "async agent launched, no notification yet",
                vec![
                    call(AFTER_LAUNCH, "a1", "Agent", r#"{"prompt":"p"}"#),
                    result(AFTER_LAUNCH, "a1", "Async agent launched successfully."),
                ],
                true,
            ),
            (
                "command the owner backgrounded by hand",
                vec![
                    call(AFTER_LAUNCH, "b1", "Bash", r#"{"command":"cargo test"}"#),
                    result(
                        AFTER_LAUNCH,
                        "b1",
                        "Command was manually backgrounded by user with ID: x.",
                    ),
                ],
                true,
            ),
            (
                "command moved to the background after its timeout",
                vec![
                    call(AFTER_LAUNCH, "b2", "Bash", r#"{"command":"cargo test"}"#),
                    result(
                        AFTER_LAUNCH,
                        "b2",
                        "Command did not complete within its 120s timeout and was moved to the background (ID: x).",
                    ),
                ],
                true,
            ),
            (
                "a file that merely mentions a background phrase",
                vec![
                    call(AFTER_LAUNCH, "r1", "Read", r#"{"file_path":"x"}"#),
                    result(
                        AFTER_LAUNCH,
                        "r1",
                        &format!("{}Async agent launched", "line of code ".repeat(40)),
                    ),
                ],
                false,
            ),
            (
                "a wakeup call without a delay keeps the pending one",
                vec![
                    call(
                        AFTER_LAUNCH,
                        "w1",
                        "ScheduleWakeup",
                        r#"{"delaySeconds":1200,"prompt":"p","reason":"r"}"#,
                    ),
                    call(AFTER_LAUNCH, "w2", "ScheduleWakeup", r#"{"prompt":"p"}"#),
                ],
                true,
            ),
            (
                "wakeup recorded without a timestamp",
                vec![r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"w1","name":"ScheduleWakeup","input":{"delaySeconds":60}}]}}"#.to_owned()],
                true,
            ),
            (
                "background task from an earlier process",
                vec![call(
                    BEFORE_LAUNCH,
                    "b1",
                    "Bash",
                    r#"{"command":"x","run_in_background":true}"#,
                )],
                false,
            ),
        ];
        for (name, lines, expected) in cases {
            assert_eq!(blocks(&lines), expected, "{name}");
        }
    }
}
