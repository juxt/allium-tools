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

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::ast::{ItemKind, Module};
use crate::diagnostic::Diagnostic;
use crate::expr::{parse_predicate, BinOp, Expr, UnOp};
use crate::parser::ParseResult;

const MAX_ATOMS: usize = 16;

/// Parse + well-formedness + name resolution + case-split + rule-set consistency.
pub fn analyse(source: &str) -> ParseResult {
    let mut r = crate::check::check(source);
    r.diagnostics.append(&mut coverage(&r.module, source));
    r.diagnostics.append(&mut consistency(&r.module, source));
    r.diagnostics.append(&mut feasibility(&r.module, source));
    r.diagnostics.append(&mut preservation(&r.module, source));
    r.diagnostics.append(&mut crate::arith::arithmetic(&r.module, source));
    r.diagnostics.append(&mut crate::arith::reachability(&r.module, source));
    // The boolean consistency check treats arithmetic as opaque, so it can report a component
    // "jointly satisfiable" while the (stronger) arithmetic tier reports it CONTRADICTORY or
    // VACUOUSLY. That dual message is misleading and the elicit gate reads it. The arithmetic
    // verdict wins: drop the boolean reassurance for any component it overrules.
    let overruled: std::collections::HashSet<String> = r
        .diagnostics
        .iter()
        .filter(|d| d.message.contains("CONTRADICTORY") || d.message.contains("VACUOUSLY"))
        .filter_map(|d| first_backtick(&d.message))
        .collect();
    r.diagnostics.retain(|d| {
        !(d.message.contains("is jointly satisfiable")
            && first_backtick(&d.message).map_or(false, |n| overruled.contains(&n)))
    });
    r
}

/// The token inside the first pair of backticks in a message (a component name in our diagnostics).
fn first_backtick(msg: &str) -> Option<String> {
    let a = msg.find('`')? + 1;
    let b = msg[a..].find('`')? + a;
    Some(msg[a..b].to_string())
}

/// Names of the boolean-typed state/given items in a declaration, so the SAT encoder can tell a
/// boolean `=` (a biconditional it must encode) from an arithmetic one (an opaque atom for the LRA path).
pub(crate) fn bool_names_of(d: &crate::ast::Decl, src: &str) -> std::collections::HashSet<String> {
    d.items
        .iter()
        .filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given))
        .filter_map(|it| {
            let name = it.name.clone()?;
            let ty = it.body?.slice(src).trim().to_ascii_lowercase();
            (ty == "bool" || ty == "boolean").then_some(name)
        })
        .collect()
}

