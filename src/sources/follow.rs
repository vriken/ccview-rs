use crate::session::transcript::Message;
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

const TAIL_CHUNK: u64 = 1 << 20;
const FIRST_VIEW_MESSAGES: usize = 100;

struct Tail {
    messages: Vec<Message>,
    complete_end: u64,
    chunk: u64,
    origin: u64,
}

pub type LineParser = fn(&str) -> Result<Vec<Message>, serde_json::Error>;

pub struct Polled {
    pub messages: Vec<Message>,
    pub replaced: bool,
    pub dropped_earlier: usize,
}

pub struct JsonlFollower {
    path: PathBuf,
    parse_line: LineParser,
    offset: u64,
    inode: Option<u64>,
    partial_line: Vec<u8>,
    backfill: Option<u64>,
    replace_pending: bool,
    pub malformed_lines: usize,
}

impl JsonlFollower {
    pub fn new(path: impl Into<PathBuf>, parse_line: LineParser) -> Self {
        Self {
            path: path.into(),
            parse_line,
            offset: 0,
            inode: None,
            partial_line: Vec::new(),
            backfill: None,
            replace_pending: true,
            malformed_lines: 0,
        }
    }

    pub fn poll(&mut self) -> io::Result<Vec<Message>> {
        Ok(self
            .poll_retaining(usize::MAX, &AtomicBool::new(false))?
            .messages)
    }

    pub fn poll_retaining(&mut self, retain: usize, cancel: &AtomicBool) -> io::Result<Polled> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let replaced = std::mem::take(&mut self.replace_pending);
                self.reset(None);
                self.replace_pending = true;
                return Ok(Polled {
                    messages: Vec::new(),
                    replaced,
                    dropped_earlier: 0,
                });
            }
            Err(err) => return Err(err),
        };
        let metadata = file.metadata()?;
        if self.inode != Some(metadata.ino()) || metadata.len() < self.offset {
            self.reset(Some(metadata.ino()));
        }
        if self.replace_pending
            && self.offset == 0
            && retain != usize::MAX
            && metadata.len() > TAIL_CHUNK
        {
            self.offset = metadata.len();
            self.replace_pending = false;
            return self.first_view(&mut file, metadata.len(), retain, cancel);
        }
        if self.backfill.is_some() {
            return self.continue_backfill(&mut file, retain, cancel);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut appended = Vec::new();
        file.read_to_end(&mut appended)?;
        self.offset += appended.len() as u64;
        if self.partial_line.is_empty() {
            self.partial_line = appended;
        } else {
            self.partial_line.extend_from_slice(&appended);
        }
        let replaced = std::mem::take(&mut self.replace_pending);
        let retain = if replaced { retain } else { usize::MAX };
        let (messages, dropped_earlier) = self.drain_complete_lines(retain, cancel);
        Ok(Polled {
            messages,
            replaced,
            dropped_earlier,
        })
    }

    fn reset(&mut self, inode: Option<u64>) {
        self.offset = 0;
        self.inode = inode;
        self.partial_line.clear();
        self.backfill = None;
        self.replace_pending = true;
    }

    pub fn backfilling(&self) -> bool {
        self.backfill.is_some()
    }

    fn first_view(
        &mut self,
        file: &mut File,
        end: u64,
        retain: usize,
        cancel: &AtomicBool,
    ) -> io::Result<Polled> {
        let mut chunk = TAIL_CHUNK;
        let complete_end = loop {
            let low = end.saturating_sub(chunk);
            let window = read_range(file, low, end)?;
            match window.iter().rposition(|&b| b == b'\n') {
                Some(last_newline) => {
                    self.partial_line = window[last_newline + 1..].to_vec();
                    break low + last_newline as u64 + 1;
                }
                None if low == 0 => {
                    self.partial_line = window;
                    return Ok(Polled {
                        messages: Vec::new(),
                        replaced: true,
                        dropped_earlier: 0,
                    });
                }
                None => chunk *= 2,
            }
        };
        let tail = Tail {
            messages: Vec::new(),
            complete_end,
            chunk,
            origin: complete_end,
        };
        self.grow_tail(file, tail, FIRST_VIEW_MESSAGES.min(retain), retain, cancel)
    }

    fn continue_backfill(
        &mut self,
        file: &mut File,
        retain: usize,
        cancel: &AtomicBool,
    ) -> io::Result<Polled> {
        let origin = self.backfill.take().expect("backfill pending");
        let tail = Tail {
            messages: Vec::new(),
            complete_end: origin,
            chunk: TAIL_CHUNK,
            origin,
        };
        self.grow_tail(file, tail, retain, retain, cancel)
    }

    fn grow_tail(
        &mut self,
        file: &mut File,
        mut tail: Tail,
        want: usize,
        retain: usize,
        cancel: &AtomicBool,
    ) -> io::Result<Polled> {
        loop {
            let low = tail.complete_end.saturating_sub(tail.chunk);
            let window = read_range(file, low, tail.complete_end)?;
            let first_line = if low == 0 {
                Some(0)
            } else {
                window.iter().position(|&b| b == b'\n').map(|i| i + 1)
            };
            let Some(first_line) = first_line else {
                tail.chunk *= 2;
                continue;
            };
            let (mut older, _) = self.parse_lines(&window[first_line..], usize::MAX, cancel);
            older.append(&mut tail.messages);
            tail.messages = older;
            let reached_start = low == 0;
            let cancelled = cancel.load(Ordering::Relaxed);
            if reached_start || cancelled || tail.messages.len() >= want {
                let excess = tail.messages.len().saturating_sub(retain);
                if !reached_start && !cancelled && tail.messages.len() < retain {
                    self.backfill = Some(tail.origin);
                }
                tail.messages.drain(..excess);
                return Ok(Polled {
                    messages: tail.messages,
                    replaced: true,
                    dropped_earlier: excess + usize::from(!reached_start),
                });
            }
            tail.complete_end = low + first_line as u64;
            tail.chunk *= 2;
        }
    }

    fn drain_complete_lines(
        &mut self,
        retain: usize,
        cancel: &AtomicBool,
    ) -> (Vec<Message>, usize) {
        let Some(last_newline) = self.partial_line.iter().rposition(|&b| b == b'\n') else {
            return (Vec::new(), 0);
        };
        let remainder = self.partial_line.split_off(last_newline + 1);
        let complete = std::mem::replace(&mut self.partial_line, remainder);
        self.parse_lines(&complete, retain, cancel)
    }

    fn parse_lines(
        &mut self,
        complete: &[u8],
        retain: usize,
        cancel: &AtomicBool,
    ) -> (Vec<Message>, usize) {
        let mut messages = Vec::new();
        let mut dropped = 0;
        for line in String::from_utf8_lossy(complete).lines() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if line.trim().is_empty() {
                continue;
            }
            match (self.parse_line)(line) {
                Ok(parsed) => messages.extend(parsed),
                Err(_) => self.malformed_lines += 1,
            }
            if messages.len() >= retain.saturating_mul(2) {
                let excess = messages.len() - retain;
                messages.drain(..excess);
                dropped += excess;
            }
        }
        let excess = messages.len().saturating_sub(retain);
        messages.drain(..excess);
        (messages, dropped + excess)
    }
}

