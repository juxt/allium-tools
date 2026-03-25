//! Semantic analysis pass over the parsed AST.
//!
//! The parser produces a syntactic AST and catches structural errors.
//! This module walks the AST to find semantic issues: undefined
//! references, unused bindings, state-machine gaps, and migration
//! hints.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diagnostic::Diagnostic;
use crate::Span;

/// Run all semantic checks on a parsed module and return any diagnostics.
pub fn analyze(module: &Module) -> Vec<Diagnostic> {
    let mut ctx = Ctx::new(module);
    ctx.check_related_surface_references();
    ctx.check_discriminator_variants();
    ctx.check_surface_binding_usage();
    ctx.check_status_state_machine();
    ctx.check_external_entity_source_hints();
    ctx.diagnostics
}

// ---------------------------------------------------------------------------
// Analysis context
// ---------------------------------------------------------------------------

struct Ctx<'a> {
    module: &'a Module,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Ctx<'a> {
    fn new(module: &'a Module) -> Self {
        Self {
            module,
            diagnostics: Vec::new(),
        }
    }

    fn blocks(&self, kind: BlockKind) -> impl Iterator<Item = &'a BlockDecl> {
        self.module.declarations.iter().filter_map(move |d| match d {
            Decl::Block(b) if b.kind == kind => Some(b),
            _ => None,
        })
    }

    fn variants(&self) -> impl Iterator<Item = &'a VariantDecl> {
        self.module
            .declarations
            .iter()
            .filter_map(|d| match d {
                Decl::Variant(v) => Some(v),
                _ => None,
            })
    }

    fn has_use_imports(&self) -> bool {
        self.module
            .declarations
            .iter()
            .any(|d| matches!(d, Decl::Use(_)))
    }
}

// ---------------------------------------------------------------------------
// 1. Related surface references
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_related_surface_references(&mut self) {
        let surface_names: HashSet<&str> = self
            .blocks(BlockKind::Surface)
            .filter_map(|b| b.name.as_ref().map(|n| n.name.as_str()))
            .collect();

        for surface in self.blocks(BlockKind::Surface) {
            let surface_name = match &surface.name {
                Some(n) => &n.name,
                None => continue,
            };

            for item in &surface.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "related" {
                    continue;
                }

                let refs = extract_related_surface_names(value);
                for ident in refs {
                    if !surface_names.contains(ident.name.as_str()) {
                        self.diagnostics.push(Diagnostic::error(
                            ident.span,
                            format!(
                                "Surface '{surface_name}' references unknown related surface '{}'.",
                                ident.name
                            ),
                        ));
                    }
                }
            }
        }
    }
}

/// Extract surface name identifiers from a related clause value.
///
/// Related clauses can be:
/// - `SurfaceName` (Ident)
/// - `SurfaceName(binding)` (Call)
/// - `SurfaceName(binding) when condition` (WhenGuard wrapping Call)
/// - Multiple entries as a Block
fn extract_related_surface_names(expr: &Expr) -> Vec<&Ident> {
    match expr {
        Expr::Ident(id) => vec![id],
        Expr::Call { function, .. } => extract_leading_ident(function).into_iter().collect(),
        Expr::WhenGuard { action, .. } => extract_related_surface_names(action),
        Expr::Block { items, .. } => items
            .iter()
            .flat_map(extract_related_surface_names)
            .collect(),
        _ => vec![],
    }
}

