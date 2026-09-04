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
pub fn arithmetic(module: &Module, src: &str, imports: &Imports) -> Vec<Diagnostic> {
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
        let defs = component_defs(d, src, imports);
        // Invariants, with their ground constraint sets.
        let mut grounded: Vec<(String, Vec<Con>)> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let mut rate_obs: HashSet<String> = HashSet::new();
        for it in &d.items {
            if it.kind != ItemKind::Invariant {
                continue;
            }
            let (name, body) = match (&it.name, it.body) {
                (Some(n), Some(b)) => (n.clone(), b),
                _ => continue,
            };
            let e = crate::monitor::inline_defs(&parse_predicate(body.slice(src)).0, &defs);
            rate_typed_products(&e, &st, &mut rate_obs);
            // A transition invariant (`watermark >= old(watermark)`) is a two-state property. These
            // single-state probes strip `old`, collapsing it to a tautology and misreporting it as
            // redundant. Skip it here; arith_preservation checks it soundly across each action.
            if crate::analyse::uses_old_expr(&e) {
                continue;
            }
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
            // Elicit: a Rate-typed per-period observable multiplied by a state is the usual reason a schedule
            // is only PARTIAL. If the rate is fixed, pinning it to a constant makes the relation checkable.
            let mut rates: Vec<String> = rate_obs.into_iter().collect();
            rates.sort();
            for obs in rates {
                out.push(Diagnostic::warning(
                    d.span,
                    format!("suggestion: `{obs}` is a per-period rate multiplied by a state, so its product is nonlinear and left unchecked. If the rate is fixed across periods, declare it as a constant (`given {obs} means <value>`) — the product then becomes linear and the relation is fully checkable at that rate."),
                ));
            }
        }
    }
    out
}

/// Elicit helper: collect the `Rate`-typed observables that appear as a factor in a product `rate * state`
/// (nonlinear, hence unchecked). A literal coefficient is fine (linear); a per-period rate is the flag.
fn rate_typed_products(e: &Expr, st: &HashMap<String, String>, out: &mut HashSet<String>) {
    if let Expr::Binary { op: BinOp::Mul, lhs, rhs } = e {
        for (factor, other) in [(lhs.as_ref(), rhs.as_ref()), (rhs.as_ref(), lhs.as_ref())] {
            if let Expr::App { head, .. } = factor {
                if let Expr::Name(obs) = head.as_ref() {
                    let rate = st.get(obs).map(|t| t == "Rate").unwrap_or(false);
                    if rate && !matches!(other, Expr::Int(_) | Expr::Dec(_, _)) {
                        out.insert(obs.clone());
                    }
                }
            }
        }
    }
    match e {
        Expr::Binary { lhs, rhs, .. } => {
            rate_typed_products(lhs, st, out);
            rate_typed_products(rhs, st, out);
        }
        Expr::Unary { e, .. } => rate_typed_products(e, st, out),
        Expr::Cond { cond, then_, els } => {
            rate_typed_products(cond, st, out);
            rate_typed_products(then_, st, out);
            rate_typed_products(els, st, out);
        }
        Expr::App { head, args } => {
            rate_typed_products(head, st, out);
            for a in args {
                rate_typed_products(a, st, out);
            }
        }
        Expr::Field { base, .. } => rate_typed_products(base, st, out),
        Expr::Quant { body, .. } | Expr::Sum { body, .. } => rate_typed_products(body, st, out),
        _ => {}
    }
}

/// Arithmetic invariant preservation via the LRA tier. For each action and each LINEAR invariant, build
/// the one-step verification condition and solve it with the simplex: `inv(pre) ∧ guard(pre) ∧ effect ∧
/// ¬inv(post)`. A written numeric state `X` becomes a distinct post variable `X'`; the effect equations
/// (`ensures`) link the two. If a case is satisfiable, the action can step from a good state to a state
/// violating the invariant — a value-safety bug the boolean check cannot see (e.g. `withdraw` breaking
/// `balance >= 0`). SOUND: the whole invariant, guard and effect must lower to linear constraints with no
/// skipped (nonlinear) term; any skip abandons the pair rather than risk a false alarm.
/// Measure-monotonicity check for objectives — a SOUND slice of progress verification. A claimed
/// progress `measure M decreasing` must never be INCREASED by an action, or it does not witness
/// progress toward the goal (corpus E28: "queue position is not monotone — new arrivals displace
/// waiting requests"). This pass ONLY reports a measure that CAN increase; it never certifies that a
/// measure proves progress (the positive direction — strict decrease + well-foundedness ⇒ termination —
/// is deferred). So it cannot manufacture a false discharge: every finding is a genuine counter-witness
/// (a reachable transition that raises the measure), and any construct it cannot lower to linear
/// arithmetic is skipped, never guessed.
pub fn objective_progress(module: &Module, src: &str, imports: &Imports) -> Vec<Diagnostic> {
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
        // Measures claimed by objectives, restricted to a bare NUMERIC state. A keyed or expression
        // measure (E27 lexicographic tuple) is skipped — conservative; the disposition note already
        // says such an objective is not yet verified.
        let measures: Vec<String> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Objective)
            .filter_map(|it| it.body.and_then(|b| crate::analyse::parse_objective_body(b.slice(src)).measure))
            .filter(|m| st.get(m).map(|t| numeric(t)).unwrap_or(false))
            .collect();
        if measures.is_empty() {
            continue;
        }
        let defs = component_defs(d, src, imports);
        let state_names: HashSet<String> =
            d.items.iter().filter(|it| it.kind == ItemKind::State).filter_map(|it| it.name.clone()).collect();
        // Pre-state context: the grounded linear invariants, so a measure-increase from an UNREACHABLE
        // pre-state (one an invariant forbids) is not reported. Dropping these would only over-report a
        // warning, never certify — but including them keeps the finding real.
        let mut all_pre: Vec<Con> = Vec::new();
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Invariant) {
            let Some(b) = it.body else { continue };
            let Some(inv) = arith_reduce(&crate::monitor::inline_defs(&parse_predicate(b.slice(src)).0, &defs)) else {
                continue;
            };
            let (cons, notes) = ground(&inv, &st);
            if !notes {
                all_pre.extend(cons);
            }
        }
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let aname = it.name.clone().unwrap_or_else(|| "<anon>".into());
            let Some(ensures_raw) = it.ensures_expr(src).map(|e| crate::monitor::inline_defs(&e, &defs)) else {
                continue;
            };
            let guard_raw = it.requires.map(|sp| crate::monitor::inline_defs(&parse_predicate(sp.slice(src)).0, &defs));
            let mut ev = HashSet::new();
            crate::analyse::collect_entity_vars(&ensures_raw, &mut ev);
            if let Some(g) = &guard_raw {
                crate::analyse::collect_entity_vars(g, &mut ev);
            }
            if ev.len() > 1 {
                continue; // two-entity action — not this slice
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
            let mut st2 = st.clone();
            for m in &modified_numeric {
                if let Some(t) = st.get(m).cloned() {
                    st2.insert(format!("{m}'"), t);
                }
            }
            let effect_expr = crate::analyse::prime(&ensures, &modified_numeric, false);
            let (effect_cons, effect_notes) = ground(&effect_expr, &st2);
            if effect_notes || effect_cons.is_empty() {
                continue;
            }
            let guard_cons = match &guard {
                Some(g) => {
                    let (c, n) = ground(g, &st2);
                    if n {
                        continue; // an unmodelled guard could hide a constraint — skip, don't false-alarm
                    }
                    c
                }
                None => Vec::new(),
            };
            for m in &measures {
                if !modified_numeric.contains(m) {
                    continue; // action leaves M framed — it cannot increase it
                }
                // VC: can this action raise the measure? all_pre ∧ guard ∧ effect ∧ (M' > M)
                let inc = Expr::Binary {
                    op: BinOp::Gt,
                    lhs: Box::new(Expr::Name(format!("{m}'"))),
                    rhs: Box::new(Expr::Name(m.clone())),
                };
                let (inc_cons, inc_notes) = ground(&inc, &st2);
                if inc_notes || inc_cons.is_empty() {
                    continue;
                }
                let mut q = all_pre.clone();
                q.extend(guard_cons.iter().cloned());
                q.extend(effect_cons.iter().cloned());
                q.extend(inc_cons);
                if let Outcome::Sat(wm) = solve(&q) {
                    out.push(Diagnostic::warning(
                        it.span,
                        crate::analyse::pretty(&format!(
                            "objective measure `{m}` can INCREASE under action `{aname}` in `{}` (e.g. {}) — it does not witness progress toward the goal (a non-monotone measure). Guard the increase, or use a measure that only decreases.",
                            d.name,
                            schedule(&wm, &st2)
                        )),
                    ));
                }
            }
        }
    }
    out
}

pub fn arith_preservation(module: &Module, src: &str, imports: &Imports) -> Vec<Diagnostic> {
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
        // Computed `given` definitions, inlined into invariants/effects so a derived value like
        // `available = limit - used` ties the invariant to the states an action actually changes.
        let defs = component_defs(d, src, imports);

        // Linear invariants, reduced to their entity-normalised quantifier-free body, with their
        // constraint sets. Skip any with a nonlinear/unhandled term (a note) — unsound to reason about.
        let mut invs: Vec<(String, Expr, Vec<Con>)> = Vec::new();
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Invariant) {
            let (name, body) = match (&it.name, it.body) {
                (Some(n), Some(b)) => (n.clone(), b),
                _ => continue,
            };
            let inv = match arith_reduce(&crate::monitor::inline_defs(&parse_predicate(body.slice(src)).0, &defs)) {
                Some(e) => e,
                None => continue,
            };
            // Derived linear bounds from a `min`/`max` equality — checkable even when the equality itself
            // is nonlinear, so a cap/floor violation is caught.
            for (i, bound) in minmax_bounds(&inv).into_iter().enumerate() {
                let (bcons, bn) = ground(&bound, &st);
                if !bn && !bcons.is_empty() {
                    invs.push((format!("{name}[bound {}]", i + 1), bound, bcons));
                }
            }
            let (cons, notes) = ground(&inv, &st);
            if notes || cons.is_empty() {
                continue;
            }
            invs.push((name, inv, cons));
        }
        if invs.is_empty() {
            continue;
        }
        // The pre-state assumes the WHOLE linear invariant set (prove the conjunction inductive), so an
        // invariant that is true-but-not-inductive alone is not spuriously flagged when another excludes
        // the bad pre-state. Sound: a reported break means the full set is genuinely not preserved.
        let mut all_pre: Vec<Con> = invs.iter().flat_map(|(_, _, c)| c.iter().cloned()).collect();

        // Relies (Decision 2, 2026-09-02): an invariant may be proved under a cited rely — a condition an
        // action's preservation is entitled to ASSUME. A linear rely enters the VC as a pre-state
        // hypothesis. It is ASSUMED (trusted, not checked here — v4 has no compositional rely-guarantee), so
        // every arithmetic verdict for a component carrying a load-bearing rely is reported conditional on
        // it: an assumed rely is never silently certified as an unconditional guarantee.
        let mut assumed_relies: Vec<String> = Vec::new();
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Rely) {
            let (Some(name), Some(body)) = (it.name.clone(), it.body) else { continue };
            let Some(reduced) = arith_reduce(&crate::monitor::inline_defs(&parse_predicate(body.slice(src)).0, &defs)) else {
                continue; // non-linear / two-state step rely — handled elsewhere, not this slice
            };
            let (cons, notes) = ground(&reduced, &st);
            if notes || cons.is_empty() {
                continue;
            }
            // An UNSATISFIABLE rely (contradictory in isolation) would make every VC vacuously UNSAT and
            // silently mask real breaks. Diagnose it (vacuity discipline) and do NOT add it as a hypothesis,
            // so genuine breaks still surface rather than hiding behind a broken assumption.
            if matches!(solve(&cons), Outcome::Unsat) {
                out.push(Diagnostic::warning(
                    d.span,
                    format!("rely `{name}` in `{}` is unsatisfiable (contradictory) — it can never hold, so every guarantee proved under it would be vacuous. Fix or remove it.", d.name),
                ));
                continue;
            }
            all_pre.extend(cons);
            assumed_relies.push(name);
        }
        if !assumed_relies.is_empty() {
            assumed_relies.sort();
            out.push(Diagnostic::warning(
                d.span,
                format!("arithmetic preservation in `{}` is conditional on assumed rely(s): {} (assumed — trusted, not checked; not an unconditional guarantee).", d.name, assumed_relies.join(", ")),
            ));
        }

        // Base case: does `init` establish each linear invariant? If the initial arithmetic state can
        // violate `A` (e.g. `init` sets `balance = -5` against `balance >= 0`), the induction has no base.
        if let Some(init_it) = d.items.iter().find(|it| it.kind == ItemKind::Init).and_then(|it| it.body) {
            let init_raw = parse_predicate(init_it.slice(src).trim().strip_prefix("means").unwrap_or(init_it.slice(src))).0;
            let mut iev = HashSet::new();
            crate::analyse::collect_entity_vars(&init_raw, &mut iev);
            let init = crate::analyse::rename_entity(&init_raw, &iev);
            let (init_cons, init_notes) = ground(&strip_enum_conjuncts(&init, &st), &st);
            // States `init` actually assigns. Only invariants over a state init sets can be *contradicted*
            // by init; an invariant over a value init leaves free is an input assumption, not init's to
            // establish, so gating on this avoids noise while still catching a genuine init contradiction.
            let mut init_states = HashSet::new();
            crate::analyse::collect_writes(&init, false, &state_names, &mut init_states);
            if !init_notes {
                for (iname, inv, icons) in &invs {
                    if !crate::analyse::mentions_any(inv, &init_states) {
                        continue;
                    }
                    let violated = icons.iter().any(|c| {
                        negate_con(c).into_iter().any(|neg| {
                            let mut q: Vec<Con> = init_cons.clone();
                            q.push(neg);
                            matches!(solve(&q), Outcome::Sat(_))
                        })
                    });
                    if violated {
                        out.push(Diagnostic::warning(
                            d.span,
                            format!("`init` in `{}` does not establish arithmetic invariant `{iname}`: the initial state can violate it.", d.name),
                        ));
                    }
                }
            }
        }

        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let aname = it.name.clone().unwrap_or_else(|| "<anon>".into());
            let ensures_raw = match it.ensures_expr(src) {
                Some(e) => crate::monitor::inline_defs(&e, &defs),
                None => continue,
            };
            let guard_raw = it.requires.map(|sp| crate::monitor::inline_defs(&parse_predicate(sp.slice(src)).0, &defs));
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

            for (iname, inv, _pre_cons) in &invs {
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
                        let mut q = all_pre.clone();
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
                    // Name the weakest guard that would preserve the bound (the invariant with the effect
                    // substituted in), the elicit value — falls back to a generic hint if it can't be
                    // derived cleanly. Reuses the boolean pass's tested suggestion machinery.
                    // A guard is a precondition, so `old(x)` in the effect reads as the pre-state `x`.
                    let fix = crate::analyse::guard_suggestion(inv, &crate::analyse::strip_old(&ensures), &modified);
                    out.push(Diagnostic::warning(
                        it.span,
                        crate::analyse::pretty(&format!(
                            "action `{aname}` in `{}` can break arithmetic invariant `{iname}`: from a state satisfying it (e.g. {}), the action reaches a state that violates it.{}",
                            d.name, w, fix
                        )),
                    ));
                }
            }
        }
    }
    out
}

