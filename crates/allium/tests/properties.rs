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