fn extract_leading_ident(expr: &Expr) -> Option<&Ident> {
    match expr {
        Expr::Ident(id) => Some(id),
        Expr::MemberAccess { object, .. } => extract_leading_ident(object),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 2. Discriminator / variant checks
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_discriminator_variants(&mut self) {
        // Collect variant declarations: base entity → set of variant names
        let mut variants_by_base: HashMap<&str, HashSet<&str>> = HashMap::new();
        for v in self.variants() {
            if let Some(base_name) = expr_as_ident(&v.base) {
                variants_by_base
                    .entry(base_name)
                    .or_default()
                    .insert(&v.name.name);
            }
        }

        for entity in self.blocks(BlockKind::Entity) {
            let entity_name = match &entity.name {
                Some(n) => &n.name,
                None => continue,
            };

            for item in &entity.items {
                let BlockItemKind::Assignment { name: field_name, value } = &item.kind else {
                    continue;
                };

                let mut pipe_idents = Vec::new();
                collect_pipe_idents(value, &mut pipe_idents);
                if pipe_idents.len() < 2 {
                    continue;
                }

                let has_capitalised = pipe_idents.iter().any(|id| starts_uppercase(&id.name));
                if !has_capitalised {
                    continue;
                }

                let all_capitalised = pipe_idents.iter().all(|id| starts_uppercase(&id.name));
                if !all_capitalised {
                    self.diagnostics.push(Diagnostic::error(
                        value.span(),
                        format!(
                            "Entity '{entity_name}' discriminator '{}' must use only capitalised variant names.",
                            field_name.name
                        ),
                    ));
                    continue;
                }

                let declared = variants_by_base
                    .get(entity_name.as_str())
                    .cloned()
                    .unwrap_or_default();

                let missing: Vec<&&Ident> = pipe_idents
                    .iter()
                    .filter(|id| !declared.contains(id.name.as_str()))
                    .collect();

                if missing.len() == pipe_idents.len() && declared.is_empty() {
                    // All capitalised, no variant declarations at all → likely v1 inline enum
                    self.diagnostics.push(Diagnostic::error(
                        value.span(),
                        format!(
                            "Entity '{entity_name}' field '{}' uses capitalised pipe values with no variant declarations. \
                             In v3, capitalised values are variant references requiring 'variant X : {entity_name}' \
                             declarations. Use lowercase values for a plain enum.",
                            field_name.name
                        ),
                    ));
                } else {
                    for id in missing {
                        self.diagnostics.push(Diagnostic::error(
                            id.span,
                            format!(
                                "Entity '{entity_name}' discriminator references '{}' without matching \
                                 'variant {} : {entity_name}'.",
                                id.name, id.name
                            ),
                        ));
                    }
                }
            }
        }
    }
}

fn starts_uppercase(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

fn collect_pipe_idents<'a>(expr: &'a Expr, out: &mut Vec<&'a Ident>) {
    match expr {
        Expr::Ident(id) => out.push(id),
        Expr::Pipe { left, right, .. } => {
            collect_pipe_idents(left, out);
            collect_pipe_idents(right, out);
        }
        _ => {}
    }
}

fn expr_as_ident(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(id) => Some(&id.name),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 3. Unused surface bindings (skip _ discard binding)
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_surface_binding_usage(&mut self) {
        for surface in self.blocks(BlockKind::Surface) {
            let surface_name = match &surface.name {
                Some(n) => &n.name,
                None => continue,
            };

            // Collect bindings from facing/context clauses
            let mut bindings: Vec<(&str, Span)> = Vec::new();
            for item in &surface.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "facing" && keyword != "context" {
                    continue;
                }
                if let Expr::Binding { name, .. } = value {
                    bindings.push((&name.name, name.span));
                }
            }

            for (name, span) in &bindings {
                if *name == "_" {
                    continue;
                }
                // Check if the binding name appears in any other item.
                // Skip only the specific clause that declares this binding,
                // not all facing/context clauses (other context clauses may
                // reference the binding).
                let used = surface.items.iter().any(|item| {
                    let BlockItemKind::Clause { keyword, value } = &item.kind else {
                        return item_contains_ident(&item.kind, name);
                    };
                    if keyword == "facing" || keyword == "context" {
                        if let Expr::Binding {
                            name: binding_name, ..
                        } = value
                        {
                            if binding_name.name == *name {
                                return false;
                            }
                        }
                    }
                    expr_contains_ident(value, name)
                });

                if !used {
                    self.diagnostics.push(Diagnostic::warning(
                        *span,
                        format!(
                            "Surface '{surface_name}' binding '{name}' is not used in the surface body.",
                        ),
                    ));
                }
            }
        }
    }
}

