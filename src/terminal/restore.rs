use std::path::Path;

use super::client::with_client_environment;
use super::durable::{Persisted, login_path};
use super::host::Size;
use super::spec::CommandSpec;
use crate::harness::{ClaudeCode, CodexCli, Harness, Launch, OpenCodeCli};

pub const MAX_RESTORED: usize = 12;
pub const RESTORE_SIZE: Size = Size { rows: 24, cols: 80 };

pub struct Plan {
    pub spec: CommandSpec,
    pub persisted: Persisted,
}

pub fn plan(
    stale: Vec<Persisted>,
    running: &[String],
    home: &Path,
    inherited_path: Option<&str>,
    disable_agent_view: bool,
) -> Vec<Plan> {
    let mut taken: Vec<String> = running.to_vec();
    let mut plans = Vec::new();
    for persisted in stale {
        let Some(session_id) = persisted.session_id.clone() else {
            eprintln!("restore: skipping {} session in {}: no session id", persisted.harness, persisted.cwd);
            continue;
        };
        if taken.contains(&session_id) {
            eprintln!("restore: skipping {session_id}: already running");
            continue;
        }
        if plans.len() >= MAX_RESTORED {
            eprintln!("restore: skipping {session_id}: more than {MAX_RESTORED} sessions");
            continue;
        }
        if !Path::new(&persisted.cwd).is_dir() {
            eprintln!("restore: skipping {session_id}: {} does not exist", persisted.cwd);
            continue;
        }
        let Some(program) = persisted.program.clone().filter(|program| Path::new(program).is_file()) else {
            eprintln!("restore: skipping {session_id}: program is gone or unknown");
            continue;
        };
        let launch = Launch::Resume(session_id.clone());
        let cwd = Path::new(&persisted.cwd);
        let command = match persisted.harness.as_str() {
            "claude" => ClaudeCode::with_binary(&program)
                .disable_agent_view(disable_agent_view)
                .command(&launch, cwd),
            "codex" => CodexCli.command(&launch, cwd),
            "opencode" => OpenCodeCli.command(&launch, cwd),
            _ => None,
        };
        let Some(mut spec) = command else {
            eprintln!("restore: skipping {session_id}: unknown harness {}", persisted.harness);
            continue;
        };
        spec.program = program;
        spec.env.push(("PATH".to_owned(), login_path(home, inherited_path)));
        taken.push(session_id);
        plans.push(Plan { spec: with_client_environment(spec), persisted });
    }
    plans
}
