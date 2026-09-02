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
/// Desugar `state x : T where <pred>` refinement clauses into synthetic invariants, so every pass checks
/// them: the value is constrained by the predicate, an invariant to establish and maintain. Run once after
/// parsing, before analysis. (A refinement type is an invariant pinned to the declaration.)
fn desugar_where(module: &mut Module) {
    for d in &mut module.decls {
        let mut synth = Vec::new();
        for it in &d.items {
            if let (Some(name), Some(wp)) = (&it.name, it.where_pred) {
                let mut inv = crate::ast::Item::new(ItemKind::Invariant, it.span);
                inv.name = Some(format!("refine[{name}]"));
                inv.body = Some(wp);
                synth.push(inv);
            }
        }
        d.items.extend(synth);
    }
}

pub fn analyse(source: &str) -> ParseResult {
    analyse_with_imports(source, &crate::arith::Imports::default())
}

/// A parsed `transitions` block. An edge is `(source, target, optional guard)`.
struct TransBlock {
    obs: Option<String>,
    initial: Option<String>,
    edges: Vec<(String, String, Option<String>)>,
    terminals: Vec<String>,
}

/// Parse a `transitions` block body. The v4 surface: the observable name, then `initial <state>`,
/// `A -> B` / `A -> B when <cond>` edge lines, and `terminal <tag>` lines (a `terminal: a, b` list is also
/// accepted).
fn parse_transitions_body(body: &str) -> TransBlock {
    let mut b = TransBlock { obs: None, initial: None, edges: Vec::new(), terminals: Vec::new() };
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("--") {
            continue;
        }
        let first = line.split_whitespace().next().unwrap_or("");
        if first == "initial" {
            let s = line["initial".len()..].trim();
            if !s.is_empty() {
                b.initial = Some(s.split('(').next().unwrap_or(s).trim().to_string());
            }
        } else if let Some((src, rest)) = line.split_once("->") {
            let src = src.trim();
            // `B` or `B when <cond>`.
            let (tgt, guard) = match rest.split_once(" when ") {
                Some((t, g)) => (t.trim(), Some(g.trim().to_string())),
                None => (rest.trim(), None),
            };
            if !src.is_empty() && !tgt.is_empty() {
                b.edges.push((src.to_string(), tgt.to_string(), guard));
            }
        } else if first == "terminal" {
            for t in line["terminal".len()..].trim_start_matches(':').split(',') {
                let t = t.trim();
                if !t.is_empty() {
                    b.terminals.push(t.to_string());
                }
            }
        } else if b.obs.is_none() {
            let name = line.split('(').next().unwrap_or(line).trim();
            if !name.is_empty() {
                b.obs = Some(name.to_string());
            }
        }
    }
    b
}

/// Desugar every `transitions <obs> …` block into checkable v4 primitives, in place on the module:
///   - one enum-guarded action per edge `A -> B` (`requires obs(e)=A ensures obs(e)=B`);
///   - a legality two-state invariant — any change to `obs` is one of the declared edges (else unchanged);
///   - a finality invariant per `terminal T` (`old(obs(e))=T implies obs(e)=T`).
/// Synthesised item bodies are appended to a returned extended source (appending never moves the original
/// offsets, so existing spans stay valid); each synth item's diagnostic span is the `transitions` block, so
/// a break points at what the author wrote, not at generated text.
pub(crate) fn desugar_transitions(module: &mut Module, src: &str) -> String {
    use crate::ast::{Item, ItemKind};
    let mut ext = String::from(src);
    let mut push = |ext: &mut String, text: &str| -> crate::span::Span {
        ext.push('\n');
        let start = ext.len();
        ext.push_str(text);
        crate::span::Span::new(start, ext.len())
    };
    for d in module.decls.iter_mut() {
        if !d.items.iter().any(|it| it.kind == ItemKind::Transitions) {
            continue;
        }
        // The enum value set per observable, to enumerate the transitions the block forbids.
        let evals = enum_values_of(d, src, false);
        let mut keep: Vec<Item> = Vec::new();
        let mut synth: Vec<Item> = Vec::new();
        for it in std::mem::take(&mut d.items) {
            if it.kind != ItemKind::Transitions {
                keep.push(it);
                continue;
            }
            let block = it.span;
            let body = it.body.map(|sp| sp.slice(src).to_string()).unwrap_or_default();
            let tb = parse_transitions_body(&body);
            let Some(obs) = tb.obs else {
                keep.push(it);
                continue;
            };
            // `initial <state>` desugars to `init means obs(e) = <state>` — the start is explicit, never
            // implied, and the start state is no longer flagged unreachable.
            if let Some(start) = &tb.initial {
                let mut init = Item::new(ItemKind::Init, block);
                init.body = Some(push(&mut ext, &format!("{obs}(e) = {start}")));
                synth.push(init);
            }
            // One enum-guarded action per edge; `A -> B when <cond>` adds `<cond>` to the guard.
            for (a, b, guard) in &tb.edges {
                let req = match guard {
                    Some(g) => format!("{obs}(e) = {a} and {g}"),
                    None => format!("{obs}(e) = {a}"),
                };
                let mut act = Item::new(ItemKind::Action, block);
                act.name = Some(format!("{a}_to_{b}"));
                act.requires = Some(push(&mut ext, &req));
                act.ensures = vec![push(&mut ext, &format!("{obs}(e) = {b}"))];
                synth.push(act);
            }
            // Legality: classify each ordered pair of distinct states — an unguarded edge is allowed; an
            // all-guarded edge is allowed only if a guard held before (`… implies old(guard)`); a non-edge is
            // forbidden (`not (old = A and cur = B)`). No enum-to-old-enum frame term.
            let states = evals.get(&obs).cloned().unwrap_or_default();
            let mut unconditional: HashSet<(String, String)> = HashSet::new();
            let mut guarded: HashMap<(String, String), Vec<String>> = HashMap::new();
            for (a, b, g) in &tb.edges {
                let key = (a.clone(), b.clone());
                match g {
                    None => {
                        unconditional.insert(key.clone());
                        guarded.remove(&key);
                    }
                    Some(g) => {
                        if !unconditional.contains(&key) {
                            guarded.entry(key).or_default().push(g.clone());
                        }
                    }
                }
            }
            // Forbidden non-edges go into ONE pure-enum legality invariant. Each guarded edge becomes its
            // OWN invariant (`(old=A and cur=B) implies old(guard)`), so an arithmetic guard cannot drop the
            // enum legality with it, and a boolean guard is checked on its own.
            let mut forbidden: Vec<String> = Vec::new();
            for a in &states {
                for b in &states {
                    if a == b {
                        continue;
                    }
                    let key = (a.clone(), b.clone());
                    if unconditional.contains(&key) {
                        continue;
                    }
                    if let Some(gs) = guarded.get(&key) {
                        let ors = gs.iter().map(|g| format!("old({g})")).collect::<Vec<_>>().join(" or ");
                        let mut inv = Item::new(ItemKind::Invariant, block);
                        inv.name = Some(format!("{obs}_{a}_{b}_legal"));
                        inv.body =
                            Some(push(&mut ext, &format!("every e :: (old({obs}(e)) = {a} and {obs}(e) = {b}) implies ({ors})")));
                        synth.push(inv);
                    } else {
                        forbidden.push(format!("not (old({obs}(e)) = {a} and {obs}(e) = {b})"));
                    }
                }
            }
            if !forbidden.is_empty() {
                let mut inv = Item::new(ItemKind::Invariant, block);
                inv.name = Some(format!("{obs}_transitions_legal"));
                inv.body = Some(push(&mut ext, &format!("every e :: {}", forbidden.join(" and "))));
                synth.push(inv);
            }
            for t in &tb.terminals {
                let mut inv = Item::new(ItemKind::Invariant, block);
                inv.name = Some(format!("{obs}_{t}_final"));
                inv.body = Some(push(&mut ext, &format!("every e :: old({obs}(e)) = {t} implies {obs}(e) = {t}")));
                synth.push(inv);
            }
        }
        keep.extend(synth);
        d.items = keep;
    }
    ext
}

/// As [`analyse`], but with definitions resolved from other modules via `use` (given bodies today).
/// The CLI resolves the import graph and passes them; single-file callers use [`analyse`].
pub fn analyse_with_imports(source: &str, imports: &crate::arith::Imports) -> ParseResult {
    let mut r = crate::check::check(source);
    desugar_where(&mut r.module);
    // Expand `transitions` blocks into edge-actions + legality + finality invariants; the passes then run
    // over the extended source (original + appended synth bodies). Original spans are unchanged.
    let ext = desugar_transitions(&mut r.module, source);
    let source: &str = &ext;
    r.diagnostics.append(&mut coverage(&r.module, source));
    r.diagnostics.append(&mut consistency(&r.module, source));
    r.diagnostics.append(&mut feasibility(&r.module, source));
    r.diagnostics.append(&mut preservation(&r.module, source));
    r.diagnostics.append(&mut relational_preservation(&r.module, source));
    // 2-entity numeric ordering preservation (#49). Where it actually checks an invariant, drop the weaker
    // "NOT preservation-checked" note that relational_preservation emitted for it.
    let (mut rel_arith, rel_checked) = crate::arith::relational_arith_preservation(&r.module, source);
    r.diagnostics.retain(|d| {
        !(d.message.contains("is NOT preservation-checked")
            && rel_checked.iter().any(|n| d.message.contains(&format!("relational invariant `{n}`"))))
    });
    r.diagnostics.append(&mut rel_arith);
    r.diagnostics.append(&mut bmc(&r.module, source));
    r.diagnostics.append(&mut bmc_enum(&r.module, source));
    r.diagnostics.append(&mut reserved_tag_check(&r.module, source));
    r.diagnostics.append(&mut crate::arith::arithmetic(&r.module, source, imports));
    r.diagnostics.append(&mut crate::arith::reachability(&r.module, source));
    r.diagnostics.append(&mut crate::arith::arith_preservation(&r.module, source, imports));
    r.diagnostics.append(&mut crate::arith::enum_guarded_preservation(&r.module, source, imports));
    r.diagnostics.append(&mut crate::arith::transition_arith_legality(&r.module, source, imports));
    r.diagnostics.append(&mut crate::arith::aggregate_preservation(&r.module, source, imports));
    r.diagnostics.append(&mut variant_access(&r.module, source));
    r.diagnostics.append(&mut stuck_states(&r.module, source));
    r.diagnostics.append(&mut dead_states(&r.module, source));
    r.diagnostics.append(&mut tier_report(&r.module, source));
    r.diagnostics.append(&mut refinement(&r.module, source, imports));
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

/// Split the inside of an enum/variant type on top-level `|`, ignoring `|` nested inside a variant's
/// `{ field : type }` block. Returns the pipe-separated parts trimmed.
fn split_variants(inner: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in inner.chars() {
        match c {
            '{' => { depth += 1; cur.push(c); }
            '}' => { depth -= 1; cur.push(c); }
            '|' if depth == 0 => { parts.push(cur.trim().to_string()); cur.clear(); }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts
}

/// The variants of an inline enum/sum type `{ a | paid { at : Time } | c }`: each part's tag (the name
/// before any `{`) and its payload fields (`name : type` pairs inside the braces, empty for a bare tag).
fn parse_variants(ty: &str) -> Option<Vec<(String, Vec<(String, String)>)>> {
    let inner = ty.trim().strip_prefix('{')?.strip_suffix('}')?;
    let mut out = Vec::new();
    for part in split_variants(inner) {
        if part.is_empty() {
            continue;
        }
        let (tag, fields) = match part.split_once('{') {
            Some((t, rest)) => {
                let body = rest.trim_end_matches('}');
                let fs: Vec<(String, String)> = body
                    .split(',')
                    .filter_map(|f| f.split_once(':').map(|(n, t)| (n.trim().to_string(), t.trim().to_string())))
                    .collect();
                (t.trim().to_string(), fs)
            }
            None => (part.trim().to_string(), Vec::new()),
        };
        if !tag.is_empty() {
            out.push((tag, fields));
        }
    }
    (out.len() >= 2).then_some(out)
}

/// Stuck-state (deadlock) detection for enum lifecycles. A reachable enum state from which NO action can
/// fire, and which is not declared `terminal`, is a dead-end where the machine gets stuck. Unblocked by the
/// terminal marker, which tells intended end states from bugs. Reachability is approximated by "some action
/// (or init) puts the state there"; "can fire" is "some action's guard is satisfiable in that state".
pub fn stuck_states(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let evals = enum_values_of(d, src, false);
        // A lifecycle needs an enum state, actions, and a defined start; without `init` there is no
        // reachability to speak of (and it screens out malformed decls).
        if evals.is_empty()
            || !d.items.iter().any(|it| it.kind == ItemKind::Init)
            || !d.items.iter().any(|it| it.kind == ItemKind::Action)
        {
            continue;
        }
        let bnames = bool_names_of(d, src);
        let all_obs: HashSet<String> = d.items.iter().filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given)).filter_map(|it| it.name.clone()).collect();

        // Declared terminal state facts (canon of the normalised condition).
        let terminals: HashSet<String> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Terminal)
            .filter_map(|it| Some(canon(&normalize(&parse_predicate(it.body?.slice(src)).0))))
            .collect();

        // Reachability facts: state equalities established by init or any action's ensures.
        let mut reachable: HashSet<String> = HashSet::new();
        if let Some(init) = d.items.iter().find(|it| it.kind == ItemKind::Init).and_then(|it| it.body) {
            let t = init.slice(src);
            reachable.extend(discriminant_facts(&normalize(&parse_predicate(t.trim().strip_prefix("means").unwrap_or(t)).0)));
        }
        // Actions, normalised, with their guards; and their ensures contribute reachability targets.
        let actions: Vec<Option<Expr>> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Action)
            .map(|it| {
                if let Some(e) = it.ensures_expr(src) {
                    reachable.extend(discriminant_facts(&normalize(&e)));
                }
                it.requires.map(|sp| normalize(&parse_predicate(sp.slice(src)).0))
            })
            .collect();

        for (obs, vals) in &evals {
            for v in vals {
                // The state `obs(_e) = v`.
                let cond = Expr::Binary {
                    op: BinOp::Eq,
                    lhs: Box::new(Expr::App { head: Box::new(Expr::Name(obs.clone())), args: vec![Expr::Name(ENT.into())] }),
                    rhs: Box::new(Expr::Name(v.clone())),
                };
                let ckey = canon(&cond);
                if terminals.contains(&ckey) || !reachable.contains(&ckey) {
                    continue;
                }
                // Can any action fire here? `guard ∧ (obs = v)` satisfiable (a guardless action always can).
                let can_fire = actions.iter().any(|g| match g {
                    None => true,
                    Some(guard) => crate::sat::satisfiable_enum(&[guard, &cond], &bnames, &evals).is_some(),
                });
                let _ = &all_obs;
                if !can_fire {
                    out.push(Diagnostic::warning(
                        d.span,
                        pretty(&format!("state `{ckey}` in `{}` is reachable but no action can fire from it, and it is not `terminal` — a stuck state. Add an action to leave it, or mark it `terminal`.", d.name)),
                    ));
                }
            }
        }
    }
    out
}