/// Read a finite-state atom `obs(..) = tag` / bare boolean `obs(..)` / negated `not obs(..)` as its
/// `(obs, tag)` pair, where a bare flag is `= true` and its negation `= false`. None otherwise.
fn finite_atom(e: &Expr, st: &HashMap<String, String>) -> Option<(String, String)> {
    match e {
        Expr::Binary { op: BinOp::Eq, lhs, rhs } => {
            let h = app_head(lhs).filter(|h| finite_typed(h, st))?;
            match rhs.as_ref() {
                Expr::Name(t) => Some((h.to_string(), t.clone())),
                _ => None,
            }
        }
        Expr::Unary { op: UnOp::Not, e } => {
            let h = app_head(e).filter(|h| bool_typed(h, st))?;
            Some((h.to_string(), "false".into()))
        }
        _ => {
            let h = app_head(e).filter(|h| bool_typed(h, st))?;
            Some((h.to_string(), "true".into()))
        }
    }
}

/// True if `e` is a ground numeric expression — a literal or arithmetic over literals, no state/given.
fn ground_num(e: &Expr) -> bool {
    match e {
        Expr::Int(_) | Expr::Dec(_, _) => true,
        Expr::Binary { op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div, lhs, rhs } => {
            ground_num(lhs) && ground_num(rhs)
        }
        Expr::Unary { e, .. } => ground_num(e),
        _ => false,
    }
}

/// Collect `state(..) = <constant>` assignments from a conjunction (an `init` body), mapping the state
/// name to the constant it is pinned to. Used to substitute init's values into a bound to reveal the
/// residual constraint on the remaining free inputs (the missing assumption).
fn const_assignments(e: &Expr, out: &mut HashMap<String, Expr>) {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            const_assignments(lhs, out);
            const_assignments(rhs, out);
        }
        Expr::Binary { op: BinOp::Eq, lhs, rhs } if ground_num(rhs) => {
            if let Some(h) = app_head(lhs) {
                out.insert(h.to_string(), (**rhs).clone());
            }
        }
        _ => {}
    }
}

/// Substitute pinned state constants into an expression (replacing `state(..)` and bare `state`).
fn subst_consts(e: &Expr, consts: &HashMap<String, Expr>) -> Expr {
    if let Some(h) = app_head(e) {
        if let Some(c) = consts.get(h) {
            return c.clone();
        }
    }
    match e {
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op: op.clone(),
            lhs: Box::new(subst_consts(lhs, consts)),
            rhs: Box::new(subst_consts(rhs, consts)),
        },
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(subst_consts(e, consts)) },
        other => other.clone(),
    }
}

/// True if `e` still references a state/given observable (an application or field) — i.e. after
/// substituting init constants, a residual free-input constraint remains (a real assumption, not a
/// constant inequality).
fn has_free_ref(e: &Expr) -> bool {
    match e {
        Expr::App { .. } | Expr::Field { .. } => true,
        Expr::Binary { lhs, rhs, .. } => has_free_ref(lhs) || has_free_ref(rhs),
        Expr::Unary { e, .. } => has_free_ref(e),
        Expr::Name(n) => !matches!(n.as_str(), "true" | "false"),
        _ => false,
    }
}

/// The component's computed `given` definitions (`given available means limit - used`), name -> (params,
/// body), for inlining derived values into invariants/effects before arithmetic checking.
/// Definitions brought into scope from other modules via `use`. Threaded from the CLI, which resolves the
/// import graph the single-module `analyse` cannot see. Empty by default (single-file analysis is unchanged).
#[derive(Default, Clone)]
pub struct Imports {
    /// Imported `given` definitions: name -> (params, pre-parsed body). Bodies are self-contained
    /// expressions (no spans), so they carry across the module boundary without their source.
    pub givens: HashMap<String, (Vec<String>, Expr)>,
    /// Imported contracts a component may `satisfy`: name -> (promises, boolean names, state/given types).
    /// Pre-extracted so the refinement pass can resolve a contract declared in another module.
    pub contracts: HashMap<String, ContractPromises>,
}

/// A contract's checkable surface: its promises (name, predicate), its boolean-valued names, and the raw
/// type text of its states/givens. Exactly what the refinement pass reads from a local contract decl.
pub type ContractPromises = (Vec<(String, Expr)>, std::collections::HashSet<String>, HashMap<String, String>);

/// Every `given` definition in a module, across all its components — used to build the [`Imports`] a
/// consumer sees when it `use`s this module. Type-annotation givens (no body computation) are excluded.
pub fn extract_givens(source: &str) -> HashMap<String, (Vec<String>, Expr)> {
    let module = crate::parse(source).module;
    let empty = Imports::default();
    let mut out = HashMap::new();
    for d in &module.decls {
        out.extend(component_defs(d, source, &empty));
    }
    out
}

fn component_defs(d: &crate::ast::Decl, src: &str, imports: &Imports) -> HashMap<String, (Vec<String>, Expr)> {
    // Imported givens are the base; a local `given` of the same name shadows the import.
    let mut defs = imports.givens.clone();
    for it in d.items.iter().filter(|it| it.kind == ItemKind::Given && it.body.is_some()) {
        let Some(name) = it.name.clone() else { continue };
        let Some(sp) = it.body else { continue };
        let body = parse_predicate(sp.slice(src)).0;
        if it.params.is_empty() && !crate::monitor::is_computation(&body) {
            continue; // a type annotation, not a definition
        }
        defs.insert(name, (it.params.clone(), body));
    }
    defs
}

/// Derived linear bounds from a `X = min/max/abs(…)` invariant: `min` gives `X <= each arg`, `max` gives
/// `X >= each arg`, `abs(a)` gives `X >= a`, `X >= -a`, `X >= 0`. Sound consequences of the equality, so
/// their preservation catches an action that pushes `X` past the cap/floor (the disjunctive `X = some arg`
/// part is not derived).
fn minmax_bounds(inv: &Expr) -> Vec<Expr> {
    let is_mm = |e: &Expr| -> Option<(String, Vec<Expr>)> {
        if let Expr::App { head, args } = e {
            if let Expr::Name(f) = &**head {
                if (f == "min" || f == "max") && args.len() >= 2 {
                    return Some((f.clone(), args.clone()));
                }
                if f == "abs" && args.len() == 1 {
                    return Some((f.clone(), args.clone()));
                }
            }
        }
        None
    };
    let Expr::Binary { op: BinOp::Eq, lhs, rhs } = inv else { return Vec::new() };
    let (x, f, args) = match (is_mm(lhs), is_mm(rhs)) {
        (None, Some((f, a))) => ((**lhs).clone(), f, a),
        (Some((f, a)), None) => ((**rhs).clone(), f, a),
        _ => return Vec::new(),
    };
    let ge = |l: Expr, r: Expr| Expr::Binary { op: BinOp::Ge, lhs: Box::new(l), rhs: Box::new(r) };
    if f == "abs" {
        let a = args.into_iter().next().unwrap();
        let neg = Expr::Binary { op: BinOp::Sub, lhs: Box::new(Expr::Int(0)), rhs: Box::new(a.clone()) };
        return vec![ge(x.clone(), a), ge(x.clone(), neg), ge(x, Expr::Int(0))];
    }
    let rel = if f == "min" { BinOp::Le } else { BinOp::Ge };
    args.into_iter()
        .map(|a| Expr::Binary { op: rel.clone(), lhs: Box::new(x.clone()), rhs: Box::new(a) })
        .collect()
}

/// Count `if/then/else` subterms in an expression.
fn count_conds(e: &Expr) -> usize {
    match e {
        Expr::Cond { cond, then_, els } => 1 + count_conds(cond) + count_conds(then_) + count_conds(els),
        Expr::Binary { lhs, rhs, .. } => count_conds(lhs) + count_conds(rhs),
        Expr::Unary { e, .. } => count_conds(e),
        Expr::App { head, args } => count_conds(head) + args.iter().map(count_conds).sum::<usize>(),
        _ => 0,
    }
}

/// Find the condition of the first `if/then/else` subterm.
fn first_cond(e: &Expr) -> Option<Expr> {
    match e {
        Expr::Cond { cond, .. } => Some((**cond).clone()),
        Expr::Binary { lhs, rhs, .. } => first_cond(lhs).or_else(|| first_cond(rhs)),
        Expr::Unary { e, .. } => first_cond(e),
        Expr::App { head, args } => first_cond(head).or_else(|| args.iter().find_map(first_cond)),
        _ => None,
    }
}

/// Replace every `if/then/else` with its then- or else-branch.
fn take_branch(e: &Expr, use_then: bool) -> Expr {
    match e {
        Expr::Cond { then_, els, .. } => take_branch(if use_then { then_ } else { els }, use_then),
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op: op.clone(),
            lhs: Box::new(take_branch(lhs, use_then)),
            rhs: Box::new(take_branch(rhs, use_then)),
        },
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(take_branch(e, use_then)) },
        Expr::App { head, args } => Expr::App {
            head: Box::new(take_branch(head, use_then)),
            args: args.iter().map(|a| take_branch(a, use_then)).collect(),
        },
        other => other.clone(),
    }
}

/// Expand an invariant with a single `if <finite-cond> then A else B` into two polarity-guarded bounds:
/// `cond implies inv[A]` and `not cond implies inv[B]`. `if/then/else` over a boolean/enum condition is
/// mixed reasoning: the SMT rung handles it by case-split. None if the shape is not a single finite `if`.
fn expand_conditional(
    name: &str,
    qf: &Expr,
    st: &HashMap<String, String>,
) -> Option<Vec<(String, Vec<(String, String, bool)>, Vec<Expr>, Expr)>> {
    if count_conds(qf) != 1 {
        return None;
    }
    let (obs, tag, pos) = guard_atom(&first_cond(qf)?, st)?;
    let mut out = Vec::new();
    for (suffix, use_then, polarity) in [("then", true, pos), ("else", false, !pos)] {
        let bound = take_branch(qf, use_then);
        let (bcons, bn) = ground(&bound, st);
        if bn || bcons.is_empty() {
            return None;
        }
        out.push((format!("{name}[{suffix}]"), vec![(obs.clone(), tag.clone(), polarity)], Vec::new(), bound));
    }
    Some(out)
}

/// Read a finite GUARD atom as `(obs, tag, positive)`: `obs = tag` and a bare flag are positive
/// (`obs == tag`); `obs <> tag` and `not flag` are negated (`obs != tag`). Unlike [`finite_atom`], which
/// reads a concrete assignment, a guard may be a not-equal condition (a disjunction over the other tags).
fn guard_atom(e: &Expr, st: &HashMap<String, String>) -> Option<(String, String, bool)> {
    match e {
        Expr::Binary { op: op @ (BinOp::Eq | BinOp::Ne), lhs, rhs } => {
            let h = app_head(lhs).filter(|h| finite_typed(h, st))?;
            match rhs.as_ref() {
                Expr::Name(t) => Some((h.to_string(), t.clone(), *op == BinOp::Eq)),
                _ => None,
            }
        }
        Expr::Unary { op: UnOp::Not, e } => {
            let h = app_head(e).filter(|h| bool_typed(h, st))?;
            Some((h.to_string(), "true".into(), false))
        }
        _ => {
            let h = app_head(e).filter(|h| bool_typed(h, st))?;
            Some((h.to_string(), "true".into(), true))
        }
    }
}

/// True if a finite observable holding value `v` activates a guard condition `(tag, positive)`.
fn cond_active(v: &str, tag: &str, positive: bool) -> bool {
    positive == (v == tag)
}

/// Split a conjunctive guard into finite-state atoms (`conds`, with polarity) and arithmetic conjuncts.
fn split_ante(e: &Expr, st: &HashMap<String, String>, conds: &mut Vec<(String, String, bool)>, arith: &mut Vec<Expr>) {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            split_ante(lhs, st, conds, arith);
            split_ante(rhs, st, conds, arith);
        }
        _ => match guard_atom(e, st) {
            Some(a) => conds.push(a),
            None => arith.push(e.clone()),
        },
    }
}

/// Extract `(conds, arith_guard, A)` from a reduced body `guard implies A`, where the guard is a
/// conjunction of finite-state atoms (enum (dis)equalities and boolean flags) and optional arithmetic
/// conditions, and `A` is the arithmetic consequent. At least one finite condition is required.
fn finite_guarded_inv(qf: &Expr, st: &HashMap<String, String>) -> Option<(Vec<(String, String, bool)>, Vec<Expr>, Expr)> {
    let Expr::Binary { op: BinOp::Implies, lhs, rhs } = qf else { return None };
    let mut conds = Vec::new();
    let mut arith = Vec::new();
    split_ante(lhs, st, &mut conds, &mut arith);
    if conds.is_empty() {
        return None;
    }
    Some((conds, arith, (**rhs).clone()))
}

/// The tag an `ensures` conjunction assigns to `obs` (`obs = tag`, or a bare/negated boolean flag), if any.
fn assigned_enum_tag(e: &Expr, obs: &str, st: &HashMap<String, String>) -> Option<String> {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            assigned_enum_tag(lhs, obs, st).or_else(|| assigned_enum_tag(rhs, obs, st))
        }
        _ => finite_atom(e, st).filter(|(o, _)| o == obs).map(|(_, t)| t),
    }
}

/// The tag a guard conjunction requires for `obs` (`obs = tag`, or a bare/negated boolean flag), if any.
fn required_enum_tag(e: &Expr, obs: &str, st: &HashMap<String, String>) -> Option<String> {
    assigned_enum_tag(e, obs, st)
}

/// Replace each enum-equality conjunct with `true`, leaving only the arithmetic part of a guard to ground.
fn strip_enum_conjuncts(e: &Expr, st: &HashMap<String, String>) -> Expr {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => Expr::Binary {
            op: BinOp::And,
            lhs: Box::new(strip_enum_conjuncts(lhs, st)),
            rhs: Box::new(strip_enum_conjuncts(rhs, st)),
        },
        _ if is_enum_guard(e, st) => Expr::Name("true".into()),
        other => other.clone(),
    }
}

