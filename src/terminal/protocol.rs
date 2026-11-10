use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::host::{ExitStatus, Size};
use super::spec::{CommandSpec, TerminalOptions};
use super::state::ModeSnapshot;

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const KIND_MESSAGE: u8 = 0;
const KIND_BYTES: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Message(Vec<u8>),
    Bytes(Vec<u8>),
}

impl Frame {
    pub fn decode<T: DeserializeOwned>(&self) -> io::Result<T> {
        match self {
            Frame::Message(payload) => serde_json::from_slice(payload).map_err(io::Error::other),
            Frame::Bytes(_) => Err(io::Error::new(io::ErrorKind::InvalidData, "expected a message frame")),
        }
    }
}

fn write_frame(writer: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let length = u32::try_from(payload.len() + 1)
        .ok()
        .filter(|&length| length as usize <= MAX_FRAME_BYTES)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.push(kind);
    frame.extend_from_slice(payload);
    writer.write_all(&frame)?;
    writer.flush()
}

pub fn write_message(writer: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    write_frame(writer, KIND_MESSAGE, &payload)
}

pub fn write_bytes(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    write_frame(writer, KIND_BYTES, bytes)
}

pub fn read_frame(reader: &mut impl Read) -> io::Result<Frame> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad frame length"));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let kind = body.remove(0);
    match kind {
        KIND_MESSAGE => Ok(Frame::Message(body)),
        KIND_BYTES => Ok(Frame::Bytes(body)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unknown frame kind")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: u64,
    pub title: String,
    pub harness: String,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub pid: Option<u32>,
    pub launched_at_ms: u64,
    pub exit: Option<ExitStatus>,
    pub size: Size,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Spawn {
        spec: CommandSpec,
        options: TerminalOptions,
        size: Size,
        title: String,
        harness: String,
        session_id: Option<String>,
    },
    List,
    Attach { id: u64, size: Option<Size> },
    Dismiss { id: u64 },
    Resize { rows: u16, cols: u16 },
    Detach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    Spawned(SessionInfo),
    Sessions(Vec<SessionInfo>),
    Attached { info: SessionInfo, modes: ModeSnapshot },
    Exited(ExitStatus),
    Done,
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn messages_and_raw_bytes_round_trip_in_order_over_one_stream() {
        let request = Request::Attach { id: 7, size: Some(Size { rows: 24, cols: 80 }) };
        let raw: Vec<u8> = (0..=255).collect();
        let mut wire = Vec::new();
        write_message(&mut wire, &request).unwrap();
        write_bytes(&mut wire, &raw).unwrap();
        write_message(&mut wire, &Request::Detach).unwrap();
        let mut reader = Cursor::new(wire);
        assert_eq!(read_frame(&mut reader).unwrap().decode::<Request>().unwrap(), request);
        assert_eq!(read_frame(&mut reader).unwrap(), Frame::Bytes(raw));
        assert_eq!(read_frame(&mut reader).unwrap().decode::<Request>().unwrap(), Request::Detach);
        assert_eq!(read_frame(&mut reader).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_hostile_length_prefix_is_rejected_without_allocating_it() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&u32::MAX.to_be_bytes());
        wire.push(KIND_BYTES);
        assert_eq!(read_frame(&mut Cursor::new(wire)).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
