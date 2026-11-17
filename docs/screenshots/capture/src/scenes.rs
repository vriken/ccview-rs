use crate::{session::Session, svg};
use std::{fs, path::Path, time::Duration};

const UP: &[u8] = b"\x1b[A";
const DOWN: &[u8] = b"\x1b[B";
const LEFT: &[u8] = b"\x1b[D";
const RIGHT: &[u8] = b"\x1b[C";
const ENTER: &[u8] = b"\r";
const PREFIX: &[u8] = b"\x00";

const LIST_ROWS: u16 = 38;
const READER_ROWS: u16 = 34;
const DOD_POLL_MARGIN: Duration = Duration::from_secs(4);

struct Demo<'a> {
    binary: &'a Path,
    dir: &'a Path,
    out: &'a Path,
}

impl Demo<'_> {
    fn start(&self, rows: u16) -> Result<Session, String> {
        Session::start(
            self.binary,
            self.dir,
            &self.dir.join("fake_harness.py"),
            rows,
        )
    }

    fn ready(&self, rows: u16) -> Result<Session, String> {
        let session = self.start(rows)?;
        session.wait_for("Active 11")?;
        session.wait_for("shares checkout")?;
        session.wait_for("asks you")?;
        session.pause(DOD_POLL_MARGIN);
        session.settle();
        Ok(session)
    }

    fn shot(&self, session: &Session, name: &str) -> Result<(), String> {
        session.settle();
        let image = session.with_screen(svg::render);
        let path = self.out.join(format!("{name}.svg"));
        fs::write(&path, image).map_err(|e| format!("{}: {e}", path.display()))?;
        fs::write(self.out.join(format!("{name}.txt")), session.text())
            .map_err(|e| e.to_string())?;
        println!("wrote {}", path.display());
        Ok(())
    }
}

fn press(session: &mut Session, key: &[u8], times: usize) {
    for _ in 0..times {
        session.send(key);
    }
}

fn to_tab_bar(s: &mut Session) {
    press(s, UP, 3);
}

fn open_active_tab(s: &mut Session) {
    to_tab_bar(s);
    press(s, RIGHT, 2);
    press(s, DOWN, 1);
}

fn list_scene(demo: &Demo) -> Result<(), String> {
    let mut s = demo.ready(LIST_ROWS)?;
    open_active_tab(&mut s);
    press(&mut s, b">", 7);
    demo.shot(&s, "list")?;
    press(&mut s, b"<", 7);
    Ok(())
}

fn search_scene(demo: &Demo) -> Result<(), String> {
    let mut s = demo.ready(READER_ROWS)?;
    press(&mut s, b"/", 1);
    s.type_text("retry");
    s.wait_for("hits")?;
    s.wait_until_gone("indexing")?;
    demo.shot(&s, "search")?;
    s.type_text(" timeout");
    s.wait_for("3 hits")?;
    press(&mut s, ENTER, 1);
    s.settle();
    press(&mut s, b"N", 1);
    demo.shot(&s, "search-transcript")
}

fn reader_scene(demo: &Demo) -> Result<(), String> {
    let mut s = demo.ready(READER_ROWS)?;
    to_tab_bar(&mut s);
    press(&mut s, RIGHT, 1);
    press(&mut s, DOWN, 2);
    press(&mut s, RIGHT, 1);
    s.wait_for("retries")?;
    press(&mut s, b"v", 1);
    s.wait_for("view: tools")?;
    press(&mut s, b"g", 1);
    s.wait_for("Update(")?;
    demo.shot(&s, "transcript-tools")?;
    press(&mut s, b"v", 2);

    press(&mut s, LEFT, 1);
    press(&mut s, b"n", 1);
    s.wait_for("demo-agent")?;
    s.type_text("Add a test for the retry backoff");
    press(&mut s, ENTER, 1);
    s.wait_for("0.5, 1, 2")?;
    press(&mut s, PREFIX, 1);
    press(&mut s, b"l", 1);
    to_tab_bar(&mut s);
    press(&mut s, RIGHT, 1);
    press(&mut s, DOWN, 2);
    press(&mut s, b"n", 1);
    s.wait_for("demo-agent")?;
    s.type_text("Fix the rounding per invoice");
    press(&mut s, ENTER, 1);
    s.wait_for("round once per invoice")?;
    demo.shot(&s, "embedded")?;

    press(&mut s, PREFIX, 1);
    press(&mut s, b"l", 1);
    press(&mut s, b"<", 4);
    demo.shot(&s, "narrow-list")
}

pub fn run(binary: &Path, dir: &Path, out: &Path) -> Result<(), String> {
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let demo = Demo { binary, dir, out };
    list_scene(&demo)?;
    search_scene(&demo)?;
    reader_scene(&demo)
}
