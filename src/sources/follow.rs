use crate::session::{cursor::Cursor, transcript::Message};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) type LineParser = fn(&str) -> Result<Vec<Message>, serde_json::Error>;

const FIRST_CHUNK: usize = 256 * 1024;
const MAX_CHUNK: usize = 4 * 1024 * 1024;
const BOUNDARY_SCAN_CHUNK: usize = 4 * 1024;
const FORWARD_BUFFER: usize = 1024 * 1024;

pub(crate) struct Page {
    pub(crate) messages: Vec<Message>,
    pub(crate) earlier: Option<Cursor>,
}

pub(crate) struct Polled {
    pub(crate) messages: Vec<Message>,
    pub(crate) replaced: bool,
    pub(crate) earlier: Option<Cursor>,
    pub(crate) landed: Option<bool>,
}

#[derive(Clone, Copy)]
pub(crate) struct Jump {
    pub(crate) offset: u64,
    pub(crate) before: usize,
    pub(crate) limit: usize,
}

pub(crate) struct JsonlFollower {
    path: PathBuf,
    parse_line: LineParser,
    offset: u64,
    inode: Option<u64>,
    partial_line: Vec<u8>,
    replace_pending: bool,
    jump: Option<Jump>,
}

impl JsonlFollower {
    pub(crate) fn new(path: impl Into<PathBuf>, parse_line: LineParser) -> Self {
        Self {
            path: path.into(),
            parse_line,
            offset: 0,
            inode: None,
            partial_line: Vec::new(),
            replace_pending: true,
            jump: None,
        }
    }

    pub(crate) fn jumping_to(mut self, jump: Jump) -> Self {
        self.jump = Some(jump);
        self
    }

    pub(crate) fn poll(&mut self, latest: usize, cancel: &AtomicBool) -> io::Result<Polled> {
        self.poll_after_recording_end(latest, cancel, &mut || {})
    }

    fn poll_after_recording_end(
        &mut self,
        latest: usize,
        cancel: &AtomicBool,
        end_recorded: &mut dyn FnMut(),
    ) -> io::Result<Polled> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let replaced = std::mem::take(&mut self.replace_pending);
                self.reset(None);
                self.replace_pending = true;
                return Ok(Polled {
                    messages: Vec::new(),
                    replaced,
                    earlier: None,
                    landed: None,
                });
            }
            Err(err) => return Err(err),
        };
        let metadata = file.metadata()?;
        if self.inode != Some(metadata.ino()) || metadata.len() < self.offset {
            self.reset(Some(metadata.ino()));
        }
        let replaced = std::mem::take(&mut self.replace_pending);
        let mut messages = Vec::new();
        let mut earlier = None;
        let mut landed = None;
        if replaced {
            let end = metadata.len();
            end_recorded();
            let resume = last_line_boundary(&mut file, end)?;
            let window = match self.jump.take() {
                Some(jump) => {
                    let window = read_window(&mut file, resume, jump, self.parse_line, cancel)?;
                    landed = Some(window.is_some());
                    window
                }
                None => None,
            };
            let page = match window {
                Some(page) => page,
                None => read_page(&mut file, resume, latest, self.parse_line, cancel)?,
            };
            self.offset = resume;
            messages = page.messages;
            earlier = page.earlier;
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
        messages.extend(self.drain_complete_lines(cancel));
        Ok(Polled {
            messages,
            replaced,
            earlier,
            landed,
        })
    }

    pub(crate) fn earlier(
        &self,
        before: &Cursor,
        count: usize,
        cancel: &AtomicBool,
    ) -> io::Result<Page> {
        let unchanged = Page {
            messages: Vec::new(),
            earlier: Some(before.clone()),
        };
        let Cursor::Offset(end) = *before else {
            return Ok(unchanged);
        };
        let mut file = File::open(&self.path)?;
        let metadata = file.metadata()?;
        if self.inode != Some(metadata.ino()) || end > metadata.len() {
            return Ok(unchanged);
        }
        read_page(&mut file, end, count, self.parse_line, cancel)
    }

    fn reset(&mut self, inode: Option<u64>) {
        self.offset = 0;
        self.inode = inode;
        self.partial_line.clear();
        self.replace_pending = true;
    }

    fn drain_complete_lines(&mut self, cancel: &AtomicBool) -> Vec<Message> {
        let Some(last_newline) = self.partial_line.iter().rposition(|&b| b == b'\n') else {
            return Vec::new();
        };
        let mut line_start = self.offset - self.partial_line.len() as u64;
        let remainder = self.partial_line.split_off(last_newline + 1);
        let complete = std::mem::replace(&mut self.partial_line, remainder);
        let mut messages = Vec::new();
        for line in complete[..last_newline].split(|&b| b == b'\n') {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            messages.extend(parse_nonblank(
                self.parse_line,
                &String::from_utf8_lossy(line),
                line_start,
            ));
            line_start += line.len() as u64 + 1;
        }
        messages
    }
}

