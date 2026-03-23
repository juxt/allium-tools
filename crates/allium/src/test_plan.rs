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
    /// Rule dependency analysis, present on rule obligations only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<RuleDependencies>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    External,
    StateTransition,
    Temporal,
    Creation,
    Chained,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleDependencies {
    pub entities_read: Vec<String>,
    pub entities_written: Vec<String>,
    pub entities_created: Vec<String>,
    pub entities_removed: Vec<String>,
    pub deferred_specs: Vec<String>,
    pub trigger_emissions: Vec<String>,
    pub trigger_source: TriggerSource,
}

/// Module-level context for dependency analysis.
struct ModuleContext {
    entity_names: std::collections::BTreeSet<String>,
    deferred_names: std::collections::BTreeSet<String>,
    /// Trigger names emitted in ensures clauses across all rules.
    emitted_triggers: std::collections::BTreeSet<String>,
}

pub fn generate_test_plan(module: &Module, source: &str) -> TestPlan {
    let mut plan = TestPlan {
        version: module.version,
        obligations: Vec::new(),
    };

    let ctx = build_module_context(module);

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
                    emit_rule_obligations(&mut plan, block, &ctx);
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
                                        dependencies: None,
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
                                        dependencies: None,
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
                                dependencies: None,
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
                    dependencies: None,
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
                    dependencies: None,
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
            dependencies: None,
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
            dependencies: None,
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
            dependencies: None,
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
            dependencies: None,
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
            dependencies: None,
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
            dependencies: None,
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
        dependencies: None,
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
            dependencies: None,
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
        dependencies: None,
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
            dependencies: None,
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
        dependencies: None,
    });
}

fn emit_rule_obligations(plan: &mut TestPlan, block: &BlockDecl, ctx: &ModuleContext) {
    let name = block_name(block);
    let deps = extract_rule_dependencies(block, ctx);

    // Success case
    plan.obligations.push(Obligation {
        id: format!("rule-success-{}", name),
        category: ObligationCategory::RuleSuccess,
        description: format!("Verify rule {} succeeds when all preconditions are met", name),
        source_construct: name.clone(),
        expression: None,
        source_span: (block.span.start, block.span.end),
        detail: None,
        dependencies: Some(deps.clone()),
    });

    // Walk items for specific obligations
    walk_rule_items(plan, &name, &block.items, block.span, &deps);
}

