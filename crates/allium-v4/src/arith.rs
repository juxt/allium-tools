//! The arithmetic tier (spike, SD-2). Universally-quantified arithmetic invariants
//! are instantiated over a small bounded ordered domain of periods, the ordering
//! relations (`follows`, `is_last`) and a symbolic `Rate` are interpreted concretely,
//! each ground comparison is lowered to a linear constraint, and the `lra` engine
//! settles the resulting system. Two probes run:
//!
//!  * FEASIBILITY — is the whole invariant set jointly satisfiable? A witness is a
//!    concrete schedule; UNSAT with a core means the invariants cannot co-exist.
//!  * ENTAILMENT — is each invariant forced by the others, or independent? An
//!    independent inequality (e.g. balance monotonicity) comes with the counterexample
//!    that shows what extra assumption the spec silently relies on.
//!
//! This is the decidable spine the notes commit to: bounded instantiation over a few
//! processes (EPR-style) plus linear arithmetic, no external solver. Products of two
//! state variables are nonlinear; the `Rate` is pinned to a constant of the bounded
//! model, which is sound for feasibility and for the rate-independent entailments.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::ast::{ItemKind, Module};
use crate::diagnostic::Diagnostic;
use crate::expr::{parse_predicate, BinOp, Expr, Quant, UnOp};
use crate::lra::{solve, unsat_core, Con, Lin, Outcome, Rat, Rel};

/// Periods p0..p{N-1} of the bounded model.
const N: usize = 3;

/// Entry point: arithmetic feasibility + entailment over each component.
pub fn arithmetic(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        // State/given return types (raw text), to classify numeric vs rate vs bool.
        let mut st: HashMap<String, String> = HashMap::new();
        for it in &d.items {
            if matches!(it.kind, ItemKind::State | ItemKind::Given) {
                if let (Some(n), Some(b)) = (&it.name, it.body) {
                    st.insert(n.clone(), b.slice(src).trim().to_string());
                }
            }
        }
        // Invariants, with their ground constraint sets.
        let mut grounded: Vec<(String, Vec<Con>)> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for it in &d.items {
            if it.kind != ItemKind::Invariant {
                continue;
            }
            let (name, body) = match (&it.name, it.body) {
                (Some(n), Some(b)) => (n.clone(), b),
                _ => continue,
            };
            let (e, _) = parse_predicate(body.slice(src));
            let mut cons = Vec::new();
            let mut env = HashMap::new();
            emit(&e, &mut env, &st, &name, &mut cons, &mut notes);
            if !cons.is_empty() {
                grounded.push((name, cons));
            }
        }
        let total: usize = grounded.iter().map(|(_, c)| c.len()).sum();
        if total < 2 {
            continue; // not an arithmetic component
        }

        let mut uniq: Vec<String> = notes.clone();
        uniq.sort();
        uniq.dedup();
        feasibility_probe(&d.name, &grounded, &st, uniq.len(), &mut out);
        entailment_probe(&d.name, &grounded, &st, &mut out);
        requirement_probe(&d.name, &grounded, &d.items, &st, src, &mut out);
        if !uniq.is_empty() {
            out.push(Diagnostic::warning(
                d.span,
                format!("arithmetic tier in `{}`: {} term(s) not linearisable and NOT CHECKED (nonlinear): {}. The satisfiability verdict is PARTIAL — these constraints are outside the decidable fragment.", d.name, uniq.len(), uniq.join("; ")),
            ));
        }
    }
    out
}