fn parse_nonblank(parse_line: LineParser, line: &str, offset: u64) -> Vec<Message> {
    if line.trim().is_empty() {
        return Vec::new();
    }
    let mut messages = parse_line(line).unwrap_or_default();
    for message in &mut messages {
        message.offset = Some(offset);
    }
    messages
}

fn read_window(
    file: &mut File,
    end: u64,
    jump: Jump,
    parse_line: LineParser,
    cancel: &AtomicBool,
) -> io::Result<Option<Page>> {
    if jump.offset >= end || !starts_line(file, jump.offset)? {
        return Ok(None);
    }
    let Some(forward) = read_forward(file, jump, end, parse_line, cancel)? else {
        return Ok(None);
    };
    let mut page = read_page(file, jump.offset, jump.before, parse_line, cancel)?;
    page.messages.extend(forward);
    Ok(Some(page))
}

fn starts_line(file: &mut File, offset: u64) -> io::Result<bool> {
    if offset == 0 {
        return Ok(true);
    }
    Ok(read_range(file, offset - 1, 1)? == b"\n")
}

fn read_forward(
    file: &mut File,
    jump: Jump,
    end: u64,
    parse_line: LineParser,
    cancel: &AtomicBool,
) -> io::Result<Option<Vec<Message>>> {
    file.seek(SeekFrom::Start(jump.offset))?;
    let mut reader = BufReader::with_capacity(FORWARD_BUFFER, file.take(end - jump.offset));
    let mut messages = Vec::new();
    let mut line = Vec::new();
    let mut line_start = jump.offset;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(Some(messages));
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        messages.extend(parse_nonblank(
            parse_line,
            &String::from_utf8_lossy(&line),
            line_start,
        ));
        if messages.len() > jump.limit {
            return Ok(None);
        }
        line_start += read as u64;
    }
}

fn last_line_boundary<R: Read + Seek>(source: &mut R, end: u64) -> io::Result<u64> {
    let mut position = end;
    let mut chunk = BOUNDARY_SCAN_CHUNK;
    while position > 0 {
        let take = (chunk as u64).min(position);
        position -= take;
        let bytes = read_range(source, position, take as usize)?;
        if let Some(newline) = bytes.iter().rposition(|&b| b == b'\n') {
            return Ok(position + newline as u64 + 1);
        }
        chunk = (chunk * 2).min(MAX_CHUNK);
    }
    Ok(0)
}

pub(crate) fn read_page<R: Read + Seek>(
    source: &mut R,
    end: u64,
    count: usize,
    parse_line: LineParser,
    cancel: &AtomicBool,
) -> io::Result<Page> {
    read_page_chunked(
        source,
        end,
        count,
        parse_line,
        cancel,
        FIRST_CHUNK,
        MAX_CHUNK,
    )
}

