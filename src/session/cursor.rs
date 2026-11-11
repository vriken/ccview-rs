#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cursor {
    Offset(u64),
    Row { created: i64, id: String },
}
