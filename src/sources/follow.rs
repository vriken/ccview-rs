use crate::parse::Message;
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

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
            replace_pending: true,
            malformed_lines: 0,
        }
    }

    pub fn poll(&mut self) -> io::Result<Vec<Message>> {
        Ok(self.poll_retaining(usize::MAX, &AtomicBool::new(false))?.messages)
    }

    pub fn poll_retaining(&mut self, retain: usize, cancel: &AtomicBool) -> io::Result<Polled> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let replaced = std::mem::take(&mut self.replace_pending);
                self.reset(None);
                self.replace_pending = true;
                return Ok(Polled { messages: Vec::new(), replaced, dropped_earlier: 0 });
            }
            Err(err) => return Err(err),
        };
        let metadata = file.metadata()?;
        if self.inode != Some(metadata.ino()) || metadata.len() < self.offset {
            self.reset(Some(metadata.ino()));
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
        Ok(Polled { messages, replaced, dropped_earlier })
    }

    fn reset(&mut self, inode: Option<u64>) {
        self.offset = 0;
        self.inode = inode;
        self.partial_line.clear();
        self.replace_pending = true;
    }

    fn drain_complete_lines(&mut self, retain: usize, cancel: &AtomicBool) -> (Vec<Message>, usize) {
        let Some(last_newline) = self.partial_line.iter().rposition(|&b| b == b'\n') else {
            return (Vec::new(), 0);
        };
        let remainder = self.partial_line.split_off(last_newline + 1);
        let complete = std::mem::replace(&mut self.partial_line, remainder);
        let mut messages = Vec::new();
        let mut dropped = 0;
        for line in String::from_utf8_lossy(&complete).lines() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn parse(line: &str) -> Result<Vec<Message>, serde_json::Error> {
        crate::parse::parse_line(line).map(|message| message.into_iter().collect())
    }

    fn transcript(name: &str, lines: usize) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ccview-sfollow-{name}-{}.jsonl", std::process::id()));
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
        assert_eq!((polled.messages.len(), polled.dropped_earlier, polled.replaced), (10, 40, true));
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
