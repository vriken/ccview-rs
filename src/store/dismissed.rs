use crate::{
    session::activity::Activity,
    store::{config_file, write_atomically},
};
use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::PathBuf,
};

const NO_PID: u32 = 0;

pub(crate) struct Observed<'a> {
    pub(crate) pid: Option<u32>,
    pub(crate) activity: Option<&'a Activity>,
}

#[derive(Default)]
pub(crate) struct Dismissed {
    path: Option<PathBuf>,
    pids: BTreeMap<String, u32>,
}

impl Dismissed {
    pub(crate) fn load_default() -> Self {
        Self::load(config_file("dismissed.json"))
    }

    pub(crate) fn load(path: PathBuf) -> Self {
        let pids = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, u32>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            pids,
        }
    }

    pub(crate) fn contains(&self, session_id: &str) -> bool {
        self.pids.contains_key(session_id)
    }

    pub(crate) fn dismiss(&mut self, session_id: &str, pid: Option<u32>) -> io::Result<()> {
        self.pids
            .insert(session_id.to_owned(), pid.unwrap_or(NO_PID));
        self.save()
    }

    pub(crate) fn undismiss(&mut self, session_id: &str) -> io::Result<()> {
        if self.pids.remove(session_id).is_some() {
            self.save()?;
        }
        Ok(())
    }

    pub(crate) fn reconcile(&mut self, observed: &HashMap<&str, Observed>) -> bool {
        let before = self.pids.len();
        self.pids.retain(|session_id, dismissed_pid| {
            observed
                .get(session_id.as_str())
                .is_none_or(|now| still_dismissed(*dismissed_pid, now))
        });
        let changed = self.pids.len() != before;
        if changed {
            let _ = self.save();
        }
        changed
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        write_atomically(path, &serde_json::to_vec_pretty(&self.pids)?)
    }
}

fn still_dismissed(dismissed_pid: u32, now: &Observed) -> bool {
    let current_pid = now.pid.unwrap_or(NO_PID);
    let anything_left = current_pid != NO_PID || now.activity.is_some();
    current_pid == dismissed_pid && anything_left
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::activity::ActivityState;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccview-dismissed-{}-{name}/dismissed.json",
            std::process::id()
        ))
    }

    fn activity(state: ActivityState) -> Activity {
        Activity {
            state,
            background: false,
            attach_id: None,
            agents: Vec::new(),
            last_activity: None,
        }
    }

    fn observed<'a>(
        pid: Option<u32>,
        activity: Option<&'a Activity>,
    ) -> HashMap<&'static str, Observed<'a>> {
        HashMap::from([("s", Observed { pid, activity })])
    }

    fn dismissed_for(pid: u32) -> Dismissed {
        let mut dismissed = Dismissed::default();
        dismissed.dismiss("s", Some(pid)).unwrap();
        dismissed
    }

    #[test]
    fn a_session_stays_dismissed_whatever_it_is_doing_while_the_same_process_lives() {
        for state in [
            ActivityState::Idle,
            ActivityState::Busy,
            ActivityState::Blocked,
        ] {
            let mut dismissed = dismissed_for(10);
            assert!(!dismissed.reconcile(&observed(Some(10), Some(&activity(state)))));
            assert!(dismissed.contains("s"));
        }
    }

    #[test]
    fn a_new_process_or_a_gone_process_clears_the_dismissal() {
        let idle = activity(ActivityState::Idle);
        let mut dismissed = dismissed_for(10);
        dismissed.reconcile(&observed(Some(11), Some(&idle)));
        assert!(!dismissed.contains("s"));
        let mut dismissed = dismissed_for(10);
        dismissed.reconcile(&observed(None, None));
        assert!(!dismissed.contains("s"));
    }

    #[test]
    fn a_session_without_a_process_stays_dismissed_while_its_idle_activity_lasts() {
        let idle = activity(ActivityState::Idle);
        let mut dismissed = Dismissed::default();
        dismissed.dismiss("s", None).unwrap();
        dismissed.reconcile(&observed(None, Some(&idle)));
        assert!(dismissed.contains("s"));
        dismissed.reconcile(&observed(None, None));
        assert!(!dismissed.contains("s"));
    }

    #[test]
    fn a_session_the_catalog_does_not_know_yet_is_left_alone() {
        let mut dismissed = dismissed_for(10);
        assert!(!dismissed.reconcile(&HashMap::new()));
        assert!(dismissed.contains("s"));
    }

    #[test]
    fn a_corrupt_file_starts_empty() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();
        assert!(!Dismissed::load(path.clone()).contains("s"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn dismissing_never_touches_a_process() {
        let forbidden = ["libc", "std::process", "kill", "signal", "Command"];
        for source in [
            include_str!("dismissed.rs"),
            include_str!("../tui/app/flags/dismiss.rs"),
        ] {
            let code = source.split("#[cfg(test)]").next().unwrap();
            for word in forbidden {
                assert!(!code.contains(word), "{word} found in dismissal code");
            }
        }
    }
}
