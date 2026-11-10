use ccview_rs::{
    autostart, discover, export,
    parse::{self, Block, Message, Role},
    registry::{self, LiveSession},
    terminal::{HostClient, RuntimeDir, daemon, durable},
    tui,
};
use std::{
    path::PathBuf,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--export") {
        return export::run(&args);
    }
    if args.first().map(String::as_str) == Some("host") {
        return host(&args[1..]);
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
        print_live();
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
    let path = match resolve_path() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let session = match parse::parse_file(&path) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("{}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    };
    for message in &session.messages {
        print_message(message);
    }
    eprintln!(
        "{}: {} messages, {} malformed lines",
        path.display(),
        session.messages.len(),
        session.malformed_lines
    );
    ExitCode::SUCCESS
}

fn host(args: &[String]) -> ExitCode {
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

fn resolve_path() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--file"), Some(path)) => Ok(PathBuf::from(path)),
        (None, _) => {
            let dir = discover::claude_projects_dir().ok_or("HOME is not set")?;
            discover::find_sessions(&dir)
                .into_iter()
                .next()
                .ok_or_else(|| format!("no sessions under {}", dir.display()))
        }
        _ => Err("usage: ccview-rs [--file <session.jsonl>]  (background session host: ccview-rs host [--list|--restore]; ccview-rs autostart on|off|status)".to_owned()),
    }
}

fn print_message(message: &Message) {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    for block in &message.blocks {
        match block {
            Block::Text(text) => println!("[{role}] {text}"),
            Block::Thinking(_) => {}
            Block::ToolUse { name, .. } => println!("[{role}] tool_use {name}"),
            Block::ToolResult(content) => {
                let first_line = content.lines().next().unwrap_or("");
                println!("[{role}] tool_result {first_line}");
            }
        }
    }
}

fn print_live() {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    for session in registry::live_sessions() {
        println!("{}", live_line(&session, now_ms));
    }
}

fn live_line(session: &LiveSession, now_ms: u64) -> String {
    let age_secs = now_ms.saturating_sub(session.updated_at_ms.unwrap_or(now_ms)) / 1000;
    format!(
        "{:<6} {:<28} {}  {}s ago",
        session.status.label(),
        session.name.as_deref().unwrap_or("(unnamed)"),
        session.cwd.as_deref().unwrap_or("?"),
        age_secs
    )
}
