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
    order: usize, // arrival index of this entity's event; enables ordering predicates before/precedes/after
}

/// Built-in ordering predicates over the event timeline (arity 2).
fn is_order_pred(h: &str) -> bool {
    matches!(h, "before" | "precedes" | "after" | "follows" | "succ" | "successor" | "next")
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
        Expr::App { head, args } if args.len() == 2 && matches!(head.as_ref(), Expr::Name(h) if is_order_pred(h)) => None,
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
        // Ordering predicate over the event timeline: before/precedes/after/follows(a, b).
        Expr::App { head, args } if args.len() == 2 && matches!(head.as_ref(), Expr::Name(h) if is_order_pred(h)) => {
            let ord = |e: &Expr| -> Option<usize> {
                match eval_rel(e, env, pop) {
                    RVal::E(ent) => pop.get(&ent).map(|s| s.order),
                    _ => None,
                }
            };
            let h = if let Expr::Name(h) = head.as_ref() { h.as_str() } else { "" };
            match (ord(&args[0]), ord(&args[1])) {
                (Some(a), Some(b)) => RVal::B(match h {
                    "after" => a > b,
                    "follows" | "succ" | "successor" | "next" => a == b + 1,
                    _ => a < b, // before / precedes
                }),
                _ => RVal::B(false),
            }
        }
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
/// Descends both a quantifier's variable list and nested `every` quantifiers in the body.
fn find_witness(e: &Expr, env: &Env, pop: &Pop) -> Option<Vec<(String, String)>> {
    match e {
        Expr::Quant { q: Quant::Every, vars, body, .. } => find_over(vars, body, env, pop),
        _ if !eval_rel(e, env, pop).truthy() => Some(vec![]),
        _ => None,
    }
}

fn find_over(vars: &[String], body: &Expr, env: &Env, pop: &Pop) -> Option<Vec<(String, String)>> {
    let Some((v, rest)) = vars.split_first() else {
        return find_witness(body, env, pop); // body may be a nested `every` or a leaf
    };
    for ent in pop.keys() {
        let mut env2 = env.clone();
        env2.insert(v.clone(), ent.clone());
        if let Some(mut w) = find_over(rest, body, &env2, pop) {
            w.insert(0, (v.clone(), ent.clone()));
            return Some(w);
        }
    }
    None
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
    let defs = collect_defs(&module, source);
    let raw: Vec<(String, Expr)> = module
        .decls
        .iter()
        .flat_map(|d| d.items.iter())
        .filter(|it| it.kind == ItemKind::Invariant)
        .filter_map(|it| it.body.map(|sp| (it.name.clone().unwrap_or_else(|| "<anon>".into()), inline_defs(&crate::expr::parse_predicate(sp.slice(source)).0, &defs))))
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

    for (idx, ev) in events.iter().enumerate() {
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

        // Update population (recording arrival order), then check relational invariants over it.
        let mut st = ev.st.clone();
        st.order = idx;
        pop.insert(ev.entity.clone(), st);
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

// ---------------------------------------------------------------------------
// Arithmetic schedule monitor: evaluate arithmetic/quantified invariants over a
// concrete numeric trace (one schedule). Where the relational path above skips
// arithmetic honestly, this path evaluates it against real values and reports, per
// invariant, whether it holds and the WORST residual — so an exact law (residual 0)
// is distinguishable from one that only holds up to rounding (residual ~ half a unit).
// ---------------------------------------------------------------------------

/// One period's concrete values.
#[derive(Default, Clone)]
struct SPeriod {
    num: HashMap<String, f64>,
    boolean: HashMap<String, bool>,
}

/// A concrete schedule: ordered periods plus 0-ary givens (e.g. `disbursed`).
struct SModel {
    periods: Vec<SPeriod>,
    givens: HashMap<String, f64>,
}
impl SModel {
    fn n(&self) -> usize {
        self.periods.len()
    }
}

fn parse_schedule(trace: &str) -> SModel {
    let mut by_idx: std::collections::BTreeMap<usize, SPeriod> = std::collections::BTreeMap::new();
    let mut givens = HashMap::new();
    for line in trace.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        let is_given = toks.iter().any(|t| *t == "given" || t.starts_with("given="));
        if is_given {
            for tok in &toks {
                if let Some((k, v)) = tok.split_once('=') {
                    if k != "given" {
                        if let Ok(f) = v.parse::<f64>() {
                            givens.insert(k.to_string(), f);
                        }
                    }
                }
            }
            continue;
        }
        // A period row: needs `period=<i>`.
        let idx = toks.iter().find_map(|t| t.strip_prefix("period=").and_then(|v| v.parse::<usize>().ok()));
        let Some(idx) = idx else { continue };
        let mut p = SPeriod::default();
        for tok in &toks {
            if let Some((k, v)) = tok.split_once('=') {
                if k == "period" {
                    continue;
                }
                // Numbers first, so a numeric field of `0`/`1` is a number, not a boolean;
                // only genuine T/F/true/false become boolean predicates.
                if let Ok(f) = v.parse::<f64>() {
                    p.num.insert(k.to_string(), f);
                } else if let Some(b) = is_bool_lit(v) {
                    p.boolean.insert(k.to_string(), b);
                }
            }
        }
        by_idx.insert(idx, p);
    }
    SModel { periods: by_idx.into_values().collect(), givens }
}

fn s_idx(a: &Expr, env: &HashMap<String, usize>) -> Option<usize> {
    match a {
        Expr::Name(v) => env.get(v).copied(),
        Expr::Int(n) if *n >= 0 => Some(*n as usize),
        _ => None,
    }
}

/// Evaluate a numeric term against the schedule, or `None` if it cannot.
fn eval_num(e: &Expr, env: &HashMap<String, usize>, m: &SModel) -> Option<f64> {
    match e {
        Expr::Int(n) => Some(*n as f64),
        Expr::Dec(num, den) => Some(*num as f64 / *den as f64),
        Expr::Name(s) => m.givens.get(s).copied(),
        Expr::App { head, args } => {
            let name = match head.as_ref() {
                Expr::Name(s) => s,
                _ => return None,
            };
            if args.len() == 1 {
                let i = s_idx(&args[0], env)?;
                return m.periods.get(i).and_then(|p| p.num.get(name).copied());
            }
            m.givens.get(name).copied()
        }
        Expr::Unary { op: UnOp::Old, .. } => None,
        Expr::Binary { op: BinOp::Add, lhs, rhs } => Some(eval_num(lhs, env, m)? + eval_num(rhs, env, m)?),
        Expr::Binary { op: BinOp::Sub, lhs, rhs } => Some(eval_num(lhs, env, m)? - eval_num(rhs, env, m)?),
        Expr::Binary { op: BinOp::Mul, lhs, rhs } => Some(eval_num(lhs, env, m)? * eval_num(rhs, env, m)?),
        Expr::Sum { vars, body, .. } => {
            let mut acc = 0.0;
            let mut env2 = env.clone();
            let ok = sum_num(vars, 0, &mut env2, m, body, &mut acc);
            ok.then_some(acc)
        }
        _ => None,
    }
}

fn sum_num(
    vars: &[String],
    from: usize,
    env: &mut HashMap<String, usize>,
    m: &SModel,
    body: &Expr,
    acc: &mut f64,
) -> bool {
    if from == vars.len() {
        return match eval_num(body, env, m) {
            Some(v) => {
                *acc += v;
                true
            }
            None => false,
        };
    }
    for i in 0..m.n() {
        env.insert(vars[from].clone(), i);
        if !sum_num(vars, from + 1, env, m, body, acc) {
            return false;
        }
    }
    env.remove(&vars[from]);
    true
}

/// Evaluate a boolean/guard term against the schedule.
fn eval_sbool(e: &Expr, env: &HashMap<String, usize>, m: &SModel, tol: f64) -> Option<bool> {
    match e {
        Expr::Name(s) if s == "true" => Some(true),
        Expr::Name(s) if s == "false" => Some(false),
        Expr::Unary { op: UnOp::Not, e } => eval_sbool(e, env, m, tol).map(|b| !b),
        Expr::Binary { op: BinOp::And, lhs, rhs } => Some(eval_sbool(lhs, env, m, tol)? && eval_sbool(rhs, env, m, tol)?),
        Expr::Binary { op: BinOp::Or, lhs, rhs } => Some(eval_sbool(lhs, env, m, tol)? || eval_sbool(rhs, env, m, tol)?),
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => Some(!eval_sbool(lhs, env, m, tol)? || eval_sbool(rhs, env, m, tol)?),
        Expr::Binary { op: op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), lhs, rhs } => {
            let (l, r) = (eval_num(lhs, env, m)?, eval_num(rhs, env, m)?);
            Some(cmp_holds(op, l, r, tol))
        }
        Expr::App { head, args } => {
            let name = match head.as_ref() {
                Expr::Name(s) => s.as_str(),
                _ => return None,
            };
            let idx: Option<Vec<usize>> = args.iter().map(|a| s_idx(a, env)).collect();
            let idx = idx?;
            match (name, idx.as_slice()) {
                ("follows" | "succ" | "successor" | "next", [a, b]) => Some(*a == b + 1),
                ("precedes" | "before", [a, b]) => Some(a < b),
                ("after", [a, b]) => Some(a > b),
                ("is_last" | "last" | "final", [a]) => Some(
                    m.periods.get(*a).and_then(|p| p.boolean.get("is_last").copied()).unwrap_or(*a + 1 == m.n()),
                ),
                ("is_first" | "first", [a]) => Some(*a == 0),
                // A per-period boolean field.
                (field, [a]) => m.periods.get(*a).and_then(|p| p.boolean.get(field).copied()),
                _ => None,
            }
        }
        Expr::Quant { q, vars, body, .. } => {
            let (mut t, mut total) = (0usize, 0usize);
            let mut env2 = env.clone();
            quant_count(vars, 0, &mut env2, m, body, tol, &mut t, &mut total);
            Some(match q {
                Quant::Every => t == total,
                Quant::Some => t >= 1,
                Quant::No => t == 0,
                Quant::ExistsOne => t == 1,
            })
        }
        _ => None,
    }
}

fn quant_count(
    vars: &[String],
    from: usize,
    env: &mut HashMap<String, usize>,
    m: &SModel,
    body: &Expr,
    tol: f64,
    t: &mut usize,
    total: &mut usize,
) {
    if from == vars.len() {
        *total += 1;
        if eval_sbool(body, env, m, tol).unwrap_or(false) {
            *t += 1;
        }
        return;
    }
    for i in 0..m.n() {
        env.insert(vars[from].clone(), i);
        quant_count(vars, from + 1, env, m, body, tol, t, total);
    }
    env.remove(&vars[from]);
}

fn cmp_holds(op: &BinOp, l: f64, r: f64, tol: f64) -> bool {
    match op {
        BinOp::Eq => (l - r).abs() <= tol,
        BinOp::Ne => (l - r).abs() > tol,
        BinOp::Le => l <= r + tol,
        BinOp::Lt => l < r + tol,
        BinOp::Ge => l >= r - tol,
        BinOp::Gt => l > r - tol,
        _ => false,
    }
}

fn cmp_residual(op: &BinOp, l: f64, r: f64) -> f64 {
    match op {
        BinOp::Eq | BinOp::Ne => (l - r).abs(),
        BinOp::Le | BinOp::Lt => (l - r).max(0.0),
        BinOp::Ge | BinOp::Gt => (r - l).max(0.0),
        _ => 0.0,
    }
}

/// A single ground check produced by instantiating an invariant.
struct Inst {
    holds: bool,
    residual: f64,
    desc: String,
}

fn env_str(env: &HashMap<String, usize>) -> String {
    let mut v: Vec<(String, usize)> = env.iter().map(|(k, i)| (k.clone(), *i)).collect();
    v.sort();
    v.into_iter().map(|(k, i)| format!("{k}=p{i}")).collect::<Vec<_>>().join(", ")
}

/// Instantiate an invariant over the schedule into ground checks (respecting `every`,
/// `and`, and `implies` guards). Arithmetic leaves record their residual.
fn collect(e: &Expr, env: &mut HashMap<String, usize>, m: &SModel, tol: f64, out: &mut Vec<Inst>) {
    match e {
        Expr::Quant { q: Quant::Every, vars, body, .. } => {
            collect_bind(vars, 0, env, m, tol, body, out);
        }
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            collect(lhs, env, m, tol, out);
            collect(rhs, env, m, tol, out);
        }
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => match eval_sbool(lhs, env, m, tol) {
            Some(true) => collect(rhs, env, m, tol, out),
            _ => {}
        },
        Expr::Binary { op: op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), lhs, rhs } => {
            if let (Some(l), Some(r)) = (eval_num(lhs, env, m), eval_num(rhs, env, m)) {
                let holds = cmp_holds(op, l, r, tol);
                out.push(Inst {
                    holds,
                    residual: cmp_residual(op, l, r),
                    desc: format!("[{}] {} ({:.4} vs {:.4})", env_str(env), canon(e), l, r),
                });
            }
        }
        // A non-arithmetic leaf (a bare boolean guard).
        other => {
            if let Some(b) = eval_sbool(other, env, m, tol) {
                out.push(Inst { holds: b, residual: 0.0, desc: format!("[{}] {}", env_str(env), canon(other)) });
            }
        }
    }
}

