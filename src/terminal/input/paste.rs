use crate::terminal::session::TerminalSession;
use std::io;

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if !bracketed {
        return text.as_bytes().to_vec();
    }
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(BRACKETED_PASTE_START);
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(BRACKETED_PASTE_END);
    bytes
}

pub fn send_paste(host: &dyn TerminalSession, text: &str) -> io::Result<()> {
    let bracketed = host.snapshot().screen().bracketed_paste();
    host.write(&encode_paste(text, bracketed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_is_bracketed_only_when_requested() {
        assert_eq!(encode_paste("hi\nthere", false), b"hi\nthere");
        assert_eq!(encode_paste("hi", true), b"\x1b[200~hi\x1b[201~");
    }
}
