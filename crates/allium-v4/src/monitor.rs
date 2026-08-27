//! Runtime monitor derived from a v4 spec's invariants. The SAME `invariant` items the
//! design-time checks verify are evaluated here over a concrete execution trace: the
//! runtime modality of one artifact (design-time verify + build + monitor from one spec).
//!
//! Three kinds of invariant are monitored, and anything outside them is honestly SKIPPED
//! with a reason (never silently mis-evaluated — a wrong verdict is worse than a gap):
//!   - point:      a per-state boolean safety check;
//!   - temporal:   uses `old(...)`, evaluated against the entity's previous trace state,
//!                 so monotonicity / non-regression ("once accepted, never rejected") work;
//!   - relational: quantified (`every`/`some`/`no`/`exists`) over the current population of
//!                 entities, with equality on value fields — uniqueness, referential
//!                 integrity, functional-dependency properties.
//!
//! Trace format (dependency-free, one event per line):
//!   `t=<int> entity=<id> <pred>=T <pred>=F <field>=<value> ...`
//! A `T/F/true/false/1/0` value is a boolean predicate; anything else is a value field
//! (for equality in relational invariants). Events for one entity are ordered; `old(p(x))`
//! reads that entity's previous event; relational invariants read the latest state of every
//! entity seen so far.

use std::collections::HashMap;

use crate::analyse::canon;
use crate::ast::ItemKind;
use crate::expr::{BinOp, Expr, Quant, UnOp};

/// Strip a predicate application to its name: `cleared(r)` -> `cleared`.
fn pred_name(e: &Expr) -> String {
    let c = canon(e);
    match c.find('(') {
        Some(i) => c[..i].trim().to_string(),
        None => c,
    }
}

/// A unary atom `pred(var)` -> (pred, var); None if not that shape.
fn atom_pred_var(e: &Expr) -> Option<(String, String)> {
    if let Expr::App { head, args } = e {
        if let (Expr::Name(p), [Expr::Name(v)]) = (head.as_ref(), args.as_slice()) {
            return Some((p.clone(), v.clone()));
        }
    }
    None
}

fn has_quant(e: &Expr) -> bool {
    match e {
        Expr::Quant { .. } => true,
        Expr::Unary { e, .. } => has_quant(e),
        Expr::Binary { lhs, rhs, .. } => has_quant(lhs) || has_quant(rhs),
        _ => false,
    }
}

