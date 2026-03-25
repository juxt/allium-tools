//! Semantic analysis pass over the parsed AST.
//!
//! The parser produces a syntactic AST and catches structural errors.
//! This module walks the AST to find semantic issues: undefined
//! references, unused bindings, state-machine gaps, and migration
//! hints.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diagnostic::Diagnostic;
use crate::lexer::SourceMap;
use crate::Span;

/// Run all semantic checks on a parsed module and return any diagnostics.
pub fn analyze(module: &Module, source: &str) -> Vec<Diagnostic> {
    let mut ctx = Ctx::new(module);

    // Existing checks
    ctx.check_related_surface_references();
    ctx.check_discriminator_variants();
    ctx.check_surface_binding_usage();
    ctx.check_status_state_machine();
    ctx.check_external_entity_source_hints();

    // New checks
    ctx.check_type_references();
    ctx.check_unreachable_triggers();
    ctx.check_unused_fields();
    ctx.check_unused_entities();
    ctx.check_unused_definitions();
    ctx.check_deferred_location_hints();
    ctx.check_rule_invalid_triggers();
    ctx.check_rule_undefined_bindings();
    ctx.check_duplicate_let_bindings();
    ctx.check_config_undefined_references();
    // TODO: surface unused path check needs proper cross-reference resolution
    // ctx.check_surface_unused_paths();

    apply_suppressions(ctx.diagnostics, source)
}

// ---------------------------------------------------------------------------
// Suppression: -- allium-ignore <code>[, <code>...]
// ---------------------------------------------------------------------------

fn apply_suppressions(diagnostics: Vec<Diagnostic>, source: &str) -> Vec<Diagnostic> {
    if diagnostics.is_empty() {
        return diagnostics;
    }
    let sm = SourceMap::new(source);
    let directives = collect_suppression_directives(source, &sm);
    if directives.is_empty() {
        return diagnostics;
    }
    diagnostics
        .into_iter()
        .filter(|d| {
            let (line, _) = sm.line_col(d.span.start);
            let line = line as i64;
            let active = directives
                .get(&(line as u32))
                .or_else(|| directives.get(&((line - 1).max(0) as u32)));
            match (active, d.code) {
                (Some(codes), Some(code)) => !(codes.contains("all") || codes.contains(&code)),
                (Some(codes), None) => !codes.contains("all"),
                _ => true,
            }
        })
        .collect()
}

fn collect_suppression_directives<'a>(source: &'a str, sm: &SourceMap) -> HashMap<u32, HashSet<&'a str>> {
    let mut directives = HashMap::new();
    let pattern = regex_lite::Regex::new(r"(?m)^[^\S\n]*--\s*allium-ignore\s+([A-Za-z0-9._,\- \t]+)$").unwrap();
    for m in pattern.find_iter(source) {
        let text = m.as_str();
        let (line, _) = sm.line_col(m.start());
        // Extract the codes portion after "allium-ignore "
        if let Some(idx) = text.find("allium-ignore") {
            let offset = m.start() + idx + "allium-ignore".len();
            let source_after = &source[offset..m.end()];
            let codes: HashSet<&'a str> = source_after
                .split(',')
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .collect();
            directives.insert(line, codes);
        }
    }
    directives
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

    fn push(&mut self, d: Diagnostic) {
        self.diagnostics.push(d);
    }

    /// All declared type names (entities, values, enums, actors, variants, externals)
    /// plus built-in types.
    fn declared_type_names(&self) -> HashSet<&'a str> {
        let mut names = HashSet::new();
        for d in &self.module.declarations {
            match d {
                Decl::Block(b) => {
                    if matches!(
                        b.kind,
                        BlockKind::Entity
                            | BlockKind::ExternalEntity
                            | BlockKind::Value
                            | BlockKind::Enum
                            | BlockKind::Actor
                    ) {
                        if let Some(n) = &b.name {
                            names.insert(n.name.as_str());
                        }
                    }
                }
                Decl::Variant(v) => {
                    names.insert(v.name.name.as_str());
                }
                _ => {}
            }
        }
        // Built-in types
        for t in &[
            "String", "Integer", "Decimal", "Boolean", "Timestamp", "Duration",
            "List", "Set", "Map", "Any", "Void",
        ] {
            names.insert(t);
        }
        // Use aliases
        for d in &self.module.declarations {
            if let Decl::Use(u) = d {
                if let Some(alias) = &u.alias {
                    names.insert(alias.name.as_str());
                }
            }
        }
        names
    }

    /// Collect all field names accessed via member access across the module.
    fn collect_all_accessed_field_names(&self) -> HashSet<&'a str> {
        let mut names = HashSet::new();
        for d in &self.module.declarations {
            match d {
                Decl::Block(b) => {
                    for item in &b.items {
                        collect_accessed_fields_from_item(&item.kind, &mut names);
                    }
                }
                Decl::Invariant(inv) => {
                    collect_accessed_fields_from_expr(&inv.body, &mut names);
                }
                _ => {}
            }
        }
        names
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
                        self.push(
                            Diagnostic::error(
                                ident.span,
                                format!(
                                    "Surface '{surface_name}' references unknown related surface '{}'.",
                                    ident.name
                                ),
                            )
                            .with_code("allium.surface.relatedUndefined"),
                        );
                    }
                }
            }
        }
    }
}

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
                    self.push(
                        Diagnostic::error(
                            value.span(),
                            format!(
                                "Entity '{entity_name}' discriminator '{}' must use only capitalised variant names.",
                                field_name.name
                            ),
                        )
                        .with_code("allium.sum.invalidDiscriminator"),
                    );
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
                    self.push(
                        Diagnostic::error(
                            value.span(),
                            format!(
                                "Entity '{entity_name}' field '{}' uses capitalised pipe values with no variant declarations. \
                                 In v3, capitalised values are variant references requiring 'variant X : {entity_name}' \
                                 declarations. Use lowercase values for a plain enum.",
                                field_name.name
                            ),
                        )
                        .with_code("allium.sum.v1InlineEnum"),
                    );
                } else {
                    for id in missing {
                        self.push(
                            Diagnostic::error(
                                id.span,
                                format!(
                                    "Entity '{entity_name}' discriminator references '{}' without matching \
                                     'variant {} : {entity_name}'.",
                                    id.name, id.name
                                ),
                            )
                            .with_code("allium.sum.discriminatorUnknownVariant"),
                        );
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
                    self.push(
                        Diagnostic::warning(
                            *span,
                            format!(
                                "Surface '{surface_name}' binding '{name}' is not used in the surface body.",
                            ),
                        )
                        .with_code("allium.surface.unusedBinding"),
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Status state machine (unreachable / noExit)
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_status_state_machine(&mut self) {
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

        let mut assigned_by_entity: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut transitions_by_entity: HashMap<&str, HashMap<&str, HashSet<&str>>> =
            HashMap::new();

        for rule in self.blocks(BlockKind::Rule) {
            let binding_types = collect_rule_binding_types(rule, &status_by_entity);
            let mut requires_by_binding: HashMap<&str, HashSet<&str>> = HashMap::new();

            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "requires" {
                    continue;
                }
                visit_status_comparisons(
                    value,
                    &binding_types,
                    &status_by_entity,
                    &mut |binding, status| {
                        requires_by_binding
                            .entry(binding)
                            .or_default()
                            .insert(status);
                    },
                );
            }

            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "ensures" {
                    continue;
                }
                visit_status_assignments(
                    value,
                    &binding_types,
                    &status_by_entity,
                    &mut |binding, target, entity| {
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
                    },
                );
            }
        }

        for (entity_name, (idents, values)) in &status_by_entity {
            let assigned = assigned_by_entity.get(entity_name);
            let transitions = transitions_by_entity.get(entity_name);

            if let Some(assigned) = assigned {
                if assigned.iter().any(|v| !values.contains(v)) {
                    continue;
                }
            }

            let assigned_set = assigned.cloned().unwrap_or_default();
            let transition_map = transitions.cloned().unwrap_or_default();

            for id in idents {
                if !assigned_set.contains(id.name.as_str()) {
                    self.push(
                        Diagnostic::warning(
                            id.span,
                            format!(
                                "Status '{}' in entity '{entity_name}' is never assigned by any rule ensures clause.",
                                id.name
                            ),
                        )
                        .with_code("allium.status.unreachableValue"),
                    );
                }

                if is_likely_terminal(&id.name) {
                    continue;
                }
                let exits = transition_map.get(id.name.as_str());
                if exits.is_some_and(|e| !e.is_empty()) {
                    continue;
                }
                self.push(
                    Diagnostic::warning(
                        id.span,
                        format!(
                            "Status '{}' in entity '{entity_name}' has no observed transition to a different status.",
                            id.name
                        ),
                    )
                    .with_code("allium.status.noExit"),
                );
            }
        }
    }
}

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
        Expr::Binding { name, value, .. } => {
            if let Some(entity_name) = extract_entity_from_trigger(value) {
                if status_by_entity.contains_key(entity_name) {
                    out.insert(&name.name, entity_name);
                }
            }
        }
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
        Expr::LogicalOp { left, right, .. } => {
            collect_binding_types_from_expr(left, status_by_entity, out);
            collect_binding_types_from_expr(right, status_by_entity, out);
        }
        _ => {}
    }
}

