pub mod host;
pub mod input;
mod responder;
mod session;
pub mod session_host;
pub mod spec;
mod state;
pub mod widget;

pub use crate::sys::command::CommandSpec;
pub use host::{Attachment, ExitStatus, Output, PtyHost, Size, Snapshot};
pub use session::{TerminalSession, Waker};
pub use session_host::{
    client::{HostClient, SpawnRequest},
    protocol::SessionInfo,
    remote::RemoteHost,
    runtime_dir::RuntimeDir,
};
pub use spec::{Rgb, TerminalOptions};
pub use state::{KeyboardFlags, ModeSnapshot, TerminalEvent};
pub use widget::TerminalWidget;
