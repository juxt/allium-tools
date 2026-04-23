use serde::Serialize;

use crate::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    pub severity: Severity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), severity: Severity::Error, code: None }
    }

    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), severity: Severity::Warning, code: None }
    }

    pub fn info(span: Span, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), severity: Severity::Info, code: None }
    }

    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }
}

// ---------------------------------------------------------------------------
// Findings: process-level analysis results for `allium analyse`
// ---------------------------------------------------------------------------

/// A finding is a flat JSON object built per-type by each `collect_*_findings`
/// method. Fields like `type`, `summary`, `affected_entities` and
/// `affected_transitions` sit alongside type-specific evidence fields.
pub type Finding = serde_json::Value;

/// Combined result from `allium analyse`.
#[derive(Debug, Clone, Serialize)]
pub struct AnalyseResult {
    pub diagnostics: Vec<Diagnostic>,
    pub findings: Vec<Finding>,
}