fn collect_bind(
    vars: &[String],
    from: usize,
    env: &mut HashMap<String, usize>,
    m: &SModel,
    tol: f64,
    body: &Expr,
    out: &mut Vec<Inst>,
) {
    if from == vars.len() {
        collect(body, env, m, tol, out);
        return;
    }
    for i in 0..m.n() {
        env.insert(vars[from].clone(), i);
        collect_bind(vars, from + 1, env, m, tol, body, out);
    }
    env.remove(&vars[from]);
}

/// Substitute `map` (param name -> argument expr) through `e`, respecting quantifier shadowing.
fn substitute(e: &Expr, map: &HashMap<String, Expr>) -> Expr {
    match e {
        Expr::Name(n) => map.get(n).cloned().unwrap_or_else(|| e.clone()),
        Expr::App { head, args } => Expr::App {
            head: Box::new(substitute(head, map)),
            args: args.iter().map(|a| substitute(a, map)).collect(),
        },
        Expr::Quant { q, vars, ty, body } => {
            let mut inner = map.clone();
            for v in vars { inner.remove(v); }
            Expr::Quant { q: q.clone(), vars: vars.clone(), ty: ty.clone(), body: Box::new(substitute(body, &inner)) }
        }
        Expr::Sum { vars, ty, body } => {
            let mut inner = map.clone();
            for v in vars { inner.remove(v); }
            Expr::Sum { vars: vars.clone(), ty: ty.clone(), body: Box::new(substitute(body, &inner)) }
        }
        Expr::Binary { op, lhs, rhs } => Expr::Binary { op: op.clone(), lhs: Box::new(substitute(lhs, map)), rhs: Box::new(substitute(rhs, map)) },
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(substitute(e, map)) },
        Expr::Field { base, name } => Expr::Field { base: Box::new(substitute(base, map)), name: name.clone() },
        other => other.clone(),
    }
}

