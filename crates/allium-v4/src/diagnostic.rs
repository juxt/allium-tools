//! v4 diagnostics. Own type; the CLI serialises it through `v4_diagnostic_to_json`
//! into the unified LLM-facing shape (`code`, lowercased `severity`, `message`,
//! optional `fix`, and a resolved `location`), so tooling consumers stay uniform
//! but the v4 grammar owns the type and can evolve without touching v3.

use serde::Serialize;

use crate::span::Span;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    /// Advisory: coverage transparency and design suggestions. Not a problem to fix, so it never
    /// affects the exit code. Kept out of the warning stream so real problems stand out.
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    pub severity: Severity,
    /// Stable, documented slug a consumer can branch on (e.g. `vacuous-component`).
    /// `None` on diagnostics not yet assigned one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    /// The remedy, kept separate from `message` (what is wrong) so it reads as a
    /// directive. Structural only — never a domain-specific guess the checker cannot justify.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), severity: Severity::Error, code: None, fix: None }
    }
    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), severity: Severity::Warning, code: None, fix: None }
    }
    /// An advisory: coverage or a design suggestion. Never affects the exit code.
    pub fn info(span: Span, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), severity: Severity::Info, code: None, fix: None }
    }
    /// Attach a stable code. Chainable: `Diagnostic::warning(s, m).with_code("undeclared-name")`.
    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }
    /// Attach a structural remedy. Chainable.
    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }
    /// A real problem to fix: an error or a warning. Advisories (`Info`) are not problems, so they
    /// never fail the gate. `check`/`analyse` exit non-zero when any diagnostic `is_problem()`.
    pub fn is_problem(&self) -> bool {
        matches!(self.severity, Severity::Error | Severity::Warning)
    }
}
