use serde::Serialize;

/// Byte offset range in source text. Attached to every AST node
/// so diagnostics can point at the exact source location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}
