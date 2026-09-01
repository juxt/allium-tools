//! v4 well-formedness (J1). Phase 4b: structural checks plus name resolution over
//! predicate bodies (using the `expr` grammar). `allium check` on a v4 set calls
//! [`check`]. Name-resolution findings are WARNINGS for now (not errors), so the
//! conformance score is not lowered by scope gaps while the pass is tuned; they
//! become errors once the corpus produces no false positives.

use std::collections::HashSet;
use std::collections::HashMap;

use crate::ast::{DeclKind, ItemKind, Module};
use crate::diagnostic::Diagnostic;
use crate::expr::{free_names, parse_predicate};
use crate::lexer::{lex, Tok};
use crate::parser::{parse, ParseResult};

/// Value/relation names the checker treats as always in scope.
const BUILTINS: &[&str] = &["none", "true", "false", "some", "no", "old"];

/// Parse, then run well-formedness and name resolution. What `allium check` runs for v4.
pub fn check(source: &str) -> ParseResult {
    let mut r = parse(source);
    r.diagnostics.append(&mut wellformedness(&r.module));
    r.diagnostics.append(&mut resolve_names(&r.module, source));
    r.diagnostics.append(&mut crate::types::typecheck(&r.module, source));
    r
}

/// Enum/state values appearing in any `{ … | … }` group (type annotations and
/// record-embedded enums). Collected source-wide so predicate references to a
/// state value (`status = processing`) resolve.
fn collect_enum_values(src: &str) -> HashSet<String> {
    let toks = lex(src);
    let mut out = HashSet::new();
    let mut stack: Vec<(bool, Vec<String>)> = Vec::new();
    for t in &toks {
        match &t.tok {
            Tok::LBrace => stack.push((false, Vec::new())),
            Tok::Pipe => {
                if let Some(f) = stack.last_mut() {
                    f.0 = true;
                }
            }
            Tok::Ident(s) => {
                if let Some(f) = stack.last_mut() {
                    f.1.push(s.clone());
                }
            }
            Tok::RBrace => {
                if let Some((has_pipe, idents)) = stack.pop() {
                    if has_pipe {
                        out.extend(idents);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Name resolution over predicate bodies. Every free name in a predicate must
/// Strip a leading `means` keyword from a raw body span (an `init means <pred>` body carries it, unlike
/// invariant bodies), so the resolver sees the predicate, not the keyword as a phantom free name.
fn strip_means(raw: &str) -> &str {
    let t = raw.trim_start();
    t.strip_prefix("means").filter(|r| r.starts_with(char::is_whitespace)).map(str::trim_start).unwrap_or(t)
}

/// resolve to something in scope: a declaration parameter, an item name, an
/// import alias, a state/enum value, or a builtin. Unresolved → warning.
fn resolve_names(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let enums = collect_enum_values(src);
    let aliases: HashSet<&str> = module
        .decls
        .iter()
        .filter(|d| d.kind == DeclKind::Import)
        .filter_map(|d| d.alias.as_deref())
        .collect();
    // Names brought in by `use "<path>"` — a stdlib of `given`/`state` definitions. Loading them here
    // makes the extensibility model usable at CHECK time (not only in the monitor): imported functions
    // resolve instead of false-flagging. Resolves paths as given (absolute or relative to CWD).
    let imported = imported_names(module);

    for d in &module.decls {
        if d.kind == DeclKind::Import {
            continue;
        }
        // Build the declaration's scope, generously.
        let mut scope: HashSet<String> = HashSet::new();
        scope.extend(BUILTINS.iter().map(|s| s.to_string()));
        scope.extend(enums.iter().cloned());
        scope.extend(aliases.iter().map(|s| s.to_string()));
        scope.extend(imported.iter().cloned());
        for p in d.params.iter().chain(d.satisfies.iter()) {
            scope.insert(p.name.clone());
        }
        for it in &d.items {
            if let Some(n) = &it.name {
                scope.insert(n.clone());
            }
        }
        // Payload fields of a sum/variant state observable are names too (`outputs` of `{ success
        // { outputs } | … }`); their guarded-access is checked separately by `variant_access`.
        scope.extend(crate::analyse::variant_fields_of(d, src).into_keys());
        // Entity variables — the bare-name arguments of observable applications (`instance` in
        // `status(instance)`) and quantifier-bound vars — are implicitly in scope: the analysis reasons
        // over a single representative entity per sort, so these are bound, not free. Collect them from
        // every predicate body in the declaration so a full-word entity var reads as declared.
        for it in &d.items {
            for span in it
                .body
                .iter()
                .chain(it.requires.iter())
                .chain(it.ensures.iter())
                .chain(it.where_pred.iter())
                .copied()
            {
                let (e, _) = parse_predicate(strip_means(span.slice(src)));
                crate::analyse::collect_entity_vars(&e, &mut scope);
            }
        }

        // Resolve each predicate body in the declaration.
        for it in &d.items {
            let spans: Vec<_> = match it.kind {
                ItemKind::Action => it.requires.iter().chain(it.ensures.iter()).copied().collect(),
                ItemKind::Invariant
                | ItemKind::Guarantee
                | ItemKind::Fault
                | ItemKind::Requirement
                | ItemKind::Axiom
                | ItemKind::Rely
                | ItemKind::Establish
                | ItemKind::Init => it.body.iter().copied().collect(),
                // State/Given bodies are types or definitions; skip for now.
                _ => Vec::new(),
            };
            for span in spans {
                let text = strip_means(span.slice(src));
                let (e, _pd) = parse_predicate(text);
                let mut bound = Vec::new();
                let mut names = Vec::new();
                free_names(&e, &mut bound, &mut names);
                for (n, _sp) in names {
                    if !scope.contains(&n) && !it.params.contains(&n) && !is_builtin_pred(&n) {
                        out.push(Diagnostic::warning(
                            it.span,
                            format!("`{n}` is not declared (name resolution, in `{}`)", d.name),
                        ));
                    }
                }
            }
        }
    }
    out
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

/// Built-in ordering/sequential predicates the checkers understand over the event/period timeline;
/// excluded from name-resolution so specs may use them without an explicit declaration.
fn is_builtin_pred(n: &str) -> bool {
    matches!(
        n,
        "before" | "precedes" | "after" | "follows" | "succ" | "successor" | "next" | "is_last" | "last"
            | "final" | "is_first" | "first" | "min" | "max" | "round"
    )
}

/// Names (given/state item names) brought in by `use "<path>"` imports, for name resolution.
/// Reads each imported file (path as given, or with a `.allium` suffix) and collects its item names.
fn imported_names(module: &Module) -> HashSet<String> {
    let mut out = HashSet::new();
    for d in module.decls.iter().filter(|d| d.kind == DeclKind::Import) {
        if d.name.is_empty() {
            continue;
        }
        for cand in [d.name.clone(), format!("{}.allium", d.name)] {
            if let Ok(src) = std::fs::read_to_string(&cand) {
                let m = parse(&src).module;
                for it in m.decls.iter().flat_map(|dd| dd.items.iter()) {
                    if let Some(n) = &it.name {
                        out.insert(n.clone());
                    }
                }
                break;
            }
        }
    }
    out
}