/// Enum-guarded arithmetic preservation — the first slice of mixed boolean+arithmetic reasoning. An
/// invariant `enum_obs(e) = tag implies A(e)` (A linear) is owned by neither the pure-enum tier (A is
/// arithmetic) nor the pure-LRA tier (the guard is an enum equality it drops). This checks it by
/// case-splitting on the enum guard: for each action, decide whether `enum_obs = tag` still holds after it
/// (from the action's enum effect and requirement) and whether A held before, then run the LRA VC
/// `A(pre)? ∧ unconditional-invariants ∧ arith-guard ∧ effect ∧ ¬A(post)`. SOUND: any unmodellable term
/// (nonlinear effect/guard, conditional enum set) abandons the pair rather than risk a false alarm; the
/// enum requirement is honoured so an action that cannot fire under `tag` is not spuriously flagged.
pub fn enum_guarded_preservation(module: &Module, src: &str, imports: &Imports) -> Vec<Diagnostic> {
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
        // A numeric variant payload field (`out` of `{ success { out : Number } | … }`) is a numeric state
        // for the arithmetic tier — its presence is already guard-checked by variant_access.
        let payload = crate::analyse::variant_field_types(d, src);
        for (f, t) in &payload {
            st.entry(f.clone()).or_insert_with(|| t.clone());
        }
        let mut state_names: HashSet<String> =
            d.items.iter().filter(|it| it.kind == ItemKind::State).filter_map(|it| it.name.clone()).collect();
        state_names.extend(payload.into_keys());
        let defs = component_defs(d, src, imports);

        // Enum-guarded linear invariants, and the unconditional linear invariants (pre-hypotheses that
        // rule out impossible pre-states, so a break is only reported from a genuinely reachable one).
        let mut guarded: Vec<(String, Vec<(String, String, bool)>, Vec<Expr>, Expr)> = Vec::new();
        let mut uncond_pre: Vec<Con> = Vec::new();
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Invariant) {
            let (name, body) = match (&it.name, it.body) {
                (Some(n), Some(b)) => (n.clone(), b),
                _ => continue,
            };
            let qf = match arith_reduce(&crate::monitor::inline_defs(&parse_predicate(body.slice(src)).0, &defs)) {
                Some(e) => e,
                None => continue,
            };
            // A conditional invariant `X = if late then base + 5 else base` expands to two guarded bounds.
            if let Some(expanded) = expand_conditional(&name, &qf, &st) {
                guarded.extend(expanded);
            } else if let Some((conds, arith_guard, a)) = finite_guarded_inv(&qf, &st) {
                let (acons, anotes) = ground(&a, &st);
                if anotes || acons.is_empty() {
                    continue; // consequent not purely linear
                }
                // The arithmetic part of the guard must ground cleanly too (else we cannot model when the
                // guard is active), otherwise skip the invariant rather than reason unsoundly.
                if arith_guard.iter().any(|g| ground(g, &st).1) {
                    continue;
                }
                guarded.push((name, conds, arith_guard, a));
            } else {
                let (c, n) = ground(&qf, &st);
                if !n {
                    uncond_pre.extend(c);
                }
            }
        }
        if guarded.is_empty() {
            continue;
        }

        // Base case: does `init` establish each guarded bound? For an invariant `Gfin ∧ Garith → A` whose
        // finite guard `init` definitely activates (init pins each finite condition to its tag), check
        // whether the initial arithmetic state can still violate `A` (with the arithmetic guard). If so the
        // bound does not hold at init — the induction has no base. Only report when the finite guard is
        // definitely active at init, so an unpinned enum never yields a false alarm.
        // `established` collects bounds init definitely establishes (checked and not violated), so that a
        // bound both established and preserved earns the stronger INDUCTIVE verdict below.
        let mut established: HashSet<String> = HashSet::new();
        if let Some(init_it) = d.items.iter().find(|it| it.kind == ItemKind::Init).and_then(|it| it.body) {
            let init_raw = crate::monitor::inline_defs(&parse_predicate(init_it.slice(src).trim().strip_prefix("means").unwrap_or(init_it.slice(src))).0, &defs);
            let mut iev = HashSet::new();
            crate::analyse::collect_entity_vars(&init_raw, &mut iev);
            let init = crate::analyse::rename_entity(&init_raw, &iev);
            let (init_arith, init_notes) = ground(&strip_enum_conjuncts(&init, &st), &st);
            let mut init_states = HashSet::new();
            crate::analyse::collect_writes(&init, false, &state_names, &mut init_states);
            let mut init_consts = HashMap::new();
            const_assignments(&init, &mut init_consts);
            if !init_notes {
                for (iname, conds, arith_guard, a) in &guarded {
                    // Per-condition activation at init: Some(true/false) if init pins the observable,
                    // None if init leaves it free.
                    let statuses: Vec<Option<bool>> = conds
                        .iter()
                        .map(|(o, t, pos)| assigned_enum_tag(&init, o, &st).map(|v| cond_active(&v, t, *pos)))
                        .collect();
                    // A guard init definitely turns OFF (some condition pinned false) makes the invariant
                    // hold vacuously at init — established, so it can earn INDUCTIVE if also preserved.
                    if statuses.iter().any(|s| *s == Some(false)) {
                        established.insert(iname.clone());
                        continue;
                    }
                    let active = statuses.iter().all(|s| *s == Some(true));
                    // Only a bound over a numeric state init assigns can be contradicted by init.
                    if !active || !crate::analyse::mentions_any(a, &init_states) {
                        continue;
                    }
                    let (acons, an) = ground(a, &st);
                    if an || acons.is_empty() {
                        continue;
                    }
                    let mut garith = Vec::new();
                    if arith_guard.iter().any(|g| {
                        let (c, n) = ground(g, &st);
                        garith.extend(c);
                        n
                    }) {
                        continue;
                    }
                    let violated = acons.iter().any(|c| {
                        negate_con(c).into_iter().any(|neg| {
                            // Include the unconditional invariants and `where`-refinements (input
                            // assumptions), which hold at init too, so a bound the spec's own assumptions
                            // guarantee is not falsely reported unestablished.
                            let mut q: Vec<Con> =
                                init_arith.iter().chain(garith.iter()).chain(uncond_pre.iter()).cloned().collect();
                            q.push(neg);
                            matches!(solve(&q), Outcome::Sat(_))
                        })
                    });
                    if violated {
                        let guard_desc = conds.iter().map(|(o, t, pos)| format!("{o} {} {t}", if *pos { "=" } else { "<>" })).collect::<Vec<_>>().join(" and ");
                        // Substitute init's pinned constants into the bound: if a residual constraint over
                        // free inputs remains, that is the assumption the spec RELIES ON but has not stated
                        // — surface it so the user can add it (the elicit value).
                        let residual = subst_consts(a, &init_consts);
                        let hint = if has_free_ref(&residual) {
                            format!(" It holds at init only if `{}` — state this assumption (e.g. a `where` refinement on the input).", crate::analyse::pretty(&crate::analyse::canon(&residual)))
                        } else {
                            String::new()
                        };
                        out.push(Diagnostic::warning(
                            d.span,
                            format!("`init` in `{}` does not establish state-guarded invariant `{iname}`: the initial state has `{guard_desc}` but can violate the arithmetic bound.{hint}", d.name),
                        ));
                    } else {
                        established.insert(iname.clone());
                    }
                }
            }
        }

        // Track which invariants an action actually engaged (ran the VC for) and which were broken, so a
        // state-guarded invariant that survives every action gets a positive PRESERVED verdict.
        let mut engaged: HashSet<String> = HashSet::new();
        let mut broken: HashSet<String> = HashSet::new();
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let aname = it.name.clone().unwrap_or_else(|| "<anon>".into());
            let ensures_raw = match it.ensures_expr(src) {
                Some(e) => crate::monitor::inline_defs(&e, &defs),
                None => continue,
            };
            let guard_raw = it.requires.map(|sp| crate::monitor::inline_defs(&parse_predicate(sp.slice(src)).0, &defs));
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
            let mut st2 = st.clone();
            for m in &modified_numeric {
                if let Some(t) = st.get(m).cloned() {
                    st2.insert(format!("{m}'"), t);
                }
            }
            // Ground only the arithmetic part of the effect: the enum assignment (`outcome = success`) is
            // accounted for by the case analysis below, not by the LRA solver, so strip it first.
            let effect_expr = crate::analyse::prime(&strip_enum_conjuncts(&ensures, &st), &modified_numeric, false);
            let (effect_cons, effect_notes) = ground(&effect_expr, &st2);
            if effect_notes {
                continue; // an unmodellable effect could hide a link -> skip, do not false-alarm
            }
            let guard_cons = match &guard {
                Some(g) => {
                    let (c, n) = ground(&strip_enum_conjuncts(g, &st), &st2);
                    if n {
                        continue;
                    }
                    c
                }
                None => Vec::new(),
            };

            // Other state-guarded bounds that are known to hold in the pre-state — those whose finite guard
            // the action `requires` — are valid pre-hypotheses. Without them a bound that another invariant
            // pins (e.g. `compacting implies wm <= off`, where `healthy implies wm <= off` already holds and
            // the action requires `healthy`) would false-alarm. Sound: the action can only fire when its
            // requires hold, so those guards hold before it.
            let mut guarded_pre: Vec<Con> = Vec::new();
            for (_, jconds, _, jbound) in &guarded {
                let all_required = jconds.iter().all(|(o, t, pos)| {
                    guard.as_ref().and_then(|g| required_enum_tag(g, o, &st)).map(|r| cond_active(&r, t, *pos)).unwrap_or(false)
                });
                if all_required {
                    let (c, n) = ground(jbound, &st);
                    if !n {
                        guarded_pre.extend(c);
                    }
                }
            }

            'inv: for (iname, conds, arith_guard, a) in &guarded {
                // The action must touch the invariant to be able to affect it — either a guard observable
                // (which could turn the guard on) or a numeric state in the bound. Otherwise it is trivially
                // preserved and should not be reported as engaged (a vacuous PRESERVED over-claims).
                let touches = conds.iter().any(|(o, _, _)| modified.contains(o)) || crate::analyse::mentions_any(a, &modified);
                if !touches {
                    continue;
                }
                // Every guard condition must still hold after the action for the bound to be required
                // (active_post), and all must have held before for A to be assumed to have held (a_pre).
                let mut a_pre = true;
                for (obs, tag, pos) in conds {
                    let required = guard.as_ref().and_then(|g| required_enum_tag(g, obs, &st));
                    let (active_i, pre_i) = if modified.contains(obs) {
                        match assigned_enum_tag(&ensures, obs, &st) {
                            // set to a value that activates this condition; the bound held before only if
                            // the pre-value (from `requires`) also activated it.
                            Some(v) if cond_active(&v, tag, *pos) => {
                                (true, required.as_deref().map(|r| cond_active(r, tag, *pos)).unwrap_or(false))
                            }
                            Some(_) => (false, false), // set to a value that deactivates it -> inactive after
                            None => continue 'inv,     // conditional/opaque enum set -> cannot decide soundly
                        }
                    } else {
                        // unchanged: active after iff it held before; the action must be able to fire then,
                        // so a required value that deactivates this condition rules the case out.
                        if required.as_deref().map(|r| !cond_active(r, tag, *pos)).unwrap_or(false) {
                            continue 'inv;
                        }
                        (true, true)
                    };
                    if !active_i {
                        continue 'inv; // a guard condition is definitely false after -> invariant vacuous
                    }
                    a_pre = a_pre && pre_i;
                }
                let (a_pre_cons, apn) = ground(a, &st);
                let a_post = crate::analyse::prime(a, &modified_numeric, false);
                let (a_post_cons, apostn) = ground(&a_post, &st2);
                if apn || apostn || a_post_cons.is_empty() {
                    continue;
                }
                // The arithmetic part of the guard, grounded pre (unprimed) and post (primed). It must hold
                // AFTER for the bound to apply (garith_post in the VC); its PRE form conditions the
                // pre-invariant `Garith_pre -> A_pre`.
                let mut garith_pre: Vec<Con> = Vec::new();
                let mut garith_post: Vec<Con> = Vec::new();
                let mut garith_ok = true;
                for g in arith_guard {
                    let (cpre, npre) = ground(g, &st);
                    let (cpost, npost) = ground(&crate::analyse::prime(g, &modified_numeric, false), &st2);
                    if npre || npost {
                        garith_ok = false;
                        break;
                    }
                    garith_pre.extend(cpre);
                    garith_post.extend(cpost);
                }
                if !garith_ok {
                    continue; // an unmodellable arithmetic guard -> skip, do not risk a false alarm
                }
                engaged.insert(iname.clone());
                // Base constraints common to every violation query: effect, the action's arithmetic guard,
                // other bounds known pre, and the arithmetic guard holding AFTER the action.
                let base: Vec<Con> = guarded_pre
                    .iter()
                    .chain(uncond_pre.iter())
                    .chain(guard_cons.iter())
                    .chain(effect_cons.iter())
                    .chain(garith_post.iter())
                    .cloned()
                    .collect();
                let mut witness: Option<String> = None;
                'search: for pc in &a_post_cons {
                    for neg in negate_con(pc) {
                        // Try to satisfy the violation `¬A_post` together with the pre-invariant. When the
                        // finite guard held before (a_pre), the pre-invariant is `Garith_pre -> A_pre`, a
                        // disjunction checked as two branches; otherwise A is not assumed to have held.
                        let mut queries: Vec<Vec<Con>> = Vec::new();
                        if a_pre {
                            // Branch (ii): the arithmetic guard held before and so did the bound.
                            let mut q = base.clone();
                            q.extend(garith_pre.clone());
                            q.extend(a_pre_cons.clone());
                            q.push(neg.clone());
                            queries.push(q);
                            // Branch (i): the arithmetic guard did NOT hold before (so A was unconstrained).
                            for gc in &garith_pre {
                                for gneg in negate_con(gc) {
                                    let mut q = base.clone();
                                    q.push(gneg);
                                    q.push(neg.clone());
                                    queries.push(q);
                                }
                            }
                        } else {
                            let mut q = base.clone();
                            q.push(neg.clone());
                            queries.push(q);
                        }
                        for q in queries {
                            if let Outcome::Sat(m) = solve(&q) {
                                witness = Some(schedule(&m, &st2));
                                break 'search;
                            }
                        }
                    }
                }
                if let Some(w) = witness {
                    broken.insert(iname.clone());
                    let guard_desc = conds.iter().map(|(o, t, pos)| format!("{o} {} {t}", if *pos { "=" } else { "<>" })).collect::<Vec<_>>().join(" and ");
                    // When the bound was not guaranteed before the action (a transition INTO the guard),
                    // the precise fix is to state the bound for the source state too. Name it.
                    let mut src_conds = Vec::new();
                    let mut src_arith = Vec::new();
                    if let Some(g) = &guard {
                        split_ante(g, &st, &mut src_conds, &mut src_arith);
                    }
                    let src_desc = src_conds
                        .iter()
                        .map(|(o, t, pos)| format!("{o} {} {t}", if *pos { "=" } else { "<>" }))
                        .collect::<Vec<_>>()
                        .join(" and ");
                    let fix = if !a_pre && !src_desc.is_empty() {
                        format!(" To fix, state the bound for the source state — add `{} implies {}` — or guard the action.", src_desc, crate::analyse::canon(a))
                    } else {
                        " Guard the action or maintain the bound.".to_string()
                    };
                    out.push(Diagnostic::warning(
                        it.span,
                        crate::analyse::pretty(&format!(
                            "action `{aname}` in `{}` can break state-guarded invariant `{iname}`: with `{guard_desc}` holding afterwards, the arithmetic bound is violated (e.g. {w}).{fix}",
                            d.name
                        )),
                    ));
                }
            }
        }
        // An invariant every action engaged but none broke is preserved under its guard — a positive result.
        for (iname, _, _, _) in &guarded {
            if engaged.contains(iname) && !broken.contains(iname) {
                if established.contains(iname) {
                    out.push(Diagnostic::warning(
                        d.span,
                        format!("state-guarded invariant `{iname}` in `{}` is INDUCTIVE: established by `init` and preserved by every action wherever its guard holds.", d.name),
                    ));
                } else {
                    out.push(Diagnostic::warning(
                        d.span,
                        format!("state-guarded invariant `{iname}` in `{}` is PRESERVED: every action maintains the arithmetic bound wherever its guard holds.", d.name),
                    ));
                }
            }
        }
    }
    out
}

/// Extract `(obs, tag, is_old)` from an enum-equality atom `obs(e) = tag` or `old(obs(e)) = tag`.
fn enum_eq_parts(e: &Expr) -> Option<(String, String, bool)> {
    let Expr::Binary { op: BinOp::Eq, lhs, rhs } = e else { return None };
    let Expr::Name(tag) = &**rhs else { return None };
    let (inner, is_old) = match &**lhs {
        Expr::Unary { op: UnOp::Old, e } => (e.as_ref(), true),
        other => (other, false),
    };
    Some((app_head(inner)?.to_string(), tag.clone(), is_old))
}