fn walk_rule_items(plan: &mut TestPlan, rule_name: &str, items: &[BlockItem], _block_span: Span, deps: &RuleDependencies) {
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
                    dependencies: Some(deps.clone()),
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
                        dependencies: Some(deps.clone()),
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
                        dependencies: Some(deps.clone()),
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
                        dependencies: Some(deps.clone()),
                    });
                }
            }
            BlockItemKind::ForBlock { items, .. } => {
                walk_rule_items(plan, rule_name, items, _block_span, deps);
            }
            BlockItemKind::IfBlock { branches, else_items } => {
                for branch in branches {
                    walk_rule_items(plan, rule_name, &branch.items, _block_span, deps);
                }
                if let Some(else_items) = else_items {
                    walk_rule_items(plan, rule_name, else_items, _block_span, deps);
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
        dependencies: None,
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
                    dependencies: None,
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
                    dependencies: None,
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
                dependencies: None,
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
                dependencies: None,
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
        dependencies: None,
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
        dependencies: None,
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

// --- Module context and dependency extraction ---

fn build_module_context(module: &Module) -> ModuleContext {
    let mut entity_names = std::collections::BTreeSet::new();
    let mut deferred_names = std::collections::BTreeSet::new();
    let mut emitted_triggers = std::collections::BTreeSet::new();

    for decl in &module.declarations {
        match decl {
            Decl::Block(block) => {
                match block.kind {
                    BlockKind::Entity | BlockKind::ExternalEntity => {
                        if let Some(name) = &block.name {
                            entity_names.insert(name.name.clone());
                        }
                    }
                    BlockKind::Rule => {
                        // Scan ensures clauses for trigger emissions
                        for item in &block.items {
                            if let BlockItemKind::Clause { keyword, value } = &item.kind {
                                if keyword == "ensures" {
                                    collect_trigger_emissions(value, &mut emitted_triggers);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Decl::Deferred(d) => {
                if let Some(name) = deferred_leaf_name(&d.path) {
                    deferred_names.insert(name);
                }
            }
            _ => {}
        }
    }

    ModuleContext { entity_names, deferred_names, emitted_triggers }
}

/// Extract the leaf name from a deferred spec path expression.
fn deferred_leaf_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(id) => Some(id.name.clone()),
        Expr::MemberAccess { field, .. } => Some(field.name.clone()),
        Expr::QualifiedName(q) => Some(q.name.clone()),
        _ => None,
    }
}

/// Collect trigger emission names from an ensures expression.
/// A trigger emission is `TriggerName(args)` — a call with an uppercase bare ident function.
fn collect_trigger_emissions(expr: &Expr, out: &mut std::collections::BTreeSet<String>) {
    match expr {
        Expr::Call { function, args, .. } => {
            if let Expr::Ident(id) = function.as_ref() {
                if id.name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    // Check it's not Entity.created() — bare Ident means it's a trigger, not a method
                    out.insert(id.name.clone());
                }
            }
            // Recurse into args
            for arg in args {
                match arg {
                    CallArg::Positional(e) => collect_trigger_emissions(e, out),
                    CallArg::Named(na) => collect_trigger_emissions(&na.value, out),
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_trigger_emissions(item, out);
            }
        }
        Expr::Binding { value, .. } | Expr::LetExpr { value, .. } => {
            collect_trigger_emissions(value, out);
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_trigger_emissions(&b.body, out);
            }
            if let Some(eb) = else_body {
                collect_trigger_emissions(eb, out);
            }
        }
        Expr::For { body, .. } => collect_trigger_emissions(body, out),
        _ => {}
    }
}

fn extract_rule_dependencies(block: &BlockDecl, ctx: &ModuleContext) -> RuleDependencies {
    let mut entities_read = std::collections::BTreeSet::new();
    let mut entities_written = std::collections::BTreeSet::new();
    let mut entities_created = std::collections::BTreeSet::new();
    let mut entities_removed = std::collections::BTreeSet::new();
    let mut deferred_specs = std::collections::BTreeSet::new();
    let mut trigger_emissions = std::collections::BTreeSet::new();
    let mut trigger_source = TriggerSource::External;

    for item in &block.items {
        if let BlockItemKind::Clause { keyword, value } = &item.kind {
            match keyword.as_str() {
                "when" => {
                    trigger_source = classify_trigger_source(value, &ctx.emitted_triggers);
                    collect_entity_refs(value, &ctx.entity_names, &mut entities_read);
                    collect_deferred_refs(value, &ctx.deferred_names, &mut deferred_specs);
                }
                "requires" => {
                    collect_entity_refs(value, &ctx.entity_names, &mut entities_read);
                    collect_deferred_refs(value, &ctx.deferred_names, &mut deferred_specs);
                }
                "ensures" => {
                    collect_written_entities(value, &ctx.entity_names, &mut entities_written);
                    collect_created_entities(value, &mut entities_created);
                    collect_removed_entities(value, &mut entities_removed);
                    collect_ensures_trigger_emissions(value, &ctx.entity_names, &mut trigger_emissions);
                    collect_deferred_refs(value, &ctx.deferred_names, &mut deferred_specs);
                }
                _ => {}
            }
        }
        // Also walk ForBlock/IfBlock items for nested clauses
        if let BlockItemKind::ForBlock { items, .. } = &item.kind {
            extract_deps_from_items(items, ctx, &mut entities_read, &mut entities_written,
                &mut entities_created, &mut entities_removed, &mut deferred_specs, &mut trigger_emissions);
        }
        if let BlockItemKind::IfBlock { branches, else_items } = &item.kind {
            for branch in branches {
                extract_deps_from_items(&branch.items, ctx, &mut entities_read, &mut entities_written,
                    &mut entities_created, &mut entities_removed, &mut deferred_specs, &mut trigger_emissions);
            }
            if let Some(items) = else_items {
                extract_deps_from_items(items, ctx, &mut entities_read, &mut entities_written,
                    &mut entities_created, &mut entities_removed, &mut deferred_specs, &mut trigger_emissions);
            }
        }
    }

    RuleDependencies {
        entities_read: entities_read.into_iter().collect(),
        entities_written: entities_written.into_iter().collect(),
        entities_created: entities_created.into_iter().collect(),
        entities_removed: entities_removed.into_iter().collect(),
        deferred_specs: deferred_specs.into_iter().collect(),
        trigger_emissions: trigger_emissions.into_iter().collect(),
        trigger_source,
    }
}

fn extract_deps_from_items(
    items: &[BlockItem],
    ctx: &ModuleContext,
    entities_read: &mut std::collections::BTreeSet<String>,
    entities_written: &mut std::collections::BTreeSet<String>,
    entities_created: &mut std::collections::BTreeSet<String>,
    entities_removed: &mut std::collections::BTreeSet<String>,
    deferred_specs: &mut std::collections::BTreeSet<String>,
    trigger_emissions: &mut std::collections::BTreeSet<String>,
) {
    for item in items {
        if let BlockItemKind::Clause { keyword, value } = &item.kind {
            match keyword.as_str() {
                "requires" => {
                    collect_entity_refs(value, &ctx.entity_names, entities_read);
                    collect_deferred_refs(value, &ctx.deferred_names, deferred_specs);
                }
                "ensures" => {
                    collect_written_entities(value, &ctx.entity_names, entities_written);
                    collect_created_entities(value, entities_created);
                    collect_removed_entities(value, entities_removed);
                    collect_ensures_trigger_emissions(value, &ctx.entity_names, trigger_emissions);
                    collect_deferred_refs(value, &ctx.deferred_names, deferred_specs);
                }
                _ => {}
            }
        }
        if let BlockItemKind::ForBlock { items: nested, .. } = &item.kind {
            extract_deps_from_items(nested, ctx, entities_read, entities_written,
                entities_created, entities_removed, deferred_specs, trigger_emissions);
        }
        if let BlockItemKind::IfBlock { branches, else_items } = &item.kind {
            for branch in branches {
                extract_deps_from_items(&branch.items, ctx, entities_read, entities_written,
                    entities_created, entities_removed, deferred_specs, trigger_emissions);
            }
            if let Some(nested) = else_items {
                extract_deps_from_items(nested, ctx, entities_read, entities_written,
                    entities_created, entities_removed, deferred_specs, trigger_emissions);
            }
        }
    }
}

/// Classify how a rule is triggered from its when clause.
fn classify_trigger_source(
    expr: &Expr,
    emitted_triggers: &std::collections::BTreeSet<String>,
) -> TriggerSource {
    match expr {
        Expr::TransitionsTo { .. } | Expr::Becomes { .. } => TriggerSource::StateTransition,
        Expr::Binding { value, .. } => classify_trigger_source(value, emitted_triggers),
        Expr::Block { items, .. } => {
            // Use the first classifiable item
            for item in items {
                let ts = classify_trigger_source(item, emitted_triggers);
                if !matches!(ts, TriggerSource::External) {
                    return ts;
                }
            }
            TriggerSource::External
        }
        Expr::Comparison { right, .. } => {
            if matches!(right.as_ref(), Expr::Now { .. }) {
                TriggerSource::Temporal
            } else {
                TriggerSource::External
            }
        }
        Expr::Call { function, .. } => {
            match function.as_ref() {
                Expr::MemberAccess { field, .. } if field.name == "created" => {
                    TriggerSource::Creation
                }
                Expr::Ident(id) => {
                    if emitted_triggers.contains(&id.name) {
                        TriggerSource::Chained
                    } else {
                        TriggerSource::External
                    }
                }
                _ => TriggerSource::External,
            }
        }
        _ => TriggerSource::External,
    }
}

/// Collect entity references from an expression, resolving lowercase variables
/// by capitalising and checking against known entity names.
fn collect_entity_refs(
    expr: &Expr,
    entity_names: &std::collections::BTreeSet<String>,
    out: &mut std::collections::BTreeSet<String>,
) {
    match expr {
        Expr::Ident(id) => {
            if entity_names.contains(&id.name) {
                out.insert(id.name.clone());
            }
        }
        Expr::MemberAccess { object, .. } | Expr::OptionalAccess { object, .. } => {
            if let Expr::Ident(id) = object.as_ref() {
                if entity_names.contains(&id.name) {
                    out.insert(id.name.clone());
                } else {
                    let cap = capitalize_first(&id.name);
                    if entity_names.contains(&cap) {
                        out.insert(cap);
                    }
                }
            } else {
                collect_entity_refs(object, entity_names, out);
            }
        }
        Expr::QualifiedName(q) => {
            if entity_names.contains(&q.name) {
                out.insert(q.name.clone());
            }
        }
        Expr::Call { function, args, .. } => {
            // Don't treat bare function names as entity refs (they're trigger names).
            // But do recurse into MemberAccess functions (Entity.method) and args.
            if let Expr::MemberAccess { object, .. } = function.as_ref() {
                collect_entity_refs(object, entity_names, out);
            }
            for arg in args {
                match arg {
                    CallArg::Positional(e) => collect_entity_refs(e, entity_names, out),
                    CallArg::Named(na) => collect_entity_refs(&na.value, entity_names, out),
                }
            }
        }
        Expr::JoinLookup { entity, fields, .. } => {
            collect_entity_refs(entity, entity_names, out);
            for f in fields {
                if let Some(v) = &f.value {
                    collect_entity_refs(v, entity_names, out);
                }
            }
        }
        // Two-child nodes
        Expr::BinaryOp { left, right, .. }
        | Expr::Comparison { left, right, .. }
        | Expr::LogicalOp { left, right, .. }
        | Expr::In { element: left, collection: right, .. }
        | Expr::NotIn { element: left, collection: right, .. }
        | Expr::NullCoalesce { left, right, .. }
        | Expr::Where { source: left, condition: right, .. }
        | Expr::With { source: left, predicate: right, .. }
        | Expr::Pipe { left, right, .. }
        | Expr::TransitionsTo { subject: left, new_state: right, .. }
        | Expr::Becomes { subject: left, new_state: right, .. }
        | Expr::WhenGuard { action: left, condition: right, .. } => {
            collect_entity_refs(left, entity_names, out);
            collect_entity_refs(right, entity_names, out);
        }
        // One-child nodes
        Expr::Not { operand, .. }
        | Expr::Exists { operand, .. }
        | Expr::NotExists { operand, .. }
        | Expr::TypeOptional { inner: operand, .. } => {
            collect_entity_refs(operand, entity_names, out);
        }
        Expr::Binding { value, .. } | Expr::LetExpr { value, .. } => {
            collect_entity_refs(value, entity_names, out);
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_entity_refs(item, entity_names, out);
            }
        }
        Expr::SetLiteral { elements, .. } => {
            for e in elements {
                collect_entity_refs(e, entity_names, out);
            }
        }
        Expr::For { collection, filter, body, .. } => {
            collect_entity_refs(collection, entity_names, out);
            if let Some(f) = filter {
                collect_entity_refs(f, entity_names, out);
            }
            collect_entity_refs(body, entity_names, out);
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_entity_refs(&b.condition, entity_names, out);
                collect_entity_refs(&b.body, entity_names, out);
            }
            if let Some(eb) = else_body {
                collect_entity_refs(eb, entity_names, out);
            }
        }
        Expr::Lambda { body, .. } | Expr::ProjectionMap { source: body, .. } => {
            collect_entity_refs(body, entity_names, out);
        }
        Expr::GenericType { name, args, .. } => {
            collect_entity_refs(name, entity_names, out);
            for a in args {
                collect_entity_refs(a, entity_names, out);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for f in fields {
                collect_entity_refs(&f.value, entity_names, out);
            }
        }
        _ => {}
    }
}

/// Collect entities that are written (field assignments, state transitions) in ensures.
fn collect_written_entities(
    expr: &Expr,
    entity_names: &std::collections::BTreeSet<String>,
    out: &mut std::collections::BTreeSet<String>,
) {
    match expr {
        // `entity.field = value` or `entity.field transitions_to state`
        Expr::Comparison { left, op: ComparisonOp::Eq, .. } => {
            if let Some(name) = resolve_entity_from_member(left, entity_names) {
                out.insert(name);
            }
        }
        Expr::TransitionsTo { subject, .. } | Expr::Becomes { subject, .. } => {
            if let Some(name) = resolve_entity_from_member(subject, entity_names) {
                out.insert(name);
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_written_entities(item, entity_names, out);
            }
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_written_entities(&b.body, entity_names, out);
            }
            if let Some(eb) = else_body {
                collect_written_entities(eb, entity_names, out);
            }
        }
        Expr::For { body, .. } => collect_written_entities(body, entity_names, out),
        Expr::Binding { value, .. } | Expr::LetExpr { value, .. } => {
            collect_written_entities(value, entity_names, out);
        }
        _ => {}
    }
}

/// Collect entities created via `.created()` calls.
/// Reports any uppercase identifier before `.created()`, whether or not it is
/// a declared entity — undeclared entities being created are still dependencies.
fn collect_created_entities(
    expr: &Expr,
    out: &mut std::collections::BTreeSet<String>,
) {
    match expr {
        Expr::Call { function, args, .. } => {
            if let Expr::MemberAccess { object, field, .. } = function.as_ref() {
                if field.name == "created" {
                    if let Expr::Ident(id) = object.as_ref() {
                        if id.name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                            out.insert(id.name.clone());
                        }
                    }
                }
            }
            for arg in args {
                match arg {
                    CallArg::Positional(e) => collect_created_entities(e, out),
                    CallArg::Named(na) => collect_created_entities(&na.value, out),
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_created_entities(item, out);
            }
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_created_entities(&b.body, out);
            }
            if let Some(eb) = else_body {
                collect_created_entities(eb, out);
            }
        }
        Expr::For { body, .. } => collect_created_entities(body, out),
        Expr::Binding { value, .. } | Expr::LetExpr { value, .. } => {
            collect_created_entities(value, out);
        }
        _ => {}
    }
}

/// Collect entities removed via `.remove()` or `remove` calls.
fn collect_removed_entities(
    expr: &Expr,
    out: &mut std::collections::BTreeSet<String>,
) {
    match expr {
        Expr::Call { function, .. } => {
            if let Expr::MemberAccess { object, field, .. } = function.as_ref() {
                if field.name == "remove" || field.name == "removed" {
                    if let Expr::Ident(id) = object.as_ref() {
                        if id.name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                            out.insert(id.name.clone());
                        }
                    }
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_removed_entities(item, out);
            }
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_removed_entities(&b.body, out);
            }
            if let Some(eb) = else_body {
                collect_removed_entities(eb, out);
            }
        }
        Expr::For { body, .. } => collect_removed_entities(body, out),
        _ => {}
    }
}

/// Collect trigger emission names from an ensures clause.
/// These are `Call { function: Ident(UppercaseName) }` that are NOT known entities.
fn collect_ensures_trigger_emissions(
    expr: &Expr,
    entity_names: &std::collections::BTreeSet<String>,
    out: &mut std::collections::BTreeSet<String>,
) {
    match expr {
        Expr::Call { function, args, .. } => {
            if let Expr::Ident(id) = function.as_ref() {
                if id.name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                    && !entity_names.contains(&id.name)
                {
                    out.insert(id.name.clone());
                }
            }
            for arg in args {
                match arg {
                    CallArg::Positional(e) => collect_ensures_trigger_emissions(e, entity_names, out),
                    CallArg::Named(na) => collect_ensures_trigger_emissions(&na.value, entity_names, out),
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_ensures_trigger_emissions(item, entity_names, out);
            }
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_ensures_trigger_emissions(&b.body, entity_names, out);
            }
            if let Some(eb) = else_body {
                collect_ensures_trigger_emissions(eb, entity_names, out);
            }
        }
        Expr::For { body, .. } => collect_ensures_trigger_emissions(body, entity_names, out),
        Expr::Binding { value, .. } | Expr::LetExpr { value, .. } => {
            collect_ensures_trigger_emissions(value, entity_names, out);
        }
        _ => {}
    }
}

/// Collect deferred spec references from an expression.
/// These are `Call { function: Ident(name) }` where name matches a known deferred spec.
fn collect_deferred_refs(
    expr: &Expr,
    deferred_names: &std::collections::BTreeSet<String>,
    out: &mut std::collections::BTreeSet<String>,
) {
    match expr {
        Expr::Call { function, args, .. } => {
            if let Expr::Ident(id) = function.as_ref() {
                if deferred_names.contains(&id.name) {
                    out.insert(id.name.clone());
                }
            }
            for arg in args {
                match arg {
                    CallArg::Positional(e) => collect_deferred_refs(e, deferred_names, out),
                    CallArg::Named(na) => collect_deferred_refs(&na.value, deferred_names, out),
                }
            }
        }
        Expr::Ident(id) => {
            if deferred_names.contains(&id.name) {
                out.insert(id.name.clone());
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_deferred_refs(item, deferred_names, out);
            }
        }
        Expr::Binding { value, .. } | Expr::LetExpr { value, .. } => {
            collect_deferred_refs(value, deferred_names, out);
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_deferred_refs(&b.condition, deferred_names, out);
                collect_deferred_refs(&b.body, deferred_names, out);
            }
            if let Some(eb) = else_body {
                collect_deferred_refs(eb, deferred_names, out);
            }
        }
        Expr::For { collection, filter, body, .. } => {
            collect_deferred_refs(collection, deferred_names, out);
            if let Some(f) = filter {
                collect_deferred_refs(f, deferred_names, out);
            }
            collect_deferred_refs(body, deferred_names, out);
        }
        Expr::BinaryOp { left, right, .. }
        | Expr::Comparison { left, right, .. }
        | Expr::LogicalOp { left, right, .. } => {
            collect_deferred_refs(left, deferred_names, out);
            collect_deferred_refs(right, deferred_names, out);
        }
        Expr::Not { operand, .. } => collect_deferred_refs(operand, deferred_names, out),
        Expr::MemberAccess { object, .. } => collect_deferred_refs(object, deferred_names, out),
        _ => {}
    }
}

