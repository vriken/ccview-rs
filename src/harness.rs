pub mod external_terminal;

use crate::{
    sources::{codex, opencode},
    terminal::CommandSpec,
};
use std::path::Path;

const CLAUDE_BIN_ENV: &str = "CCVIEW_CLAUDE_BIN";
const CLAUDE_DEFAULT_BIN: &str = "claude";
const DISABLE_AGENT_VIEW_ENV: &str = "CLAUDE_CODE_DISABLE_AGENT_VIEW";
const SESSION_IDENTITY_VARS: [&str; 9] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PID",
];

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
    fn display_name(&self) -> &'static str;
    fn supports(&self, kind: LaunchKind) -> bool;
    fn command(&self, launch: &Launch, cwd: &Path) -> Option<CommandSpec>;
}

pub struct ClaudeCode {
    binary: String,
    disable_agent_view: bool,
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
        Self::with_binary(claude_binary())
    }

    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            disable_agent_view: true,
        }
    }

    pub fn disable_agent_view(mut self, disable: bool) -> Self {
        self.disable_agent_view = disable;
        self
    }
}

impl Harness for ClaudeCode {
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
            spec = spec.env_remove(SESSION_IDENTITY_VARS.map(str::to_owned));
            if self.disable_agent_view {
                spec = spec.env(DISABLE_AGENT_VIEW_ENV, "1");
            }
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
    fn agent_view_is_disabled_for_everything_but_attach() {
        let disabled = |launch| {
            command(launch)
                .env
                .contains(&(DISABLE_AGENT_VIEW_ENV.to_owned(), "1".to_owned()))
        };
        for launch in [
            Launch::New,
            Launch::Resume("a".into()),
            Launch::Fork("a".into()),
            Launch::ContinueLatest,
        ] {
            assert!(disabled(launch.clone()), "{launch:?}");
        }
        assert!(!disabled(Launch::Attach("a".into())));
    }

    #[test]
    fn only_session_identity_variables_are_removed_for_a_claude_launch() {
        let removed = command(Launch::New).env_remove;
        for identity in [
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_PID",
        ] {
            assert!(
                removed.iter().any(|name| name == identity),
                "{identity} survives"
            );
        }
        for intent in [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            "CLAUDE_EFFORT",
            DISABLE_AGENT_VIEW_ENV,
        ] {
            assert!(
                !removed.iter().any(|name| name == intent),
                "{intent} was stripped"
            );
        }
    }

    #[test]
    fn attach_keeps_the_whole_environment() {
        assert!(command(Launch::Attach("a".into())).env_remove.is_empty());
    }
}
