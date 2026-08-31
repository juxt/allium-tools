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
use crate::expr::{parse_predicate, BinOp, Expr, Quant, UnOp};
use crate::parser::ParseResult;

const MAX_ATOMS: usize = 16;

/// Parse + well-formedness + name resolution + case-split + rule-set consistency.
pub fn analyse(source: &str) -> ParseResult {
    let mut r = crate::check::check(source);
    r.diagnostics.append(&mut coverage(&r.module, source));
    r.diagnostics.append(&mut consistency(&r.module, source));
    r.diagnostics.append(&mut feasibility(&r.module, source));
    r.diagnostics.append(&mut preservation(&r.module, source));
    r.diagnostics.append(&mut bmc(&r.module, source));
    r.diagnostics.append(&mut crate::arith::arithmetic(&r.module, source));
    r.diagnostics.append(&mut crate::arith::reachability(&r.module, source));
    r.diagnostics.append(&mut crate::arith::arith_preservation(&r.module, source));
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

    // A k-induction safety PROOF supersedes the preservation pass's weaker 1-step "can break invariant Y"
    // note for the same invariant: the invariant is provably safe (just not 1-inductive), so the break was
    // a false alarm. Drop it. (Arithmetic breaks read "can break arithmetic invariant" and are untouched.)
    let proved_safe: std::collections::HashSet<String> = r
        .diagnostics
        .iter()
        .filter(|d| d.message.contains("is SAFE (proved by") && d.message.contains("-induction)"))
        .filter_map(|d| first_backtick(&d.message))
        .collect();
    if !proved_safe.is_empty() {
        r.diagnostics.retain(|d| {
            !proved_safe.iter().any(|y| d.message.contains(&format!("can break invariant `{y}`")))
        });
    }
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
        // Each invariant reduced to an entity-normalised boolean body (plain, or a single-variable
        // universal). Arithmetic, multi-entity, and existential invariants are skipped (sound).
        let invariants: Vec<(String, Expr)> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Invariant)
            .filter_map(|it| {
                let sp = it.body?;
                let inv = parse_predicate(sp.slice(src)).0;
                checkable_invariant(&inv, &bool_base, &all_obs)
                    .map(|e| (it.name.clone().unwrap_or_else(|| "<anon>".into()), e))
            })
            .collect();
        if invariants.is_empty() {
            continue;
        }

        // Base case of induction: does `init` establish each invariant? `init ∧ ¬I` satisfiable means
        // the initial state can already violate I. Only meaningful when init is itself boolean-fragment.
        let init_pred: Option<Expr> = d
            .items
            .iter()
            .find(|it| it.kind == ItemKind::Init)
            .and_then(|it| it.body)
            .map(|sp| {
                // The init body span keeps the `means` keyword; drop it before parsing the predicate.
                let text = sp.slice(src);
                let text = text.trim().strip_prefix("means").unwrap_or(text);
                parse_predicate(text).0
            })
            .filter(|e| boolean_fragment(e, &bool_base, &all_obs))
            .map(|e| {
                let mut ev = HashSet::new();
                collect_entity_vars(&e, &mut ev);
                rename_entity(&e, &ev)
            });
        // Per-invariant status: established by init, and not broken by any action.
        let mut established = vec![true; invariants.len()];
        let mut broken = vec![false; invariants.len()];
        if let Some(init) = &init_pred {
            for (i, (iname, inv)) in invariants.iter().enumerate() {
                let neg = Expr::Unary { op: UnOp::Not, e: Box::new(inv.clone()) };
                if let Some(m) = crate::sat::satisfiable(&[init, &neg], &bnames) {
                    established[i] = false;
                    let w: Vec<String> = m.iter().map(|(k, v)| format!("{k}={}", if *v { "T" } else { "F" })).collect();
                    out.push(Diagnostic::warning(
                        d.span,
                        format!("`init` in `{}` does not establish invariant `{iname}`: the initial state can violate it (e.g. {}).", d.name, pretty(&w.join(", "))),
                    ));
                }
            }
        }

        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let aname = it.name.clone().unwrap_or_else(|| "<anon>".into());
            let ensures_raw = match it.ensures {
                Some(sp) => parse_predicate(sp.slice(src)).0,
                None => continue,
            };
            let guard_raw = it.requires.map(|sp| parse_predicate(sp.slice(src)).0);
            // Normalise the action's entity variables to the same canonical entity as the invariants.
            // An action touching more than one distinct entity cannot be collapsed soundly, so skip it.
            let mut ev = HashSet::new();
            collect_entity_vars(&ensures_raw, &mut ev);
            if let Some(g) = &guard_raw {
                collect_entity_vars(g, &mut ev);
            }
            if ev.len() > 1 {
                continue;
            }
            let ensures = rename_entity(&ensures_raw, &ev);
            let guard = guard_raw.map(|g| rename_entity(&g, &ev));
            let mut modified = HashSet::new();
            collect_writes(&ensures, false, &state_names, &mut modified);
            if modified.is_empty() {
                continue; // writes no state: cannot break any invariant
            }
            let effect = prime(&ensures, &modified, false);
            for (i, (iname, inv)) in invariants.iter().enumerate() {
                if !mentions_any(inv, &modified) {
                    continue; // invariant untouched by this action
                }
                let inv_post = prime(inv, &modified, false);
                let violation = Expr::Unary { op: UnOp::Not, e: Box::new(inv_post) };
                // Pre-state: the WHOLE invariant set holds (prove the conjunction is inductive), not just
                // this one. This excludes bad pre-states another invariant already forbids, so a true-but-
                // not-inductive-alone invariant is not spuriously flagged. Sound: reporting a break means
                // the full set is genuinely not preserved.
                let mut es: Vec<&Expr> = vec![&effect, &violation];
                for (_, other) in &invariants {
                    es.push(other);
                }
                if let Some(g) = &guard {
                    es.push(g);
                }
                if let Some(m) = crate::sat::satisfiable(&es, &bnames) {
                    broken[i] = true;
                    let pre: Vec<String> = m
                        .iter()
                        .filter(|(k, _)| !k.contains('\'') && !k.starts_with("old "))
                        .map(|(k, v)| format!("{k}={}", if *v { "T" } else { "F" }))
                        .collect();
                    let fix = guard_suggestion(inv, &ensures, &modified);
                    out.push(Diagnostic::warning(
                        it.span,
                        pretty(&format!(
                            "action `{aname}` in `{}` can break invariant `{iname}`: from a state satisfying it (e.g. {}), the action reaches a state that violates it.{}",
                            d.name,
                            pre.join(", "),
                            fix
                        )),
                    ));
                }
            }
        }

        // A full inductive proof: established by init AND preserved by every action => holds in all
        // reachable states. Emit the positive result only when init is present to certify the base case.
        if init_pred.is_some() {
            for (i, (iname, _)) in invariants.iter().enumerate() {
                if established[i] && !broken[i] {
                    out.push(Diagnostic::warning(
                        d.span,
                        format!("invariant `{iname}` in `{}` is INDUCTIVE: established by `init` and preserved by every action, so it holds in every reachable state.", d.name),
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

/// A suggested guard: the weakest precondition, the invariant with each written observable replaced by
/// the value the action gives it, then simplified. `requires <that>` makes the action preserve the
/// invariant. Falls back to a generic hint when the effect is not a simple assignment we can invert.
fn guard_suggestion(inv: &Expr, ensures: &Expr, modified: &HashSet<String>) -> String {
    let mut post: HashMap<String, Expr> = HashMap::new();
    collect_post_values(ensures, modified, &mut post);
    if post.is_empty() {
        return " Add a guard (`requires …`) that rules out this pre-state.".to_string();
    }
    let wp = simplify(&substitute_by_canon(inv, &post));
    // A trivial wp (`true`) means the substitution lost the constraint; fall back rather than mislead.
    if matches!(&wp, Expr::Name(n) if n == "true") {
        return " Add a guard (`requires …`) that rules out this pre-state.".to_string();
    }
    format!(" To fix, guard it: `requires {}`.", canon(&wp))
}

/// From an `ensures`, the post value each written observable takes: `X` -> true, `not X` -> false,
/// `X = e` -> e. Walks top-level conjunctions; ignores conjuncts that are not simple assignments.
fn collect_post_values(e: &Expr, modified: &HashSet<String>, out: &mut HashMap<String, Expr>) {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            collect_post_values(lhs, modified, out);
            collect_post_values(rhs, modified, out);
        }
        Expr::Unary { op: UnOp::Not, e } => {
            if is_modified_app(e, modified) {
                out.insert(canon(e), Expr::Name("false".into()));
            }
        }
        Expr::Binary { op: BinOp::Eq, lhs, rhs } if is_modified_app(lhs, modified) => {
            out.insert(canon(lhs), (**rhs).clone());
        }
        _ if is_modified_app(e, modified) => {
            out.insert(canon(e), Expr::Name("true".into()));
        }
        _ => {}
    }
}

fn is_modified_app(e: &Expr, modified: &HashSet<String>) -> bool {
    match e {
        Expr::App { head, .. } => matches!(&**head, Expr::Name(h) if modified.contains(h)),
        Expr::Field { name, .. } => modified.contains(name),
        Expr::Name(n) => modified.contains(n),
        _ => false,
    }
}

/// Replace every sub-expression whose canonical form is a key in `post` with the mapped value.
fn substitute_by_canon(e: &Expr, post: &HashMap<String, Expr>) -> Expr {
    if let Some(v) = post.get(&canon(e)) {
        return v.clone();
    }
    match e {
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(substitute_by_canon(e, post)) },
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op: op.clone(),
            lhs: Box::new(substitute_by_canon(lhs, post)),
            rhs: Box::new(substitute_by_canon(rhs, post)),
        },
        Expr::App { head, args } => Expr::App {
            head: Box::new(substitute_by_canon(head, post)),
            args: args.iter().map(|a| substitute_by_canon(a, post)).collect(),
        },
        Expr::Cond { cond, then_, els } => Expr::Cond {
            cond: Box::new(substitute_by_canon(cond, post)),
            then_: Box::new(substitute_by_canon(then_, post)),
            els: Box::new(substitute_by_canon(els, post)),
        },
        other => other.clone(),
    }
}

/// Boolean simplification enough to make a substituted wp readable: fold `true`/`false` through the
/// connectives and drop double negation.
fn simplify(e: &Expr) -> Expr {
    let t = || Expr::Name("true".into());
    let f = || Expr::Name("false".into());
    let is_t = |x: &Expr| matches!(x, Expr::Name(n) if n == "true");
    let is_f = |x: &Expr| matches!(x, Expr::Name(n) if n == "false");
    match e {
        Expr::Unary { op: UnOp::Not, e } => {
            let s = simplify(e);
            if is_t(&s) { f() } else if is_f(&s) { t() } else { Expr::Unary { op: UnOp::Not, e: Box::new(s) } }
        }
        Expr::Binary { op, lhs, rhs } => {
            let l = simplify(lhs);
            let r = simplify(rhs);
            match op {
                BinOp::And => {
                    if is_f(&l) || is_f(&r) { f() } else if is_t(&l) { r } else if is_t(&r) { l } else { Expr::Binary { op: BinOp::And, lhs: Box::new(l), rhs: Box::new(r) } }
                }
                BinOp::Or => {
                    if is_t(&l) || is_t(&r) { t() } else if is_f(&l) { r } else if is_f(&r) { l } else { Expr::Binary { op: BinOp::Or, lhs: Box::new(l), rhs: Box::new(r) } }
                }
                BinOp::Implies => {
                    if is_f(&l) || is_t(&r) { t() } else if is_t(&l) { r } else { Expr::Binary { op: BinOp::Implies, lhs: Box::new(l), rhs: Box::new(r) } }
                }
                _ => Expr::Binary { op: op.clone(), lhs: Box::new(l), rhs: Box::new(r) },
            }
        }
        other => other.clone(),
    }
}

/// Present the canonical entity `_e` as a readable `e` in a diagnostic (cosmetic only; matching uses `_e`).
pub(crate) fn pretty(s: &str) -> String {
    s.replace(ENT, "e")
}

/// Longest counterexample trace bounded model checking will search for.
const BMC_MAX: usize = 6;

/// A boolean state literal from an `ensures`/`init`: the observable's representative application and its
/// target truth. Returns `false` if `e` is not a conjunction of bare/negated state applications (BMC then
/// declines the whole spec, soundly, rather than model a transition it cannot represent exactly).
fn as_literals(e: &Expr, state: &HashSet<String>, out: &mut Vec<(Expr, bool)>) -> bool {
    let lit = |x: &Expr| -> Option<String> {
        match x {
            Expr::App { head, .. } => match &**head {
                Expr::Name(h) if state.contains(h) => Some(h.clone()),
                _ => None,
            },
            Expr::Field { name, .. } if state.contains(name) => Some(name.clone()),
            Expr::Name(n) if state.contains(n) => Some(n.clone()),
            _ => None,
        }
    };
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => as_literals(lhs, state, out) && as_literals(rhs, state, out),
        Expr::Unary { op: UnOp::Not, e } if lit(e).is_some() => {
            out.push(((**e).clone(), false));
            true
        }
        _ if lit(e).is_some() => {
            out.push((e.clone(), true));
            true
        }
        _ => false,
    }
}

