use crate::{
    parse::{self, Message},
    sources::follow::JsonlFollower,
};
use std::path::PathBuf;

pub fn tailer(path: impl Into<PathBuf>) -> JsonlFollower {
    JsonlFollower::new(path, parse_claude_line)
}

fn parse_claude_line(line: &str) -> Result<Vec<Message>, serde_json::Error> {
    parse::parse_line(line).map(|message| message.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, File},
        io::Write,
    };

    const LINE: &str = r#"{"type":"user","message":{"content":"hi"}}"#;

    #[test]
    fn buffers_partial_line_and_rereads_after_truncation() {
        let path = std::env::temp_dir().join(format!("ccview-tail-{}.jsonl", std::process::id()));
        let mut tailer = tailer(&path);
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