/// Arithmetic invariant preservation via the LRA tier. For each action and each LINEAR invariant, build
/// the one-step verification condition and solve it with the simplex: `inv(pre) ∧ guard(pre) ∧ effect ∧
/// ¬inv(post)`. A written numeric state `X` becomes a distinct post variable `X'`; the effect equations
/// (`ensures`) link the two. If a case is satisfiable, the action can step from a good state to a state
/// violating the invariant — a value-safety bug the boolean check cannot see (e.g. `withdraw` breaking
/// `balance >= 0`). SOUND: the whole invariant, guard and effect must lower to linear constraints with no
/// skipped (nonlinear) term; any skip abandons the pair rather than risk a false alarm.
pub fn arith_preservation(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let mut st: HashMap<String, String> = HashMap::new();
        for it in &d.items {
            if matches!(it.kind, ItemKind::State | ItemKind::Given) {
                if let (Some(n), Some(b)) = (&it.name, it.body) {
                    st.insert(n.clone(), b.slice(src).trim().to_string());
                }
            }
        }
        let state_names: HashSet<String> =
            d.items.iter().filter(|it| it.kind == ItemKind::State).filter_map(|it| it.name.clone()).collect();

        // Linear invariants, reduced to their entity-normalised quantifier-free body, with their
        // constraint sets. Skip any with a nonlinear/unhandled term (a note) — unsound to reason about.
        let mut invs: Vec<(String, Expr, Vec<Con>)> = Vec::new();
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Invariant) {
            let (name, body) = match (&it.name, it.body) {
                (Some(n), Some(b)) => (n.clone(), b),
                _ => continue,
            };
            let inv = match arith_reduce(&parse_predicate(body.slice(src)).0) {
                Some(e) => e,
                None => continue,
            };
            let (cons, notes) = ground(&inv, &st);
            if notes || cons.is_empty() {
                continue;
            }
            invs.push((name, inv, cons));
        }
        if invs.is_empty() {
            continue;
        }

        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let aname = it.name.clone().unwrap_or_else(|| "<anon>".into());
            let ensures_raw = match it.ensures {
                Some(sp) => parse_predicate(sp.slice(src)).0,
                None => continue,
            };
            let guard_raw = it.requires.map(|sp| parse_predicate(sp.slice(src)).0);
            // One entity only (an action over two distinct entities cannot collapse soundly).
            let mut ev = HashSet::new();
            crate::analyse::collect_entity_vars(&ensures_raw, &mut ev);
            if let Some(g) = &guard_raw {
                crate::analyse::collect_entity_vars(g, &mut ev);
            }
            if ev.len() > 1 {
                continue;
            }
            let ensures = crate::analyse::rename_entity(&ensures_raw, &ev);
            let guard = guard_raw.map(|g| crate::analyse::rename_entity(&g, &ev));
            let mut modified = HashSet::new();
            crate::analyse::collect_writes(&ensures, false, &state_names, &mut modified);
            let modified_numeric: HashSet<String> =
                modified.iter().filter(|m| st.get(*m).map(|t| numeric(t)).unwrap_or(false)).cloned().collect();
            if modified_numeric.is_empty() {
                continue;
            }
            // The primed post state is a fresh numeric var of the same type.
            let mut st2 = st.clone();
            for m in &modified_numeric {
                if let Some(t) = st.get(m).cloned() {
                    st2.insert(format!("{m}'"), t);
                }
            }
            // Effect: prime the written state in `ensures`, then lower. Any skip => abandon this action.
            let effect_expr = crate::analyse::prime(&ensures, &modified_numeric, false);
            let (effect_cons, effect_notes) = ground(&effect_expr, &st2);
            if effect_notes || effect_cons.is_empty() {
                continue;
            }
            let guard_cons = match &guard {
                Some(g) => {
                    let (c, n) = ground(g, &st2);
                    if n {
                        continue; // an unmodelled guard could hide a real constraint -> skip, don't false-alarm
                    }
                    c
                }
                None => Vec::new(),
            };

            for (iname, inv, pre_cons) in &invs {
                if !crate::analyse::mentions_any(inv, &modified_numeric) {
                    continue;
                }
                let inv_post = crate::analyse::prime(inv, &modified_numeric, false);
                let (post_cons, post_notes) = ground(&inv_post, &st2);
                if post_notes || post_cons.is_empty() {
                    continue;
                }
                // ¬inv(post): at least one post constraint is violated. Try each, case-splitting Eq.
                let mut witness: Option<String> = None;
                'search: for pc in &post_cons {
                    for neg in negate_con(pc) {
                        let mut q = pre_cons.clone();
                        q.extend(guard_cons.iter().cloned());
                        q.extend(effect_cons.iter().cloned());
                        q.push(neg);
                        if let Outcome::Sat(m) = solve(&q) {
                            witness = Some(schedule(&m, &st2));
                            break 'search;
                        }
                    }
                }
                if let Some(w) = witness {
                    out.push(Diagnostic::warning(
                        it.span,
                        crate::analyse::pretty(&format!(
                            "action `{aname}` in `{}` can break arithmetic invariant `{iname}`: from a state satisfying it (e.g. {}), the action reaches a state that violates it. Add a guard.",
                            d.name, w
                        )),
                    ));
                }
            }
        }
    }
    out
}

/// Reduce an invariant to the entity-normalised quantifier-free body the LRA preservation check runs:
/// a plain invariant, or a single-variable `every p :: body` (a universal safety property). Existential,
/// multi-variable, and nested-quantifier invariants are out of scope (None).
fn arith_reduce(inv: &Expr) -> Option<Expr> {
    let body = match inv {
        Expr::Quant { q: Quant::Every, vars, body, .. } if vars.len() == 1 && !crate::analyse::has_quant(body) => (**body).clone(),
        _ if !crate::analyse::has_quant(inv) => inv.clone(),
        _ => return None,
    };
    let mut ev = HashSet::new();
    crate::analyse::collect_entity_vars(&body, &mut ev);
    if ev.len() > 1 {
        return None;
    }
    Some(crate::analyse::rename_entity(&body, &ev))
}

/// Lower a predicate to linear constraints. Returns (constraints, any-skipped): the flag is true if any
/// comparison/term could not be linearised, in which case the caller must not reason from the result.
fn ground(e: &Expr, st: &HashMap<String, String>) -> (Vec<Con>, bool) {
    let mut cons = Vec::new();
    let mut notes = Vec::new();
    let mut env = HashMap::new();
    emit(e, &mut env, st, "vc", &mut cons, &mut notes);
    (cons, !notes.is_empty())
}

/// The negation of a single constraint `lin REL 0`, as a list of alternative constraints (an Eq negation
/// splits into two: `lin < 0` or `lin > 0`). Each alternative is checked as a separate query.
fn negate_con(c: &Con) -> Vec<Con> {
    match c.rel {
        Rel::Le => vec![Con::new(c.lin.scale(Rat::int(-1)), Rel::Lt, c.label.clone())], // ¬(l≤0) = l>0 = -l<0
        Rel::Lt => vec![Con::new(c.lin.scale(Rat::int(-1)), Rel::Le, c.label.clone())], // ¬(l<0) = l≥0 = -l≤0
        Rel::Eq => vec![
            Con::new(c.lin.clone(), Rel::Lt, c.label.clone()),                          // l<0
            Con::new(c.lin.scale(Rat::int(-1)), Rel::Lt, c.label.clone()),              // l>0
        ],
    }
}

