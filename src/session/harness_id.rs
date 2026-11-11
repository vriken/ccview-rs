#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HarnessId {
    Claude,
    Codex,
    OpenCode,
}

impl HarnessId {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            HarnessId::Claude => "claude",
            HarnessId::Codex => "codex",
            HarnessId::OpenCode => "opencode",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        [HarnessId::Claude, HarnessId::Codex, HarnessId::OpenCode]
            .into_iter()
            .find(|harness| harness.as_str() == name)
    }
}
