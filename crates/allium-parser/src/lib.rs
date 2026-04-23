pub mod analysis;
pub mod ast;
pub mod diagnostic;
pub mod lexer;
pub mod parser;
pub mod span;

pub use analysis::{
    analyze, analyze_with_cross_module, analyze_with_external_refs, analyse,
    analyse_with_cross_module, analyse_with_external_refs, collect_qualified_references,
};
pub use ast::Module;
pub use diagnostic::{AnalyseResult, Diagnostic, Finding};
pub use parser::{parse, ParseResult};
pub use span::Span;
