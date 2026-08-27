//! v4 analyse — first slice: bounded case-split analysis.
//!
//! For a declaration whose actions carry `requires` guards, this checks whether the
//! guards form an EXHAUSTIVE and DISJOINT case-split over the boolean condition
//! space, by enumerating every assignment to the atomic conditions (bounded, so the
//! strength is `bounded`, not `proved`). A gap means a subject in some state matches
//! no action and silently falls through — e.g. a trade that mints no UTI. This is a
//! property reading cannot reliably settle across a dozen guards; enumeration is sound.
//!
//! Atoms are the leaf boolean terms of the guards (an application like `cleared(t)`,
//! a field, or a comparison), identified by a canonical string. `and`/`or`/`not`/
//! `implies` are interpreted; everything else is an opaque boolean atom. This is
//! exact for boolean-state guards (the waterfall) and an over-approximation where
//! guards use enum equalities (noted in the diagnostic).

use std::collections::BTreeSet;
use std::collections::HashMap;

use crate::ast::{ItemKind, Module};
use crate::diagnostic::Diagnostic;
use crate::expr::{BinOp, Expr, UnOp};
use crate::parser::ParseResult;

const MAX_ATOMS: usize = 16;

/// Parse + well-formedness + name resolution + case-split + rule-set consistency.
pub fn analyse(source: &str) -> ParseResult {
    let mut r = crate::check::check(source);
    r.diagnostics.append(&mut coverage(&r.module, source));
    r.diagnostics.append(&mut consistency(&r.module, source));
    r
}

fn canon(e: &Expr) -> String {
    match e {
        Expr::Name(s) => s.clone(),
        Expr::Int(n) => n.to_string(),
        Expr::SetLit(s) => s.clone(),
        Expr::Field { base, name } => format!("{}.{}", canon(base), name),
        Expr::App { head, args } => {
            let a: Vec<String> = args.iter().map(canon).collect();
            format!("{}({})", canon(head), a.join(", "))
        }
        Expr::Unary { op, e } => match op {
            UnOp::Not => format!("not {}", canon(e)),
            UnOp::Old => format!("old {}", canon(e)),
        },
        Expr::Binary { op, lhs, rhs } => format!("{} {} {}", canon(lhs), binop_str(op), canon(rhs)),
        Expr::Quant { .. } => "<quantified>".to_string(),
        Expr::Error => "<error>".to_string(),
    }
}

fn binop_str(op: &BinOp) -> &'static str {
    match op {
        BinOp::Implies => "implies",
        BinOp::Or => "or",
        BinOp::And => "and",
        BinOp::Eq => "=",
        BinOp::Ne => "<>",
        BinOp::In => "in",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
    }
}

/// Collect the leaf boolean atoms of a guard (splitting on and/or/not/implies).
fn collect_atoms(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Binary { op: BinOp::And | BinOp::Or | BinOp::Implies, lhs, rhs } => {
            collect_atoms(lhs, out);
            collect_atoms(rhs, out);
        }
        Expr::Unary { op: UnOp::Not, e } => collect_atoms(e, out),
        _ => {
            out.insert(canon(e));
        }
    }
}

/// Evaluate a guard under a boolean assignment to atoms.
fn eval(e: &Expr, assign: &HashMap<String, bool>) -> bool {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => eval(lhs, assign) && eval(rhs, assign),
        Expr::Binary { op: BinOp::Or, lhs, rhs } => eval(lhs, assign) || eval(rhs, assign),
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => !eval(lhs, assign) || eval(rhs, assign),
        Expr::Unary { op: UnOp::Not, e } => !eval(e, assign),
        other => *assign.get(&canon(other)).unwrap_or(&false),
    }
}

