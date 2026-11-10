use crate::{
    sources::{HarnessId, codex, opencode},
    terminal::CommandSpec,
};
use std::path::Path;

const CLAUDE_BIN_ENV: &str = "CCVIEW_CLAUDE_BIN";
const CLAUDE_DEFAULT_BIN: &str = "claude";
const DISABLE_AGENT_VIEW_ENV: &str = "CLAUDE_CODE_DISABLE_AGENT_VIEW";
const INHERITED_SESSION_PREFIXES: [&str; 2] = ["CLAUDE_CODE_", "CLAUDE_BG"];
const INHERITED_SESSION_NAME: &str = "CLAUDECODE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    New,
    Resume(String),
    Fork(String),
    ContinueLatest,
    Attach(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    New,
    Resume,
    Fork,
    ContinueLatest,
    Attach,
}

impl Launch {
    pub fn kind(&self) -> LaunchKind {
        match self {
            Launch::New => LaunchKind::New,
            Launch::Resume(_) => LaunchKind::Resume,
            Launch::Fork(_) => LaunchKind::Fork,
            Launch::ContinueLatest => LaunchKind::ContinueLatest,
            Launch::Attach(_) => LaunchKind::Attach,
        }
    }
}

pub trait Harness: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn supports(&self, kind: LaunchKind) -> bool;
    fn command(&self, launch: &Launch, cwd: &Path) -> Option<CommandSpec>;
}

pub struct ClaudeCode {
    binary: String,
}

pub fn claude_binary() -> String {
    std::env::var(CLAUDE_BIN_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| CLAUDE_DEFAULT_BIN.to_owned())
}

pub fn claude_binary_is_overridden() -> bool {
    claude_binary() != CLAUDE_DEFAULT_BIN
}

impl ClaudeCode {
    pub fn from_env() -> Self {
        Self { binary: claude_binary() }
    }

    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self { binary: binary.into() }
    }
}

fn inherited_session_vars() -> impl Iterator<Item = String> {
    std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| {
            name == INHERITED_SESSION_NAME
                || INHERITED_SESSION_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
        })
}

impl Harness for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn supports(&self, _kind: LaunchKind) -> bool {
        true
    }

    fn command(&self, launch: &Launch, cwd: &Path) -> Option<CommandSpec> {
        if !self.supports(launch.kind()) {
            return None;
        }
        let mut spec = CommandSpec::new(&self.binary).cwd(cwd);
        if !matches!(launch, Launch::Attach(_)) {
            spec = spec
                .env_remove(inherited_session_vars().filter(|name| name != DISABLE_AGENT_VIEW_ENV))
                .env(DISABLE_AGENT_VIEW_ENV, "1");
        }
        Some(match launch {
            Launch::New => spec,
            Launch::Resume(id) => spec.args(["--resume", id]),
            Launch::Fork(id) => spec.args(["--resume", id, "--fork-session"]),
            Launch::ContinueLatest => spec.args(["--continue"]),
            Launch::Attach(id) => spec.args(["attach", id]),
        })
    }
}

pub struct CodexCli;

pub struct OpenCodeCli;

impl Harness for CodexCli {
    fn id(&self) -> &'static str {
        HarnessId::Codex.as_str()
    }

    fn display_name(&self) -> &'static str {
        "Codex"
    }

    fn supports(&self, kind: LaunchKind) -> bool {
        kind != LaunchKind::Attach
    }

    fn command(&self, launch: &Launch, cwd: &Path) -> Option<CommandSpec> {
        Some(match launch {
            Launch::New => codex::new_session(cwd),
            Launch::Resume(id) => codex::resume_session(id, cwd),
            Launch::Fork(id) => codex::fork_session(id, cwd),
            Launch::ContinueLatest => codex::continue_latest(cwd),
            Launch::Attach(_) => return None,
        })
    }
}

impl Harness for OpenCodeCli {
    fn id(&self) -> &'static str {
        HarnessId::OpenCode.as_str()
    }

    fn display_name(&self) -> &'static str {
        "OpenCode"
    }

    fn supports(&self, kind: LaunchKind) -> bool {
        kind != LaunchKind::Attach
    }

    fn command(&self, launch: &Launch, cwd: &Path) -> Option<CommandSpec> {
        Some(match launch {
            Launch::New => opencode::new_session(cwd),
            Launch::Resume(id) => opencode::resume_session(id, cwd),
            Launch::Fork(id) => opencode::fork_session(id, cwd),
            Launch::ContinueLatest => opencode::continue_latest(cwd),
            Launch::Attach(_) => return None,
        })
    }
}

pub fn default_harness() -> Box<dyn Harness> {
    Box::new(ClaudeCode::from_env())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(launch: Launch) -> CommandSpec {
        ClaudeCode::with_binary("fake-claude")
            .command(&launch, Path::new("/work/app"))
            .unwrap()
    }

    #[test]
    fn claude_builds_the_documented_argv_for_every_launch() {
        let cases = [
            (Launch::New, vec![]),
            (Launch::Resume("abc".into()), vec!["--resume", "abc"]),
            (Launch::Fork("abc".into()), vec!["--resume", "abc", "--fork-session"]),
            (Launch::ContinueLatest, vec!["--continue"]),
            (Launch::Attach("bb004089".into()), vec!["attach", "bb004089"]),
        ];
        for (launch, args) in cases {
            let spec = command(launch.clone());
            assert_eq!(spec.program, "fake-claude");
            assert_eq!(spec.args, args, "{launch:?}");
            assert_eq!(spec.cwd.as_deref(), Some(Path::new("/work/app")));
        }
    }

    #[test]
    fn agent_view_is_disabled_for_everything_but_attach() {
        let disabled = |launch| {
            command(launch)
                .env
                .contains(&(DISABLE_AGENT_VIEW_ENV.to_owned(), "1".to_owned()))
        };
        for launch in [Launch::New, Launch::Resume("a".into()), Launch::Fork("a".into()), Launch::ContinueLatest] {
            assert!(disabled(launch.clone()), "{launch:?}");
        }
        assert!(!disabled(Launch::Attach("a".into())));
    }

    #[test]
    fn inherited_claude_session_variables_do_not_reach_the_child() {
        let dir = std::env::temp_dir().join(format!("ccview-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake-claude");
        let dump = dir.join("env.txt");
        std::fs::write(&fake, format!("#!/bin/sh\nenv > {}\n", dump.display())).unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        unsafe {
            std::env::set_var("CLAUDE_CODE_SESSION_PROBE", "x");
            std::env::set_var("CLAUDE_BG_PROBE", "x");
            std::env::set_var("CLAUDECODE", "1");
            std::env::set_var("CLAUDE_CODE_DISABLE_AGENT_VIEW", "inherited");
            std::env::set_var("CLAUDE_UNRELATED_PROBE", "kept");
        }
        let spec = ClaudeCode::with_binary(fake.to_str().unwrap())
            .command(&Launch::New, &dir)
            .unwrap();
        let host = crate::terminal::PtyHost::spawn(&spec, crate::terminal::Size { rows: 5, cols: 40 }).unwrap();
        host.wait_for_exit(std::time::Duration::from_secs(10)).unwrap();
        let env = std::fs::read_to_string(&dump).unwrap();
        for gone in ["CLAUDE_CODE_SESSION_PROBE", "CLAUDE_BG_PROBE", "CLAUDECODE="] {
            assert!(!env.contains(gone), "{gone} leaked");
        }
        assert!(env.contains("CLAUDE_CODE_DISABLE_AGENT_VIEW=1"));
        assert!(env.contains("CLAUDE_UNRELATED_PROBE=kept"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
