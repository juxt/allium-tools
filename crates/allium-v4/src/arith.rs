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

use crate::ast::{ItemKind, Module};
use crate::diagnostic::Diagnostic;
use crate::expr::{parse_predicate, BinOp, Expr, Quant, UnOp};
use crate::lra::{solve, unsat_core, Con, Lin, Outcome, Rat, Rel};

/// Periods p0..p{N-1} of the bounded model.
const N: usize = 3;
/// The pinned per-period rate factor of the bounded model (10%).
fn rate() -> Rat {
    Rat::new(1, 10)
}

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

        feasibility_probe(&d.name, &grounded, &st, &mut out);
        entailment_probe(&d.name, &grounded, &st, &mut out);
        if !notes.is_empty() {
            let mut uniq: Vec<String> = notes.clone();
            uniq.sort();
            uniq.dedup();
            out.push(Diagnostic::warning(
                d.span,
                format!("arithmetic tier in `{}`: {} term(s) not linearisable and skipped: {}", d.name, uniq.len(), uniq.join("; ")),
            ));
        }
    }
    out
}

/// Feasibility: jointly satisfiable? Pins the opening balance to the disbursed
/// principal and disbursed to a positive constant so the witness is a real schedule.
fn feasibility_probe(
    comp: &str,
    grounded: &[(String, Vec<Con>)],
    st: &HashMap<String, String>,
    out: &mut Vec<Diagnostic>,
) {
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
                "arithmetic invariants in `{comp}` are JOINTLY SATISFIABLE over {N} periods (rate {}). Witness schedule: {}",
                rate().show(),
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
                Some(t) if is_rate(t) => Some(Lin::konst(rate())),
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
