use crate::{session::Session, svg};
use std::{fs, path::Path};

const UP: &[u8] = b"\x1b[A";
const DOWN: &[u8] = b"\x1b[B";
const LEFT: &[u8] = b"\x1b[D";
const RIGHT: &[u8] = b"\x1b[C";
const ENTER: &[u8] = b"\r";
const PREFIX: &[u8] = b"\x00";

fn shot(session: &Session, out: &Path, name: &str) -> Result<(), String> {
    session.settle();
    let image = session.with_screen(svg::render);
    let path = out.join(format!("{name}.svg"));
    fs::write(&path, image).map_err(|e| format!("{}: {e}", path.display()))?;
    fs::write(out.join(format!("{name}.txt")), session.text()).map_err(|e| e.to_string())?;
    println!("wrote {}", path.display());
    Ok(())
}

fn press(session: &mut Session, key: &[u8], times: usize) {
    for _ in 0..times {
        session.send(key);
    }
}

pub fn run(binary: &Path, demo: &Path, out: &Path) -> Result<(), String> {
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let harness = demo.join("fake_harness.py");
    let mut s = Session::start(binary, demo, &harness)?;
    s.wait_for("needs you")?;
    s.wait_for("Active 3")?;
    press(&mut s, DOWN, 3);
    shot(&s, out, "list")?;
    press(&mut s, UP, 3);

    press(&mut s, UP, 1);
    press(&mut s, RIGHT, 3);
    press(&mut s, DOWN, 1);
    shot(&s, out, "directory-tab")?;

    press(&mut s, UP, 1);
    press(&mut s, LEFT, 2);
    press(&mut s, DOWN, 1);
    shot(&s, out, "favorites-tab")?;

    press(&mut s, DOWN, 1);
    press(&mut s, RIGHT, 1);
    s.wait_for("retries")?;
    shot(&s, out, "transcript-chat")?;
    press(&mut s, b"v", 1);
    s.wait_for("view: tools")?;
    press(&mut s, b"g", 1);
    s.wait_for("Update(")?;
    shot(&s, out, "transcript-tools")?;
    press(&mut s, b"v", 2);

    press(&mut s, LEFT, 1);
    press(&mut s, b"n", 1);
    s.wait_for("demo-agent")?;
    s.type_text("Add a test for the retry backoff");
    press(&mut s, ENTER, 1);
    s.wait_for("0.5, 1, 2")?;
    press(&mut s, PREFIX, 1);
    press(&mut s, b"l", 1);
    press(&mut s, UP, 1);
    press(&mut s, RIGHT, 1);
    press(&mut s, DOWN, 1);
    press(&mut s, b"n", 1);
    s.wait_for("demo-agent")?;
    s.type_text("Where is the timeout configured?");
    press(&mut s, ENTER, 1);
    s.wait_for("Looking at")?;
    shot(&s, out, "embedded")?;

    press(&mut s, PREFIX, 1);
    press(&mut s, b"l", 1);
    press(&mut s, b"<", 4);
    shot(&s, out, "narrow-list")?;
    Ok(())
}
