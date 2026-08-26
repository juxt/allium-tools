//! v4 well-formedness (J1). Phase 4b start: structural checks that need no
//! predicate expression tree. Name resolution INSIDE predicates arrives in 4c
//! with the expression grammar; for now the rule is that no name is declared
//! twice. `allium check` on a v4 set calls [`check`].

use std::collections::HashMap;

use crate::ast::{DeclKind, Module};
use crate::diagnostic::Diagnostic;
use crate::parser::{parse, ParseResult};

/// Parse, then run well-formedness. What `allium check` runs for v4.
pub fn check(source: &str) -> ParseResult {
    let mut r = parse(source);
    r.diagnostics.append(&mut wellformedness(&r.module));
    r
}

/// Structural well-formedness diagnostics over a parsed module.
pub fn wellformedness(module: &Module) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    // No two top-level declarations share a name.
    let mut decl_names: HashMap<&str, ()> = HashMap::new();
    for d in &module.decls {
        if d.kind == DeclKind::Import || d.name.is_empty() {
            continue;
        }
        if decl_names.insert(d.name.as_str(), ()).is_some() {
            out.push(Diagnostic::error(
                d.span,
                format!("`{}` is declared more than once", d.name),
            ));
        }

        // Within a declaration, no two named items share a name.
        let mut item_names: HashMap<&str, ()> = HashMap::new();
        for it in &d.items {
            if let Some(name) = &it.name {
                if item_names.insert(name.as_str(), ()).is_some() {
                    out.push(Diagnostic::error(
                        it.span,
                        format!("`{name}` is declared more than once in `{}`", d.name),
                    ));
                }
            }
        }
    }

    // No two imports bind the same alias.
    let mut aliases: HashMap<&str, ()> = HashMap::new();
    for d in &module.decls {
        if d.kind == DeclKind::Import {
            if let Some(a) = &d.alias {
                if aliases.insert(a.as_str(), ()).is_some() {
                    out.push(Diagnostic::error(
                        d.span,
                        format!("alias `{a}` is bound more than once"),
                    ));
                }
            }
        }
    }

    out
}