fn item_contains_ident(kind: &BlockItemKind, name: &str) -> bool {
    match kind {
        BlockItemKind::Clause { value, .. } => expr_contains_ident(value, name),
        BlockItemKind::Assignment { value, .. } => expr_contains_ident(value, name),
        BlockItemKind::ParamAssignment { value, .. } => expr_contains_ident(value, name),
        BlockItemKind::Let { value, .. } => expr_contains_ident(value, name),
        BlockItemKind::ForBlock {
            collection,
            filter,
            items,
            ..
        } => {
            expr_contains_ident(collection, name)
                || filter.as_ref().is_some_and(|f| expr_contains_ident(f, name))
                || items.iter().any(|i| item_contains_ident(&i.kind, name))
        }
        BlockItemKind::IfBlock {
            branches,
            else_items,
        } => {
            branches.iter().any(|b| {
                expr_contains_ident(&b.condition, name)
                    || b.items.iter().any(|i| item_contains_ident(&i.kind, name))
            }) || else_items
                .as_ref()
                .is_some_and(|items| items.iter().any(|i| item_contains_ident(&i.kind, name)))
        }
        BlockItemKind::PathAssignment { path, value } => {
            expr_contains_ident(path, name) || expr_contains_ident(value, name)
        }
        BlockItemKind::InvariantBlock { body, .. } => expr_contains_ident(body, name),
        BlockItemKind::FieldWithWhen { value, .. } => expr_contains_ident(value, name),
        BlockItemKind::ContractsClause { .. }
        | BlockItemKind::EnumVariant { .. }
        | BlockItemKind::OpenQuestion { .. }
        | BlockItemKind::Annotation(_)
        | BlockItemKind::TransitionsBlock(_) => false,
    }
}

fn expr_contains_ident(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Ident(id) => id.name == name,
        Expr::MemberAccess { object, .. } | Expr::OptionalAccess { object, .. } => {
            expr_contains_ident(object, name)
        }
        Expr::Call { function, args, .. } => {
            expr_contains_ident(function, name)
                || args.iter().any(|a| match a {
                    CallArg::Positional(e) => expr_contains_ident(e, name),
                    CallArg::Named(n) => expr_contains_ident(&n.value, name),
                })
        }
        Expr::JoinLookup { entity, fields, .. } => {
            expr_contains_ident(entity, name)
                || fields
                    .iter()
                    .any(|f| f.value.as_ref().is_some_and(|v| expr_contains_ident(v, name)))
        }
        Expr::BinaryOp { left, right, .. }
        | Expr::Comparison { left, right, .. }
        | Expr::LogicalOp { left, right, .. }
        | Expr::Pipe { left, right, .. }
        | Expr::NullCoalesce { left, right, .. } => {
            expr_contains_ident(left, name) || expr_contains_ident(right, name)
        }
        Expr::Not { operand, .. }
        | Expr::Exists { operand, .. }
        | Expr::NotExists { operand, .. }
        | Expr::TypeOptional { inner: operand, .. } => expr_contains_ident(operand, name),
        Expr::In { element, collection, .. } | Expr::NotIn { element, collection, .. } => {
            expr_contains_ident(element, name) || expr_contains_ident(collection, name)
        }
        Expr::Where {
            source, condition, ..
        }
        | Expr::With {
            source,
            predicate: condition,
            ..
        } => expr_contains_ident(source, name) || expr_contains_ident(condition, name),
        Expr::WhenGuard {
            action, condition, ..
        } => expr_contains_ident(action, name) || expr_contains_ident(condition, name),
        Expr::Lambda { param, body, .. } => {
            expr_contains_ident(param, name) || expr_contains_ident(body, name)
        }
        Expr::Binding { name: n, value, .. } => {
            n.name == name || expr_contains_ident(value, name)
        }
        Expr::SetLiteral { elements, .. } => {
            elements.iter().any(|e| expr_contains_ident(e, name))
        }
        Expr::ObjectLiteral { fields, .. } => {
            fields.iter().any(|f| expr_contains_ident(&f.value, name))
        }
        Expr::GenericType { name: n, args, .. } => {
            expr_contains_ident(n, name) || args.iter().any(|a| expr_contains_ident(a, name))
        }
        Expr::Conditional {
            branches,
            else_body,
            ..
        } => {
            branches.iter().any(|b| {
                expr_contains_ident(&b.condition, name) || expr_contains_ident(&b.body, name)
            }) || else_body
                .as_ref()
                .is_some_and(|e| expr_contains_ident(e, name))
        }
        Expr::For {
            collection,
            filter,
            body,
            ..
        } => {
            expr_contains_ident(collection, name)
                || filter
                    .as_ref()
                    .is_some_and(|f| expr_contains_ident(f, name))
                || expr_contains_ident(body, name)
        }
        Expr::TransitionsTo {
            subject, new_state, ..
        }
        | Expr::Becomes {
            subject, new_state, ..
        } => expr_contains_ident(subject, name) || expr_contains_ident(new_state, name),
        Expr::ProjectionMap { source, .. } => expr_contains_ident(source, name),
        Expr::LetExpr { value, .. } => expr_contains_ident(value, name),
        Expr::Block { items, .. } => items.iter().any(|e| expr_contains_ident(e, name)),
        Expr::QualifiedName(_)
        | Expr::StringLiteral(_)
        | Expr::BacktickLiteral { .. }
        | Expr::NumberLiteral { .. }
        | Expr::BoolLiteral { .. }
        | Expr::Null { .. }
        | Expr::Now { .. }
        | Expr::This { .. }
        | Expr::Within { .. }
        | Expr::DurationLiteral { .. } => false,
    }
}

