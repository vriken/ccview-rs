mod app;
mod bindings;
mod embedded;
mod input;
mod outer_terminal;
mod overlay;
mod reader;
mod runtime;
mod style;
mod widgets;

use crate::{
    catalog,
    harness::external_terminal,
    sources::claude::paths,
    store::{
        dismissed::Dismissed, favorite_dirs::FavoriteDirs, favorites::Favorites,
        hidden::HiddenSessions, important::ImportantSessions, settings::UiSettingsStore,
    },
    terminal::{HostClient, RuntimeDir},
};
pub use app::App;
use reader::verbosity::ViewSetting;
use runtime::LoopEvent;
use std::{io, sync::mpsc};
use style::identity::Identity;

pub fn run() -> io::Result<()> {
    let projects_dir = paths::projects_dir().ok_or_else(|| io::Error::other("HOME is not set"))?;
    external_terminal::remove_stale_scripts();
    let (sender, messages) = mpsc::channel();
    let wake = sender.clone();
    let updates = catalog::worker::spawn(projects_dir, move || {
        let _ = wake.send(LoopEvent::Updated);
    });
    let mut app = App::new(
        Favorites::load_default(),
        Identity::load_default(),
        Some(updates),
        sender.clone(),
    )
    .with_ui_settings(UiSettingsStore::load_default())
    .with_hidden(HiddenSessions::load_default())
    .with_important(ImportantSessions::load_default())
    .with_dismissed(Dismissed::load_default())
    .with_favorite_dirs(FavoriteDirs::load_default())
    .with_session_host(HostClient::new(RuntimeDir::from_env()));
    app.view = ViewSetting::load_default();
    let mut terminal = ratatui::init();
    let capabilities = outer_terminal::detect();
    outer_terminal::install_panic_hook(capabilities);
    outer_terminal::enter(&mut io::stdout(), capabilities)?;
    if !capabilities.keyboard_enhancement {
        app.notice = Some(
            "shift+enter sends plain enter here (terminal lacks kitty keyboard protocol)"
                .to_owned(),
        );
    }
    runtime::spawn_input_reader(sender);
    let result = runtime::event_loop(
        &mut app,
        &messages,
        |app| terminal.draw(|frame| app.draw(frame)).map(|_| ()),
        runtime::set_mouse_capture,
        runtime::write_outer,
    );
    let _ = outer_terminal::leave(&mut io::stdout(), capabilities);
    ratatui::restore();
    drop(app);
    result
}