fn describe(atoms: &[String], mask: u64) -> String {
    atoms
        .iter()
        .enumerate()
        .map(|(i, a)| format!("{a}={}", if (mask >> i) & 1 == 1 { "T" } else { "F" }))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The literals of a purely-conjunctive guard: (atom canonical string, polarity).
/// `None` if the guard is not a conjunction of (possibly negated) atoms.
fn literals(e: &Expr) -> Option<Vec<(String, bool)>> {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            let mut a = literals(lhs)?;
            a.extend(literals(rhs)?);
            Some(a)
        }
        Expr::Unary { op: UnOp::Not, e } => match e.as_ref() {
            Expr::Binary { op: BinOp::And | BinOp::Or | BinOp::Implies, .. }
            | Expr::Unary { .. }
            | Expr::Quant { .. } => None,
            atom => Some(vec![(canon(atom), false)]),
        },
        Expr::Binary { op: BinOp::Or | BinOp::Implies, .. } | Expr::Quant { .. } => None,
        atom => Some(vec![(canon(atom), true)]),
    }
}

/// Two literal-sets contradict if some atom appears with opposite polarity in each.
fn contradict(a: &[(String, bool)], b: &[(String, bool)]) -> bool {
    a.iter().any(|(name, pol)| b.iter().any(|(n2, p2)| n2 == name && p2 != pol))
}

/// Dump, for every boolean assignment to the guards' atoms, which actions fire — so an
/// external oracle can check ROUTING FIDELITY (does the case-split send each state to the
/// intended outcome), a stronger property than the structural disjoint+exhaustive check.
/// v4-only; JSON: `{"atoms":[...ordered], "rows":[[firing action names] per mask]}` where
/// bit i of the mask is `atoms[i]`. Bounded to 16 atoms (65536 rows).
pub fn route_json(source: &str) -> String {
    let module = crate::check::check(source).module;
    let mut named: Vec<(String, Expr)> = Vec::new();
    for d in &module.decls {
        for it in &d.items {
            if it.kind == ItemKind::Action {
                if let Some(sp) = it.requires {
                    named.push((
                        it.name.clone().unwrap_or_else(|| "<anon>".into()),
                        crate::expr::parse_predicate(sp.slice(source)).0,
                    ));
                }
            }
        }
    }
    let mut set = BTreeSet::new();
    for (_, e) in &named {
        collect_atoms(e, &mut set);
    }
    let atoms: Vec<String> = set.into_iter().collect();
    let n = atoms.len();
    if n == 0 || n > 16 {
        return format!("{{\"error\":\"{n} atoms (route needs 1..=16)\",\"atoms\":[],\"rows\":[]}}");
    }
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut rows = String::new();
    for mask in 0u64..(1u64 << n) {
        let assign: HashMap<String, bool> =
            atoms.iter().enumerate().map(|(i, a)| (a.clone(), (mask >> i) & 1 == 1)).collect();
        let firing: Vec<String> =
            named.iter().filter(|(_, e)| eval(e, &assign)).map(|(nm, _)| format!("\"{}\"", esc(nm))).collect();
        if mask > 0 {
            rows.push(',');
        }
        rows.push('[');
        rows.push_str(&firing.join(","));
        rows.push(']');
    }
    let atoms_json: Vec<String> = atoms.iter().map(|a| format!("\"{}\"", esc(a))).collect();
    format!("{{\"atoms\":[{}],\"rows\":[{}]}}", atoms_json.join(","), rows)
}

