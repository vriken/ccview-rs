use crate::{sources::claude::transcript, terminal::session_host::autostart, tui};
use std::process::ExitCode;

pub mod export;
pub mod host;
pub mod print;

pub fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--export") {
        return export::run(&args);
    }
    if args.first().map(String::as_str) == Some("host") {
        return host::run(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("autostart") {
        return match autostart::run_command(&args[1..], &autostart::SystemLaunchctl) {
            Ok(message) => {
                println!("{message}");
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        };
    }
    if args.iter().any(|arg| arg == "--live") {
        print::print_live();
        return ExitCode::SUCCESS;
    }
    if args.is_empty() {
        return match tui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        };
    }
    let path = match print::resolve_path() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let session = match transcript::parse_file(&path) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("{}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    };
    for message in &session.messages {
        print::print_message(message);
    }
    eprintln!(
        "{}: {} messages, {} malformed lines",
        path.display(),
        session.messages.len(),
        session.malformed_lines
    );
    ExitCode::SUCCESS
}
