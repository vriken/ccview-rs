use crate::sources::resolve_binary;

pub(crate) mod activity;
pub(crate) mod agents_cli;
pub(crate) mod asking;
pub(crate) mod dod;
pub(crate) mod paths;
pub(crate) mod registry;
pub(crate) mod scan;
pub(crate) mod subagents;
pub(crate) mod transcript;

pub(crate) const BINARY_ENV: &str = "CCVIEW_CLAUDE_BIN";
const DEFAULT_BINARY: &str = "claude";

pub(crate) fn binary() -> String {
    resolve_binary(BINARY_ENV, DEFAULT_BINARY)
}

pub(crate) fn binary_is_overridden() -> bool {
    binary() != DEFAULT_BINARY
}
