//! v4 diagnostics. Own type; serialises to the same JSON shape the CLI already
//! prints for v3 (span, message, severity), so tooling consumers stay uniform,
//! but the v4 grammar owns it and can evolve without touching v3.

use serde::Serialize;

use crate::span::Span;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
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
    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }
}
