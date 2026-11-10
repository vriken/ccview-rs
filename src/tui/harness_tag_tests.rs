use super::{test_support::*, *};
use crate::terminal::{CommandSpec, Size};
use sessions::Launched;

const HARNESSES: [HarnessId; 3] = [HarnessId::Claude, HarnessId::Codex, HarnessId::OpenCode];
const SIZES: [(u16, u16); 2] = [(120, 35), (60, 20)];

fn mixed_app(name: &str) -> App {
    app_with(
        name,
        HARNESSES
            .iter()
            .map(|&harness| entry_in(harness.as_str(), "/w/one", harness))
            .collect(),
    )
}

fn launch(app: &mut App, harness: HarnessId) -> u64 {
    let spec = CommandSpec::new("sh").args(["-c", "exec sleep 60"]);
    let launched = Launched { title: "t".to_owned(), session_id: None, harness };
    let id = app.sessions.launch(&spec, launched, Size { rows: 10, cols: 40 }).unwrap();
    app.show_embedded(id);
    id
}

#[test]
fn list_rows_do_not_name_the_harness() {
    for (width, height) in SIZES {
        let mut app = mixed_app("rows");
        let lines = screen_lines(&mut app, width, height);
        for harness in HARNESSES {
            let tag = format!("{} ·", harness.as_str());
            for line in lines.iter().skip(2) {
                let list_part: String = line.chars().take(24).collect();
                assert!(!list_part.contains(&tag), "{width}x{height} {harness:?}: {list_part}");
            }
        }
    }
}

#[test]
fn the_transcript_title_names_the_selected_entrys_harness() {
    for harness in HARNESSES {
        let mut app = mixed_app("transcript");
        app.select_path(Some(PathBuf::from(format!("/p/{}.jsonl", harness.as_str()))));
        for (width, height) in SIZES {
            let title = screen_lines(&mut app, width, height)[0].clone();
            assert!(title.contains(&format!("TRANSCRIPT · {}", harness.as_str())), "{width}: {title}");
        }
    }
}

#[test]
fn strip_pane_title_and_footer_chip_name_the_running_harness() {
    for harness in HARNESSES {
        for (width, height) in SIZES {
            let mut app = mixed_app("embedded");
            let id = launch(&mut app, harness);
            let lines = screen_lines(&mut app, width, height);
            let name = harness.as_str();
            assert!(lines[0].contains(name), "{width} strip: {}", lines[0]);
            assert!(lines[1].contains(&format!("SESSION · {name} · t")), "{width} title: {}", lines[1]);
            let footer = lines.last().unwrap();
            assert!(footer.contains(&format!("SESSION · {name}")), "{width} footer: {footer}");
            app.sessions.remove(id);
        }
    }
}