/// Dead enum states: a declared enum value that no `init` and no action `ensures` ever produces is
/// unreachable — a spec smell (a lifecycle state that can never be entered). Conservative: a value that
/// appears anywhere in an init/ensures body is considered producible, so only a truly-never-mentioned
/// value is flagged.
pub fn dead_states(module: &Module, src: &str) -> Vec<Diagnostic> {
    fn collect_names(e: &Expr, out: &mut HashSet<String>) {
        match e {
            Expr::Name(n) => {
                out.insert(n.clone());
            }
            Expr::App { head, args } => {
                collect_names(head, out);
                args.iter().for_each(|a| collect_names(a, out));
            }
            Expr::Field { base, .. } => collect_names(base, out),
            Expr::Unary { e, .. } => collect_names(e, out),
            Expr::Binary { lhs, rhs, .. } => {
                collect_names(lhs, out);
                collect_names(rhs, out);
            }
            Expr::Cond { cond, then_, els } => {
                collect_names(cond, out);
                collect_names(then_, out);
                collect_names(els, out);
            }
            Expr::Quant { body, .. } | Expr::Sum { body, .. } => collect_names(body, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for d in &module.decls {
        let evals = enum_values_of(d, src, false);
        if evals.is_empty() {
            continue;
        }
        // Values any init/ensures can produce (over-approximated by every name that appears there), and
        // the enum observables an init/action actually WRITES (only those are lifecycle states — an enum
        // input that no action sets has all its values valid, so it must not be flagged).
        let enum_names: HashSet<String> = evals.keys().cloned().collect();
        let mut produced: HashSet<String> = HashSet::new();
        let mut written: HashSet<String> = HashSet::new();
        for it in &d.items {
            let spans = match it.kind {
                ItemKind::Init => it.body.into_iter().collect::<Vec<_>>(),
                ItemKind::Action => it.ensures.clone(),
                _ => Vec::new(),
            };
            for sp in spans {
                let t = sp.slice(src);
                let t = t.trim().strip_prefix("means").unwrap_or(t);
                let e = parse_predicate(t).0;
                collect_names(&e, &mut produced);
                collect_writes(&e, false, &enum_names, &mut written);
            }
        }
        for (obs, tags) in &evals {
            if !written.contains(obs) {
                continue; // an enum input, not a lifecycle state — all its values are valid
            }
            for tag in tags {
                if !produced.contains(tag) {
                    out.push(Diagnostic::warning(
                        d.span,
                        format!("enum value `{tag}` of `{obs}` in `{}` is never produced by `init` or any action — it is unreachable (a dead lifecycle state). Add an action that reaches it, or remove the value.", d.name),
                    ));
                }
            }
        }
    }
    out
}

/// Correct-by-construction check for sum/variant state: a payload field may only be READ where its
/// discriminant is known to hold. `outputs` of `outcome : { success { outputs } | failure }` is present
/// only when `outcome = success`, so reading `outputs(e)` outside that guard is ill-formed. This moves the
/// "the field exists here" obligation from a proof into a well-formedness fact, which is why a sum type is
/// stronger than a status field with per-field presence: the illegal read cannot be written.
pub fn variant_access(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let fields = variant_fields_of(d, src);
        if fields.is_empty() {
            continue;
        }
        for it in &d.items {
            // An action's `requires` establishes facts for its `ensures`; other items stand alone.
            let (established0, bodies): (HashSet<String>, Vec<crate::span::Span>) = match it.kind {
                ItemKind::Action => {
                    let est = it.requires.map(|sp| discriminant_facts(&parse_predicate(sp.slice(src)).0)).unwrap_or_default();
                    (est, it.ensures.clone())
                }
                _ => (HashSet::new(), it.body.into_iter().collect()),
            };
            for sp in bodies {
                let e = parse_predicate(sp.slice(src)).0;
                check_access(&e, &established0, &fields, it.span, &d.name, &mut out);
            }
        }
    }
    out
}

/// The discriminant equalities an expression GUARANTEES (usable as established facts downstream): a bare
/// `disc = tag`, or a conjunction of them. Implication, disjunction and negation guarantee nothing.
fn discriminant_facts(e: &Expr) -> HashSet<String> {
    match e {
        Expr::Binary { op: BinOp::Eq, .. } => [canon(e)].into_iter().collect(),
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            let mut s = discriminant_facts(lhs);
            s.extend(discriminant_facts(rhs));
            s
        }
        _ => HashSet::new(),
    }
}

/// Walk `e`, and for every read of a variant payload field verify its discriminant guard is established.
fn check_access(e: &Expr, est: &HashSet<String>, fields: &HashMap<String, (String, String)>, span: crate::span::Span, comp: &str, out: &mut Vec<Diagnostic>) {
    match e {
        Expr::App { head, args } => {
            if let Expr::Name(h) = &**head {
                if let Some((disc, tag)) = fields.get(h) {
                    // Required guard: `disc(args) = tag` with the field's own args.
                    let guard = Expr::Binary {
                        op: BinOp::Eq,
                        lhs: Box::new(Expr::App { head: Box::new(Expr::Name(disc.clone())), args: args.clone() }),
                        rhs: Box::new(Expr::Name(tag.clone())),
                    };
                    if !est.contains(&canon(&guard)) {
                        out.push(Diagnostic::error(
                            span,
                            format!("in `{comp}`: field `{h}` is only present when `{}`, but it is read without that guard. Guard the access (e.g. `{} implies …`).", canon(&guard), canon(&guard)),
                        ));
                    }
                }
            }
            for a in args {
                check_access(a, est, fields, span, comp, out);
            }
        }
        Expr::Binary { op: BinOp::Implies, lhs, rhs } => {
            check_access(lhs, est, fields, span, comp, out);
            let mut est2 = est.clone();
            est2.extend(discriminant_facts(lhs));
            check_access(rhs, &est2, fields, span, comp, out);
        }
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            let mut for_l = est.clone();
            for_l.extend(discriminant_facts(rhs));
            let mut for_r = est.clone();
            for_r.extend(discriminant_facts(lhs));
            check_access(lhs, &for_l, fields, span, comp, out);
            check_access(rhs, &for_r, fields, span, comp, out);
        }
        Expr::Binary { lhs, rhs, .. } => {
            check_access(lhs, est, fields, span, comp, out);
            check_access(rhs, est, fields, span, comp, out);
        }
        Expr::Unary { e, .. } => check_access(e, est, fields, span, comp, out),
        Expr::Quant { body, .. } | Expr::Sum { body, .. } => check_access(body, est, fields, span, comp, out),
        Expr::Cond { cond, then_, els } => {
            check_access(cond, est, fields, span, comp, out);
            let mut est2 = est.clone();
            est2.extend(discriminant_facts(cond));
            check_access(then_, &est2, fields, span, comp, out);
            check_access(els, est, fields, span, comp, out);
        }
        Expr::Field { base, .. } => check_access(base, est, fields, span, comp, out),
        _ => {}
    }
}

/// Enum-typed state/given observables and their declared value (variant tag) sets. An inline enum
/// `{ a | b | c }` gives `{a, b, c}`; a sum `{ ok { x : T } | err }` gives `{ok, err}` (tags only). The
/// SAT encoder uses this to make an enum observable take exactly one value. `with_primes` adds `X'`.
pub(crate) fn enum_values_of(d: &crate::ast::Decl, src: &str, with_primes: bool) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    for it in d.items.iter().filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given)) {
        let (Some(name), Some(bsp)) = (&it.name, it.body) else { continue };
        if let Some(variants) = parse_variants(bsp.slice(src).trim()) {
            let tags: Vec<String> = variants.into_iter().map(|(t, _)| t).collect();
            if with_primes {
                out.insert(format!("{name}'"), tags.clone());
            }
            out.insert(name.clone(), tags);
        }
    }
    out
}

/// Variant PAYLOAD fields across a declaration: field name -> (discriminant observable, required tag).
/// A payload field `outputs` of `outcome : { success { outputs : Text } | failure }` is present only when
/// `outcome = success`; reading it elsewhere is ill-formed (a correct-by-construction guard).
pub(crate) fn variant_fields_of(d: &crate::ast::Decl, src: &str) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    for it in d.items.iter().filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given)) {
        let (Some(name), Some(bsp)) = (&it.name, it.body) else { continue };
        if let Some(variants) = parse_variants(bsp.slice(src).trim()) {
            for (tag, fields) in variants {
                for (fname, _fty) in fields {
                    out.insert(fname, (name.clone(), tag.clone()));
                }
            }
        }
    }
    out
}

