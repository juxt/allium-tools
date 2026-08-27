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

/// Parse + well-formedness + name resolution + case-split analysis.
pub fn analyse(source: &str) -> ParseResult {
    let mut r = crate::check::check(source);
    r.diagnostics.append(&mut coverage(&r.module, source));
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

/// Case-split exhaustiveness + disjointness over each declaration's guarded actions.
pub fn coverage(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let guards: Vec<Expr> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Action)
            .filter_map(|it| it.requires.map(|sp| crate::expr::parse_predicate(sp.slice(src)).0))
            .collect();
        if guards.len() < 2 {
            continue; // not a case-split
        }

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

        // DISJOINTNESS — SOUND. Two conjunctive guards are mutually exclusive if they
        // share a contradicting condition (an atom asserted true in one, false in the
        // other). This holds regardless of domain, so it needs no enumeration and
        // yields no false positives. A pair with no contradicting condition MAY overlap.
        let lits: Vec<Option<Vec<(String, bool)>>> = guards.iter().map(literals).collect();
        let mut may_overlap: Vec<(usize, usize)> = Vec::new();
        let mut undetermined = 0usize;
        for i in 0..guards.len() {
            for j in (i + 1)..guards.len() {
                match (&lits[i], &lits[j]) {
                    (Some(a), Some(b)) => {
                        if !contradict(a, b) {
                            may_overlap.push((i, j));
                        }
                    }
                    _ => undetermined += 1,
                }
            }
        }
        if may_overlap.is_empty() && undetermined == 0 {
            out.push(Diagnostic::warning(
                d.span,
                format!("case-split in `{}` is DISJOINT (sound: every guard pair shares a contradicting condition).", d.name),
            ));
        } else if !may_overlap.is_empty() {
            out.push(Diagnostic::warning(
                d.span,
                format!("case-split in `{}`: {} guard-pair(s) share no contradicting condition and MAY overlap — verify mutual exclusivity or state a domain axiom.", d.name, may_overlap.len()),
            ));
        }

        // EXHAUSTIVENESS — bounded atom enumeration, honest about its limit: it is
        // RELATIVE to domain axioms it does not model (e.g. "every cleared trade has a
        // CCP"). A reported gap is a prompt to state the missing axiom, which is the
        // design-time value; a sound verdict is the typed quantified analysis (4c).
        let mut gaps = 0u64;
        let mut gap_eg = None;
        for mask in 0u64..(1u64 << n) {
            let assign: HashMap<String, bool> =
                atoms.iter().enumerate().map(|(i, a)| (a.clone(), (mask >> i) & 1 == 1)).collect();
            if guards.iter().all(|g| !eval(g, &assign)) {
                gaps += 1;
                gap_eg.get_or_insert_with(|| describe(&atoms, mask));
            }
        }
        if gaps > 0 {
            out.push(Diagnostic::warning(
                d.span,
                format!(
                    "case-split in `{}` may leave {gaps}/{} atom-combinations uncovered (e.g. {}) — a subject in that state matches no action. RELATIVE to domain axioms not modelled; state them (e.g. every cleared trade has a CCP) for a sound verdict, which is the typed quantified analysis (4c).",
                    d.name, 1u64 << n, gap_eg.unwrap()
                ),
            ));
        }
    }
    out
}
