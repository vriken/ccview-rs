use super::support::*;
use crate::tui::app::*;
use crate::{catalog::feed::Update, store::favorites::Favorites, tui::style::identity::Identity};
use std::sync::mpsc;

#[test]
fn entries_queued_before_the_app_exists_are_on_the_first_frame() {
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Update::Scanned(vec![entry("cached-session", "/w/one")]))
        .unwrap();
    let mut app = App::new(
        Favorites::load(
            std::env::temp_dir().join(format!("ccview-first-{}/f.json", std::process::id())),
        ),
        Identity::load(
            std::env::temp_dir().join(format!("ccview-first-{}/c.json", std::process::id())),
        ),
        Some(receiver),
        mpsc::channel().0,
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("cached-session"));
}
