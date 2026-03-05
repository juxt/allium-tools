use serde::Serialize;

use crate::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    pub severity: Severity,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), severity: Severity::Error }
    }

    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), severity: Severity::Warning }
    }
}