/// Inline calls to defined givens (`f(params) means body`, an OCaml-style pure reference function) by
/// substituting arguments into the body. Lets invariants use reference-oracle definitions; nested
/// definition calls resolve by re-inlining the substituted body.
fn inline_defs(e: &Expr, defs: &HashMap<String, (Vec<String>, Expr)>) -> Expr {
    match e {
        Expr::App { head, args } => {
            let iargs: Vec<Expr> = args.iter().map(|a| inline_defs(a, defs)).collect();
            if let Expr::Name(f) = head.as_ref() {
                if let Some((params, body)) = defs.get(f) {
                    if params.len() == iargs.len() {
                        let map: HashMap<String, Expr> = params.iter().cloned().zip(iargs).collect();
                        return inline_defs(&substitute(body, &map), defs);
                    }
                }
            }
            Expr::App { head: Box::new(inline_defs(head, defs)), args: iargs }
        }
        Expr::Quant { q, vars, ty, body } => Expr::Quant { q: q.clone(), vars: vars.clone(), ty: ty.clone(), body: Box::new(inline_defs(body, defs)) },
        Expr::Sum { vars, ty, body } => Expr::Sum { vars: vars.clone(), ty: ty.clone(), body: Box::new(inline_defs(body, defs)) },
        Expr::Binary { op, lhs, rhs } => Expr::Binary { op: op.clone(), lhs: Box::new(inline_defs(lhs, defs)), rhs: Box::new(inline_defs(rhs, defs)) },
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(inline_defs(e, defs)) },
        Expr::Field { base, name } => Expr::Field { base: Box::new(inline_defs(base, defs)), name: name.clone() },
        other => other.clone(),
    }
}