/// The state name at the head of an application/field/name atom.
fn atom_head(e: &Expr) -> Option<String> {
    match e {
        Expr::App { head, .. } => match &**head {
            Expr::Name(h) => Some(h.clone()),
            _ => None,
        },
        Expr::Field { name, .. } => Some(name.clone()),
        Expr::Name(n) => Some(n.clone()),
        _ => None,
    }
}

/// Entity-normalise every variable in `e` to the canonical entity `ENT`.
fn normalize(e: &Expr) -> Expr {
    let mut ev = HashSet::new();
    collect_entity_vars(e, &mut ev);
    rename_entity(e, &ev)
}

/// Bounded model checking: for each boolean safety invariant, search for a concrete execution from `init`
/// that reaches a state violating it, up to `BMC_MAX` steps, by iterative deepening (so the reported trace
/// is minimal). Where inductive preservation proves safety and reports a *possible* one-step break, BMC
/// answers the complementary question — is a violating state actually REACHABLE? — with a witness action
/// sequence. Sound within the bound: a reported trace is a genuine execution; silence means no violation
/// of length <= BMC_MAX (not a proof of safety, which is what preservation provides). Restricted to the
/// literal-conjunction state-machine fragment with a fully-pinned init; declines other specs cleanly.
pub fn bmc(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let state_names: HashSet<String> =
            d.items.iter().filter(|it| it.kind == ItemKind::State).filter_map(|it| it.name.clone()).collect();
        let all_obs: HashSet<String> =
            d.items.iter().filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given)).filter_map(|it| it.name.clone()).collect();
        let bool_base = bool_names_of(d, src);
        if state_names.is_empty() || state_names.iter().any(|s| !bool_base.contains(s)) {
            continue; // BMC models boolean state machines; a non-boolean state is out of this fragment
        }

        // init, as a full assignment of every state literal (must pin all states, else skip).
        let init_item = match d.items.iter().find(|it| it.kind == ItemKind::Init).and_then(|it| it.body) {
            Some(sp) => {
                let t = sp.slice(src);
                parse_predicate(t.trim().strip_prefix("means").unwrap_or(t)).0
            }
            None => continue,
        };
        let mut init_lits = Vec::new();
        if !as_literals(&normalize(&init_item), &state_names, &mut init_lits) {
            continue;
        }
        if init_lits.len() < state_names.len() {
            continue; // init leaves a state free: reachability would be an over-approximation
        }

        struct Act {
            name: String,
            guard: Option<Expr>,
            writes: Vec<(Expr, bool)>,
            modified: HashSet<String>,
        }
        let mut acts: Vec<Act> = Vec::new();
        let mut literal_ok = true;
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let ens = match it.ensures {
                Some(sp) => normalize(&parse_predicate(sp.slice(src)).0),
                None => continue,
            };
            let mut writes = Vec::new();
            if !as_literals(&ens, &state_names, &mut writes) {
                literal_ok = false;
                break;
            }
            let modified: HashSet<String> = writes.iter().filter_map(|(a, _)| atom_head(a)).collect();
            let guard = it.requires.map(|sp| normalize(&parse_predicate(sp.slice(src)).0));
            acts.push(Act { name: it.name.clone().unwrap_or_else(|| "<anon>".into()), guard, writes, modified });
        }
        if !literal_ok || acts.is_empty() {
            continue;
        }

        // Representative atom per state name (for framing and violation terms).
        let mut atoms: HashMap<String, Expr> = HashMap::new();
        for (a, _) in init_lits.iter().chain(acts.iter().flat_map(|a| a.writes.iter())) {
            if let Some(h) = atom_head(a) {
                atoms.entry(h).or_insert_with(|| a.clone());
            }
        }

        let invs: Vec<(String, Expr)> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Invariant)
            .filter_map(|it| {
                let sp = it.body?;
                checkable_invariant(&parse_predicate(sp.slice(src)).0, &bool_base, &all_obs)
                    .map(|e| (it.name.clone().unwrap_or_else(|| "<anon>".into()), e))
            })
            .collect();
        // Note: no early return on empty `invs` — the invariant loop below is then a no-op, but
        // dead-action detection still runs (it needs only the actions, not the invariants).

        // Boolean names for every stamped atom, so `=` frames encode as biconditionals.
        let step_name = |n: &str, t: usize| format!("{n}@{t}");
        let mut bnames = HashSet::new();
        for s in &state_names {
            for t in 0..=BMC_MAX {
                bnames.insert(step_name(s, t));
            }
        }
        let stamp = |e: &Expr, t: usize| rename_states(e, &|n: &str| state_names.contains(n).then(|| step_name(n, t)));

        // One step of the transition relation at time `t`: exactly one action fires, its guard holds at
        // t, its literal effects hold at t+1, and the frame equates every unmodified state across the step.
        let mk_trans = |t: usize| -> Vec<Expr> {
            let mut v = Vec::new();
            let fire = |i: usize| Expr::Name(format!("fire@{t}#{i}"));
            let mut some = fire(0);
            for i in 1..acts.len() {
                some = Expr::Binary { op: BinOp::Or, lhs: Box::new(some), rhs: Box::new(fire(i)) };
            }
            v.push(some);
            for i in 0..acts.len() {
                for j in (i + 1)..acts.len() {
                    v.push(Expr::Unary { op: UnOp::Not, e: Box::new(Expr::Binary { op: BinOp::And, lhs: Box::new(fire(i)), rhs: Box::new(fire(j)) }) });
                }
            }
            for (i, act) in acts.iter().enumerate() {
                let imp = |body: Expr| Expr::Binary { op: BinOp::Implies, lhs: Box::new(fire(i)), rhs: Box::new(body) };
                if let Some(g) = &act.guard {
                    v.push(imp(stamp(g, t)));
                }
                for (a, pol) in &act.writes {
                    let at = stamp(a, t + 1);
                    v.push(imp(if *pol { at } else { Expr::Unary { op: UnOp::Not, e: Box::new(at) } }));
                }
                for (name, atom) in &atoms {
                    if !act.modified.contains(name) {
                        v.push(imp(Expr::Binary { op: BinOp::Eq, lhs: Box::new(stamp(atom, t + 1)), rhs: Box::new(stamp(atom, t)) }));
                    }
                }
            }
            v
        };

        for (iname, inv) in &invs {
            // (1) BMC: search for a concrete reachable counterexample, shortest first.
            let mut counterexample = false;
            'depth: for k in 1..=BMC_MAX {
                let mut cx: Vec<Expr> = Vec::new();
                for (a, pol) in &init_lits {
                    let at = stamp(a, 0);
                    cx.push(if *pol { at } else { Expr::Unary { op: UnOp::Not, e: Box::new(at) } });
                }
                for t in 0..k {
                    cx.extend(mk_trans(t));
                }
                cx.push(Expr::Unary { op: UnOp::Not, e: Box::new(stamp(inv, k)) });
                let refs: Vec<&Expr> = cx.iter().collect();
                if let Some(m) = crate::sat::satisfiable(&refs, &bnames) {
                    let mut trace = Vec::new();
                    for t in 0..k {
                        for (i, act) in acts.iter().enumerate() {
                            if *m.get(&format!("fire@{t}#{i}")).unwrap_or(&false) {
                                trace.push(act.name.clone());
                            }
                        }
                    }
                    out.push(Diagnostic::warning(
                        d.span,
                        format!(
                            "invariant `{iname}` in `{}` is REACHABLY VIOLATED in {k} step(s): init -> {} -> a state where it fails. A concrete counterexample, not just a non-inductive warning.",
                            d.name,
                            trace.join(" -> ")
                        ),
                    ));
                    counterexample = true;
                    break 'depth;
                }
            }
            if counterexample {
                continue;
            }
            // (2) k-INDUCTION: no bounded counterexample, so try to PROVE the invariant safe unboundedly.
            // Step case at length kk: no path of kk transitions where the invariant holds in the first kk
            // states but fails at the (kk+1)-th. With the base case (BMC found no violation up to BMC_MAX
            // >= kk), UNSAT of the step case proves the invariant holds in every reachable state. kk=1 is
            // ordinary 1-induction, which the preservation pass already reports as INDUCTIVE, so only the
            // stronger kk>=2 proof is announced here (and it supersedes preservation's 1-step break note).
            for kk in 1..=BMC_MAX {
                let mut step: Vec<Expr> = Vec::new();
                for t in 0..kk {
                    step.extend(mk_trans(t));
                }
                for j in 0..kk {
                    step.push(stamp(inv, j));
                }
                step.push(Expr::Unary { op: UnOp::Not, e: Box::new(stamp(inv, kk)) });
                let refs: Vec<&Expr> = step.iter().collect();
                if crate::sat::satisfiable(&refs, &bnames).is_none() {
                    if kk >= 2 {
                        out.push(Diagnostic::warning(
                            d.span,
                            format!("invariant `{iname}` in `{}` is SAFE (proved by {kk}-induction): no reachable state violates it, though it is not 1-inductive. It holds in every reachable state.", d.name),
                        ));
                    }
                    break;
                }
            }
        }

        // Dead-action detection: an action whose guard holds in no state reachable within BMC_MAX steps
        // can never fire — dead spec code. For each prefix length t, ask whether the guard is satisfiable
        // at step t of a t-step execution from init; if some length works the action is live. Checking each
        // length separately avoids forcing the machine to keep stepping past a terminal state (which would
        // spuriously make the query UNSAT). A guardless action is always enabled and is skipped.
        for act in acts.iter().filter(|a| a.guard.is_some()) {
            let g = act.guard.as_ref().unwrap();
            let mut live = false;
            for t in 0..=BMC_MAX {
                let mut cx: Vec<Expr> = Vec::new();
                for (a, pol) in &init_lits {
                    let at = stamp(a, 0);
                    cx.push(if *pol { at } else { Expr::Unary { op: UnOp::Not, e: Box::new(at) } });
                }
                for s in 0..t {
                    cx.extend(mk_trans(s));
                }
                cx.push(stamp(g, t));
                let refs: Vec<&Expr> = cx.iter().collect();
                if crate::sat::satisfiable(&refs, &bnames).is_some() {
                    live = true;
                    break;
                }
            }
            if !live {
                out.push(Diagnostic::warning(
                    d.span,
                    format!("action `{}` in `{}` is never enabled in any reachable state (within {BMC_MAX} steps): its guard is never satisfied, so it can never fire — dead code, or a guard that contradicts the reachable states.", act.name, d.name),
                ));
            }
        }
    }
    out
}