fn read_page_chunked<R: Read + Seek>(
    source: &mut R,
    end: u64,
    count: usize,
    parse_line: LineParser,
    cancel: &AtomicBool,
    first_chunk: usize,
    max_chunk: usize,
) -> io::Result<Page> {
    let mut by_line_newest_first: Vec<Vec<Message>> = Vec::new();
    let mut found = 0;
    let mut position = end;
    let mut cursor = end;
    let mut carry: Vec<u8> = Vec::new();
    let mut chunk = first_chunk;
    'chunks: while position > 0 && !cancel.load(Ordering::Relaxed) {
        let take = (chunk as u64).min(position);
        position -= take;
        let mut bytes = read_range(source, position, take as usize)?;
        bytes.extend_from_slice(&carry);
        let lines_start = if position == 0 {
            0
        } else {
            match bytes.iter().position(|&b| b == b'\n') {
                Some(newline) => newline + 1,
                None => {
                    carry = bytes;
                    chunk = (chunk * 2).min(max_chunk);
                    continue;
                }
            }
        };
        carry = bytes[..lines_start].to_vec();
        let lines = &bytes[lines_start..];
        let lines_offset = position + lines_start as u64;
        let mut line_end = lines.len();
        while line_end > 0 {
            if cancel.load(Ordering::Relaxed) {
                break 'chunks;
            }
            let body_end = line_end - 1;
            let line_start = lines[..body_end]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |newline| newline + 1);
            let parsed = parse_nonblank(
                parse_line,
                &String::from_utf8_lossy(&lines[line_start..body_end]),
                lines_offset + line_start as u64,
            );
            found += parsed.len();
            by_line_newest_first.push(parsed);
            cursor = lines_offset + line_start as u64;
            line_end = line_start;
            if found >= count {
                break 'chunks;
            }
        }
        chunk = (chunk * 2).min(max_chunk);
    }
    Ok(Page {
        messages: by_line_newest_first.into_iter().rev().flatten().collect(),
        earlier: (cursor > 0).then_some(Cursor::Offset(cursor)),
    })
}

