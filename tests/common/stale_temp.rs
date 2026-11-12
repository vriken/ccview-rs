use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::Path,
    sync::Once,
    time::{Duration, SystemTime},
};

const MIN_AGE: Duration = Duration::from_secs(600);
static SWEPT: Once = Once::new();

pub fn sweep_once(uid: u32, alive: fn(u32) -> bool) {
    SWEPT.call_once(|| sweep(&std::env::temp_dir(), uid, alive, MIN_AGE));
}

fn owner_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("ccview-")?;
    rest.split('-')
        .enumerate()
        .filter(|(index, segment)| {
            *index > 0 && !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit())
        })
        .last()?
        .1
        .parse()
        .ok()
}

fn sweep(temp: &Path, uid: u32, alive: fn(u32) -> bool, min_age: Duration) {
    let Ok(entries) = fs::read_dir(temp) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(owner_pid) else {
            continue;
        };
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let old_enough = metadata
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= min_age);
        if metadata.is_dir() && metadata.uid() == uid && old_enough && !alive(pid) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEAD: u32 = 900_001;
    const LIVE: u32 = 900_002;

    fn alive_if_live(pid: u32) -> bool {
        pid == LIVE
    }

    fn make(root: &Path, name: &str) {
        fs::create_dir_all(root.join(name)).unwrap();
    }

    #[test]
    fn only_old_directories_of_dead_owners_are_removed() {
        let root =
            std::env::temp_dir().join(format!("ccview-sweep-fixture-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dead = format!("ccview-x-{DEAD}");
        let dead_mid = format!("ccview-x-{DEAD}-y");
        let live = format!("ccview-x-{LIVE}");
        for name in [&dead, &dead_mid, &live, "ccview-501", "other-x-900001"] {
            make(&root, name);
        }
        let uid = fs::metadata(&root).unwrap().uid();

        sweep(&root, uid, alive_if_live, Duration::from_secs(3600));
        assert!(root.join(&dead).exists(), "too young to remove");

        sweep(&root, uid.wrapping_add(1), alive_if_live, Duration::ZERO);
        assert!(root.join(&dead).exists(), "another user's directory");

        sweep(&root, uid, alive_if_live, Duration::ZERO);
        assert!(!root.join(&dead).exists());
        assert!(!root.join(&dead_mid).exists());
        assert!(root.join(&live).exists(), "owner pid is alive");
        assert!(root.join("ccview-501").exists(), "per-user runtime dir");
        assert!(root.join("other-x-900001").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