/// Variant PAYLOAD field types across a declaration: field name -> declared type text. So the arithmetic
/// tiers can treat a numeric payload (`out` of `{ success { out : Number } | … }`) as a numeric state.
pub(crate) fn variant_field_types(d: &crate::ast::Decl, src: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for it in d.items.iter().filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given)) {
        let Some(bsp) = it.body else { continue };
        if let Some(variants) = parse_variants(bsp.slice(src).trim()) {
            for (_tag, fields) in variants {
                for (fname, fty) in fields {
                    out.insert(fname, fty);
                }
            }
        }
    }
    out
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
        // Enum observables and their value sets (with primed post versions), so the encoder makes an enum
        // status a one-of-N state, and enum-equality invariants (`status = shipped`) are checkable.
        let evals = enum_values_of(d, src, true);
        let enum_names: HashSet<String> = evals.keys().cloned().collect();
        // Each invariant reduced to an entity-normalised boolean body (plain, or a single-variable
        // universal). Arithmetic, multi-entity, and existential invariants are skipped (sound).
        let mut invariants: Vec<(String, Expr)> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Invariant)
            .filter_map(|it| {
                let sp = it.body?;
                let inv = parse_predicate(sp.slice(src)).0;
                checkable_invariant_e(&inv, &bool_base, &all_obs, &enum_names)
                    .map(|e| (it.name.clone().unwrap_or_else(|| "<anon>".into()), e))
            })
            .collect();
        // `terminal <cond>` desugars to a finality invariant: once the state holds, it is never left.
        // `old(cond) implies cond` is exactly the transition/finality form the preservation pass checks.
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Terminal) {
            let Some(sp) = it.body else { continue };
            let cond = parse_predicate(sp.slice(src)).0;
            let finality = Expr::Binary {
                op: BinOp::Implies,
                lhs: Box::new(Expr::Unary { op: UnOp::Old, e: Box::new(cond.clone()) }),
                rhs: Box::new(cond.clone()),
            };
            if let Some(e) = checkable_invariant_e(&finality, &bool_base, &all_obs, &enum_names) {
                invariants.push((format!("terminal[{}]", canon(&cond)), e));
            }
        }
        if invariants.is_empty() {
            continue;
        }

        // Relies (Decision 2): a boolean/enum rely is a pre-state hypothesis in the preservation VC, so an
        // invariant preserved only under it is not false-flagged. It is ASSUMED (trusted, not checked), so
        // the verdict is reported conditional on it — never silently certified unconditional. (An arithmetic
        // rely reduces in the arithmetic tier instead, which notes it there; each rely lands in one tier.)
        let relies: Vec<(String, Expr)> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Rely)
            .filter_map(|it| {
                let sp = it.body?;
                let inv = parse_predicate(sp.slice(src)).0;
                checkable_invariant_e(&inv, &bool_base, &all_obs, &enum_names)
                    .map(|e| (it.name.clone().unwrap_or_else(|| "<rely>".into()), e))
            })
            .collect();
        if !relies.is_empty() {
            let mut names: Vec<String> = relies.iter().map(|(n, _)| n.clone()).collect();
            names.sort();
            out.push(Diagnostic::warning(
                d.span,
                format!("preservation in `{}` is conditional on assumed rely(s): {} (assumed — trusted, not checked; not an unconditional guarantee).", d.name, names.join(", ")),
            ));
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
            // Project init onto its boolean/enum fragment, dropping arithmetic conjuncts (e.g. `paid = 0`).
            // A boolean/enum invariant's establishment cannot depend on an arithmetic init fact, so this is
            // sound and lets enum terminals be certified even when init also pins numeric state.
            .and_then(|e| boolean_project(&e, &bool_base, &all_obs, &enum_names))
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
                // At the initial state there is no prior step, so `old(X)` reads the current `X` (a
                // stutter). Stripping `old` gives that: a transition invariant like `old(settled) implies
                // settled` becomes `settled implies settled`, trivially established, rather than a false
                // init-violation from treating `old settled` as a free atom.
                let inv0 = strip_old(inv);
                let neg = Expr::Unary { op: UnOp::Not, e: Box::new(inv0) };
                if let Some(m) = crate::sat::satisfiable_enum(&[init, &neg], &bnames, &evals) {
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
            let ensures_raw = match it.ensures_expr(src) {
                Some(e) => e,
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
                // Relies are assumed to hold in the pre-state — extra hypotheses that exclude pre-states the
                // environment guarantees against (Decision 2). Sound: adding a trusted assumption can only
                // remove spurious breaks, and the assumption is surfaced in the conditional note above.
                for (_, r) in &relies {
                    es.push(r);
                }
                if let Some(g) = &guard {
                    es.push(g);
                }
                if let Some(m) = crate::sat::satisfiable_enum(&es, &bnames, &evals) {
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
    boolean_fragment_e(e, bool_names, obs, &HashSet::new())
}

/// Project a conjunction onto its boolean/enum-fragment conjuncts, dropping arithmetic ones. None if
/// nothing survives (a purely arithmetic predicate). Used to keep the boolean/enum part of a mixed `init`
/// so enum/boolean invariants can still be certified from it.
fn boolean_project(e: &Expr, bool_names: &HashSet<String>, obs: &HashSet<String>, enum_names: &HashSet<String>) -> Option<Expr> {
    if let Expr::Binary { op: BinOp::And, lhs, rhs } = e {
        return match (
            boolean_project(lhs, bool_names, obs, enum_names),
            boolean_project(rhs, bool_names, obs, enum_names),
        ) {
            (Some(l), Some(r)) => Some(Expr::Binary { op: BinOp::And, lhs: Box::new(l), rhs: Box::new(r) }),
            (Some(x), None) | (None, Some(x)) => Some(x),
            (None, None) => None,
        };
    }
    boolean_fragment_e(e, bool_names, obs, enum_names).then(|| e.clone())
}

/// True if `e` is `enum_obs(args) = value` (or `<>`) — an equality between a declared enum observable and
/// a bare value name, which is a decidable proposition (the SAT engine adds the exactly-one-value axiom).
fn is_enum_eq(e: &Expr, enum_names: &HashSet<String>) -> bool {
    let head = |x: &Expr| {
        // See through `old(...)`: `old(status(e)) = closed` is an enum equality on the pre-state.
        let x = match x {
            Expr::Unary { op: UnOp::Old, e } => &**e,
            other => other,
        };
        match x {
            Expr::App { head, .. } => matches!(&**head, Expr::Name(h) if enum_names.contains(h)),
            Expr::Name(n) => enum_names.contains(n),
            _ => false,
        }
    };
    matches!(e, Expr::Binary { op: BinOp::Eq | BinOp::Ne, lhs, rhs }
        if head(lhs) && matches!(&**rhs, Expr::Name(_)))
}

/// `enum_obs(e) in { a, b, c }` — membership in a finite set of enum tags. The SAT encoder expands it to
/// `= a or = b or = c`, each of which registers the exactly-one axiom, so it is a decidable atom.
fn is_enum_membership(e: &Expr, enum_names: &HashSet<String>) -> bool {
    let Expr::Binary { op: BinOp::In, lhs, rhs } = e else { return false };
    if !matches!(&**rhs, Expr::SetLit(_)) {
        return false;
    }
    match &**lhs {
        Expr::App { head, .. } => matches!(&**head, Expr::Name(h) if enum_names.contains(h)),
        Expr::Name(n) => enum_names.contains(n),
        _ => false,
    }
}

/// As [`boolean_fragment`], additionally admitting enum-observable equalities `status(e) = paid` as
/// decidable atoms (the SAT engine constrains an enum observable to exactly one value).
fn boolean_fragment_e(e: &Expr, bool_names: &HashSet<String>, obs: &HashSet<String>, enum_names: &HashSet<String>) -> bool {
    if is_enum_eq(e, enum_names) || is_enum_membership(e, enum_names) {
        return true;
    }
    match e {
        Expr::Int(_) | Expr::Dec(_, _) | Expr::Cond { .. } | Expr::Quant { .. } | Expr::Sum { .. } => false,
        Expr::Name(_) => true, // a bound entity variable or bool literal
        Expr::App { head, args } => {
            let head_ok = match &**head {
                Expr::Name(h) => !obs.contains(h) || bool_names.contains(h),
                _ => boolean_fragment_e(head, bool_names, obs, enum_names),
            };
            head_ok && args.iter().all(|a| boolean_fragment_e(a, bool_names, obs, enum_names))
        }
        Expr::Field { name, base } => (!obs.contains(name) || bool_names.contains(name)) && boolean_fragment_e(base, bool_names, obs, enum_names),
        Expr::Unary { e, .. } => boolean_fragment_e(e, bool_names, obs, enum_names),
        Expr::Binary { op, lhs, rhs } => match op {
            BinOp::And | BinOp::Or | BinOp::Implies => boolean_fragment_e(lhs, bool_names, obs, enum_names) && boolean_fragment_e(rhs, bool_names, obs, enum_names),
            BinOp::Eq | BinOp::Ne => {
                crate::sat::is_bool_valued(lhs, bool_names) && crate::sat::is_bool_valued(rhs, bool_names)
                    && boolean_fragment_e(lhs, bool_names, obs, enum_names)
                    && boolean_fragment_e(rhs, bool_names, obs, enum_names)
            }
            _ => false, // ordering comparisons and arithmetic operators
        },
        _ => false,
    }
}

/// A suggested guard: the weakest precondition, the invariant with each written observable replaced by
/// the value the action gives it, then simplified. `requires <that>` makes the action preserve the
/// invariant. Falls back to a generic hint when the effect is not a simple assignment we can invert.
pub(crate) fn guard_suggestion(inv: &Expr, ensures: &Expr, modified: &HashSet<String>) -> String {
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

/// Refinement checking (the abstraction ladder's glue). A `component X satisfies (_ : C)` claims that X's
/// behaviour honours contract C. This verifies the standard interface-refinement reading: X's own
/// guarantees (its invariants/axioms/guarantees) must ENTAIL every promise C makes. For each promise P,
/// `X_guarantees ∧ ¬P` is checked unsatisfiable — if so X guarantees P; if not, refinement fails with a
/// witness. Lets each layer be verified alone and the abstract contract be trusted without reading the
/// detail. Boolean fragment (single entity); other promises are reported as not-statically-checked.
/// NOTE (for the human): the *semantics* of `satisfies` — entailment here vs behavioural simulation — is a
/// design decision; this ships the entailment reading with a proposal note (ladders/REFINEMENT-NOTE.md).
/// A contract decl's checkable surface: promises, boolean names, and state/given type text. Shared by the
/// refinement pass (for local contracts) and `extract_contracts` (for imported ones).
pub(crate) fn contract_promises_of(c: &crate::ast::Decl, src: &str) -> crate::arith::ContractPromises {
    let ps = c
        .items
        .iter()
        .filter(|it| matches!(it.kind, ItemKind::Invariant | ItemKind::Axiom | ItemKind::Guarantee))
        .filter_map(|it| Some((it.name.clone().unwrap_or_else(|| "<anon>".into()), parse_predicate(it.body?.slice(src)).0)))
        .collect();
    let c_st: HashMap<String, String> = c
        .items
        .iter()
        .filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given))
        .filter_map(|it| Some((it.name.clone()?, it.body?.slice(src).trim().to_string())))
        .collect();
    (ps, bool_names_of(c, src), c_st)
}

/// Every contract in a module, pre-extracted for consumers that `use` it (see [`crate::arith::Imports`]).
pub fn extract_contracts(source: &str) -> HashMap<String, crate::arith::ContractPromises> {
    let module = crate::parse(source).module;
    module
        .decls
        .iter()
        .filter(|d| d.kind == crate::ast::DeclKind::Contract)
        .map(|c| (c.name.clone(), contract_promises_of(c, source)))
        .collect()
}

pub fn refinement(module: &Module, src: &str, imports: &crate::arith::Imports) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let promises_of = |name: &str| -> Option<crate::arith::ContractPromises> {
        // Local contract first; fall back to one resolved from an imported module.
        if let Some(c) = module.decls.iter().find(|d| d.name == name) {
            return Some(contract_promises_of(c, src));
        }
        imports.contracts.get(name).cloned()
    };
    for d in &module.decls {
        if d.satisfies.is_empty() {
            continue;
        }
        // X's own guarantees plus its `rely` assumptions (assume-guarantee: X promises its guarantees
        // ASSUMING the environment provides its relies, so the relies are available as hypotheses). The
        // rely names are tracked so the verdict can say the satisfaction is conditional on them.
        let x_guar: Vec<Expr> = d
            .items
            .iter()
            .filter(|it| matches!(it.kind, ItemKind::Invariant | ItemKind::Axiom | ItemKind::Guarantee | ItemKind::Rely))
            .filter_map(|it| Some(normalize(&parse_predicate(it.body?.slice(src)).0)))
            .collect();
        let x_relies: Vec<String> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Rely)
            .filter_map(|it| it.name.clone())
            .collect();
        let x_bool = bool_names_of(d, src);
        // Numeric type map spanning X (and, added per-contract below, C), for linear-arithmetic promises.
        let x_st: HashMap<String, String> = d
            .items
            .iter()
            .filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given))
            .filter_map(|it| Some((it.name.clone()?, it.body?.slice(src).trim().to_string())))
            .collect();
        // The component's own `given` definitions, used as the VOCABULARY MAPPING between the abstract
        // contract and the detail: a promise phrased in abstract terms (`funded`) is rewritten into detail
        // terms (`cash_moved and sec_moved`) by inlining these before the entailment check. Reuses the
        // existing `given … means …` construct — no new syntax — so layers may use different words.
        let x_defs: HashMap<String, (Vec<String>, Expr)> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Given && it.body.is_some())
            .filter_map(|it| {
                let name = it.name.clone()?;
                let body = parse_predicate(it.body?.slice(src)).0;
                if it.params.is_empty() && !crate::monitor::is_computation(&body) {
                    return None; // a type annotation, not a definition
                }
                Some((name, (it.params.clone(), body)))
            })
            .collect();

        let kw = if d.kind == crate::ast::DeclKind::Contract { "contract" } else { "component" };
        for sat in &d.satisfies {
            let cname = &sat.ty;
            let (promises, c_bool, _c_st) = match promises_of(cname) {
                Some(x) => x,
                None => {
                    out.push(Diagnostic::warning(d.span, format!("{kw} `{}` claims to satisfy `{}`, but no such contract is declared.", d.name, cname)));
                    continue;
                }
            };
            let mut bnames: HashSet<String> = x_bool.union(&c_bool).cloned().collect();
            bnames.extend(bnames.clone().into_iter().map(|n| format!("{n}'")));
            let all_obs: HashSet<String> = x_bool.union(&c_bool).cloned().collect();

            let mut entailed = Vec::new();
            let mut failed = Vec::new();
            let mut skipped = Vec::new();
            for (pname, promise) in &promises {
                // Rewrite the promise from abstract to detail vocabulary via the component's definitions.
                let mapped = crate::monitor::inline_defs(promise, &x_defs);
                let pn = normalize(&mapped);
                // Only the boolean fragment for now; classify others out honestly.
                let body = universal_body(&pn).map(|(_, b)| b).unwrap_or_else(|| pn.clone());
                if !has_quant(&body) && boolean_fragment_rel(&body, &x_bool, &all_obs) {
                    // Boolean promise: entailment via SAT.
                    let neg = Expr::Unary { op: UnOp::Not, e: Box::new(body.clone()) };
                    let mut es: Vec<&Expr> = x_guar.iter().collect();
                    es.push(&neg);
                    match crate::sat::satisfiable(&es, &bnames) {
                        None => entailed.push(pname.clone()),
                        Some(_) => failed.push(pname.clone()),
                    }
                } else {
                    // Linear-arithmetic promise: entailment via the simplex. The type map spans X and C.
                    let mut st = x_st.clone();
                    st.extend(_c_st.clone());
                    // A state-guarded promise (`status = active implies balance >= min`) is checked by
                    // case-splitting on the finite guard (the SMT rung); a plain linear promise directly.
                    let verdict = crate::arith::entails_guarded_linear(&x_guar, &pn, &st)
                        .or_else(|| crate::arith::entails_linear(&x_guar, &pn, &st));
                    match verdict {
                        Some(true) => entailed.push(pname.clone()),
                        Some(false) => failed.push(pname.clone()),
                        None => skipped.push(pname.clone()),
                    }
                }
            }
            if failed.is_empty() && skipped.is_empty() && !entailed.is_empty() {
                // The guarantee is conditional on the entailing invariants holding. For a state machine
                // that condition is discharged by the preservation pass (each invariant proved INDUCTIVE);
                // for a purely declarative component the invariants are assumed. Say so, so the reader
                // knows the guarantee's footing rather than over-reading "SATISFIES".
                let is_machine = d.items.iter().any(|it| it.kind == ItemKind::Action);
                let footing = if is_machine {
                    " Provided those invariants are maintained (the preservation pass checks each action; see any findings above), the contract holds in every reachable state."
                } else {
                    " This holds wherever those invariants hold (a declarative component; no actions to check for preservation)."
                };
                let assuming = if x_relies.is_empty() {
                    String::new()
                } else {
                    format!(" Assuming its rely-conditions ({}), which the environment must provide.", x_relies.join(", "))
                };
                out.push(Diagnostic::warning(d.span, format!("{kw} `{}` SATISFIES contract `{}`: its invariants entail every promise ({}).{}{}", d.name, cname, entailed.join(", "), assuming, footing)));
            } else {
                for f in &failed {
                    out.push(Diagnostic::warning(d.span, format!("{kw} `{}` does NOT satisfy contract `{}`: promise `{}` is not entailed by its invariants — the detailed layer does not guarantee the abstract contract.", d.name, cname, f)));
                }
                if !entailed.is_empty() {
                    out.push(Diagnostic::warning(d.span, format!("{kw} `{}` vs contract `{}`: entailed {}.", d.name, cname, entailed.join(", "))));
                }
                for s in &skipped {
                    out.push(Diagnostic::warning(d.span, format!("{kw} `{}` vs contract `{}`: promise `{}` not statically checked (outside the boolean fragment).", d.name, cname, s)));
                }
            }
        }
    }
    out
}

/// The reasoning tier an invariant needs, decided syntactically (cheap and sound: it only classifies; the
/// proofs come from the other passes). This is the "which rung" of the reasoning ladder, made explicit so
/// coverage is transparent rather than silently skipped.
enum Tier {
    /// Boolean logic (flags, guards, entity equality) — the SAT rung.
    Boolean,
    /// Linear arithmetic — the LRA rung (add, subtract, multiply-by-constant, comparisons, sums).
    LinearArith,
    /// Beyond the statically-decidable rungs we ship; carry the reason and the runtime fallback.
    NotStatic(String),
}

/// Names of numeric-typed state/given items in a declaration (for classifying arithmetic as linear).
fn numeric_names_of(d: &crate::ast::Decl, src: &str) -> HashSet<String> {
    d.items
        .iter()
        .filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given))
        .filter_map(|it| {
            let name = it.name.clone()?;
            crate::arith::numeric(it.body?.slice(src).trim()).then_some(name)
        })
        .collect()
}

/// Does `e` mention a numeric variable (an application/name whose head is a numeric state/given)?
fn has_num_var(e: &Expr, num: &HashSet<String>) -> bool {
    match e {
        Expr::Name(n) => num.contains(n),
        Expr::App { head, args } => matches!(&**head, Expr::Name(h) if num.contains(h)) || has_num_var(head, num) || args.iter().any(|a| has_num_var(a, num)),
        Expr::Field { name, base } => num.contains(name) || has_num_var(base, num),
        Expr::Unary { e, .. } => has_num_var(e, num),
        Expr::Binary { lhs, rhs, .. } => has_num_var(lhs, num) || has_num_var(rhs, num),
        Expr::Cond { cond, then_, els } => has_num_var(cond, num) || has_num_var(then_, num) || has_num_var(els, num),
        _ => false,
    }
}

