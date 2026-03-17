//! Walks the Allium AST and emits generator specifications.
//!
//! For each entity: fields with types and constraints, relationships,
//! applicable invariants, config-derived bounds, transition graphs and
//! per-state field-presence guarantees.

use allium_parser::ast::*;
use allium_parser::{Module, Span};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct GeneratorSpec {
    pub version: Option<u32>,
    pub entities: Vec<EntityGen>,
    pub value_types: Vec<ValueTypeGen>,
    pub enums: Vec<EnumGen>,
    pub config: Vec<ConfigParam>,
}

#[derive(Debug, Serialize)]
pub struct EntityGen {
    pub name: String,
    pub kind: EntityKind,
    pub fields: Vec<FieldGen>,
    pub relationships: Vec<RelationshipGen>,
    pub projections: Vec<ProjectionGen>,
    pub derived_values: Vec<DerivedValueGen>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition_graph: Option<TransitionGraphGen>,
    /// Per-state field-presence guarantees, computed from produces/consumes.
    /// Key: state name, Value: fields guaranteed present at that state.
    pub state_guarantees: Vec<StateGuarantee>,
    pub invariants: Vec<InvariantGen>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Internal,
    External,
}

#[derive(Debug, Serialize)]
pub struct FieldGen {
    pub name: String,
    pub type_expr: String,
    pub optional: bool,
    /// Inline enum values, if this field uses an inline enum.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RelationshipGen {
    pub name: String,
    pub target: String,
}