/// Inductive invariant preservation. For each action and each quantifier-free invariant, build the
/// one-step verification condition `inv(pre) ∧ guard(pre) ∧ effect ∧ ¬inv(post)` and ask whether it is
/// satisfiable. A state observable the action WRITES (appears bare, outside `old`, in `ensures`) becomes a
/// distinct post variable `X'`; everything the action does not touch keeps its pre variable, so the frame
/// is implicit. If the VC is satisfiable, the action can step from a state satisfying the invariant to one
/// that violates it — a missing-guard bug the field's model checkers catch and runtime monitoring cannot.
/// Arithmetic and quantifiers stay opaque to the SAT engine, so the check never manufactures a false alarm.
pub fn preservation(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let state_names: HashSet<String> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::State)
            .filter_map(|it| it.name.clone())
            .collect();
        let all_obs: HashSet<String> = d
            .items
            .iter()
            .filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given))
            .filter_map(|it| it.name.clone())
            .collect();
        let bool_base = bool_names_of(d, src);
        // Boolean names for the SAT encoder, plus the primed post versions (also boolean).
        let mut bnames = bool_base.clone();
        for n in bool_base.clone() {
            bnames.insert(format!("{n}'"));
        }
        let invariants: Vec<(String, Expr)> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Invariant)
            .filter_map(|it| it.body.map(|sp| (it.name.clone().unwrap_or_else(|| "<anon>".into()), parse_predicate(sp.slice(src)).0)))
            // Only the boolean fragment: an arithmetic invariant becomes opaque atoms whose post version
            // is unconstrained, which would make the violation query trivially satisfiable (a false
            // alarm). Skipping it is sound — the check simply says nothing about arithmetic preservation.
            .filter(|(_, e)| !has_quant(e) && boolean_fragment(e, &bool_base, &all_obs))
            .collect();
        if invariants.is_empty() {
            continue;
        }
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let aname = it.name.clone().unwrap_or_else(|| "<anon>".into());
            let ensures = match it.ensures {
                Some(sp) => parse_predicate(sp.slice(src)).0,
                None => continue,
            };
            let mut modified = HashSet::new();
            collect_writes(&ensures, false, &state_names, &mut modified);
            if modified.is_empty() {
                continue; // writes no state: cannot break any invariant
            }
            let guard = it.requires.map(|sp| parse_predicate(sp.slice(src)).0);
            let effect = prime(&ensures, &modified, false);
            for (iname, inv) in &invariants {
                if !mentions_any(inv, &modified) {
                    continue; // invariant untouched by this action
                }
                let inv_post = prime(inv, &modified, false);
                let violation = Expr::Unary { op: UnOp::Not, e: Box::new(inv_post) };
                let mut es: Vec<&Expr> = vec![inv, &effect, &violation];
                if let Some(g) = &guard {
                    es.push(g);
                }
                if let Some(m) = crate::sat::satisfiable(&es, &bnames) {
                    let pre: Vec<String> = m
                        .iter()
                        .filter(|(k, _)| !k.contains('\'') && !k.starts_with("old "))
                        .map(|(k, v)| format!("{k}={}", if *v { "T" } else { "F" }))
                        .collect();
                    out.push(Diagnostic::warning(
                        it.span,
                        format!(
                            "action `{aname}` in `{}` can break invariant `{iname}`: from a state satisfying it (e.g. {}), the action reaches a state that violates it. Add a guard.",
                            d.name,
                            pre.join(", ")
                        ),
                    ));
                }
            }
        }
    }
    out
}

/// Is `e` in the pure boolean fragment the SAT preservation check can decide soundly? No arithmetic
/// operators or literals, no ordering comparisons, every equality between two booleans, and every
/// observable it applies is boolean-typed. Anything else would rest on opaque atoms and could false-alarm.
fn boolean_fragment(e: &Expr, bool_names: &HashSet<String>, obs: &HashSet<String>) -> bool {
    match e {
        Expr::Int(_) | Expr::Dec(_, _) | Expr::Cond { .. } | Expr::Quant { .. } | Expr::Sum { .. } => false,
        Expr::Name(_) => true, // a bound entity variable or bool literal
        Expr::App { head, args } => {
            let head_ok = match &**head {
                Expr::Name(h) => !obs.contains(h) || bool_names.contains(h),
                _ => boolean_fragment(head, bool_names, obs),
            };
            head_ok && args.iter().all(|a| boolean_fragment(a, bool_names, obs))
        }
        Expr::Field { name, base } => (!obs.contains(name) || bool_names.contains(name)) && boolean_fragment(base, bool_names, obs),
        Expr::Unary { e, .. } => boolean_fragment(e, bool_names, obs),
        Expr::Binary { op, lhs, rhs } => match op {
            BinOp::And | BinOp::Or | BinOp::Implies => boolean_fragment(lhs, bool_names, obs) && boolean_fragment(rhs, bool_names, obs),
            BinOp::Eq | BinOp::Ne => {
                crate::sat::is_bool_valued(lhs, bool_names) && crate::sat::is_bool_valued(rhs, bool_names)
                    && boolean_fragment(lhs, bool_names, obs)
                    && boolean_fragment(rhs, bool_names, obs)
            }
            _ => false, // ordering comparisons and arithmetic operators
        },
        _ => false,
    }
}

/// True if `e` contains an explicit quantifier or aggregate (deferred by the preservation check).
fn has_quant(e: &Expr) -> bool {
    match e {
        Expr::Quant { .. } | Expr::Sum { .. } => true,
        Expr::Binary { lhs, rhs, .. } => has_quant(lhs) || has_quant(rhs),
        Expr::Unary { e, .. } => has_quant(e),
        Expr::Cond { cond, then_, els } => has_quant(cond) || has_quant(then_) || has_quant(els),
        Expr::App { head, args } => has_quant(head) || args.iter().any(has_quant),
        Expr::Field { base, .. } => has_quant(base),
        _ => false,
    }
}

