use crate::parse::Message;
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::PathBuf,
};

pub type LineParser = fn(&str) -> Result<Vec<Message>, serde_json::Error>;

pub struct JsonlFollower {
    path: PathBuf,
    parse_line: LineParser,
    offset: u64,
    inode: Option<u64>,
    partial_line: Vec<u8>,
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
            malformed_lines: 0,
        }
    }

    pub fn poll(&mut self) -> io::Result<Vec<Message>> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                self.reset(None);
                return Ok(Vec::new());
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
        self.partial_line.extend_from_slice(&appended);
        Ok(self.drain_complete_lines())
    }

    fn reset(&mut self, inode: Option<u64>) {
        self.offset = 0;
        self.inode = inode;
        self.partial_line.clear();
    }

    fn drain_complete_lines(&mut self) -> Vec<Message> {
        let Some(last_newline) = self.partial_line.iter().rposition(|&b| b == b'\n') else {
            return Vec::new();
        };
        let remainder = self.partial_line.split_off(last_newline + 1);
        let complete = std::mem::replace(&mut self.partial_line, remainder);
        let mut messages = Vec::new();
        for line in String::from_utf8_lossy(&complete).lines() {
            if line.trim().is_empty() {
                continue;
            }
            match (self.parse_line)(line) {
                Ok(parsed) => messages.extend(parsed),
                Err(_) => self.malformed_lines += 1,
            }
        }
        messages
    }
}