fn extract_entity_from_trigger(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Becomes { subject, .. } | Expr::TransitionsTo { subject, .. } => {
            extract_entity_from_member(subject)
        }
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
                self.push(Diagnostic::info(name.span, msg).with_code("allium.externalEntity.missingSourceHint"));
            } else {
                self.push(Diagnostic::warning(name.span, msg).with_code("allium.externalEntity.missingSourceHint"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Type reference checks (undeclared types in entity/value fields)
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_type_references(&mut self) {
        let known = self.declared_type_names();

        for d in &self.module.declarations {
            let block = match d {
                Decl::Block(b)
                    if matches!(
                        b.kind,
                        BlockKind::Entity
                            | BlockKind::ExternalEntity
                            | BlockKind::Value
                    ) =>
                {
                    b
                }
                Decl::Variant(v) => {
                    // Check variant items
                    for item in &v.items {
                        self.check_type_ref_in_item(item, &known);
                    }
                    continue;
                }
                _ => continue,
            };

            for item in &block.items {
                self.check_type_ref_in_item(item, &known);
            }
        }

        // Check rule type references (when clauses, ensures entity references)
        for rule in self.blocks(BlockKind::Rule) {
            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword == "when" || keyword == "ensures" || keyword == "requires" {
                    self.check_type_refs_in_rule_expr(value, &known);
                }
            }
        }
    }

    fn check_type_ref_in_item(&mut self, item: &BlockItem, known: &HashSet<&str>) {
        match &item.kind {
            BlockItemKind::Assignment { value, .. }
            | BlockItemKind::FieldWithWhen { value, .. } => {
                self.check_type_refs_in_value(value, known);
            }
            _ => {}
        }
    }

    fn check_type_refs_in_value(&mut self, expr: &Expr, known: &HashSet<&str>) {
        match expr {
            Expr::Ident(id) if starts_uppercase(&id.name) => {
                if !known.contains(id.name.as_str()) {
                    self.push(
                        Diagnostic::error(
                            id.span,
                            format!(
                                "Type reference '{}' is not declared locally or imported.",
                                id.name
                            ),
                        )
                        .with_code("allium.type.undefinedReference"),
                    );
                }
            }
            Expr::GenericType { name, args, .. } => {
                self.check_type_refs_in_value(name, known);
                for arg in args {
                    self.check_type_refs_in_value(arg, known);
                }
            }
            Expr::Pipe { left, right, .. } => {
                self.check_type_refs_in_value(left, known);
                self.check_type_refs_in_value(right, known);
            }
            Expr::TypeOptional { inner, .. } => {
                self.check_type_refs_in_value(inner, known);
            }
            _ => {}
        }
    }

    fn check_type_refs_in_rule_expr(&mut self, expr: &Expr, known: &HashSet<&str>) {
        match expr {
            // binding: Entity.field becomes ... — check Entity
            Expr::Binding { value, .. } => {
                self.check_type_refs_in_rule_expr(value, known);
            }
            Expr::Becomes { subject, .. } | Expr::TransitionsTo { subject, .. } => {
                if let Expr::MemberAccess { object, .. } = subject.as_ref() {
                    if let Expr::Ident(id) = object.as_ref() {
                        if starts_uppercase(&id.name) && !known.contains(id.name.as_str()) {
                            self.push(
                                Diagnostic::error(
                                    id.span,
                                    format!(
                                        "Type reference '{}' is not declared locally or imported.",
                                        id.name
                                    ),
                                )
                                .with_code("allium.rule.undefinedTypeReference"),
                            );
                        }
                    }
                }
            }
            // Entity.created(...) or Entity.lookup(...)
            Expr::Call { function, .. } => {
                if let Expr::MemberAccess { object, .. } = function.as_ref() {
                    if let Expr::Ident(id) = object.as_ref() {
                        if starts_uppercase(&id.name) && !known.contains(id.name.as_str()) {
                            self.push(
                                Diagnostic::error(
                                    id.span,
                                    format!(
                                        "Type reference '{}' is not declared locally or imported.",
                                        id.name
                                    ),
                                )
                                .with_code("allium.rule.undefinedTypeReference"),
                            );
                        }
                    }
                }
            }
            Expr::Block { items, .. } => {
                for item in items {
                    self.check_type_refs_in_rule_expr(item, known);
                }
            }
            Expr::LogicalOp { left, right, .. } => {
                self.check_type_refs_in_rule_expr(left, known);
                self.check_type_refs_in_rule_expr(right, known);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// 7. Unreachable triggers
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_unreachable_triggers(&mut self) {
        // Collect triggers provided by surfaces
        let mut provided: HashSet<&str> = HashSet::new();
        for surface in self.blocks(BlockKind::Surface) {
            for item in &surface.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "provides" {
                    continue;
                }
                collect_call_names(value, &mut provided);
            }
        }

        // Collect triggers emitted by rule ensures
        let mut emitted: HashSet<&str> = HashSet::new();
        for rule in self.blocks(BlockKind::Rule) {
            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "ensures" {
                    continue;
                }
                collect_call_names(value, &mut emitted);
            }
        }

        for rule in self.blocks(BlockKind::Rule) {
            let rule_name = match &rule.name {
                Some(n) => &n.name,
                None => continue,
            };
            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "when" {
                    continue;
                }
                let trigger_names = extract_trigger_names(value);
                for (name, span) in trigger_names {
                    if !provided.contains(name) && !emitted.contains(name) {
                        self.push(
                            Diagnostic::info(
                                span,
                                format!(
                                    "Rule '{rule_name}' listens for trigger '{name}' but no local surface provides or rule emits it.",
                                ),
                            )
                            .with_code("allium.rule.unreachableTrigger"),
                        );
                    }
                }
            }
        }
    }
}

fn collect_call_names<'a>(expr: &'a Expr, out: &mut HashSet<&'a str>) {
    match expr {
        Expr::Call { function, .. } => {
            if let Expr::Ident(id) = function.as_ref() {
                if starts_uppercase(&id.name) {
                    out.insert(&id.name);
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_call_names(item, out);
            }
        }
        Expr::WhenGuard { action, .. } => {
            collect_call_names(action, out);
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_call_names(&b.body, out);
            }
            if let Some(body) = else_body {
                collect_call_names(body, out);
            }
        }
        _ => {}
    }
}