/// The first nonlinear construct in `e`, if any: a product of two unknowns, a division by an unknown, or a
/// power. These are outside the linear-arithmetic rung and are checked only at runtime by `monitor`.
fn nonlinear_reason(e: &Expr, num: &HashSet<String>) -> Option<String> {
    match e {
        Expr::Binary { op: BinOp::Pow, .. } => Some("a power (`^`)".into()),
        Expr::Binary { op: BinOp::Mul, lhs, rhs } if has_num_var(lhs, num) && has_num_var(rhs, num) => {
            Some(format!("a product of two unknowns (`{}`)", canon(e)))
        }
        Expr::Binary { op: BinOp::Div, lhs, rhs } if has_num_var(rhs, num) => {
            let _ = lhs;
            Some(format!("a division by an unknown (`{}`)", canon(e)))
        }
        Expr::Binary { lhs, rhs, .. } => nonlinear_reason(lhs, num).or_else(|| nonlinear_reason(rhs, num)),
        Expr::Unary { e, .. } => nonlinear_reason(e, num),
        Expr::Cond { cond, then_, els } => nonlinear_reason(cond, num).or_else(|| nonlinear_reason(then_, num)).or_else(|| nonlinear_reason(els, num)),
        Expr::App { args, .. } => args.iter().find_map(|a| nonlinear_reason(a, num)),
        Expr::Sum { body, .. } => nonlinear_reason(body, num),
        _ => None,
    }
}

/// Classify an invariant by the rung it needs. Existential quantifiers and 3+ entity variables are honestly
/// reported as not-statically-covered; a nonlinear term names its shape; otherwise boolean or linear.
fn classify(inv: &Expr, bool_base: &HashSet<String>, all_obs: &HashSet<String>, num: &HashSet<String>, enum_names: &HashSet<String>) -> Tier {
    // Peel universal quantifiers; an existential anywhere is beyond the bounded static fragment.
    if contains_existential(inv) {
        return Tier::NotStatic("an existential quantifier (`some`/`exists`)".into());
    }
    let (vars, body) = universal_body(inv).unwrap_or_else(|| (Vec::new(), inv.clone()));
    if vars.len() > 2 {
        return Tier::NotStatic(format!("{}-way quantification (static support covers up to two entities)", vars.len()));
    }
    // An enum-fragment invariant (booleans plus enum-value equalities) is decided by SAT with the
    // exactly-one-value axiom, so it belongs at the boolean tier, not the arithmetic one.
    if boolean_fragment_e(&body, bool_base, all_obs, enum_names) {
        return Tier::Boolean;
    }
    if let Some(r) = nonlinear_reason(&body, num) {
        return Tier::NotStatic(r);
    }
    if boolean_fragment_rel(&body, bool_base, all_obs) {
        Tier::Boolean
    } else {
        Tier::LinearArith
    }
}

/// Does `e` contain an existential/exists-one quantifier anywhere?
fn contains_existential(e: &Expr) -> bool {
    match e {
        Expr::Quant { q, body, .. } => !matches!(q, Quant::Every | Quant::No) || contains_existential(body),
        Expr::Binary { lhs, rhs, .. } => contains_existential(lhs) || contains_existential(rhs),
        Expr::Unary { e, .. } => contains_existential(e),
        Expr::Cond { cond, then_, els } => contains_existential(cond) || contains_existential(then_) || contains_existential(els),
        Expr::App { args, .. } => args.iter().any(contains_existential),
        Expr::Sum { body, .. } => contains_existential(body),
        _ => false,
    }
}

/// Transparent tiering. For each component, report which reasoning rung each invariant was checked at, and
/// name every invariant that falls outside the statically-decidable rungs together with the reason and the
/// runtime fallback. This turns silent skips into an explicit coverage account: the spec stays natural, and
/// the tool says exactly what it proved and what it could not reach. The "no vacuous filler" promise.
pub fn tier_report(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let bool_base = bool_names_of(d, src);
        let all_obs: HashSet<String> =
            d.items.iter().filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given)).filter_map(|it| it.name.clone()).collect();
        let num = numeric_names_of(d, src);
        let enum_names: HashSet<String> = enum_values_of(d, src, false).into_keys().collect();
        let invs: Vec<(String, Expr)> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Invariant)
            .filter_map(|it| Some((it.name.clone()?, parse_predicate(it.body?.slice(src)).0)))
            .collect();
        if invs.is_empty() {
            continue;
        }
        let mut boolean = Vec::new();
        let mut linear = Vec::new();
        let mut runtime = Vec::new();
        for (name, inv) in &invs {
            match classify(inv, &bool_base, &all_obs, &num, &enum_names) {
                Tier::Boolean => boolean.push(name.clone()),
                Tier::LinearArith => linear.push(name.clone()),
                Tier::NotStatic(reason) => runtime.push(format!("{name} ({reason})")),
            }
        }
        let mut parts = Vec::new();
        if !boolean.is_empty() {
            parts.push(format!("boolean tier: {}", boolean.join(", ")));
        }
        if !linear.is_empty() {
            parts.push(format!("linear-arithmetic tier: {}", linear.join(", ")));
        }
        if !runtime.is_empty() {
            parts.push(format!("NOT statically checked, verify with `monitor` against real traces: {}", runtime.join("; ")));
        }
        out.push(Diagnostic::warning(
            d.span,
            format!("analysis coverage for `{}` ({} invariant(s)) — {}.", d.name, invs.len(), parts.join(" | ")),
        ));
    }
    out
}

/// A second canonical entity, distinct from `ENT`, for relational (two-entity) invariants.
const ENT2: &str = "_f";

/// Leading universally-quantified variables of `inv` and the quantifier-free body beneath them, or `None`
/// if `inv` is not a run of `every`s over a QF body. Handles both `every a, b :: …` and nested `every a ::
/// every b :: …`.
pub(crate) fn universal_body(inv: &Expr) -> Option<(Vec<String>, Expr)> {
    match inv {
        Expr::Quant { q: Quant::Every, vars, body, .. } => {
            let mut vs = vars.clone();
            match universal_body(body) {
                Some((mut more, inner)) => {
                    vs.append(&mut more);
                    Some((vs, inner))
                }
                None if !has_quant(body) => Some((vs, (**body).clone())),
                None => None,
            }
        }
        _ => None,
    }
}

/// Rename bare variable names via `map`.
pub(crate) fn rename_vars(e: &Expr, map: &HashMap<String, String>) -> Expr {
    match e {
        Expr::Name(n) => Expr::Name(map.get(n).cloned().unwrap_or_else(|| n.clone())),
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(rename_vars(e, map)) },
        Expr::Binary { op, lhs, rhs } => Expr::Binary { op: op.clone(), lhs: Box::new(rename_vars(lhs, map)), rhs: Box::new(rename_vars(rhs, map)) },
        Expr::App { head, args } => Expr::App { head: Box::new(rename_vars(head, map)), args: args.iter().map(|a| rename_vars(a, map)).collect() },
        Expr::Field { base, name } => Expr::Field { base: Box::new(rename_vars(base, map)), name: name.clone() },
        Expr::Cond { cond, then_, els } => Expr::Cond { cond: Box::new(rename_vars(cond, map)), then_: Box::new(rename_vars(then_, map)), els: Box::new(rename_vars(els, map)) },
        other => other.clone(),
    }
}

/// Resolve entity equality between the two symbolic entities: `_e = _f` (distinct) becomes `false`,
/// `_e = _e` becomes `true`; likewise `<>`. Leaves boolean-state equalities untouched.
pub(crate) fn resolve_entity_eq(e: &Expr) -> Expr {
    let is_ent = |x: &Expr| matches!(x, Expr::Name(n) if n == ENT || n == ENT2);
    match e {
        Expr::Binary { op: op @ (BinOp::Eq | BinOp::Ne), lhs, rhs } if is_ent(lhs) && is_ent(rhs) => {
            let same = matches!((&**lhs, &**rhs), (Expr::Name(a), Expr::Name(b)) if a == b);
            let val = if matches!(op, BinOp::Eq) { same } else { !same };
            Expr::Name(if val { "true" } else { "false" }.into())
        }
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(resolve_entity_eq(e)) },
        Expr::Binary { op, lhs, rhs } => Expr::Binary { op: op.clone(), lhs: Box::new(resolve_entity_eq(lhs)), rhs: Box::new(resolve_entity_eq(rhs)) },
        Expr::Cond { cond, then_, els } => Expr::Cond { cond: Box::new(resolve_entity_eq(cond)), then_: Box::new(resolve_entity_eq(then_)), els: Box::new(resolve_entity_eq(els)) },
        other => other.clone(),
    }
}

/// Post-state rewrite for a two-entity check: a modified state observable applied to the MODIFIED entity
/// `_e` (outside `old`) is primed; the same observable applied to the framed other entity `_f`, and every
/// unmodified observable, is left at its pre value. `old(X)` reads pre.
pub(crate) fn to_post(e: &Expr, modified: &HashSet<String>, in_old: bool) -> Expr {
    let is_e = |args: &[Expr]| args.len() == 1 && matches!(&args[0], Expr::Name(n) if n == ENT);
    match e {
        Expr::Unary { op: UnOp::Old, e } => to_post(e, modified, true),
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(to_post(e, modified, in_old)) },
        Expr::App { head, args } => {
            let head = match &**head {
                Expr::Name(h) if !in_old && modified.contains(h) && is_e(args) => Box::new(Expr::Name(format!("{h}'"))),
                other => Box::new(to_post(other, modified, in_old)),
            };
            Expr::App { head, args: args.iter().map(|a| to_post(a, modified, in_old)).collect() }
        }
        Expr::Binary { op, lhs, rhs } => Expr::Binary { op: op.clone(), lhs: Box::new(to_post(lhs, modified, in_old)), rhs: Box::new(to_post(rhs, modified, in_old)) },
        Expr::Cond { cond, then_, els } => Expr::Cond { cond: Box::new(to_post(cond, modified, in_old)), then_: Box::new(to_post(then_, modified, in_old)), els: Box::new(to_post(els, modified, in_old)) },
        other => other.clone(),
    }
}

/// Relational (two-entity) safety preservation. For a universal invariant over two entities — uniqueness,
/// mutual exclusion, segregation (`every a :: every b :: (active(a) and active(b)) implies a = b`) — an
/// action that modifies one entity `_e` can break it against some OTHER entity `_f`. We instantiate the
/// invariant at the pairs `(_e, _f)` and `(_f, _e)` with `_f` a distinct symbolic other (its state framed),
/// assume both held pre, and check whether the action's effect on `_e` can make either fail. Sound bounded
/// two-entity instantiation (the action touches only `_e`, so pairs not involving `_e` are unaffected).
/// Boolean fragment only; single-entity invariants are handled by [`preservation`].
pub fn relational_preservation(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let state_names: HashSet<String> =
            d.items.iter().filter(|it| it.kind == ItemKind::State).filter_map(|it| it.name.clone()).collect();
        let all_obs: HashSet<String> =
            d.items.iter().filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given)).filter_map(|it| it.name.clone()).collect();
        let bool_base = bool_names_of(d, src);
        let mut bnames = bool_base.clone();
        for n in bool_base.clone() {
            bnames.insert(format!("{n}'"));
        }

        // Two-entity universal invariants, each as its two ordered instances (pre and unprimed).
        let mut invs: Vec<(String, Expr, Expr)> = Vec::new(); // (name, inst_ef, inst_fe)
        let mut unchecked: Vec<String> = Vec::new(); // 2-entity invariants beyond the boolean fragment
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Invariant) {
            let (name, body) = match (&it.name, it.body) {
                (Some(n), Some(b)) => (n.clone(), parse_predicate(b.slice(src)).0),
                _ => continue,
            };
            let (vars, qf) = match universal_body(&body) {
                Some(x) => x,
                None => continue,
            };
            if vars.len() != 2 {
                continue; // single-entity handled elsewhere; 3+ out of scope
            }
            if !boolean_fragment_rel(&qf, &bool_base, &all_obs) {
                // A two-entity invariant over a numeric key (uniqueness/ordering) is beyond the boolean
                // relational fragment, and nothing else checks it (arith preservation is single-entity).
                // Record it so its preservation is reported as unchecked rather than silently assumed.
                unchecked.push(name);
                continue;
            }
            let map_ef: HashMap<String, String> = [(vars[0].clone(), ENT.into()), (vars[1].clone(), ENT2.into())].into();
            let map_fe: HashMap<String, String> = [(vars[0].clone(), ENT2.into()), (vars[1].clone(), ENT.into())].into();
            let inst_ef = simplify(&resolve_entity_eq(&rename_vars(&qf, &map_ef)));
            let inst_fe = simplify(&resolve_entity_eq(&rename_vars(&qf, &map_fe)));
            invs.push((name, inst_ef, inst_fe));
        }
        // Report any two-entity invariant whose preservation is not checked, so the coverage report does
        // not over-claim. Only meaningful when an action exists that could disturb it.
        let has_action = d.items.iter().any(|it| it.kind == ItemKind::Action);
        if has_action {
            for name in &unchecked {
                out.push(Diagnostic::warning(
                    d.span,
                    format!("relational invariant `{name}` in `{}` is NOT preservation-checked: a two-entity property over a numeric key is beyond the boolean relational fragment, and the arithmetic tier is single-entity. Its consistency may be reported, but no action is verified to preserve it (a rule that duplicates the key would pass silently).", d.name),
                ));
            }
        }
        if invs.is_empty() {
            continue;
        }

        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let ens_raw = match it.ensures_expr(src) {
                Some(e) => e,
                None => continue,
            };
            let grd_raw = it.requires.map(|sp| parse_predicate(sp.slice(src)).0);
            // The action subject is one entity, mapped to _e.
            let mut ev = HashSet::new();
            collect_entity_vars(&ens_raw, &mut ev);
            if let Some(g) = &grd_raw {
                collect_entity_vars(g, &mut ev);
            }
            if ev.len() > 1 {
                continue;
            }
            let subj: HashMap<String, String> = ev.iter().map(|v| (v.clone(), ENT.to_string())).collect();
            let ensures = rename_vars(&ens_raw, &subj);
            let guard = grd_raw.map(|g| rename_vars(&g, &subj));
            let mut modified = HashSet::new();
            collect_writes(&ensures, false, &state_names, &mut modified);
            if modified.is_empty() {
                continue;
            }
            let effect = to_post(&ensures, &modified, false);

            for (iname, inst_ef, inst_fe) in &invs {
                // Only relevant if the action's writes can affect the pair.
                if !mentions_any(inst_ef, &modified) && !mentions_any(inst_fe, &modified) {
                    continue;
                }
                let post_ef = to_post(inst_ef, &modified, false);
                let post_fe = to_post(inst_fe, &modified, false);
                let mut broke = false;
                for post in [&post_ef, &post_fe] {
                    let violation = Expr::Unary { op: UnOp::Not, e: Box::new(post.clone()) };
                    let mut es: Vec<&Expr> = vec![inst_ef, inst_fe, &effect, &violation];
                    if let Some(g) = &guard {
                        es.push(g);
                    }
                    if crate::sat::satisfiable(&es, &bnames).is_some() {
                        broke = true;
                        break;
                    }
                }
                if broke {
                    let aname = it.name.clone().unwrap_or_else(|| "<anon>".into());
                    out.push(Diagnostic::warning(
                        it.span,
                        format!("action `{aname}` in `{}` can break relational invariant `{iname}`: acting on one entity can violate it against another entity. Guard the action so the relation is preserved.", d.name),
                    ));
                }
            }
        }
    }
    out
}