#[derive(Debug, Serialize)]
pub struct ProjectionGen {
    pub name: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct DerivedValueGen {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct TransitionGraphGen {
    pub field: String,
    pub edges: Vec<EdgeGen>,
    pub terminal: Vec<String>,
    /// All states that appear in the graph.
    pub states: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct EdgeGen {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct StateGuarantee {
    pub state: String,
    pub guaranteed_fields: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct InvariantGen {
    pub name: String,
    pub scope: InvariantScope,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum InvariantScope {
    Entity,
    TopLevel,
}

#[derive(Debug, Serialize)]
pub struct ValueTypeGen {
    pub name: String,
    pub fields: Vec<FieldGen>,
}

#[derive(Debug, Serialize)]
pub struct EnumGen {
    pub name: String,
    pub values: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ConfigParam {
    pub name: String,
    pub type_expr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_expr: Option<String>,
}

pub fn generate_generators(module: &Module, source: &str) -> GeneratorSpec {
    let mut spec = GeneratorSpec {
        version: module.version,
        entities: Vec::new(),
        value_types: Vec::new(),
        enums: Vec::new(),
        config: Vec::new(),
    };

    // First pass: collect rule produces/consumes for state guarantee computation
    let mut rule_produces: Vec<RuleProduces> = Vec::new();
    for decl in &module.declarations {
        if let Decl::Block(block) = decl {
            if block.kind == BlockKind::Rule {
                if let Some(rp) = extract_rule_produces(block) {
                    rule_produces.push(rp);
                }
            }
        }
    }

    // Second pass: build generator specs
    for decl in &module.declarations {
        if let Decl::Block(block) = decl {
            match block.kind {
                BlockKind::Entity | BlockKind::ExternalEntity => {
                    spec.entities.push(build_entity_gen(block, source, &rule_produces));
                }
                BlockKind::Value => {
                    spec.value_types.push(build_value_gen(block, source));
                }
                BlockKind::Enum => {
                    spec.enums.push(build_enum_gen(block));
                }
                BlockKind::Config => {
                    build_config_gen(&mut spec, block, source);
                }
                _ => {}
            }
        }
    }

    spec
}

struct RuleProduces {
    #[allow(dead_code)]
    rule_name: String,
    target_state: Option<(String, String)>, // (field, target_state_value)
    produces: Vec<String>,
}

fn extract_rule_produces(block: &BlockDecl) -> Option<RuleProduces> {
    let name = block.name.as_ref()?.name.clone();
    let mut produces = Vec::new();
    let mut target_state: Option<(String, String)> = None;

    for item in &block.items {
        match &item.kind {
            BlockItemKind::ProducesClause { fields } => {
                produces.extend(fields.iter().map(|f| f.name.clone()));
            }
            BlockItemKind::Clause { keyword, value } if keyword == "ensures" => {
                // Look for state assignments like entity.status = shipped
                extract_state_target(value, &mut target_state);
            }
            _ => {}
        }
    }

    if produces.is_empty() {
        return None;
    }

    Some(RuleProduces {
        rule_name: name,
        target_state,
        produces,
    })
}

fn extract_state_target(expr: &Expr, target: &mut Option<(String, String)>) {
    match expr {
        Expr::Comparison {
            left,
            op: ComparisonOp::Eq,
            right,
            ..
        } => {
            if let Expr::MemberAccess { field, .. } = left.as_ref() {
                if field.name == "status" {
                    if let Expr::Ident(id) = right.as_ref() {
                        *target = Some(("status".to_string(), id.name.clone()));
                    }
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                extract_state_target(item, target);
            }
        }
        _ => {}
    }
}

fn build_entity_gen(block: &BlockDecl, source: &str, rule_produces: &[RuleProduces]) -> EntityGen {
    let name = block.name.as_ref().map(|n| n.name.clone()).unwrap_or_default();
    let kind = if block.kind == BlockKind::ExternalEntity {
        EntityKind::External
    } else {
        EntityKind::Internal
    };

    let mut fields = Vec::new();
    let mut relationships = Vec::new();
    let mut projections = Vec::new();
    let mut derived_values = Vec::new();
    let mut transition_graph = None;
    let mut invariants = Vec::new();

    for item in &block.items {
        match &item.kind {
            BlockItemKind::Assignment { name: field_name, value } => {
                let type_expr = span_text(source, value.span());
                let optional = matches!(value, Expr::TypeOptional { .. });
                let enum_values = extract_inline_enum_values(value);

                if matches!(value, Expr::With { .. }) {
                    let target = extract_with_target(value);
                    relationships.push(RelationshipGen {
                        name: field_name.name.clone(),
                        target,
                    });
                } else if matches!(value, Expr::Where { .. }) {
                    projections.push(ProjectionGen {
                        name: field_name.name.clone(),
                        source: extract_where_source(value),
                    });
                } else if is_derived(value) {
                    derived_values.push(DerivedValueGen {
                        name: field_name.name.clone(),
                    });
                } else {
                    fields.push(FieldGen {
                        name: field_name.name.clone(),
                        type_expr,
                        optional,
                        enum_values,
                    });
                }
            }
            BlockItemKind::TransitionsBlock(graph) => {
                let mut states = std::collections::BTreeSet::new();
                let edges: Vec<EdgeGen> = graph.edges.iter().map(|e| {
                    states.insert(e.from.name.clone());
                    states.insert(e.to.name.clone());
                    EdgeGen {
                        from: e.from.name.clone(),
                        to: e.to.name.clone(),
                    }
                }).collect();
                for t in &graph.terminal {
                    states.insert(t.name.clone());
                }

                transition_graph = Some(TransitionGraphGen {
                    field: graph.field.name.clone(),
                    edges,
                    terminal: graph.terminal.iter().map(|t| t.name.clone()).collect(),
                    states: states.into_iter().collect(),
                });
            }
            BlockItemKind::InvariantBlock { name: inv_name, .. } => {
                invariants.push(InvariantGen {
                    name: inv_name.name.clone(),
                    scope: InvariantScope::Entity,
                });
            }
            _ => {}
        }
    }

    // Compute state guarantees from rule_produces
    let state_guarantees = compute_state_guarantees(&transition_graph, rule_produces);

    EntityGen {
        name,
        kind,
        fields,
        relationships,
        projections,
        derived_values,
        transition_graph,
        state_guarantees,
        invariants,
    }
}

fn compute_state_guarantees(
    graph: &Option<TransitionGraphGen>,
    rule_produces: &[RuleProduces],
) -> Vec<StateGuarantee> {
    let graph = match graph {
        Some(g) => g,
        None => return Vec::new(),
    };

    let mut guarantees = Vec::new();
    for state in &graph.states {
        // Find all rules that transition TO this state
        let inbound_rules: Vec<&RuleProduces> = rule_produces
            .iter()
            .filter(|rp| {
                rp.target_state
                    .as_ref()
                    .map(|(_, s)| s == state)
                    .unwrap_or(false)
            })
            .collect();

        if inbound_rules.is_empty() {
            continue;
        }

        // Intersection of all produces sets
        let mut guaranteed: Option<std::collections::BTreeSet<String>> = None;
        for rp in &inbound_rules {
            let set: std::collections::BTreeSet<String> =
                rp.produces.iter().cloned().collect();
            guaranteed = Some(match guaranteed {
                Some(existing) => existing.intersection(&set).cloned().collect(),
                None => set,
            });
        }

        if let Some(fields) = guaranteed {
            if !fields.is_empty() {
                guarantees.push(StateGuarantee {
                    state: state.clone(),
                    guaranteed_fields: fields.into_iter().collect(),
                });
            }
        }
    }

    guarantees
}

fn build_value_gen(block: &BlockDecl, source: &str) -> ValueTypeGen {
    let name = block.name.as_ref().map(|n| n.name.clone()).unwrap_or_default();
    let fields = block.items.iter().filter_map(|item| {
        if let BlockItemKind::Assignment { name, value } = &item.kind {
            if !is_derived(value) {
                return Some(FieldGen {
                    name: name.name.clone(),
                    type_expr: span_text(source, value.span()),
                    optional: matches!(value, Expr::TypeOptional { .. }),
                    enum_values: Vec::new(),
                });
            }
        }
        None
    }).collect();

    ValueTypeGen { name, fields }
}

fn build_enum_gen(block: &BlockDecl) -> EnumGen {
    let name = block.name.as_ref().map(|n| n.name.clone()).unwrap_or_default();
    let values = block.items.iter().filter_map(|item| {
        if let BlockItemKind::EnumVariant { name, .. } = &item.kind {
            Some(name.name.clone())
        } else {
            None
        }
    }).collect();

    EnumGen { name, values }
}

fn build_config_gen(spec: &mut GeneratorSpec, block: &BlockDecl, source: &str) {
    for item in &block.items {
        if let BlockItemKind::Assignment { name, value } = &item.kind {
            // Try to extract type and default from "Type = default" patterns
            let type_expr = extract_type_from_assignment(value, source);
            let default_expr = extract_default_from_assignment(value, source);
            spec.config.push(ConfigParam {
                name: name.name.clone(),
                type_expr,
                default_expr,
            });
        }
    }
}

// --- Helpers ---

fn span_text(source: &str, span: Span) -> String {
    source.get(span.start..span.end).unwrap_or("").to_string()
}

fn extract_inline_enum_values(expr: &Expr) -> Vec<String> {
    match expr {
        Expr::Pipe { left, right, .. } => {
            let mut values = extract_inline_enum_values(left);
            values.extend(extract_inline_enum_values(right));
            values
        }
        Expr::Ident(id) => {
            // Lowercase idents are enum values
            if id.name.chars().next().map(|c| c.is_lowercase()).unwrap_or(false) {
                vec![id.name.clone()]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn extract_with_target(expr: &Expr) -> String {
    match expr {
        Expr::With { source, .. } => {
            if let Expr::Ident(id) = source.as_ref() {
                id.name.clone()
            } else {
                "unknown".to_string()
            }
        }
        _ => "unknown".to_string(),
    }
}

fn extract_where_source(expr: &Expr) -> String {
    match expr {
        Expr::Where { source, .. } => {
            if let Expr::Ident(id) = source.as_ref() {
                id.name.clone()
            } else {
                "unknown".to_string()
            }
        }
        _ => "unknown".to_string(),
    }
}

fn is_derived(value: &Expr) -> bool {
    matches!(
        value,
        Expr::Comparison { .. }
            | Expr::LogicalOp { .. }
            | Expr::BinaryOp { .. }
            | Expr::Not { .. }
    )
}

fn extract_type_from_assignment(value: &Expr, source: &str) -> String {
    // For assignments like `param: Type = default`, the parser sees the whole
    // right side as one expression. Just use the source text.
    span_text(source, value.span())
}

fn extract_default_from_assignment(value: &Expr, _source: &str) -> Option<String> {
    // The parser combines "Type = default" into a Comparison with Eq.
    // If we see an Eq comparison, the right side is the default.
    match value {
        Expr::Comparison {
            op: ComparisonOp::Eq,
            right,
            ..
        } => Some(format!("{:?}", right)),
        _ => None,
    }
}