/// A canonical single entity: all entity variables are normalised to this so an invariant written over
/// `p` and an action written over `t` line up (an action touches one entity, so the interesting instance
/// of a universal invariant is that entity). Underscore-led so it cannot clash with a real spec name.
const ENT: &str = "_e";

/// Collect entity-variable names: quantifier-bound variables and bare-name arguments of applications.
pub(crate) fn collect_entity_vars(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Quant { vars, body, .. } | Expr::Sum { vars, body, .. } => {
            out.extend(vars.iter().cloned());
            collect_entity_vars(body, out);
        }
        Expr::App { head, args } => {
            for a in args {
                if let Expr::Name(n) = a {
                    out.insert(n.clone());
                } else {
                    collect_entity_vars(a, out);
                }
            }
            collect_entity_vars(head, out);
        }
        Expr::Field { base, .. } => collect_entity_vars(base, out),
        Expr::Unary { e, .. } => collect_entity_vars(e, out),
        Expr::Binary { lhs, rhs, .. } => {
            collect_entity_vars(lhs, out);
            collect_entity_vars(rhs, out);
        }
        Expr::Cond { cond, then_, els } => {
            collect_entity_vars(cond, out);
            collect_entity_vars(then_, out);
            collect_entity_vars(els, out);
        }
        _ => {}
    }
}

