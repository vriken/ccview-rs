mod key;
mod mouse;
mod paste;

pub use key::{encode_key, encode_key_with_flags, send_key};
pub use mouse::{encode_mouse, send_mouse};
pub use paste::{encode_paste, send_paste};
