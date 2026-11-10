#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HarnessId {
    Claude,
    Codex,
    OpenCode,
}

impl HarnessId {
    pub fn as_str(self) -> &'static str {
        match self {
            HarnessId::Claude => "claude",
            HarnessId::Codex => "codex",
            HarnessId::OpenCode => "opencode",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [HarnessId::Claude, HarnessId::Codex, HarnessId::OpenCode]
            .into_iter()
            .find(|harness| harness.as_str() == name)
    }
}