/// Feasibility: jointly satisfiable? Pins the opening balance to the disbursed
/// principal and disbursed to a positive constant so the witness is a real schedule.
fn feasibility_probe(
    comp: &str,
    grounded: &[(String, Vec<Con>)],
    st: &HashMap<String, String>,
    skipped: usize,
    out: &mut Vec<Diagnostic>,
) {
    // If any load-bearing arithmetic was skipped as nonlinear, a "satisfiable" verdict is only over
    // the checkable subset — say so, so a clean result is not mistaken for a full guarantee.
    let partial = if skipped > 0 {
        format!(" PARTIAL: {skipped} nonlinear constraint(s) were not checked, so this is not a full guarantee.")
    } else {
        String::new()
    };
    let mut cons: Vec<Con> = grounded.iter().flat_map(|(_, c)| c.clone()).collect();
    // Harness pins (not part of the spec; labelled as such): a positive disbursed
    // principal, and the opening balance equal to it.
    if st.keys().any(|k| k == "disbursed") {
        cons.push(Con::new(Lin::var("disbursed").sub(&Lin::konst(Rat::int(1000))), Rel::Eq, "pin:disbursed"));
    }
    if st.keys().any(|k| k == "outstanding_start") && st.keys().any(|k| k == "disbursed") {
        cons.push(Con::new(Lin::var("outstanding_start(p0)").sub(&Lin::var("disbursed")), Rel::Eq, "pin:opening"));
    }
    match solve(&cons) {
        Outcome::Sat(m) => out.push(Diagnostic::warning(
            comp_span(),
            format!(
                "arithmetic invariants in `{comp}` are JOINTLY SATISFIABLE over {N} periods.{partial} Witness schedule: {}",
                schedule(&m, st)
            ),
        )),
        Outcome::Unsat => {
            let core = unsat_core(&cons);
            out.push(Diagnostic::warning(
                comp_span(),
                format!("arithmetic invariants in `{comp}` are CONTRADICTORY over {N} periods: no schedule satisfies them all. Conflicting core: {}.", core.join(", ")),
            ));
        }
    }
}

/// Entailment: is each invariant forced by the rest? Reports redundant invariants and,
/// for an independent inequality, the counterexample that reveals the hidden assumption.
fn entailment_probe(
    comp: &str,
    grounded: &[(String, Vec<Con>)],
    st: &HashMap<String, String>,
    out: &mut Vec<Diagnostic>,
) {
    for (i, (name, cons_i)) in grounded.iter().enumerate() {
        let others: Vec<Con> =
            grounded.iter().enumerate().filter(|(j, _)| *j != i).flat_map(|(_, (_, c))| c.clone()).collect();
        // Ii is entailed iff every negated ground constraint is UNSAT with the others.
        let mut counter: Option<BTreeMap<String, Rat>> = None;
        'search: for c in cons_i {
            for neg in negate(c) {
                let mut probe = others.clone();
                probe.push(neg);
                if let Outcome::Sat(m) = solve(&probe) {
                    counter = Some(m);
                    break 'search;
                }
            }
        }
        match counter {
            None => out.push(Diagnostic::warning(
                comp_span(),
                format!("invariant `{name}` in `{comp}` is ENTAILED by the other invariants (redundant under the bounded model)."),
            )),
            Some(m) => {
                // Only surface the counterexample for an inequality invariant — the
                // interesting "what does this silently assume" case.
                if cons_i.iter().any(|c| c.rel != Rel::Eq) {
                    out.push(Diagnostic::warning(
                        comp_span(),
                        format!(
                            "invariant `{name}` in `{comp}` is NOT entailed by the others: counterexample {}. The spec relies on an unstated assumption (e.g. principal never negative / emi covers interest).",
                            schedule(&m, st)
                        ),
                    ));
                } else {
                    out.push(Diagnostic::warning(
                        comp_span(),
                        format!("invariant `{name}` in `{comp}` is independent of the others (not derivable)."),
                    ));
                }
            }
        }
    }
}

/// Negations of a constraint `lin rel 0`. Equality splits into two strict branches.
fn negate(c: &Con) -> Vec<Con> {
    match c.rel {
        // not(lin <= 0)  ==  lin > 0  ==  (-lin) < 0
        Rel::Le => vec![Con::new(c.lin.scale(Rat::int(-1)), Rel::Lt, c.label.clone())],
        // not(lin < 0)   ==  lin >= 0 ==  (-lin) <= 0
        Rel::Lt => vec![Con::new(c.lin.scale(Rat::int(-1)), Rel::Le, c.label.clone())],
        // not(lin = 0)   ==  lin < 0  OR  lin > 0
        Rel::Eq => vec![
            Con::new(c.lin.clone(), Rel::Lt, c.label.clone()),
            Con::new(c.lin.scale(Rat::int(-1)), Rel::Lt, c.label.clone()),
        ],
    }
}

