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
    /// Invariant expression text, when this obligation is an invariant property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
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
    WhenFieldPresence,
    WhenPresenceObligation,
    WhenAbsenceObligation,
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
    WhenFieldPresence {
        entity: String,
        field: String,
        status_field: String,
        qualifying_states: Vec<String>,
    },
    WhenPresence {
        rule: String,
        entity: String,
        field: String,
        source_state: String,
        target_state: String,
        qualifying_states: Vec<String>,
    },
    WhenAbsence {
        rule: String,
        entity: String,
        field: String,
        source_state: String,
        target_state: String,
        qualifying_states: Vec<String>,
    },
    Surface { surface: String, items: Vec<String> },
}

pub fn generate_test_plan(module: &Module, source: &str) -> TestPlan {
    let mut plan = TestPlan {
        version: module.version,
        obligations: Vec::new(),
    };

    for decl in &module.declarations {
        match decl {
            Decl::Block(block) => match block.kind {
                BlockKind::Entity | BlockKind::ExternalEntity => {
                    emit_entity_obligations(&mut plan, block, source);
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
                emit_invariant_obligation(&mut plan, inv, source);
            }
            _ => {}
        }
    }

    // Cross-reference rules with entity when-qualified fields for presence/absence obligations
    emit_when_crossing_obligations(&mut plan, &module.declarations);

    plan
}

/// Collect when-qualified fields from all entities, then check each rule for
/// status transitions that cross the boundary of a field's when set.
fn emit_when_crossing_obligations(plan: &mut TestPlan, declarations: &[Decl]) {
    struct WhenField {
        entity: String,
        field: String,
        status_field: String,
        qualifying_states: Vec<String>,
    }

    // Per-entity: when-qualified fields and transition graph edges
    struct EntityInfo {
        when_fields: Vec<WhenField>,
        /// Transition edges as (from, to) for the status field referenced by when-fields
        edges: Vec<(String, String)>,
        /// All states in the transition graph
        states: Vec<String>,
    }

    let mut entities: std::collections::HashMap<String, EntityInfo> = std::collections::HashMap::new();

    for decl in declarations {
        if let Decl::Block(block) = decl {
            if block.kind == BlockKind::Entity || block.kind == BlockKind::ExternalEntity {
                let entity_name = block.name.as_ref().map(|n| n.name.clone()).unwrap_or_default();
                let info = entities.entry(entity_name.clone()).or_insert_with(|| EntityInfo {
                    when_fields: Vec::new(),
                    edges: Vec::new(),
                    states: Vec::new(),
                });

                for item in &block.items {
                    match &item.kind {
                        BlockItemKind::FieldWithWhen { name, when_clause, .. } => {
                            info.when_fields.push(WhenField {
                                entity: entity_name.clone(),
                                field: name.name.clone(),
                                status_field: when_clause.status_field.name.clone(),
                                qualifying_states: when_clause.qualifying_states.iter().map(|s| s.name.clone()).collect(),
                            });
                        }
                        BlockItemKind::TransitionsBlock(graph) => {
                            for edge in &graph.edges {
                                info.edges.push((edge.from.name.clone(), edge.to.name.clone()));
                                if !info.states.contains(&edge.from.name) {
                                    info.states.push(edge.from.name.clone());
                                }
                                if !info.states.contains(&edge.to.name) {
                                    info.states.push(edge.to.name.clone());
                                }
                            }
                            for t in &graph.terminal {
                                if !info.states.contains(&t.name) {
                                    info.states.push(t.name.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Filter to entities that have when-fields
    let entities_with_when: Vec<(&String, &EntityInfo)> = entities.iter()
        .filter(|(_, info)| !info.when_fields.is_empty())
        .collect();

    if entities_with_when.is_empty() {
        return;
    }

    // For each rule, extract source/target state and match against entities
    for decl in declarations {
        if let Decl::Block(block) = decl {
            if block.kind != BlockKind::Rule {
                continue;
            }
            let rule_name = block.name.as_ref().map(|n| n.name.clone()).unwrap_or_default();
            let rule_span = block.span;

            let source_states = extract_requires_states(&block.items);
            let target_state = extract_ensures_target_state(&block.items);

            if let Some((target_obj, target_field, target_value)) = &target_state {
                // Use object variable to scope to the correct entity.
                // Convention: `order.status` → entity `Order` (capitalised variable name).
                let target_entity_hint = if !target_obj.is_empty() {
                    let mut hint = target_obj.clone();
                    if let Some(first) = hint.get_mut(0..1) {
                        first.make_ascii_uppercase();
                    }
                    Some(hint)
                } else {
                    None
                };

                for (entity_name, info) in &entities_with_when {
                    // If we have an entity hint, use it to filter
                    if let Some(hint) = &target_entity_hint {
                        if hint != *entity_name {
                            continue;
                        }
                    } else if !info.states.contains(target_value) {
                        // No hint — fall back to checking graph membership
                        continue;
                    }

                    // Find source states relevant to this entity from the requires clause
                    let entity_source_states: Vec<&StateInfo> = source_states.iter()
                        .filter(|(obj, field, _)| {
                            field == target_field && (obj.is_empty() || {
                                let mut h = obj.clone();
                                if let Some(f) = h.get_mut(0..1) { f.make_ascii_uppercase(); }
                                &h == *entity_name
                            })
                        })
                        .collect();

                    for wf in &info.when_fields {
                        if wf.status_field != *target_field {
                            continue;
                        }

                        let target_in_set = wf.qualifying_states.contains(target_value);

                        if !entity_source_states.is_empty() {
                            // Collect valid (source_value, direction) pairs from graph edges
                            let mut emitted_presence = false;
                            let mut emitted_absence = false;
                            for (_, _, src_value) in &entity_source_states {
                                if !info.edges.iter().any(|(f, t)| f == src_value && t == target_value) {
                                    continue;
                                }
                                let source_in_set = wf.qualifying_states.contains(src_value);

                                if !source_in_set && target_in_set && !emitted_presence {
                                    emitted_presence = true;
                                    plan.obligations.push(Obligation {
                                        id: format!("when-set-{}-{}-{}", rule_name, wf.entity, wf.field),
                                        category: ObligationCategory::WhenPresenceObligation,
                                        description: format!(
                                            "Verify rule {} sets {}.{} when transitioning {} from {} to {} (entering when set {{{}}})",
                                            rule_name, wf.entity, wf.field, wf.status_field,
                                            src_value, target_value,
                                            wf.qualifying_states.join(", ")
                                        ),
                                        source_construct: rule_name.clone(),
                                        expression: None,
                                        source_span: (rule_span.start, rule_span.end),
                                        detail: Some(ObligationDetail::WhenPresence {
                                            rule: rule_name.clone(),
                                            entity: wf.entity.clone(),
                                            field: wf.field.clone(),
                                            source_state: src_value.clone(),
                                            target_state: target_value.clone(),
                                            qualifying_states: wf.qualifying_states.clone(),
                                        }),
                                    });
                                } else if source_in_set && !target_in_set && !emitted_absence {
                                    emitted_absence = true;
                                    plan.obligations.push(Obligation {
                                        id: format!("when-clear-{}-{}-{}", rule_name, wf.entity, wf.field),
                                        category: ObligationCategory::WhenAbsenceObligation,
                                        description: format!(
                                            "Verify rule {} clears {}.{} when transitioning {} from {} to {} (leaving when set {{{}}})",
                                            rule_name, wf.entity, wf.field, wf.status_field,
                                            src_value, target_value,
                                            wf.qualifying_states.join(", ")
                                        ),
                                        source_construct: rule_name.clone(),
                                        expression: None,
                                        source_span: (rule_span.start, rule_span.end),
                                        detail: Some(ObligationDetail::WhenAbsence {
                                            rule: rule_name.clone(),
                                            entity: wf.entity.clone(),
                                            field: wf.field.clone(),
                                            source_state: src_value.clone(),
                                            target_state: target_value.clone(),
                                            qualifying_states: wf.qualifying_states.clone(),
                                        }),
                                    });
                                }
                            }
                        } else if target_in_set {
                            // No source state info — emit conservatively if entity has edges to target
                            if !info.edges.iter().any(|(_, t)| t == target_value) {
                                continue;
                            }
                            plan.obligations.push(Obligation {
                                id: format!("when-set-{}-{}-{}", rule_name, wf.entity, wf.field),
                                category: ObligationCategory::WhenPresenceObligation,
                                description: format!(
                                    "Verify rule {} sets {}.{} when transitioning {} to {} (entering when set {{{}}})",
                                    rule_name, wf.entity, wf.field, wf.status_field,
                                    target_value,
                                    wf.qualifying_states.join(", ")
                                ),
                                source_construct: rule_name.clone(),
                                expression: None,
                                source_span: (rule_span.start, rule_span.end),
                                detail: Some(ObligationDetail::WhenPresence {
                                    rule: rule_name.clone(),
                                    entity: wf.entity.clone(),
                                    field: wf.field.clone(),
                                    source_state: "unknown".to_string(),
                                    target_state: target_value.clone(),
                                    qualifying_states: wf.qualifying_states.clone(),
                                }),
                            });
                        }
                    }
                }
            }
        }
    }
}

/// State extraction result: (object_variable, field_name, state_value)
/// e.g. `order.status = pending` → ("order", "status", "pending")
type StateInfo = (String, String, String);

/// Extract source states from requires clause.
/// Returns a list because `in {a, b}` produces multiple source states.
fn extract_requires_states(items: &[BlockItem]) -> Vec<StateInfo> {
    for item in items {
        if let BlockItemKind::Clause { keyword, value } = &item.kind {
            if keyword == "requires" {
                return extract_states_from_expr(value);
            }
        }
    }
    Vec::new()
}

/// Extract target state from ensures: `entity.status = state`
fn extract_ensures_target_state(items: &[BlockItem]) -> Option<StateInfo> {
    for item in items {
        if let BlockItemKind::Clause { keyword, value } = &item.kind {
            if keyword == "ensures" {
                let result = extract_eq_state(value);
                if result.is_some() {
                    return result;
                }
                if let Expr::Block { items: exprs, .. } = value {
                    for expr in exprs {
                        let result = extract_eq_state(expr);
                        if result.is_some() {
                            return result;
                        }
                    }
                }
            }
        }
    }
    None
}

/// Extract states from an expression. Handles `obj.field = state` and `obj.field in {a, b}`.
fn extract_states_from_expr(expr: &Expr) -> Vec<StateInfo> {
    // Single equality
    if let Some(info) = extract_eq_state(expr) {
        return vec![info];
    }
    // `obj.field in {a, b, c}`
    if let Expr::In { element, collection, .. } = expr {
        let (obj, field) = match element.as_ref() {
            Expr::MemberAccess { object, field, .. } => {
                if let Expr::Ident(id) = object.as_ref() {
                    (id.name.clone(), field.name.clone())
                } else {
                    return Vec::new();
                }
            }
            _ => return Vec::new(),
        };
        if let Expr::SetLiteral { elements, .. } = collection.as_ref() {
            return elements.iter().filter_map(|e| {
                if let Expr::Ident(id) = e {
                    Some((obj.clone(), field.clone(), id.name.clone()))
                } else {
                    None
                }
            }).collect();
        }
    }
    // LogicalOp (and/or) — check both sides
    if let Expr::LogicalOp { left, right, .. } = expr {
        let mut results = extract_states_from_expr(left);
        results.extend(extract_states_from_expr(right));
        return results;
    }
    Vec::new()
}

/// Extract `obj.field = value` → (obj, field, value)
fn extract_eq_state(expr: &Expr) -> Option<StateInfo> {
    if let Expr::Comparison { left, op: ComparisonOp::Eq, right, .. } = expr {
        let (obj, field) = match left.as_ref() {
            Expr::MemberAccess { object, field, .. } => {
                let obj_name = match object.as_ref() {
                    Expr::Ident(id) => id.name.clone(),
                    _ => String::new(),
                };
                (obj_name, field.name.clone())
            }
            Expr::Ident(id) => (String::new(), id.name.clone()),
            _ => return None,
        };
        let value_name = match right.as_ref() {
            Expr::Ident(id) => Some(id.name.clone()),
            _ => None,
        };
        if let Some(v) = value_name {
            return Some((obj, field, v));
        }
    }
    None
}

fn block_name(block: &BlockDecl) -> String {
    block.name.as_ref().map(|n| n.name.clone()).unwrap_or_default()
}

fn emit_entity_obligations(plan: &mut TestPlan, block: &BlockDecl, source: &str) {
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
            BlockItemKind::FieldWithWhen { name: field_name, value, when_clause } => {
                fields.push(field_name.name.clone());

                if matches!(value, Expr::TypeOptional { .. }) {
                    optional_fields.push(field_name.name.clone());
                }

                // Emit when-field-presence obligation
                let states: Vec<String> = when_clause.qualifying_states.iter().map(|s| s.name.clone()).collect();
                plan.obligations.push(Obligation {
                    id: format!("when-presence-{}-{}", name, field_name.name),
                    category: ObligationCategory::WhenFieldPresence,
                    description: format!(
                        "Verify {}.{} is present when {} in {{{}}} and absent otherwise",
                        name, field_name.name, when_clause.status_field.name,
                        states.join(", ")
                    ),
                    source_construct: format!("{}.{}", name, field_name.name),
                    expression: None,
                    source_span: (item.span.start, item.span.end),
                    detail: Some(ObligationDetail::WhenFieldPresence {
                        entity: name.clone(),
                        field: field_name.name.clone(),
                        status_field: when_clause.status_field.name.clone(),
                        qualifying_states: states,
                    }),
                });
            }
            BlockItemKind::TransitionsBlock(graph) => {
                emit_transition_obligations(plan, &name, graph);
            }
            BlockItemKind::InvariantBlock { name: inv_name, body } => {
                plan.obligations.push(Obligation {
                    id: format!("invariant-entity-{}-{}", name, inv_name.name),
                    category: ObligationCategory::InvariantProperty,
                    description: format!(
                        "Verify invariant {} holds after any field mutation on {}",
                        inv_name.name, name
                    ),
                    source_construct: format!("{}.{}", name, inv_name.name),
                    expression: Some(span_text(source, body.span())),
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
            expression: None,
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
            expression: None,
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
            expression: None,
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
            expression: None,
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
            expression: None,
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
            expression: None,
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
        expression: None,
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
            expression: None,
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
        expression: None,
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
            expression: None,
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
        expression: None,
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
        expression: None,
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
                    expression: None,
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
                        expression: None,
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
                        expression: None,
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
                        expression: None,
                        source_span: (item.span.start, item.span.end),
                        detail: None,
                    });
                }
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
        expression: None,
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
                    expression: None,
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
                    expression: None,
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
                expression: None,
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
                expression: None,
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
        expression: None,
        source_span: (v.span.start, v.span.end),
        detail: None,
    });
}

fn emit_invariant_obligation(plan: &mut TestPlan, inv: &InvariantDecl, source: &str) {
    plan.obligations.push(Obligation {
        id: format!("invariant-{}", inv.name.name),
        category: ObligationCategory::InvariantProperty,
        description: format!(
            "Verify invariant {} holds after every state-changing rule that touches constrained entities",
            inv.name.name
        ),
        source_construct: inv.name.name.clone(),
        expression: Some(span_text(source, inv.body.span())),
        source_span: (inv.span.start, inv.span.end),
        detail: None,
    });
}

// --- Helpers ---

fn span_text(source: &str, span: Span) -> String {
    source.get(span.start..span.end).unwrap_or("").to_string()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_plan(source: &str) -> TestPlan {
        let result = allium_parser::parse(source);
        generate_test_plan(&result.module, source)
    }

    fn to_json(plan: &TestPlan) -> serde_json::Value {
        serde_json::to_value(plan).unwrap()
    }

    fn find_obligation<'a>(plan: &'a TestPlan, id_substring: &str) -> &'a Obligation {
        plan.obligations.iter().find(|o| o.id.contains(id_substring))
            .unwrap_or_else(|| panic!("no obligation matching '{}'; have: {:?}",
                id_substring,
                plan.obligations.iter().map(|o| &o.id).collect::<Vec<_>>()))
    }

    fn obligations_matching<'a>(plan: &'a TestPlan, id_substring: &str) -> Vec<&'a Obligation> {
        plan.obligations.iter().filter(|o| o.id.contains(id_substring)).collect()
    }

    // --- Version passthrough ---

    #[test]
    fn version_passed_through() {
        let plan = parse_plan("-- allium: 3\nentity Foo { x: Integer }");
        assert_eq!(plan.version, Some(3));
    }

    #[test]
    fn version_none_when_absent() {
        let plan = parse_plan("entity Foo { x: Integer }");
        assert_eq!(plan.version, None);
    }

    // --- Entity field obligations ---

    #[test]
    fn entity_fields_obligation_lists_fields() {
        let source = "-- allium: 3\nentity Order {\n  name: String\n  total: Integer\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "entity-fields-Order");
        assert!(matches!(ob.category, ObligationCategory::EntityFields));
        if let Some(ObligationDetail::Fields { fields }) = &ob.detail {
            assert_eq!(fields, &["name", "total"]);
        } else {
            panic!("expected Fields detail");
        }
    }

    #[test]
    fn entity_fields_source_construct() {
        let source = "-- allium: 3\nentity Widget { size: Integer }";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "entity-fields-Widget");
        assert_eq!(ob.source_construct, "Widget");
    }

    #[test]
    fn empty_entity_no_fields_obligation() {
        let plan = parse_plan("-- allium: 3\nentity Empty {}");
        let matching = obligations_matching(&plan, "entity-fields-Empty");
        assert!(matching.is_empty());
    }

    // --- Optional field obligations ---

    #[test]
    fn optional_field_obligation() {
        let source = "-- allium: 3\nentity User {\n  bio: String?\n  name: String\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "entity-optional-User-bio");
        assert!(matches!(ob.category, ObligationCategory::EntityOptional));
        assert!(ob.description.contains("bio"));
    }

    #[test]
    fn non_optional_field_no_optional_obligation() {
        let source = "-- allium: 3\nentity User { name: String }";
        let plan = parse_plan(source);
        let matching = obligations_matching(&plan, "entity-optional");
        assert!(matching.is_empty());
    }

    // --- Relationship obligations ---

    #[test]
    fn relationship_obligation() {
        let source = "-- allium: 3\nentity Order { items: OrderItem with order }";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "entity-relationship-Order-items");
        assert!(matches!(ob.category, ObligationCategory::EntityRelationship));
    }

    // --- Projection obligations ---

    #[test]
    fn projection_obligation() {
        let source = "-- allium: 3\nentity Order {\n  items: OrderItem with order\n  active_items: items where active = true\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "projection-Order-active_items");
        assert!(matches!(ob.category, ObligationCategory::Projection));
    }

    // --- Derived value obligations ---

    #[test]
    fn derived_value_obligation() {
        let source = "-- allium: 3\nentity Order {\n  a: Integer\n  b: Integer\n  total: a + b\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "derived-Order-total");
        assert!(matches!(ob.category, ObligationCategory::DerivedValue));
        assert!(ob.expression.is_none());
    }

    // --- Transition obligations ---

    #[test]
    fn transition_edge_obligations() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | shipped | done\n  transitions status {\n    pending -> shipped\n    shipped -> done\n    terminal: done\n  }\n}";
        let plan = parse_plan(source);
        let edge1 = find_obligation(&plan, "transition-edge-Order-pending-shipped");
        assert!(matches!(edge1.category, ObligationCategory::TransitionEdge));
        if let Some(ObligationDetail::Transition { from, to, .. }) = &edge1.detail {
            assert_eq!(from, "pending");
            assert_eq!(to, "shipped");
        } else {
            panic!("expected Transition detail");
        }
        find_obligation(&plan, "transition-edge-Order-shipped-done");
    }

    #[test]
    fn transition_rejected_obligation() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | done\n  transitions status {\n    pending -> done\n    terminal: done\n  }\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "transition-rejected-Order");
        assert!(matches!(ob.category, ObligationCategory::TransitionRejected));
    }

    #[test]
    fn transition_terminal_obligation() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | done\n  transitions status {\n    pending -> done\n    terminal: done\n  }\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "transition-terminal-Order");
        assert!(matches!(ob.category, ObligationCategory::TransitionTerminal));
        if let Some(ObligationDetail::Terminal { states, .. }) = &ob.detail {
            assert_eq!(states, &["done"]);
        } else {
            panic!("expected Terminal detail");
        }
    }

    #[test]
    fn no_terminal_obligation_when_no_terminal_states() {
        let source = "-- allium: 3\nentity Order {\n  status: a | b\n  transitions status {\n    a -> b\n    b -> a\n  }\n}";
        let plan = parse_plan(source);
        let matching = obligations_matching(&plan, "transition-terminal");
        assert!(matching.is_empty());
    }

    // --- When field presence ---

    #[test]
    fn when_field_presence_obligation() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | shipped\n  tracking: String when status = shipped\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "when-presence-Order-tracking");
        assert!(matches!(ob.category, ObligationCategory::WhenFieldPresence));
        if let Some(ObligationDetail::WhenFieldPresence { entity, field, status_field, qualifying_states }) = &ob.detail {
            assert_eq!(entity, "Order");
            assert_eq!(field, "tracking");
            assert_eq!(status_field, "status");
            assert_eq!(qualifying_states, &["shipped"]);
        } else {
            panic!("expected WhenFieldPresence detail");
        }
    }

    // --- Value type obligations ---

    #[test]
    fn value_type_equality_obligation() {
        let source = "-- allium: 3\nvalue Address {\n  street: String\n  city: String\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "value-equality-Address");
        assert!(matches!(ob.category, ObligationCategory::ValueEquality));
    }

    #[test]
    fn value_type_fields_obligation() {
        let source = "-- allium: 3\nvalue Address {\n  street: String\n  city: String\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "entity-fields-Address");
        if let Some(ObligationDetail::Fields { fields }) = &ob.detail {
            assert_eq!(fields, &["street", "city"]);
        } else {
            panic!("expected Fields detail");
        }
    }

    // --- Enum obligations ---

    #[test]
    fn enum_comparable_obligation() {
        let source = "-- allium: 3\nenum Colour {\n  red\n  green\n  blue\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "enum-comparable-Colour");
        assert!(matches!(ob.category, ObligationCategory::EnumComparable));
    }

    // --- Rule obligations ---

    #[test]
    fn rule_success_obligation() {
        let source = "-- allium: 3\nrule DoThing {\n  requires: x = 1\n  ensures: x = 2\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "rule-success-DoThing");
        assert!(matches!(ob.category, ObligationCategory::RuleSuccess));
        assert!(ob.expression.is_none());
    }

    #[test]
    fn rule_failure_obligation_from_requires() {
        let source = "-- allium: 3\nrule DoThing {\n  requires: x = 1\n  ensures: x = 2\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "rule-failure-DoThing");
        assert!(matches!(ob.category, ObligationCategory::RuleFailure));
    }

    #[test]
    fn rule_without_requires_no_failure_obligation() {
        let source = "-- allium: 3\nrule DoThing {\n  ensures: x = 2\n}";
        let plan = parse_plan(source);
        let matching = obligations_matching(&plan, "rule-failure");
        assert!(matching.is_empty());
    }

    // --- Surface obligations ---

    #[test]
    fn surface_actor_obligation() {
        let source = "-- allium: 3\nsurface Dashboard {\n  facing: admin\n  exposes: Order.status\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "surface-actor-Dashboard");
        assert!(matches!(ob.category, ObligationCategory::SurfaceActor));
    }

    #[test]
    fn surface_exposure_obligation() {
        let source = "-- allium: 3\nsurface Dashboard {\n  facing: admin\n  exposes: Order.status\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "surface-exposure-Dashboard");
        assert!(matches!(ob.category, ObligationCategory::SurfaceExposure));
    }

    #[test]
    fn surface_provides_obligation() {
        let source = "-- allium: 3\nsurface Dashboard {\n  facing: admin\n  provides: Order.cancel\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "surface-provides-Dashboard");
        assert!(matches!(ob.category, ObligationCategory::SurfaceProvides));
    }

    // --- Config obligations ---

    #[test]
    fn config_default_obligation() {
        let source = "-- allium: 3\nconfig {\n  max_retries: Integer = 3\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "config-default-max_retries");
        assert!(matches!(ob.category, ObligationCategory::ConfigDefault));
        assert_eq!(ob.source_construct, "config.max_retries");
    }

    #[test]
    fn config_multiple_params() {
        let source = "-- allium: 3\nconfig {\n  max_retries: Integer = 3\n  batch_size: Integer = 10\n}";
        let plan = parse_plan(source);
        find_obligation(&plan, "config-default-max_retries");
        find_obligation(&plan, "config-default-batch_size");
    }

    // --- Contract obligations ---

    #[test]
    fn contract_signature_obligation() {
        let source = "-- allium: 3\ncontract PaymentGateway {\n  charge: Amount -> Result\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "contract-sig-PaymentGateway-charge");
        assert!(matches!(ob.category, ObligationCategory::ContractSignature));
        assert_eq!(ob.source_construct, "PaymentGateway.charge");
    }

    // --- Variant obligations ---

    #[test]
    fn variant_obligation() {
        let source = "-- allium: 3\nvariant NetworkError : Error {\n  code: Integer\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "sum-type-variant-NetworkError");
        assert!(matches!(ob.category, ObligationCategory::SumTypeVariant));
    }

    // --- Entity-level invariant obligations ---

    #[test]
    fn entity_invariant_obligation_includes_expression() {
        let source = "-- allium: 3\nentity Order {\n  total: Integer\n  invariant NonNeg { this.total >= 0 }\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "invariant-entity-Order-NonNeg");
        assert_eq!(ob.expression.as_deref(), Some("this.total >= 0"));
    }

    #[test]
    fn entity_invariant_obligation_category_and_description() {
        let source = "-- allium: 3\nentity Foo {\n  x: Integer\n  invariant XPos { this.x >= 0 }\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "invariant-entity-Foo-XPos");
        assert!(matches!(ob.category, ObligationCategory::InvariantProperty));
        assert!(ob.description.contains("XPos"));
        assert!(ob.description.contains("Foo"));
    }

    #[test]
    fn entity_invariant_source_construct() {
        let source = "-- allium: 3\nentity Foo {\n  x: Integer\n  invariant XPos { this.x >= 0 }\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "invariant-entity-Foo-XPos");
        assert_eq!(ob.source_construct, "Foo.XPos");
    }

    // --- Top-level invariant obligations ---

    #[test]
    fn top_level_invariant_obligation_includes_expression() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | done\n}\ninvariant AllDone {\n  for o in Orders: o.status = done\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "invariant-AllDone");
        assert!(ob.expression.is_some());
        let expr = ob.expression.as_deref().unwrap();
        assert!(expr.contains("for o in Orders"), "expression was: {}", expr);
    }

    #[test]
    fn top_level_invariant_complex_expression_preserved() {
        let source = "-- allium: 3\nentity Account {\n  balance: Decimal\n}\ninvariant AllSolvent {\n  for a in Accounts: a.balance >= 0\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "invariant-AllSolvent");
        let expr = ob.expression.as_deref().unwrap();
        assert!(expr.contains("a.balance >= 0"), "expression was: {}", expr);
    }

    #[test]
    fn top_level_invariant_source_construct() {
        let source = "-- allium: 3\ninvariant GlobalCheck {\n  true\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "invariant-GlobalCheck");
        assert_eq!(ob.source_construct, "GlobalCheck");
    }

    // --- Expression field absent on non-invariant obligations ---

    #[test]
    fn entity_fields_obligation_omits_expression() {
        let source = "-- allium: 3\nentity Order {\n  total: Integer\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "entity-fields-Order");
        assert!(ob.expression.is_none());
    }

    #[test]
    fn expression_omitted_from_json_when_none() {
        let source = "-- allium: 3\nentity Order {\n  total: Integer\n}";
        let plan = parse_plan(source);
        let json = to_json(&plan);
        let ob = json["obligations"].as_array().unwrap()
            .iter().find(|o| o["id"].as_str().unwrap().contains("entity-fields"))
            .unwrap();
        assert!(ob.get("expression").is_none(), "expression key should be absent from JSON");
    }

    #[test]
    fn expression_present_in_json_for_invariant() {
        let source = "-- allium: 3\nentity Foo {\n  x: Integer\n  invariant XPos { this.x >= 0 }\n}";
        let plan = parse_plan(source);
        let json = to_json(&plan);
        let ob = json["obligations"].as_array().unwrap()
            .iter().find(|o| o["id"].as_str().unwrap().contains("invariant-entity"))
            .unwrap();
        assert_eq!(ob["expression"].as_str().unwrap(), "this.x >= 0");
    }

    #[test]
    fn rule_obligation_omits_expression() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | done\n}\nrule Confirm {\n  requires: status = pending\n  ensures: status = done\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "rule-success-Confirm");
        assert!(ob.expression.is_none());
    }

    // --- Multiple invariants on one entity ---

    #[test]
    fn multiple_entity_invariants_each_have_expression() {
        let source = "-- allium: 3\nentity Gauge {\n  level: Integer\n  invariant MinLevel { this.level >= 0 }\n  invariant MaxLevel { this.level <= 100 }\n}";
        let plan = parse_plan(source);
        let min = find_obligation(&plan, "MinLevel");
        let max = find_obligation(&plan, "MaxLevel");
        assert_eq!(min.expression.as_deref(), Some("this.level >= 0"));
        assert_eq!(max.expression.as_deref(), Some("this.level <= 100"));
    }

    // --- Source spans are non-zero ---

    #[test]
    fn obligation_source_spans_are_nonzero() {
        let source = "-- allium: 3\nentity Order {\n  total: Integer\n  invariant NonNeg { this.total >= 0 }\n}";
        let plan = parse_plan(source);
        for ob in &plan.obligations {
            let (start, end) = ob.source_span;
            assert!(end > start, "obligation {} has zero-width span", ob.id);
        }
    }

    // --- Temporal trigger obligations ---

    #[test]
    fn temporal_trigger_obligation() {
        let source = "-- allium: 3\nrule Timeout {\n  when: o: Order.created_at + 48.hours <= now\n  ensures: o.status = cancelled\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "temporal-Timeout");
        assert!(matches!(ob.category, ObligationCategory::TemporalTrigger));
    }

    // --- Entity creation obligations ---

    #[test]
    fn entity_creation_obligation_from_ensures() {
        let source = "-- allium: 3\nrule Notify {\n  when: order: Order.status transitions_to shipped\n  ensures: Email.created(to: order.customer.email)\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "rule-entity-creation-Notify");
        assert!(matches!(ob.category, ObligationCategory::RuleEntityCreation));
    }

    // --- When crossing obligations ---

    #[test]
    fn when_crossing_presence_obligation() {
        let source = "-- allium: 3\n\
            entity Order {\n  status: pending | shipped\n  tracking: String when status = shipped\n  \
            transitions status {\n    pending -> shipped\n    terminal: shipped\n  }\n}\n\
            rule Ship {\n  when: Ship(order)\n  requires: order.status = pending\n  ensures: order.status = shipped\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "when-set-Ship-Order-tracking");
        assert!(matches!(ob.category, ObligationCategory::WhenPresenceObligation));
        if let Some(ObligationDetail::WhenPresence { rule, entity, field, source_state, target_state, qualifying_states }) = &ob.detail {
            assert_eq!(rule, "Ship");
            assert_eq!(entity, "Order");
            assert_eq!(field, "tracking");
            assert_eq!(source_state, "pending");
            assert_eq!(target_state, "shipped");
            assert_eq!(qualifying_states, &["shipped"]);
        } else {
            panic!("expected WhenPresence detail");
        }
    }

    #[test]
    fn when_crossing_absence_obligation() {
        let source = "-- allium: 3\n\
            entity Order {\n  status: active | shipped | cancelled\n  tracking: String when status = shipped\n  \
            transitions status {\n    active -> shipped\n    shipped -> cancelled\n    terminal: cancelled\n  }\n}\n\
            rule Cancel {\n  when: Cancel(order)\n  requires: order.status = shipped\n  ensures: order.status = cancelled\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "when-clear-Cancel-Order-tracking");
        assert!(matches!(ob.category, ObligationCategory::WhenAbsenceObligation));
        if let Some(ObligationDetail::WhenAbsence { rule, entity, field, source_state, target_state, .. }) = &ob.detail {
            assert_eq!(rule, "Cancel");
            assert_eq!(entity, "Order");
            assert_eq!(field, "tracking");
            assert_eq!(source_state, "shipped");
            assert_eq!(target_state, "cancelled");
        } else {
            panic!("expected WhenAbsence detail");
        }
    }

    // --- External entity ---

    #[test]
    fn external_entity_emits_field_obligations() {
        let source = "-- allium: 3\nexternal entity Customer {\n  email: String\n  name: String\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "entity-fields-Customer");
        if let Some(ObligationDetail::Fields { fields }) = &ob.detail {
            assert_eq!(fields, &["email", "name"]);
        } else {
            panic!("expected Fields detail");
        }
    }

    // --- Surface exposes detail ---

    #[test]
    fn surface_exposure_detail_lists_items() {
        let source = "-- allium: 3\nsurface Dashboard {\n  facing: admin\n  exposes:\n    Order.status\n    Order.tracking_number\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "surface-exposure-Dashboard");
        if let Some(ObligationDetail::Surface { surface, items }) = &ob.detail {
            assert_eq!(surface, "Dashboard");
            assert_eq!(items, &["status", "tracking_number"]);
        } else {
            panic!("expected Surface detail, got {:?}", ob.detail);
        }
    }

    // --- Multiple when-qualifying states ---

    #[test]
    fn when_field_multiple_qualifying_states() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | shipped | delivered\n  tracking: String when status = shipped | delivered\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "when-presence-Order-tracking");
        if let Some(ObligationDetail::WhenFieldPresence { qualifying_states, .. }) = &ob.detail {
            assert_eq!(qualifying_states, &["shipped", "delivered"]);
        } else {
            panic!("expected WhenFieldPresence detail");
        }
    }

    // --- For-block and if-block recursion in rules ---

    #[test]
    fn for_block_entity_creation_in_rule() {
        let source = "-- allium: 3\nrule NotifyAll {\n  when: NotifyAll(orders)\n  for order in orders:\n    ensures: Email.created(to: order.email)\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "rule-entity-creation-NotifyAll");
        assert!(matches!(ob.category, ObligationCategory::RuleEntityCreation));
    }

    #[test]
    fn if_block_entity_creation_in_rule() {
        let source = "-- allium: 3\nrule Process {\n  when: Process(order, express)\n  if express:\n    ensures: Express.created(order: order)\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "rule-entity-creation-Process");
        assert!(matches!(ob.category, ObligationCategory::RuleEntityCreation));
    }

    // --- When-crossing: no requires (conservative fallback) ---

    #[test]
    fn when_crossing_presence_without_requires() {
        let source = "-- allium: 3\n\
            entity Order {\n  status: pending | shipped\n  tracking: String when status = shipped\n  \
            transitions status {\n    pending -> shipped\n    terminal: shipped\n  }\n}\n\
            rule AutoShip {\n  when: AutoShip(order)\n  ensures: order.status = shipped\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "when-set-AutoShip-Order-tracking");
        assert!(matches!(ob.category, ObligationCategory::WhenPresenceObligation));
        if let Some(ObligationDetail::WhenPresence { source_state, .. }) = &ob.detail {
            assert_eq!(source_state, "unknown");
        } else {
            panic!("expected WhenPresence detail");
        }
    }

    // --- When-crossing: requires with in-set syntax ---

    #[test]
    fn when_crossing_with_in_set_requires() {
        let source = "-- allium: 3\n\
            entity Order {\n  status: pending | confirmed | shipped\n  tracking: String when status = shipped\n  \
            transitions status {\n    pending -> shipped\n    confirmed -> shipped\n    terminal: shipped\n  }\n}\n\
            rule Ship {\n  when: Ship(order)\n  requires: order.status in {pending, confirmed}\n  ensures: order.status = shipped\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "when-set-Ship-Order-tracking");
        assert!(matches!(ob.category, ObligationCategory::WhenPresenceObligation));
    }

    // --- Transition rejected: verify source_construct ---

    #[test]
    fn transition_rejected_describes_field() {
        let source = "-- allium: 3\nentity Order {\n  status: a | b\n  transitions status {\n    a -> b\n  }\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "transition-rejected-Order");
        assert_eq!(ob.source_construct, "Order.status");
        assert!(ob.description.contains("Order") && ob.description.contains("status"));
    }

    // --- Multiple requires clauses ---

    #[test]
    fn multiple_requires_produce_multiple_failure_obligations() {
        let source = "-- allium: 3\nrule DoThing {\n  requires: x = 1\n  requires: y = 2\n  ensures: z = 3\n}";
        let plan = parse_plan(source);
        let failures = obligations_matching(&plan, "rule-failure-DoThing");
        assert_eq!(failures.len(), 2);
    }

    // --- Entity creation inside multi-line ensures block ---

    #[test]
    fn entity_creation_in_block_ensures() {
        let source = "-- allium: 3\nrule Notify {\n  when: Notify(order)\n  ensures:\n    order.status = shipped\n    Email.created(to: order.email)\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "rule-entity-creation-Notify");
        assert!(matches!(ob.category, ObligationCategory::RuleEntityCreation));
    }

    // --- Empty config ---

    #[test]
    fn empty_config_no_obligations() {
        let plan = parse_plan("-- allium: 3\nconfig {}");
        let matching = obligations_matching(&plan, "config-default");
        assert!(matching.is_empty());
    }

    // --- ParamAssignment excluded from fields and derived ---

    #[test]
    fn param_assignment_excluded_from_fields_and_derived() {
        let source = "-- allium: 3\nentity Order {\n  subtotal: Decimal\n  total(tax_rate): subtotal * tax_rate\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "entity-fields-Order");
        if let Some(ObligationDetail::Fields { fields }) = &ob.detail {
            assert!(fields.contains(&"subtotal".to_string()));
            assert!(!fields.contains(&"total".to_string()));
        } else {
            panic!("expected Fields detail");
        }
        let derived = obligations_matching(&plan, "derived-Order-total");
        assert!(derived.is_empty());
    }

    // --- Full round-trip: mixed spec produces correct obligation set ---

    #[test]
    fn mixed_spec_obligation_categories() {
        let source = "\
-- allium: 3
entity Order {
  total: Integer
  status: pending | done
  transitions status {
    pending -> done
    terminal: done
  }
  invariant NonNeg { this.total >= 0 }
}
enum Priority { low high }
value Money { amount: Decimal }
config { max_retries: Integer = 3 }
invariant GlobalCheck { for o in Orders: o.total >= 0 }";
        let plan = parse_plan(source);

        // Spot-check that each category is present
        find_obligation(&plan, "entity-fields-Order");
        find_obligation(&plan, "transition-edge-Order-pending-done");
        find_obligation(&plan, "transition-rejected-Order");
        find_obligation(&plan, "transition-terminal-Order");
        find_obligation(&plan, "invariant-entity-Order-NonNeg");
        find_obligation(&plan, "enum-comparable-Priority");
        find_obligation(&plan, "value-equality-Money");
        find_obligation(&plan, "config-default-max_retries");
        find_obligation(&plan, "invariant-GlobalCheck");

        // Invariants have expressions, others don't
        assert!(find_obligation(&plan, "invariant-entity-Order-NonNeg").expression.is_some());
        assert!(find_obligation(&plan, "invariant-GlobalCheck").expression.is_some());
        assert!(find_obligation(&plan, "entity-fields-Order").expression.is_none());
        assert!(find_obligation(&plan, "config-default-max_retries").expression.is_none());
    }
}
