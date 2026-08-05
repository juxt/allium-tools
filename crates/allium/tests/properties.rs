//! Metamorphic / property-based tests over the analyser.
//!
//! The generator is hand-rolled (seeded SplitMix64, pure std) so there is no
//! external property-testing dependency. A property is a loop over many seeds
//! that builds a spec and asserts an invariant; the seed is printed on failure
//! so any counterexample reproduces exactly.
//!
//! Started for #71 (deterministic output ordering). Designed to grow to cover
//! #70 (split-invariance: one file == the same spec split across a `use` edge)
//! and #72 (a malformed provides entry is diagnosed at the entry).

use allium_parser::{analyse, analyze, parse, Diagnostic, Finding};

// ---------------------------------------------------------------------------
// Tiny deterministic RNG (SplitMix64) — enough to drive a generator.
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// Generator: a valid multi-entity spec. Each entity has a two-state lifecycle,
// a creation rule that parks it in the first state, and a surface providing the
// creation trigger, but no rule advancing it — so each entity draws two
// lifecycle warnings (unreachableValue on the terminal, noExit on the start)
// and one deadlock finding. Multiple entities at different source offsets are
// what make ordering observable.
// ---------------------------------------------------------------------------

fn gen_spec(rng: &mut Rng) -> String {
    let n = 3 + rng.below(4); // 3..=6 entities
    let mut src = String::from("-- allium: 3\n");
    for i in 0..n {
        let name = format!("Ent{i}");
        let s0 = format!("s{i}a");
        let s1 = format!("s{i}b");
        src.push_str(&format!(
            "\nentity {name} {{\n\
             \x20   status: {s0} | {s1}\n\
             \x20   transitions status {{ {s0} -> {s1}  terminal: {s1} }}\n\
             }}\n\
             \nrule Create{name} {{\n\
             \x20   when: Create{name}Requested()\n\
             \x20   ensures: {name}.created(status: {s0})\n\
             }}\n\
             \nsurface {name}Desk {{\n\
             \x20   provides:\n\
             \x20       Create{name}Requested()\n\
             }}\n",
        ));
    }
    src
}

// ---------------------------------------------------------------------------
// Canonical ordering keys.
// ---------------------------------------------------------------------------

fn diag_key(d: &Diagnostic) -> (usize, usize, &'static str) {
    (d.span.start, d.span.end, d.code.unwrap_or(""))
}

fn finding_key(f: &Finding) -> (String, String) {
    (
        f["type"].as_str().unwrap_or("").to_string(),
        f["summary"].as_str().unwrap_or("").to_string(),
    )
}

fn diagnostics_of(src: &str) -> Vec<Diagnostic> {
    let parsed = parse(src);
    analyze(&parsed.module, src)
}

fn findings_of(src: &str) -> Vec<Finding> {
    let parsed = parse(src);
    analyse(&parsed.module, src).findings
}

// ---------------------------------------------------------------------------
// #71 — output ordering is a deterministic function of the input.
//
// The property is expressed as "the emitted arrays are in canonical sorted
// order", which is what the fix guarantees and which a HashMap-iteration order
// violates on essentially every multi-entity spec.
// ---------------------------------------------------------------------------