/// Rename every name in `vars` to the canonical entity `ENT`.
pub(crate) fn rename_entity(e: &Expr, vars: &HashSet<String>) -> Expr {
    match e {
        Expr::Name(n) if vars.contains(n) => Expr::Name(ENT.into()),
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(rename_entity(e, vars)) },
        Expr::Binary { op, lhs, rhs } => Expr::Binary { op: op.clone(), lhs: Box::new(rename_entity(lhs, vars)), rhs: Box::new(rename_entity(rhs, vars)) },
        Expr::App { head, args } => Expr::App { head: Box::new(rename_entity(head, vars)), args: args.iter().map(|a| rename_entity(a, vars)).collect() },
        Expr::Field { base, name } => Expr::Field { base: Box::new(rename_entity(base, vars)), name: name.clone() },
        Expr::Cond { cond, then_, els } => Expr::Cond { cond: Box::new(rename_entity(cond, vars)), then_: Box::new(rename_entity(then_, vars)), els: Box::new(rename_entity(els, vars)) },
        other => other.clone(),
    }
}

/// The boolean body a preservation check should run for this invariant, entity-normalised to `ENT`, or
/// `None` if out of scope: a plain (quantifier-free) boolean invariant, or a single-variable `every`/`no`
/// over a boolean body (a universal safety property). Multi-entity, `some`/`exists`, nested-quantifier,
/// and arithmetic invariants are skipped (sound: the check simply says nothing about them).
fn checkable_invariant(inv: &Expr, bool_base: &HashSet<String>, obs: &HashSet<String>) -> Option<Expr> {
    let body = match inv {
        Expr::Quant { q, vars, body, .. } if vars.len() == 1 && !has_quant(body) => match q {
            Quant::Every => (**body).clone(),
            Quant::No => Expr::Unary { op: UnOp::Not, e: body.clone() },
            _ => return None,
        },
        _ if !has_quant(inv) => inv.clone(),
        _ => return None,
    };
    let mut ev = HashSet::new();
    collect_entity_vars(&body, &mut ev);
    if ev.len() > 1 {
        return None; // relates distinct entities; cannot collapse to one symbolic entity
    }
    if !boolean_fragment(&body, bool_base, obs) {
        return None;
    }
    Some(rename_entity(&body, &ev))
}