/// Collect state names written by `ensures`: an observable appearing bare (outside `old`).
fn collect_writes(e: &Expr, in_old: bool, state: &HashSet<String>, out: &mut HashSet<String>) {
    match e {
        Expr::Unary { op: UnOp::Old, e } => collect_writes(e, true, state, out),
        Expr::Unary { e, .. } => collect_writes(e, in_old, state, out),
        Expr::App { head, args } => {
            if let Expr::Name(h) = &**head {
                if !in_old && state.contains(h) {
                    out.insert(h.clone());
                }
            }
            collect_writes(head, in_old, state, out);
            args.iter().for_each(|a| collect_writes(a, in_old, state, out));
        }
        Expr::Field { base, name } => {
            if !in_old && state.contains(name) {
                out.insert(name.clone());
            }
            collect_writes(base, in_old, state, out);
        }
        Expr::Name(n) => {
            if !in_old && state.contains(n) {
                out.insert(n.clone());
            }
        }
        Expr::Binary { lhs, rhs, .. } => {
            collect_writes(lhs, in_old, state, out);
            collect_writes(rhs, in_old, state, out);
        }
        Expr::Cond { cond, then_, els } => {
            collect_writes(cond, in_old, state, out);
            collect_writes(then_, in_old, state, out);
            collect_writes(els, in_old, state, out);
        }
        _ => {}
    }
}

/// Rewrite `e` to its post-state reading: an observable in `modified`, appearing outside `old`, is
/// primed (`X` -> `X'`); `old(X)` is stripped to the pre reading `X`; everything else is unchanged.
fn prime(e: &Expr, modified: &HashSet<String>, in_old: bool) -> Expr {
    match e {
        Expr::Unary { op: UnOp::Old, e } => prime(e, modified, true),
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(prime(e, modified, in_old)) },
        Expr::App { head, args } => {
            let head = match &**head {
                Expr::Name(h) if !in_old && modified.contains(h) => Box::new(Expr::Name(format!("{h}'"))),
                other => Box::new(prime(other, modified, in_old)),
            };
            Expr::App { head, args: args.iter().map(|a| prime(a, modified, in_old)).collect() }
        }
        Expr::Field { base, name } => {
            let name = if !in_old && modified.contains(name) { format!("{name}'") } else { name.clone() };
            Expr::Field { base: Box::new(prime(base, modified, in_old)), name }
        }
        Expr::Name(n) if !in_old && modified.contains(n) => Expr::Name(format!("{n}'")),
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op: op.clone(),
            lhs: Box::new(prime(lhs, modified, in_old)),
            rhs: Box::new(prime(rhs, modified, in_old)),
        },
        Expr::Cond { cond, then_, els } => Expr::Cond {
            cond: Box::new(prime(cond, modified, in_old)),
            then_: Box::new(prime(then_, modified, in_old)),
            els: Box::new(prime(els, modified, in_old)),
        },
        other => other.clone(),
    }
}

/// Does `e` reference any name in `names` (as an application head, field, or bare name)?
fn mentions_any(e: &Expr, names: &HashSet<String>) -> bool {
    match e {
        Expr::Name(n) => names.contains(n),
        Expr::App { head, args } => {
            (matches!(&**head, Expr::Name(h) if names.contains(h))) || mentions_any(head, names) || args.iter().any(|a| mentions_any(a, names))
        }
        Expr::Field { base, name } => names.contains(name) || mentions_any(base, names),
        Expr::Unary { e, .. } => mentions_any(e, names),
        Expr::Binary { lhs, rhs, .. } => mentions_any(lhs, names) || mentions_any(rhs, names),
        Expr::Cond { cond, then_, els } => mentions_any(cond, names) || mentions_any(then_, names) || mentions_any(els, names),
        _ => false,
    }
}