/// Match a desugared arithmetic-guarded-edge legality body `(old(obs(e)) = A and obs(e) = B) implies
/// old(<cmp>)` and return `(obs, A, B, cmp)`. The `old`-wrapped enum equality is the source state; the bare
/// one is the target; the consequent is the edge guard as it held before the step.
fn arith_guarded_edge(qf: &Expr) -> Option<(String, String, String, Expr)> {
    let Expr::Binary { op: BinOp::Implies, lhs, rhs } = qf else { return None };
    let Expr::Binary { op: BinOp::And, lhs: a1, rhs: a2 } = &**lhs else { return None };
    let p1 = enum_eq_parts(a1)?;
    let p2 = enum_eq_parts(a2)?;
    if p1.0 != p2.0 {
        return None;
    }
    let (obs, a, b) = match (p1.2, p2.2) {
        (true, false) => (p1.0, p1.1, p2.1),
        (false, true) => (p2.0, p2.1, p1.1),
        _ => return None, // both old or both bare -> not the edge shape
    };
    let Expr::Unary { op: UnOp::Old, e: cmp } = &**rhs else { return None };
    Some((obs, a, b, (**cmp).clone()))
}

/// Enforce arithmetic edge guards in transition legality (task #68). An arith-guarded edge `A -> B when
/// <cmp>` desugars to the legality invariant `(old(obs)=A and obs=B) implies old(<cmp>)`, which the boolean
/// preservation fragment drops (an arithmetic consequent under `old`). For each such invariant and each
/// action that drives `obs` from A to B, run the pre-state LRA VC `guard_arith ∧ unconditional-bounds ∧
/// ¬cmp`: if satisfiable, the action performs the guarded transition without the guard holding beforehand
/// and so breaks legality. SOUND: an unmodellable guard/consequent, or an action that cannot fire from A,
/// abandons the pair rather than risk a false alarm.
pub fn transition_arith_legality(module: &Module, src: &str, imports: &Imports) -> Vec<Diagnostic> {
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
        for (f, t) in crate::analyse::variant_field_types(d, src) {
            st.entry(f).or_insert(t);
        }
        let defs = component_defs(d, src, imports);

        // Collect the arith-guarded-edge legality invariants, each renamed to the canonical entity so its
        // bound aligns with the actions', with its consequent grounded (a non-arithmetic consequent — a
        // boolean guard — grounds to nothing and is left to the boolean pass).
        struct Edge {
            name: String,
            obs: String,
            a: String,
            b: String,
            cmp: Expr,
            cons: Vec<Con>,
            span: crate::span::Span,
        }
        let mut edges: Vec<Edge> = Vec::new();
        // The unconditional linear invariants hold in the pre-state — pre-hypotheses that keep a bound the
        // spec's own standing invariants guarantee from being falsely reported.
        let mut uncond_pre: Vec<Con> = Vec::new();
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Invariant) {
            let (name, body) = match (&it.name, it.body) {
                (Some(n), Some(b)) => (n.clone(), b),
                _ => continue,
            };
            let parsed = crate::monitor::inline_defs(&parse_predicate(body.slice(src)).0, &defs);
            // Strip a single universal so an edge-legality body (`every e :: … implies …`) matches;
            // standing bounds are handled by `arith_reduce`, which accepts explicit or implicit quant-ation.
            let qf_for_edge = match &parsed {
                Expr::Quant { q: Quant::Every, body, .. } => (**body).clone(),
                other => other.clone(),
            };
            if let Some((obs, a, b, cmp)) = arith_guarded_edge(&qf_for_edge) {
                let mut ev = HashSet::new();
                crate::analyse::collect_entity_vars(&cmp, &mut ev);
                let cmp = crate::analyse::rename_entity(&cmp, &ev);
                let (cons, notes) = ground(&cmp, &st);
                if notes || cons.is_empty() {
                    continue; // boolean or unmodellable guard -> not this pass's job
                }
                edges.push(Edge { name, obs, a, b, cmp, cons, span: it.span });
            } else if let Some(reduced) = arith_reduce(&parsed) {
                let (c, n) = ground(&reduced, &st); // arith_reduce already renames to the canonical entity
                if !n {
                    uncond_pre.extend(c);
                }
            }
        }
        if edges.is_empty() {
            continue;
        }

        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let aname = it.name.clone().unwrap_or_else(|| "<anon>".into());
            let ensures_raw = match it.ensures_expr(src) {
                Some(e) => crate::monitor::inline_defs(&e, &defs),
                None => continue,
            };
            let guard_raw =
                it.requires.map(|sp| crate::monitor::inline_defs(&parse_predicate(sp.slice(src)).0, &defs));
            let mut ev = HashSet::new();
            crate::analyse::collect_entity_vars(&ensures_raw, &mut ev);
            if let Some(g) = &guard_raw {
                crate::analyse::collect_entity_vars(g, &mut ev);
            }
            if ev.len() > 1 {
                continue; // multi-entity action -> outside this pass
            }
            let ensures = crate::analyse::rename_entity(&ensures_raw, &ev);
            let guard = guard_raw.map(|g| crate::analyse::rename_entity(&g, &ev));

            // The arithmetic part of the guard (its enum conjuncts are handled by the case analysis below).
            let guard_arith = match &guard {
                Some(g) => {
                    let (c, n) = ground(&strip_enum_conjuncts(g, &st), &st);
                    if n {
                        continue; // unmodellable guard -> cannot reason, skip the action
                    }
                    c
                }
                None => Vec::new(),
            };

            for e in &edges {
                // The action must drive obs A -> B: it must set obs to B, and be able to fire from A (a
                // `requires` that pins obs to some other tag rules the transition out).
                if assigned_enum_tag(&ensures, &e.obs, &st).as_deref() != Some(e.b.as_str()) {
                    continue;
                }
                if let Some(g) = &guard {
                    if let Some(req) = required_enum_tag(g, &e.obs, &st) {
                        if req != e.a {
                            continue;
                        }
                    }
                }
                // VC: can the guarded transition happen with the edge guard FALSE beforehand? Query
                // `guard_arith ∧ unconditional-bounds ∧ ¬cmp` (all pre-state); SAT ⇒ legality breaks.
                let mut witness = false;
                'search: for c in &e.cons {
                    for neg in negate_con(c) {
                        let mut q: Vec<Con> =
                            guard_arith.iter().chain(uncond_pre.iter()).cloned().collect();
                        q.push(neg);
                        if matches!(solve(&q), Outcome::Sat(_)) {
                            witness = true;
                            break 'search;
                        }
                    }
                }
                if witness {
                    out.push(Diagnostic::warning(
                        e.span,
                        crate::analyse::pretty(&format!(
                            "action `{aname}` in `{}` can break invariant `{}`: it drives {} from {} to {} without the edge guard `{}` holding beforehand. Require `{}` on the action.",
                            d.name,
                            e.name,
                            e.obs,
                            e.a,
                            e.b,
                            crate::analyse::canon(&e.cmp),
                            crate::analyse::canon(&e.cmp),
                        )),
                    ));
                }
            }
        }
    }
    out
}

/// Collect the `sum` aggregate subterms of an expression.
/// Does `e` contain an `if … then … else …`? A conditional sum body has a delta that depends on the
/// condition, which the linear `ground` collapses unsoundly — so the aggregate pass skips such a body.
fn contains_cond(e: &Expr) -> bool {
    match e {
        Expr::Cond { .. } => true,
        Expr::Binary { lhs, rhs, .. } => contains_cond(lhs) || contains_cond(rhs),
        Expr::Unary { e, .. } => contains_cond(e),
        Expr::App { head, args } => contains_cond(head) || args.iter().any(contains_cond),
        Expr::Field { base, .. } => contains_cond(base),
        Expr::Sum { body, .. } | Expr::Quant { body, .. } => contains_cond(body),
        _ => false,
    }
}

fn collect_sums(e: &Expr, out: &mut Vec<Expr>) {
    match e {
        Expr::Sum { .. } => out.push(e.clone()),
        Expr::Binary { lhs, rhs, .. } => {
            collect_sums(lhs, out);
            collect_sums(rhs, out);
        }
        Expr::Unary { e, .. } => collect_sums(e, out),
        Expr::App { head, args } => {
            collect_sums(head, out);
            args.iter().for_each(|a| collect_sums(a, out));
        }
        _ => {}
    }
}

/// Replace the `sum` subterms of `e` (left-to-right, matching [`collect_sums`] order) with `repls`.
fn replace_sums(e: &Expr, repls: &[Expr]) -> Expr {
    fn go(e: &Expr, repls: &[Expr], i: &mut usize) -> Expr {
        match e {
            Expr::Sum { .. } => {
                let r = repls.get(*i).cloned().unwrap_or_else(|| e.clone());
                *i += 1;
                r
            }
            Expr::Binary { op, lhs, rhs } => Expr::Binary { op: op.clone(), lhs: Box::new(go(lhs, repls, i)), rhs: Box::new(go(rhs, repls, i)) },
            Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(go(e, repls, i)) },
            other => other.clone(),
        }
    }
    let mut i = 0;
    go(e, repls, &mut i)
}

/// Aggregate/conservation invariant preservation. An invariant `total = sum p :: body(p)` is preserved by
/// an action iff the change it makes to `total` equals the change to the sum — the modified entity's
/// contribution delta `body'(e) - body(e)`. The sum is treated as an opaque variable S with the update
/// `S' = S + delta`, so a debit that shrinks a balance without adjusting `total` (a broken conservation)
/// is caught. Single-entity actions only (a 2-entity transfer, whose deltas cancel, is out of this slice
/// and skipped, not false-alarmed). SOUND: any nonlinear term or non-`linear = single-sum` shape is skipped.
pub fn aggregate_preservation(module: &Module, src: &str, imports: &Imports) -> Vec<Diagnostic> {
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
        // The opaque sum variables are numeric — one pair `__S{i}` / `__S{i}'` per sum.
        let mut st_s = st.clone();
        for i in 0..8 {
            st_s.insert(format!("__S{i}"), "Number".into());
            st_s.insert(format!("__S{i}'"), "Number".into());
        }
        let sname = |i: usize| Expr::Name(format!("__S{i}"));
        let spname = |i: usize| Expr::Name(format!("__S{i}'"));
        let defs = component_defs(d, src, imports);

        // Conservation invariants: `<linear> = <linear over one or more single-var sums>` (e.g.
        // `net = (sum p :: asset(p)) - (sum p :: liab(p))`).
        struct Cons {
            name: String,
            body_qf: Expr,            // the equality with each sum in place
            sums: Vec<(Expr, String)>, // per sum: (body, var)
        }
        let mut cons_invs: Vec<Cons> = Vec::new();
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Invariant) {
            let (name, sp) = match (&it.name, it.body) {
                (Some(n), Some(b)) => (n.clone(), b),
                _ => continue,
            };
            let raw = crate::monitor::inline_defs(&parse_predicate(sp.slice(src)).0, &defs);
            // Reduce a leading single-entity `every`, keeping the equality body.
            let body_qf = match &raw {
                Expr::Quant { q: Quant::Every, vars, body, .. } if vars.len() == 1 => (**body).clone(),
                _ => raw.clone(),
            };
            let mut sums = Vec::new();
            collect_sums(&body_qf, &mut sums);
            if sums.is_empty() || sums.len() > 8 {
                continue;
            }
            // Each sum must be single-variable.
            let mut sum_bodies = Vec::new();
            let mut ok = true;
            for s in &sums {
                let Expr::Sum { vars, body, .. } = s else { ok = false; break };
                if vars.len() != 1 {
                    ok = false;
                    break;
                }
                sum_bodies.push(((**body).clone(), vars[0].clone()));
            }
            if !ok {
                continue;
            }
            // Must be a linear equality once each sum is a variable.
            let repls: Vec<Expr> = (0..sums.len()).map(sname).collect();
            let inv_s = replace_sums(&body_qf, &repls);
            if !matches!(inv_s, Expr::Binary { op: BinOp::Eq, .. }) || ground(&inv_s, &st_s).1 {
                continue;
            }
            cons_invs.push(Cons { name, body_qf, sums: sum_bodies });
        }
        if cons_invs.is_empty() {
            continue;
        }
        let has_action = d.items.iter().any(|it| it.kind == ItemKind::Action && !it.ensures.is_empty());
        let mut broken_cons: HashSet<String> = HashSet::new();
        // Invariants an action touched but could not be soundly checked (a conditional summed body). They
        // must NOT earn a PRESERVED verdict — the one action that could break them went unchecked.
        let mut skipped_cons: HashSet<String> = HashSet::new();

        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let aname = it.name.clone().unwrap_or_else(|| "<anon>".into());
            let ensures_raw = match it.ensures_expr(src) {
                Some(e) => crate::monitor::inline_defs(&e, &defs),
                None => continue,
            };
            let guard_raw = it.requires.map(|sp| crate::monitor::inline_defs(&parse_predicate(sp.slice(src)).0, &defs));
            let mut ev = HashSet::new();
            crate::analyse::collect_entity_vars(&ensures_raw, &mut ev);
            if let Some(g) = &guard_raw {
                crate::analyse::collect_entity_vars(g, &mut ev);
            }
            if ev.len() > 2 {
                continue; // single actions, 2-entity transfers, and total-only actions; 3+ out of scope
            }
            // Map the action's (up to two) entities to distinct markers _e, _f, so a transfer's two
            // contributions to the sum are kept separate (their deltas cancel when balanced).
            let mut evs: Vec<String> = ev.into_iter().collect();
            evs.sort();
            let markers: Vec<&str> = ["_e", "_f"][..evs.len()].to_vec();
            let ent_map: HashMap<String, String> =
                evs.iter().zip(markers.iter()).map(|(v, m)| (v.clone(), m.to_string())).collect();
            let ensures = crate::analyse::rename_vars(&ensures_raw, &ent_map);
            let guard = guard_raw.map(|g| crate::analyse::rename_vars(&g, &ent_map));
            let mut modified = HashSet::new();
            crate::analyse::collect_writes(&ensures, false, &state_names, &mut modified);
            let modified_num: HashSet<String> =
                modified.iter().filter(|m| st.get(*m).map(|t| numeric(t)).unwrap_or(false)).cloned().collect();
            if modified_num.is_empty() {
                continue;
            }
            let mut st2 = st_s.clone();
            for m in &modified_num {
                if let Some(t) = st.get(m).cloned() {
                    st2.insert(format!("{m}'"), t);
                }
            }
            let effect_expr = crate::analyse::prime(&ensures, &modified_num, false);
            let (effect_cons, en) = ground(&effect_expr, &st2);
            if en {
                continue;
            }
            let guard_cons = match &guard {
                Some(g) => {
                    let (c, n) = ground(g, &st2);
                    if n {
                        continue;
                    }
                    c
                }
                None => Vec::new(),
            };

            for c in &cons_invs {
                // Each sum's change = the sum over the action's entities of that entity's body delta.
                let mut upd_cons: Vec<Con> = Vec::new();
                let mut any_body_modified = false;
                let mut upd_ok = true;
                for (i, (body, var)) in c.sums.iter().enumerate() {
                    let mut delta = Expr::Int(0);
                    for m in &markers {
                        let map: HashMap<String, String> = [(var.clone(), m.to_string())].into();
                        let body_m = crate::analyse::rename_vars(body, &map);
                        if !crate::analyse::mentions_any(&body_m, &modified_num) {
                            continue;
                        }
                        // A conditional body whose value changes (`if active(a) then bal(a) else 0`) has a
                        // delta that depends on the condition — `ground` would reduce the `if` unsoundly and
                        // could certify a false PRESERVED (a debit of an INACTIVE account leaves the sum
                        // unchanged but moves the total). Until the case-split lands, skip this pair rather
                        // than trust the collapsed delta. A non-conditional body is unaffected.
                        if contains_cond(&body_m) {
                            upd_ok = false;
                            skipped_cons.insert(c.name.clone());
                            break;
                        }
                        any_body_modified = true;
                        let body_m_post = crate::analyse::prime(&body_m, &modified_num, false);
                        delta = Expr::Binary {
                            op: BinOp::Add,
                            lhs: Box::new(delta),
                            rhs: Box::new(Expr::Binary { op: BinOp::Sub, lhs: Box::new(body_m_post), rhs: Box::new(body_m) }),
                        };
                    }
                    if !upd_ok {
                        break; // a conditional modified body: skip this invariant for this action
                    }
                    // __S{i}' = __S{i} + delta
                    let s_update = Expr::Binary {
                        op: BinOp::Eq,
                        lhs: Box::new(spname(i)),
                        rhs: Box::new(Expr::Binary { op: BinOp::Add, lhs: Box::new(sname(i)), rhs: Box::new(delta) }),
                    };
                    let (uc, un) = ground(&s_update, &st2);
                    if un {
                        upd_ok = false;
                        break;
                    }
                    upd_cons.extend(uc);
                }
                if !upd_ok {
                    continue;
                }
                let repls: Vec<Expr> = (0..c.sums.len()).map(sname).collect();
                let repls_post: Vec<Expr> = (0..c.sums.len()).map(spname).collect();
                let inv_pre = replace_sums(&c.body_qf, &repls);
                // Relevant if the action changes a summed quantity OR the total side; else untouched.
                if !any_body_modified && !crate::analyse::mentions_any(&inv_pre, &modified_num) {
                    continue;
                }
                let inv_post = crate::analyse::prime(&replace_sums(&c.body_qf, &repls_post), &modified_num, false);
                let (pre_cons, pn) = ground(&inv_pre, &st2);
                let (post_cons, on) = ground(&inv_post, &st2);
                if pn || on || post_cons.is_empty() {
                    continue;
                }
                let mut broke = false;
                'search: for pc in &post_cons {
                    for neg in negate_con(pc) {
                        let mut q: Vec<Con> = pre_cons.clone();
                        q.extend(guard_cons.clone());
                        q.extend(effect_cons.clone());
                        q.extend(upd_cons.clone());
                        q.push(neg);
                        if let Outcome::Sat(_) = solve(&q) {
                            broke = true;
                            break 'search;
                        }
                    }
                }
                if broke {
                    broken_cons.insert(c.name.clone());
                    out.push(Diagnostic::warning(
                        it.span,
                        format!("action `{aname}` in `{}` can break conservation invariant `{}`: it changes the summed quantity without an equal change to the total, so the aggregate no longer balances. Adjust the total (or offset with a matching change).", d.name, c.name),
                    ));
                }
            }
        }
        // A conservation invariant no action breaks is preserved (every action keeps the aggregate balanced).
        if has_action {
            for c in &cons_invs {
                if !broken_cons.contains(&c.name) && !skipped_cons.contains(&c.name) {
                    out.push(Diagnostic::warning(
                        d.span,
                        format!("conservation invariant `{}` in `{}` is PRESERVED: every action keeps the total equal to the sum.", c.name, d.name),
                    ));
                }
            }
        }
    }
    out
}

