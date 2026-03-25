pub mod analysis;
pub mod ast;
pub mod diagnostic;
pub mod lexer;
pub mod parser;
pub mod span;

pub use analysis::analyze;
pub use ast::Module;
pub use diagnostic::Diagnostic;
pub use parser::{parse, ParseResult};
pub use span::Span;
