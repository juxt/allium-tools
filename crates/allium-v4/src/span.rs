//! Byte-offset spans. Own copy, so the v4 grammar shares nothing with v3.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
    pub fn merge(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
    /// The source slice this span covers.
    pub fn slice(self, src: &str) -> &str {
        src.get(self.start..self.end).unwrap_or("")
    }
}