/// Flatten a conjunction `A and B and C` into its conjuncts; a non-conjunction is a single-element list.
fn conjuncts(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            let mut v = conjuncts(lhs);
            v.extend(conjuncts(rhs));
            v
        }
        _ => vec![e],
    }
}

/// Extract `(obs, arg, tag)` from a single-argument enum equality `obs(arg) = tag`.
fn enum_guard_atom(e: &Expr) -> Option<(String, String, String)> {
    let Expr::Binary { op: BinOp::Eq, lhs, rhs } = e else { return None };
    let Expr::Name(tag) = &**rhs else { return None };
    let Expr::App { head, args } = &**lhs else { return None };
    let Expr::Name(obs) = head.as_ref() else { return None };
    if args.len() != 1 {
        return None;
    }
    let Expr::Name(arg) = &args[0] else { return None };
    Some((obs.clone(), arg.clone(), tag.clone()))
}

/// The post-state truth of an enum-guard antecedent `obs(arg) = tag` under a single-subject action.
enum GuardPost {
    /// Definitely true post — the action sets `obs` to `tag`, or requires `tag` and leaves it unchanged.
    /// The bound may be soundly asserted broken (the LRA need not force the guard; it holds).
    True,
    /// Definitely false post — the action sets `obs` to another tag, or requires another tag. The invariant
    /// is vacuous for this action, so it genuinely cannot break it; safe to leave the target certifiable.
    False,
    /// Undecidable — the action does not determine the guard (it neither sets nor requires `obs`, or the
    /// guard is on the OTHER entity). The guard COULD be true, so a break is possible but the LRA cannot
    /// force the opaque enum; the caller must not assert it (false-alarm risk) AND must not certify the
    /// target preserved (a real break could be missed).
    Unknown,
}

/// Classify the post-state truth of an enum-guard antecedent under a single-subject action (subject `_e`).
fn classify_guard_post(g: &Expr, ensures: &Expr, guard: &Option<Expr>, st: &HashMap<String, String>) -> GuardPost {
    let Some((obs, arg, tag)) = enum_guard_atom(g) else { return GuardPost::Unknown };
    if arg != "_e" {
        return GuardPost::Unknown; // guard on the non-subject entity: free, cannot decide
    }
    let decide = |v: &str| if v == tag { GuardPost::True } else { GuardPost::False };
    match assigned_enum_tag(ensures, &obs, st) {
        Some(v) => decide(&v),
        None => match guard.as_ref().and_then(|g| required_enum_tag(g, &obs, st)) {
            Some(r) => decide(&r),
            None => GuardPost::Unknown,
        },
    }
}

/// 2-entity numeric ordering preservation (#49): `every a, b :: G(a,b) implies R(a,b)`, G and R single
/// linear comparisons over numeric keys (e.g. `ver(a) > ver(b) implies off(a) >= off(b)`). For each
/// single-subject action it asks: can the action, acting on entity `e`, break the ordering against some
/// other entity `f`? The VC assumes the FULL inductive hypothesis — the invariant for both orderings of the
/// pair AND every other invariant at `e` and `f` — then applies the effect and guard and negates the post.
/// A SAT witness is a real reachable break. `emit` drops numeric-antecedent implications, so each
/// implication is case-split manually; an antecedent the LRA cannot model is treated opaquely (either its
/// consequent holds, or it is simply dropped) — both branches sound. Returns the diagnostics and the names
/// it actually checked, so the caller drops the weaker "NOT preservation-checked" note for them.
pub fn relational_arith_preservation(module: &Module, src: &str) -> (Vec<Diagnostic>, HashSet<String>) {
    let mut out = Vec::new();
    let mut checked: HashSet<String> = HashSet::new();
    // Targets any action left incompletely checked (an undecidable enum guard, or a bounded-out enumeration).
    // Certification is withheld for these even if another action engaged them — else a missed break from the
    // incomplete action would be silently certified preserved.
    let mut incomplete_targets: HashSet<String> = HashSet::new();
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

        // A single linear comparison that grounds cleanly under `st` (numeric).
        let is_lin_cmp = |e: &Expr, st: &HashMap<String, String>| -> bool {
            matches!(e, Expr::Binary { op: BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq, .. })
                && !ground(e, st).1
        };
        let split_impl = |e: &Expr| -> (Option<Expr>, Expr) {
            if let Expr::Binary { op: BinOp::Implies, lhs, rhs } = e {
                (Some((**lhs).clone()), (**rhs).clone())
            } else {
                (None, e.clone())
            }
        };

        // Targets: 2-entity ordering invariants we can check. Hypotheses: every invariant, kept as its
        // quantifier-free body plus its entity variables, to instantiate at the pair.
        struct Target {
            name: String,
            a: String,
            b: String,
            qf: Expr, // G implies R, with vars a,b
        }
        let mut targets: Vec<Target> = Vec::new();
        let mut hyp_invs: Vec<(Vec<String>, Expr)> = Vec::new(); // (entity vars, quantifier-free body)
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Invariant) {
            let Some(b) = it.body else { continue };
            let body = parse_predicate(b.slice(src)).0;
            let (vars, qf) = match crate::analyse::universal_body(&body) {
                Some(x) => x,
                None if !crate::analyse::has_quant(&body) => {
                    let mut ev = HashSet::new();
                    crate::analyse::collect_entity_vars(&body, &mut ev);
                    (ev.into_iter().collect(), body.clone())
                }
                None => continue,
            };
            hyp_invs.push((vars.clone(), qf.clone()));
            if vars.len() == 2 {
                let (ante, cons) = split_impl(&qf);
                if let Some(ante) = ante {
                    // The antecedent may be a single linear comparison, an enum-equality guard (#72), OR a
                    // CONJUNCTION of those (achronic `ekey(a)=ekey(b) and version(a)>version(b)`, #77). The VC
                    // grounds the whole antecedent and `branch_alts` disjunctively splits its negation, so a
                    // conjunctive guard is now soundly checkable.
                    let single_ok = |e: &Expr| is_lin_cmp(e, &st) || (enum_guard_atom(e).is_some() && ground(e, &st).1);
                    let ante_ok = conjuncts(&ante).iter().all(|c| single_ok(c));
                    if ante_ok && is_lin_cmp(&cons, &st) {
                        if let (Some(n),) = (it.name.clone(),) {
                            targets.push(Target { name: n, a: vars[0].clone(), b: vars[1].clone(), qf: qf.clone() });
                        }
                    }
                }
            }
        }
        if targets.is_empty() {
            continue;
        }

        let mut st2 = st.clone();
        for n in &state_names {
            if let Some(t) = st.get(n) {
                if numeric(t) {
                    st2.insert(format!("{n}'"), t.clone());
                }
            }
        }

        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let Some(ens_raw) = it.ensures_expr(src) else { continue };
            let grd_raw = it.requires.map(|s| parse_predicate(s.slice(src)).0);
            let mut ev = HashSet::new();
            crate::analyse::collect_entity_vars(&ens_raw, &mut ev);
            if let Some(g) = &grd_raw {
                crate::analyse::collect_entity_vars(g, &mut ev);
            }
            if ev.len() > 1 {
                continue; // single-subject actions only
            }
            let subj: HashMap<String, String> = ev.iter().map(|v| (v.clone(), "_e".to_string())).collect();
            let ensures = crate::analyse::rename_vars(&ens_raw, &subj);
            let guard = grd_raw.map(|g| crate::analyse::rename_vars(&g, &subj));
            let mut modified = HashSet::new();
            crate::analyse::collect_writes(&ensures, false, &state_names, &mut modified);
            let modified: HashSet<String> = modified.into_iter().filter(|m| st.get(m).map(|t| numeric(t)).unwrap_or(false)).collect();
            if modified.is_empty() {
                continue;
            }
            let effect = crate::analyse::to_post(&ensures, &modified, false);
            let (effect_cons, en) = ground(&effect, &st2);
            if en {
                continue;
            }
            let guard_cons = match &guard {
                Some(g) => {
                    let (c, n) = ground(g, &st2);
                    if n {
                        Vec::new() // an unmodellable guard asserts nothing (sound: keeps more pre-states)
                    } else {
                        c
                    }
                }
                None => Vec::new(),
            };

            for t in &targets {
                // Instantiate the target at (e,f) and (f,e); the pair is only disturbed if the action's
                // writes touch it.
                let map_ef: HashMap<String, String> = [(t.a.clone(), "_e".into()), (t.b.clone(), "_f".into())].into();
                let map_fe: HashMap<String, String> = [(t.a.clone(), "_f".into()), (t.b.clone(), "_e".into())].into();
                let inst_ef = crate::analyse::resolve_entity_eq(&crate::analyse::rename_vars(&t.qf, &map_ef));
                let inst_fe = crate::analyse::resolve_entity_eq(&crate::analyse::rename_vars(&t.qf, &map_fe));
                if !crate::analyse::mentions_any(&inst_ef, &modified) && !crate::analyse::mentions_any(&inst_fe, &modified) {
                    continue;
                }

                // Build the implication hypotheses — the FULL inductive hypothesis so a SAT witness is a
                // genuinely reachable pre-state: every invariant instantiated at the pair. Single-entity
                // invariants at _e and at _f; every two-entity invariant (the target included) at both
                // orderings of the pair; a constant invariant as itself. Omitting any true invariant would
                // only ever add false positives, so include them all.
                let mut hyps: Vec<(Option<Expr>, Expr)> = Vec::new();
                for (vars, qf) in &hyp_invs {
                    let insts: Vec<HashMap<String, String>> = match vars.len() {
                        0 => vec![HashMap::new()],
                        1 => ["_e", "_f"]
                            .iter()
                            .map(|who| [(vars[0].clone(), who.to_string())].into())
                            .collect(),
                        2 => vec![
                            [(vars[0].clone(), "_e".into()), (vars[1].clone(), "_f".into())].into(),
                            [(vars[0].clone(), "_f".into()), (vars[1].clone(), "_e".into())].into(),
                        ],
                        _ => continue,
                    };
                    for m in insts {
                        let inst = crate::analyse::resolve_entity_eq(&crate::analyse::rename_vars(qf, &m));
                        let (a, c) = split_impl(&inst);
                        hyps.push((a, c));
                    }
                }

                // Ground one implication hypothesis under an assumed truth of its antecedent, returning the
                // ALTERNATIVE constraint sets to add (the branch is a disjunction; an empty outer vec means
                // this branch is unusable, skip the combo). A single alternative is the common case; a
                // multi-constraint antecedent being FALSE is `¬(c1 ∧ … ∧ ck) = ¬c1 ∨ … ∨ ¬ck`, one
                // alternative per negated conjunct (#77), so a conjunctive-guard invariant is now checkable.
                let branch_alts = |ante: &Option<Expr>, cons: &Expr, ante_true: bool| -> Vec<Vec<Con>> {
                    match ante {
                        None => {
                            let (c, note) = ground(cons, &st2);
                            if note { vec![] } else { vec![c] }
                        }
                        Some(a) => {
                            let (ca, na) = ground(a, &st2);
                            if ante_true {
                                let (cc, nc) = ground(cons, &st2);
                                // Consequent must model; antecedent may be opaque (boolean) — then just
                                // assert the consequent (sound: if the guard holds, the bound holds).
                                if nc {
                                    return vec![];
                                }
                                let mut v = cc;
                                if !na {
                                    v.extend(ca);
                                }
                                vec![v]
                            } else if na {
                                // Opaque antecedent being false asserts nothing — one empty alternative.
                                vec![vec![]]
                            } else {
                                // ¬(c1 ∧ … ∧ ck): one alternative per negated conjunct (each may itself
                                // split when it is an equality).
                                ca.iter().flat_map(|c| negate_con(c).into_iter().map(|neg| vec![neg])).collect()
                            }
                        }
                    }
                };

                // Keep the case-split bounded; a very large hypothesis set stays unchecked (the #50 note
                // remains) rather than being marked checked without a verdict.
                if hyps.len() > 16 {
                    continue;
                }

                let mut broke = false;
                let mut engaged = false;
                let mut incomplete = false;
                for post_src in [&inst_ef, &inst_fe] {
                    let post = crate::analyse::to_post(post_src, &modified, false);
                    let (g_post, r_post) = split_impl(&post);
                    let Some(g_post) = g_post else { continue };
                    let (rc, rn) = ground(&r_post, &st2);
                    if rn || rc.len() != 1 {
                        continue; // consequent not a clean single comparison
                    }
                    let (gc, gn) = ground(&g_post, &st2);
                    let gc = if gn {
                        // An enum-guard antecedent adds no LRA constraint: it is decided from the action.
                        match classify_guard_post(&g_post, &ensures, &guard, &st) {
                            // Guard true post: assert `¬R` (the LRA need not force the guard, it holds).
                            GuardPost::True => Vec::new(),
                            // Guard false post: the invariant is vacuous for this action — genuinely no break,
                            // and the target stays certifiable.
                            GuardPost::False => continue,
                            // Guard undecidable: a break is possible but cannot be asserted (false-alarm risk)
                            // AND the target must not be certified preserved — a real break could be missed
                            // (e.g. an action that raises the bound without constraining the guard).
                            GuardPost::Unknown => {
                                incomplete_targets.insert(t.name.clone());
                                continue;
                            }
                        }
                    } else {
                        gc
                    };
                    engaged = true;
                    // ¬post = G_post ∧ ¬R_post; ¬R_post may split (Eq → two).
                    for neg_r in negate_con(&rc[0]) {
                        let mut base = effect_cons.clone();
                        base.extend(guard_cons.clone());
                        base.extend(gc.clone());
                        base.push(neg_r);
                        // Enumerate antecedent truths of the implication hypotheses.
                        let n = hyps.len();
                        'combos: for mask in 0..(1u32 << n) {
                            // Per-hypothesis alternative constraint sets for this true/false assignment.
                            let mut per_hyp: Vec<Vec<Vec<Con>>> = Vec::with_capacity(n);
                            for (i, (ante, conseq)) in hyps.iter().enumerate() {
                                let ante_true = (mask >> i) & 1 == 1;
                                let alts = branch_alts(ante, conseq, ante_true);
                                if alts.is_empty() {
                                    continue 'combos; // this hypothesis can't be split; skip combo
                                }
                                per_hyp.push(alts);
                            }
                            // Try the cross-product of alternatives (a disjunctive ¬antecedent contributes
                            // more than one). Bound it: a product too large to enumerate leaves the target
                            // INCOMPLETE (not certified checked) rather than risk reading a missed break as
                            // preservation.
                            let total: usize = per_hyp.iter().map(|a| a.len()).product();
                            if total > 4096 {
                                incomplete = true;
                                continue 'combos;
                            }
                            for combo_idx in 0..total {
                                let mut cons = base.clone();
                                let mut rem = combo_idx;
                                for alts in &per_hyp {
                                    let pick = rem % alts.len();
                                    rem /= alts.len();
                                    cons.extend(alts[pick].iter().cloned());
                                }
                                if let Outcome::Sat(_) = solve(&cons) {
                                    broke = true;
                                    break 'combos;
                                }
                            }
                        }
                        if broke {
                            break;
                        }
                    }
                    if broke {
                        break;
                    }
                }
                // Certify checked only when the VC engaged AND enumeration was complete — an incomplete
                // (bounded-out) case must stay honestly unchecked, never read as preservation. A bounded-out
                // enumeration for THIS action also withholds certification across all actions.
                if incomplete {
                    incomplete_targets.insert(t.name.clone());
                }
                if engaged && !incomplete {
                    checked.insert(t.name.clone());
                }
                if broke {
                    let aname = it.name.clone().unwrap_or_else(|| "<anon>".into());
                    out.push(Diagnostic::warning(
                        it.span,
                        format!("action `{aname}` in `{}` can break relational invariant `{}`: acting on one entity can drive its key past another's without preserving the ordering. Guard the action so the relation is maintained.", d.name, t.name),
                    ));
                }
            }
        }
    }
    // Withhold certification from any target an action left incompletely checked (cross-action safety).
    checked.retain(|n| !incomplete_targets.contains(n));
    (out, checked)
}

