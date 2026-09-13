//! Domain-model extraction for allium v4.
//!
//! v4 is component-centric: a component groups entities, observable states,
//! `given` definitions, actions, role-tagged predicates (invariant/axiom/
//! requirement/fault/guarantee/rely) and objectives. That shape does not map
//! onto the v1-v3 entity/field model in `domain_model.rs`, so v4 gets its own
//! extractor. Invoked by `allium model` on a v4 spec.

use allium_v4::ast::{DeclKind, ItemKind, Module};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct V4DomainModel {
    pub version: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contracts: Vec<Component>,
}

#[derive(Debug, Serialize)]
pub struct Component {
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<State>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub givens: Vec<Given>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub predicates: Vec<Predicate>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub objectives: Vec<Objective>,
}

#[derive(Debug, Serialize)]
pub struct State {
    pub name: String,
    /// The entity sort the observable ranges over, if any (`Period` in `state x(Period)`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// The declared value type (`Money`, `Number`, `bool`, an inline enum), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Given {
    pub name: String,
    /// The type (`given disbursed : Money`) or reference definition (`given k means <expr>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Action {
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Predicate {
    pub name: String,
    pub role: String,
}

#[derive(Debug, Serialize)]
pub struct Objective {
    pub goal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measure: Option<String>,
}

fn role_name(k: &ItemKind) -> &'static str {
    match k {
        ItemKind::Invariant => "invariant",
        ItemKind::Axiom => "axiom",
        ItemKind::Requirement => "requirement",
        ItemKind::Fault => "fault",
        ItemKind::Guarantee => "guarantee",
        ItemKind::Rely => "rely",
        _ => "predicate",
    }
}

fn clean(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

pub fn extract_v4_domain_model(module: &Module, src: &str) -> V4DomainModel {
    let mut components = Vec::new();
    let mut contracts = Vec::new();

    for d in &module.decls {
        if d.kind == DeclKind::Import {
            continue;
        }
        let mut c = Component {
            name: d.name.clone(),
            entities: Vec::new(),
            states: Vec::new(),
            givens: Vec::new(),
            actions: Vec::new(),
            predicates: Vec::new(),
            objectives: Vec::new(),
        };
        for it in &d.items {
            let name = it.name.clone().unwrap_or_default();
            match it.kind {
                ItemKind::Entity => c.entities.push(name),
                ItemKind::State => c.states.push(State {
                    name,
                    sort: it.params.first().cloned(),
                    value_type: it.body.and_then(|b| clean(b.slice(src))),
                }),
                ItemKind::Given => c.givens.push(Given {
                    name,
                    definition: it.body.and_then(|b| clean(b.slice(src))),
                }),
                ItemKind::Action => c.actions.push(Action { name, params: it.params.clone() }),
                ItemKind::Invariant
                | ItemKind::Axiom
                | ItemKind::Requirement
                | ItemKind::Fault
                | ItemKind::Guarantee
                | ItemKind::Rely => c.predicates.push(Predicate { name, role: role_name(&it.kind).to_string() }),
                ItemKind::Objective => {
                    if let Some(b) = it.body {
                        let p = allium_v4::analyse::parse_objective_body(b.slice(src));
                        c.objectives.push(Objective { goal: p.goal, bound: p.bound, measure: p.measure });
                    }
                }
                _ => {}
            }
        }
        match d.kind {
            DeclKind::Contract => contracts.push(c),
            _ => components.push(c),
        }
    }

    V4DomainModel { version: 4, components, contracts }
}