/// True if `e` contains an explicit quantifier or aggregate (deferred by the preservation check).
pub(crate) fn has_quant(e: &Expr) -> bool {
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

/// Collect the state observables an `ensures` WRITES, understanding assignment form. A conjunct is a
/// write of `X` when it is a bare state app `X` (sets it true), `not X` (false), or an equation `X = e`
/// (X is the target; the right-hand side is a READ, not a write). This distinction matters for arithmetic
/// effects like `balance(a) = old(balance(a)) - amount(a)`, where `amount` on the RHS is read, not written.
/// The `in_old` parameter is retained for signature compatibility and ignored (writes are top-level).
pub(crate) fn collect_writes(e: &Expr, _in_old: bool, state: &HashSet<String>, out: &mut HashSet<String>) {
    let head_name = |x: &Expr| -> Option<String> {
        match x {
            Expr::App { head, .. } => match &**head {
                Expr::Name(h) if state.contains(h) => Some(h.clone()),
                _ => None,
            },
            Expr::Field { name, .. } if state.contains(name) => Some(name.clone()),
            Expr::Name(n) if state.contains(n) => Some(n.clone()),
            _ => None,
        }
    };
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            collect_writes(lhs, false, state, out);
            collect_writes(rhs, false, state, out);
        }
        Expr::Unary { op: UnOp::Not, e } => {
            if let Some(n) = head_name(e) {
                out.insert(n);
            }
        }
        Expr::Binary { op: BinOp::Eq, lhs, .. } => {
            if let Some(n) = head_name(lhs) {
                out.insert(n);
            }
        }
        _ => {
            if let Some(n) = head_name(e) {
                out.insert(n);
            }
        }
    }
}

