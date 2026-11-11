mod app;
mod bindings;
mod embedded;
mod frame;
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
        dismissed::Dismissed,
        favorite_dirs::FavoriteDirs,
        favorites::Favorites,
        hidden::HiddenSessions,
        important::ImportantSessions,
        on_hold::OnHoldSessions,
        order::ManualOrder,
        settings::UiSettingsStore,
        ui_state::{self, UiState},
    },
    sys::executable,
    terminal::{HostClient, RuntimeDir},
};
pub(crate) use app::App;
use app::lifecycle::self_update;
use reader::verbosity::ViewSetting;
use runtime::LoopEvent;
use std::{io, sync::mpsc, time::Instant};
use style::identity::Identity;

pub(crate) fn run() -> io::Result<()> {
    let projects_dir = paths::projects_dir().ok_or_else(|| io::Error::other("HOME is not set"))?;
    external_terminal::remove_stale_scripts();
    let (sender, messages) = mpsc::channel();
    let wake = sender.clone();
    let favorites = Favorites::load_default();
    let important = ImportantSessions::load_default();
    let on_hold = OnHoldSessions::load_default();
    let ui_settings = UiSettingsStore::load_default();
    let age = app::lifecycle::age::starting_filter(
        ui_settings.settings(),
        &favorites,
        &important,
        &on_hold,
    );
    let (updates, age_sender) = catalog::worker::spawn(projects_dir, age.clone(), move || {
        let _ = wake.send(LoopEvent::Updated);
    });
    let mut app = App::new(
        favorites,
        Identity::load_default(),
        Some(updates),
        sender.clone(),
    )
    .with_ui_settings(ui_settings)
    .with_hidden(HiddenSessions::load_default())
    .with_important(important)
    .with_on_hold(on_hold)
    .with_order(ManualOrder::load_default())
    .with_dismissed(Dismissed::load_default())
    .with_favorite_dirs(FavoriteDirs::load_default())
    .with_age_updates(age_sender, age)
    .with_session_host(HostClient::new(RuntimeDir::from_env()));
    app.view = ViewSetting::load_default();
    let mut app = app
        .with_restored_state(UiState::take(&ui_state::default_path(), ui_state::now_ms()))
        .with_self_update(self_update::Watcher::start(Instant::now()));
    let mut terminal = ratatui::init();
    let mut painter = frame::FramePainter::new(app.restarted, Instant::now());
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
        |app| painter.paint(&mut terminal, app, Instant::now()),
        runtime::set_mouse_capture,
        runtime::write_outer,
    );
    let _ = outer_terminal::leave(&mut io::stdout(), capabilities);
    let relaunch = app.take_relaunch().filter(|_| result.is_ok());
    drop(app);
    if let Some(relaunch) = relaunch {
        let _ = crossterm::terminal::disable_raw_mode();
        let err = executable::replace_process(&relaunch.path, &relaunch.arg0, &relaunch.args);
        ratatui::restore();
        return Err(err);
    }
    ratatui::restore();
    result
}