/// Linear-arithmetic entailment for refinement: do the component's invariants `x_invs` entail `promise`?
/// Returns `Some(true)` if every way the promise could fail is inconsistent with the invariants,
/// `Some(false)` with the first counterexample shape it finds, or `None` if the promise is not linearisable
/// (the caller then reports it as not-statically-checked). Reuses the same `_e`-normalisation and simplex
/// as arithmetic preservation, so it is sound: only cleanly-linear invariants contribute as hypotheses.
pub fn entails_linear(x_invs: &[Expr], promise: &Expr, st: &HashMap<String, String>) -> Option<bool> {
    let mut x_cons: Vec<Con> = Vec::new();
    for inv in x_invs {
        if let Some(body) = arith_reduce(inv) {
            let (c, notes) = ground(&body, st);
            if !notes {
                x_cons.extend(c);
            }
        }
    }
    let body = arith_reduce(promise)?;
    let (p_cons, p_notes) = ground(&body, st);
    if p_notes || p_cons.is_empty() {
        return None;
    }
    for pc in &p_cons {
        for neg in negate_con(pc) {
            let mut q = x_cons.clone();
            q.push(neg);
            if let Outcome::Sat(_) = solve(&q) {
                return Some(false);
            }
        }
    }
    Some(true)
}

/// Entailment for a STATE-GUARDED linear promise `Gfin implies A_c` (the SMT rung applied to refinement).
/// A contract promise that couples a finite state to an arithmetic bound is entailed by X iff, whenever the
/// guard holds, X's invariants entail the bound. The hypotheses valid under the guard are X's unconditional
/// linear invariants plus the bounds of X's invariants guarded by the SAME finite condition, plus the
/// promise's own arithmetic guard. None if the promise is not a finite-guarded linear form (caller falls
/// back to [`entails_linear`]). Sound: only cleanly-linear hypotheses contribute.
pub fn entails_guarded_linear(x_invs: &[Expr], promise: &Expr, st: &HashMap<String, String>) -> Option<bool> {
    let pqf = arith_reduce(promise)?;
    let (pconds, parith, pa) = finite_guarded_inv(&pqf, st)?;
    let mut hyps: Vec<Expr> = Vec::new();
    for inv in x_invs {
        let Some(iqf) = arith_reduce(inv) else { continue };
        match finite_guarded_inv(&iqf, st) {
            // An X invariant guarded by the same finite condition contributes its bound under the guard.
            Some((iconds, _, ia)) if iconds == pconds => hyps.push(ia),
            Some(_) => {} // guarded by a different condition: does not apply under this guard
            None => hyps.push(iqf), // unconditional: always holds
        }
    }
    hyps.extend(parith); // the promise's own arithmetic guard is assumed when it holds
    entails_linear(&hyps, &pa, st)
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
        // A refinement type (`refine[x]`, from a `where` clause) is a stated constraint on the value, not
        // a derived property: it is *meant* to be independent, so the "relies on an unstated assumption"
        // critique is a false alarm. Keep it as a hypothesis for the others, but do not critique it here.
        if name.starts_with("refine[") {
            continue;
        }
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
                            "invariant `{name}` in `{comp}` is NOT entailed by the others: they permit `{}`, which it forbids. So it relies on an assumption not captured by the other invariants (typically a sign or ordering constraint on an input) — state that assumption if it is meant to hold.",
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
pub(crate) fn numeric(t: &str) -> bool {
    let h = head(t);
    matches!(
        h.as_str(),
        "money" | "amount" | "cash" | "rate" | "ratio" | "factor" | "percent" | "percentage"
            | "int" | "integer" | "nat" | "natural" | "count" | "number" | "num" | "decimal"
            | "scalar" | "mass" | "length" | "duration" | "weight" | "distance" | "quantity" | "volume"
    )
}
pub(crate) fn head(t: &str) -> String {
    t.trim().split('(').next().unwrap_or("").trim().to_lowercase()
}

/// Bind quantifiers over the domain and emit a linear constraint per ground comparison.
/// True if the state named `n` has an enum/variant type (`{ a | b | c }`), which the enum/SAT tier owns.
fn enum_typed(n: &str, st: &HashMap<String, String>) -> bool {
    st.get(n).map(|t| t.trim_start().starts_with('{')).unwrap_or(false)
}

/// True if the state named `n` is boolean-typed. A boolean is a two-valued finite type (`{ true | false }`)
/// for the purpose of finite-guard reasoning.
fn bool_typed(n: &str, st: &HashMap<String, String>) -> bool {
    st.get(n).map(|t| matches!(t.trim().to_ascii_lowercase().as_str(), "bool" | "boolean")).unwrap_or(false)
}

/// True if `n` has a finite type the SAT/enum tier owns — an enum/variant or a boolean.
fn finite_typed(n: &str, st: &HashMap<String, String>) -> bool {
    enum_typed(n, st) || bool_typed(n, st)
}

/// The head observable name of `obs(..)` or `obs`, if any.
fn app_head(e: &Expr) -> Option<&str> {
    match e {
        Expr::App { head, .. } => match head.as_ref() {
            Expr::Name(h) => Some(h),
            _ => None,
        },
        Expr::Name(n) => Some(n),
        _ => None,
    }
}

/// True if `e` is a guard built purely from finite-state atoms — enum (dis)equalities `outcome = success`,
/// boolean equalities `active = true`, bare boolean flags `active`, and their and/or/not combinations.
/// Such a guard is checked by the enum/SAT tier, not the arithmetic tier, so the arithmetic tier should
/// drop it silently rather than report it as an unchecked nonlinear term.
fn is_enum_guard(e: &Expr, st: &HashMap<String, String>) -> bool {
    match e {
        Expr::Binary { op: BinOp::Eq | BinOp::Ne, lhs, rhs } => {
            let head_finite = |x: &Expr| app_head(x).map(|h| finite_typed(h, st)).unwrap_or(false);
            (head_finite(lhs) && matches!(rhs.as_ref(), Expr::Name(_)))
                || (head_finite(rhs) && matches!(lhs.as_ref(), Expr::Name(_)))
        }
        Expr::Binary { op: BinOp::And | BinOp::Or, lhs, rhs } => is_enum_guard(lhs, st) && is_enum_guard(rhs, st),
        Expr::Unary { op: UnOp::Not, e } => is_enum_guard(e, st),
        // a bare boolean flag used as a guard (`active`)
        _ => app_head(e).map(|h| bool_typed(h, st)).unwrap_or(false),
    }
}

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
            // A guard the arithmetic tier cannot evaluate drops the whole implication (sound: it asserts
            // nothing). Only note it as unchecked when it is a genuine arithmetic concern — a pure ENUM
            // guard (`outcome = success`) is owned by the enum/SAT tier, so noting it here as "nonlinear
            // not checked" double-accounts and misleads. Drop it silently.
            None if is_enum_guard(lhs, st) => {}
            None => notes.push(format!("guard `{}`", crate::analyse::canon(lhs))),
        },
        Expr::Binary { op: op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), lhs, rhs } => {
            let (l, r) = match (lower(lhs, env, st), lower(rhs, env, st)) {
                (Some(l), Some(r)) => (l, r),
                // A comparison over an `if <finite-cond> then A else B` is conditional (mixed), not
                // nonlinear: the conditional-invariant expansion checks it via the SMT rung, so do not
                // report it here as an unchecked nonlinear term.
                _ if count_conds(e) == 1 && first_cond(e).map(|c| is_enum_guard(&c, st)).unwrap_or(false) => return,
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
    use super::{aggregate_preservation, arithmetic, enum_guarded_preservation};
    use crate::parser::parse;

    fn run(src: &str) -> Vec<String> {
        let m = parse(src).module;
        arithmetic(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
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
    fn enum_guarded_preservation_catches_and_spares_correctly() {
        let egp = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            enum_guarded_preservation(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
        };
        let hdr = "-- allium: 4\ncomponent E\n  entity O\n  observable state outcome(O) : { success | failure }\n  observable state count(O) : Number\n  invariant ok means outcome(o) = success implies count(o) >= 0\n";
        // Breaks: sets count negative while success holds after.
        let botch = format!("{hdr}  action botch\n    requires outcome(o) = success\n    ensures count(o) = 0 - 1\nend\n");
        assert!(any(&egp(&botch), "`botch` in `E` can break state-guarded invariant `ok`"), "{:#?}", egp(&botch));
        // Breaks: transitions failure->success while setting count negative (must establish the bound).
        let finish = format!("{hdr}  action finish\n    requires outcome(o) = failure\n    ensures outcome(o) = success and count(o) = 0 - 1\nend\n");
        assert!(any(&egp(&finish), "`finish` in `E` can break state-guarded invariant `ok`"), "{:#?}", egp(&finish));
        // Safe: maintains the bound under success.
        let safe = format!("{hdr}  action safe\n    requires outcome(o) = success\n    ensures count(o) = 5\nend\n");
        assert!(!any(&egp(&safe), "can break state-guarded"), "{:#?}", egp(&safe));
        // Safe: acts under failure (guard inactive), so a negative count is fine.
        let onfail = format!("{hdr}  action onfail\n    requires outcome(o) = failure\n    ensures count(o) = 0 - 1\nend\n");
        assert!(!any(&egp(&onfail), "can break state-guarded"), "{:#?}", egp(&onfail));
        // Safe: transitions success->failure (guard inactive after), so a negative count is fine.
        let failit = format!("{hdr}  action failit\n    ensures outcome(o) = failure and count(o) = 0 - 1\nend\n");
        assert!(!any(&egp(&failit), "can break state-guarded"), "{:#?}", egp(&failit));

        // A numeric sum-type PAYLOAD field is a numeric state: a variant-guarded bound over it is checked.
        let phdr = "-- allium: 4\ncomponent V\n  entity O\n  observable state outcome(O) : { success { out : Number } | failure { err : Number } }\n  invariant outok means outcome(o) = success implies out(o) >= 0\n";
        let bad = format!("{phdr}  action rec\n    requires outcome(o) = success\n    ensures out(o) = 0 - 3\nend\n");
        assert!(any(&egp(&bad), "`rec` in `V` can break state-guarded invariant `outok`"), "{:#?}", egp(&bad));
        let good = format!("{phdr}  action rec\n    requires outcome(o) = success\n    ensures out(o) = 4\nend\n");
        assert!(!any(&egp(&good), "can break state-guarded"), "{:#?}", egp(&good));

        // A BOOLEAN flag guard is a two-valued finite guard: `active implies balance >= 0` is checked too.
        let bhdr = "-- allium: 4\ncomponent Acct\n  entity X\n  observable state active(X) : bool\n  observable state balance(X) : Money\n  invariant solvent means active(x) implies balance(x) >= 0\n";
        let drain = format!("{bhdr}  action drain\n    requires active(x)\n    ensures balance(x) = 0 - 1\nend\n");
        assert!(any(&egp(&drain), "`drain` in `Acct` can break state-guarded invariant `solvent`"), "{:#?}", egp(&drain));
        let onclosed = format!("{bhdr}  action drain\n    requires not active(x)\n    ensures balance(x) = 0 - 1\nend\n");
        assert!(!any(&egp(&onclosed), "can break state-guarded"), "{:#?}", egp(&onclosed));
        let open = format!("{bhdr}  action open\n    ensures active(x) and balance(x) = 0 - 5\nend\n");
        assert!(any(&egp(&open), "`open` in `Acct` can break state-guarded invariant `solvent`"), "{:#?}", egp(&open));

        // A CONJUNCTIVE finite guard: the bound applies only when BOTH conditions hold.
        let chdr = "-- allium: 4\ncomponent E\n  entity O\n  observable state outcome(O) : { success | failure }\n  observable state phase(O) : { running | halted }\n  observable state count(O) : Number\n  invariant ok means outcome(o) = success and phase(o) = running implies count(o) >= 0\n";
        let both = format!("{chdr}  action botch\n    requires outcome(o) = success and phase(o) = running\n    ensures count(o) = 0 - 1\nend\n");
        assert!(any(&egp(&both), "with `outcome = success and phase = running` holding"), "{:#?}", egp(&both));
        // One condition false after the action (phase halted): the bound does not apply, so no break.
        let one = format!("{chdr}  action botch\n    requires outcome(o) = success and phase(o) = halted\n    ensures count(o) = 0 - 1\nend\n");
        assert!(!any(&egp(&one), "can break state-guarded"), "{:#?}", egp(&one));

        // A safe action that engages the invariant earns a positive PRESERVED verdict.
        let safe2 = format!("{hdr}  action ok_act\n    requires outcome(o) = success\n    ensures count(o) = 7\nend\n");
        assert!(any(&egp(&safe2), "state-guarded invariant `ok` in `E` is PRESERVED"), "{:#?}", egp(&safe2));
        // When some action breaks it, there is no PRESERVED verdict.
        assert!(!any(&egp(&botch), "is PRESERVED"), "{:#?}", egp(&botch));

        // A MIXED guard (finite + arithmetic): the bound applies only when the finite guard holds AND the
        // arithmetic condition holds. `advance_past` sets `wm = off + 1` while `healthy` -> breaks it; a
        // guarded advance to `off` does not.
        let ghdr = "-- allium: 4\ncomponent L\n  entity S\n  observable state status(S) : { healthy | corrupted }\n  observable state wm(S) : Number\n  given off : Number\n  invariant b means status(s) = healthy and wm(s) >= 0 implies wm(s) <= off\n";
        let past = format!("{ghdr}  action advance_past\n    requires status(s) = healthy\n    ensures wm(s) = off + 1\nend\n");
        assert!(any(&egp(&past), "`advance_past` in `L` can break state-guarded invariant `b`"), "{:#?}", egp(&past));
        let ok = format!("{ghdr}  action advance\n    requires status(s) = healthy and wm(s) < off\n    ensures wm(s) = off\nend\n");
        assert!(!any(&egp(&ok), "can break state-guarded"), "{:#?}", egp(&ok));

        // Cross-invariant pre-hypothesis: `begin_compact` sets `compacting` but not the watermark; because
        // `wm_bounded` (guard `healthy`, which the action requires) already pins `wm <= off`, the compacting
        // bound cannot be broken. Without other guarded bounds as pre-hypotheses this false-alarmed.
        let mhdr = "-- allium: 4\ncomponent L\n  entity S\n  observable state status(S) : { healthy | corrupted }\n  observable state wm(S) : Number\n  observable state compacting(S) : bool\n  given off : Number\n  invariant wm_bounded means status(s) = healthy implies wm(s) <= off\n  invariant compact_frozen means compacting(s) implies wm(s) <= off\n";
        let bc = format!("{mhdr}  action begin_compact\n    requires status(s) = healthy\n    ensures compacting(s)\nend\n");
        assert!(!any(&egp(&bc), "can break state-guarded invariant `compact_frozen`"), "cross-invariant pre-hypothesis: {:#?}", egp(&bc));

        // Base case: init that activates the guard but violates the bound is caught; a good init is not; a
        // guard inactive at init is not (the bound does not apply there).
        let ib = "-- allium: 4\ncomponent E\n  entity O\n  observable state outcome(O) : { success | failure }\n  observable state count(O) : Number\n  init means outcome(o) = success and count(o) = 0 - 1\n  invariant ok means outcome(o) = success implies count(o) >= 0\n  action noop\n    requires outcome(o) = failure\n    ensures count(o) = count(o)\nend\n";
        assert!(any(&egp(ib), "`init` in `E` does not establish state-guarded invariant `ok`"), "{:#?}", egp(ib));
        let ig = ib.replace("count(o) = 0 - 1", "count(o) = 0");
        assert!(!any(&egp(&ig), "does not establish state-guarded"), "{:#?}", egp(&ig));
        let inact = ib.replace("outcome(o) = success and count(o) = 0 - 1", "outcome(o) = failure and count(o) = 0 - 1");
        assert!(!any(&egp(&inact), "does not establish state-guarded"), "guard inactive at init: {:#?}", egp(&inact));
        // init that does NOT assign the bound's state (count) leaves it a free input — not init's to
        // establish, so no false alarm.
        let free = "-- allium: 4\ncomponent E\n  entity O\n  observable state outcome(O) : { success | failure }\n  observable state count(O) : Number\n  init means outcome(o) = success\n  invariant ok means outcome(o) = success implies count(o) >= 0\n  action noop\n    requires outcome(o) = failure\n    ensures count(o) = count(o)\nend\n";
        assert!(!any(&egp(free), "does not establish state-guarded"), "free input at init: {:#?}", egp(free));

        // Assumption surfacing (elicit): init pins `wm = -1` but the bound `wm <= off` needs `off >= -1`,
        // an input the spec has not constrained; the diagnostic names the missing assumption.
        let elicit = "-- allium: 4\ncomponent L\n  entity S\n  observable state status(S) : { healthy | corrupted }\n  observable state wm(S) : Number\n  given off : Number\n  invariant b means status(s) = healthy implies wm(s) <= off\n  init means status(s) = healthy and wm(s) = 0 - 1\n  action advance\n    requires status(s) = healthy and wm(s) < off\n    ensures wm(s) = off\nend\n";
        assert!(any(&egp(elicit), "holds at init only if `0 - 1 <= off`"), "{:#?}", egp(elicit));

        // A guarded bound whose guard is OFF at init (init sets status=active, guard is `status=closed`)
        // is vacuously established, so preserved => INDUCTIVE, not merely PRESERVED.
        let vac = "-- allium: 4\ncomponent L\n  entity O\n  observable state status(O) : { active | closed }\n  observable state bal(O) : Number\n  init means status(o) = active and bal(o) = 5\n  invariant closed_zero means status(o) = closed implies bal(o) = 0\n  action close\n    requires status(o) = active and bal(o) = 0\n    ensures status(o) = closed\nend\n";
        assert!(any(&egp(vac), "state-guarded invariant `closed_zero` in `L` is INDUCTIVE"), "vacuously established: {:#?}", egp(vac));

        // Established by init AND preserved -> the stronger INDUCTIVE verdict.
        let ind = "-- allium: 4\ncomponent E\n  entity O\n  observable state outcome(O) : { success | failure }\n  observable state count(O) : Number\n  init means outcome(o) = success and count(o) = 0\n  invariant ok means outcome(o) = success implies count(o) >= 0\n  action inc\n    requires outcome(o) = success\n    ensures count(o) = count(o) + 1\nend\n";
        assert!(any(&egp(ind), "state-guarded invariant `ok` in `E` is INDUCTIVE"), "{:#?}", egp(ind));

        // A NEGATED enum guard (`phase <> pending`): the bound applies once out of pending. Breaks under
        // active; not under pending; a transition into active must respect it; out to pending is exempt.
        let nhdr = "-- allium: 4\ncomponent E\n  entity O\n  observable state phase(O) : { pending | active | done }\n  observable state bal(O) : Number\n  invariant nonneg means phase(o) <> pending implies bal(o) >= 0\n";
        let under_active = format!("{nhdr}  action botch\n    requires phase(o) = active\n    ensures bal(o) = 0 - 1\nend\n");
        assert!(any(&egp(&under_active), "`botch` in `E` can break state-guarded invariant `nonneg`"), "{:#?}", egp(&under_active));
        let under_pending = format!("{nhdr}  action botch\n    requires phase(o) = pending\n    ensures bal(o) = 0 - 1\nend\n");
        assert!(!any(&egp(&under_pending), "can break state-guarded"), "guard inactive under pending: {:#?}", egp(&under_pending));
        let into_active = format!("{nhdr}  action activate\n    requires phase(o) = pending\n    ensures phase(o) = active and bal(o) = 0 - 1\nend\n");
        assert!(any(&egp(&into_active), "`activate` in `E` can break state-guarded invariant `nonneg`"), "turning the guard on must respect it: {:#?}", egp(&into_active));
        // The break on a transition INTO the guard suggests the precise missing invariant (source-state bound).
        assert!(any(&egp(&into_active), "add `phase = pending implies bal(e) >= 0`"), "elicit suggestion: {:#?}", egp(&into_active));
        let out_to_pending = format!("{nhdr}  action reset\n    requires phase(o) = active\n    ensures phase(o) = pending and bal(o) = 0 - 1\nend\n");
        assert!(!any(&egp(&out_to_pending), "can break state-guarded"), "guard off after -> exempt: {:#?}", egp(&out_to_pending));

        // Guarded MONOTONICITY (an `old`-based bound under a guard): while healthy the watermark never
        // decreases. `retreat` breaks it; `advance` does not (old grounds to the pre-value).
        let mhdr2 = "-- allium: 4\ncomponent L\n  entity S\n  observable state status(S) : { healthy | corrupted }\n  observable state wm(S) : Number\n  invariant advances means status(s) = healthy implies wm(s) >= old(wm(s))\n";
        let retreat = format!("{mhdr2}  action retreat\n    requires status(s) = healthy\n    ensures wm(s) = old(wm(s)) - 1\nend\n");
        assert!(any(&egp(&retreat), "`retreat` in `L` can break state-guarded invariant `advances`"), "{:#?}", egp(&retreat));
        let advance = format!("{mhdr2}  action advance\n    requires status(s) = healthy\n    ensures wm(s) = old(wm(s)) + 1\nend\n");
        assert!(!any(&egp(&advance), "can break state-guarded"), "{:#?}", egp(&advance));
    }

    #[test]
    fn objective_measure_monotonicity() {
        let prog = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            super::objective_progress(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
        };
        // A non-monotone measure: `enqueue` raises `pending`, so it does not witness progress.
        let bad = "-- allium: 4\ncomponent Q\n  entity R\n  observable state pending : Number\n  observable state drained(R) : bool\n  invariant nn means pending >= 0\n  action process\n    requires pending >= 1\n    ensures pending = old(pending) - 1\n  action enqueue\n    ensures pending = old(pending) + 1\n  objective drained(r) within eod\n    measure pending decreasing\nend\n";
        assert!(any(&prog(bad), "can INCREASE under action `enqueue`"), "{:#?}", prog(bad));
        // The decreasing action alone is never flagged (no false positive).
        assert!(!prog(bad).iter().any(|m| m.contains("under action `process`")), "process wrongly flagged: {:#?}", prog(bad));
        // A monotone measure (only `process`) produces no finding at all.
        let good = "-- allium: 4\ncomponent Q\n  entity R\n  observable state pending : Number\n  observable state drained(R) : bool\n  invariant nn means pending >= 0\n  action process\n    requires pending >= 1\n    ensures pending = old(pending) - 1\n  objective drained(r) within eod\n    measure pending decreasing\nend\n";
        assert!(prog(good).is_empty(), "monotone measure should be silent: {:#?}", prog(good));
    }

    #[test]
    fn init_establishment_of_unconditional_arithmetic_invariant() {
        let run_ap = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            super::arith_preservation(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
        };
        // init sets balance = -5, contradicting the invariant balance >= 0.
        let bad = "-- allium: 4\ncomponent B\n  entity A\n  observable state balance(A) : Money\n  observable state floor(A) : Money\n  init means balance(a) = 0 - 5 and floor(a) = 0\n  invariant nonneg means balance(a) >= floor(a)\n  action dep\n    ensures balance(a) = balance(a) + 1\nend\n";
        assert!(any(&run_ap(bad), "`init` in `B` does not establish arithmetic invariant `nonneg`"), "{:#?}", run_ap(bad));
        // A good init (balance = 0) does not.
        let good = bad.replace("balance(a) = 0 - 5", "balance(a) = 0");
        assert!(!any(&run_ap(&good), "does not establish arithmetic"), "{:#?}", run_ap(&good));
    }

    #[test]
    fn rely_is_a_pre_hypothesis_and_reported_conditional() {
        // Decision 2: a linear rely enters the preservation VC as a pre-state hypothesis, so an invariant
        // preserved only under it is not false-flagged; the verdict is reported conditional on the assumed
        // rely, and dropping the rely re-exposes the break.
        let run_ap = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            super::arith_preservation(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
        };
        let with = "-- allium: 4\ncomponent C\n  entity E\n  observable state x(E) : Number\n  observable state y(E) : Number\n  rely input_nonneg means every e :: y(e) >= 0\n  invariant x_nonneg means every e :: x(e) >= 0\n  action load\n    requires x(e) >= 0\n    ensures x(e) = y(e)\nend\n";
        let m = run_ap(with);
        assert!(!any(&m, "can break"), "the rely must be assumed as a pre-hypothesis: {m:#?}");
        assert!(any(&m, "conditional on assumed rely(s): input_nonneg"), "the assumed rely must be reported: {m:#?}");
        // Drop the rely -> the break is re-exposed (y unconstrained can be negative).
        let without = with.lines().filter(|l| !l.contains("rely input_nonneg")).collect::<Vec<_>>().join("\n");
        assert!(any(&run_ap(&without), "can break arithmetic invariant `x_nonneg`"), "without the rely the break must show: {:#?}", run_ap(&without));
        // An UNSATISFIABLE rely must be diagnosed and must NOT mask a real break (vacuity discipline).
        let contra = "-- allium: 4\ncomponent C\n  entity E\n  observable state x(E) : Number\n  rely bad means every e :: x(e) >= 0 and x(e) <= 0 - 1\n  invariant inv means every e :: x(e) >= 5\n  action drop\n    ensures x(e) = 0\nend\n";
        let cm = run_ap(contra);
        assert!(any(&cm, "rely `bad` in `C` is unsatisfiable"), "a contradictory rely must be diagnosed: {cm:#?}");
        assert!(any(&cm, "can break arithmetic invariant `inv`"), "a contradictory rely must not mask the real break: {cm:#?}");
    }

    #[test]
    fn computed_given_is_inlined_in_preservation() {
        let run_ap = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            super::arith_preservation(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
        };
        // `available = limit - used`; an unguarded spend can drive it negative -> break `solvent`.
        let bad = "-- allium: 4\ncomponent Credit\n  entity C\n  observable state limit(C) : Money\n  observable state used(C) : Money\n  given available(c) means limit(c) - used(c)\n  invariant solvent means available(c) >= 0\n  action spend\n    ensures used(c) = old(used(c)) + 1000000\nend\n";
        assert!(any(&run_ap(bad), "`spend` in `Credit` can break arithmetic invariant `solvent`"), "{:#?}", run_ap(bad));
        // A spend guarded by the (inlined) available bound keeps it non-negative.
        let good = "-- allium: 4\ncomponent Credit\n  entity C\n  observable state limit(C) : Money\n  observable state used(C) : Money\n  given available(c) means limit(c) - used(c)\n  invariant solvent means available(c) >= 0\n  action spend\n    requires available(c) >= 1\n    ensures used(c) = old(used(c)) + 1\nend\n";
        assert!(!any(&run_ap(good), "can break arithmetic invariant `solvent`"), "{:#?}", run_ap(good));
    }

    #[test]
    fn arith_break_names_the_weakest_guard() {
        let run_ap = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            super::arith_preservation(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
        };
        // `charge` subtracts an unconstrained input `fee`; the elicit value is naming the guard that
        // preserves `balance >= 0`, with `old` read as the pre-state (a guard is a precondition).
        let src = "-- allium: 4\ncomponent C\n  entity X\n  observable state balance(X) : Money\n  observable state fee(X) : Money\n  invariant nonneg means balance(x) >= 0\n  action charge\n    ensures balance(x) = old(balance(x)) - fee(x)\nend\n";
        assert!(any(&run_ap(src), "requires balance(e) - fee(e) >= 0"), "{:#?}", run_ap(src));
    }

    #[test]
    fn minmax_bound_preservation() {
        let run_ap = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            super::arith_preservation(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
        };
        let hdr = "-- allium: 4\ncomponent Pay\n  entity L\n  observable state due(L) : Money\n  observable state balance(L) : Money\n  observable state payment(L) : Money\n  invariant capped means payment(l) = min(due(l), balance(l))\n";
        // Overpaying past the cap breaks a derived min bound.
        let over = format!("{hdr}  action overpay\n    ensures payment(l) = due(l) + balance(l)\nend\n");
        assert!(any(&run_ap(&over), "can break arithmetic invariant `capped[bound"), "{:#?}", run_ap(&over));
        // With `due <= balance` stated, paying exactly the due respects both bounds.
        let safe = format!("-- allium: 4\ncomponent Pay\n  entity L\n  observable state due(L) : Money\n  observable state balance(L) : Money\n  observable state payment(L) : Money\n  invariant order means due(l) <= balance(l)\n  invariant capped means payment(l) = min(due(l), balance(l))\n  action pay_due\n    ensures payment(l) = due(l)\nend\n");
        assert!(!any(&run_ap(&safe), "can break arithmetic invariant `capped"), "{:#?}", run_ap(&safe));
        // abs derives a non-negativity bound: setting the magnitude negative breaks it.
        let absneg = "-- allium: 4\ncomponent A\n  entity X\n  observable state delta(X) : Money\n  observable state mag(X) : Money\n  invariant m means mag(x) = abs(delta(x))\n  action bad\n    ensures mag(x) = 0 - 1\nend\n";
        assert!(any(&run_ap(absneg), "can break arithmetic invariant `m[bound"), "{:#?}", run_ap(absneg));
    }

    #[test]
    fn conditional_arithmetic_invariant() {
        let egp = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            enum_guarded_preservation(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
        };
        // `payment = if late then base + 5 else base` expands to two guarded bounds; a charge that ignores
        // the late fee breaks fee_rule[then], the correct one does not.
        let hdr = "-- allium: 4\ncomponent Fee\n  entity L\n  observable state late(L) : bool\n  observable state base(L) : Money\n  observable state payment(L) : Money\n  invariant fee_rule means payment(l) = if late(l) then base(l) + 5 else base(l)\n";
        let wrong = format!("{hdr}  action mischarge\n    requires late(l)\n    ensures payment(l) = base(l)\nend\n");
        assert!(any(&egp(&wrong), "`mischarge` in `Fee` can break state-guarded invariant `fee_rule[then]`"), "{:#?}", egp(&wrong));
        let right = format!("{hdr}  action charge\n    requires late(l)\n    ensures payment(l) = base(l) + 5\nend\n");
        assert!(!any(&egp(&right), "can break state-guarded"), "{:#?}", egp(&right));
    }

    #[test]
    fn aggregate_conservation_preservation() {
        let agg = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            aggregate_preservation(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
        };
        let hdr = "-- allium: 4\ncomponent Bank\n  entity Acct\n  observable state balance(Acct) : Money\n  observable state total : Money\n  invariant conserved means total = sum p :: balance(p)\n";
        // A debit that shrinks a balance without adjusting total breaks conservation.
        let debit = format!("{hdr}  action debit\n    ensures balance(a) = old(balance(a)) - 1\nend\n");
        assert!(any(&agg(&debit), "`debit` in `Bank` can break conservation invariant `conserved`"), "{:#?}", agg(&debit));
        // Inflating total without a balance change also breaks it.
        let inflate = format!("{hdr}  action inflate\n    ensures total = old(total) + 100\nend\n");
        assert!(any(&agg(&inflate), "can break conservation invariant `conserved`"), "{:#?}", agg(&inflate));
        // Adjusting total to match the balance change preserves it.
        let deposit = format!("{hdr}  action deposit\n    ensures balance(a) = old(balance(a)) + 1 and total = old(total) + 1\nend\n");
        assert!(!any(&agg(&deposit), "can break conservation"), "{:#?}", agg(&deposit));
        // A balanced 2-entity transfer preserves conservation (deltas cancel); an unbalanced one breaks it.
        let transfer = format!("{hdr}  action transfer\n    ensures balance(a) = old(balance(a)) - 1 and balance(b) = old(balance(b)) + 1\nend\n");
        assert!(!any(&agg(&transfer), "can break conservation"), "balanced transfer: {:#?}", agg(&transfer));
        let skew = format!("{hdr}  action transfer\n    ensures balance(a) = old(balance(a)) - 1 and balance(b) = old(balance(b)) + 2\nend\n");
        assert!(any(&agg(&skew), "`transfer` in `Bank` can break conservation invariant `conserved`"), "unbalanced: {:#?}", agg(&skew));

        // A TWO-SUM accounting invariant `net = sum(asset) - sum(liab)`.
        let bhdr = "-- allium: 4\ncomponent Books\n  entity Acct\n  observable state asset(Acct) : Money\n  observable state liab(Acct) : Money\n  observable state net : Money\n  invariant balanced means net = (sum p :: asset(p)) - (sum p :: liab(p))\n";
        let growbad = format!("{bhdr}  action grow\n    ensures asset(a) = old(asset(a)) + 1\nend\n");
        assert!(any(&agg(&growbad), "`grow` in `Books` can break conservation invariant `balanced`"), "{:#?}", agg(&growbad));
        let growok = format!("{bhdr}  action grow\n    ensures asset(a) = old(asset(a)) + 1 and net = old(net) + 1\nend\n");
        assert!(!any(&agg(&growok), "can break conservation"), "{:#?}", agg(&growok));
    }

    #[test]
    fn relational_ordering_preservation() {
        let rel = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            super::relational_arith_preservation(&m, src).0.into_iter().map(|d| d.message).collect()
        };
        let ord = "invariant ordered means every a :: every b :: ver(a) > ver(b) implies off(a) >= off(b)\n";
        // Bumping one entity's version above another's without lifting its offset breaks the ordering.
        let brk = format!("-- allium: 4\ncomponent Log\n  entity E\n  observable state ver(E) : Number\n  observable state off(E) : Number\n  {ord}  action bump\n    ensures ver(e) = old(ver(e)) + 10\nend\n");
        assert!(rel(&brk).iter().any(|m| m.contains("can break relational invariant `ordered`")), "{:#?}", rel(&brk));
        // With `off = ver` as an invariant, a joint +1 bump keeps the ordering — the pass must USE that
        // hypothesis and stay clean (the soundness case the earlier reverted pass failed).
        let clean = format!("-- allium: 4\ncomponent Log\n  entity E\n  observable state ver(E) : Number\n  observable state off(E) : Number\n  invariant synced means every x :: off(x) = ver(x)\n  {ord}  action bump\n    ensures ver(e) = old(ver(e)) + 1 and off(e) = old(off(e)) + 1\nend\n");
        assert!(!rel(&clean).iter().any(|m| m.contains("can break")), "hypothesis must be used: {:#?}", rel(&clean));
        // An action on an unrelated state cannot disturb the ordering.
        let untouched = format!("-- allium: 4\ncomponent Log\n  entity E\n  observable state ver(E) : Number\n  observable state off(E) : Number\n  observable state wm(E) : Number\n  {ord}  action tick\n    ensures wm(e) = old(wm(e)) + 1\nend\n");
        assert!(!rel(&untouched).iter().any(|m| m.contains("can break")), "{:#?}", rel(&untouched));
    }

    #[test]
    fn enum_guarded_relational_preservation() {
        // #72: a two-entity, enum-guarded, arithmetic bound (the real achronic ShardWatermarkBound shape).
        // A single-subject action that races a healthy shard's watermark up must BREAK it; the offset
        // non-negativity hypothesis must clear a reset-to-zero; a corrupting action makes the guard false
        // post so the bound is vacuous and must NOT be flagged.
        let rel = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            super::relational_arith_preservation(&m, src).0.into_iter().map(|d| d.message).collect()
        };
        let hdr = "-- allium: 4\ncomponent Core\n  entity Shard\n  entity Part\n  observable state status(Shard) : { healthy | corrupted }\n  observable state wm(Shard) : Number\n  observable state off(Part) : Number\n  invariant offset_nonneg means every p :: off(p) >= 0\n  invariant bound means every s :: every p :: status(s) = healthy implies wm(s) <= off(p)\n";
        let brk = format!("{hdr}  action race\n    requires status(s) = healthy\n    ensures wm(s) = 1000000\nend\n");
        assert!(rel(&brk).iter().any(|m| m.contains("can break relational invariant `bound`")), "racing wm while healthy must break: {:#?}", rel(&brk));
        let safe = format!("{hdr}  action reset\n    requires status(s) = healthy\n    ensures wm(s) = 0\nend\n");
        assert!(!rel(&safe).iter().any(|m| m.contains("can break")), "offset_nonneg hypothesis must clear a reset to zero: {:#?}", rel(&safe));
        let vacuous = format!("{hdr}  action corrupt_and_race\n    requires status(s) = healthy\n    ensures status(s) = corrupted and wm(s) = 1000000\nend\n");
        assert!(!rel(&vacuous).iter().any(|m| m.contains("can break")), "guard false post -> bound vacuous, must not flag: {:#?}", rel(&vacuous));
        // FALSE-CERTIFICATION guard: `reset` engages+preserves `bound`, but `raise` raises wm without
        // constraining the status guard (undecidable post), so a real break from a healthy shard could be
        // missed — the target must NOT be certified checked even though `reset` engaged it.
        let mixed = format!("{hdr}  action reset\n    requires status(s) = healthy\n    ensures wm(s) = 0\n  action raise\n    ensures wm(s) = 1000000\nend\n");
        let m = parse(&mixed).module;
        let (_d, checked) = super::relational_arith_preservation(&m, &mixed);
        assert!(!checked.contains("bound"), "an undecidable-guard action must withhold certification of `bound`: {checked:?}");
    }

    #[test]
    fn conjunctive_antecedent_relational_preservation() {
        // #77: a relational invariant with a CONJUNCTIVE antecedent (achronic versions-ordered shape). The
        // VC now grounds the whole antecedent and disjunctively splits its negation, so an action that
        // inverts the ordering BREAKS it, and a hypothesis pinning the guard's second conjunct clears it.
        let rel = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            super::relational_arith_preservation(&m, src).0.into_iter().map(|d| d.message).collect()
        };
        let hdr = "-- allium: 4\ncomponent L\n  entity E\n  observable state ekey(E) : Number\n  observable state version(E) : Number\n  observable state off(E) : Number\n  invariant vo means every a :: every b :: ekey(a) = ekey(b) and version(a) > version(b) implies off(a) >= off(b)\n";
        let brk = format!("{hdr}  action lower_off\n    ensures off(e) = 0\nend\n");
        assert!(rel(&brk).iter().any(|m| m.contains("can break relational invariant `vo`")), "inverting the ordering must break the conjunctive-guard invariant: {:#?}", rel(&brk));
        // Pinning versions equal (frozen) makes `version(a) > version(b)` unsatisfiable — the guard never
        // fires, so the invariant is vacuously preserved and must NOT be flagged.
        let safe = format!("{hdr}  invariant vfrozen means version(e) = 0\n  action lower_off\n    ensures off(e) = 0\nend\n");
        assert!(!rel(&safe).iter().any(|m| m.contains("can break")), "a version-frozen hypothesis must clear the break: {:#?}", rel(&safe));
        // Adversarial: an action on an UNRELATED state must not false-positive against the conjunctive guard.
        let unrelated = format!("{hdr}  observable state other(E) : Number\n  action tick\n    ensures other(e) = old(other(e)) + 1\nend\n");
        assert!(!rel(&unrelated).iter().any(|m| m.contains("can break")), "an unrelated action must not break the conjunctive-guard invariant: {:#?}", rel(&unrelated));
    }

    #[test]
    fn conditional_summed_body_is_not_falsely_preserved() {
        let agg = |src: &str| -> Vec<String> {
            let m = parse(src).module;
            aggregate_preservation(&m, src, &super::Imports::default()).into_iter().map(|d| d.message).collect()
        };
        // `total = sum of ACTIVE balances`. A debit of `total` and `bal(a)` looks balanced only when `a`
        // is active; debiting an inactive account moves the total but not the sum. The pass must NOT claim
        // PRESERVED (the conditional delta is beyond the linear collapse) — a silent false certification.
        let hdr = "-- allium: 4\ncomponent C\n  entity Acct\n  observable state bal(Acct) : Money\n  observable state active(Acct) : Boolean\n  observable state total : Money\n  invariant conserved means total = sum a :: (if active(a) then bal(a) else 0)\n";
        let debit = format!("{hdr}  action debit\n    ensures bal(a) = old(bal(a)) - 100 and total = old(total) - 100\nend\n");
        assert!(!any(&agg(&debit), "is PRESERVED"), "conditional summed body must not earn a false PRESERVED: {:#?}", agg(&debit));
        // A non-conditional break in the same shape is still caught (the total moves, no balance changes).
        let skim = format!("{hdr}  action skim\n    ensures total = old(total) - 100\nend\n");
        assert!(any(&agg(&skim), "can break conservation invariant `conserved`"), "non-conditional break still caught: {:#?}", agg(&skim));
    }

    #[test]
    fn enum_guard_is_not_reported_as_unchecked_nonlinear() {
        // An enum-guarded arithmetic invariant (`outcome = success implies count >= 0`) is owned by the
        // enum tier. The arithmetic tier must not report the enum guard as a nonlinear unchecked term; a
        // genuinely nonlinear guard still is.
        let enumg = "-- allium: 4\ncomponent E\n  entity O\n  observable state outcome(O) : { success | failure }\n  observable state count(O) : Number\n  observable state floor(O) : Number\n  invariant a means outcome(o) = success implies count(o) >= 0\n  invariant b means count(o) >= floor(o)\nend\n";
        let m = run(enumg);
        assert!(!any(&m, "NOT CHECKED"), "enum guard must not be an unchecked nonlinear term: {m:#?}");
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
    fn rate_product_elicits_pin_the_rate_suggestion() {
        // A `Rate`-typed per-period observable times a state is the usual reason a schedule is only PARTIAL.
        // The elicit surface must name the rate and suggest pinning it — but only when it is a free rate,
        // never when it is already a concrete constant.
        let free = "-- allium: 4\ncomponent Loan\n  entity P\n  observable state rate(P) : Rate\n  observable state bal(P) : Money\n  observable state interest(P) : Money\n  invariant io means every p :: interest(p) = rate(p) * bal(p)\n  invariant nn means every p :: interest(p) >= 0\nend\n";
        let m = run(free);
        assert!(any(&m, "suggestion:") && any(&m, "`rate`") && any(&m, "given rate means"), "must suggest pinning the rate: {m:#?}");
        // A concrete rate is already linear — no suggestion, no PARTIAL.
        let concrete = "-- allium: 4\ncomponent Loan\n  entity P\n  given rate means 0.05\n  observable state bal(P) : Money\n  observable state interest(P) : Money\n  invariant io means every p :: interest(p) = rate * bal(p)\n  invariant nn means every p :: interest(p) >= 0\nend\n";
        let m2 = run(concrete);
        assert!(!any(&m2, "suggestion:"), "a concrete rate needs no suggestion: {m2:#?}");
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

    #[test]
    fn old_based_transition_invariant_is_not_swept_into_the_entailment_probe() {
        // `watermark >= old(watermark)` is a two-state monotonicity property. The single-state probe would
        // strip `old`, collapse it to a tautology, and call it redundant. It must be excluded instead
        // (arith_preservation checks it), so no ENTAILED/redundant verdict is emitted for it.
        let src = "-- allium: 4\ncomponent Ledger\n  entity Shard\n  observable state watermark(Shard) : Number where watermark(s) >= -1\n  observable state proposed(Shard) : Number\n  action advance\n    requires proposed(s) > watermark(s)\n    ensures watermark(s) = proposed(s)\n  invariant mono means watermark(s) >= old(watermark(s))\nend\n";
        let m = run(src);
        assert!(!any(&m, "`mono` in `Ledger` is ENTAILED"), "old-based invariant must not be called redundant: {m:#?}");
    }
}
