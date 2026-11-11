pub fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid == 0 {
        return false;
    }
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn terminate(pid: u32) -> std::io::Result<()> {
    let pid = libc::pid_t::try_from(pid)
        .ok()
        .filter(|&pid| pid > 1)
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub fn process_start(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8(output.stdout).ok()?;
    let normalised = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalised.is_empty()).then_some(normalised)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const BEYOND_PID_T: u32 = i32::MAX as u32 + 1;

    #[test]
    fn terminate_refuses_pids_that_would_signal_a_group_or_everything() {
        for pid in [0, 1, BEYOND_PID_T, u32::MAX] {
            let error = terminate(pid).expect_err(&format!("pid {pid}"));
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "pid {pid}");
        }
    }

    #[test]
    fn pid_zero_and_out_of_range_pids_are_never_alive() {
        assert!(!pid_alive(0));
        assert!(!pid_alive(BEYOND_PID_T));
        assert!(!pid_alive(u32::MAX));
    }

    #[test]
    fn a_live_own_child_is_alive_and_terminate_ends_it() {
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        let pid = child.id();
        assert!(pid_alive(pid));
        terminate(pid).unwrap();
        child.wait().unwrap();
        assert!(!pid_alive(pid));
    }
}