/// Rename the head of every state observable via `f` (applied to the state name; `None` leaves it as is).
/// Renames application heads `X(..)`, bare names `X`, and record fields `.X`. Used to stamp a step index
/// onto every state atom for bounded model checking (`X` -> `X@t`).
fn rename_states(e: &Expr, f: &impl Fn(&str) -> Option<String>) -> Expr {
    match e {
        Expr::App { head, args } => {
            let head = match &**head {
                Expr::Name(h) => Box::new(Expr::Name(f(h).unwrap_or_else(|| h.clone()))),
                other => Box::new(rename_states(other, f)),
            };
            Expr::App { head, args: args.iter().map(|a| rename_states(a, f)).collect() }
        }
        Expr::Field { base, name } => Expr::Field { base: Box::new(rename_states(base, f)), name: f(name).unwrap_or_else(|| name.clone()) },
        Expr::Name(n) => Expr::Name(f(n).unwrap_or_else(|| n.clone())),
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(rename_states(e, f)) },
        Expr::Binary { op, lhs, rhs } => Expr::Binary { op: op.clone(), lhs: Box::new(rename_states(lhs, f)), rhs: Box::new(rename_states(rhs, f)) },
        Expr::Cond { cond, then_, els } => Expr::Cond { cond: Box::new(rename_states(cond, f)), then_: Box::new(rename_states(then_, f)), els: Box::new(rename_states(els, f)) },
        other => other.clone(),
    }
}