// ---------------------------------------------------------------------------
// 4. Status state machine (unreachable / noExit)
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_status_state_machine(&mut self) {
        // Collect entity status enums: entity name → set of status values
        let mut status_by_entity: HashMap<&str, (Vec<&Ident>, HashSet<&str>)> = HashMap::new();
        for entity in self.blocks(BlockKind::Entity) {
            let entity_name = match &entity.name {
                Some(n) => n.name.as_str(),
                None => continue,
            };
            for item in &entity.items {
                let BlockItemKind::Assignment { name, value } = &item.kind else {
                    continue;
                };
                if name.name != "status" {
                    continue;
                }
                let mut idents = Vec::new();
                collect_pipe_idents(value, &mut idents);
                if idents.len() < 2 {
                    continue;
                }
                // Only lowercase pipe values are status enums
                if idents.iter().any(|id| starts_uppercase(&id.name)) {
                    continue;
                }
                let set: HashSet<&str> = idents.iter().map(|id| id.name.as_str()).collect();
                status_by_entity.insert(entity_name, (idents, set));
            }
        }

        if status_by_entity.is_empty() {
            return;
        }

        // Collect rule binding types and status assignments
        let mut assigned_by_entity: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut transitions_by_entity: HashMap<&str, HashMap<&str, HashSet<&str>>> =
            HashMap::new();

        for rule in self.blocks(BlockKind::Rule) {
            let binding_types = collect_rule_binding_types(rule, &status_by_entity);
            let mut requires_by_binding: HashMap<&str, HashSet<&str>> = HashMap::new();

            // Scan requires clauses for status preconditions
            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "requires" {
                    continue;
                }
                visit_status_comparisons(value, &binding_types, &status_by_entity, &mut |binding, status| {
                    requires_by_binding
                        .entry(binding)
                        .or_default()
                        .insert(status);
                });
            }

            // Scan ensures clauses for status assignments
            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "ensures" {
                    continue;
                }
                visit_status_assignments(value, &binding_types, &status_by_entity, &mut |binding, target, entity| {
                    assigned_by_entity
                        .entry(entity)
                        .or_default()
                        .insert(target);

                    if let Some(sources) = requires_by_binding.get(binding) {
                        let entity_transitions =
                            transitions_by_entity.entry(entity).or_default();
                        for source in sources {
                            entity_transitions
                                .entry(source)
                                .or_default()
                                .insert(target);
                        }
                    }
                });
            }
        }

        // Emit diagnostics
        for (entity_name, (idents, values)) in &status_by_entity {
            let assigned = assigned_by_entity.get(entity_name);
            let transitions = transitions_by_entity.get(entity_name);

            // If any assigned value is not a declared status, it's a variable
            // covering all values — skip checks for this entity
            if let Some(assigned) = assigned {
                if assigned.iter().any(|v| !values.contains(v)) {
                    continue;
                }
            }

            let assigned_set = assigned.cloned().unwrap_or_default();
            let transition_map = transitions.cloned().unwrap_or_default();

            for id in idents {
                if !assigned_set.contains(id.name.as_str()) {
                    self.diagnostics.push(Diagnostic::warning(
                        id.span,
                        format!(
                            "Status '{}' in entity '{entity_name}' is never assigned by any rule ensures clause.",
                            id.name
                        ),
                    ));
                }

                if is_likely_terminal(&id.name) {
                    continue;
                }
                let exits = transition_map.get(id.name.as_str());
                if exits.is_some_and(|e| !e.is_empty()) {
                    continue;
                }
                self.diagnostics.push(Diagnostic::warning(
                    id.span,
                    format!(
                        "Status '{}' in entity '{entity_name}' has no observed transition to a different status.",
                        id.name
                    ),
                ));
            }
        }
    }
}

