pub mod analysis;
pub mod ast;
pub mod diagnostic;
pub mod lexer;
pub mod parser;
pub mod span;

pub use analysis::{
    analyze, analyze_with_cross_module, analyze_with_external_refs, analyse,
    analyse_with_cross_module, analyse_with_external_refs, collect_all_referenced_idents,
    collect_declared_names, collect_entity_field_schemas, collect_entity_status_schemas,
    collect_qualified_references,
    collect_referenced_trigger_names, collect_reverse_contributions, collect_trigger_outputs,
    AmbiguousImports, ReverseContributions,
};
pub use ast::Module;
pub use diagnostic::{AnalyseResult, Diagnostic, Finding};
pub use parser::{detect_version, parse, ParseResult};
pub use span::Span;