fn extract_trigger_names(expr: &Expr) -> Vec<(&str, Span)> {
    match expr {
        Expr::Call { function, .. } => {
            if let Expr::Ident(id) = function.as_ref() {
                if starts_uppercase(&id.name) {
                    return vec![(&id.name, id.span)];
                }
            }
            vec![]
        }
        Expr::Binding { .. } => {
            // binding: Entity.field becomes ... — not a trigger call
            vec![]
        }
        Expr::LogicalOp { left, right, .. } => {
            let mut out = extract_trigger_names(left);
            out.extend(extract_trigger_names(right));
            out
        }
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// 8. Unused fields
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_unused_fields(&mut self) {
        let accessed = self.collect_all_accessed_field_names();

        for d in &self.module.declarations {
            let block = match d {
                Decl::Block(b)
                    if matches!(
                        b.kind,
                        BlockKind::Entity | BlockKind::ExternalEntity | BlockKind::Value
                    ) =>
                {
                    b
                }
                Decl::Variant(v) => {
                    let entity_name = &v.name.name;
                    for item in &v.items {
                        if let BlockItemKind::Assignment { name, .. }
                        | BlockItemKind::FieldWithWhen { name, .. } = &item.kind
                        {
                            if !accessed.contains(name.name.as_str()) {
                                self.push(
                                    Diagnostic::info(
                                        name.span,
                                        format!(
                                            "Field '{entity_name}.{}' is declared but not referenced elsewhere.",
                                            name.name
                                        ),
                                    )
                                    .with_code("allium.field.unused"),
                                );
                            }
                        }
                    }
                    continue;
                }
                _ => continue,
            };

            let entity_name = match &block.name {
                Some(n) => &n.name,
                None => continue,
            };

            for item in &block.items {
                if let BlockItemKind::Assignment { name, .. }
                | BlockItemKind::FieldWithWhen { name, .. } = &item.kind
                {
                    if !accessed.contains(name.name.as_str()) {
                        self.push(
                            Diagnostic::info(
                                name.span,
                                format!(
                                    "Field '{entity_name}.{}' is declared but not referenced elsewhere.",
                                    name.name
                                ),
                            )
                            .with_code("allium.field.unused"),
                        );
                    }
                }
            }
        }
    }
}

fn collect_accessed_fields_from_item<'a>(kind: &'a BlockItemKind, out: &mut HashSet<&'a str>) {
    match kind {
        BlockItemKind::Clause { value, .. }
        | BlockItemKind::Assignment { value, .. }
        | BlockItemKind::ParamAssignment { value, .. }
        | BlockItemKind::Let { value, .. }
        | BlockItemKind::PathAssignment { value, .. }
        | BlockItemKind::InvariantBlock { body: value, .. }
        | BlockItemKind::FieldWithWhen { value, .. } => {
            collect_accessed_fields_from_expr(value, out);
        }
        BlockItemKind::ForBlock {
            collection,
            filter,
            items,
            ..
        } => {
            collect_accessed_fields_from_expr(collection, out);
            if let Some(f) = filter {
                collect_accessed_fields_from_expr(f, out);
            }
            for item in items {
                collect_accessed_fields_from_item(&item.kind, out);
            }
        }
        BlockItemKind::IfBlock {
            branches,
            else_items,
        } => {
            for b in branches {
                collect_accessed_fields_from_expr(&b.condition, out);
                for item in &b.items {
                    collect_accessed_fields_from_item(&item.kind, out);
                }
            }
            if let Some(items) = else_items {
                for item in items {
                    collect_accessed_fields_from_item(&item.kind, out);
                }
            }
        }
        _ => {}
    }
}