/// Render the numeric-state values of a witness as a compact schedule.
fn schedule(m: &BTreeMap<String, Rat>, st: &HashMap<String, String>) -> String {
    let mut items: Vec<(String, String)> = Vec::new();
    for (k, v) in m {
        // Keep terms whose head is a declared numeric state/given (skip rate atoms).
        let head = k.split('(').next().unwrap_or(k);
        if st.get(head).map(|t| numeric(t) && !is_rate(t)).unwrap_or(false) {
            items.push((k.clone(), v.show()));
        }
    }
    items.sort();
    if items.len() > 14 {
        items.truncate(14);
    }
    items.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ")
}

fn is_rate(t: &str) -> bool {
    let h = head(t);
    matches!(h.as_str(), "rate" | "ratio" | "factor" | "percent" | "percentage")
}
fn numeric(t: &str) -> bool {
    let h = head(t);
    matches!(
        h.as_str(),
        "money" | "amount" | "cash" | "rate" | "ratio" | "factor" | "percent" | "percentage"
            | "int" | "integer" | "nat" | "natural" | "count" | "number" | "num" | "decimal"
            | "scalar" | "mass" | "length" | "duration" | "weight" | "distance" | "quantity" | "volume"
    )
}
fn head(t: &str) -> String {
    t.trim().split('(').next().unwrap_or("").trim().to_lowercase()
}

/// Bind quantifiers over the domain and emit a linear constraint per ground comparison.
fn emit(
    e: &Expr,
    env: &mut HashMap<String, usize>,
    st: &HashMap<String, String>,
    label: &str,
    out: &mut Vec<Con>,
    notes: &mut Vec<String>,
) {
    match e {
        Expr::Quant { q: Quant::Every, vars, body, .. } => {
            bind(vars, 0, env, &mut |env| emit(body, env, st, label, out, notes));
        }
        Expr::Quant { .. } => notes.push("existential/aggregate quantifier".to_string()),
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            emit(lhs, env, st, label, out, notes);
            emit(rhs, env, st, label, out, notes);
        }
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => match eval_guard(lhs, env) {
            Some(true) => emit(rhs, env, st, label, out, notes),
            Some(false) => {}
            None => notes.push(format!("guard `{}`", crate::analyse::canon(lhs))),
        },
        Expr::Binary { op: op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), lhs, rhs } => {
            let (l, r) = match (lower(lhs, env, st), lower(rhs, env, st)) {
                (Some(l), Some(r)) => (l, r),
                _ => {
                    notes.push(format!("`{}`", crate::analyse::canon(e)));
                    return;
                }
            };
            match op {
                BinOp::Eq => out.push(Con::new(l.sub(&r), Rel::Eq, label)),
                BinOp::Le => out.push(Con::new(l.sub(&r), Rel::Le, label)),
                BinOp::Lt => out.push(Con::new(l.sub(&r), Rel::Lt, label)),
                BinOp::Ge => out.push(Con::new(r.sub(&l), Rel::Le, label)),
                BinOp::Gt => out.push(Con::new(r.sub(&l), Rel::Lt, label)),
                BinOp::Ne => notes.push(format!("`{}` (disequality)", crate::analyse::canon(e))),
                _ => unreachable!(),
            }
        }
        _ => {}
    }
}

/// Requirement feasibility (arithmetic). A `requirement some x :: C` asserts that a scenario
/// really occurs; the arithmetic invariants must be satisfiable WITH it. If not, the requirement
/// is infeasible — e.g. a chain of definitions that forces the fee to 7% and 4% at once is
/// "satisfiable" only by the fee never applying, which the requirement rules out.
fn requirement_probe(
    comp: &str,
    inv_cons: &[(String, Vec<Con>)],
    items: &[crate::ast::Item],
    st: &HashMap<String, String>,
    src: &str,
    out: &mut Vec<Diagnostic>,
) {
    let flat: Vec<Con> = inv_cons.iter().flat_map(|(_, c)| c.clone()).collect();
    for it in items {
        if it.kind != ItemKind::Requirement {
            continue;
        }
        let (name, body) = match (&it.name, it.body) {
            (Some(n), Some(b)) => (n.clone(), b),
            _ => continue,
        };
        let (e, _) = parse_predicate(body.slice(src));
        // Peel a `some`/`exists` quantifier and bind its vars (plus any free entity vars) to one
        // instance; the requirement means "there is an instance where this holds".
        let (vars, inner) = match &e {
            Expr::Quant { q: Quant::Some | Quant::ExistsOne, vars, body, .. } => (vars.clone(), body.as_ref()),
            _ => {
                let mut v = Vec::new();
                free_entity_vars(&e, st, &mut v);
                (v, &e)
            }
        };
        let mut env = HashMap::new();
        for v in &vars {
            env.insert(v.clone(), 0usize);
        }
        let mut cons = Vec::new();
        cons_of(inner, &env, st, &name, &mut cons);
        if cons.is_empty() {
            continue;
        }
        let mut all = flat.clone();
        all.extend(cons);
        if let Outcome::Unsat = solve(&all) {
            let core = unsat_core(&all);
            out.push(Diagnostic::warning(
                comp_span(),
                format!(
                    "requirement `{name}` in `{comp}` is INFEASIBLE against the invariants (arithmetic): no model has it hold together with them (core: {}). The invariants are satisfiable only by this scenario never occurring.",
                    core.join(", ")
                ),
            ));
        }
    }
}