pub(crate) fn canon(e: &Expr) -> String {
    match e {
        Expr::Name(s) => s.clone(),
        Expr::Int(n) => n.to_string(),
        Expr::Dec(num, den) => format!("{num}/{den}"),
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
        Expr::Sum { body, .. } => format!("sum({})", canon(body)),
        Expr::Cond { cond, then_, els } => format!("if {} then {} else {}", canon(cond), canon(then_), canon(els)),
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
        BinOp::Div => "/",
        BinOp::Pow => "^",
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
        // Invariants and axioms must JOINTLY hold; requirements are handled by feasibility().
        let rules: Vec<(String, Expr)> = d
            .items
            .iter()
            .filter(|it| matches!(it.kind, ItemKind::Invariant | ItemKind::Axiom))
            .filter_map(|it| {
                it.body
                    .map(|sp| (it.name.clone().unwrap_or_else(|| "<anon>".into()), crate::expr::parse_predicate(sp.slice(src)).0))
            })
            .collect();
        if rules.len() < 2 {
            continue;
        }
        let bnames = bool_names_of(d, src);

        // Joint satisfiability via the dependency-free SAT engine (scales past enumeration).
        let subset = |active: &[bool]| -> Vec<&Expr> {
            rules.iter().enumerate().filter(|(k, _)| active[*k]).map(|(_, (_, e))| e).collect()
        };
        match crate::sat::satisfiable(&subset(&vec![true; rules.len()]), &bnames) {
            Some(m) => out.push(Diagnostic::warning(
                d.span,
                format!("rule set in `{}` is jointly satisfiable (e.g. {}).", d.name, crate::sat::describe(&m)),
            )),
            None => {
                // Minimal UNSAT core: drop each rule; keep it only if its removal restores SAT.
                let mut active = vec![true; rules.len()];
                for k in 0..rules.len() {
                    active[k] = false;
                    if crate::sat::satisfiable(&subset(&active), &bnames).is_some() {
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

/// Per-scenario feasibility against a contract. `axiom` items are background truths that
/// hold of every report; `requirement` items are report shapes the integrating system
/// declares it will emit. A requirement is INFEASIBLE if no report satisfies it together
/// with the axioms — the system plans to send reports the contract can never accept, an
/// integration defect surfaced at design time. On infeasibility a minimal blocking core of
/// axioms is reported (greedy). Bounded; exact for independent boolean atoms.
pub fn feasibility(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let pick = |kind: ItemKind| -> Vec<(String, Expr)> {
            d.items
                .iter()
                .filter(|it| it.kind == kind)
                .filter_map(|it| {
                    it.body
                        .map(|sp| (it.name.clone().unwrap_or_else(|| "<anon>".into()), crate::expr::parse_predicate(sp.slice(src)).0))
                })
                .collect()
        };
        let axioms = pick(ItemKind::Axiom);
        let reqs = pick(ItemKind::Requirement);
        if reqs.is_empty() {
            continue;
        }
        let bnames = bool_names_of(d, src);

        // Does some report satisfy `req` together with every active axiom? (SAT engine.)
        let sat = |rexpr: &Expr, active: &[bool]| -> Option<BTreeMap<String, bool>> {
            let mut es: Vec<&Expr> = axioms.iter().enumerate().filter(|(k, _)| active[*k]).map(|(_, (_, e))| e).collect();
            es.push(rexpr);
            crate::sat::satisfiable(&es, &bnames)
        };

        for (rname, rexpr) in &reqs {
            match sat(rexpr, &vec![true; axioms.len()]) {
                Some(m) => out.push(Diagnostic::warning(
                    d.span,
                    format!("requirement `{}` in `{}` is feasible under the contract (e.g. {}).", rname, d.name, crate::sat::describe(&m)),
                )),
                None => {
                    // Minimal blocking core: axioms whose removal restores feasibility.
                    let mut active = vec![true; axioms.len()];
                    for k in 0..axioms.len() {
                        active[k] = false;
                        if sat(rexpr, &active).is_some() {
                            active[k] = true; // removing k restored feasibility -> k is a blocker
                        }
                    }
                    let core: Vec<String> =
                        axioms.iter().enumerate().filter(|(k, _)| active[*k]).map(|(_, (nm, _))| nm.clone()).collect();
                    out.push(Diagnostic::warning(
                        d.span,
                        format!("requirement `{}` in `{}` is INFEASIBLE under the contract: no acceptable report satisfies it. Blocked by: {}. The integration would emit reports the contract rejects.", rname, d.name, core.join(", ")),
                    ));
                }
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
    fn preservation_flags_missing_guard_and_clears_guarded_action() {
        // An action that writes `captured` with no guard can break `captured => authed`.
        let bad = "-- allium: 4\ncomponent Pay\n  entity Txn\n  observable state authed(Txn) : bool\n  observable state captured(Txn) : bool\n  action capture\n    ensures captured(t)\n  invariant no_cap_without_auth means captured(t) implies authed(t)\nend\n";
        assert!(any(bad, "can break invariant `no_cap_without_auth`"), "{:?}", msgs(bad));
        assert!(any(bad, "action `capture`"), "{:?}", msgs(bad));
        // Adding the guard `requires authed(t)` makes it safe: no preservation finding.
        let good = "-- allium: 4\ncomponent Pay\n  entity Txn\n  observable state authed(Txn) : bool\n  observable state captured(Txn) : bool\n  action capture\n    requires authed(t)\n    ensures captured(t)\n  invariant no_cap_without_auth means captured(t) implies authed(t)\nend\n";
        assert!(!any(good, "can break"), "{:?}", msgs(good));
    }

    #[test]
    fn preservation_is_silent_on_arithmetic_invariants() {
        // An arithmetic invariant rests on opaque atoms; the check must NOT false-alarm on it.
        let src = "-- allium: 4\ncomponent Bank\n  entity A\n  observable state bal(A) : Money\n  observable state amt(A) : Money\n  action withdraw\n    ensures bal(a) = old(bal(a)) - amt(a)\n  invariant non_negative means bal(a) >= 0\nend\n";
        assert!(!any(src, "can break"), "{:?}", msgs(src));
    }

    #[test]
    fn preservation_ignores_actions_that_write_unrelated_state() {
        // `authorize` writes `authed`, which cannot break `captured => authed` (it can only help).
        let src = "-- allium: 4\ncomponent Pay\n  entity Txn\n  observable state authed(Txn) : bool\n  observable state captured(Txn) : bool\n  action authorize\n    ensures authed(t)\n  invariant no_cap_without_auth means captured(t) implies authed(t)\nend\n";
        assert!(!any(src, "can break"), "{:?}", msgs(src));
    }

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
    fn boolean_satisfiable_suppressed_when_arithmetic_overrules() {
        // Guarded floor-above-cap: boolean consistency (opaque) would say "jointly satisfiable",
        // but the arithmetic tier reports VACUOUSLY. The misleading boolean line must be dropped.
        let src = "-- allium: 4\ncomponent F\n  entity I\n  observable state fee(I) : Money\n  observable state on(I) : bool\n  invariant cap means every i :: on(i) implies fee(i) <= 10\n  invariant floor means every i :: on(i) implies fee(i) >= 20\nend\n";
        assert!(any(src, "VACUO"), "{:?}", msgs(src));
        assert!(!any(src, "jointly satisfiable"), "{:?}", msgs(src));
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
    fn feasibility_flags_infeasible_requirement_with_emergent_core() {
        // c requires b; d forbids b -> a report that is c-and-d is infeasible via a 2-rule
        // core, with no single axiom forbidding it.
        let src = format!(
            "{HDR}  axiom needs_b means c(t) implies b(t)\n  axiom forbids_b means a(t) implies not b(t)\n  requirement can_ship means c(t) and a(t)\n  requirement plain means b(t)\nend\n"
        );
        assert!(any(&src, "`can_ship`") && any(&src, "INFEASIBLE"));
        let bad = msgs(&src).into_iter().find(|m| m.contains("can_ship")).unwrap();
        assert!(bad.contains("needs_b") && bad.contains("forbids_b"));
        assert!(any(&src, "`plain`") && any(&src, "feasible under the contract"));
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