fn uses_old(e: &Expr) -> bool {
    match e {
        Expr::Unary { op: UnOp::Old, .. } => true,
        Expr::Unary { e, .. } => uses_old(e),
        Expr::Binary { lhs, rhs, .. } => uses_old(lhs) || uses_old(rhs),
        Expr::Quant { body, .. } => uses_old(body),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Point / temporal path (single entity per event)
// ---------------------------------------------------------------------------

/// The atoms a point/temporal invariant reads, tagged `old`, for a focused witness.
fn atoms_of(e: &Expr, in_old: bool, out: &mut Vec<(String, bool)>) {
    match e {
        Expr::Binary { op: BinOp::And | BinOp::Or | BinOp::Implies, lhs, rhs } => {
            atoms_of(lhs, in_old, out);
            atoms_of(rhs, in_old, out);
        }
        Expr::Unary { op: UnOp::Not, e } => atoms_of(e, in_old, out),
        Expr::Unary { op: UnOp::Old, e } => atoms_of(e, true, out),
        atom => {
            let key = (pred_name(atom), in_old);
            if !out.contains(&key) {
                out.push(key);
            }
        }
    }
}

fn eval_state(e: &Expr, s: &HashMap<String, bool>) -> bool {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => eval_state(lhs, s) && eval_state(rhs, s),
        Expr::Binary { op: BinOp::Or, lhs, rhs } => eval_state(lhs, s) || eval_state(rhs, s),
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => !eval_state(lhs, s) || eval_state(rhs, s),
        Expr::Unary { op: UnOp::Not, e } => !eval_state(e, s),
        other => *s.get(&pred_name(other)).unwrap_or(&false),
    }
}

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

/// Unsupported point/temporal form (relational forms go through the relational path).
fn pt_unsupported(e: &Expr) -> Option<String> {
    match e {
        Expr::Binary { op: BinOp::And | BinOp::Or | BinOp::Implies, lhs, rhs } => pt_unsupported(lhs).or_else(|| pt_unsupported(rhs)),
        Expr::Binary { .. } => Some("value comparison (monitor is boolean-only outside quantifiers)".into()),
        Expr::Unary { e, .. } => pt_unsupported(e),
        Expr::App { args, .. } if args.len() != 1 => Some(format!("relational atom of arity {} (use a quantified invariant)", args.len())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Relational path (quantified over the population)
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum RVal {
    B(bool),
    S(String),
    E(String),
}
impl RVal {
    fn truthy(&self) -> bool {
        matches!(self, RVal::B(true))
    }
}

#[derive(Default, Clone)]
struct EState {
    bools: HashMap<String, bool>,
    vals: HashMap<String, String>,
}

type Env = HashMap<String, String>;
type Pop = HashMap<String, EState>;

/// Relational forms this monitor cannot faithfully evaluate.
fn rel_unsupported(e: &Expr) -> Option<String> {
    match e {
        Expr::Unary { op: UnOp::Old, .. } => Some("`old` inside a quantifier (temporal+relational not supported)".into()),
        Expr::Quant { body, .. } => rel_unsupported(body),
        Expr::Binary { op: BinOp::And | BinOp::Or | BinOp::Implies | BinOp::Eq | BinOp::Ne, lhs, rhs } => {
            rel_unsupported(lhs).or_else(|| rel_unsupported(rhs))
        }
        Expr::Binary { .. } => Some("arithmetic/inequality in a quantified invariant (not supported)".into()),
        Expr::Unary { op: UnOp::Not, e } => rel_unsupported(e),
        Expr::App { args, .. } if args.len() != 1 => Some(format!("relational atom of arity {}", args.len())),
        _ => None,
    }
}

fn eval_rel(e: &Expr, env: &Env, pop: &Pop) -> RVal {
    match e {
        Expr::Quant { q, vars, body, .. } => eval_quant(q, vars, body, env, pop),
        Expr::Binary { op: BinOp::And, lhs, rhs } => RVal::B(eval_rel(lhs, env, pop).truthy() && eval_rel(rhs, env, pop).truthy()),
        Expr::Binary { op: BinOp::Or, lhs, rhs } => RVal::B(eval_rel(lhs, env, pop).truthy() || eval_rel(rhs, env, pop).truthy()),
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => RVal::B(!eval_rel(lhs, env, pop).truthy() || eval_rel(rhs, env, pop).truthy()),
        Expr::Binary { op: BinOp::Eq, lhs, rhs } => RVal::B(eval_rel(lhs, env, pop) == eval_rel(rhs, env, pop)),
        Expr::Binary { op: BinOp::Ne, lhs, rhs } => RVal::B(eval_rel(lhs, env, pop) != eval_rel(rhs, env, pop)),
        Expr::Unary { op: UnOp::Not, e } => RVal::B(!eval_rel(e, env, pop).truthy()),
        // A unary atom: boolean predicate -> B, value field -> S, else absent.
        app if atom_pred_var(app).is_some() => {
            let (pred, var) = atom_pred_var(app).unwrap();
            match env.get(&var).and_then(|ent| pop.get(ent)) {
                Some(st) => {
                    if let Some(b) = st.bools.get(&pred) {
                        RVal::B(*b)
                    } else if let Some(v) = st.vals.get(&pred) {
                        RVal::S(v.clone())
                    } else {
                        RVal::B(false)
                    }
                }
                None => RVal::B(false),
            }
        }
        // A bare name is an entity variable reference (for `a = b` identity).
        Expr::Name(v) => match env.get(v) {
            Some(ent) => RVal::E(ent.clone()),
            None => RVal::B(false),
        },
        _ => RVal::B(false),
    }
}

fn eval_quant(q: &Quant, vars: &[String], body: &Expr, env: &Env, pop: &Pop) -> RVal {
    if vars.is_empty() {
        return eval_rel(body, env, pop);
    }
    let (v, rest) = vars.split_first().unwrap();
    let mut n_true = 0usize;
    let mut n = 0usize;
    for ent in pop.keys() {
        let mut env2 = env.clone();
        env2.insert(v.clone(), ent.clone());
        if eval_quant(q, rest, body, &env2, pop).truthy() {
            n_true += 1;
        }
        n += 1;
    }
    RVal::B(match q {
        Quant::Every => n_true == n,
        Quant::Some => n_true >= 1,
        Quant::No => n_true == 0,
        Quant::ExistsOne => n_true == 1,
    })
}

/// Best-effort witness for a violated universal (`every`) prefix: a binding that falsifies.
fn find_witness(e: &Expr, env: &Env, pop: &Pop) -> Option<Vec<(String, String)>> {
    if let Expr::Quant { q: Quant::Every, vars, body, .. } = e {
        let (v, rest) = vars.split_first().unwrap();
        for ent in pop.keys() {
            let mut env2 = env.clone();
            env2.insert(v.clone(), ent.clone());
            let sub = if rest.is_empty() {
                if !eval_rel(body, &env2, pop).truthy() {
                    Some(vec![])
                } else {
                    None
                }
            } else {
                find_witness(&Expr::Quant { q: Quant::Every, vars: rest.to_vec(), ty: None, body: body.clone() }, &env2, pop)
            };
            if let Some(mut w) = sub {
                w.insert(0, (v.clone(), ent.clone()));
                return Some(w);
            }
        }
        None
    } else if !eval_rel(e, env, pop).truthy() {
        Some(vec![])
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Trace + driver
// ---------------------------------------------------------------------------

struct Event {
    t: String,
    entity: String,
    st: EState,
}

fn is_bool_lit(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "t" | "true" | "1" => Some(true),
        "f" | "false" | "0" => Some(false),
        _ => None,
    }
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
        let mut st = EState::default();
        for tok in line.split_whitespace() {
            let Some((k, v)) = tok.split_once('=') else { continue };
            match k {
                "t" => t = v.to_string(),
                "entity" => entity = v.to_string(),
                _ => match is_bool_lit(v) {
                    Some(b) => {
                        st.bools.insert(k.to_string(), b);
                    }
                    None => {
                        st.vals.insert(k.to_string(), v.to_string());
                    }
                },
            }
        }
        events.push(Event { t, entity, st });
    }
    events
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

enum Kind {
    Point,
    Temporal,
    Relational,
}

/// Run the monitor. Returns a JSON report `{events, monitored, skipped, violations, ok}`.
pub fn monitor(source: &str, trace: &str) -> String {
    let module = crate::check::check(source).module;
    let raw: Vec<(String, Expr)> = module
        .decls
        .iter()
        .flat_map(|d| d.items.iter())
        .filter(|it| it.kind == ItemKind::Invariant)
        .filter_map(|it| it.body.map(|sp| (it.name.clone().unwrap_or_else(|| "<anon>".into()), crate::expr::parse_predicate(sp.slice(source)).0)))
        .collect();

    let mut skipped: Vec<String> = Vec::new();
    let mut invs: Vec<(String, Expr, Kind)> = Vec::new();
    for (name, e) in raw {
        let reason = if has_quant(&e) { rel_unsupported(&e) } else { pt_unsupported(&e) };
        if let Some(r) = reason {
            skipped.push(format!("{{\"invariant\":\"{}\",\"reason\":\"{}\"}}", esc(&name), esc(&r)));
            continue;
        }
        let kind = if has_quant(&e) {
            Kind::Relational
        } else if uses_old(&e) {
            Kind::Temporal
        } else {
            Kind::Point
        };
        invs.push((name, e, kind));
    }

    let events = parse_trace(trace);
    let mut prev: HashMap<String, HashMap<String, bool>> = HashMap::new();
    let mut pop: Pop = HashMap::new();
    let mut rel_reported: HashMap<String, ()> = HashMap::new();
    let mut violations: Vec<String> = Vec::new();

    for ev in &events {
        // Point / temporal invariants: bound to this event's entity.
        for (name, expr, kind) in &invs {
            match kind {
                Kind::Point => {
                    if !eval_state(expr, &ev.st.bools) {
                        violations.push(point_violation(&ev.t, &ev.entity, name, "point", expr, &ev.st.bools, None));
                    }
                }
                Kind::Temporal => {
                    if let Some(p) = prev.get(&ev.entity) {
                        if !eval_temporal(expr, &ev.st.bools, p) {
                            violations.push(point_violation(&ev.t, &ev.entity, name, "temporal", expr, &ev.st.bools, Some(p)));
                        }
                    }
                }
                Kind::Relational => {}
            }
        }

        // Update population, then check relational invariants over it.
        pop.insert(ev.entity.clone(), ev.st.clone());
        for (name, expr, kind) in &invs {
            if !matches!(kind, Kind::Relational) || rel_reported.contains_key(name) {
                continue;
            }
            if let Some(binding) = find_witness(expr, &HashMap::new(), &pop) {
                rel_reported.insert(name.clone(), ());
                let w = binding.iter().map(|(v, e)| format!("{v}={e}")).collect::<Vec<_>>().join(", ");
                violations.push(format!(
                    "{{\"t\":\"{}\",\"invariant\":\"{}\",\"kind\":\"relational\",\"witness\":\"{}\"}}",
                    esc(&ev.t), esc(name), esc(&w)
                ));
            }
        }

        prev.insert(ev.entity.clone(), ev.st.bools.clone());
    }

    format!(
        "{{\"events\":{},\"monitored\":{},\"skipped\":[{}],\"violations\":[{}],\"ok\":{}}}",
        events.len(),
        invs.len(),
        skipped.join(","),
        violations.join(","),
        violations.is_empty()
    )
}

fn point_violation(t: &str, entity: &str, name: &str, kind: &str, expr: &Expr, cur: &HashMap<String, bool>, prev: Option<&HashMap<String, bool>>) -> String {
    let mut atoms = Vec::new();
    atoms_of(expr, false, &mut atoms);
    let witness = atoms
        .iter()
        .map(|(pred, is_old)| {
            let v = if *is_old { prev.and_then(|s| s.get(pred)).copied() } else { cur.get(pred).copied() };
            let label = if *is_old { format!("old {pred}") } else { pred.clone() };
            format!("{label}={}", v.map(|b| if b { "T" } else { "F" }).unwrap_or("?"))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{{\"t\":\"{}\",\"entity\":\"{}\",\"invariant\":\"{}\",\"kind\":\"{}\",\"witness\":\"{}\"}}",
        esc(t), esc(entity), esc(name), kind, esc(&witness)
    )
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
        let r = monitor(SPEC, "t=1 entity=R1 collateralised=T has_code=F accepted=F rejected=F\n");
        assert!(count(&r, "collat_needs_code") == 1, "{r}");
        assert!(r.contains("\"ok\":false"));
    }

    #[test]
    fn catches_temporal_regression() {
        let trace = "t=1 entity=R2 collateralised=F has_code=F accepted=T rejected=F\n\
                     t=2 entity=R2 collateralised=F has_code=F accepted=F rejected=T\n";
        let r = monitor(SPEC, trace);
        assert!(count(&r, "once_accepted") == 1, "{r}");
    }

    #[test]
    fn skips_unsupported_point_invariant() {
        let spec = "-- allium: 4\ncomponent M\n  entity R\n  observable state bal(R) : bool\n  invariant keeps means bal(r) = old(bal(r))\nend\n";
        let r = monitor(spec, "t=1 entity=R1 bal=T\nt=2 entity=R1 bal=F\n");
        assert!(r.contains("\"monitored\":0"), "{r}");
        assert!(r.contains("keeps") && r.contains("boolean-only"), "{r}");
        assert!(r.contains("\"ok\":true"), "{r}");
    }

    #[test]
    fn relational_uniqueness_violation() {
        // no two reports share a uti
        let spec = "-- allium: 4\ncomponent M\n  entity R\n  observable state uti(R) : bool\n  invariant unique_uti means every a :: every b :: (uti(a) = uti(b)) implies (a = b)\nend\n";
        let clean = "t=1 entity=R1 uti=U1\nt=2 entity=R2 uti=U2\n";
        assert!(monitor(spec, clean).contains("\"ok\":true"), "{}", monitor(spec, clean));
        let dup = "t=1 entity=R1 uti=U1\nt=2 entity=R2 uti=U1\n";
        let r = monitor(spec, dup);
        assert!(r.contains("unique_uti") && r.contains("relational"), "{r}");
        assert!(r.contains("\"ok\":false"), "{r}");
    }

    #[test]
    fn relational_referential_integrity() {
        // every allocation references an existing block uti
        let spec = "-- allium: 4\ncomponent M\n  entity R\n  observable state allocation(R) : bool\n  observable state prior(R) : bool\n  observable state uti(R) : bool\n  invariant ref_ok means every a :: allocation(a) implies some b :: prior(a) = uti(b)\nend\n";
        // R2 allocation refers to prior=U9 but no report has uti=U9 -> violation
        let bad = "t=1 entity=R1 allocation=F prior=none uti=U1\nt=2 entity=R2 allocation=T prior=U9 uti=U2\n";
        let r = monitor(spec, bad);
        assert!(r.contains("ref_ok"), "{r}");
        // when a block with uti=U9 exists, no violation
        let good = "t=1 entity=R1 allocation=F prior=none uti=U9\nt=2 entity=R2 allocation=T prior=U9 uti=U2\n";
        assert!(monitor(spec, good).contains("\"ok\":true"), "{}", monitor(spec, good));
    }
}