/// Resolve rule binding types from when: clauses.
///
/// Looks for patterns like `when: binding: Entity.status becomes ...` and
/// `when: TriggerName(binding)` where TriggerName matches an entity name.
fn collect_rule_binding_types<'a>(
    rule: &'a BlockDecl,
    status_by_entity: &HashMap<&str, (Vec<&Ident>, HashSet<&str>)>,
) -> HashMap<&'a str, &'a str> {
    let mut types = HashMap::new();
    for item in &rule.items {
        let BlockItemKind::Clause { keyword, value } = &item.kind else {
            continue;
        };
        if keyword != "when" {
            continue;
        }
        collect_binding_types_from_expr(value, status_by_entity, &mut types);
    }
    types
}

fn collect_binding_types_from_expr<'a>(
    expr: &'a Expr,
    status_by_entity: &HashMap<&str, (Vec<&Ident>, HashSet<&str>)>,
    out: &mut HashMap<&'a str, &'a str>,
) {
    match expr {
        // `binding: Entity.status becomes ...`
        Expr::Binding { name, value, .. } => {
            if let Some(entity_name) = extract_entity_from_trigger(value) {
                if status_by_entity.contains_key(entity_name) {
                    out.insert(&name.name, entity_name);
                }
            }
        }
        // `TriggerName(binding, ...)` where TriggerName is an entity name
        Expr::Call { function, args, .. } => {
            if let Expr::Ident(fn_name) = function.as_ref() {
                for arg in args {
                    if let CallArg::Positional(Expr::Ident(binding)) = arg {
                        if status_by_entity.contains_key(fn_name.name.as_str()) {
                            out.insert(&binding.name, &fn_name.name);
                        }
                    }
                }
            }
        }
        // `a or b` — check both sides
        Expr::LogicalOp { left, right, .. } => {
            collect_binding_types_from_expr(left, status_by_entity, out);
            collect_binding_types_from_expr(right, status_by_entity, out);
        }
        _ => {}
    }
}

fn extract_entity_from_trigger(expr: &Expr) -> Option<&str> {
    match expr {
        // Entity.status becomes ...
        Expr::Becomes { subject, .. } | Expr::TransitionsTo { subject, .. } => {
            extract_entity_from_member(subject)
        }
        // Entity.field ...
        Expr::MemberAccess { object, .. } => expr_as_ident(object),
        _ => None,
    }
}

fn extract_entity_from_member(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::MemberAccess { object, .. } => expr_as_ident(object),
        _ => None,
    }
}

