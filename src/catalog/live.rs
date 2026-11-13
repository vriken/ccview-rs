use crate::{
    catalog::{
        checkout::{CheckoutKeys, CheckoutResolver},
        feed::{Feed, Update},
    },
    session::{
        activity::ActivityMap,
        live::{LiveSession, Status},
    },
    sources::claude::{
        activity::Tracker,
        agents_cli::{self, CliSession, JobWatch},
        dod::{DodCounts, DodWatcher, HandoffOwners},
        registry::RegistryWatcher,
    },
    store::settings::UiSettingsStore,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant, SystemTime},
};

const REGISTRY_POLL: Duration = Duration::from_millis(250);
const ACTIVITY_REFRESH: Duration = Duration::from_millis(500);
const CLI_SAFETY_REFRESH: Duration = Duration::from_secs(5);
const CLI_MIN_GAP: Duration = Duration::from_millis(500);
const DOD_REFRESH: Duration = Duration::from_secs(2);

type CliResult = Option<Vec<CliSession>>;
type StateSignature = Vec<(Option<String>, Status, u32)>;

fn state_signature(live: &[LiveSession]) -> StateSignature {
    live.iter()
        .map(|session| {
            (
                session.session_id.clone(),
                session.status.clone(),
                session.pid,
            )
        })
        .collect()
}

pub(crate) fn spawn(feed: Feed, projects_dir: PathBuf) {
    thread::spawn(move || run(&feed, projects_dir));
}

fn run(feed: &Feed, projects_dir: PathBuf) {
    let mut registry = RegistryWatcher::from_env()
        .unwrap_or_else(|| RegistryWatcher::new(PathBuf::new(), PathBuf::new()));
    let (cli_requests, cli_results) = spawn_cli_worker();
    let mut jobs = JobWatch::default();
    let mut signature = StateSignature::new();
    let mut tracker = Tracker::new(projects_dir);
    let mut resolver = CheckoutResolver::default();
    let mut checkouts = CheckoutKeys::new();
    let mut live = Vec::new();
    let mut cli: Vec<CliSession> = Vec::new();
    let mut cli_arrived = false;
    let mut last_activity: Option<ActivityMap> = None;
    let mut next_activity = Instant::now();
    let mut dod_watcher = DodWatcher::from_env(HandoffOwners::from_setting(
        &UiSettingsStore::load_default().settings().handoff_owner,
    ));
    let mut last_dod: HashMap<String, DodCounts> = HashMap::new();
    let mut next_dod = Instant::now();
    let _ = cli_requests.send(());
    loop {
        let polled = registry.poll();
        let jobs_changed = jobs.changed();
        let mut state_changed = false;
        if polled.changed {
            live = polled.live;
            state_changed = signature != state_signature(&live);
            signature = state_signature(&live);
            if !feed.send(Update::Live(live.clone())) {
                return;
            }
            let resolved = resolver.resolve(&live);
            if resolved != checkouts {
                checkouts = resolved;
                if !feed.send(Update::Checkouts(checkouts.clone())) {
                    return;
                }
            }
        }
        if state_changed || jobs_changed {
            let _ = cli_requests.send(());
        }
        if cli_arrived || polled.changed || Instant::now() >= next_activity {
            cli_arrived = false;
            next_activity = Instant::now() + ACTIVITY_REFRESH;
            let activity = tracker.compute(&live, &cli, SystemTime::now());
            if last_activity.as_ref() != Some(&activity) {
                last_activity = Some(activity.clone());
                if !feed.send(Update::Activity(activity)) {
                    return;
                }
            }
        }
        if let Some(watcher) = dod_watcher.as_mut()
            && Instant::now() >= next_dod
        {
            next_dod = Instant::now() + DOD_REFRESH;
            let ids = live
                .iter()
                .filter_map(|session| session.session_id.as_deref())
                .chain(
                    last_activity
                        .iter()
                        .flat_map(|map| map.keys().map(String::as_str)),
                );
            let counts = watcher.poll(ids);
            if counts != last_dod {
                last_dod = counts.clone();
                if !feed.send(Update::Dod(counts)) {
                    return;
                }
            }
        }
        match cli_results.recv_timeout(REGISTRY_POLL) {
            Ok(result) => {
                cli = result.unwrap_or_default();
                cli_arrived = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => thread::sleep(REGISTRY_POLL),
        }
    }
}

fn spawn_cli_worker() -> (Sender<()>, Receiver<CliResult>) {
    let (request_sender, requests) = mpsc::channel::<()>();
    let (result_sender, results) = mpsc::channel();
    thread::spawn(move || {
        let mut last_fetch: Option<Instant> = None;
        loop {
            if requests.recv_timeout(CLI_SAFETY_REFRESH) == Err(RecvTimeoutError::Disconnected) {
                return;
            }
            if let Some(at) = last_fetch {
                thread::sleep(CLI_MIN_GAP.saturating_sub(at.elapsed()));
            }
            while requests.try_recv().is_ok() {}
            last_fetch = Some(Instant::now());
            if result_sender.send(agents_cli::fetch()).is_err() {
                return;
            }
        }
    });
    (request_sender, results)
}
