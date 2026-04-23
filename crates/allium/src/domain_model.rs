//! Walks the Allium AST and emits a domain model.
//!
//! For each entity: fields with types and constraints, relationships,
//! applicable invariants, config-derived bounds, transition graphs and
//! per-field lifecycle qualification via `when` sets.
//!
//! Invoked by `allium model`.

use allium_parser::ast::*;
use allium_parser::{Module, Span};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct DomainModel {
    pub version: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<EntityGen>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub value_types: Vec<ValueTypeGen>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enums: Vec<EnumGen>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub config: Vec<ConfigParam>,
}

#[derive(Debug, Serialize)]
pub struct EntityGen {
    pub name: String,
    pub kind: EntityKind,
    pub fields: Vec<FieldGen>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<RelationshipGen>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub projections: Vec<ProjectionGen>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub derived_values: Vec<DerivedValueGen>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub transition_graphs: Vec<TransitionGraphGen>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invariants: Vec<InvariantGen>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Internal,
    External,
}

#[derive(Debug, Clone, Serialize)]
pub struct WhenSet {
    pub status_field: String,
    pub qualifying_states: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct FieldGen {
    pub name: String,
    pub type_expr: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    /// Inline enum values, if this field uses an inline enum.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_set: Option<WhenSet>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<FieldConstraint>,
}

#[derive(Debug, Serialize)]
pub struct FieldConstraint {
    pub invariant: String,
    pub bound: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_set: Option<WhenSet>,
}

#[derive(Debug, Serialize)]
pub struct TransitionGraphGen {
    pub field: String,
    pub edges: Vec<EdgeGen>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
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
pub struct InvariantGen {
    pub name: String,
    pub scope: InvariantScope,
    pub expression: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvariantScope {
    Entity,
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

pub fn extract_domain_model(module: &Module, source: &str) -> DomainModel {
    let mut spec = DomainModel {
        version: module.version,
        entities: Vec::new(),
        value_types: Vec::new(),
        enums: Vec::new(),
        config: Vec::new(),
    };

    for decl in &module.declarations {
        if let Decl::Block(block) = decl {
            match block.kind {
                BlockKind::Entity | BlockKind::ExternalEntity => {
                    spec.entities.push(build_entity_gen(block, source));
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

fn build_entity_gen(block: &BlockDecl, source: &str) -> EntityGen {
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
    let mut transition_graphs = Vec::new();
    let mut invariants = Vec::new();

    // Collect when sets keyed by field name, for derived value inference
    let mut field_when_sets: Vec<(String, WhenSet)> = Vec::new();

    for item in &block.items {
        match &item.kind {
            BlockItemKind::Assignment { name: field_name, value } => {
                let type_expr = span_text(source, value.span());
                let optional = matches!(value, Expr::TypeOptional { .. });
                let enum_values = extract_inline_enum_values(value);

                if matches!(value, Expr::With { .. }) {
                    let target = extract_source_ident(value);
                    relationships.push(RelationshipGen {
                        name: field_name.name.clone(),
                        target,
                    });
                } else if matches!(value, Expr::Where { .. }) {
                    projections.push(ProjectionGen {
                        name: field_name.name.clone(),
                        source: extract_source_ident(value),
                    });
                } else if is_derived(value) {
                    let when_set = infer_derived_when_set(value, &field_when_sets);
                    derived_values.push(DerivedValueGen {
                        name: field_name.name.clone(),
                        when_set,
                    });
                } else {
                    fields.push(FieldGen {
                        name: field_name.name.clone(),
                        type_expr,
                        optional,
                        enum_values,
                        when_set: None,
                        constraints: Vec::new(),
                    });
                }
            }
            BlockItemKind::FieldWithWhen { name: field_name, value, when_clause } => {
                let type_expr = span_text(source, value.span());
                let optional = matches!(value, Expr::TypeOptional { .. });
                let enum_values = extract_inline_enum_values(value);
                let ws = WhenSet {
                    status_field: when_clause.status_field.name.clone(),
                    qualifying_states: when_clause.qualifying_states.iter().map(|s| s.name.clone()).collect(),
                };
                field_when_sets.push((field_name.name.clone(), ws.clone()));
                fields.push(FieldGen {
                    name: field_name.name.clone(),
                    type_expr,
                    optional,
                    enum_values,
                    when_set: Some(ws),
                    constraints: Vec::new(),
                });
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

                transition_graphs.push(TransitionGraphGen {
                    field: graph.field.name.clone(),
                    edges,
                    terminal: graph.terminal.iter().map(|t| t.name.clone()).collect(),
                    states: states.into_iter().collect(),
                });
            }
            BlockItemKind::InvariantBlock { name: inv_name, body } => {
                let expr_text = span_text(source, body.span());
                invariants.push(InvariantGen {
                    name: inv_name.name.clone(),
                    scope: InvariantScope::Entity,
                    expression: expr_text,
                });
            }
            _ => {}
        }
    }

    // Attach simple single-field constraints from invariants to their fields.
    for item in &block.items {
        if let BlockItemKind::InvariantBlock { name: inv_name, body } = &item.kind {
            if let Some((field_name, bound)) = extract_single_field_bound(body) {
                if let Some(field) = fields.iter_mut().find(|f| f.name == field_name) {
                    field.constraints.push(FieldConstraint {
                        invariant: inv_name.name.clone(),
                        bound,
                    });
                }
            }
        }
    }

    EntityGen {
        name,
        kind,
        fields,
        relationships,
        projections,
        derived_values,
        transition_graphs,
        invariants,
    }
}

/// Infer a when set for a derived value by intersecting the when sets of referenced fields.
fn infer_derived_when_set(expr: &Expr, field_when_sets: &[(String, WhenSet)]) -> Option<WhenSet> {
    let refs = collect_field_refs(expr);
    if refs.is_empty() {
        return None;
    }

    let mut result: Option<(String, std::collections::BTreeSet<String>)> = None;

    for field_ref in &refs {
        if let Some((_, ws)) = field_when_sets.iter().find(|(name, _)| name == field_ref) {
            match &mut result {
                None => {
                    result = Some((
                        ws.status_field.clone(),
                        ws.qualifying_states.iter().cloned().collect(),
                    ));
                }
                Some((status_field, states)) => {
                    if *status_field == ws.status_field {
                        let other: std::collections::BTreeSet<String> =
                            ws.qualifying_states.iter().cloned().collect();
                        *states = states.intersection(&other).cloned().collect();
                    }
                    // Different status fields: cannot intersect, skip
                }
            }
        }
    }

    result.map(|(status_field, states)| WhenSet {
        status_field,
        qualifying_states: states.into_iter().collect(),
    })
}

/// Collect simple field references (identifiers) from an expression.
fn collect_field_refs(expr: &Expr) -> Vec<String> {
    let mut refs = Vec::new();
    collect_field_refs_inner(expr, &mut refs);
    refs
}

fn collect_field_refs_inner(expr: &Expr, refs: &mut Vec<String>) {
    match expr {
        Expr::Ident(id) => refs.push(id.name.clone()),
        Expr::BinaryOp { left, right, .. }
        | Expr::Comparison { left, right, .. }
        | Expr::LogicalOp { left, right, .. } => {
            collect_field_refs_inner(left, refs);
            collect_field_refs_inner(right, refs);
        }
        Expr::Not { operand, .. } => collect_field_refs_inner(operand, refs),
        Expr::MemberAccess { field, .. } => refs.push(field.name.clone()),
        _ => {}
    }
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
                    when_set: None,
                    constraints: Vec::new(),
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

fn build_config_gen(spec: &mut DomainModel, block: &BlockDecl, source: &str) {
    for item in &block.items {
        if let BlockItemKind::Assignment { name, value } = &item.kind {
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
            if id.name.chars().next().map(|c| c.is_lowercase()).unwrap_or(false) {
                vec![id.name.clone()]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// Extract the source identifier from a `with` or `where` expression.
fn extract_source_ident(expr: &Expr) -> String {
    let inner = match expr {
        Expr::With { source, .. } | Expr::Where { source, .. } => Some(source.as_ref()),
        _ => None,
    };
    match inner {
        Some(Expr::Ident(id)) => id.name.clone(),
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
    match value {
        Expr::Comparison {
            op: ComparisonOp::Eq,
            left,
            ..
        } => span_text(source, left.span()),
        _ => span_text(source, value.span()),
    }
}

/// Extract a single-field bound from a simple invariant expression.
///
/// Matches patterns like `this.field >= 1`, `this.field != null`,
/// and the reversed form `1 <= this.field`. Returns (field_name, bound_string).
fn extract_single_field_bound(body: &Expr) -> Option<(String, String)> {
    if let Expr::Comparison { left, op, right, .. } = body {
        // `this.field <op> value`
        if let Some(field_name) = extract_this_field(left) {
            let op_str = comparison_op_str(*op);
            let val = expr_literal_str(right);
            if let Some(v) = val {
                return Some((field_name, format!("{} {}", op_str, v)));
            }
        }
        // `value <op> this.field` (reversed)
        if let Some(field_name) = extract_this_field(right) {
            let op_str = comparison_op_str(flip_comparison(*op));
            let val = expr_literal_str(left);
            if let Some(v) = val {
                return Some((field_name, format!("{} {}", op_str, v)));
            }
        }
    }
    None
}

fn extract_this_field(expr: &Expr) -> Option<String> {
    if let Expr::MemberAccess { object, field, .. } = expr {
        if matches!(object.as_ref(), Expr::This { .. }) {
            return Some(field.name.clone());
        }
    }
    None
}

fn comparison_op_str(op: ComparisonOp) -> &'static str {
    match op {
        ComparisonOp::Eq => "=",
        ComparisonOp::NotEq => "!=",
        ComparisonOp::Lt => "<",
        ComparisonOp::LtEq => "<=",
        ComparisonOp::Gt => ">",
        ComparisonOp::GtEq => ">=",
    }
}

fn flip_comparison(op: ComparisonOp) -> ComparisonOp {
    match op {
        ComparisonOp::Lt => ComparisonOp::Gt,
        ComparisonOp::LtEq => ComparisonOp::GtEq,
        ComparisonOp::Gt => ComparisonOp::Lt,
        ComparisonOp::GtEq => ComparisonOp::LtEq,
        other => other,
    }
}

fn expr_literal_str(expr: &Expr) -> Option<String> {
    match expr {
        Expr::NumberLiteral { value, .. } => Some(value.clone()),
        Expr::Null { .. } => Some("null".to_string()),
        Expr::BoolLiteral { value, .. } => Some(value.to_string()),
        Expr::StringLiteral(s) => {
            let text: String = s.parts.iter().map(|p| match p {
                StringPart::Text(t) => t.clone(),
                StringPart::Interpolation(id) => format!("${{{}}}", id.name),
            }).collect();
            Some(format!("\"{}\"", text))
        }
        Expr::Ident(id) => Some(id.name.clone()),
        _ => None,
    }
}

fn extract_default_from_assignment(value: &Expr, source: &str) -> Option<String> {
    match value {
        Expr::Comparison {
            op: ComparisonOp::Eq,
            right,
            ..
        } => Some(span_text(source, right.span())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_model(source: &str) -> DomainModel {
        let result = allium_parser::parse(source);
        extract_domain_model(&result.module, source)
    }

    fn to_json(spec: &DomainModel) -> serde_json::Value {
        serde_json::to_value(spec).unwrap()
    }

    fn find_entity<'a>(spec: &'a DomainModel, name: &str) -> &'a EntityGen {
        spec.entities.iter().find(|e| e.name == name)
            .unwrap_or_else(|| panic!("no entity '{}'", name))
    }

    fn find_field<'a>(entity: &'a EntityGen, name: &str) -> &'a FieldGen {
        entity.fields.iter().find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no field '{}'", name))
    }

    // --- Version passthrough ---

    #[test]
    fn version_passed_through() {
        let spec = parse_model("-- allium: 3\nentity Foo { x: Integer }");
        assert_eq!(spec.version, Some(3));
    }

    #[test]
    fn version_none_when_absent() {
        let spec = parse_model("entity Foo { x: Integer }");
        assert_eq!(spec.version, None);
    }

    // --- Entity basics ---

    #[test]
    fn entity_name_and_kind_internal() {
        let spec = parse_model("-- allium: 3\nentity Order { total: Integer }");
        let e = find_entity(&spec, "Order");
        let json = serde_json::to_value(e).unwrap();
        assert_eq!(json["kind"], "internal");
    }

    #[test]
    fn entity_kind_external() {
        let spec = parse_model("-- allium: 3\nexternal entity Customer { email: String }");
        let e = find_entity(&spec, "Customer");
        let json = serde_json::to_value(e).unwrap();
        assert_eq!(json["kind"], "external");
    }

    #[test]
    fn empty_entity_has_no_fields() {
        let spec = parse_model("-- allium: 3\nentity Empty {}");
        let e = find_entity(&spec, "Empty");
        assert!(e.fields.is_empty());
        assert!(e.invariants.is_empty());
        assert!(e.relationships.is_empty());
    }

    #[test]
    fn multiple_entities_collected() {
        let source = "-- allium: 3\nentity A { x: Integer }\nentity B { y: String }";
        let spec = parse_model(source);
        assert_eq!(spec.entities.len(), 2);
        assert_eq!(spec.entities[0].name, "A");
        assert_eq!(spec.entities[1].name, "B");
    }

    // --- Fields ---

    #[test]
    fn field_name_and_type() {
        let spec = parse_model("-- allium: 3\nentity Foo { name: String }");
        let f = find_field(find_entity(&spec, "Foo"), "name");
        assert_eq!(f.type_expr, "String");
        assert!(!f.optional);
    }

    #[test]
    fn optional_field() {
        let spec = parse_model("-- allium: 3\nentity Foo { bio: String? }");
        let f = find_field(find_entity(&spec, "Foo"), "bio");
        assert!(f.optional);
    }

    #[test]
    fn multiple_fields_preserved_in_order() {
        let spec = parse_model("-- allium: 3\nentity Foo {\n  a: Integer\n  b: String\n  c: Decimal\n}");
        let names: Vec<&str> = spec.entities[0].fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    // --- Inline enum values ---

    #[test]
    fn inline_enum_values_extracted() {
        let spec = parse_model("-- allium: 3\nentity Order { status: pending | done | cancelled }");
        let f = find_field(find_entity(&spec, "Order"), "status");
        assert_eq!(f.enum_values, vec!["pending", "done", "cancelled"]);
    }

    #[test]
    fn non_enum_field_has_no_enum_values() {
        let spec = parse_model("-- allium: 3\nentity Foo { name: String }");
        let f = find_field(find_entity(&spec, "Foo"), "name");
        assert!(f.enum_values.is_empty());
    }

    #[test]
    fn enum_values_omitted_from_json_when_empty() {
        let spec = parse_model("-- allium: 3\nentity Foo { name: String }");
        let json = to_json(&spec);
        assert!(json["entities"][0]["fields"][0].get("enum_values").is_none());
    }

    // --- Relationships (with) ---

    #[test]
    fn relationship_detected() {
        let source = "-- allium: 3\nentity Order { items: OrderItem with order }";
        let spec = parse_model(source);
        let e = find_entity(&spec, "Order");
        assert_eq!(e.relationships.len(), 1);
        assert_eq!(e.relationships[0].name, "items");
        assert_eq!(e.relationships[0].target, "OrderItem");
        // with-fields are relationships, not in fields
        assert!(e.fields.is_empty());
    }

    // --- Projections (where) ---

    #[test]
    fn projection_detected() {
        let source = "-- allium: 3\nentity Order { active_items: items where active = true }";
        let spec = parse_model(source);
        let e = find_entity(&spec, "Order");
        assert_eq!(e.projections.len(), 1);
        assert_eq!(e.projections[0].name, "active_items");
        assert_eq!(e.projections[0].source, "items");
    }

    // --- Derived values ---

    #[test]
    fn derived_value_detected() {
        let source = "-- allium: 3\nentity Order {\n  a: Integer\n  b: Integer\n  total: a + b\n}";
        let spec = parse_model(source);
        let e = find_entity(&spec, "Order");
        assert_eq!(e.derived_values.len(), 1);
        assert_eq!(e.derived_values[0].name, "total");
        // derived values are not in fields
        assert!(!e.fields.iter().any(|f| f.name == "total"));
    }

    // --- Transition graphs ---

    #[test]
    fn transition_graph_edges_and_terminal() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | done\n  transitions status {\n    pending -> done\n    terminal: done\n  }\n}";
        let spec = parse_model(source);
        let graph = &find_entity(&spec, "Order").transition_graphs[0];
        assert_eq!(graph.field, "status");
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].from, "pending");
        assert_eq!(graph.edges[0].to, "done");
        assert_eq!(graph.terminal, vec!["done"]);
    }

    #[test]
    fn transition_graph_collects_all_states() {
        let source = "-- allium: 3\nentity Order {\n  status: a | b | c\n  transitions status {\n    a -> b\n    b -> c\n    terminal: c\n  }\n}";
        let spec = parse_model(source);
        let graph = &find_entity(&spec, "Order").transition_graphs[0];
        // BTreeSet ordering
        assert_eq!(graph.states, vec!["a", "b", "c"]);
    }

    #[test]
    fn transition_graphs_omitted_when_absent() {
        let spec = parse_model("-- allium: 3\nentity Foo { x: Integer }");
        let json = to_json(&spec);
        assert!(json["entities"][0].get("transition_graphs").is_none());
    }

    // --- When sets ---

    #[test]
    fn when_set_on_field() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | shipped\n  tracking: String when status = shipped\n}";
        let spec = parse_model(source);
        let f = find_field(find_entity(&spec, "Order"), "tracking");
        let ws = f.when_set.as_ref().unwrap();
        assert_eq!(ws.status_field, "status");
        assert_eq!(ws.qualifying_states, vec!["shipped"]);
    }

    #[test]
    fn when_set_multiple_states() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | shipped | delivered\n  tracking: String when status = shipped | delivered\n}";
        let spec = parse_model(source);
        let f = find_field(find_entity(&spec, "Order"), "tracking");
        let ws = f.when_set.as_ref().unwrap();
        assert_eq!(ws.qualifying_states, vec!["shipped", "delivered"]);
    }

    // --- Value types ---

    #[test]
    fn value_type_with_fields() {
        let source = "-- allium: 3\nvalue Address {\n  street: String\n  city: String\n}";
        let spec = parse_model(source);
        assert_eq!(spec.value_types.len(), 1);
        assert_eq!(spec.value_types[0].name, "Address");
        let names: Vec<&str> = spec.value_types[0].fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["street", "city"]);
    }

    // --- Enums ---

    #[test]
    fn enum_values() {
        let source = "-- allium: 3\nenum Colour {\n  red\n  green\n  blue\n}";
        let spec = parse_model(source);
        assert_eq!(spec.enums.len(), 1);
        assert_eq!(spec.enums[0].name, "Colour");
        assert_eq!(spec.enums[0].values, vec!["red", "green", "blue"]);
    }

    // --- Config ---

    #[test]
    fn config_param_extracted() {
        let source = "-- allium: 3\nconfig {\n  max_retries: Integer = 3\n}";
        let spec = parse_model(source);
        assert_eq!(spec.config.len(), 1);
        assert_eq!(spec.config[0].name, "max_retries");
    }

    // --- Invariant expression on InvariantGen ---

    #[test]
    fn entity_invariant_includes_expression() {
        let source = "-- allium: 3\nentity Order {\n  total: Integer\n  invariant PositiveTotal { this.total >= 1 }\n}";
        let spec = parse_model(source);
        let entity = &spec.entities[0];
        assert_eq!(entity.invariants.len(), 1);
        assert_eq!(entity.invariants[0].name, "PositiveTotal");
        assert_eq!(entity.invariants[0].expression, "this.total >= 1");
    }

    #[test]
    fn entity_invariant_complex_expression() {
        let source = "-- allium: 3\nentity Account {\n  balance: Decimal\n  credit: Decimal\n  invariant Solvent { this.balance + this.credit >= 0 }\n}";
        let spec = parse_model(source);
        let inv = &spec.entities[0].invariants[0];
        assert_eq!(inv.name, "Solvent");
        assert_eq!(inv.expression, "this.balance + this.credit >= 0");
    }

    #[test]
    fn entity_invariant_scope_is_entity() {
        let source = "-- allium: 3\nentity Foo {\n  x: Integer\n  invariant XPos { this.x >= 0 }\n}";
        let spec = parse_model(source);
        let json = to_json(&spec);
        let scope = json["entities"][0]["invariants"][0]["scope"].as_str().unwrap();
        assert_eq!(scope, "entity");
    }

    // --- Top-level invariants do not appear in entity output ---

    #[test]
    fn top_level_invariant_not_in_entities() {
        let source = "-- allium: 3\nentity Order {\n  status: pending | done\n}\ninvariant AllDone {\n  for o in Orders: o.status = done\n}";
        let spec = parse_model(source);
        assert!(spec.entities[0].invariants.is_empty());
    }

    // --- Field-level constraints ---

    #[test]
    fn simple_invariant_attaches_field_constraint() {
        let source = "-- allium: 3\nentity Order {\n  gap: Integer\n  invariant PositiveGap { this.gap >= 1 }\n}";
        let spec = parse_model(source);
        let field = &spec.entities[0].fields[0];
        assert_eq!(field.name, "gap");
        assert_eq!(field.constraints.len(), 1);
        assert_eq!(field.constraints[0].invariant, "PositiveGap");
        assert_eq!(field.constraints[0].bound, ">= 1");
    }

    #[test]
    fn constraint_not_attached_to_derived_field() {
        let source = "-- allium: 3\nentity Order {\n  a: Integer\n  b: Integer\n  total: a + b\n  invariant NonNeg { this.total >= 0 }\n}";
        let spec = parse_model(source);
        for field in &spec.entities[0].fields {
            assert!(field.constraints.is_empty(), "no field should have constraints");
        }
        assert_eq!(spec.entities[0].invariants[0].expression, "this.total >= 0");
    }

    #[test]
    fn reversed_comparison_flips_operator() {
        let source = "-- allium: 3\nentity Item {\n  count: Integer\n  invariant MinCount { 0 <= this.count }\n}";
        let spec = parse_model(source);
        let field = &spec.entities[0].fields[0];
        assert_eq!(field.constraints.len(), 1);
        assert_eq!(field.constraints[0].bound, ">= 0");
    }

    #[test]
    fn reversed_gt_flipped_to_lt() {
        let source = "-- allium: 3\nentity Item {\n  count: Integer\n  invariant MaxCount { 100 > this.count }\n}";
        let spec = parse_model(source);
        let field = &spec.entities[0].fields[0];
        assert_eq!(field.constraints[0].bound, "< 100");
    }

    #[test]
    fn not_equal_null_constraint() {
        let source = "-- allium: 3\nentity User {\n  email: String\n  invariant EmailPresent { this.email != null }\n}";
        let spec = parse_model(source);
        let field = &spec.entities[0].fields[0];
        assert_eq!(field.constraints.len(), 1);
        assert_eq!(field.constraints[0].bound, "!= null");
    }

    #[test]
    fn equality_constraint_with_identifier() {
        let source = "-- allium: 3\nentity Foo {\n  kind: String\n  invariant KindFixed { this.kind = bar }\n}";
        let spec = parse_model(source);
        let field = find_field(find_entity(&spec, "Foo"), "kind");
        assert_eq!(field.constraints[0].bound, "= bar");
    }

    #[test]
    fn complex_multi_field_invariant_no_field_constraint() {
        let source = "-- allium: 3\nentity Transfer {\n  credit: Decimal\n  debit: Decimal\n  invariant Balanced { this.credit >= this.debit }\n}";
        let spec = parse_model(source);
        for field in &spec.entities[0].fields {
            assert!(field.constraints.is_empty(), "multi-field invariant should not attach to any single field");
        }
        assert_eq!(spec.entities[0].invariants[0].expression, "this.credit >= this.debit");
    }

    #[test]
    fn multiple_invariants_attach_to_same_field() {
        let source = "-- allium: 3\nentity Gauge {\n  level: Integer\n  invariant MinLevel { this.level >= 0 }\n  invariant MaxLevel { this.level <= 100 }\n}";
        let spec = parse_model(source);
        let field = &spec.entities[0].fields[0];
        assert_eq!(field.constraints.len(), 2);
        assert_eq!(field.constraints[0].bound, ">= 0");
        assert_eq!(field.constraints[1].bound, "<= 100");
    }

    #[test]
    fn logical_invariant_no_field_constraint() {
        let source = "-- allium: 3\nentity Foo {\n  a: Integer\n  invariant Complex { this.a >= 0 and this.a <= 10 }\n}";
        let spec = parse_model(source);
        let field = find_field(find_entity(&spec, "Foo"), "a");
        // LogicalOp wrapping, not a bare Comparison — no constraint extracted
        assert!(field.constraints.is_empty());
    }

    // --- JSON serialisation ---

    #[test]
    fn constraints_omitted_from_json_when_empty() {
        let source = "-- allium: 3\nentity Plain {\n  name: String\n}";
        let spec = parse_model(source);
        let json = to_json(&spec);
        let field = &json["entities"][0]["fields"][0];
        assert!(field.get("constraints").is_none(), "empty constraints should be omitted from JSON");
    }

    #[test]
    fn constraints_present_in_json_when_populated() {
        let source = "-- allium: 3\nentity Order {\n  gap: Integer\n  invariant PositiveGap { this.gap >= 1 }\n}";
        let spec = parse_model(source);
        let json = to_json(&spec);
        let constraints = &json["entities"][0]["fields"][0]["constraints"];
        assert!(constraints.is_array());
        assert_eq!(constraints[0]["invariant"], "PositiveGap");
        assert_eq!(constraints[0]["bound"], ">= 1");
    }

    #[test]
    fn when_set_omitted_from_json_when_absent() {
        let spec = parse_model("-- allium: 3\nentity Foo { x: Integer }");
        let json = to_json(&spec);
        assert!(json["entities"][0]["fields"][0].get("when_set").is_none());
    }

    // --- Full round-trip: entity with mixed items ---

    #[test]
    fn entity_with_fields_derived_relationship_transitions_invariant() {
        let source = "\
-- allium: 3
entity Order {
  customer: Customer
  items: OrderItem with order
  total: Integer
  status: pending | done
  is_big: total >= 100
  invariant NonNeg { this.total >= 0 }
  transitions status {
    pending -> done
    terminal: done
  }
}";
        let spec = parse_model(source);
        let e = find_entity(&spec, "Order");
        assert_eq!(e.fields.len(), 3); // customer, total, status
        assert_eq!(e.relationships.len(), 1);
        assert_eq!(e.derived_values.len(), 1);
        assert_eq!(e.invariants.len(), 1);
        assert_eq!(e.transition_graphs.len(), 1);
        assert_eq!(e.invariants[0].expression, "this.total >= 0");
        let total = find_field(e, "total");
        assert_eq!(total.constraints[0].bound, ">= 0");
    }
}