/// Joint satisfiability of a component's stated constraints (invariant/requirement/axiom).
/// A rule set that NO state satisfies is contradictory: the rules cannot hold together.
/// This is a bug that emerges from rule INTERACTION and is invisible in any single rule,
/// which is why reading a dozen rules cannot settle it and enumeration can. Bounded, so
/// exact for independent boolean atoms; a `means` body using quantifiers is treated as an
/// opaque atom (imprecise but never a false alarm). On UNSAT, a minimal conflicting core
/// is reported by greedy removal so the operator sees exactly which rules clash.
pub fn consistency(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let rules: Vec<(String, Expr)> = d
            .items
            .iter()
            .filter(|it| matches!(it.kind, ItemKind::Invariant | ItemKind::Requirement | ItemKind::Axiom))
            .filter_map(|it| {
                it.body
                    .map(|sp| (it.name.clone().unwrap_or_else(|| "<anon>".into()), crate::expr::parse_predicate(sp.slice(src)).0))
            })
            .collect();
        if rules.len() < 2 {
            continue;
        }

        let mut set = BTreeSet::new();
        for (_, e) in &rules {
            collect_atoms(e, &mut set);
        }
        let atoms: Vec<String> = set.into_iter().collect();
        let n = atoms.len();
        if n == 0 || n > MAX_ATOMS {
            out.push(Diagnostic::warning(
                d.span,
                format!("rule set in `{}` not checked for consistency: {n} atoms (bounded needs 1..={MAX_ATOMS})", d.name),
            ));
            continue;
        }

        // SAT over the currently-active rules: return a satisfying assignment if one exists.
        let sat = |active: &[bool]| -> Option<u64> {
            (0u64..(1u64 << n)).find(|&mask| {
                let assign: HashMap<String, bool> =
                    atoms.iter().enumerate().map(|(i, a)| (a.clone(), (mask >> i) & 1 == 1)).collect();
                rules.iter().enumerate().all(|(k, (_, e))| !active[k] || eval(e, &assign))
            })
        };

        match sat(&vec![true; rules.len()]) {
            Some(mask) => out.push(Diagnostic::warning(
                d.span,
                format!("rule set in `{}` is jointly satisfiable over the bounded atom space (e.g. {}).", d.name, describe(&atoms, mask)),
            )),
            None => {
                // Minimal UNSAT core: drop each rule; keep it only if its removal restores SAT.
                let mut active = vec![true; rules.len()];
                for k in 0..rules.len() {
                    active[k] = false;
                    if sat(&active).is_some() {
                        active[k] = true;
                    }
                }
                let core: Vec<String> =
                    rules.iter().enumerate().filter(|(k, _)| active[*k]).map(|(_, (nm, _))| nm.clone()).collect();
                out.push(Diagnostic::warning(
                    d.span,
                    format!("rule set in `{}` is CONTRADICTORY: no state satisfies all constraints. Minimal conflicting core: {}. These rules cannot hold together.", d.name, core.join(", ")),
                ));
            }
        }
    }
    out
}