fn collect_accessed_fields_from_expr<'a>(expr: &'a Expr, out: &mut HashSet<&'a str>) {
    match expr {
        Expr::MemberAccess { object, field, .. } | Expr::OptionalAccess { object, field, .. } => {
            out.insert(&field.name);
            collect_accessed_fields_from_expr(object, out);
        }
        Expr::Call { function, args, .. } => {
            collect_accessed_fields_from_expr(function, out);
            for a in args {
                match a {
                    CallArg::Positional(e) => collect_accessed_fields_from_expr(e, out),
                    CallArg::Named(n) => collect_accessed_fields_from_expr(&n.value, out),
                }
            }
        }
        Expr::BinaryOp { left, right, .. }
        | Expr::Comparison { left, right, .. }
        | Expr::LogicalOp { left, right, .. }
        | Expr::Pipe { left, right, .. }
        | Expr::NullCoalesce { left, right, .. } => {
            collect_accessed_fields_from_expr(left, out);
            collect_accessed_fields_from_expr(right, out);
        }
        Expr::Not { operand, .. }
        | Expr::Exists { operand, .. }
        | Expr::NotExists { operand, .. }
        | Expr::TypeOptional { inner: operand, .. } => {
            collect_accessed_fields_from_expr(operand, out);
        }
        Expr::In { element, collection, .. } | Expr::NotIn { element, collection, .. } => {
            collect_accessed_fields_from_expr(element, out);
            collect_accessed_fields_from_expr(collection, out);
        }
        Expr::Where { source, condition, .. }
        | Expr::With {
            source,
            predicate: condition,
            ..
        } => {
            collect_accessed_fields_from_expr(source, out);
            collect_accessed_fields_from_expr(condition, out);
        }
        Expr::WhenGuard { action, condition, .. } => {
            collect_accessed_fields_from_expr(action, out);
            collect_accessed_fields_from_expr(condition, out);
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_accessed_fields_from_expr(item, out);
            }
        }
        Expr::Binding { value, .. } | Expr::LetExpr { value, .. } => {
            collect_accessed_fields_from_expr(value, out);
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_accessed_fields_from_expr(&b.condition, out);
                collect_accessed_fields_from_expr(&b.body, out);
            }
            if let Some(body) = else_body {
                collect_accessed_fields_from_expr(body, out);
            }
        }
        Expr::For { collection, filter, body, .. } => {
            collect_accessed_fields_from_expr(collection, out);
            if let Some(f) = filter {
                collect_accessed_fields_from_expr(f, out);
            }
            collect_accessed_fields_from_expr(body, out);
        }
        Expr::Lambda { body, .. } => {
            collect_accessed_fields_from_expr(body, out);
        }
        Expr::JoinLookup { entity, fields, .. } => {
            collect_accessed_fields_from_expr(entity, out);
            for f in fields {
                out.insert(&f.field.name);
                if let Some(v) = &f.value {
                    collect_accessed_fields_from_expr(v, out);
                }
            }
        }
        Expr::TransitionsTo { subject, new_state, .. }
        | Expr::Becomes { subject, new_state, .. } => {
            collect_accessed_fields_from_expr(subject, out);
            collect_accessed_fields_from_expr(new_state, out);
        }
        Expr::SetLiteral { elements, .. } => {
            for e in elements {
                collect_accessed_fields_from_expr(e, out);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for f in fields {
                collect_accessed_fields_from_expr(&f.value, out);
            }
        }
        Expr::GenericType { name, args, .. } => {
            collect_accessed_fields_from_expr(name, out);
            for a in args {
                collect_accessed_fields_from_expr(a, out);
            }
        }
        Expr::ProjectionMap { source, .. } => {
            collect_accessed_fields_from_expr(source, out);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// 9. Unused entities
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_unused_entities(&mut self) {
        let all_idents = self.collect_all_referenced_idents();
        let mut findings = Vec::new();

        for d in &self.module.declarations {
            let block = match d {
                Decl::Block(b)
                    if matches!(
                        b.kind,
                        BlockKind::Entity | BlockKind::ExternalEntity
                    ) =>
                {
                    b
                }
                _ => continue,
            };
            let name = match &block.name {
                Some(n) => n,
                None => continue,
            };
            if !all_idents.contains(name.name.as_str()) {
                findings.push(
                    Diagnostic::warning(
                        name.span,
                        format!(
                            "Entity '{}' is declared but not referenced elsewhere in this specification.",
                            name.name
                        ),
                    )
                    .with_code("allium.entity.unused"),
                );
            }
        }
        self.diagnostics.extend(findings);
    }

    fn check_unused_definitions(&mut self) {
        let all_idents = self.collect_all_referenced_idents();
        let mut findings = Vec::new();

        for d in &self.module.declarations {
            match d {
                Decl::Block(b) if b.kind == BlockKind::Value || b.kind == BlockKind::Enum => {
                    let name = match &b.name {
                        Some(n) => n,
                        None => continue,
                    };
                    if !all_idents.contains(name.name.as_str()) {
                        findings.push(
                            Diagnostic::warning(
                                name.span,
                                format!(
                                    "Value '{}' is declared but not referenced elsewhere.",
                                    name.name
                                ),
                            )
                            .with_code("allium.definition.unused"),
                        );
                    }
                }
                _ => {}
            }
        }
        self.diagnostics.extend(findings);
    }

    /// Collect all capitalised identifiers referenced in expressions across the module,
    /// excluding the declaration name positions themselves.
    fn collect_all_referenced_idents(&self) -> HashSet<&str> {
        let mut names = HashSet::new();
        for d in &self.module.declarations {
            match d {
                Decl::Block(b) => {
                    for item in &b.items {
                        collect_uppercase_idents_from_item(&item.kind, &mut names);
                    }
                }
                Decl::Variant(v) => {
                    // The base type is a reference
                    if let Some(name) = expr_as_ident(&v.base) {
                        names.insert(name);
                    }
                    for item in &v.items {
                        collect_uppercase_idents_from_item(&item.kind, &mut names);
                    }
                }
                Decl::Invariant(inv) => {
                    collect_uppercase_idents_from_expr(&inv.body, &mut names);
                }
                Decl::Default(def) => {
                    if let Some(tn) = &def.type_name {
                        names.insert(tn.name.as_str());
                    }
                    collect_uppercase_idents_from_expr(&def.value, &mut names);
                }
                _ => {}
            }
        }
        names
    }
}

fn collect_uppercase_idents_from_item<'a>(kind: &'a BlockItemKind, out: &mut HashSet<&'a str>) {
    match kind {
        BlockItemKind::Clause { value, .. }
        | BlockItemKind::Assignment { value, .. }
        | BlockItemKind::ParamAssignment { value, .. }
        | BlockItemKind::Let { value, .. }
        | BlockItemKind::PathAssignment { value, .. }
        | BlockItemKind::InvariantBlock { body: value, .. }
        | BlockItemKind::FieldWithWhen { value, .. } => {
            collect_uppercase_idents_from_expr(value, out);
        }
        BlockItemKind::ForBlock {
            collection,
            filter,
            items,
            ..
        } => {
            collect_uppercase_idents_from_expr(collection, out);
            if let Some(f) = filter {
                collect_uppercase_idents_from_expr(f, out);
            }
            for item in items {
                collect_uppercase_idents_from_item(&item.kind, out);
            }
        }
        BlockItemKind::IfBlock {
            branches,
            else_items,
        } => {
            for b in branches {
                collect_uppercase_idents_from_expr(&b.condition, out);
                for item in &b.items {
                    collect_uppercase_idents_from_item(&item.kind, out);
                }
            }
            if let Some(items) = else_items {
                for item in items {
                    collect_uppercase_idents_from_item(&item.kind, out);
                }
            }
        }
        BlockItemKind::ContractsClause { entries } => {
            for e in entries {
                out.insert(e.name.name.as_str());
            }
        }
        _ => {}
    }
}