/// Find `binding.status = value` comparisons (used as assignments in ensures).
fn visit_status_assignments<'a>(
    expr: &'a Expr,
    binding_types: &HashMap<&'a str, &'a str>,
    status_by_entity: &HashMap<&'a str, (Vec<&Ident>, HashSet<&'a str>)>,
    cb: &mut impl FnMut(&'a str, &'a str, &'a str),
) {
    match expr {
        Expr::Comparison {
            left,
            op: ComparisonOp::Eq,
            right,
            ..
        } => {
            if let (Some((binding, "status")), Some(target)) =
                (expr_as_member_access(left), expr_as_ident(right))
            {
                let entity = binding_types.get(binding).copied().or_else(|| {
                    status_by_entity
                        .keys()
                        .find(|name| name.eq_ignore_ascii_case(binding))
                        .copied()
                });
                if let Some(entity) = entity {
                    cb(binding, target, entity);
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                visit_status_assignments(item, binding_types, status_by_entity, cb);
            }
        }
        Expr::Conditional {
            branches,
            else_body,
            ..
        } => {
            for branch in branches {
                visit_status_assignments(&branch.body, binding_types, status_by_entity, cb);
            }
            if let Some(body) = else_body {
                visit_status_assignments(body, binding_types, status_by_entity, cb);
            }
        }
        _ => {}
    }
}

/// Find `binding.status = value` comparisons in requires clauses.
fn visit_status_comparisons<'a>(
    expr: &'a Expr,
    binding_types: &HashMap<&'a str, &'a str>,
    status_by_entity: &HashMap<&'a str, (Vec<&Ident>, HashSet<&'a str>)>,
    cb: &mut impl FnMut(&'a str, &'a str),
) {
    match expr {
        Expr::Comparison {
            left,
            op: ComparisonOp::Eq,
            right,
            ..
        } => {
            if let (Some((binding, "status")), Some(target)) =
                (expr_as_member_access(left), expr_as_ident(right))
            {
                let known = binding_types.contains_key(binding)
                    || status_by_entity
                        .keys()
                        .any(|name| name.eq_ignore_ascii_case(binding));
                if known {
                    cb(binding, target);
                }
            }
        }
        Expr::LogicalOp { left, right, .. } => {
            visit_status_comparisons(left, binding_types, status_by_entity, cb);
            visit_status_comparisons(right, binding_types, status_by_entity, cb);
        }
        Expr::Block { items, .. } => {
            for item in items {
                visit_status_comparisons(item, binding_types, status_by_entity, cb);
            }
        }
        _ => {}
    }
}

fn expr_as_member_access(expr: &Expr) -> Option<(&str, &str)> {
    match expr {
        Expr::MemberAccess { object, field, .. } => {
            expr_as_ident(object).map(|obj| (obj, field.name.as_str()))
        }
        _ => None,
    }
}

fn is_likely_terminal(status: &str) -> bool {
    matches!(
        status,
        "completed"
            | "cancelled"
            | "canceled"
            | "expired"
            | "closed"
            | "deleted"
            | "archived"
            | "failed"
            | "rejected"
            | "done"
    )
}

