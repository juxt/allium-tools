//! Allium v4 front end: a standalone lexer, parser, AST and diagnostics for the
//! v4 surface. Parallel to `allium-parser` (v3); it shares no grammar code, so the
//! v4 grammar can be reshaped freely. The CLI dispatches to this crate when a
//! check-set is all v4.

pub mod ast;
pub mod diagnostic;
pub mod lexer;
pub mod parser;
pub mod span;

pub use ast::Module;
pub use diagnostic::{Diagnostic, Severity};
pub use parser::{detect_version, parse, ParseResult};
pub use span::Span;
