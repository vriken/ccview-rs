#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cursor {
    Offset(u64),
    Row { created: i64, id: String },
}