/// Boolean fragment test that additionally allows entity equality (`a = b` between the two bound
/// variables), which relational invariants use and which is resolved to a constant before solving.
fn boolean_fragment_rel(e: &Expr, bool_names: &HashSet<String>, obs: &HashSet<String>) -> bool {
    match e {
        Expr::Binary { op: BinOp::Eq | BinOp::Ne, lhs, rhs } if matches!(&**lhs, Expr::Name(_)) && matches!(&**rhs, Expr::Name(_)) => true,
        Expr::Binary { op: BinOp::And | BinOp::Or | BinOp::Implies, lhs, rhs } => boolean_fragment_rel(lhs, bool_names, obs) && boolean_fragment_rel(rhs, bool_names, obs),
        Expr::Unary { op: UnOp::Not, e } => boolean_fragment_rel(e, bool_names, obs),
        _ => boolean_fragment(e, bool_names, obs),
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
            let ens = match it.ensures_expr(src) {
                Some(e) => normalize(&e),
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
                let raw = parse_predicate(sp.slice(src)).0;
                // Exclude transition invariants (they use `old`): BMC's per-step state model cannot
                // represent `old`, so it would treat it as a free atom and could report a false trace.
                // Action-preservation checks these soundly instead.
                if uses_old_expr(&raw) {
                    return None;
                }
                checkable_invariant(&raw, &bool_base, &all_obs)
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

/// Evaluate an enum/boolean predicate against a concrete single-representative assignment of enum
/// observables to variant tags. Returns None when the expression falls outside the evaluable fragment
/// (arithmetic, aggregates, unresolved names): the caller declines rather than guess.
fn eval_enum(e: &Expr, st: &HashMap<String, String>) -> Option<bool> {
    match e {
        Expr::Binary { op, lhs, rhs } => match op {
            BinOp::And => Some(eval_enum(lhs, st)? && eval_enum(rhs, st)?),
            BinOp::Or => Some(eval_enum(lhs, st)? || eval_enum(rhs, st)?),
            BinOp::Implies => Some(!eval_enum(lhs, st)? || eval_enum(rhs, st)?),
            BinOp::Eq | BinOp::Ne => {
                let obs = atom_head(lhs)?;
                let val = match rhs.as_ref() {
                    Expr::Name(v) => v,
                    _ => return None,
                };
                let eq = st.get(&obs)? == val;
                Some(if *op == BinOp::Eq { eq } else { !eq })
            }
            _ => None,
        },
        Expr::Unary { op: UnOp::Not, e } => Some(!eval_enum(e, st)?),
        // A universal over the single representative entity is just its body; matches how the rest of the
        // analysis reasons over one entity. `some`/`no` are not evaluated on one representative.
        Expr::Quant { q: Quant::Every, body, .. } => eval_enum(body, st),
        Expr::Name(n) if n == "true" => Some(true),
        Expr::Name(n) if n == "false" => Some(false),
        _ => None,
    }
}

/// Collect `obs = tag` enum assignments from a conjunction (an `init` or an action `ensures`). Returns
/// false if any conjunct is not such an assignment over a declared enum observable, so a body mixing in
/// arithmetic or a non-enum effect makes the caller decline the whole declaration.
fn enum_assignments(e: &Expr, evals: &HashMap<String, Vec<String>>, out: &mut HashMap<String, String>) -> bool {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            enum_assignments(lhs, evals, out) && enum_assignments(rhs, evals, out)
        }
        Expr::Binary { op: BinOp::Eq, lhs, rhs } => {
            if let (Some(obs), Expr::Name(val)) = (atom_head(lhs), rhs.as_ref()) {
                if evals.get(&obs).is_some_and(|vs| vs.contains(val)) {
                    out.insert(obs, val.clone());
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Keywords that break predicate parsing when used as a bare value (an enum variant tag). A tag in value
/// position (`status(e) = no`) is read as the start of this keyword's construct, so init/guards parse
/// wrongly and the tier declines with confusing downstream noise. Reject the tag with a pointed message.
const RESERVED_TAGS: &[&str] = &[
    "every", "some", "no", "exists", "not", "and", "or", "implies", "in", "if", "then", "else", "means",
    "old", "sum", "true", "false",
];

/// Flag an enum/variant tag that is a reserved word (`{ yes | no }` — `no` is the negation quantifier).
/// Such a tag silently breaks parsing at every use site; name the tag and the collision directly.
pub fn reserved_tag_check(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        for it in d.items.iter().filter(|it| matches!(it.kind, ItemKind::State | ItemKind::Given)) {
            let (Some(name), Some(bsp)) = (&it.name, it.body) else { continue };
            let Some(variants) = parse_variants(bsp.slice(src).trim()) else { continue };
            for (tag, _) in variants {
                if RESERVED_TAGS.contains(&tag.as_str()) {
                    out.push(Diagnostic::error(
                        it.span,
                        format!("variant tag `{tag}` of `{name}` in `{}` is a reserved word: used as a value (`{name}(e) = {tag}`) it is parsed as the `{tag}` keyword, silently breaking every guard and init that mentions it. Rename the tag.", d.name),
                    ));
                }
            }
        }
    }
    out
}

/// Collect `obs = <tag-valued expr>` action effects from a conjunction. Like [`enum_assignments`] but the
/// RHS may be an `if/then/else` over tags (a branching transition, e.g. `status = if ok then done else
/// failed`), resolved to a concrete tag at fire time by [`resolve_tag`]. Returns false if any conjunct is
/// not such an effect over a declared enum observable.
fn enum_effect(e: &Expr, evals: &HashMap<String, Vec<String>>, out: &mut Vec<(String, Expr)>) -> bool {
    match e {
        Expr::Binary { op: BinOp::And, lhs, rhs } => {
            enum_effect(lhs, evals, out) && enum_effect(rhs, evals, out)
        }
        Expr::Binary { op: BinOp::Eq, lhs, rhs } => {
            if let Some(obs) = atom_head(lhs) {
                if let Some(tags) = evals.get(&obs) {
                    if tag_valued(rhs, tags) {
                        out.push((obs, (**rhs).clone()));
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// True if `e` yields a variant tag: a bare tag name, or an `if/then/else` whose branches do (recursively).
/// The branch conditions are not checked here — [`resolve_tag`] evaluates them against a concrete state.
fn tag_valued(e: &Expr, tags: &[String]) -> bool {
    match e {
        Expr::Name(t) => tags.contains(t),
        Expr::Cond { then_, els, .. } => tag_valued(then_, tags) && tag_valued(els, tags),
        _ => false,
    }
}

/// Resolve a tag-valued effect expression to a concrete tag against a state, evaluating `if/then/else`
/// conditions with [`eval_enum`]. None if a condition cannot be settled from the state.
fn resolve_tag(e: &Expr, st: &HashMap<String, String>) -> Option<String> {
    match e {
        Expr::Name(t) => Some(t.clone()),
        Expr::Cond { cond, then_, els } => {
            if eval_enum(cond, st)? {
                resolve_tag(then_, st)
            } else {
                resolve_tag(els, st)
            }
        }
        _ => None,
    }
}

/// Explicit-state reachability for enum lifecycles — the counterexample-trace complement to preservation,
/// for the state machines the SAT-based [`bmc`] declines (its states must all be boolean). When every
/// `observable state` is enum-typed, `init` pins them all, and actions are guarded enum transitions, this
/// walks the reachable state graph breadth-first from `init` (deduping visited states) and reports the
/// shortest action sequence that reaches a state violating an invariant. Exact, not an over-approximation:
/// a reported trace is a real execution. Silence is not a proof — preservation and k-induction prove; this
/// witnesses. Transition invariants (`old`) are excluded (a single reached state cannot represent `old`).
pub fn bmc_enum(module: &Module, src: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for d in &module.decls {
        let evals = enum_values_of(d, src, false);
        if evals.is_empty() {
            continue;
        }
        let states: Vec<String> =
            d.items.iter().filter(|it| it.kind == ItemKind::State).filter_map(|it| it.name.clone()).collect();
        // Every state must be enum-typed to enumerate the graph; a Money/Number state cannot be walked.
        if states.is_empty() || states.iter().any(|s| !evals.contains_key(s)) {
            continue;
        }
        let init_body = match d.items.iter().find(|it| it.kind == ItemKind::Init).and_then(|it| it.body) {
            Some(sp) => {
                let t = sp.slice(src);
                parse_predicate(t.trim().strip_prefix("means").unwrap_or(t)).0
            }
            None => continue,
        };
        let mut init_state = HashMap::new();
        if !enum_assignments(&normalize(&init_body), &evals, &mut init_state) {
            continue;
        }
        if states.iter().any(|s| !init_state.contains_key(s)) {
            continue; // init leaves a state free: reachability would be an over-approximation
        }

        struct Act {
            name: String,
            guard: Option<Expr>,
            // Each effect is `obs = <tag-valued expr>`, where the RHS is a variant tag or an `if/then/else`
            // over tags: resolved to a concrete tag against the current state when the action fires.
            eff: Vec<(String, Expr)>,
        }
        let mut acts: Vec<Act> = Vec::new();
        let mut ok = true;
        for it in d.items.iter().filter(|it| it.kind == ItemKind::Action) {
            let ens = match it.ensures_expr(src) {
                Some(e) => normalize(&e),
                None => continue,
            };
            let mut eff = Vec::new();
            if !enum_effect(&ens, &evals, &mut eff) {
                ok = false;
                break;
            }
            let guard = it.requires.map(|sp| normalize(&parse_predicate(sp.slice(src)).0));
            acts.push(Act { name: it.name.clone().unwrap_or_else(|| "<anon>".into()), guard, eff });
        }
        if !ok || acts.is_empty() {
            continue;
        }

        let invs: Vec<(String, Expr)> = d
            .items
            .iter()
            .filter(|it| it.kind == ItemKind::Invariant)
            .filter_map(|it| {
                let raw = parse_predicate(it.body?.slice(src)).0;
                if uses_old_expr(&raw) {
                    return None;
                }
                Some((it.name.clone().unwrap_or_else(|| "<anon>".into()), normalize(&raw)))
            })
            .collect();
        if invs.is_empty() {
            continue;
        }

        let key = |st: &HashMap<String, String>| {
            let mut v: Vec<(String, String)> = st.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            v.sort();
            v
        };
        // Render a concrete state as `a = x, b = y` (sorted) for the counterexample.
        let render = |st: &HashMap<String, String>| {
            let mut kv: Vec<String> = st.iter().map(|(k, v)| format!("{k} = {v}")).collect();
            kv.sort();
            kv.join(", ")
        };
        let mut reported: HashSet<String> = HashSet::new();
        // An invariant that eval_enum cannot settle (Some/None) in some reachable state — e.g. its
        // consequent is arithmetic — is neither witnessed nor proven here; arith_preservation owns it.
        let mut indeterminate: HashSet<String> = HashSet::new();
        let mut seen: HashSet<Vec<(String, String)>> = HashSet::new();
        seen.insert(key(&init_state));
        let mut frontier: Vec<(HashMap<String, String>, Vec<String>)> = vec![(init_state, Vec::new())];
        // `closed` becomes true only if the reachable set is fully explored (a step produced no new state)
        // before the depth bound: then the safe invariants hold over *every* reachable state, an exact proof.
        let mut closed = false;
        // Cleared if a conditional effect ever fails to resolve to a tag from a concrete state: the graph
        // is then under-explored, so no exact-proof claim can be made (witnesses stay sound).
        let mut sound_closure = true;
        for depth in 0..=BMC_MAX {
            for (st, path) in &frontier {
                for (iname, inv) in &invs {
                    if reported.contains(iname) {
                        continue;
                    }
                    match eval_enum(inv, st) {
                        Some(false) => {
                            let trace = if path.is_empty() {
                                "init".to_string()
                            } else {
                                format!("init -> {}", path.join(" -> "))
                            };
                            out.push(Diagnostic::warning(
                                d.span,
                                format!(
                                    "invariant `{iname}` in `{}` is REACHABLY VIOLATED in {} step(s): {} reaches {{{}}}, where it fails. A concrete counterexample, not just a non-inductive warning.",
                                    d.name,
                                    path.len(),
                                    trace,
                                    render(st)
                                ),
                            ));
                            reported.insert(iname.clone());
                        }
                        None => {
                            indeterminate.insert(iname.clone());
                        }
                        Some(true) => {}
                    }
                }
            }
            if reported.len() == invs.len() || depth == BMC_MAX {
                break; // all invariants already witnessed, or the bound is reached with states still open
            }
            let mut next = Vec::new();
            for (st, path) in &frontier {
                for act in &acts {
                    let fires = match &act.guard {
                        Some(g) => eval_enum(g, st) == Some(true),
                        None => true,
                    };
                    if !fires {
                        continue;
                    }
                    let mut ns = st.clone();
                    let mut resolved = true;
                    for (obs, rhs) in &act.eff {
                        match resolve_tag(rhs, st) {
                            Some(tag) => {
                                ns.insert(obs.clone(), tag);
                            }
                            None => {
                                resolved = false; // a branch condition we cannot settle from this state
                                break;
                            }
                        }
                    }
                    if !resolved {
                        sound_closure = false;
                        continue;
                    }
                    if seen.insert(key(&ns)) {
                        let mut np = path.clone();
                        np.push(act.name.clone());
                        next.push((ns, np));
                    }
                }
            }
            if next.is_empty() {
                closed = true; // reachable set exhausted within the bound
                break;
            }
            frontier = next;
        }
        // Each invariant with no witnessed violation over the fully-explored reachable set is proven safe.
        if closed && sound_closure {
            for (iname, _) in &invs {
                if !reported.contains(iname) && !indeterminate.contains(iname) {
                    out.push(Diagnostic::warning(
                        d.span,
                        format!(
                            "invariant `{iname}` in `{}` is PROVED SAFE: no reachable state violates it (explicit-state, {} reachable state(s) explored to fixpoint). An exact proof over the whole lifecycle, not a bounded search.",
                            d.name,
                            seen.len()
                        ),
                    ));
                }
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
    checkable_invariant_e(inv, bool_base, obs, &HashSet::new())
}

/// As [`checkable_invariant`], additionally admitting enum-observable equalities as decidable atoms.
fn checkable_invariant_e(inv: &Expr, bool_base: &HashSet<String>, obs: &HashSet<String>, enum_names: &HashSet<String>) -> Option<Expr> {
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
    if !boolean_fragment_e(&body, bool_base, obs, enum_names) {
        return None;
    }
    Some(rename_entity(&body, &ev))
}

/// Replace `old(sub)` with `sub` throughout — the reading at the initial state, where there is no prior
/// step, so `old(X)` is just the current `X` (a stutter). Used to check init-establishment of a
/// transition invariant without treating `old X` as a free atom.
pub(crate) fn strip_old(e: &Expr) -> Expr {
    match e {
        Expr::Unary { op: UnOp::Old, e } => strip_old(e),
        Expr::Unary { op, e } => Expr::Unary { op: op.clone(), e: Box::new(strip_old(e)) },
        Expr::Binary { op, lhs, rhs } => Expr::Binary { op: op.clone(), lhs: Box::new(strip_old(lhs)), rhs: Box::new(strip_old(rhs)) },
        Expr::App { head, args } => Expr::App { head: Box::new(strip_old(head)), args: args.iter().map(strip_old).collect() },
        Expr::Field { base, name } => Expr::Field { base: Box::new(strip_old(base)), name: name.clone() },
        Expr::Cond { cond, then_, els } => Expr::Cond { cond: Box::new(strip_old(cond)), then_: Box::new(strip_old(then_)), els: Box::new(strip_old(els)) },
        other => other.clone(),
    }
}

/// Does `e` mention `old(...)` anywhere? Such an invariant is a two-state TRANSITION invariant (e.g.
/// finality, `old(settled) implies settled`), handled soundly by action-preservation (via `prime`, which
/// maps `old`→pre and bare→post) but NOT by the reachability model of BMC/k-induction, whose per-step
/// state atoms cannot represent `old` — so those passes exclude it rather than risk a false counterexample.
pub(crate) fn uses_old_expr(e: &Expr) -> bool {
    match e {
        Expr::Unary { op: UnOp::Old, .. } => true,
        Expr::Unary { e, .. } => uses_old_expr(e),
        Expr::Binary { lhs, rhs, .. } => uses_old_expr(lhs) || uses_old_expr(rhs),
        Expr::App { head, args } => uses_old_expr(head) || args.iter().any(uses_old_expr),
        Expr::Field { base, .. } => uses_old_expr(base),
        Expr::Cond { cond, then_, els } => uses_old_expr(cond) || uses_old_expr(then_) || uses_old_expr(els),
        Expr::Quant { body, .. } | Expr::Sum { body, .. } => uses_old_expr(body),
        _ => false,
    }
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
        let evals = enum_values_of(d, src, false);

        // Joint satisfiability via the dependency-free SAT engine (scales past enumeration).
        let subset = |active: &[bool]| -> Vec<&Expr> {
            rules.iter().enumerate().filter(|(k, _)| active[*k]).map(|(_, (_, e))| e).collect()
        };
        match crate::sat::satisfiable_enum(&subset(&vec![true; rules.len()]), &bnames, &evals) {
            Some(m) => out.push(Diagnostic::warning(
                d.span,
                format!("rule set in `{}` is jointly satisfiable (e.g. {}).", d.name, crate::sat::describe(&m)),
            )),
            None => {
                // Minimal UNSAT core: drop each rule; keep it only if its removal restores SAT.
                let mut active = vec![true; rules.len()];
                for k in 0..rules.len() {
                    active[k] = false;
                    if crate::sat::satisfiable_enum(&subset(&active), &bnames, &evals).is_some() {
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
        let evals = enum_values_of(d, src, false);

        // Does some report satisfy `req` together with every active axiom? (SAT engine.)
        let sat = |rexpr: &Expr, active: &[bool]| -> Option<BTreeMap<String, bool>> {
            let mut es: Vec<&Expr> = axioms.iter().enumerate().filter(|(k, _)| active[*k]).map(|(_, (_, e))| e).collect();
            es.push(rexpr);
            crate::sat::satisfiable_enum(&es, &bnames, &evals)
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
        // A lifecycle is not a decision table. When every guarded action is a transition — its guard reads
        // an enum state that its own `ensures` writes — disjointness and exhaustiveness of the guards are
        // the wrong properties (overlap is nondeterminism, gaps are terminal states). Stuck-state detection
        // and bmc own the lifecycle; skip the case-split verdict rather than raise a category-error alarm.
        let evals = enum_values_of(d, src, false);
        if !evals.is_empty() {
            let all_transitions = d
                .items
                .iter()
                .filter(|it| it.kind == ItemKind::Action && it.requires.is_some())
                .all(|it| {
                    let guard = parse_predicate(it.requires.unwrap().slice(src)).0;
                    let mut gatoms = BTreeSet::new();
                    collect_atoms(&guard, &mut gatoms);
                    let reads_enum = gatoms.iter().any(|a| evals.keys().any(|e| a.contains(e.as_str())));
                    // Writes at least one enum state — even if it also updates arithmetic state (`settle`
                    // sets `phase = settled and paid = amount`). A lifecycle transition need not be a pure
                    // enum assignment.
                    let writes_enum = it.ensures_expr(src).is_some_and(|e| {
                        let ens = normalize(&e);
                        let mut w = HashSet::new();
                        let enum_states: HashSet<String> = evals.keys().cloned().collect();
                        collect_writes(&ens, false, &enum_states, &mut w);
                        !w.is_empty()
                    });
                    reads_enum && writes_enum
                });
            if all_transitions {
                continue;
            }
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
    fn boolean_rely_is_a_pre_hypothesis_and_reported_conditional() {
        // Decision 2, slice 2: a boolean/enum rely enters the boolean preservation VC as a pre-state
        // hypothesis, so an invariant preserved only under it (here vacuous under `not p`) is not
        // false-flagged; the verdict is reported conditional; dropping the rely re-exposes the break.
        let with = "-- allium: 4\ncomponent C\n  entity E\n  observable state p(E) : Boolean\n  observable state q(E) : Boolean\n  rely env_not_p means every e :: not p(e)\n  invariant q_when_p means every e :: p(e) implies q(e)\n  action clear_q\n    ensures not q(e)\nend\n";
        assert!(!any(with, "can break"), "the rely must be assumed as a pre-hypothesis: {:?}", msgs(with));
        assert!(any(with, "conditional on assumed rely(s): env_not_p"), "the assumed rely must be reported: {:?}", msgs(with));
        let without = with.lines().filter(|l| !l.contains("rely env_not_p")).collect::<Vec<_>>().join("\n");
        assert!(any(&without, "can break invariant `q_when_p`"), "without the rely the break must show: {:?}", msgs(&without));
    }

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
    fn enum_membership_guard_is_expanded_and_checked() {
        // `role in {user,admin} implies active` is a guarded safety invariant. It must be preservation-
        // checked (not silently skipped), and the set literal must expand to real enum tags — a stray
        // brace element (`admin }`) would float free of the exactly-one axiom and false-alarm the guard-off
        // case.
        let hdr = "-- allium: 4\ncomponent C\n  entity X\n  observable state role(X) : { guest | user | admin }\n  observable state active(X) : Boolean\n  init means role(x) = guest and active(x) = false\n  invariant privileged means role(x) in { user, admin } implies active(x)\n";
        // promote into the set without activating breaks it.
        let bad = format!("{hdr}  action promote\n    ensures role(x) = admin\nend\n");
        assert!(any(&bad, "can break invariant `privileged`"), "in-guard break must be caught: {:?}", msgs(&bad));
        // demote out of the set turns the guard off — no false break.
        let good = format!("{hdr}  action demote\n    ensures role(x) = guest\nend\n");
        assert!(!any(&good, "can break"), "guard-off must not false-alarm: {:?}", msgs(&good));
    }

    #[test]
    fn boolean_literal_true_is_a_constant_not_a_free_atom() {
        // `ensures logged(x) = true` must set logged true. Previously `true` was encoded as a free
        // SAT atom, so the solver could pick `true = false` and manufacture a spurious break of a
        // guarded invariant. The action records the fault, so it PRESERVES `fail => logged`.
        let src = "-- allium: 4\ncomponent C\n  entity X\n  observable state outcome(X) : { pass | fail }\n  observable state logged(X) : Boolean\n  invariant note means outcome(x) = fail implies logged(x)\n  action record\n    requires outcome(x) = fail\n    ensures logged(x) = true\nend\n";
        assert!(!any(src, "can break"), "`= true` must not manufacture a break: {:?}", msgs(src));
    }

    #[test]
    fn multiple_ensures_clauses_read_as_their_conjunction() {
        // Separate `ensures` lines are the conjunction of their clauses. A balanced transfer written as
        // three lines must verify PRESERVED — the same as the `and`-joined form — with no parse error.
        let multi = "-- allium: 4\ncomponent Book\n  entity Acct\n  observable state bal(Acct) : Money\n  observable state total : Money\n  invariant conserved means total = sum a :: bal(a)\n  action transfer\n    ensures bal(a) = old(bal(a)) - 100\n    ensures bal(b) = old(bal(b)) + 100\n    ensures total = old(total)\nend\n";
        assert!(!any(multi, "takes a single `ensures` clause"), "multi-ensures must be accepted: {:?}", msgs(multi));
        assert!(any(multi, "is PRESERVED"), "balanced multi-line transfer must be PRESERVED: {:?}", msgs(multi));
        // Every clause participates: a break in the SECOND clause is caught (would be missed if dropped).
        let broken = "-- allium: 4\ncomponent C\n  entity X\n  observable state a(X) : Money\n  observable state b(X) : Money\n  invariant nn means b(x) >= 0\n  action act\n    ensures a(x) = old(a(x)) + 1\n    ensures b(x) = old(b(x)) - 1000000\nend\n";
        assert!(any(broken, "can break arithmetic invariant `nn`"), "the second clause must be checked: {:?}", msgs(broken));
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
    fn relational_preservation_catches_uniqueness_break() {
        // "at most one active": an unguarded acquire that sets `active` can make two distinct holders both
        // active, violating the relation. A deactivating action cannot, and an unrelated write cannot.
        let bad = "-- allium: 4\ncomponent L\n  entity H\n  observable state active(H) : bool\n  action acquire\n    ensures active(h)\n  invariant at_most_one means every a :: every b :: (active(a) and active(b)) implies a = b\nend\n";
        assert!(any(bad, "can break relational invariant `at_most_one`"), "{:?}", msgs(bad));
        let release = "-- allium: 4\ncomponent L\n  entity H\n  observable state active(H) : bool\n  action release\n    requires active(h)\n    ensures not active(h)\n  invariant at_most_one means every a :: every b :: (active(a) and active(b)) implies a = b\nend\n";
        assert!(!any(release, "can break relational"), "deactivation cannot break at-most-one: {:?}", msgs(release));
    }

    #[test]
    fn refinement_entailment_check() {
        // The detailed component's two invariants (settle->cash, cash->funded) entail the contract's
        // promise (settled->funded); dropping the second breaks refinement.
        let ok = "-- allium: 4\ncontract Settle\n  entity T\n  observable state settled(T) : bool\n  observable state funded(T) : bool\n  guarantee sif means settled(t) implies funded(t)\nend\ncomponent Impl satisfies (s : Settle)\n  entity T\n  observable state settled(T) : bool\n  observable state cash(T) : bool\n  observable state funded(T) : bool\n  invariant a means settled(t) implies cash(t)\n  invariant b means cash(t) implies funded(t)\nend\n";
        assert!(any(ok, "`Impl` SATISFIES contract `Settle`"), "{:?}", msgs(ok));
        let bad = "-- allium: 4\ncontract Settle\n  entity T\n  observable state settled(T) : bool\n  observable state funded(T) : bool\n  guarantee sif means settled(t) implies funded(t)\nend\ncomponent Impl satisfies (s : Settle)\n  entity T\n  observable state settled(T) : bool\n  observable state cash(T) : bool\n  observable state funded(T) : bool\n  invariant a means settled(t) implies cash(t)\nend\n";
        assert!(any(bad, "does NOT satisfy contract `Settle`"), "{:?}", msgs(bad));
    }

    #[test]
    fn refinement_across_vocabularies_via_given_mapping() {
        // The abstract `funded` is bridged to detail terms by a `given` definition (no new construct);
        // refinement inlines it, so the layers may use different words. Dropping a leg breaks it.
        let ok = "-- allium: 4\ncontract Settle\n  entity T\n  observable state settled(T) : bool\n  observable state funded(T) : bool\n  guarantee sf means settled(t) implies funded(t)\nend\ncomponent Impl satisfies (s : Settle)\n  entity T\n  observable state settled(T) : bool\n  observable state cash(T) : bool\n  observable state sec(T) : bool\n  given funded(t) means cash(t) and sec(t)\n  invariant a means settled(t) implies cash(t)\n  invariant b means settled(t) implies sec(t)\nend\n";
        assert!(any(ok, "`Impl` SATISFIES contract `Settle`"), "{:?}", msgs(ok));
        let bad = "-- allium: 4\ncontract Settle\n  entity T\n  observable state settled(T) : bool\n  observable state funded(T) : bool\n  guarantee sf means settled(t) implies funded(t)\nend\ncomponent Impl satisfies (s : Settle)\n  entity T\n  observable state settled(T) : bool\n  observable state cash(T) : bool\n  observable state sec(T) : bool\n  given funded(t) means cash(t) and sec(t)\n  invariant a means settled(t) implies cash(t)\nend\n";
        assert!(any(bad, "does NOT satisfy contract `Settle`"), "{:?}", msgs(bad));
    }

    #[test]
    fn refinement_assume_guarantee_via_rely() {
        // Service delivers `ok` only ASSUMING the environment keeps it `up` (a rely). With the rely it
        // satisfies the contract, and the verdict states the assumption; without it, `ok` is not entailed.
        let ok = "-- allium: 4\ncontract Avail\n  entity R\n  observable state ok(R) : bool\n  guarantee dok means ok(r)\nend\ncomponent Svc satisfies (a : Avail)\n  entity R\n  observable state up(R) : bool\n  observable state ok(R) : bool\n  rely env_up means up(r)\n  invariant okwhenup means up(r) implies ok(r)\nend\n";
        assert!(any(ok, "`Svc` SATISFIES contract `Avail`"), "{:?}", msgs(ok));
        assert!(any(ok, "Assuming its rely-conditions (env_up)"), "verdict must state the assumption: {:?}", msgs(ok));
        let bad = "-- allium: 4\ncontract Avail\n  entity R\n  observable state ok(R) : bool\n  guarantee dok means ok(r)\nend\ncomponent Svc satisfies (a : Avail)\n  entity R\n  observable state up(R) : bool\n  observable state ok(R) : bool\n  invariant okwhenup means up(r) implies ok(r)\nend\n";
        assert!(any(bad, "does NOT satisfy contract `Avail`"), "without the rely, ok is not entailed: {:?}", msgs(bad));
    }

    #[test]
    fn refinement_arithmetic_entailment_via_lra() {
        // Contract promises net >= 0; the component defines net = assets - liabilities and asserts
        // assets >= liabilities. The simplex proves the promise entailed; dropping coverage breaks it.
        let ok = "-- allium: 4\ncontract Solvent\n  entity A\n  observable state net(A) : Money\n  guarantee nn means every a :: net(a) >= 0\nend\ncomponent Impl satisfies (s : Solvent)\n  entity A\n  observable state assets(A) : Money\n  observable state liabilities(A) : Money\n  observable state net(A) : Money\n  invariant d means every a :: net(a) = assets(a) - liabilities(a)\n  invariant c means every a :: assets(a) >= liabilities(a)\nend\n";
        assert!(any(ok, "`Impl` SATISFIES contract `Solvent`"), "{:?}", msgs(ok));
        let bad = "-- allium: 4\ncontract Solvent\n  entity A\n  observable state net(A) : Money\n  guarantee nn means every a :: net(a) >= 0\nend\ncomponent Impl satisfies (s : Solvent)\n  entity A\n  observable state assets(A) : Money\n  observable state liabilities(A) : Money\n  observable state net(A) : Money\n  invariant d means every a :: net(a) = assets(a) - liabilities(a)\nend\n";
        assert!(any(bad, "does NOT satisfy contract `Solvent`"), "{:?}", msgs(bad));
    }

    #[test]
    fn refinement_state_guarded_promise() {
        // A guarded promise `active => balance >= 0`: a component whose bound under the same guard is
        // stronger (>= 100) or unconditional (>= 100) satisfies it; a weaker guarded bound (>= -50) does not.
        let c = "-- allium: 4\ncontract Solvent\n  entity A\n  observable state active(A) : bool\n  observable state balance(A) : Money\n  guarantee solvent means active(a) implies balance(a) >= 0\nend\n";
        let strong = format!("{c}component Account satisfies (s : Solvent)\n  entity A\n  observable state active(A) : bool\n  observable state balance(A) : Money\n  invariant strong means active(a) implies balance(a) >= 100\nend\n");
        assert!(any(&strong, "`Account` SATISFIES contract `Solvent`"), "{:?}", msgs(&strong));
        let uncond = format!("{c}component Account satisfies (s : Solvent)\n  entity A\n  observable state active(A) : bool\n  observable state balance(A) : Money\n  invariant always means balance(a) >= 100\nend\n");
        assert!(any(&uncond, "`Account` SATISFIES contract `Solvent`"), "{:?}", msgs(&uncond));
        let weak = format!("{c}component Account satisfies (s : Solvent)\n  entity A\n  observable state active(A) : bool\n  observable state balance(A) : Money\n  invariant weak means active(a) implies balance(a) >= 0 - 50\nend\n");
        assert!(any(&weak, "does NOT satisfy contract `Solvent`"), "{:?}", msgs(&weak));
    }

    #[test]
    fn tier_report_classifies_each_invariant_transparently() {
        // A boolean invariant, a linear-arithmetic one, and a nonlinear one (rate x balance). The coverage
        // report must place the first two at their tiers and name the third as not-statically-checked with
        // its reason — no silent skip.
        let src = "-- allium: 4\ncomponent C\n  entity P\n  observable state paid(P) : bool\n  observable state settled(P) : bool\n  observable state bal(P) : Money\n  observable state amt(P) : Money\n  observable state rate(P) : Rate\n  observable state interest(P) : Money\n  invariant flag_ok means every p :: settled(p) implies paid(p)\n  invariant sum_ok means every p :: bal(p) = amt(p)\n  invariant int_ok means every p :: interest(p) = rate(p) * bal(p)\nend\n";
        let cov = msgs(src).into_iter().find(|m| m.contains("analysis coverage")).unwrap();
        assert!(cov.contains("boolean tier: flag_ok"), "{cov}");
        assert!(cov.contains("linear-arithmetic tier: sum_ok"), "{cov}");
        assert!(cov.contains("int_ok (a product of two unknowns"), "{cov}");
        assert!(cov.contains("verify with `monitor`"), "{cov}");
    }

    #[test]
    fn finality_transition_invariant() {
        // `old(settled) implies settled` (finality: never un-settle). An `unsettle` action breaks it; with
        // no such action it is INDUCTIVE. Crucially, init is NOT falsely reported as violating it (old at
        // init reads the current value), and BMC does not fabricate a violation from an idle step.
        let bad = "-- allium: 4\ncomponent L\n  entity T\n  observable state settled(T) : bool\n  init means not settled(t)\n  action settle\n    requires not settled(t)\n    ensures settled(t)\n  action unsettle\n    requires settled(t)\n    ensures not settled(t)\n  invariant finality means old(settled(t)) implies settled(t)\nend\n";
        assert!(any(bad, "`unsettle` in `L` can break invariant `finality`"), "{:?}", msgs(bad));
        assert!(!any(bad, "does not establish invariant `finality`"), "init must not falsely fail: {:?}", msgs(bad));
        let good = "-- allium: 4\ncomponent L\n  entity T\n  observable state settled(T) : bool\n  observable state logged(T) : bool\n  init means not settled(t) and not logged(t)\n  action settle\n    requires not settled(t)\n    ensures settled(t)\n  action log\n    ensures logged(t)\n  invariant finality means old(settled(t)) implies settled(t)\nend\n";
        assert!(any(good, "`finality` in `L` is INDUCTIVE"), "{:?}", msgs(good));
        assert!(!any(good, "can break"), "no false break from an idle step: {:?}", msgs(good));
        assert!(!any(good, "REACHABLY VIOLATED"), "BMC must not fabricate a transition-invariant trace: {:?}", msgs(good));
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
    fn arithmetic_transition_invariant_monotone() {
        // `old(total) <= total` (a non-decreasing / append-only total). With `amt >= 0`, `record` (adds)
        // preserves it and `rollback` (subtracts) breaks it. The arithmetic preservation path handles the
        // two-state invariant via the same pre/post priming, soundly, with no init or BMC misfire.
        let src = "-- allium: 4\ncomponent Meter\n  entity M\n  observable state total(M) : Money\n  observable state amt(M) : Money\n  action record\n    requires amt(m) >= 0\n    ensures total(m) = old(total(m)) + amt(m)\n  action rollback\n    requires amt(m) >= 0\n    ensures total(m) = old(total(m)) - amt(m)\n  invariant monotone means old(total(m)) <= total(m)\nend\n";
        assert!(any(src, "`rollback` in `Meter` can break arithmetic invariant `monotone`"), "{:?}", msgs(src));
        assert!(!any(src, "`record` in `Meter` can break"), "adding a non-negative amount preserves it: {:?}", msgs(src));
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
    fn bmc_enum_witnesses_a_reachable_lifecycle_violation() {
        // A one-step reachable violation: `ship` from `created` reaches `shipped` without `paid`.
        let bad = "-- allium: 4\ncomponent Order\n  entity O\n  observable state status(O) : { created | paid | shipped }\n  init means status(o) = created\n  action pay\n    requires status(o) = created\n    ensures status(o) = paid\n  action ship\n    requires status(o) = created\n    ensures status(o) = shipped\n  invariant no_unpaid_ship means status(o) = shipped implies status(o) = paid\nend\n";
        assert!(any(bad, "`no_unpaid_ship` in `Order` is REACHABLY VIOLATED in 1 step(s): init -> ship"), "{:?}", msgs(bad));
    }

    #[test]
    fn bmc_enum_reports_the_shortest_trace() {
        // The only route to `bad` is a -> b -> bad; BFS must report the 2-step trace, not a longer one.
        let src = "-- allium: 4\ncomponent Flow\n  entity F\n  observable state s(F) : { a | b | bad }\n  init means s(f) = a\n  action t1\n    requires s(f) = a\n    ensures s(f) = b\n  action t2\n    requires s(f) = b\n    ensures s(f) = bad\n  invariant never_bad means s(f) <> bad\nend\n";
        assert!(any(src, "REACHABLY VIOLATED in 2 step(s): init -> t1 -> t2"), "{:?}", msgs(src));
    }

    #[test]
    fn bmc_enum_is_silent_on_a_safe_lifecycle() {
        // A monotone lifecycle with no reachable violation must produce no trace (no false counterexample).
        let good = "-- allium: 4\ncomponent Cyc\n  entity C\n  observable state status(C) : { partitioning | processing | delivering }\n  init means status(c) = partitioning\n  action partition\n    requires status(c) = partitioning\n    ensures status(c) = processing\n  action deliver\n    requires status(c) = processing\n    ensures status(c) = delivering\n  invariant deliver_after_process means status(c) = delivering implies status(c) <> partitioning\nend\n";
        assert!(!any(good, "REACHABLY VIOLATED"), "no reachable violation exists: {:?}", msgs(good));
        // The reachable graph closes (3 states), so the invariant is proven exactly, not just unwitnessed.
        assert!(any(good, "`deliver_after_process` in `Cyc` is PROVED SAFE"), "graph closes → exact proof: {:?}", msgs(good));
    }

    #[test]
    fn reserved_word_variant_tag_is_flagged() {
        // `no` is the negation quantifier: as a tag it silently breaks parsing, so name the collision.
        let bad = "-- allium: 4\ncomponent Eval\n  entity E\n  observable state ok(E) : { yes | no }\n  init means ok(e) = yes\nend\n";
        assert!(any(bad, "variant tag `no` of `ok` in `Eval` is a reserved word"), "{:?}", msgs(bad));
        // A tag set with no reserved words is clean.
        let good = "-- allium: 4\ncomponent Eval\n  entity E\n  observable state ok(E) : { good | bad }\n  init means ok(e) = good\nend\n";
        assert!(!any(good, "reserved word"), "clean tags must not be flagged: {:?}", msgs(good));
    }

    #[test]
    fn dead_enum_state_is_flagged_but_not_inputs() {
        // `archived` is declared but no init/action produces it -> flagged. `active`/`closed` are.
        let dead = "-- allium: 4\ncomponent D\n  entity O\n  observable state status(O) : { active | closed | archived }\n  init means status(o) = active\n  action close\n    requires status(o) = active\n    ensures status(o) = closed\n  terminal status(o) = closed\nend\n";
        assert!(any(dead, "enum value `archived` of `status` in `D` is never produced"), "{:?}", msgs(dead));
        assert!(!any(dead, "value `active`"), "init value not dead: {:?}", msgs(dead));
        // An enum INPUT (no action writes it) has all values valid — never flagged.
        let input = "-- allium: 4\ncomponent E\n  entity O\n  observable state mode(O) : { fast | slow }\n  observable state count(O) : Number\n  invariant m means mode(o) = fast implies count(o) >= 0\nend\n";
        assert!(!any(input, "dead lifecycle state"), "enum input not flagged: {:?}", msgs(input));
    }

    #[test]
    fn enum_terminal_proved_despite_arithmetic_init_and_invariant() {
        // A mixed spec: init pins both an enum state and a numeric one, and an arithmetic invariant is
        // present. The enum terminal must still be certified INDUCTIVE (init is projected to its enum part).
        let src = "-- allium: 4\ncomponent S\n  entity T\n  observable state phase(T) : { pending | settled }\n  observable state paid(T) : Money\n  observable state amount(T) : Money\n  init means phase(t) = pending and paid(t) = 0\n  invariant settled_paid means phase(t) = settled implies paid(t) >= amount(t)\n  action settle\n    requires phase(t) = pending\n    ensures phase(t) = settled and paid(t) = amount(t)\n  terminal phase(t) = settled\nend\n";
        assert!(any(src, "invariant `terminal[phase(t) = settled]` in `S` is INDUCTIVE"), "{:?}", msgs(src));
    }

    #[test]
    fn transitions_block_is_desugared_and_checked() {
        // The block desugars to edge-actions + a legality invariant + finality; it is no longer warned as
        // unmodelled, and the invariant AFTER it still parses (capture stopped at the next real item).
        let src = "-- allium: 4\ncomponent Node\n  entity I\n  observable state status(I) : { starting | running | dead }\n  init means status(i) = starting\n  transitions status(i)\n    starting -> running\n    running -> dead\n    terminal: dead\n  invariant sane means status(i) = dead implies status(i) <> starting\nend\n";
        assert!(!any(src, "not yet modelled"), "the block must now be modelled: {:?}", msgs(src));
        assert!(any(src, "invariant `sane`"), "the invariant after the block must still be analysed: {:?}", msgs(src));
        // A move that is not a declared edge (starting -> dead) breaks the generated legality invariant.
        let illegal = "-- allium: 4\ncomponent Node\n  entity I\n  observable state status(I) : { starting | running | dead }\n  transitions status\n    starting -> running\n    running -> dead\n  action jump\n    requires status(i) = starting\n    ensures status(i) = dead\nend\n";
        assert!(any(illegal, "can break invariant `status_transitions_legal`"), "an illegal transition must break legality: {:?}", msgs(illegal));
    }

    #[test]
    fn transitions_initial_and_edge_guards() {
        let hdr = "-- allium: 4\ncomponent Order\n  entity O\n  observable state status(O) : { created | paid | shipped | delivered }\n  observable state funds_cleared(O) : Boolean\n  transitions status\n    initial created\n    created -> paid\n    paid -> shipped when funds_cleared(e)\n    shipped -> delivered\n    terminal delivered\n";
        // `initial created` makes the start explicit — it must not read as dead, and init is established.
        let clean = format!("{hdr}end\n");
        assert!(!any(&clean, "never produced"), "initial must make the start reachable: {:?}", msgs(&clean));
        assert!(!any(&clean, "does not establish"), "{:?}", msgs(&clean));
        assert!(!any(&clean, "can break"), "{:?}", msgs(&clean));
        // A `when` guard is part of legality: shipping without funds_cleared breaks it; with it is clean.
        // Each guarded edge is now its OWN legality invariant, so shipping without the guard breaks the
        // precise `status_paid_shipped_legal` (not the combined enum legality).
        let bad = format!("{hdr}  action rush\n    requires status(o) = paid\n    ensures status(o) = shipped\nend\n");
        assert!(any(&bad, "can break invariant `status_paid_shipped_legal`"), "guard must be enforced: {:?}", msgs(&bad));
        let ok = format!("{hdr}  action ship\n    requires status(o) = paid and funds_cleared(o)\n    ensures status(o) = shipped\nend\n");
        assert!(!any(&ok, "can break"), "a guard-satisfying transition is legal: {:?}", msgs(&ok));
    }

    #[test]
    fn transitions_arithmetic_edge_guard_is_enforced() {
        // An arithmetic edge guard `closing -> closed when balance <= 0` must be enforced in legality: an
        // action closing without the bound breaks it, one that requires the bound does not, and an enum
        // non-edge is still caught (the arith guard no longer drops the whole legality).
        let hdr = "-- allium: 4\ncomponent Loan\n  entity L\n  observable state phase(L) : { open | closing | closed }\n  observable state balance(L) : Money\n  transitions phase\n    initial open\n    open -> closing\n    closing -> closed when balance(e) <= 0\n";
        let bad = format!("{hdr}  action force_close\n    requires phase(l) = closing\n    ensures phase(l) = closed\nend\n");
        assert!(any(&bad, "can break invariant `phase_closing_closed_legal`"), "arith guard must be enforced: {:?}", msgs(&bad));
        let ok = format!("{hdr}  action force_close\n    requires phase(l) = closing and balance(l) <= 0\n    ensures phase(l) = closed\nend\n");
        assert!(!any(&ok, "can break"), "a bound-satisfying close is legal: {:?}", msgs(&ok));
        // The enum non-edge open -> closed is still caught even though an arith-guarded edge exists.
        let skip = format!("{hdr}  action skip\n    requires phase(l) = open\n    ensures phase(l) = closed\nend\n");
        assert!(any(&skip, "can break invariant `phase_transitions_legal`"), "enum legality must survive alongside an arith guard: {:?}", msgs(&skip));
    }

    #[test]
    fn lifecycle_actions_are_not_a_case_split() {
        // A lifecycle's overlapping transitions (both fireable from `running`) are nondeterminism, not an
        // ambiguous classification: no disjointness/exhaustiveness verdict should be raised.
        let life = "-- allium: 4\ncomponent Node\n  entity I\n  observable state status(I) : { starting | running | dead }\n  init means status(i) = starting\n  action go\n    requires status(i) = starting\n    ensures status(i) = running\n  action drain\n    requires status(i) = running\n    ensures status(i) = dead\n  action die\n    requires status(i) = running\n    ensures status(i) = dead\nend\n";
        assert!(!any(life, "case-split"), "a lifecycle is not a case-split: {:?}", msgs(life));
        // A genuine decision table (actions derive an output from disjoint input guards) still is one.
        let table = "-- allium: 4\ncomponent Rate\n  entity L\n  observable state tier(L) : Number\n  observable state rate(L) : Number\n  action low\n    requires tier(l) < 100\n    ensures rate(l) = 2\n  action high\n    requires tier(l) >= 100\n    ensures rate(l) = 5\nend\n";
        assert!(any(table, "case-split in `Rate`"), "a decision table is still checked: {:?}", msgs(table));
        // A transition that ALSO updates arithmetic state (`settle` sets phase AND paid) is still a
        // lifecycle transition, not a case-split.
        let mixed = "-- allium: 4\ncomponent S\n  entity T\n  observable state phase(T) : { pending | settled | failed }\n  observable state paid(T) : Money\n  observable state amount(T) : Money\n  init means phase(t) = pending\n  action settle\n    requires phase(t) = pending\n    ensures phase(t) = settled and paid(t) = amount(t)\n  action fail\n    requires phase(t) = pending\n    ensures phase(t) = failed\nend\n";
        assert!(!any(mixed, "case-split"), "a transition with an arithmetic effect is not a case-split: {:?}", msgs(mixed));
    }

    #[test]
    fn numeric_key_relational_uniqueness_is_reported_unchecked() {
        // A 2-entity uniqueness over a numeric key is beyond the boolean relational fragment; its
        // preservation is unchecked and must be reported so, not silently implied covered.
        let numeric = "-- allium: 4\ncomponent Clerk\n  entity Copy\n  observable state prio(Copy) : Number\n  observable state reg(Copy) : bool\n  invariant unique_prio means every a :: every b :: reg(a) and reg(b) and a <> b implies prio(a) <> prio(b)\n  action register\n    ensures reg(a) and prio(a) = 5\nend\n";
        assert!(any(numeric, "relational invariant `unique_prio` in `Clerk` is NOT preservation-checked"), "{:?}", msgs(numeric));
        // A boolean relational uniqueness IS checked, so it must NOT carry the unchecked note.
        let boolean = "-- allium: 4\ncomponent C\n  entity X\n  observable state leader(X) : bool\n  invariant one means every a :: every b :: leader(a) and leader(b) implies a = b\n  action elect\n    ensures leader(a)\nend\n";
        assert!(!any(boolean, "NOT preservation-checked"), "boolean relational is checked: {:?}", msgs(boolean));
    }

    #[test]
    fn refinement_type_is_not_critiqued_as_unstated_assumption() {
        // A `where` refinement is a stated constraint, not a hidden assumption: the entailment probe must
        // not flag it as "relies on an unstated assumption".
        let src = "-- allium: 4\ncomponent Ev\n  entity E\n  observable state gap(E) : Number where gap(e) >= 1\n  observable state idx(E) : Number where idx(e) >= 0\nend\n";
        assert!(!any(src, "`refine[gap]` in `Ev` is NOT entailed"), "refinement is definitional: {:?}", msgs(src));
    }

    #[test]
    fn bmc_enum_handles_conditional_effects() {
        // A branching transition `phase = if ok = good then done else failed`: the branch is resolved
        // against the concrete state, so a `bad` init reaches `failed` and violates `never_failed`.
        let bad = "-- allium: 4\ncomponent Eval\n  entity E\n  observable state phase(E) : { pending | done | failed }\n  observable state ok(E) : { good | bad }\n  init means phase(e) = pending and ok(e) = bad\n  action run\n    requires phase(e) = pending\n    ensures phase(e) = if ok(e) = good then done else failed\n  invariant never_failed means phase(e) <> failed\nend\n";
        assert!(any(bad, "`never_failed` in `Eval` is REACHABLY VIOLATED in 1 step(s): init -> run"), "{:?}", msgs(bad));
        // The other branch (`good`) never reaches `failed`, and the graph closes: exact proof.
        let good = "-- allium: 4\ncomponent Eval\n  entity E\n  observable state phase(E) : { pending | done | failed }\n  observable state ok(E) : { good | bad }\n  init means phase(e) = pending and ok(e) = good\n  action run\n    requires phase(e) = pending\n    ensures phase(e) = if ok(e) = good then done else failed\n  invariant never_failed means phase(e) <> failed\nend\n";
        assert!(any(good, "`never_failed` in `Eval` is PROVED SAFE"), "good branch never fails: {:?}", msgs(good));
    }

    #[test]
    fn bmc_enum_does_not_prove_an_arithmetic_invariant() {
        // With a Money state present the decl leaves the enum-only fragment, so bmc_enum must not claim to
        // have proved the arithmetic invariant `pos` (arith_preservation owns it).
        let src = "-- allium: 4\ncomponent M\n  entity C\n  observable state status(C) : { open | closed }\n  observable state bal(C) : Money\n  init means status(c) = open\n  action shut\n    requires status(c) = open\n    ensures status(c) = closed\n  invariant pos means status(c) = closed implies bal(c) >= 0\nend\n";
        assert!(!any(src, "`pos` in `M` is PROVED SAFE"), "must not prove an arithmetic invariant: {:?}", msgs(src));
    }

    #[test]
    fn refinement_type_where_clause() {
        // `state balance : Money where balance(a) >= 0` pins the invariant to the type; an unguarded
        // withdraw that could go negative breaks it, a guarded one does not.
        let bad = "-- allium: 4\ncomponent Acct\n  entity A\n  observable state balance(A) : Money where balance(a) >= 0\n  observable state amt(A) : Money\n  action withdraw\n    ensures balance(a) = old(balance(a)) - amt(a)\nend\n";
        assert!(any(bad, "`withdraw` in `Acct` can break arithmetic invariant `refine[balance]`"), "{:?}", msgs(bad));
        let good = "-- allium: 4\ncomponent Acct\n  entity A\n  observable state balance(A) : Money where balance(a) >= 0\n  observable state amt(A) : Money\n  action withdraw\n    requires amt(a) >= 0 and amt(a) <= balance(a)\n    ensures balance(a) = old(balance(a)) - amt(a)\nend\n";
        assert!(!any(good, "can break arithmetic invariant `refine[balance]`"), "a guarded withdraw preserves it: {:?}", msgs(good));
    }

    #[test]
    fn stuck_state_detection_respects_terminal() {
        // `delivered` is reachable and no action leaves it: a stuck state — UNLESS declared `terminal`.
        let stuck = "-- allium: 4\ncomponent O\n  entity X\n  observable state status(X) : { created | paid | delivered }\n  init means status(x) = created\n  action pay\n    requires status(x) = created\n    ensures status(x) = paid\n  action deliver\n    requires status(x) = paid\n    ensures status(x) = delivered\nend\n";
        assert!(any(stuck, "state `status(e) = delivered` in `O` is reachable but no action can fire"), "{:?}", msgs(stuck));
        let terminal = "-- allium: 4\ncomponent O\n  entity X\n  observable state status(X) : { created | paid | delivered }\n  init means status(x) = created\n  action pay\n    requires status(x) = created\n    ensures status(x) = paid\n  action deliver\n    requires status(x) = paid\n    ensures status(x) = delivered\n  terminal status(x) = delivered\nend\n";
        assert!(!any(terminal, "stuck state"), "a declared terminal must not be flagged: {:?}", msgs(terminal));
    }

    #[test]
    fn variant_payload_field_guarded_access() {
        // A sum type `outcome : { success { outputs } | failure { error } }`. Reading `outputs` under the
        // `success` guard is well-formed; reading `error` without the `failure` guard is ill-formed.
        let src = "-- allium: 4\ncomponent H\n  entity E\n  observable state outcome(E) : { success { outputs : Number } | failure { error : Number } }\n  observable state done(E) : bool\n  invariant ok means outcome(e) = success implies outputs(e) >= 0\n  invariant bad means done(e) implies error(e) >= 0\nend\n";
        assert!(any(src, "field `error` is only present when `outcome(e) = failure`"), "{:?}", msgs(src));
        assert!(!any(src, "field `outputs`"), "guarded read must be well-formed: {:?}", msgs(src));
    }

    #[test]
    fn terminal_marker_desugars_to_finality() {
        // `terminal status = delivered` means the state is never left. Proven INDUCTIVE for a legal
        // machine; a `reopen` action that leaves it is caught.
        let ok = "-- allium: 4\ncomponent O\n  entity X\n  observable state status(X) : { created | paid | delivered }\n  init means status(x) = created\n  action pay\n    requires status(x) = created\n    ensures status(x) = paid\n  action deliver\n    requires status(x) = paid\n    ensures status(x) = delivered\n  terminal status(x) = delivered\nend\n";
        assert!(any(ok, "`terminal[status(x) = delivered]` in `O` is INDUCTIVE"), "{:?}", msgs(ok));
        let bad = "-- allium: 4\ncomponent O\n  entity X\n  observable state status(X) : { created | paid | delivered }\n  init means status(x) = created\n  action pay\n    requires status(x) = created\n    ensures status(x) = paid\n  action deliver\n    requires status(x) = paid\n    ensures status(x) = delivered\n  action reopen\n    requires status(x) = delivered\n    ensures status(x) = created\n  terminal status(x) = delivered\nend\n";
        assert!(any(bad, "`reopen` in `O` can break invariant `terminal[status(x) = delivered]`"), "{:?}", msgs(bad));
    }

    #[test]
    fn enum_state_lifecycle_preservation() {
        // A lifecycle over an ENUM status (not boolean flags). The finality invariant is proven INDUCTIVE
        // for a legal machine; a `reopen` action that regresses from `delivered` breaks it. The exactly-one
        // axiom makes `status = a` and `status = b` mutually exclusive, so preservation reasons over it.
        let ok = "-- allium: 4\ncomponent O\n  entity X\n  observable state status(X) : { created | paid | delivered }\n  init means status(x) = created\n  action pay\n    requires status(x) = created\n    ensures status(x) = paid\n  action deliver\n    requires status(x) = paid\n    ensures status(x) = delivered\n  invariant fin means old(status(x) = delivered) implies status(x) = delivered\nend\n";
        assert!(any(ok, "`fin` in `O` is INDUCTIVE"), "{:?}", msgs(ok));
        assert!(any(ok, "boolean tier: fin"), "enum invariant should be boolean tier: {:?}", msgs(ok));
        let bad = "-- allium: 4\ncomponent O\n  entity X\n  observable state status(X) : { created | paid | delivered }\n  init means status(x) = created\n  action pay\n    requires status(x) = created\n    ensures status(x) = paid\n  action deliver\n    requires status(x) = paid\n    ensures status(x) = delivered\n  action reopen\n    requires status(x) = delivered\n    ensures status(x) = created\n  invariant fin means old(status(x) = delivered) implies status(x) = delivered\nend\n";
        assert!(any(bad, "`reopen` in `O` can break invariant `fin`"), "{:?}", msgs(bad));
    }

    #[test]
    fn enum_state_exactly_one_value() {
        // An enum observable takes exactly one value, so `flag` forcing status to be both `created` and
        // `paid` is INFEASIBLE — before enum support, those were independent atoms and it looked feasible.
        let bad = "-- allium: 4\ncomponent C\n  entity O\n  observable state status(O) : { created | paid | shipped }\n  observable state flag(O) : bool\n  axiom a1 means flag(o) implies status(o) = created\n  axiom a2 means flag(o) implies status(o) = paid\n  requirement r means flag(o)\nend\n";
        assert!(any(bad, "requirement `r` in `C` is INFEASIBLE"), "{:?}", msgs(bad));
        // Forcing a single value is fine (feasible).
        let ok = "-- allium: 4\ncomponent C\n  entity O\n  observable state status(O) : { created | paid | shipped }\n  observable state flag(O) : bool\n  axiom a1 means flag(o) implies status(o) = created\n  requirement r means flag(o)\nend\n";
        assert!(any(ok, "requirement `r` in `C` is feasible"), "{:?}", msgs(ok));
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