/// Case-split exhaustiveness + disjointness over each declaration's guarded actions.
pub fn coverage(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let named: Vec<(String, Expr)> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Action)
            .filter_map(|it| {
                it.requires
                    .map(|sp| (it.name.clone().unwrap_or_else(|| "<anon>".into()), crate::expr::parse_predicate(sp.slice(src)).0))
            })
            .collect();
        if named.len() < 2 {
            continue; // not a case-split
        }
        let names: Vec<String> = named.iter().map(|(n, _)| n.clone()).collect();
        let guards: Vec<Expr> = named.into_iter().map(|(_, e)| e).collect();

        let mut set = BTreeSet::new();
        for g in &guards {
            collect_atoms(g, &mut set);
        }
        let atoms: Vec<String> = set.into_iter().collect();
        let n = atoms.len();
        if n == 0 || n > MAX_ATOMS {
            out.push(Diagnostic::warning(
                d.span,
                format!("case-split in `{}` not checked: {n} atoms (bounded coverage needs 1..={MAX_ATOMS})", d.name),
            ));
            continue;
        }

        // Sound disjointness shortcut: two conjunctive guards are mutually exclusive if
        // they share a contradicting condition, and that holds regardless of domain.
        let lits: Vec<Option<Vec<(String, bool)>>> = guards.iter().map(literals).collect();
        let all_pairwise_contradict = (0..guards.len()).all(|i| {
            (i + 1..guards.len()).all(|j| matches!((&lits[i], &lits[j]), (Some(a), Some(b)) if contradict(a, b)))
        });

        // Bounded enumeration over the atom space: uncovered (gap) and multiply-covered
        // (overlap) combinations, each with a witness. EXACT for independent boolean
        // atoms (the decision-table case); OVER-APPROXIMATE where atoms are enum-exclusive
        // or relational, which is why exhaustiveness is reported as axiom-relative.
        let mut gaps = 0u64;
        let mut overlaps = 0u64;
        let mut gap_eg = None;
        let mut over_eg = None;
        let mut over_names: Vec<String> = Vec::new();
        for mask in 0u64..(1u64 << n) {
            let assign: HashMap<String, bool> =
                atoms.iter().enumerate().map(|(i, a)| (a.clone(), (mask >> i) & 1 == 1)).collect();
            let firing: Vec<usize> = guards.iter().enumerate().filter(|(_, g)| eval(g, &assign)).map(|(i, _)| i).collect();
            if firing.is_empty() {
                gaps += 1;
                gap_eg.get_or_insert_with(|| describe(&atoms, mask));
            } else if firing.len() >= 2 {
                overlaps += 1;
                if over_eg.is_none() {
                    over_eg = Some(describe(&atoms, mask));
                    over_names = firing.iter().map(|&i| names[i].clone()).collect();
                }
            }
        }
        let combos = 1u64 << n;

        // Disjointness verdict — one line always emitted for a detected case-split.
        if all_pairwise_contradict {
            out.push(Diagnostic::warning(
                d.span,
                format!("case-split in `{}` is DISJOINT (sound: every guard pair shares a contradicting condition).", d.name),
            ));
        } else if overlaps > 0 {
            out.push(Diagnostic::warning(
                d.span,
                format!("case-split in `{}` is NOT disjoint: actions {} both fire in {overlaps}/{combos} condition-combinations (e.g. {}) — an ambiguous classification.", d.name, over_names.join(" + "), over_eg.unwrap()),
            ));
        } else {
            out.push(Diagnostic::warning(
                d.span,
                format!("case-split in `{}` is disjoint over the bounded atom space (no combination matches two guards; exact for independent boolean conditions).", d.name),
            ));
        }
        // Exhaustiveness verdict — one line always emitted.
        if gaps > 0 {
            out.push(Diagnostic::warning(
                d.span,
                format!("case-split in `{}` may leave {gaps}/{combos} atom-combinations uncovered (e.g. {}) — a subject in that state matches no action. Bounded/axiom-relative: state the domain axioms (e.g. every cleared trade has a CCP) for a sound verdict.", d.name, gap_eg.unwrap()),
            ));
        } else {
            out.push(Diagnostic::warning(
                d.span,
                format!("case-split in `{}` is exhaustive over the bounded atom space (every combination matches an action).", d.name),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::analyse;

    fn msgs(src: &str) -> Vec<String> {
        analyse(src).diagnostics.into_iter().map(|d| d.message).collect()
    }
    fn any(src: &str, needle: &str) -> bool {
        msgs(src).iter().any(|m| m.contains(needle))
    }

    const HDR: &str = "-- allium: 4\ncomponent R\n  entity T\n  observable state a(T) : bool\n  observable state b(T) : bool\n  observable state c(T) : bool\n";

    #[test]
    fn consistency_flags_contradiction_with_minimal_core() {
        let src = format!(
            "{HDR}  invariant r1 means a(t) implies b(t)\n  invariant r2 means b(t) implies not c(t)\n  invariant r3 means a(t) and c(t)\n  invariant r4 means a(t) implies a(t)\nend\n"
        );
        assert!(any(&src, "is CONTRADICTORY"));
        // core is the three interacting rules, not the tautology r4
        let core = msgs(&src).into_iter().find(|m| m.contains("CONTRADICTORY")).unwrap();
        assert!(core.contains("r1") && core.contains("r2") && core.contains("r3"));
        assert!(!core.contains("r4"));
    }

    #[test]
    fn consistency_accepts_satisfiable_rule_set() {
        let src = format!(
            "{HDR}  invariant r1 means a(t) implies b(t)\n  invariant r2 means b(t) implies not c(t)\n  invariant r3 means a(t) implies c(t)\nend\n"
        );
        assert!(any(&src, "jointly satisfiable"));
        assert!(!any(&src, "is CONTRADICTORY"));
    }

    #[test]
    fn coverage_disjoint_exhaustive_split_is_clean() {
        let src = format!(
            "{HDR}  action x(t : T) requires a(t) ; ensures done(t)\n  action y(t : T) requires not a(t) ; ensures done(t)\nend\n"
        );
        assert!(any(&src, "is DISJOINT (sound"));
        assert!(any(&src, "is exhaustive"));
    }

    #[test]
    fn coverage_flags_overlap_and_gap() {
        let src = format!(
            "{HDR}  action x(t : T) requires a(t) ; ensures done(t)\n  action y(t : T) requires b(t) ; ensures done(t)\nend\n"
        );
        assert!(any(&src, "is NOT disjoint"));
        assert!(any(&src, "uncovered"));
    }
}
