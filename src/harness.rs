use crate::terminal::CommandSpec;
use std::path::Path;

const CLAUDE_BIN_ENV: &str = "CCVIEW_CLAUDE_BIN";
const CLAUDE_DEFAULT_BIN: &str = "claude";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    New,
    Resume(String),
    Fork(String),
    ContinueLatest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    New,
    Resume,
    Fork,
    ContinueLatest,
}

impl Launch {
    pub fn kind(&self) -> LaunchKind {
        match self {
            Launch::New => LaunchKind::New,
            Launch::Resume(_) => LaunchKind::Resume,
            Launch::Fork(_) => LaunchKind::Fork,
            Launch::ContinueLatest => LaunchKind::ContinueLatest,
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

impl ClaudeCode {
    pub fn from_env() -> Self {
        let binary = std::env::var(CLAUDE_BIN_ENV)
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| CLAUDE_DEFAULT_BIN.to_owned());
        Self { binary }
    }

    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self { binary: binary.into() }
    }
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
        let spec = CommandSpec::new(&self.binary).cwd(cwd);
        Some(match launch {
            Launch::New => spec,
            Launch::Resume(id) => spec.args(["--resume", id]),
            Launch::Fork(id) => spec.args(["--resume", id, "--fork-session"]),
            Launch::ContinueLatest => spec.args(["--continue"]),
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
        ];
        for (launch, args) in cases {
            let spec = command(launch.clone());
            assert_eq!(spec.program, "fake-claude");
            assert_eq!(spec.args, args, "{launch:?}");
            assert_eq!(spec.cwd.as_deref(), Some(Path::new("/work/app")));
        }
    }
}
