//! Runtime monitor derived from a v4 spec's invariants. The SAME `invariant` items the
//! design-time checks verify are evaluated here over a concrete execution trace: this is
//! the runtime modality of one artifact (design-time verify + build + monitor from one
//! spec). A point invariant is a per-state safety check; a past-temporal invariant using
//! `old(...)` is evaluated against the entity's previous trace state, so monotonicity and
//! non-regression properties ("once accepted, never rejected") are monitorable — the sort
//! of "fault meaning" a hand-written assert does not carry and cannot keep in step with
//! the design.
//!
//! Trace format (dependency-free, one event per line):
//!   `t=<int> entity=<id> <pred>=T <pred>=F ...`
//! where each `<pred>` is a boolean observable-state predicate name. Events for the same
//! entity are ordered; `old(p(x))` reads that entity's previous event's value of `p`.

use std::collections::HashMap;

use crate::analyse::canon;
use crate::ast::ItemKind;
use crate::expr::{BinOp, Expr, UnOp};

/// Strip a predicate application to its name: `cleared(r)` -> `cleared`.
fn pred_name(e: &Expr) -> String {
    let c = canon(e);
    match c.find('(') {
        Some(i) => c[..i].trim().to_string(),
        None => c,
    }
}

/// Does the invariant reference a previous state (`old`)? Then it is temporal.
fn uses_old(e: &Expr) -> bool {
    match e {
        Expr::Unary { op: UnOp::Old, .. } => true,
        Expr::Unary { e, .. } => uses_old(e),
        Expr::Binary { lhs, rhs, .. } => uses_old(lhs) || uses_old(rhs),
        _ => false,
    }
}

/// Evaluate a boolean expression in a single state (no temporal operators inside).
fn eval_state(e: &Expr, s: &HashMap<String, bool>) -> bool {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => eval_state(lhs, s) && eval_state(rhs, s),
        Expr::Binary { op: BinOp::Or, lhs, rhs } => eval_state(lhs, s) || eval_state(rhs, s),
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => !eval_state(lhs, s) || eval_state(rhs, s),
        Expr::Unary { op: UnOp::Not, e } => !eval_state(e, s),
        other => *s.get(&pred_name(other)).unwrap_or(&false),
    }
}

/// Evaluate with history: `old(x)` reads `prev`, everything else reads `cur`.
fn eval_temporal(e: &Expr, cur: &HashMap<String, bool>, prev: &HashMap<String, bool>) -> bool {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => eval_temporal(lhs, cur, prev) && eval_temporal(rhs, cur, prev),
        Expr::Binary { op: BinOp::Or, lhs, rhs } => eval_temporal(lhs, cur, prev) || eval_temporal(rhs, cur, prev),
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => !eval_temporal(lhs, cur, prev) || eval_temporal(rhs, cur, prev),
        Expr::Unary { op: UnOp::Not, e } => !eval_temporal(e, cur, prev),
        Expr::Unary { op: UnOp::Old, e } => eval_state(e, prev),
        other => *cur.get(&pred_name(other)).unwrap_or(&false),
    }
}

struct Event {
    t: String,
    entity: String,
    vals: HashMap<String, bool>,
}

fn parse_trace(trace: &str) -> Vec<Event> {
    let mut events = Vec::new();
    for line in trace.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut t = String::new();
        let mut entity = String::new();
        let mut vals = HashMap::new();
        for tok in line.split_whitespace() {
            let Some((k, v)) = tok.split_once('=') else { continue };
            match k {
                "t" => t = v.to_string(),
                "entity" => entity = v.to_string(),
                _ => {
                    vals.insert(k.to_string(), v.eq_ignore_ascii_case("t") || v == "1" || v.eq_ignore_ascii_case("true"));
                }
            }
        }
        events.push(Event { t, entity, vals });
    }
    events
}

/// Run the monitor. Returns a JSON report `{events, violations:[...], ok}`.
pub fn monitor(source: &str, trace: &str) -> String {
    let module = crate::check::check(source).module;
    let invs: Vec<(String, Expr, bool)> = module
        .decls
        .iter()
        .flat_map(|d| d.items.iter())
        .filter(|it| it.kind == ItemKind::Invariant)
        .filter_map(|it| {
            it.body.map(|sp| {
                let e = crate::expr::parse_predicate(sp.slice(source)).0;
                let temporal = uses_old(&e);
                (it.name.clone().unwrap_or_else(|| "<anon>".into()), e, temporal)
            })
        })
        .collect();

    let events = parse_trace(trace);
    let mut prev: HashMap<String, HashMap<String, bool>> = HashMap::new();
    let mut violations: Vec<String> = Vec::new();

    for ev in &events {
        let p = prev.get(&ev.entity);
        for (name, expr, temporal) in &invs {
            let holds = if *temporal {
                match p {
                    Some(prev_state) => eval_temporal(expr, &ev.vals, prev_state),
                    None => true, // no history yet; the temporal property starts at the 2nd event
                }
            } else {
                eval_state(expr, &ev.vals)
            };
            if !holds {
                let kind = if *temporal { "temporal" } else { "point" };
                violations.push(format!(
                    "{{\"t\":\"{}\",\"entity\":\"{}\",\"invariant\":\"{}\",\"kind\":\"{}\",\"state\":\"{}\"}}",
                    esc(&ev.t), esc(&ev.entity), esc(name), kind, esc(&show_state(&ev.vals))
                ));
            }
        }
        prev.insert(ev.entity.clone(), ev.vals.clone());
    }

    format!(
        "{{\"events\":{},\"violations\":[{}],\"ok\":{}}}",
        events.len(),
        violations.join(","),
        violations.is_empty()
    )
}

fn show_state(s: &HashMap<String, bool>) -> String {
    let mut kv: Vec<(&String, &bool)> = s.iter().collect();
    kv.sort();
    kv.iter().map(|(k, v)| format!("{k}={}", if **v { "T" } else { "F" })).collect::<Vec<_>>().join(" ")
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = "-- allium: 4\ncomponent M\n  entity R\n  observable state collateralised(R) : bool\n  observable state has_code(R) : bool\n  observable state accepted(R) : bool\n  observable state rejected(R) : bool\n  invariant collat_needs_code means collateralised(r) implies has_code(r)\n  invariant once_accepted means old(accepted(r)) implies not rejected(r)\nend\n";

    fn count(report: &str, needle: &str) -> usize {
        report.matches(needle).count()
    }

    #[test]
    fn catches_point_violation() {
        let trace = "t=1 entity=R1 collateralised=T has_code=F accepted=F rejected=F\n";
        let r = monitor(SPEC, trace);
        assert!(count(&r, "collat_needs_code") == 1, "{r}");
        assert!(r.contains("\"ok\":false"));
    }

    #[test]
    fn catches_temporal_regression() {
        // R2 accepted at t1, then rejected at t2 -> once_accepted fires at t2.
        let trace = "t=1 entity=R2 collateralised=F has_code=F accepted=T rejected=F\n\
                     t=2 entity=R2 collateralised=F has_code=F accepted=F rejected=T\n";
        let r = monitor(SPEC, trace);
        assert!(count(&r, "once_accepted") == 1, "{r}");
    }

    #[test]
    fn clean_trace_has_no_violations() {
        let trace = "t=1 entity=R3 collateralised=T has_code=T accepted=T rejected=F\n\
                     t=2 entity=R3 collateralised=T has_code=T accepted=T rejected=F\n";
        let r = monitor(SPEC, trace);
        assert!(r.contains("\"ok\":true"), "{r}");
    }
}
