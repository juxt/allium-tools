//! Test obligations for a v4 spec (`allium plan` over v4). The v3 test-plan runs the v3 parser and
//! cannot see v4 constructs; this walks the v4 module and emits an obligation per checkable claim.
//!
//! The load-bearing difference from v3 is the OBJECTIVE obligation: the anti-vacuity floor. A safety
//! spec (invariants/faults) is satisfied by a do-nothing implementation, so tests generated from it
//! alone never catch a vacuous implementation. An objective obligation is exactly the test such an
//! implementation fails. v3 has no objective construct, so it cannot emit this obligation at all.

use serde::Serialize;

#[derive(Serialize)]
pub struct V4Plan {
    pub language_version: u32,
    pub obligations: Vec<Obligation>,
}

#[derive(Serialize)]
pub struct Obligation {
    /// safety | objective | budget | action | contract
    pub category: &'static str,
    pub subject: String,
    /// What a test must assert.
    pub obligation: String,
    /// What an implementation that fails this obligation looks like — the bug the test catches.
    pub falsified_by: String,
}

pub fn generate(module: &allium_v4::ast::Module, src: &str) -> V4Plan {
    use allium_v4::ast::{DeclKind, ItemKind};
    let mut obligations = Vec::new();
    for d in &module.decls {
        if d.kind != DeclKind::Component {
            continue; // contracts and other decls carry no directly-testable behaviour here
        }
        let comp = &d.name;
        for it in &d.items {
            let name = it.name.clone().unwrap_or_default();
            match it.kind {
                ItemKind::Invariant => obligations.push(Obligation {
                    category: "safety",
                    subject: format!("{comp}.{name}"),
                    obligation: format!("invariant `{name}` holds in every reachable state"),
                    falsified_by: "an action sequence that reaches a state violating it".into(),
                }),
                ItemKind::Objective => {
                    let p = allium_v4::analyse::parse_objective_body(it.body.map(|b| b.slice(src)).unwrap_or(""));
                    let g = if p.goal.is_empty() { "the goal".to_string() } else { format!("`{}`", p.goal) };
                    let bound = p.bound.as_deref().map(|b| format!(" within `{b}`")).unwrap_or_default();
                    let via = p.measure.as_deref().map(|m| format!(" (progress: `{m}` decreases to its floor)")).unwrap_or_default();
                    obligations.push(Obligation {
                        category: "objective",
                        subject: format!("{comp}.{name}"),
                        obligation: format!("the objective {g} is REACHED{bound}{via}"),
                        falsified_by: format!("an implementation that never achieves {g} — e.g. a do-nothing implementation that satisfies every safety invariant but makes no progress (the anti-vacuity test)"),
                    });
                }
                ItemKind::Budget => obligations.push(Obligation {
                    category: "budget",
                    subject: format!("{comp}.budget"),
                    obligation: format!("`{}` stays within budget over its window (monitored)", it.body.map(|b| b.slice(src).split_whitespace().next().unwrap_or("metric")).unwrap_or("metric")),
                    falsified_by: "a run whose measured percentile/rate breaches the threshold".into(),
                }),
                ItemKind::Action => obligations.push(Obligation {
                    category: "action",
                    subject: format!("{comp}.{name}"),
                    obligation: format!("action `{name}` establishes its postcondition when its guard holds"),
                    falsified_by: "an invocation under the guard that does not produce the ensured state".into(),
                }),
                _ => {}
            }
        }
        for sat in &d.satisfies {
            obligations.push(Obligation {
                category: "contract",
                subject: format!("{comp} satisfies {}", sat.ty),
                obligation: format!("`{comp}` honours every promise of contract `{}`", sat.ty),
                falsified_by: "a reachable state where the component's invariants do not entail a promise".into(),
            });
        }
    }
    V4Plan { language_version: 4, obligations }
}