/// Rewrite `e` to its post-state reading: an observable in `modified`, appearing outside `old`, is
/// primed (`X` -> `X'`); `old(X)` is stripped to the pre reading `X`; everything else is unchanged.
pub(crate) fn prime(e: &Expr, modified: &HashSet<String>, in_old: bool) -> Expr {
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
pub(crate) fn mentions_any(e: &Expr, names: &HashSet<String>) -> bool {
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
    fn preservation_suggests_the_weakest_guard() {
        let bad = "-- allium: 4\ncomponent Pay\n  entity Txn\n  observable state authed(Txn) : bool\n  observable state captured(Txn) : bool\n  action capture\n    ensures captured(t)\n  invariant no_cap_without_auth means captured(t) implies authed(t)\nend\n";
        assert!(any(bad, "requires authed(e)"), "should suggest the weakest guard: {:?}", msgs(bad));
    }

    #[test]
    fn init_and_preservation_prove_inductive() {
        // init establishes the invariant AND the guarded action preserves it -> a full inductive proof.
        let src = "-- allium: 4\ncomponent Pay\n  entity Txn\n  observable state authed(Txn) : bool\n  observable state captured(Txn) : bool\n  init means not authed(t) and not captured(t)\n  action capture\n    requires authed(t)\n    ensures captured(t)\n  invariant no_cap means captured(t) implies authed(t)\nend\n";
        assert!(any(src, "is INDUCTIVE"), "{:?}", msgs(src));
        assert!(!any(src, "can break"), "{:?}", msgs(src));
        assert!(!any(src, "does not establish"), "{:?}", msgs(src));
    }

    #[test]
    fn preservation_handles_quantified_invariant_with_mismatched_var() {
        // `every p :: ...` invariant (var p) against an action written over t: normalisation unifies them.
        let bad = "-- allium: 4\ncomponent Pay\n  entity Txn\n  observable state authed(Txn) : bool\n  observable state captured(Txn) : bool\n  action capture\n    ensures captured(t)\n  invariant no_cap means every p :: captured(p) implies authed(p)\nend\n";
        assert!(any(bad, "can break invariant `no_cap`"), "{:?}", msgs(bad));
        let good = "-- allium: 4\ncomponent Pay\n  entity Txn\n  observable state authed(Txn) : bool\n  observable state captured(Txn) : bool\n  init means not authed(t) and not captured(t)\n  action capture\n    requires authed(t)\n    ensures captured(t)\n  invariant no_cap means every p :: captured(p) implies authed(p)\nend\n";
        assert!(any(good, "is INDUCTIVE"), "{:?}", msgs(good));
        assert!(!any(good, "can break"), "{:?}", msgs(good));
    }

    #[test]
    fn preservation_uses_the_invariant_conjunction() {
        // `ship_needs_paid` (shipped => paid) is NOT inductive alone: `ship` sets shipped from a paid=F
        // state. But `always_paid` forbids paid=F, so together they are inductive. No break must be
        // reported, and both must be certified inductive.
        let src = "-- allium: 4\ncomponent Order\n  entity O\n  observable state paid(O) : bool\n  observable state shipped(O) : bool\n  init means paid(o) and not shipped(o)\n  action ship\n    ensures shipped(o)\n  invariant always_paid means paid(o)\n  invariant ship_needs_paid means shipped(o) implies paid(o)\nend\n";
        assert!(!any(src, "can break"), "conjunction should exclude the bad pre-state: {:?}", msgs(src));
        assert!(any(src, "`ship_needs_paid` in `Order` is INDUCTIVE"), "{:?}", msgs(src));
    }

    #[test]
    fn bmc_finds_a_minimal_reachable_counterexample_trace() {
        // capture with no auth guard: BMC reaches the violation in one step (init -> capture).
        let bad = "-- allium: 4\ncomponent Pay\n  entity P\n  observable state authed(P) : bool\n  observable state captured(P) : bool\n  init means not authed(p) and not captured(p)\n  action authorize\n    requires not authed(p)\n    ensures authed(p)\n  action capture\n    ensures captured(p)\n  invariant no_cap means captured(p) implies authed(p)\nend\n";
        assert!(any(bad, "REACHABLY VIOLATED"), "{:?}", msgs(bad));
        assert!(any(bad, "1 step"), "should be a one-step trace: {:?}", msgs(bad));
    }

    #[test]
    fn k_induction_proves_a_non_one_inductive_but_safe_invariant() {
        // `r implies p` is safe but not 1-inductive: `setr` breaks it from p=F,q=T, which is unreachable
        // (q is only set by `advance`, which requires p). No helper invariant is declared, so conjunction-
        // strengthening cannot prove it — only 2-induction (one step back forces p=T). The 1-step break
        // must be suppressed and replaced by the k-induction safety proof; no reachable counterexample.
        let src = "-- allium: 4\ncomponent Staged\n  entity S\n  observable state p(S) : bool\n  observable state q(S) : bool\n  observable state r(S) : bool\n  init means not p(s) and not q(s) and not r(s)\n  action start\n    requires not p(s)\n    ensures p(s)\n  action advance\n    requires p(s) and not q(s)\n    ensures q(s)\n  action setr\n    requires q(s) and not r(s)\n    ensures r(s)\n  invariant r_implies_p means r(s) implies p(s)\nend\n";
        assert!(any(src, "SAFE (proved by 2-induction)"), "{:?}", msgs(src));
        assert!(!any(src, "can break invariant `r_implies_p`"), "the 1-step break must be superseded: {:?}", msgs(src));
        assert!(!any(src, "REACHABLY VIOLATED"), "no reachable counterexample exists: {:?}", msgs(src));
    }

    #[test]
    fn dead_action_detection() {
        // `deadact` requires `a and b`, but `b` is only ever set by `deadact` itself — so `b` is never
        // reachably true and the guard can never hold. It is dead code, and a live action is not flagged.
        let src = "-- allium: 4\ncomponent M\n  entity X\n  observable state a(X) : bool\n  observable state b(X) : bool\n  init means not a(x) and not b(x)\n  action seta\n    requires not a(x)\n    ensures a(x)\n  action deadact\n    requires a(x) and b(x)\n    ensures b(x)\nend\n";
        assert!(any(src, "`deadact` in `M` is never enabled"), "{:?}", msgs(src));
        assert!(!any(src, "`seta` in `M` is never enabled"), "live action must not be flagged: {:?}", msgs(src));
    }

    #[test]
    fn bmc_is_silent_when_no_violation_is_reachable() {
        // With the guard, the violating state is unreachable: BMC must find no counterexample.
        let good = "-- allium: 4\ncomponent Pay\n  entity P\n  observable state authed(P) : bool\n  observable state captured(P) : bool\n  init means not authed(p) and not captured(p)\n  action authorize\n    requires not authed(p)\n    ensures authed(p)\n  action capture\n    requires authed(p)\n    ensures captured(p)\n  invariant no_cap means captured(p) implies authed(p)\nend\n";
        assert!(!any(good, "REACHABLY VIOLATED"), "no counterexample should exist: {:?}", msgs(good));
    }

    #[test]
    fn init_that_violates_invariant_is_flagged() {
        // init leaves captured true but authed false: it already violates captured => authed.
        let src = "-- allium: 4\ncomponent Pay\n  entity Txn\n  observable state authed(Txn) : bool\n  observable state captured(Txn) : bool\n  init means captured(t) and not authed(t)\n  invariant no_cap means captured(t) implies authed(t)\nend\n";
        assert!(any(src, "`init` in `Pay` does not establish invariant `no_cap`"), "{:?}", msgs(src));
    }

    #[test]
    fn arithmetic_preservation_catches_and_clears_via_lra() {
        // The LRA tier catches value-safety: unguarded withdraw can drive balance below zero.
        let bad = "-- allium: 4\ncomponent Bank\n  entity A\n  observable state bal(A) : Money\n  observable state amt(A) : Money\n  action withdraw\n    ensures bal(a) = old(bal(a)) - amt(a)\n  invariant non_negative means bal(a) >= 0\nend\n";
        assert!(any(bad, "can break arithmetic invariant `non_negative`"), "{:?}", msgs(bad));
        // Guarding it (requires amt <= bal) makes it safe — the simplex proves the post stays >= 0.
        let good = "-- allium: 4\ncomponent Bank\n  entity A\n  observable state bal(A) : Money\n  observable state amt(A) : Money\n  action withdraw\n    requires amt(a) <= bal(a)\n    ensures bal(a) = old(bal(a)) - amt(a)\n  invariant non_negative means bal(a) >= 0\nend\n";
        assert!(!any(good, "can break"), "{:?}", msgs(good));
    }

    #[test]
    fn boolean_preservation_ignores_arithmetic_invariants() {
        // The BOOLEAN preservation pass must stay silent on an arithmetic invariant (no opaque-atom
        // false alarm); the LRA pass owns it. Assert no *boolean* break message is emitted.
        let src = "-- allium: 4\ncomponent Bank\n  entity A\n  observable state bal(A) : Money\n  observable state amt(A) : Money\n  action withdraw\n    requires amt(a) <= bal(a)\n    ensures bal(a) = old(bal(a)) - amt(a)\n  invariant non_negative means bal(a) >= 0\nend\n";
        assert!(!any(src, "can break invariant"), "boolean pass should be silent: {:?}", msgs(src));
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