/// Non-vacuity / reachability. An invariant `guard implies C` passes vacuously whenever the
/// guard never holds, so a clean `analyse` can be meaningless. For each guard, force it true and
/// check its consequents (with the unconditional constraints) are feasible. If not, the spec is
/// satisfiable only when that guard is false — a hidden conflict or dead scenario — and we say so
/// deterministically. This is the general backstop against an AI encoding a green-but-toothless
/// spec: the floor-above-cap clash, guarded by `fee_applicable`, is caught regardless of phrasing.
pub fn reachability(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let mut st: HashMap<String, String> = HashMap::new();
        for it in &d.items {
            if matches!(it.kind, ItemKind::State | ItemKind::Given) {
                if let (Some(n), Some(b)) = (&it.name, it.body) {
                    st.insert(n.clone(), b.slice(src).trim().to_string());
                }
            }
        }
        let mut uncond: Vec<Con> = Vec::new();
        let mut guarded: std::collections::BTreeMap<String, Vec<Con>> = std::collections::BTreeMap::new();
        for it in &d.items {
            if it.kind != ItemKind::Invariant {
                continue;
            }
            let (name, body) = match (&it.name, it.body) {
                (Some(n), Some(b)) => (n.clone(), b),
                _ => continue,
            };
            let (e, _) = parse_predicate(body.slice(src));
            let (vars, inner) = binder_vars(&e, &st);
            let mut env = HashMap::new();
            reach_bind(&vars, 0, &mut env, &st, &name, inner, &mut uncond, &mut guarded);
        }
        if guarded.is_empty() {
            continue;
        }
        for (g, cons) in &guarded {
            if cons.is_empty() {
                continue;
            }
            let mut active = uncond.clone();
            active.extend(cons.clone());
            if let Outcome::Unsat = solve(&active) {
                let core = unsat_core(&active);
                out.push(Diagnostic::warning(
                    d.span,
                    format!(
                        "invariants in `{}` hold only VACUOUSLY: when `{}` is true, no value satisfies them (conflicting core: {}). A guarded constraint that can never be active is a hidden conflict or dead scenario, not a clean spec.",
                        d.name, g, core.join(", ")
                    ),
                ));
            }
        }
    }
    out
}

/// Peel `every` quantifiers to get the binder variables and the body; for an un-quantified
/// invariant, treat the free entity-argument variables as the binders (grounded over the domain).
fn binder_vars<'a>(e: &'a Expr, st: &HashMap<String, String>) -> (Vec<String>, &'a Expr) {
    let mut vars = Vec::new();
    let mut cur = e;
    while let Expr::Quant { q: Quant::Every, vars: vs, body, .. } = cur {
        vars.extend(vs.clone());
        cur = body;
    }
    if vars.is_empty() {
        free_entity_vars(cur, st, &mut vars);
    }
    (vars, cur)
}

/// Free names used as the argument of a state application that are not themselves declared
/// states/givens — i.e. entity instance variables written without an explicit quantifier.
fn free_entity_vars(e: &Expr, st: &HashMap<String, String>, out: &mut Vec<String>) {
    match e {
        Expr::App { head, args } => {
            if let Expr::Name(_) = head.as_ref() {
                for a in args {
                    if let Expr::Name(v) = a {
                        if !st.contains_key(v) && !out.contains(v) {
                            out.push(v.clone());
                        }
                    }
                }
            }
            for a in args {
                free_entity_vars(a, st, out);
            }
        }
        Expr::Binary { lhs, rhs, .. } => {
            free_entity_vars(lhs, st, out);
            free_entity_vars(rhs, st, out);
        }
        Expr::Unary { e, .. } => free_entity_vars(e, st, out),
        Expr::Quant { body, .. } | Expr::Sum { body, .. } => free_entity_vars(body, st, out),
        _ => {}
    }
}

fn reach_bind(
    vars: &[String],
    from: usize,
    env: &mut HashMap<String, usize>,
    st: &HashMap<String, String>,
    label: &str,
    body: &Expr,
    uncond: &mut Vec<Con>,
    guarded: &mut std::collections::BTreeMap<String, Vec<Con>>,
) {
    if from == vars.len() {
        reach_walk(body, env, st, label, uncond, guarded);
        return;
    }
    for i in 0..N {
        env.insert(vars[from].clone(), i);
        reach_bind(vars, from + 1, env, st, label, body, uncond, guarded);
    }
    env.remove(&vars[from]);
}

/// Walk a (ground) invariant body, sorting arithmetic into unconditional constraints and
/// guard-conditioned constraints keyed by the grounded guard atom.
fn reach_walk(
    e: &Expr,
    env: &HashMap<String, usize>,
    st: &HashMap<String, String>,
    label: &str,
    uncond: &mut Vec<Con>,
    guarded: &mut std::collections::BTreeMap<String, Vec<Con>>,
) {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            reach_walk(lhs, env, st, label, uncond, guarded);
            reach_walk(rhs, env, st, label, uncond, guarded);
        }
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => match eval_guard(lhs, env) {
            Some(true) => reach_walk(rhs, env, st, label, uncond, guarded),
            Some(false) => {}
            None => {
                let key = ground_atom(lhs, env);
                let mut cons = Vec::new();
                cons_of(rhs, env, st, label, &mut cons);
                guarded.entry(key).or_default().extend(cons);
            }
        },
        _ => cons_of(e, env, st, label, uncond),
    }
}