/// Defined givens with parameters: `given f(params) means body` -> (name, (params, body-expr)).
fn collect_defs(module: &crate::ast::Module, source: &str) -> HashMap<String, (Vec<String>, Expr)> {
    module
        .decls
        .iter()
        .flat_map(|d| d.items.iter())
        .filter(|it| it.kind == ItemKind::Given && !it.params.is_empty() && it.body.is_some())
        .filter_map(|it| {
            let name = it.name.clone()?;
            let body = crate::expr::parse_predicate(it.body?.slice(source)).0;
            Some((name, (it.params.clone(), body)))
        })
        .collect()
}

/// Monitor arithmetic invariants over one concrete schedule trace. `tol` is the
/// tolerance for equalities/inequalities (e.g. 0.005 to allow currency rounding).
/// JSON: `{periods, monitored, results:[{invariant,holds,checks,max_residual,witness}], ok}`.
pub fn monitor_schedule(source: &str, trace: &str, tol: f64) -> String {
    let module = crate::check::check(source).module;
    let defs = collect_defs(&module, source);
    let invs: Vec<(String, Expr)> = module
        .decls
        .iter()
        .flat_map(|d| d.items.iter())
        .filter(|it| it.kind == ItemKind::Invariant)
        .filter_map(|it| it.body.map(|sp| (it.name.clone().unwrap_or_else(|| "<anon>".into()), inline_defs(&crate::expr::parse_predicate(sp.slice(source)).0, &defs))))
        .collect();
    let m = parse_schedule(trace);

    let mut results = Vec::new();
    let mut all_ok = true;
    let mut monitored = 0usize;
    for (name, e) in &invs {
        let mut insts = Vec::new();
        let mut env = HashMap::new();
        collect(e, &mut env, &m, tol, &mut insts);
        if insts.is_empty() {
            continue; // nothing to check (non-arithmetic / no instances)
        }
        monitored += 1;
        let holds = insts.iter().all(|i| i.holds);
        let max_res = insts.iter().map(|i| i.residual).fold(0.0f64, f64::max);
        let witness = insts.iter().find(|i| !i.holds).map(|i| i.desc.clone()).unwrap_or_default();
        if !holds {
            all_ok = false;
        }
        results.push(format!(
            "{{\"invariant\":\"{}\",\"holds\":{},\"checks\":{},\"max_residual\":{:.6},\"witness\":\"{}\"}}",
            esc(name),
            holds,
            insts.len(),
            max_res,
            esc(&witness)
        ));
    }
    format!(
        "{{\"periods\":{},\"monitored\":{},\"tol\":{},\"results\":[{}],\"ok\":{}}}",
        m.n(),
        monitored,
        tol,
        results.join(","),
        all_ok
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
    fn reference_oracle_defined_given() {
        // OCaml-style pure reference function via `given f(params) means body`, inlined and checked.
        let spec = "-- allium: 4\ncomponent S\n  entity Period\n  given rate : Rate\n  given expected_interest(bal) means rate * bal\n  observable state interest(Period) : Money\n  observable state outstanding_start(Period) : Money\n  invariant interest_ok means every p :: interest(p) = expected_interest(outstanding_start(p))\nend\n";
        let ok = "period=0 interest=100.00 outstanding_start=1000.00\ngiven rate=0.10\n";
        assert!(monitor_schedule(spec, ok, 0.005).contains("\"ok\":true"), "correct interest should hold: {}", monitor_schedule(spec, ok, 0.005));
        let bad = "period=0 interest=90.00 outstanding_start=1000.00\ngiven rate=0.10\n";
        let r = monitor_schedule(spec, bad, 0.005);
        assert!(r.contains("interest_ok") && r.contains("\"ok\":false"), "wrong interest should be caught: {r}");
    }

    #[test]
    fn multistep_ordering_before() {
        // before(a,c): a capture must have some authorize earlier in the event timeline.
        let spec = "-- allium: 4\ncomponent Payments\n  entity Event\n  observable state is_auth(Event) : bool\n  observable state is_capture(Event) : bool\n  invariant capture_needs_prior_auth means every c :: is_capture(c) implies some a :: (is_auth(a) and before(a, c))\nend\n";
        let ok = "entity=e1 is_auth=T is_capture=F\nentity=e2 is_auth=F is_capture=T\n";
        assert!(monitor(spec, ok).contains("\"ok\":true"), "auth before capture should hold: {}", monitor(spec, ok));
        let bad = "entity=e1 is_auth=F is_capture=T\nentity=e2 is_auth=T is_capture=F\n";
        let r = monitor(spec, bad);
        assert!(r.contains("capture_needs_prior_auth") && r.contains("\"ok\":false"), "auth after capture should violate: {r}");
    }

    #[test]
    fn relational_quantified_forms_correct() {
        // Locks in that some / at-most-one / entity-identity evaluate correctly (guards the D8 retraction).
        let spec = "-- allium: 4\ncomponent S\n  entity E\n  observable state is_open(E) : bool\n  invariant at_most_one means every a :: every b :: (is_open(a) and is_open(b)) implies (a = b)\n  invariant some_open means some a :: is_open(a)\nend\n";
        assert!(monitor(spec, "entity=a is_open=T\nentity=b is_open=T\n").contains("at_most_one"), "at_most_one must catch two open");
        assert!(!monitor(spec, "entity=a is_open=T\nentity=b is_open=F\n").contains("at_most_one"), "at_most_one must hold with one open");
        assert!(monitor(spec, "entity=a is_open=F\nentity=b is_open=F\n").contains("some_open"), "some_open must violate when none open");
        assert!(!monitor(spec, "entity=a is_open=T\nentity=b is_open=F\n").contains("some_open"), "some_open must hold when one open");
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

    // A schedule spec fragment: the load-bearing arithmetic loan invariants.
    const LOAN: &str = "-- allium: 4\ncomponent LoanSchedule\n  entity Period\n  given disbursed : Money\n  observable state emi(Period) : Money\n  observable state interest(Period) : Money\n  observable state principal(Period) : Money\n  observable state outstanding_start(Period) : Money\n  observable state is_last(Period) : bool\n  invariant principal_split means every p :: principal(p) = emi(p) - interest(p)\n  invariant balance_rolls means every p :: every next :: follows(next, p) implies (outstanding_start(next) = outstanding_start(p) - principal(p))\n  invariant balance_monotonic means every p :: every next :: follows(next, p) implies (outstanding_start(next) <= outstanding_start(p))\n  invariant conservation means sum p :: principal(p) = disbursed\n  invariant closes_to_zero means every p :: is_last(p) implies (outstanding_start(p) - principal(p) = 0)\nend\n";

    // A correct 3-period schedule: disbursed 1000, principals 300/330/370, roll-forward exact.
    // Last period's instalment equals its principal+interest (370+0); emi_constant only
    // constrains non-final periods, so the final instalment may differ.
    const GOOD: &str = "period=0 emi=400 interest=100 principal=300 outstanding_start=1000 is_last=F\nperiod=1 emi=400 interest=70 principal=330 outstanding_start=700 is_last=F\nperiod=2 emi=370 interest=0 principal=370 outstanding_start=370 is_last=T\ngiven disbursed=1000\n";

    #[test]
    fn schedule_monitor_passes_a_correct_schedule() {
        let r = monitor_schedule(LOAN, GOOD, 0.005);
        assert!(r.contains("\"ok\":true"), "{r}");
        assert!(r.contains("\"invariant\":\"conservation\",\"holds\":true"), "{r}");
        assert!(r.contains("\"invariant\":\"principal_split\",\"holds\":true"), "{r}");
    }

    #[test]
    fn schedule_monitor_catches_a_broken_conservation() {
        // principals 300/330/360 sum to 990, not the disbursed 1000.
        let bad = "period=0 emi=400 interest=100 principal=300 outstanding_start=1000 is_last=F\nperiod=1 emi=400 interest=70 principal=330 outstanding_start=700 is_last=F\nperiod=2 emi=400 interest=10 principal=360 outstanding_start=370 is_last=T\ngiven disbursed=1000\n";
        let r = monitor_schedule(LOAN, bad, 0.005);
        assert!(r.contains("\"invariant\":\"conservation\",\"holds\":false"), "{r}");
        assert!(r.contains("\"ok\":false"), "{r}");
    }

    #[test]
    fn schedule_monitor_catches_a_balance_increase() {
        // outstanding goes UP from p1 to p2 (negative amortisation): monotonicity breaks.
        let bad = "period=0 emi=400 interest=100 principal=300 outstanding_start=1000 is_last=F\nperiod=1 emi=50 interest=70 principal=-20 outstanding_start=700 is_last=F\nperiod=2 emi=400 interest=0 principal=720 outstanding_start=720 is_last=T\ngiven disbursed=1000\n";
        let r = monitor_schedule(LOAN, bad, 0.005);
        assert!(r.contains("\"invariant\":\"balance_monotonic\",\"holds\":false"), "{r}");
    }

    #[test]
    fn schedule_monitor_reports_residual_for_near_misses() {
        // conservation off by 0.01 (a penny): should FAIL at tol 0.005 with residual ~0.01.
        let bad = "period=0 emi=400 interest=100 principal=300 outstanding_start=1000 is_last=F\nperiod=1 emi=400 interest=70 principal=330 outstanding_start=700 is_last=F\nperiod=2 emi=400 interest=0 principal=369.99 outstanding_start=370 is_last=T\ngiven disbursed=1000\n";
        let r = monitor_schedule(LOAN, bad, 0.005);
        assert!(r.contains("\"invariant\":\"conservation\",\"holds\":false"), "{r}");
        // and PASS at a looser tol of 0.05
        assert!(monitor_schedule(LOAN, bad, 0.05).contains("\"invariant\":\"conservation\",\"holds\":true"));
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