fn collect_uppercase_idents_from_expr<'a>(expr: &'a Expr, out: &mut HashSet<&'a str>) {
    match expr {
        Expr::Ident(id) if starts_uppercase(&id.name) => {
            out.insert(&id.name);
        }
        Expr::MemberAccess { object, .. } | Expr::OptionalAccess { object, .. } => {
            collect_uppercase_idents_from_expr(object, out);
        }
        Expr::Call { function, args, .. } => {
            collect_uppercase_idents_from_expr(function, out);
            for a in args {
                match a {
                    CallArg::Positional(e) => collect_uppercase_idents_from_expr(e, out),
                    CallArg::Named(n) => collect_uppercase_idents_from_expr(&n.value, out),
                }
            }
        }
        Expr::JoinLookup { entity, fields, .. } => {
            collect_uppercase_idents_from_expr(entity, out);
            for f in fields {
                if let Some(v) = &f.value {
                    collect_uppercase_idents_from_expr(v, out);
                }
            }
        }
        Expr::BinaryOp { left, right, .. }
        | Expr::Comparison { left, right, .. }
        | Expr::LogicalOp { left, right, .. }
        | Expr::Pipe { left, right, .. }
        | Expr::NullCoalesce { left, right, .. } => {
            collect_uppercase_idents_from_expr(left, out);
            collect_uppercase_idents_from_expr(right, out);
        }
        Expr::Not { operand, .. }
        | Expr::Exists { operand, .. }
        | Expr::NotExists { operand, .. }
        | Expr::TypeOptional { inner: operand, .. } => {
            collect_uppercase_idents_from_expr(operand, out);
        }
        Expr::In { element, collection, .. } | Expr::NotIn { element, collection, .. } => {
            collect_uppercase_idents_from_expr(element, out);
            collect_uppercase_idents_from_expr(collection, out);
        }
        Expr::Where { source, condition, .. }
        | Expr::With {
            source,
            predicate: condition,
            ..
        } => {
            collect_uppercase_idents_from_expr(source, out);
            collect_uppercase_idents_from_expr(condition, out);
        }
        Expr::WhenGuard { action, condition, .. } => {
            collect_uppercase_idents_from_expr(action, out);
            collect_uppercase_idents_from_expr(condition, out);
        }
        Expr::Binding { value, .. } | Expr::LetExpr { value, .. } => {
            collect_uppercase_idents_from_expr(value, out);
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_uppercase_idents_from_expr(item, out);
            }
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_uppercase_idents_from_expr(&b.condition, out);
                collect_uppercase_idents_from_expr(&b.body, out);
            }
            if let Some(body) = else_body {
                collect_uppercase_idents_from_expr(body, out);
            }
        }
        Expr::For { collection, filter, body, .. } => {
            collect_uppercase_idents_from_expr(collection, out);
            if let Some(f) = filter {
                collect_uppercase_idents_from_expr(f, out);
            }
            collect_uppercase_idents_from_expr(body, out);
        }
        Expr::Lambda { body, .. } => {
            collect_uppercase_idents_from_expr(body, out);
        }
        Expr::TransitionsTo { subject, new_state, .. }
        | Expr::Becomes { subject, new_state, .. } => {
            collect_uppercase_idents_from_expr(subject, out);
            collect_uppercase_idents_from_expr(new_state, out);
        }
        Expr::GenericType { name, args, .. } => {
            collect_uppercase_idents_from_expr(name, out);
            for a in args {
                collect_uppercase_idents_from_expr(a, out);
            }
        }
        Expr::SetLiteral { elements, .. } => {
            for e in elements {
                collect_uppercase_idents_from_expr(e, out);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for f in fields {
                collect_uppercase_idents_from_expr(&f.value, out);
            }
        }
        Expr::ProjectionMap { source, .. } => {
            collect_uppercase_idents_from_expr(source, out);
        }
        Expr::QualifiedName(q) => {
            out.insert(&q.name);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// 10. Deferred location hints
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_deferred_location_hints(&mut self) {
        for d in &self.module.declarations {
            let Decl::Deferred(def) = d else {
                continue;
            };
            // The TypeScript check looks for a string literal or URL on the deferred line.
            // Since the Rust parser only stores the path expression, we emit a warning
            // if there's no additional hint (the parser doesn't capture comments/URLs).
            self.push(
                Diagnostic::warning(
                    def.span,
                    format!(
                        "Deferred specification '{}' should include a location hint.",
                        expr_to_dotpath(&def.path),
                    ),
                )
                .with_code("allium.deferred.missingLocationHint"),
            );
        }
    }
}

fn expr_to_dotpath(expr: &Expr) -> String {
    match expr {
        Expr::Ident(id) => id.name.clone(),
        Expr::MemberAccess { object, field, .. } => {
            format!("{}.{}", expr_to_dotpath(object), field.name)
        }
        _ => "?".to_string(),
    }
}

// ---------------------------------------------------------------------------
// 11. Invalid triggers
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_rule_invalid_triggers(&mut self) {
        for rule in self.blocks(BlockKind::Rule) {
            let rule_name = match &rule.name {
                Some(n) => &n.name,
                None => continue,
            };

            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "when" {
                    continue;
                }
                if !is_valid_trigger(value) {
                    self.push(
                        Diagnostic::error(
                            item.span,
                            format!(
                                "Rule '{rule_name}' uses an unsupported trigger form in 'when:'.",
                            ),
                        )
                        .with_code("allium.rule.invalidTrigger"),
                    );
                }
            }
        }
    }
}