fn read_range<R: Read + Seek>(source: &mut R, start: u64, len: usize) -> io::Result<Vec<u8>> {
    source.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; len];
    source.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, fs, io::Cursor as Bytes, rc::Rc};

    fn parse(line: &str) -> Result<Vec<Message>, serde_json::Error> {
        crate::sources::claude::transcript::parse_line(line)
            .map(|message| message.into_iter().collect())
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn texts(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .flat_map(|message| &message.blocks)
            .filter_map(|block| match block {
                crate::session::transcript::Block::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn user_line(text: &str) -> String {
        format!(r#"{{"type":"user","message":{{"content":"{text}"}}}}"#)
    }

    fn mixed_transcript(messages: usize) -> String {
        let mut body = String::new();
        for index in 0..messages {
            body.push_str(&user_line(&format!("héllo 漢字 🙂 {index}")));
            body.push('\n');
            if index % 3 == 0 {
                body.push_str(r#"{"type":"summary","summary":"not a message"}"#);
                body.push('\n');
            }
        }
        body
    }

    fn read_all_backwards(bytes: &[u8], chunk: usize, count: usize) -> Vec<String> {
        let mut source = Bytes::new(bytes.to_vec());
        let mut end = bytes.len() as u64;
        let mut pages = Vec::new();
        loop {
            let page =
                read_page_chunked(&mut source, end, count, parse, &never(), chunk, chunk).unwrap();
            pages.push(texts(&page.messages));
            match page.earlier {
                Some(Cursor::Offset(offset)) => end = offset,
                _ => break,
            }
        }
        pages.into_iter().rev().flatten().collect()
    }

    #[test]
    fn paging_backwards_through_a_file_yields_exactly_the_forward_parse_for_every_chunk_size() {
        let body = mixed_transcript(40);
        let expected: Vec<String> = (0..40).map(|i| format!("héllo 漢字 🙂 {i}")).collect();
        for chunk in 1..=64 {
            for count in [1, 7, 1000] {
                assert_eq!(
                    read_all_backwards(body.as_bytes(), chunk, count),
                    expected,
                    "chunk {chunk} count {count}"
                );
            }
        }
    }

    #[test]
    fn a_line_longer_than_the_chunk_is_read_whole_and_an_empty_file_has_nothing_earlier() {
        let long = "x".repeat(10_000);
        let body = format!("{}\n{}\n", user_line(&long), user_line("tail"));
        assert_eq!(
            read_all_backwards(body.as_bytes(), 64, 1),
            [long, "tail".to_owned()]
        );
        let page = read_page(&mut Bytes::new(Vec::new()), 0, 5, parse, &never()).unwrap();
        assert!(page.messages.is_empty() && page.earlier.is_none());
    }

    #[test]
    fn a_file_with_fewer_messages_than_the_target_reaches_the_start_at_once() {
        let body = mixed_transcript(3);
        let page = read_page(
            &mut Bytes::new(body.clone().into_bytes()),
            body.len() as u64,
            200,
            parse,
            &never(),
        )
        .unwrap();
        assert_eq!(page.messages.len(), 3);
        assert!(page.earlier.is_none());
    }

    struct Counting<R> {
        inner: R,
        bytes: Rc<Cell<u64>>,
    }

    impl<R: Read> Read for Counting<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let read = self.inner.read(buf)?;
            self.bytes.set(self.bytes.get() + read as u64);
            Ok(read)
        }
    }

    impl<R: Seek> Seek for Counting<R> {
        fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            self.inner.seek(to)
        }
    }

    #[test]
    fn opening_a_large_file_reads_only_its_tail_and_older_bytes_are_read_only_on_demand() {
        let body = mixed_transcript(80_000);
        assert!(body.len() > 20 * FIRST_CHUNK);
        let bytes = Rc::new(Cell::new(0));
        let mut source = Counting {
            inner: Bytes::new(body.clone().into_bytes()),
            bytes: Rc::clone(&bytes),
        };
        let end = last_line_boundary(&mut source, body.len() as u64).unwrap();
        let first = read_page(&mut source, end, 200, parse, &never()).unwrap();
        assert_eq!(first.messages.len(), 200);
        let after_open = bytes.get();
        assert!(
            after_open <= 2 * FIRST_CHUNK as u64,
            "read {after_open} bytes of {}",
            body.len()
        );

        let Some(Cursor::Offset(cursor)) = first.earlier else {
            panic!("expected an earlier cursor")
        };
        assert_eq!(bytes.get(), after_open);
        let second = read_page(&mut source, cursor, 200, parse, &never()).unwrap();
        assert_eq!(second.messages.len(), 200);
        assert!(bytes.get() > after_open);
        assert!(bytes.get() <= 4 * FIRST_CHUNK as u64);
        let newest_of_second = texts(&second.messages).pop().unwrap();
        let oldest_of_first = texts(&first.messages).remove(0);
        let number = |text: &str| text.rsplit(' ').next().unwrap().parse::<usize>().unwrap();
        assert_eq!(number(&newest_of_second) + 1, number(&oldest_of_first));
    }

    fn transcript(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ccview-sfollow-{name}-{}.jsonl",
            std::process::id()
        ));
        fs::write(&path, body).unwrap();
        path
    }

    fn append(path: &PathBuf, text: &str) {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn a_message_appended_while_the_tail_is_read_appears_exactly_once() {
        let path = transcript(
            "race",
            &format!("{}\n{}\n", user_line("one"), user_line("two")),
        );
        let mut follower = JsonlFollower::new(&path, parse);
        let mut during = || append(&path, &format!("{}\n", user_line("during")));
        let polled = follower
            .poll_after_recording_end(10, &never(), &mut during)
            .unwrap();
        assert_eq!(texts(&polled.messages), ["one", "two", "during"]);

        append(&path, &format!("{}\n", user_line("after")));
        let next = follower.poll(10, &never()).unwrap();
        assert_eq!(texts(&next.messages), ["after"]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_unfinished_last_line_is_ignored_until_it_is_complete() {
        let complete = user_line("whole");
        let path = transcript("partial", &format!("{complete}\n{}", &complete[..10]));
        let mut follower = JsonlFollower::new(&path, parse);
        let first = follower.poll(10, &never()).unwrap();
        assert_eq!(texts(&first.messages), ["whole"]);

        append(&path, &format!("{}\n", &complete[10..]));
        let second = follower.poll(10, &never()).unwrap();
        assert_eq!(texts(&second.messages), ["whole"]);
        assert!(!second.replaced);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_superseded_load_stops_before_parsing_the_file() {
        let path = transcript("cancel", &format!("{}\n", user_line("hi")).repeat(50));
        let polled = JsonlFollower::new(&path, parse)
            .poll(usize::MAX, &AtomicBool::new(true))
            .unwrap();
        assert!(polled.messages.is_empty());
        fs::remove_file(path).unwrap();
    }
}