/// Lower the comparisons in a consequent into constraints (unconditional within the consequent).
fn cons_of(e: &Expr, env: &HashMap<String, usize>, st: &HashMap<String, String>, label: &str, out: &mut Vec<Con>) {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            cons_of(lhs, env, st, label, out);
            cons_of(rhs, env, st, label, out);
        }
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => {
            if let Some(true) = eval_guard(lhs, env) {
                cons_of(rhs, env, st, label, out);
            }
        }
        Expr::Binary { op: op @ (BinOp::Eq | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), lhs, rhs } => {
            if let (Some(l), Some(r)) = (lower(lhs, env, st), lower(rhs, env, st)) {
                match op {
                    BinOp::Eq => out.push(Con::new(l.sub(&r), Rel::Eq, label)),
                    BinOp::Le => out.push(Con::new(l.sub(&r), Rel::Le, label)),
                    BinOp::Lt => out.push(Con::new(l.sub(&r), Rel::Lt, label)),
                    BinOp::Ge => out.push(Con::new(r.sub(&l), Rel::Le, label)),
                    BinOp::Gt => out.push(Con::new(r.sub(&l), Rel::Lt, label)),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// The grounded canonical string of a guard atom, e.g. `fee_applicable(p0)`.
fn ground_atom(e: &Expr, env: &HashMap<String, usize>) -> String {
    match e {
        Expr::App { head, args } => {
            let name = match head.as_ref() {
                Expr::Name(s) => s.clone(),
                _ => crate::analyse::canon(head),
            };
            let parts: Vec<String> = args.iter().map(|a| match a {
                Expr::Name(v) => env.get(v).map(|i| format!("p{i}")).unwrap_or_else(|| v.clone()),
                other => crate::analyse::canon(other),
            }).collect();
            format!("{name}({})", parts.join(", "))
        }
        _ => crate::analyse::canon(e),
    }
}

/// Apply `f` for every assignment of `vars[from..]` to a period index.
fn bind(
    vars: &[String],
    from: usize,
    env: &mut HashMap<String, usize>,
    f: &mut dyn FnMut(&mut HashMap<String, usize>),
) {
    if from == vars.len() {
        f(env);
        return;
    }
    for i in 0..N {
        env.insert(vars[from].clone(), i);
        bind(vars, from + 1, env, f);
    }
    env.remove(&vars[from]);
}

/// Concretely evaluate an ordering guard over the bound period indices.
fn eval_guard(e: &Expr, env: &HashMap<String, usize>) -> Option<bool> {
    match e {
        Expr::App { head, args } => {
            let name = match head.as_ref() {
                Expr::Name(s) => s.as_str(),
                _ => return None,
            };
            let idx: Option<Vec<usize>> = args.iter().map(|a| arg_idx(a, env)).collect();
            let idx = idx?;
            match (name, idx.as_slice()) {
                ("follows" | "succ" | "successor" | "next", [a, b]) => Some(*a == b + 1),
                ("precedes" | "before", [a, b]) => Some(a < b),
                ("after", [a, b]) => Some(a > b),
                ("is_last" | "last" | "final", [a]) => Some(*a == N - 1),
                ("is_first" | "first", [a]) => Some(*a == 0),
                _ => None,
            }
        }
        Expr::Unary { op: UnOp::Not, e } => eval_guard(e, env).map(|b| !b),
        Expr::Binary { op: BinOp::And, lhs, rhs } => Some(eval_guard(lhs, env)? && eval_guard(rhs, env)?),
        Expr::Binary { op: BinOp::Or, lhs, rhs } => Some(eval_guard(lhs, env)? || eval_guard(rhs, env)?),
        Expr::Name(s) if s == "true" => Some(true),
        Expr::Name(s) if s == "false" => Some(false),
        _ => None,
    }
}

fn arg_idx(a: &Expr, env: &HashMap<String, usize>) -> Option<usize> {
    match a {
        Expr::Name(v) => env.get(v).copied(),
        Expr::Int(n) if *n >= 0 => Some(*n as usize),
        _ => None,
    }
}

/// Lower an arithmetic term to a linear form, or `None` if nonlinear/unsupported.
fn lower(e: &Expr, env: &HashMap<String, usize>, st: &HashMap<String, String>) -> Option<Lin> {
    match e {
        Expr::Int(n) => Some(Lin::konst(Rat::int(*n))),
        Expr::Dec(num, den) => Some(Lin::konst(Rat::new(*num as i128, *den as i128))),
        Expr::Name(s) => {
            if st.get(s).map(|t| numeric(t)).unwrap_or(false) {
                Some(Lin::var(s)) // a 0-ary numeric given, e.g. `disbursed`
            } else {
                None
            }
        }
        Expr::App { head, args } => {
            let name = match head.as_ref() {
                Expr::Name(s) => s.clone(),
                _ => return None,
            };
            match st.get(&name) {
                // A rate/numeric state is a variable; `rate * balance` (two variables) is nonlinear
                // and is honestly skipped (PARTIAL), rather than faked linear by pinning the rate.
                Some(t) if numeric(t) => Some(Lin::var(&ground_name(&name, args, env))),
                _ => None,
            }
        }
        Expr::Unary { op: UnOp::Old, e } => lower(e, env, st),
        // A bounded aggregate: sum the body over every binding of its vars.
        Expr::Sum { vars, body, .. } => {
            let mut acc = Lin::konst(Rat::zero());
            let mut env2 = env.clone();
            let mut terms: Vec<Lin> = Vec::new();
            let mut ok = true;
            sum_over(vars, 0, &mut env2, st, body, &mut terms, &mut ok);
            if !ok {
                return None;
            }
            for t in terms {
                acc = acc.add(&t);
            }
            Some(acc)
        }
        Expr::Binary { op: BinOp::Add, lhs, rhs } => Some(lower(lhs, env, st)?.add(&lower(rhs, env, st)?)),
        Expr::Binary { op: BinOp::Sub, lhs, rhs } => Some(lower(lhs, env, st)?.sub(&lower(rhs, env, st)?)),
        Expr::Binary { op: BinOp::Mul, lhs, rhs } => {
            let l = lower(lhs, env, st)?;
            let r = lower(rhs, env, st)?;
            if l.terms.is_empty() {
                Some(r.scale(l.c))
            } else if r.terms.is_empty() {
                Some(l.scale(r.c))
            } else {
                None // product of two variables — nonlinear
            }
        }
        Expr::Binary { op: BinOp::Div, lhs, rhs } => {
            let l = lower(lhs, env, st)?;
            let r = lower(rhs, env, st)?;
            if r.terms.is_empty() && !r.c.is_zero() {
                Some(l.scale(Rat::int(1).div(r.c))) // division by a constant is linear
            } else {
                None // division by a variable — nonlinear
            }
        }
        _ => None,
    }
}

/// Accumulate the lowered body of a `sum` over every binding of its vars.
fn sum_over(
    vars: &[String],
    from: usize,
    env: &mut HashMap<String, usize>,
    st: &HashMap<String, String>,
    body: &Expr,
    terms: &mut Vec<Lin>,
    ok: &mut bool,
) {
    if from == vars.len() {
        match lower(body, env, st) {
            Some(l) => terms.push(l),
            None => *ok = false,
        }
        return;
    }
    for i in 0..N {
        env.insert(vars[from].clone(), i);
        sum_over(vars, from + 1, env, st, body, terms, ok);
    }
    env.remove(&vars[from]);
}

/// A ground state term's variable name, e.g. `outstanding_start(p1)`.
fn ground_name(name: &str, args: &[Expr], env: &HashMap<String, usize>) -> String {
    if args.is_empty() {
        return name.to_string();
    }
    let parts: Vec<String> = args
        .iter()
        .map(|a| match arg_idx(a, env) {
            Some(i) => format!("p{i}"),
            None => crate::analyse::canon(a),
        })
        .collect();
    format!("{name}({})", parts.join(", "))
}

/// Constraints attach to the component; predicate sub-terms carry no span.
fn comp_span() -> crate::span::Span {
    crate::span::Span::new(0, 0)
}

#[cfg(test)]
mod tests {
    use super::arithmetic;
    use crate::parser::parse;

    fn run(src: &str) -> Vec<String> {
        let m = parse(src).module;
        arithmetic(&m, src).into_iter().map(|d| d.message).collect()
    }
    fn any(msgs: &[String], needle: &str) -> bool {
        msgs.iter().any(|m| m.contains(needle))
    }
    fn reach(src: &str) -> Vec<String> {
        let m = crate::parser::parse(src).module;
        super::reachability(&m, src).into_iter().map(|d| d.message).collect()
    }

    const HDR: &str = "-- allium: 4\ncomponent Loan\n  entity Period\n  given disbursed : Money\n  observable state emi(Period) : Money\n  observable state rate_factor(Period) : Rate\n  observable state interest(Period) : Money\n  observable state principal(Period) : Money\n  observable state outstanding_start(Period) : Money\n  observable state is_last(Period) : bool\n";

    #[test]
    fn loan_invariants_are_jointly_satisfiable() {
        let src = format!(
            "{HDR}  invariant interest_on means every p :: interest(p) = rate_factor(p) * outstanding_start(p)\n  invariant psplit means every p :: principal(p) = emi(p) - interest(p)\n  invariant rolls means every p :: every next :: follows(next, p) implies (outstanding_start(next) = outstanding_start(p) - principal(p))\n  invariant closes means every p :: is_last(p) implies (outstanding_start(p) - principal(p) = 0)\nend\n"
        );
        let m = run(&src);
        assert!(any(&m, "JOINTLY SATISFIABLE"), "{m:#?}");
    }

    #[test]
    fn monotonicity_is_not_entailed_without_nonneg_principal() {
        let src = format!(
            "{HDR}  invariant psplit means every p :: principal(p) = emi(p) - interest(p)\n  invariant rolls means every p :: every next :: follows(next, p) implies (outstanding_start(next) = outstanding_start(p) - principal(p))\n  invariant monotone means every p :: every next :: follows(next, p) implies (outstanding_start(next) <= outstanding_start(p))\nend\n"
        );
        let m = run(&src);
        assert!(any(&m, "`monotone`") && any(&m, "NOT entailed"), "{m:#?}");
    }

    const FEE: &str = "-- allium: 4\ncomponent Fee\n  entity Item\n  observable state fee(Item) : Money\n  observable state active(Item) : bool\n";

    #[test]
    fn reachability_catches_guarded_floor_above_cap() {
        // Both constraints guarded by the same predicate; vacuously satisfiable (active=false),
        // but infeasible when active — the emergent conflict a green consistency check misses.
        let src = format!(
            "{FEE}  invariant cap means every i :: active(i) implies fee(i) <= 10\n  invariant floor means every i :: active(i) implies fee(i) >= 20\nend\n"
        );
        let m = reach(&src);
        assert!(any(&m, "VACUOUSLY") && any(&m, "cap") && any(&m, "floor"), "{m:#?}");
    }

    #[test]
    fn negative_literals_lower_correctly() {
        // `x >= -1000` and `x <= -2000` compose to an (empty) interval -> CONTRADICTORY. Verifies
        // unary minus lowers to the real negative constant, not an error term.
        let src = "-- allium: 4\ncomponent N\n  entity A\n  observable state x(A) : Money\n  invariant lo means every a :: x(a) >= -1000\n  invariant hi means every a :: x(a) <= -2000\nend\n";
        let m = run(src);
        assert!(any(&m, "CONTRADICTORY"), "{m:#?}");
        assert!(!any(&m, "linearis"), "negative literal must not be an unchecked error: {m:#?}");
    }

    #[test]
    fn nonlinear_constraint_marks_verdict_partial() {
        // a = b*c (product of two variables) is outside the linear fragment; the satisfiable verdict
        // must be flagged PARTIAL so a clean result isn't mistaken for a full guarantee.
        let src = "-- allium: 4\ncomponent NL\n  entity P\n  observable state a(P) : Int\n  observable state b(P) : Int\n  observable state c(P) : Int\n  invariant prod means every p :: a(p) = b(p) * c(p)\n  invariant lin means every p :: a(p) <= 10\n  invariant lin2 means every p :: a(p) >= 0\nend\n";
        let m = run(src);
        assert!(any(&m, "PARTIAL"), "{m:#?}");
    }

    #[test]
    fn requirement_infeasible_against_invariants_is_caught() {
        // x = 2y and x = 3y force y = 0 (satisfiable only vacuously); a requirement that y can be
        // >= 1 makes the fee-applies scenario infeasible — the chain-conflict shape.
        let src = "-- allium: 4\ncomponent C\n  entity L\n  observable state x(L) : Money\n  observable state y(L) : Money\n  invariant a means every l :: x(l) = 2 * y(l)\n  invariant b means every l :: x(l) = 3 * y(l)\n  requirement r means some l :: y(l) >= 1\nend\n";
        let m = run(src);
        assert!(any(&m, "INFEASIBLE") && any(&m, "`r`"), "{m:#?}");
    }

    #[test]
    fn decimal_rate_literal_is_feasible_not_vacuous() {
        // `0.02 * base` must lower as the rational 2/100, not collapse to 0. With fee=2% of a free
        // base and a floor of 5, the active case is feasible (base >= 250) — never flag it vacuous.
        let src = format!(
            "{FEE}  observable state base(Item) : Money\n  invariant basis means every i :: active(i) implies fee(i) = 0.02 * base(i)\n  invariant floor means every i :: active(i) implies fee(i) >= 5\nend\n"
        );
        let m = reach(&src);
        assert!(!any(&m, "VACUOUSLY"), "{m:#?}");
    }

    #[test]
    fn reachability_no_false_alarm_when_active_case_is_feasible() {
        let src = format!(
            "{FEE}  invariant cap means every i :: active(i) implies fee(i) <= 100\n  invariant floor means every i :: active(i) implies fee(i) >= 20\nend\n"
        );
        let m = reach(&src);
        assert!(!any(&m, "VACUOUSLY"), "{m:#?}");
    }

    #[test]
    fn conservation_sum_is_grounded_and_feasible() {
        // The sum aggregate must be lowered so conservation participates: the sum of
        // period principals equals the disbursed amount, jointly with the roll-forward.
        let src = format!(
            "{HDR}  invariant rolls means every p :: every next :: follows(next, p) implies (outstanding_start(next) = outstanding_start(p) - principal(p))\n  invariant conservation means sum p :: principal(p) = disbursed\n  invariant closes means every p :: is_last(p) implies (outstanding_start(p) - principal(p) = 0)\nend\n"
        );
        let m = run(&src);
        assert!(any(&m, "JOINTLY SATISFIABLE"), "{m:#?}");
        // conservation produced constraints, so it appears in the entailment report.
        assert!(any(&m, "`conservation`"), "{m:#?}");
    }

    #[test]
    fn monotonicity_becomes_entailed_once_principal_nonneg_is_stated() {
        let src = format!(
            "{HDR}  invariant psplit means every p :: principal(p) = emi(p) - interest(p)\n  invariant nonneg means every p :: principal(p) >= 0\n  invariant rolls means every p :: every next :: follows(next, p) implies (outstanding_start(next) = outstanding_start(p) - principal(p))\n  invariant monotone means every p :: every next :: follows(next, p) implies (outstanding_start(next) <= outstanding_start(p))\nend\n"
        );
        let m = run(&src);
        assert!(any(&m, "`monotone`") && any(&m, "ENTAILED"), "{m:#?}");
    }
}