fn is_valid_trigger(expr: &Expr) -> bool {
    match expr {
        // EventName(params...) — external stimulus trigger
        Expr::Call { function, .. } => {
            matches!(function.as_ref(), Expr::Ident(_) | Expr::MemberAccess { .. })
        }
        // binding: Entity.field becomes/transitions_to state
        Expr::Binding { .. } => true,
        // a or b — combined triggers
        Expr::LogicalOp {
            op: LogicalOp::Or,
            left,
            right,
            ..
        } => is_valid_trigger(left) && is_valid_trigger(right),
        // Temporal: Entity.field <= now, Entity.field comparison ...
        Expr::Comparison { left, .. } => {
            matches!(left.as_ref(), Expr::MemberAccess { .. })
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// 12. Undefined rule bindings
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_rule_undefined_bindings(&mut self) {
        // Collect context bindings from given blocks
        let mut given_bindings: HashSet<&str> = HashSet::new();
        for given in self.blocks(BlockKind::Given) {
            for item in &given.items {
                if let BlockItemKind::Assignment { name, .. } = &item.kind {
                    given_bindings.insert(&name.name);
                }
            }
        }

        // Collect default instance names
        let mut default_names: HashSet<&str> = HashSet::new();
        for d in &self.module.declarations {
            if let Decl::Default(def) = d {
                default_names.insert(&def.name.name);
            }
        }

        for rule in self.blocks(BlockKind::Rule) {
            let rule_name = match &rule.name {
                Some(n) => &n.name,
                None => continue,
            };

            let mut bound: HashSet<&str> = HashSet::new();
            bound.extend(&given_bindings);
            bound.extend(&default_names);

            // Collect bindings from when clause
            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "when" {
                    continue;
                }
                collect_bound_names(value, &mut bound);
            }

            // Collect let bindings
            for item in &rule.items {
                if let BlockItemKind::Let { name, .. } = &item.kind {
                    bound.insert(&name.name);
                }
            }

            // Check requires/ensures for unbound references
            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "requires" && keyword != "ensures" {
                    continue;
                }
                check_unbound_roots(value, &bound, rule_name, &mut self.diagnostics);
            }

            // Check for-block and if-block items
            for item in &rule.items {
                match &item.kind {
                    BlockItemKind::ForBlock {
                        binding,
                        items,
                        ..
                    } => {
                        let mut inner_bound = bound.clone();
                        match binding {
                            ForBinding::Single(id) => { inner_bound.insert(&id.name); }
                            ForBinding::Destructured(ids, _) => {
                                for id in ids {
                                    inner_bound.insert(&id.name);
                                }
                            }
                        }
                        for sub_item in items {
                            if let BlockItemKind::Clause { keyword, value } = &sub_item.kind {
                                if keyword == "ensures" || keyword == "requires" {
                                    check_unbound_roots(value, &inner_bound, rule_name, &mut self.diagnostics);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn collect_bound_names<'a>(expr: &'a Expr, out: &mut HashSet<&'a str>) {
    match expr {
        Expr::Binding { name, .. } => {
            out.insert(&name.name);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                if let CallArg::Positional(Expr::Ident(id)) = arg {
                    out.insert(&id.name);
                }
            }
        }
        Expr::LogicalOp { left, right, .. } => {
            collect_bound_names(left, out);
            collect_bound_names(right, out);
        }
        _ => {}
    }
}

fn check_unbound_roots(
    expr: &Expr,
    bound: &HashSet<&str>,
    rule_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::MemberAccess { object, .. } => {
            if let Expr::Ident(id) = object.as_ref() {
                if !starts_uppercase(&id.name)
                    && !bound.contains(id.name.as_str())
                    && !is_builtin_name(&id.name)
                {
                    diagnostics.push(
                        Diagnostic::error(
                            id.span,
                            format!(
                                "Rule '{rule_name}' references '{}' but no matching binding exists in context, trigger params, default instances, or local lets.",
                                id.name
                            ),
                        )
                        .with_code("allium.rule.undefinedBinding"),
                    );
                }
            }
        }
        Expr::Comparison { left, right, .. } => {
            check_unbound_roots(left, bound, rule_name, diagnostics);
            check_unbound_roots(right, bound, rule_name, diagnostics);
        }
        Expr::LogicalOp { left, right, .. } => {
            check_unbound_roots(left, bound, rule_name, diagnostics);
            check_unbound_roots(right, bound, rule_name, diagnostics);
        }
        Expr::Block { items, .. } => {
            for item in items {
                check_unbound_roots(item, bound, rule_name, diagnostics);
            }
        }
        Expr::Call { function, args, .. } => {
            // Don't descend into function position for member access (Entity.method)
            if !matches!(function.as_ref(), Expr::MemberAccess { .. }) {
                check_unbound_roots(function, bound, rule_name, diagnostics);
            }
            // Collect lambda params from any arg — they scope over all args
            let mut call_bound = bound.clone();
            for a in args {
                if let CallArg::Positional(Expr::Lambda { param, .. }) = a {
                    if let Expr::Ident(id) = param.as_ref() {
                        call_bound.insert(id.name.as_str());
                    }
                }
            }
            for a in args {
                match a {
                    CallArg::Positional(Expr::Lambda { body, .. }) => {
                        check_unbound_roots(body, &call_bound, rule_name, diagnostics);
                    }
                    CallArg::Positional(e) => {
                        check_unbound_roots(e, &call_bound, rule_name, diagnostics);
                    }
                    CallArg::Named(n) => check_unbound_roots(&n.value, &call_bound, rule_name, diagnostics),
                }
            }
        }
        Expr::Not { operand, .. }
        | Expr::Exists { operand, .. }
        | Expr::NotExists { operand, .. } => {
            check_unbound_roots(operand, bound, rule_name, diagnostics);
        }
        Expr::In { element, collection, .. } | Expr::NotIn { element, collection, .. } => {
            check_unbound_roots(element, bound, rule_name, diagnostics);
            check_unbound_roots(collection, bound, rule_name, diagnostics);
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                check_unbound_roots(&b.condition, bound, rule_name, diagnostics);
                check_unbound_roots(&b.body, bound, rule_name, diagnostics);
            }
            if let Some(body) = else_body {
                check_unbound_roots(body, bound, rule_name, diagnostics);
            }
        }
        _ => {}
    }
}

fn is_builtin_name(name: &str) -> bool {
    matches!(name, "config" | "now" | "this" | "within" | "true" | "false" | "null")
}

// ---------------------------------------------------------------------------
// 13. Duplicate let bindings
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_duplicate_let_bindings(&mut self) {
        for rule in self.blocks(BlockKind::Rule) {
            let mut seen: HashMap<&str, Span> = HashMap::new();
            for item in &rule.items {
                if let BlockItemKind::Let { name, .. } = &item.kind {
                    if let Some(prev_span) = seen.get(name.name.as_str()) {
                        let _ = prev_span; // first occurrence
                        self.push(
                            Diagnostic::error(
                                name.span,
                                format!(
                                    "Duplicate let binding '{}' in this rule.",
                                    name.name
                                ),
                            )
                            .with_code("allium.let.duplicateBinding"),
                        );
                    } else {
                        seen.insert(&name.name, name.span);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 14. Config undefined references
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_config_undefined_references(&mut self) {
        let mut config_params: HashSet<&str> = HashSet::new();
        for config in self.blocks(BlockKind::Config) {
            for item in &config.items {
                if let BlockItemKind::Assignment { name, .. } = &item.kind {
                    config_params.insert(&name.name);
                }
            }
        }

        if config_params.is_empty() {
            return;
        }

        // Walk all expressions looking for config.field references
        for d in &self.module.declarations {
            let block = match d {
                Decl::Block(b) => b,
                _ => continue,
            };
            if block.kind == BlockKind::Config {
                continue;
            }
            for item in &block.items {
                self.check_config_refs_in_item(&item.kind, &config_params);
            }
        }
    }

    fn check_config_refs_in_item(&mut self, kind: &BlockItemKind, params: &HashSet<&str>) {
        match kind {
            BlockItemKind::Clause { value, .. }
            | BlockItemKind::Assignment { value, .. }
            | BlockItemKind::ParamAssignment { value, .. }
            | BlockItemKind::Let { value, .. }
            | BlockItemKind::FieldWithWhen { value, .. } => {
                self.check_config_refs_in_expr(value, params);
            }
            BlockItemKind::ForBlock { collection, filter, items, .. } => {
                self.check_config_refs_in_expr(collection, params);
                if let Some(f) = filter {
                    self.check_config_refs_in_expr(f, params);
                }
                for item in items {
                    self.check_config_refs_in_item(&item.kind, params);
                }
            }
            BlockItemKind::IfBlock { branches, else_items } => {
                for b in branches {
                    self.check_config_refs_in_expr(&b.condition, params);
                    for item in &b.items {
                        self.check_config_refs_in_item(&item.kind, params);
                    }
                }
                if let Some(items) = else_items {
                    for item in items {
                        self.check_config_refs_in_item(&item.kind, params);
                    }
                }
            }
            _ => {}
        }
    }

    fn check_config_refs_in_expr(&mut self, expr: &Expr, params: &HashSet<&str>) {
        match expr {
            Expr::MemberAccess { object, field, .. } => {
                if let Expr::Ident(id) = object.as_ref() {
                    if id.name == "config" && !params.contains(field.name.as_str()) {
                        self.push(
                            Diagnostic::error(
                                field.span,
                                format!(
                                    "Config reference 'config.{}' is not declared in any config block.",
                                    field.name
                                ),
                            )
                            .with_code("allium.config.undefinedReference"),
                        );
                        return;
                    }
                }
                self.check_config_refs_in_expr(object, params);
            }
            Expr::Call { function, args, .. } => {
                self.check_config_refs_in_expr(function, params);
                for a in args {
                    match a {
                        CallArg::Positional(e) => self.check_config_refs_in_expr(e, params),
                        CallArg::Named(n) => self.check_config_refs_in_expr(&n.value, params),
                    }
                }
            }
            Expr::BinaryOp { left, right, .. }
            | Expr::Comparison { left, right, .. }
            | Expr::LogicalOp { left, right, .. }
            | Expr::Pipe { left, right, .. }
            | Expr::NullCoalesce { left, right, .. } => {
                self.check_config_refs_in_expr(left, params);
                self.check_config_refs_in_expr(right, params);
            }
            Expr::Not { operand, .. }
            | Expr::Exists { operand, .. }
            | Expr::NotExists { operand, .. } => {
                self.check_config_refs_in_expr(operand, params);
            }
            Expr::Block { items, .. } => {
                for item in items {
                    self.check_config_refs_in_expr(item, params);
                }
            }
            Expr::Conditional { branches, else_body, .. } => {
                for b in branches {
                    self.check_config_refs_in_expr(&b.condition, params);
                    self.check_config_refs_in_expr(&b.body, params);
                }
                if let Some(body) = else_body {
                    self.check_config_refs_in_expr(body, params);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// 15. Surface unused paths
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn check_surface_unused_paths(&mut self) {
        // Collect all field access paths from rules
        let mut rule_paths: HashSet<String> = HashSet::new();
        for rule in self.blocks(BlockKind::Rule) {
            for item in &rule.items {
                collect_dotpaths_from_item(&item.kind, &mut rule_paths);
            }
        }

        for surface in self.blocks(BlockKind::Surface) {
            let surface_name = match &surface.name {
                Some(n) => &n.name,
                None => continue,
            };

            // Collect binding names
            let mut binding_names: HashSet<&str> = HashSet::new();
            for item in &surface.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword == "facing" || keyword == "context" {
                    if let Expr::Binding { name, .. } = value {
                        binding_names.insert(&name.name);
                    }
                }
            }

            for item in &surface.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "exposes" {
                    continue;
                }
                check_unused_surface_paths(
                    value,
                    &binding_names,
                    &rule_paths,
                    surface_name,
                    &mut self.diagnostics,
                );
            }
        }
    }
}

fn check_unused_surface_paths(
    expr: &Expr,
    binding_names: &HashSet<&str>,
    rule_paths: &HashSet<String>,
    surface_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::MemberAccess { object, field, .. } => {
            // Check if root is a binding
            if let Expr::Ident(root) = object.as_ref() {
                if binding_names.contains(root.name.as_str()) {
                    let path = format!("{}.{}", root.name, field.name);
                    // Check if any rule references this field
                    let field_used = rule_paths.iter().any(|p| p.ends_with(&format!(".{}", field.name)));
                    if !field_used {
                        diagnostics.push(
                            Diagnostic::info(
                                expr.span(),
                                format!(
                                    "Surface '{surface_name}' path '{path}' is not observed in rule field references.",
                                ),
                            )
                            .with_code("allium.surface.unusedPath"),
                        );
                    }
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                check_unused_surface_paths(item, binding_names, rule_paths, surface_name, diagnostics);
            }
        }
        _ => {}
    }
}

fn collect_dotpaths_from_item(kind: &BlockItemKind, out: &mut HashSet<String>) {
    match kind {
        BlockItemKind::Clause { value, .. }
        | BlockItemKind::Assignment { value, .. }
        | BlockItemKind::Let { value, .. }
        | BlockItemKind::FieldWithWhen { value, .. } => {
            collect_dotpaths_from_expr(value, out);
        }
        BlockItemKind::ForBlock {
            collection,
            filter,
            items,
            ..
        } => {
            collect_dotpaths_from_expr(collection, out);
            if let Some(f) = filter {
                collect_dotpaths_from_expr(f, out);
            }
            for item in items {
                collect_dotpaths_from_item(&item.kind, out);
            }
        }
        BlockItemKind::IfBlock {
            branches,
            else_items,
        } => {
            for b in branches {
                collect_dotpaths_from_expr(&b.condition, out);
                for item in &b.items {
                    collect_dotpaths_from_item(&item.kind, out);
                }
            }
            if let Some(items) = else_items {
                for item in items {
                    collect_dotpaths_from_item(&item.kind, out);
                }
            }
        }
        _ => {}
    }
}

fn collect_dotpaths_from_expr(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::MemberAccess { object, field, .. } => {
            out.insert(format!("{}.{}", expr_root_name(object).unwrap_or("?"), field.name));
            collect_dotpaths_from_expr(object, out);
        }
        Expr::Comparison { left, right, .. } => {
            collect_dotpaths_from_expr(left, out);
            collect_dotpaths_from_expr(right, out);
        }
        Expr::LogicalOp { left, right, .. } => {
            collect_dotpaths_from_expr(left, out);
            collect_dotpaths_from_expr(right, out);
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_dotpaths_from_expr(item, out);
            }
        }
        Expr::Call { function, args, .. } => {
            collect_dotpaths_from_expr(function, out);
            for a in args {
                match a {
                    CallArg::Positional(e) => collect_dotpaths_from_expr(e, out),
                    CallArg::Named(n) => {
                        // Named args in Entity.created(field: value) reference the field
                        out.insert(format!("_.{}", n.name.name));
                        collect_dotpaths_from_expr(&n.value, out);
                    }
                }
            }
        }
        Expr::In { element, collection, .. } | Expr::NotIn { element, collection, .. } => {
            collect_dotpaths_from_expr(element, out);
            collect_dotpaths_from_expr(collection, out);
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_dotpaths_from_expr(&b.condition, out);
                collect_dotpaths_from_expr(&b.body, out);
            }
            if let Some(body) = else_body {
                collect_dotpaths_from_expr(body, out);
            }
        }
        _ => {}
    }
}

fn expr_root_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(id) => Some(&id.name),
        Expr::MemberAccess { object, .. } => expr_root_name(object),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers: AST walking
// ---------------------------------------------------------------------------

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;
    use crate::parser::parse;

    fn analyze_src(src: &str) -> Vec<Diagnostic> {
        let input = if src.starts_with("-- allium:") {
            src.to_string()
        } else {
            format!("-- allium: 3\n{src}")
        };
        let result = parse(&input);
        analyze(&result.module, &input)
    }

    fn has_code(diagnostics: &[Diagnostic], code: &str) -> bool {
        diagnostics.iter().any(|d| d.code == Some(code))
    }

    fn count_code(diagnostics: &[Diagnostic], code: &str) -> usize {
        diagnostics.iter().filter(|d| d.code == Some(code)).count()
    }

    // -- Suppression --

    #[test]
    fn suppression_on_previous_line() {
        let ds = analyze_src("entity A {\n  -- allium-ignore allium.field.unused\n  x: String\n}\n");
        assert!(!has_code(&ds, "allium.field.unused"));
    }

    #[test]
    fn suppression_all() {
        let ds = analyze_src("entity A {\n  -- allium-ignore all\n  x: String\n}\n");
        assert!(!has_code(&ds, "allium.field.unused"));
    }

    // -- Related surface references --

    #[test]
    fn related_clause_with_binding_and_guard() {
        let ds = analyze_src(
            "surface QuoteVersions {\n  facing user: User\n}\n\n\
             surface Dashboard {\n  facing user: User\n  related:\n    QuoteVersions(quote) when quote.version_count > 1\n}\n",
        );
        assert!(!has_code(&ds, "allium.surface.relatedUndefined"));
    }

    #[test]
    fn related_clause_reports_unknown_surface() {
        let ds = analyze_src(
            "surface Dashboard {\n  facing user: User\n  related:\n    MissingSurface\n}\n",
        );
        assert!(has_code(&ds, "allium.surface.relatedUndefined"));
    }

    // -- Discriminator --

    #[test]
    fn v1_capitalised_inline_enum() {
        let ds = analyze_src("entity Quote {\n  status: Quoted | OrderSubmitted | Filled\n}\n");
        assert!(has_code(&ds, "allium.sum.v1InlineEnum"));
    }

    // -- Unused bindings --

    #[test]
    fn discard_binding_no_warning() {
        let ds = analyze_src(
            "surface QuoteFeed {\n  facing _: Service\n  exposes:\n    System.status\n}\n",
        );
        assert!(!has_code(&ds, "allium.surface.unusedBinding"));
    }

    // -- Status state machine --

    #[test]
    fn variable_status_assignment_suppresses_unreachable() {
        let ds = analyze_src(
            "entity Quote {\n  status: pending | quoted | filled\n}\n\n\
             rule ApplyStatusUpdate {\n  when: update: Quote.status becomes pending\n  \
             ensures: update.status = new_status\n}\n",
        );
        assert!(!has_code(&ds, "allium.status.unreachableValue"));
        assert!(!has_code(&ds, "allium.status.noExit"));
    }

    // -- External entity --

    #[test]
    fn external_entity_referenced_in_rules_info() {
        let ds = analyze_src(
            "external entity Client {\n  id: String\n}\n\n\
             rule IngestQuote {\n  when: RawQuoteReceived(data)\n  ensures:\n    Client.lookup(data.client_id)\n}\n",
        );
        let hint = ds.iter().find(|d| d.code == Some("allium.externalEntity.missingSourceHint"));
        assert!(hint.is_some());
        assert_eq!(hint.unwrap().severity, Severity::Info);
    }

    // -- Type references --

    #[test]
    fn undefined_type_reference() {
        let ds = analyze_src("entity Foo {\n  bar: MissingType\n}\n");
        assert!(has_code(&ds, "allium.type.undefinedReference"));
    }

    #[test]
    fn known_type_reference_ok() {
        let ds = analyze_src("entity Foo {\n  bar: String\n}\n");
        assert!(!has_code(&ds, "allium.type.undefinedReference"));
    }

    // -- Unreachable triggers --

    #[test]
    fn unreachable_trigger_reported() {
        let ds = analyze_src(
            "rule A {\n  when: ExternalEvent(x)\n  ensures: Done()\n}\n",
        );
        assert!(has_code(&ds, "allium.rule.unreachableTrigger"));
    }

    // -- Unused fields --

    #[test]
    fn unused_field_reported() {
        let ds = analyze_src("entity A {\n  x: String\n  y: String\n}\n\nrule R {\n  when: Ping(a)\n  ensures: a.x = \"hi\"\n}\n");
        assert!(has_code(&ds, "allium.field.unused"));
        // y is unused, x is used
        let unused: Vec<_> = ds.iter().filter(|d| d.code == Some("allium.field.unused")).collect();
        assert!(unused.iter().any(|d| d.message.contains("A.y")));
        assert!(!unused.iter().any(|d| d.message.contains("A.x")));
    }

    // -- Unused entities --

    #[test]
    fn unused_entity_reported() {
        let ds = analyze_src("entity Orphan {\n  x: String\n}\n");
        assert!(has_code(&ds, "allium.entity.unused"));
    }

    // -- Deferred location hints --

    #[test]
    fn deferred_missing_location_hint() {
        let ds = analyze_src("deferred Foo.bar\n");
        assert!(has_code(&ds, "allium.deferred.missingLocationHint"));
    }

    // -- Invalid triggers --

    #[test]
    fn valid_trigger_ok() {
        let ds = analyze_src("rule A {\n  when: Ping(x)\n  ensures: Done()\n}\n");
        assert!(!has_code(&ds, "allium.rule.invalidTrigger"));
    }

    // -- Duplicate let --

    #[test]
    fn duplicate_let_binding() {
        let ds = analyze_src(
            "rule A {\n  when: Ping(x)\n  let a = 1\n  let a = 2\n  ensures: Done()\n}\n",
        );
        assert!(has_code(&ds, "allium.let.duplicateBinding"));
    }

    // -- Config references --

    #[test]
    fn config_undefined_reference() {
        let ds = analyze_src(
            "config {\n  max_retries: 3\n}\n\nrule A {\n  when: Ping(x)\n  requires: config.missing_param > 0\n  ensures: Done()\n}\n",
        );
        assert!(has_code(&ds, "allium.config.undefinedReference"));
    }

    #[test]
    fn config_valid_reference_ok() {
        let ds = analyze_src(
            "config {\n  max_retries: 3\n}\n\nrule A {\n  when: Ping(x)\n  requires: config.max_retries > 0\n  ensures: Done()\n}\n",
        );
        assert!(!has_code(&ds, "allium.config.undefinedReference"));
    }
}
