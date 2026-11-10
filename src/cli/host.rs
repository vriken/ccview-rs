use crate::terminal::{
    HostClient, RuntimeDir,
    session_host::{daemon, durable},
};
use std::process::ExitCode;

pub fn run(args: &[String]) -> ExitCode {
    let dir = RuntimeDir::from_env();
    if args.iter().any(|arg| arg == "--list") {
        return match HostClient::new(dir).list() {
            Ok(sessions) => {
                for session in sessions {
                    let state = match &session.exit {
                        Some(exit) => format!("exited {}", exit.code),
                        None => "running".to_owned(),
                    };
                    println!(
                        "{:<4} {:<8} {:<10} pid {:<8} {}  {}",
                        session.id,
                        session.harness,
                        state,
                        session.pid.map_or("?".to_owned(), |pid| pid.to_string()),
                        session.cwd.as_deref().unwrap_or("?"),
                        session.title
                    );
                }
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        };
    }
    if args.iter().any(|arg| arg == "--restore") {
        return match HostClient::new(dir).restore() {
            Ok(started) => {
                println!("restored {started} session(s)");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        };
    }
    let idle_exit = std::env::var("CCVIEW_HOST_IDLE_MS")
        .ok()
        .and_then(|millis| millis.parse().ok())
        .map_or(daemon::IDLE_EXIT, std::time::Duration::from_millis);
    let exit_grace = std::env::var("CCVIEW_HOST_EXIT_GRACE_MS")
        .ok()
        .and_then(|millis| millis.parse().ok())
        .map_or(durable::EXIT_GRACE, std::time::Duration::from_millis);
    daemon::install_shutdown_handlers();
    let sessions = durable::Durable::open(durable::sessions_file()).with_exit_grace(exit_grace);
    match daemon::run_durable(&dir, &daemon::DaemonConfig { idle_exit }, Some(sessions)) {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}
