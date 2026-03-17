//! Walks the Allium AST and emits a test obligations document.
//!
//! Each obligation represents a test that should exist, derived mechanically
//! from the spec's constructs. The output is language-agnostic structured data.

use allium_parser::ast::*;
use allium_parser::{Module, Span};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct TestPlan {
    pub version: Option<u32>,
    pub obligations: Vec<Obligation>,
}

#[derive(Debug, Serialize)]
pub struct Obligation {
    pub id: String,
    pub category: ObligationCategory,
    pub description: String,
    /// Name of the entity, rule, surface, etc. that sourced this obligation.
    pub source_construct: String,
    /// Byte offset range in the source file.
    pub source_span: (usize, usize),
    /// Category-specific data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<ObligationDetail>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationCategory {
    EntityFields,
    EntityOptional,
    EntityRelationship,
    ValueEquality,
    EnumComparable,
    SumTypeVariant,
    DerivedValue,
    Projection,
    ConfigDefault,
    InvariantProperty,
    RuleSuccess,
    RuleFailure,
    RuleEntityCreation,
    TransitionEdge,
    TransitionRejected,
    TransitionTerminal,
    ProducesGuarantee,
    ConsumesPresence,
    TemporalTrigger,
    SurfaceExposure,
    SurfaceProvides,
    SurfaceActor,
    ContractSignature,
    #[allow(dead_code)]
    ScenarioHappyPath,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ObligationDetail {
    Fields { fields: Vec<String> },
    Transition { entity: String, field: String, from: String, to: String },
    Terminal { entity: String, field: String, states: Vec<String> },
    Produces { rule: String, fields: Vec<String> },
    Consumes { rule: String, fields: Vec<String> },
    Surface { surface: String, items: Vec<String> },
}

pub fn generate_test_plan(module: &Module) -> TestPlan {
    let mut plan = TestPlan {
        version: module.version,
        obligations: Vec::new(),
    };

    for decl in &module.declarations {
        match decl {
            Decl::Block(block) => match block.kind {
                BlockKind::Entity | BlockKind::ExternalEntity => {
                    emit_entity_obligations(&mut plan, block);
                }
                BlockKind::Value => {
                    emit_value_obligations(&mut plan, block);
                }
                BlockKind::Enum => {
                    emit_enum_obligations(&mut plan, block);
                }
                BlockKind::Rule => {
                    emit_rule_obligations(&mut plan, block);
                }
                BlockKind::Surface => {
                    emit_surface_obligations(&mut plan, block);
                }
                BlockKind::Config => {
                    emit_config_obligations(&mut plan, block);
                }
                BlockKind::Contract => {
                    emit_contract_obligations(&mut plan, block);
                }
                _ => {}
            },
            Decl::Variant(v) => {
                emit_variant_obligations(&mut plan, v);
            }
            Decl::Invariant(inv) => {
                emit_invariant_obligation(&mut plan, inv);
            }
            _ => {}
        }
    }

    plan
}

fn block_name(block: &BlockDecl) -> String {
    block.name.as_ref().map(|n| n.name.clone()).unwrap_or_default()
}

fn emit_entity_obligations(plan: &mut TestPlan, block: &BlockDecl) {
    let name = block_name(block);

    // Collect fields
    let mut fields = Vec::new();
    let mut optional_fields = Vec::new();
    let mut relationships = Vec::new();
    let mut derived_values = Vec::new();
    let mut projections = Vec::new();

    for item in &block.items {
        match &item.kind {
            BlockItemKind::Assignment { name: field_name, value } => {
                fields.push(field_name.name.clone());

                // Check for optional type
                if matches!(value, Expr::TypeOptional { .. }) {
                    optional_fields.push(field_name.name.clone());
                }

                // Check for relationship (has `with`)
                if matches!(value, Expr::With { .. }) {
                    relationships.push(field_name.name.clone());
                }

                // Check for projection (has `where` on a field ref)
                if matches!(value, Expr::Where { .. }) {
                    projections.push(field_name.name.clone());
                }

                // Derived values: anything that's a comparison, logical op, member access chain, etc.
                if is_derived_expression(value) {
                    derived_values.push(field_name.name.clone());
                }
            }
            BlockItemKind::TransitionsBlock(graph) => {
                emit_transition_obligations(plan, &name, graph);
            }
            BlockItemKind::InvariantBlock { name: inv_name, .. } => {
                plan.obligations.push(Obligation {
                    id: format!("invariant-entity-{}-{}", name, inv_name.name),
                    category: ObligationCategory::InvariantProperty,
                    description: format!(
                        "Verify invariant {} holds after any field mutation on {}",
                        inv_name.name, name
                    ),
                    source_construct: format!("{}.{}", name, inv_name.name),
                    source_span: (item.span.start, item.span.end),
                    detail: None,
                });
            }
            _ => {}
        }
    }

    if !fields.is_empty() {
        plan.obligations.push(Obligation {
            id: format!("entity-fields-{}", name),
            category: ObligationCategory::EntityFields,
            description: format!("Verify all declared fields on {} are present with correct types", name),
            source_construct: name.clone(),
            source_span: (block.span.start, block.span.end),
            detail: Some(ObligationDetail::Fields { fields: fields.clone() }),
        });
    }

    for f in &optional_fields {
        plan.obligations.push(Obligation {
            id: format!("entity-optional-{}-{}", name, f),
            category: ObligationCategory::EntityOptional,
            description: format!("Verify optional field {}.{} accepts null and non-null values", name, f),
            source_construct: format!("{}.{}", name, f),
            source_span: (block.span.start, block.span.end),
            detail: None,
        });
    }

    for r in &relationships {
        plan.obligations.push(Obligation {
            id: format!("entity-relationship-{}-{}", name, r),
            category: ObligationCategory::EntityRelationship,
            description: format!("Verify relationship {}.{} navigates to the correct related entities", name, r),
            source_construct: format!("{}.{}", name, r),
            source_span: (block.span.start, block.span.end),
            detail: None,
        });
    }

    for p in &projections {
        plan.obligations.push(Obligation {
            id: format!("projection-{}-{}", name, p),
            category: ObligationCategory::Projection,
            description: format!("Verify projection {}.{} filters correctly", name, p),
            source_construct: format!("{}.{}", name, p),
            source_span: (block.span.start, block.span.end),
            detail: None,
        });
    }

    for d in &derived_values {
        plan.obligations.push(Obligation {
            id: format!("derived-{}-{}", name, d),
            category: ObligationCategory::DerivedValue,
            description: format!("Verify derived value {}.{} computes correctly", name, d),
            source_construct: format!("{}.{}", name, d),
            source_span: (block.span.start, block.span.end),
            detail: None,
        });
    }
}

fn emit_transition_obligations(plan: &mut TestPlan, entity: &str, graph: &TransitionGraph) {
    let field = &graph.field.name;

    // One obligation per declared edge
    for edge in &graph.edges {
        plan.obligations.push(Obligation {
            id: format!("transition-edge-{}-{}-{}", entity, edge.from.name, edge.to.name),
            category: ObligationCategory::TransitionEdge,
            description: format!(
                "Verify transition {} -> {} on {}.{} is reachable via a witnessing rule",
                edge.from.name, edge.to.name, entity, field
            ),
            source_construct: format!("{}.{}", entity, field),
            source_span: (edge.span.start, edge.span.end),
            detail: Some(ObligationDetail::Transition {
                entity: entity.to_string(),
                field: field.clone(),
                from: edge.from.name.clone(),
                to: edge.to.name.clone(),
            }),
        });
    }

    // Obligation: undeclared transitions are rejected
    plan.obligations.push(Obligation {
        id: format!("transition-rejected-{}", entity),
        category: ObligationCategory::TransitionRejected,
        description: format!(
            "Verify undeclared transitions on {}.{} are rejected",
            entity, field
        ),
        source_construct: format!("{}.{}", entity, field),
        source_span: (graph.span.start, graph.span.end),
        detail: None,
    });

    // Obligation per terminal state: no outbound transitions
    if !graph.terminal.is_empty() {
        plan.obligations.push(Obligation {
            id: format!("transition-terminal-{}", entity),
            category: ObligationCategory::TransitionTerminal,
            description: format!(
                "Verify terminal states on {}.{} have no outbound transitions",
                entity, field
            ),
            source_construct: format!("{}.{}", entity, field),
            source_span: (graph.span.start, graph.span.end),
            detail: Some(ObligationDetail::Terminal {
                entity: entity.to_string(),
                field: field.clone(),
                states: graph.terminal.iter().map(|t| t.name.clone()).collect(),
            }),
        });
    }
}

fn emit_value_obligations(plan: &mut TestPlan, block: &BlockDecl) {
    let name = block_name(block);
    plan.obligations.push(Obligation {
        id: format!("value-equality-{}", name),
        category: ObligationCategory::ValueEquality,
        description: format!("Verify value type {} has structural equality", name),
        source_construct: name.clone(),
        source_span: (block.span.start, block.span.end),
        detail: None,
    });

    let fields: Vec<String> = block.items.iter().filter_map(|item| {
        if let BlockItemKind::Assignment { name, .. } = &item.kind {
            Some(name.name.clone())
        } else {
            None
        }
    }).collect();

    if !fields.is_empty() {
        plan.obligations.push(Obligation {
            id: format!("entity-fields-{}", name),
            category: ObligationCategory::EntityFields,
            description: format!("Verify all declared fields on {} are present with correct types", name),
            source_construct: name,
            source_span: (block.span.start, block.span.end),
            detail: Some(ObligationDetail::Fields { fields }),
        });
    }
}

fn emit_enum_obligations(plan: &mut TestPlan, block: &BlockDecl) {
    let name = block_name(block);
    plan.obligations.push(Obligation {
        id: format!("enum-comparable-{}", name),
        category: ObligationCategory::EnumComparable,
        description: format!("Verify fields typed with enum {} are comparable", name),
        source_construct: name,
        source_span: (block.span.start, block.span.end),
        detail: None,
    });
}

fn emit_rule_obligations(plan: &mut TestPlan, block: &BlockDecl) {
    let name = block_name(block);

    // Success case
    plan.obligations.push(Obligation {
        id: format!("rule-success-{}", name),
        category: ObligationCategory::RuleSuccess,
        description: format!("Verify rule {} succeeds when all preconditions are met", name),
        source_construct: name.clone(),
        source_span: (block.span.start, block.span.end),
        detail: None,
    });

    // Walk items for specific obligations
    walk_rule_items(plan, &name, &block.items, block.span);
}

fn walk_rule_items(plan: &mut TestPlan, rule_name: &str, items: &[BlockItem], _block_span: Span) {
    for item in items {
        match &item.kind {
            BlockItemKind::Clause { keyword, .. } if keyword == "requires" => {
                plan.obligations.push(Obligation {
                    id: format!("rule-failure-{}-{}", rule_name, plan.obligations.len()),
                    category: ObligationCategory::RuleFailure,
                    description: format!(
                        "Verify rule {} is rejected when requires clause fails",
                        rule_name
                    ),
                    source_construct: rule_name.to_string(),
                    source_span: (item.span.start, item.span.end),
                    detail: None,
                });
            }
            BlockItemKind::Clause { keyword, value } if keyword == "when" => {
                // Check for temporal triggers
                if contains_temporal(value) {
                    plan.obligations.push(Obligation {
                        id: format!("temporal-{}", rule_name),
                        category: ObligationCategory::TemporalTrigger,
                        description: format!(
                            "Verify temporal trigger in {} fires at deadline, not before, and does not re-fire",
                            rule_name
                        ),
                        source_construct: rule_name.to_string(),
                        source_span: (item.span.start, item.span.end),
                        detail: None,
                    });
                }
                // Check for entity creation triggers
                if contains_entity_creation(value) {
                    plan.obligations.push(Obligation {
                        id: format!("rule-entity-creation-{}", rule_name),
                        category: ObligationCategory::RuleEntityCreation,
                        description: format!(
                            "Verify entity creation in rule {} produces the specified fields",
                            rule_name
                        ),
                        source_construct: rule_name.to_string(),
                        source_span: (item.span.start, item.span.end),
                        detail: None,
                    });
                }
            }
            BlockItemKind::Clause { keyword, value } if keyword == "ensures" => {
                if contains_entity_creation(value) {
                    plan.obligations.push(Obligation {
                        id: format!("rule-entity-creation-{}-{}", rule_name, plan.obligations.len()),
                        category: ObligationCategory::RuleEntityCreation,
                        description: format!(
                            "Verify entity creation in rule {} ensures clause produces the specified fields",
                            rule_name
                        ),
                        source_construct: rule_name.to_string(),
                        source_span: (item.span.start, item.span.end),
                        detail: None,
                    });
                }
            }
            BlockItemKind::ProducesClause { fields } => {
                let field_names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
                plan.obligations.push(Obligation {
                    id: format!("produces-{}", rule_name),
                    category: ObligationCategory::ProducesGuarantee,
                    description: format!(
                        "Verify rule {} produces fields [{}] as non-null after execution",
                        rule_name,
                        field_names.join(", ")
                    ),
                    source_construct: rule_name.to_string(),
                    source_span: (item.span.start, item.span.end),
                    detail: Some(ObligationDetail::Produces {
                        rule: rule_name.to_string(),
                        fields: field_names,
                    }),
                });
            }
            BlockItemKind::ConsumesClause { fields } => {
                let field_names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
                plan.obligations.push(Obligation {
                    id: format!("consumes-{}", rule_name),
                    category: ObligationCategory::ConsumesPresence,
                    description: format!(
                        "Verify fields [{}] are present when rule {} triggers",
                        field_names.join(", "),
                        rule_name,
                    ),
                    source_construct: rule_name.to_string(),
                    source_span: (item.span.start, item.span.end),
                    detail: Some(ObligationDetail::Consumes {
                        rule: rule_name.to_string(),
                        fields: field_names,
                    }),
                });
            }
            BlockItemKind::ForBlock { items, .. } => {
                walk_rule_items(plan, rule_name, items, _block_span);
            }
            BlockItemKind::IfBlock { branches, else_items } => {
                for branch in branches {
                    walk_rule_items(plan, rule_name, &branch.items, _block_span);
                }
                if let Some(else_items) = else_items {
                    walk_rule_items(plan, rule_name, else_items, _block_span);
                }
            }
            _ => {}
        }
    }
}

fn emit_surface_obligations(plan: &mut TestPlan, block: &BlockDecl) {
    let name = block_name(block);

    // Actor restriction
    plan.obligations.push(Obligation {
        id: format!("surface-actor-{}", name),
        category: ObligationCategory::SurfaceActor,
        description: format!("Verify surface {} is accessible only to the specified actor", name),
        source_construct: name.clone(),
        source_span: (block.span.start, block.span.end),
        detail: None,
    });

    for item in &block.items {
        match &item.kind {
            BlockItemKind::Clause { keyword, value } if keyword == "exposes" => {
                let items = collect_exposed_names(value);
                plan.obligations.push(Obligation {
                    id: format!("surface-exposure-{}", name),
                    category: ObligationCategory::SurfaceExposure,
                    description: format!("Verify each exposed item on {} is accessible", name),
                    source_construct: name.clone(),
                    source_span: (item.span.start, item.span.end),
                    detail: if items.is_empty() { None } else {
                        Some(ObligationDetail::Surface { surface: name.clone(), items })
                    },
                });
            }
            BlockItemKind::Clause { keyword, .. } if keyword == "provides" => {
                plan.obligations.push(Obligation {
                    id: format!("surface-provides-{}", name),
                    category: ObligationCategory::SurfaceProvides,
                    description: format!(
                        "Verify provided operations on {} appear/hide based on when conditions",
                        name
                    ),
                    source_construct: name.clone(),
                    source_span: (item.span.start, item.span.end),
                    detail: None,
                });
            }
            _ => {}
        }
    }
}

fn emit_config_obligations(plan: &mut TestPlan, block: &BlockDecl) {
    for item in &block.items {
        if let BlockItemKind::Assignment { name, .. } = &item.kind {
            plan.obligations.push(Obligation {
                id: format!("config-default-{}", name.name),
                category: ObligationCategory::ConfigDefault,
                description: format!("Verify config parameter {} has its declared default", name.name),
                source_construct: format!("config.{}", name.name),
                source_span: (item.span.start, item.span.end),
                detail: None,
            });
        }
    }
}

fn emit_contract_obligations(plan: &mut TestPlan, block: &BlockDecl) {
    let name = block_name(block);
    for item in &block.items {
        if let BlockItemKind::Assignment { name: sig_name, .. } = &item.kind {
            plan.obligations.push(Obligation {
                id: format!("contract-sig-{}-{}", name, sig_name.name),
                category: ObligationCategory::ContractSignature,
                description: format!(
                    "Verify implementation satisfies contract {}.{}",
                    name, sig_name.name
                ),
                source_construct: format!("{}.{}", name, sig_name.name),
                source_span: (item.span.start, item.span.end),
                detail: None,
            });
        }
    }
}

fn emit_variant_obligations(plan: &mut TestPlan, v: &VariantDecl) {
    plan.obligations.push(Obligation {
        id: format!("sum-type-variant-{}", v.name.name),
        category: ObligationCategory::SumTypeVariant,
        description: format!(
            "Verify variant {} has its specific fields accessible within a type guard",
            v.name.name
        ),
        source_construct: v.name.name.clone(),
        source_span: (v.span.start, v.span.end),
        detail: None,
    });
}

fn emit_invariant_obligation(plan: &mut TestPlan, inv: &InvariantDecl) {
    plan.obligations.push(Obligation {
        id: format!("invariant-{}", inv.name.name),
        category: ObligationCategory::InvariantProperty,
        description: format!(
            "Verify invariant {} holds after every state-changing rule that touches constrained entities",
            inv.name.name
        ),
        source_construct: inv.name.name.clone(),
        source_span: (inv.span.start, inv.span.end),
        detail: None,
    });
}

// --- Helpers ---

fn is_derived_expression(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Comparison { .. }
            | Expr::LogicalOp { .. }
            | Expr::BinaryOp { .. }
            | Expr::Not { .. }
    )
}

fn contains_temporal(expr: &Expr) -> bool {
    match expr {
        Expr::Comparison { right, .. } => matches!(right.as_ref(), Expr::Now { .. }),
        Expr::Binding { value, .. } => contains_temporal(value),
        Expr::Block { items, .. } => items.iter().any(contains_temporal),
        _ => false,
    }
}

fn contains_entity_creation(expr: &Expr) -> bool {
    match expr {
        Expr::Call { function, .. } => {
            if let Expr::MemberAccess { field, .. } = function.as_ref() {
                field.name == "created"
            } else {
                false
            }
        }
        Expr::Block { items, .. } => items.iter().any(contains_entity_creation),
        _ => false,
    }
}

fn collect_exposed_names(expr: &Expr) -> Vec<String> {
    match expr {
        Expr::MemberAccess { field, .. } => vec![field.name.clone()],
        Expr::Block { items, .. } => items.iter().flat_map(collect_exposed_names).collect(),
        Expr::WhenGuard { action, .. } => collect_exposed_names(action),
        _ => Vec::new(),
    }
}