fn read_range(file: &mut File, from: u64, to: u64) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0; (to - from) as usize];
    file.seek(SeekFrom::Start(from))?;
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn parse(line: &str) -> Result<Vec<Message>, serde_json::Error> {
        crate::sources::claude::transcript::parse_line(line)
            .map(|message| message.into_iter().collect())
    }

    fn transcript(name: &str, lines: usize) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ccview-sfollow-{name}-{}.jsonl",
            std::process::id()
        ));
        let line = r#"{"type":"user","message":{"content":"hi"}}"#;
        fs::write(&path, format!("{line}\n").repeat(lines)).unwrap();
        path
    }

    #[test]
    fn an_initial_load_keeps_only_the_latest_messages_and_counts_the_rest() {
        let path = transcript("retain", 50);
        let polled = JsonlFollower::new(&path, parse)
            .poll_retaining(10, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            (
                polled.messages.len(),
                polled.dropped_earlier,
                polled.replaced
            ),
            (10, 40, true)
        );
        fs::remove_file(path).unwrap();
    }

    fn record(role: &str, text: &str) -> String {
        format!("{{\"type\":\"{role}\",\"message\":{{\"content\":\"{text}\"}}}}\n")
    }

    fn texts(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .flat_map(|message| &message.blocks)
            .map(|block| format!("{block:?}"))
            .collect()
    }

    fn big_transcript(name: &str) -> (PathBuf, String) {
        let path = std::env::temp_dir().join(format!(
            "ccview-sfollow-{name}-{}.jsonl",
            std::process::id()
        ));
        let garbage = "not json at all\n".repeat(3 * (TAIL_CHUNK as usize) / 16);
        let call = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"listing"}]}}"#;
        let filler = "x".repeat(300 * 1024);
        let mut body = garbage;
        body.push_str(&format!("{call}\n"));
        for index in 0..5 {
            body.push_str(&record("assistant", &format!("{filler}{index}")));
        }
        body.push_str(&format!("{result}\n"));
        for index in 0..3 {
            body.push_str(&record("user", &format!("tail {index}")));
        }
        fs::write(&path, &body).unwrap();
        (path, body)
    }

    #[test]
    fn a_large_transcript_loads_its_latest_messages_without_reading_the_early_part() {
        let (path, body) = big_transcript("tail-first");
        let valid_from = body.find('{').unwrap();
        let valid =
            std::env::temp_dir().join(format!("ccview-sfollow-valid-{}.jsonl", std::process::id()));
        fs::write(&valid, &body[valid_from..]).unwrap();
        let expected = JsonlFollower::new(&valid, parse)
            .poll_retaining(6, &AtomicBool::new(false))
            .unwrap();

        let mut follower = JsonlFollower::new(&path, parse);
        let polled = follower.poll_retaining(6, &AtomicBool::new(false)).unwrap();
        while follower.backfilling() {
            follower.poll_retaining(6, &AtomicBool::new(false)).unwrap();
        }
        assert_eq!(texts(&polled.messages), texts(&expected.messages));
        assert!(polled.dropped_earlier > 0);
        assert_eq!(follower.malformed_lines, 0);
        fs::remove_file(path).unwrap();
        fs::remove_file(valid).unwrap();
    }

    #[test]
    fn following_after_a_tail_first_load_appends_each_new_record_once() {
        let path =
            std::env::temp_dir().join(format!("ccview-sfollow-grow-{}.jsonl", std::process::id()));
        let filler = "x".repeat(20 * 1024);
        let body: String = (0..400)
            .map(|index| record("assistant", &format!("{filler}{index}")))
            .collect();
        fs::write(&path, body).unwrap();
        let cancel = AtomicBool::new(false);
        let mut follower = JsonlFollower::new(&path, parse);

        let first = follower.poll_retaining(5000, &cancel).unwrap();
        assert!(follower.backfilling() && first.replaced);
        assert!(first.messages.len() < 400);

        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(&mut file, record("user", "new one").as_bytes()).unwrap();
        std::io::Write::write_all(&mut file, record("user", "new two").as_bytes()).unwrap();

        let backfilled = follower.poll_retaining(5000, &cancel).unwrap();
        assert!(backfilled.replaced);
        assert_eq!(backfilled.messages.len(), 400);
        assert!(!follower.backfilling());

        let appended = follower.poll_retaining(5000, &cancel).unwrap();
        assert!(!appended.replaced);
        let appended = texts(&appended.messages);
        assert_eq!(appended.len(), 2);
        assert!(appended[0].contains("new one") && appended[1].contains("new two"));
        assert!(
            follower
                .poll_retaining(5000, &cancel)
                .unwrap()
                .messages
                .is_empty()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn truncation_after_a_tail_first_load_replaces_instead_of_finishing_the_old_backfill() {
        let path =
            std::env::temp_dir().join(format!("ccview-sfollow-trunc-{}.jsonl", std::process::id()));
        let filler = "x".repeat(20 * 1024);
        let body: String = (0..400)
            .map(|index| record("assistant", &format!("{filler}{index}")))
            .collect();
        fs::write(&path, body).unwrap();
        let cancel = AtomicBool::new(false);
        let mut follower = JsonlFollower::new(&path, parse);
        follower.poll_retaining(5000, &cancel).unwrap();
        assert!(follower.backfilling());

        fs::write(&path, record("user", "fresh start")).unwrap();
        let polled = follower.poll_retaining(5000, &cancel).unwrap();
        assert!(polled.replaced);
        let polled = texts(&polled.messages);
        assert_eq!(polled.len(), 1);
        assert!(polled[0].contains("fresh start"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_superseded_load_stops_before_parsing_the_file() {
        let path = transcript("cancel", 50);
        let polled = JsonlFollower::new(&path, parse)
            .poll_retaining(usize::MAX, &AtomicBool::new(true))
            .unwrap();
        assert!(polled.messages.is_empty());
        fs::remove_file(path).unwrap();
    }
}
