use crate::parse::{self, Message};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::PathBuf,
};

pub struct Tailer {
    path: PathBuf,
    offset: u64,
    inode: Option<u64>,
    partial_line: Vec<u8>,
    pub malformed_lines: usize,
}

impl Tailer {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
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
            match parse::parse_line(line) {
                Ok(message) => messages.extend(message),
                Err(_) => self.malformed_lines += 1,
            }
        }
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};

    const LINE: &str = r#"{"type":"user","message":{"content":"hi"}}"#;

    #[test]
    fn buffers_partial_line_and_rereads_after_truncation() {
        let path = std::env::temp_dir().join(format!("ccview-tail-{}.jsonl", std::process::id()));
        let mut tailer = Tailer::new(&path);
        assert!(tailer.poll().unwrap().is_empty());

        let mut file = File::create(&path).unwrap();
        write!(file, "{LINE}\n{}", &LINE[..10]).unwrap();
        assert_eq!(tailer.poll().unwrap().len(), 1);
        assert!(tailer.poll().unwrap().is_empty());

        writeln!(file, "{}", &LINE[10..]).unwrap();
        assert_eq!(tailer.poll().unwrap().len(), 1);

        fs::write(&path, format!("{LINE}\n")).unwrap();
        assert_eq!(tailer.poll().unwrap().len(), 1);
        fs::remove_file(&path).unwrap();
    }
}