#[test]
fn prop_diagnostics_are_sorted() {
    for seed in 0..200u64 {
        let src = gen_spec(&mut Rng::new(seed));
        let ds = diagnostics_of(&src);
        for w in ds.windows(2) {
            assert!(
                diag_key(&w[0]) <= diag_key(&w[1]),
                "seed {seed}: diagnostics not in canonical order.\norder: {:?}\nspec:\n{src}",
                ds.iter().map(diag_key).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn prop_findings_are_sorted() {
    for seed in 0..200u64 {
        let src = gen_spec(&mut Rng::new(seed));
        let fs = findings_of(&src);
        for w in fs.windows(2) {
            assert!(
                finding_key(&w[0]) <= finding_key(&w[1]),
                "seed {seed}: findings not in canonical order.\norder: {:?}\nspec:\n{src}",
                fs.iter().map(finding_key).collect::<Vec<_>>()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// #70 — a transition-trigger rule establishes its start state.
//
// A rule triggered by `becomes S` / `transitions_to S` fires with the entity in
// S, so its status assignment performs a transition out of S, exactly as a
// `requires: b.status = S` guard would. The metamorphic property: adding that
// redundant guard must not change any report.
// ---------------------------------------------------------------------------

/// A spec whose only exit from the start state is performed by a
/// transition-trigger rule. With `redundant_guard`, the rule also carries a
/// `requires:` that merely restates what the trigger already establishes.
fn gen_transition_trigger_spec(rng: &mut Rng, redundant_guard: bool) -> String {
    let name = format!("Ent{}", rng.below(1000));
    let s0 = format!("s{}start", rng.below(100));
    let s1 = format!("s{}end", rng.below(100));
    let trigger = if rng.below(2) == 0 { "becomes" } else { "transitions_to" };
    let guard = if redundant_guard {
        format!("    requires: t.status = {s0}\n")
    } else {
        String::new()
    };
    format!(
        "-- allium: 3\n\
         entity {name} {{\n\
         \x20   status: {s0} | {s1}\n\
         \x20   transitions status {{ {s0} -> {s1}  terminal: {s1} }}\n\
         }}\n\
         rule Create{name} {{\n\
         \x20   when: Create{name}Requested()\n\
         \x20   ensures: {name}.created(status: {s0})\n\
         }}\n\
         rule Advance{name} {{\n\
         \x20   when: t: {name}.status {trigger} {s0}\n\
         {guard}\
         \x20   ensures: t.status = {s1}\n\
         }}\n\
         surface {name}Desk {{\n\
         \x20   provides:\n\
         \x20       Create{name}Requested()\n\
         }}\n",
    )
}

/// A canonical, comparable representation of everything a spec reports.
fn report_set(src: &str) -> Vec<String> {
    let parsed = parse(src);
    let mut out: Vec<String> = analyze(&parsed.module, src)
        .iter()
        .map(|d| format!("D {} {}", d.code.unwrap_or(""), d.message))
        .collect();
    for f in analyse(&parsed.module, src).findings.iter() {
        out.push(format!(
            "F {} {}",
            f["type"].as_str().unwrap_or(""),
            f["summary"].as_str().unwrap_or("")
        ));
    }
    out.sort();
    out
}

#[test]
fn prop_redundant_trigger_guard_is_invariant() {
    for seed in 0..100u64 {
        // Same seed for both variants, so only the guard differs.
        let without = gen_transition_trigger_spec(&mut Rng::new(seed), false);
        let with = gen_transition_trigger_spec(&mut Rng::new(seed), true);
        let a = report_set(&without);
        let b = report_set(&with);
        assert_eq!(
            a, b,
            "seed {seed}: a redundant requires that restates the trigger's start state changed the reports.\n\
             WITHOUT guard:\n{without}\n-> {a:?}\n\nWITH guard:\n{with}\n-> {b:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Branch-nesting invariance for undefined-binding detection.
//
// A reference to an unbound name must be flagged the same whether it sits at the
// top level of a rule or inside an `if`/`else` body. The undefined-binding pass
// used to walk only the top level (plus one level of `for`), so a branch-nested
// reference went silently unflagged.
// ---------------------------------------------------------------------------

fn undefined_binding_codes(src: &str) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = diagnostics_of(src)
        .iter()
        .filter_map(|d| d.code)
        .filter(|c| *c == "allium.rule.undefinedBinding")
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn undefined_binding_flagged_inside_if_branch() {
    let base = "-- allium: 3\n\nentity Job {\n    status: pending | done\n    transitions status { pending -> done  terminal: done }\n}\n\nsurface S {\n    provides:\n        Go(flag)\n}\n";
    let top = format!(
        "{base}\nrule R {{\n    when: Go(flag)\n    requires: ghost.status = pending\n    ensures: Job.created(status: pending)\n}}\n"
    );
    let branched = format!(
        "{base}\nrule R {{\n    when: Go(flag)\n    if flag:\n        requires: ghost.status = pending\n        ensures: Job.created(status: pending)\n    else:\n        ensures: Job.created(status: pending)\n}}\n"
    );
    let t = undefined_binding_codes(&top);
    let b = undefined_binding_codes(&branched);
    assert!(!t.is_empty(), "control: a top-level undefined binding should be flagged, got {t:?}");
    assert_eq!(
        t, b,
        "an undefined binding nested in an if-branch was not flagged like the top-level form"
    );
}

#[test]
fn branch_local_let_is_not_a_false_positive() {
    // A `let` declared inside a branch scopes that branch, so referencing it
    // there must not trip undefinedBinding.
    let src = "-- allium: 3\n\nentity Job {\n    status: pending | done\n    transitions status { pending -> done  terminal: done }\n}\n\nsurface S {\n    provides:\n        Go(flag)\n}\n\nrule R {\n    when: Go(flag)\n    if flag:\n        let j = Job\n        ensures: j.status = done\n    else:\n        ensures: Job.created(status: pending)\n}\n";
    assert!(
        undefined_binding_codes(src).is_empty(),
        "a branch-local let was wrongly flagged as undefined: {:?}",
        undefined_binding_codes(src)
    );
}

#[test]
fn undeclared_type_flagged_inside_if_branch() {
    // A type reference to an undeclared entity must be flagged the same whether
    // it sits at the top level of a rule or inside an `if`/`else` body. The
    // type-reference pass used to walk only top-level clauses.
    let base = "-- allium: 3\n\nentity Job {\n    status: pending | done\n    transitions status { pending -> done  terminal: done }\n}\n\nsurface S {\n    provides:\n        Go(flag)\n}\n";
    let top = format!(
        "{base}\nrule R {{\n    when: Go(flag)\n    ensures: Ghost.created(status: pending)\n}}\n"
    );
    let branched = format!(
        "{base}\nrule R {{\n    when: Go(flag)\n    if flag:\n        ensures: Ghost.created(status: pending)\n    else:\n        ensures: Job.created(status: pending)\n}}\n"
    );
    let has_undeclared = |src: &str| {
        diagnostics_of(src)
            .iter()
            .any(|d| d.message.contains("Type reference 'Ghost' is not declared"))
    };
    assert!(has_undeclared(&top), "control: a top-level undeclared type should be flagged");
    assert!(
        has_undeclared(&branched),
        "an undeclared type nested in an if-branch was not flagged like the top-level form"
    );
}

#[test]
fn becomes_triggered_transition_has_no_false_noexit() {
    // #70 subject, single file: the exit from `closed` is witnessed by the
    // becomes-triggered rule, so no noExit.
    let src = "-- allium: 3\n\
        entity Ticket {\n    status: closed | archived\n    transitions status { closed -> archived  terminal: archived }\n}\n\
        rule Create {\n    when: CreateRequested()\n    ensures: Ticket.created(status: closed)\n}\n\
        rule Archive {\n    when: t: Ticket.status becomes closed\n    ensures: t.status = archived\n}\n\
        surface Desk {\n    provides:\n        CreateRequested()\n}\n";
    let ds = diagnostics_of(src);
    assert!(
        !ds.iter().any(|d| d.code == Some("allium.status.noExit")),
        "a becomes-triggered exit must clear noExit on closed. Got: {:?}",
        ds.iter().map(|d| (d.code, &d.message)).collect::<Vec<_>>()
    );
}

// A fixed six-entity example, so a failure is inspectable without a seed.
#[test]
fn six_entity_spec_emits_sorted_reports() {
    let src = gen_spec(&mut Rng::new(6)); // seed 6 -> a 6-entity spec shape
    let ds = diagnostics_of(&src);
    assert!(
        ds.windows(2).all(|w| diag_key(&w[0]) <= diag_key(&w[1])),
        "diagnostics not sorted: {:?}",
        ds.iter().map(diag_key).collect::<Vec<_>>()
    );
    let fs = findings_of(&src);
    assert!(
        fs.windows(2).all(|w| finding_key(&w[0]) <= finding_key(&w[1])),
        "findings not sorted: {:?}",
        fs.iter().map(finding_key).collect::<Vec<_>>()
    );
}
