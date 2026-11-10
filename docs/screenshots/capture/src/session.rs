use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub const ROWS: u16 = 34;
pub const COLS: u16 = 120;
const WAIT: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_millis(400);
const DEVICE_ATTRIBUTES_QUERY: &[u8] = b"\x1b[c";
const KEYBOARD_QUERY: &[u8] = b"\x1b[?u";
const KEYBOARD_REPLY: &[u8] = b"\x1b[?0u";
const DEVICE_ATTRIBUTES_REPLY: &[u8] = b"\x1b[?62;c";

fn nz(value: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(value).expect("non-zero size")
}

pub struct Session {
    _master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    parser: Arc<Mutex<vt100::Parser>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Session {
    pub fn start(binary: &Path, demo: &Path, harness: &Path) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(PtySize { rows: ROWS, cols: COLS, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
        let mut command = CommandBuilder::new(binary);
        command.env_clear();
        let path = std::env::var("PATH").unwrap_or_default();
        let home = demo.join("home");
        let harness = harness.to_string_lossy().into_owned();
        let vars: [(&str, String); 15] = [
            ("PATH", path),
            ("TERM", "xterm-256color".into()),
            ("LANG", "en_US.UTF-8".into()),
            ("TZ", "UTC".into()),
            ("HOME", home.to_string_lossy().into()),
            ("XDG_CONFIG_HOME", demo.join("config").to_string_lossy().into()),
            ("XDG_CACHE_HOME", demo.join("cache").to_string_lossy().into()),
            ("XDG_STATE_HOME", demo.join("state").to_string_lossy().into()),
            ("CCVIEW_RUNTIME_DIR", demo.join("run").to_string_lossy().into()),
            ("XDG_DATA_HOME", demo.join("data").to_string_lossy().into()),
            ("CLAUDE_CONFIG_DIR", home.join(".claude").to_string_lossy().into()),
            ("CODEX_HOME", demo.join("codex").to_string_lossy().into()),
            ("CCVIEW_CLAUDE_BIN", harness.clone()),
            ("CCVIEW_CODEX_BIN", harness.clone()),
            ("CCVIEW_OPENCODE_BIN", harness),
        ];
        for (key, value) in vars {
            command.env(key, value);
        }
        command.cwd(&home);
        let child = pair.slave.spawn_command(command).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| e.to_string())?));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(nz(ROWS), nz(COLS), 0)));
        let feed = parser.clone();
        let reply = writer.clone();
        thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                let chunk = &buffer[..read];
                feed.lock().unwrap().process(chunk);
                if chunk.windows(KEYBOARD_QUERY.len()).any(|w| w == KEYBOARD_QUERY) {
                    let _ = reply.lock().unwrap().write_all(KEYBOARD_REPLY);
                }
                if chunk.windows(DEVICE_ATTRIBUTES_QUERY.len()).any(|w| w == DEVICE_ATTRIBUTES_QUERY) {
                    let _ = reply.lock().unwrap().write_all(DEVICE_ATTRIBUTES_REPLY);
                }
            }
        });
        Ok(Self { _master: pair.master, writer, parser, child })
    }

    pub fn send(&mut self, bytes: &[u8]) {
        let mut writer = self.writer.lock().unwrap();
        let _ = writer.write_all(bytes);
        let _ = writer.flush();
        drop(writer);
        thread::sleep(Duration::from_millis(150));
    }

    pub fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            let mut buffer = [0u8; 4];
            self.send(ch.encode_utf8(&mut buffer).as_bytes());
        }
    }

    pub fn text(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    pub fn wait_for(&self, needle: &str) -> Result<(), String> {
        let started = Instant::now();
        while !self.text().contains(needle) {
            if started.elapsed() > WAIT {
                return Err(format!("timed out waiting for {needle:?}; screen:\n{}", self.text()));
            }
            thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    pub fn settle(&self) {
        let mut last = self.text();
        let mut stable_since = Instant::now();
        let started = Instant::now();
        while stable_since.elapsed() < SETTLE && started.elapsed() < WAIT {
            thread::sleep(Duration::from_millis(50));
            let now = self.text();
            if now != last {
                last = now;
                stable_since = Instant::now();
            }
        }
    }

    pub fn with_screen<T>(&self, f: impl FnOnce(&vt100::Screen) -> T) -> T {
        f(self.parser.lock().unwrap().screen())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