// ---------------------------------------------------------------------------
// 5. External entity source hints
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_external_entity_source_hints(&mut self) {
        if self.has_use_imports() {
            return;
        }

        let rule_blocks: Vec<&BlockDecl> = self.blocks(BlockKind::Rule).collect();

        for entity in self.blocks(BlockKind::ExternalEntity) {
            let name = match &entity.name {
                Some(n) => n,
                None => continue,
            };

            let referenced_in_rules = rule_blocks
                .iter()
                .any(|rule| rule.items.iter().any(|i| item_contains_ident(&i.kind, &name.name)));

            let msg = format!(
                "External entity '{}' has no obvious governing specification import in this module.",
                name.name
            );
            if referenced_in_rules {
                self.diagnostics.push(Diagnostic::info(name.span, msg));
            } else {
                self.diagnostics.push(Diagnostic::warning(name.span, msg));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::diagnostic::Severity;

    fn analyze_src(src: &str) -> Vec<Diagnostic> {
        let input = if src.starts_with("-- allium:") {
            src.to_string()
        } else {
            format!("-- allium: 3\n{src}")
        };
        let result = parse(&input);
        analyze(&result.module)
    }

    fn has_message_containing(diagnostics: &[Diagnostic], needle: &str) -> bool {
        diagnostics.iter().any(|d| d.message.contains(needle))
    }

    // -- Fix 1: related surface references --

    #[test]
    fn related_clause_with_binding_and_guard_only_checks_surface_name() {
        let ds = analyze_src(
            "surface QuoteVersions {\n  facing user: User\n}\n\n\
             surface Dashboard {\n  facing user: User\n  related:\n    QuoteVersions(quote) when quote.version_count > 1\n}\n",
        );
        assert!(
            !has_message_containing(&ds, "relatedUndefined")
                && !has_message_containing(&ds, "unknown related surface"),
            "should not report QuoteVersions as undefined"
        );
    }

    #[test]
    fn related_clause_reports_unknown_surface() {
        let ds = analyze_src(
            "surface Dashboard {\n  facing user: User\n  related:\n    MissingSurface\n}\n",
        );
        assert!(has_message_containing(&ds, "unknown related surface 'MissingSurface'"));
    }

    // -- Fix 2: v1 capitalised inline enum --

    #[test]
    fn capitalised_pipe_values_without_variants_reports_v1_enum() {
        let ds = analyze_src("entity Quote {\n  status: Quoted | OrderSubmitted | Filled\n}\n");
        assert!(has_message_containing(&ds, "capitalised pipe values"));
    }

    // -- Fix 3: discard binding _ --

    #[test]
    fn discard_binding_underscore_does_not_warn_unused() {
        let ds = analyze_src(
            "surface QuoteFeed {\n  facing _: Service\n  exposes:\n    System.status\n}\n",
        );
        assert!(
            !ds.iter().any(|d| d.message.contains("not used")),
            "should not warn about _ binding"
        );
    }

    #[test]
    fn named_unused_binding_does_warn() {
        let ds = analyze_src(
            "surface Dashboard {\n  facing viewer: User\n  exposes:\n    System.status\n}\n",
        );
        assert!(has_message_containing(&ds, "not used"));
    }

    // -- Fix 4: variable status assignment suppresses unreachable/noExit --

    #[test]
    fn variable_status_assignment_suppresses_unreachable() {
        let ds = analyze_src(
            "entity Quote {\n  status: pending | quoted | filled\n}\n\n\
             rule ApplyStatusUpdate {\n  when: update: Quote.status becomes pending\n  \
             ensures: update.status = new_status\n}\n",
        );
        assert!(
            !has_message_containing(&ds, "never assigned"),
            "variable assignment should cover all status values"
        );
        assert!(
            !has_message_containing(&ds, "no observed transition"),
            "variable assignment should cover all transitions"
        );
    }

    // -- Fix 5: external entity source hint --

    #[test]
    fn external_entity_referenced_in_rules_downgrades_to_info() {
        let ds = analyze_src(
            "external entity Client {\n  id: String\n}\n\n\
             rule IngestQuote {\n  when: RawQuoteReceived(data)\n  ensures:\n    Client.lookup(data.client_id)\n}\n",
        );
        let hint = ds
            .iter()
            .find(|d| d.message.contains("no obvious governing specification import"));
        assert!(hint.is_some(), "should still emit the hint");
        assert_eq!(hint.unwrap().severity, Severity::Info);
    }

    #[test]
    fn external_entity_not_in_rules_warns() {
        let ds = analyze_src("external entity Client {\n  id: String\n}\n");
        let hint = ds
            .iter()
            .find(|d| d.message.contains("no obvious governing specification import"));
        assert!(hint.is_some());
        assert_eq!(hint.unwrap().severity, Severity::Warning);
    }
}