/// Resolve a MemberAccess expression to an entity name.
/// Handles both `Entity.field` (direct) and `var.field` (capitalise variable name).
fn resolve_entity_from_member(
    expr: &Expr,
    entity_names: &std::collections::BTreeSet<String>,
) -> Option<String> {
    if let Expr::MemberAccess { object, .. } = expr {
        if let Expr::Ident(id) = object.as_ref() {
            if entity_names.contains(&id.name) {
                return Some(id.name.clone());
            }
            let cap = capitalize_first(&id.name);
            if entity_names.contains(&cap) {
                return Some(cap);
            }
        }
    }
    None
}

fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
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

    // --- Rule dependency analysis ---

    #[test]
    fn rule_dependencies_external_trigger() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule Confirm {\n  when: Confirm(order)\n  requires: order.status = pending\n  ensures: order.status = done\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "rule-success-Confirm");
        let deps = ob.dependencies.as_ref().expect("rule should have dependencies");
        assert!(matches!(deps.trigger_source, TriggerSource::External));
    }

    #[test]
    fn rule_dependencies_entities_read_from_requires() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule Confirm {\n  when: Confirm(order)\n  requires: order.status = pending\n  ensures: order.status = done\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Confirm").dependencies.as_ref().unwrap();
        assert!(deps.entities_read.contains(&"Order".to_string()));
    }

    #[test]
    fn rule_dependencies_entities_written_from_ensures() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule Confirm {\n  when: Confirm(order)\n  requires: order.status = pending\n  ensures: order.status = done\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Confirm").dependencies.as_ref().unwrap();
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    #[test]
    fn rule_dependencies_state_transition_trigger() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | shipped }\n\
            rule Ship {\n  when: order: Order.status transitions_to shipped\n  ensures: order.status = shipped\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Ship").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::StateTransition));
        assert!(deps.entities_read.contains(&"Order".to_string()));
    }

    #[test]
    fn rule_dependencies_temporal_trigger() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | cancelled, created_at: Timestamp }\n\
            rule Timeout {\n  when: o: Order.created_at + 48.hours <= now\n  ensures: o.status = cancelled\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Timeout").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::Temporal));
    }

    #[test]
    fn rule_dependencies_creation_trigger() {
        let source = "-- allium: 3\n\
            entity Order { total: Integer }\n\
            entity Email { to: String }\n\
            rule Notify {\n  when: order: Order.created()\n  ensures: Email.created(to: order.to)\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Notify").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::Creation));
    }

    #[test]
    fn rule_dependencies_chained_trigger() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule First {\n  when: Start(order)\n  ensures:\n    order.status = done\n    Notify(order)\n}\n\
            rule Second {\n  when: Notify(order)\n  ensures: order.status = done\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Second").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::Chained));
    }

    #[test]
    fn rule_dependencies_entity_creation_in_ensures() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | shipped }\n\
            entity Email { to: String }\n\
            rule Ship {\n  when: Ship(order)\n  ensures:\n    order.status = shipped\n    Email.created(to: order.to)\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Ship").dependencies.as_ref().unwrap();
        assert!(deps.entities_created.contains(&"Email".to_string()));
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    #[test]
    fn rule_dependencies_trigger_emissions() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule Process {\n  when: Process(order)\n  ensures:\n    order.status = done\n    Notify(order)\n    Alert(order)\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Process").dependencies.as_ref().unwrap();
        assert!(deps.trigger_emissions.contains(&"Notify".to_string()));
        assert!(deps.trigger_emissions.contains(&"Alert".to_string()));
    }

    #[test]
    fn rule_dependencies_deferred_specs() {
        let source = "-- allium: 3\n\
            entity Order { total: Integer }\n\
            deferred Evaluate\n\
            rule Process {\n  when: Process(order)\n  ensures: order.total = Evaluate(order)\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Process").dependencies.as_ref().unwrap();
        assert!(deps.deferred_specs.contains(&"Evaluate".to_string()));
    }

    #[test]
    fn rule_dependencies_present_on_failure_obligation() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule Confirm {\n  requires: order.status = pending\n  ensures: order.status = done\n}";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "rule-failure-Confirm");
        assert!(ob.dependencies.is_some());
    }

    #[test]
    fn rule_dependencies_absent_on_non_rule_obligations() {
        let source = "-- allium: 3\nentity Order { total: Integer }";
        let plan = parse_plan(source);
        let ob = find_obligation(&plan, "entity-fields-Order");
        assert!(ob.dependencies.is_none());
    }

    #[test]
    fn rule_dependencies_empty_arrays_when_no_deps() {
        let source = "-- allium: 3\n\
            rule Simple {\n  ensures: true\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Simple").dependencies.as_ref().unwrap();
        assert!(deps.entities_read.is_empty());
        assert!(deps.entities_written.is_empty());
        assert!(deps.entities_created.is_empty());
        assert!(deps.entities_removed.is_empty());
        assert!(deps.deferred_specs.is_empty());
        assert!(deps.trigger_emissions.is_empty());
    }

    #[test]
    fn rule_dependencies_json_serialisation() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule Confirm {\n  when: Confirm(order)\n  requires: order.status = pending\n  ensures: order.status = done\n}";
        let plan = parse_plan(source);
        let json = to_json(&plan);
        let ob = json["obligations"].as_array().unwrap()
            .iter().find(|o| o["id"].as_str().unwrap() == "rule-success-Confirm")
            .unwrap();
        let deps = &ob["dependencies"];
        assert!(deps.is_object());
        assert!(deps["entities_read"].is_array());
        assert!(deps["entities_written"].is_array());
        assert!(deps["trigger_source"].is_string());
    }

    #[test]
    fn rule_dependencies_omitted_from_json_for_non_rules() {
        let source = "-- allium: 3\nentity Order { total: Integer }";
        let plan = parse_plan(source);
        let json = to_json(&plan);
        let ob = json["obligations"].as_array().unwrap()
            .iter().find(|o| o["id"].as_str().unwrap().contains("entity-fields"))
            .unwrap();
        assert!(ob.get("dependencies").is_none());
    }

    #[test]
    fn rule_dependencies_direct_entity_ref_in_when() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | shipped }\n\
            rule Ship {\n  when: order: Order.status transitions_to shipped\n  ensures: order.status = shipped\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Ship").dependencies.as_ref().unwrap();
        assert!(deps.entities_read.contains(&"Order".to_string()));
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    #[test]
    fn rule_dependencies_multiple_entities() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            entity Customer { email: String }\n\
            rule Process {\n  when: Process(order)\n  requires: order.status = pending\n  ensures:\n    order.status = done\n    customer.email = order.email\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Process").dependencies.as_ref().unwrap();
        assert!(deps.entities_read.contains(&"Order".to_string()));
        assert!(deps.entities_written.contains(&"Order".to_string()));
        assert!(deps.entities_written.contains(&"Customer".to_string()));
    }

    // --- Trigger source: becomes ---

    #[test]
    fn rule_dependencies_becomes_trigger() {
        let source = "-- allium: 3\n\
            entity Order { status: shipped | delivered }\n\
            rule Archive {\n  when: order: Order.status becomes delivered\n  ensures: AuditLog.created(action: delivered, order: order)\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Archive").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::StateTransition));
        assert!(deps.entities_read.contains(&"Order".to_string()));
    }

    // --- For-block and if-block dependency extraction ---

    #[test]
    fn rule_dependencies_for_block_writes() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | confirmed }\n\
            rule BulkConfirm {\n  when: BulkConfirm(batch)\n  for order in batch.orders where order.status = pending:\n    ensures: order.status = confirmed\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-BulkConfirm").dependencies.as_ref().unwrap();
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    #[test]
    fn rule_dependencies_if_block_writes() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | cancelled, cancelled_by: String? }\n\
            entity Customer { name: String }\n\
            rule Cancel {\n  when: Cancel(order, reason)\n  ensures:\n    order.status = cancelled\n    if reason = customer_request:\n      order.cancelled_by = order.customer.name\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Cancel").dependencies.as_ref().unwrap();
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    // --- Multi-line ensures with multiple field writes ---

    #[test]
    fn rule_dependencies_multi_field_ensures() {
        let source = "-- allium: 3\n\
            entity Order { status: picking | shipped, tracking_number: String, shipped_at: Timestamp }\n\
            rule ShipOrder {\n  when: ShipOrder(order, tracking)\n  requires: order.status = picking\n  ensures:\n    order.status = shipped\n    order.tracking_number = tracking\n    order.shipped_at = now\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-ShipOrder").dependencies.as_ref().unwrap();
        assert!(deps.entities_read.contains(&"Order".to_string()));
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    // --- Entity creation with named args ---

    #[test]
    fn rule_dependencies_entity_creation_named_args() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | shipped }\n\
            entity Email { to: String }\n\
            rule Notify {\n  when: order: Order.status transitions_to shipped\n  ensures: Email.created(to: order.customer.email, template: order_shipped)\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Notify").dependencies.as_ref().unwrap();
        assert!(deps.entities_created.contains(&"Email".to_string()));
        assert!(deps.entities_read.contains(&"Order".to_string()));
        // .created() is entity creation, not a write
        assert!(!deps.entities_written.contains(&"Email".to_string()));
    }

    // --- Entity in requires via `in` set membership ---

    #[test]
    fn rule_dependencies_in_set_requires() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | confirmed | cancelled }\n\
            rule Cancel {\n  when: Cancel(order)\n  requires: order.status in {pending, confirmed}\n  ensures: order.status = cancelled\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Cancel").dependencies.as_ref().unwrap();
        assert!(deps.entities_read.contains(&"Order".to_string()));
    }

    // --- External entity included in entity_names ---

    #[test]
    fn rule_dependencies_external_entity_resolved() {
        let source = "-- allium: 3\n\
            external entity Customer { email: String }\n\
            rule Notify {\n  when: Notify(customer)\n  ensures: customer.email = null\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Notify").dependencies.as_ref().unwrap();
        assert!(deps.entities_written.contains(&"Customer".to_string()));
    }

    // --- Deferred spec with qualified path ---

    #[test]
    fn rule_dependencies_deferred_qualified_path() {
        let source = "-- allium: 3\n\
            entity Order { total: Integer }\n\
            deferred Order.fraud_check\n\
            rule Check {\n  when: Check(order)\n  ensures: order.total = fraud_check(order)\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Check").dependencies.as_ref().unwrap();
        assert!(deps.deferred_specs.contains(&"fraud_check".to_string()));
    }

    // --- Same entity appears in both read and written ---

    #[test]
    fn rule_dependencies_entity_both_read_and_written() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule Confirm {\n  when: Confirm(order)\n  requires: order.status = pending\n  ensures: order.status = done\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Confirm").dependencies.as_ref().unwrap();
        assert!(deps.entities_read.contains(&"Order".to_string()));
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    // --- Trigger emission not confused with entity name ---

    #[test]
    fn rule_dependencies_entity_name_not_in_trigger_emissions() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            entity Email { to: String }\n\
            rule Process {\n  when: Process(order)\n  ensures:\n    order.status = done\n    Email.created(to: order.email)\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Process").dependencies.as_ref().unwrap();
        // Email.created() is entity creation, not a trigger emission
        assert!(!deps.trigger_emissions.contains(&"Email".to_string()));
        assert!(deps.entities_created.contains(&"Email".to_string()));
    }

    // --- Rule with no when clause ---

    #[test]
    fn rule_dependencies_no_when_clause() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule Update {\n  requires: order.status = pending\n  ensures: order.status = done\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Update").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::External));
        assert!(deps.entities_read.contains(&"Order".to_string()));
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    // --- Dependencies shared across obligation types from same rule ---

    #[test]
    fn rule_dependencies_shared_across_obligation_types() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | cancelled, created_at: Timestamp }\n\
            rule Timeout {\n  when: o: Order.created_at + 48.hours <= now\n  requires: o.status = pending\n  ensures: o.status = cancelled\n}";
        let plan = parse_plan(source);
        let success_deps = find_obligation(&plan, "rule-success-Timeout").dependencies.as_ref().unwrap();
        let failure_deps = find_obligation(&plan, "rule-failure-Timeout").dependencies.as_ref().unwrap();
        let temporal_deps = find_obligation(&plan, "temporal-Timeout").dependencies.as_ref().unwrap();
        // All three should carry the same dependency info
        assert_eq!(success_deps.entities_read, failure_deps.entities_read);
        assert_eq!(success_deps.entities_read, temporal_deps.entities_read);
        assert!(matches!(failure_deps.trigger_source, TriggerSource::Temporal));
        assert!(matches!(temporal_deps.trigger_source, TriggerSource::Temporal));
    }

    // --- trigger_source serialises to snake_case ---

    #[test]
    fn rule_dependencies_trigger_source_snake_case_json() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | shipped }\n\
            rule Ship {\n  when: order: Order.status transitions_to shipped\n  ensures: order.status = shipped\n}";
        let plan = parse_plan(source);
        let json = to_json(&plan);
        let ob = json["obligations"].as_array().unwrap()
            .iter().find(|o| o["id"].as_str().unwrap() == "rule-success-Ship")
            .unwrap();
        assert_eq!(ob["dependencies"]["trigger_source"].as_str().unwrap(), "state_transition");
    }

    #[test]
    fn rule_dependencies_trigger_source_chained_json() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule A {\n  when: Start(x)\n  ensures: Next(x)\n}\n\
            rule B {\n  when: Next(x)\n  ensures: x.status = done\n}";
        let plan = parse_plan(source);
        let json = to_json(&plan);
        let ob = json["obligations"].as_array().unwrap()
            .iter().find(|o| o["id"].as_str().unwrap() == "rule-success-B")
            .unwrap();
        assert_eq!(ob["dependencies"]["trigger_source"].as_str().unwrap(), "chained");
    }

    // --- BTreeSet ordering: entities are alphabetically sorted ---

    #[test]
    fn rule_dependencies_entities_sorted() {
        let source = "-- allium: 3\n\
            entity Zebra { x: Integer }\n\
            entity Alpha { x: Integer }\n\
            entity Middle { x: Integer }\n\
            rule Foo {\n  when: Foo(zebra, alpha, middle)\n  requires: zebra.x = 1 and alpha.x = 2 and middle.x = 3\n  ensures: zebra.x = 4\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Foo").dependencies.as_ref().unwrap();
        assert_eq!(deps.entities_read, vec!["Alpha", "Middle", "Zebra"]);
    }

    // --- Entity creation not reported as entity written ---

    #[test]
    fn rule_dependencies_created_entity_not_in_written() {
        let source = "-- allium: 3\n\
            entity AuditLog { action: String }\n\
            rule Log {\n  when: Log(x)\n  ensures: AuditLog.created(action: x)\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Log").dependencies.as_ref().unwrap();
        assert!(deps.entities_created.contains(&"AuditLog".to_string()));
        assert!(deps.entities_written.is_empty());
    }

    // --- Multiple entity creations ---

    #[test]
    fn rule_dependencies_multiple_entity_creations() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | shipped }\n\
            entity Email { to: String }\n\
            entity AuditLog { action: String }\n\
            rule Ship {\n  when: Ship(order)\n  ensures:\n    order.status = shipped\n    Email.created(to: order.email)\n    AuditLog.created(action: shipped)\n}";
        let plan = parse_plan(source);
        let deps = find_obligation(&plan, "rule-success-Ship").dependencies.as_ref().unwrap();
        assert_eq!(deps.entities_created, vec!["AuditLog", "Email"]);
    }

    // --- Chained: emitter rule is external, not chained to itself ---

    #[test]
    fn rule_dependencies_emitter_is_external_not_self_chained() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            rule First {\n  when: Start(order)\n  ensures:\n    order.status = done\n    Notify(order)\n}\n\
            rule Second {\n  when: Notify(order)\n  ensures: order.status = done\n}";
        let plan = parse_plan(source);
        let first_deps = find_obligation(&plan, "rule-success-First").dependencies.as_ref().unwrap();
        let second_deps = find_obligation(&plan, "rule-success-Second").dependencies.as_ref().unwrap();
        // First emits Notify — its trigger_source is external (Start is not emitted by any rule)
        assert!(matches!(first_deps.trigger_source, TriggerSource::External));
        // Second listens on Notify — chained
        assert!(matches!(second_deps.trigger_source, TriggerSource::Chained));
    }

    // --- JSON: all dependency fields present with correct types ---

    #[test]
    fn rule_dependencies_json_shape_complete() {
        let source = "-- allium: 3\n\
            entity Order { status: pending | done }\n\
            entity Email { to: String }\n\
            deferred Evaluate\n\
            rule Process {\n  when: Process(order)\n  requires: order.status = pending\n  ensures:\n    order.status = done\n    Email.created(to: Evaluate(order))\n    Notify(order)\n}";
        let plan = parse_plan(source);
        let json = to_json(&plan);
        let ob = json["obligations"].as_array().unwrap()
            .iter().find(|o| o["id"].as_str().unwrap() == "rule-success-Process")
            .unwrap();
        let deps = &ob["dependencies"];
        // All seven keys present
        assert!(deps["entities_read"].is_array());
        assert!(deps["entities_written"].is_array());
        assert!(deps["entities_created"].is_array());
        assert!(deps["entities_removed"].is_array());
        assert!(deps["deferred_specs"].is_array());
        assert!(deps["trigger_emissions"].is_array());
        assert!(deps["trigger_source"].is_string());
        // Check specific values
        assert!(deps["entities_read"].as_array().unwrap().iter().any(|v| v == "Order"));
        assert!(deps["entities_written"].as_array().unwrap().iter().any(|v| v == "Order"));
        assert!(deps["entities_created"].as_array().unwrap().iter().any(|v| v == "Email"));
        assert!(deps["deferred_specs"].as_array().unwrap().iter().any(|v| v == "Evaluate"));
        assert!(deps["trigger_emissions"].as_array().unwrap().iter().any(|v| v == "Notify"));
        assert_eq!(deps["trigger_source"], "external");
    }

    // --- Integration: v3-lifecycle fixture ---

    #[test]
    fn fixture_confirm_order_dependencies() {
        let source = std::fs::read_to_string("../allium-parser/tests/fixtures/v3-lifecycle.allium").unwrap();
        let plan = parse_plan(&source);
        let deps = find_obligation(&plan, "rule-success-ConfirmOrder").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::External));
        assert!(deps.entities_read.contains(&"Order".to_string()));
        assert!(deps.entities_written.contains(&"Order".to_string()));
        assert!(deps.entities_created.is_empty());
        assert!(deps.trigger_emissions.is_empty());
    }

    #[test]
    fn fixture_cancel_by_timeout_dependencies() {
        let source = std::fs::read_to_string("../allium-parser/tests/fixtures/v3-lifecycle.allium").unwrap();
        let plan = parse_plan(&source);
        let deps = find_obligation(&plan, "rule-success-CancelByTimeout").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::Temporal));
        assert!(deps.entities_read.contains(&"Order".to_string()));
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    #[test]
    fn fixture_notify_on_shipment_dependencies() {
        let source = std::fs::read_to_string("../allium-parser/tests/fixtures/v3-lifecycle.allium").unwrap();
        let plan = parse_plan(&source);
        let deps = find_obligation(&plan, "rule-success-NotifyOnShipment").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::StateTransition));
        assert!(deps.entities_read.contains(&"Order".to_string()));
        assert!(deps.entities_created.contains(&"Email".to_string()));
        assert!(deps.entities_written.is_empty());
    }

    #[test]
    fn fixture_archive_delivered_dependencies() {
        let source = std::fs::read_to_string("../allium-parser/tests/fixtures/v3-lifecycle.allium").unwrap();
        let plan = parse_plan(&source);
        let deps = find_obligation(&plan, "rule-success-ArchiveDelivered").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::StateTransition));
        assert!(deps.entities_created.contains(&"AuditLog".to_string()));
    }

    #[test]
    fn fixture_bulk_confirm_dependencies() {
        let source = std::fs::read_to_string("../allium-parser/tests/fixtures/v3-lifecycle.allium").unwrap();
        let plan = parse_plan(&source);
        let deps = find_obligation(&plan, "rule-success-BulkConfirm").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::External));
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    #[test]
    fn fixture_process_cancellation_dependencies() {
        let source = std::fs::read_to_string("../allium-parser/tests/fixtures/v3-lifecycle.allium").unwrap();
        let plan = parse_plan(&source);
        let deps = find_obligation(&plan, "rule-success-ProcessCancellation").dependencies.as_ref().unwrap();
        assert!(matches!(deps.trigger_source, TriggerSource::External));
        assert!(deps.entities_read.contains(&"Order".to_string()));
        assert!(deps.entities_written.contains(&"Order".to_string()));
    }

    #[test]
    fn fixture_ship_order_dependencies() {
        let source = std::fs::read_to_string("../allium-parser/tests/fixtures/v3-lifecycle.allium").unwrap();
        let plan = parse_plan(&source);
        let deps = find_obligation(&plan, "rule-success-ShipOrder").dependencies.as_ref().unwrap();
        assert!(deps.entities_read.contains(&"Order".to_string()));
        assert!(deps.entities_written.contains(&"Order".to_string()));
        assert!(deps.entities_created.is_empty());
    }
}
