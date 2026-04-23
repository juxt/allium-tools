//! Semantic analysis pass over the parsed AST.
//!
//! The parser produces a syntactic AST and catches structural errors.
//! This module walks the AST to find semantic issues: undefined
//! references, unused bindings, state-machine gaps, and migration
//! hints.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diagnostic::{Diagnostic, Finding};
use crate::lexer::SourceMap;
use crate::Span;

/// Run structural checks on a parsed module (`allium check`).
/// Returns line-level diagnostics only.
pub fn analyze(module: &Module, source: &str) -> Vec<Diagnostic> {
    let mut ctx = Ctx::new(module);

    ctx.check_related_surface_references();
    ctx.check_discriminator_variants();
    ctx.check_surface_binding_usage();
    ctx.check_status_state_machine();
    ctx.check_external_entity_source_hints();
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

    apply_suppressions(ctx.diagnostics, source)
}

/// Run structural checks plus process-level analysis (`allium analyse`).
/// Returns diagnostics and typed findings with evidence.
pub fn analyse(module: &Module, source: &str) -> crate::diagnostic::AnalyseResult {
    let diagnostics = analyze(module, source);
    let findings = find_process_issues(module);
    crate::diagnostic::AnalyseResult {
        diagnostics,
        findings,
    }
}

/// Shared entity data collected once and used by all finding methods.
struct EntityInfo<'a> {
    /// entity name → (status values set, status value idents)
    status_values: HashMap<&'a str, (HashSet<&'a str>, Vec<&'a Ident>)>,
    /// entity name → (field name → referenced entity type name)
    field_types: HashMap<&'a str, HashMap<&'a str, &'a str>>,
    /// entity name → transition edge list [(from, to)]
    graph_edges: HashMap<&'a str, Vec<(&'a str, &'a str)>>,
    /// entity name → terminal state set
    terminals: HashMap<&'a str, HashSet<&'a str>>,
}

impl<'a> EntityInfo<'a> {
    fn from_module(module: &'a Module) -> Self {
        let mut status_values: HashMap<&str, (HashSet<&str>, Vec<&Ident>)> = HashMap::new();
        let mut field_types: HashMap<&str, HashMap<&str, &str>> = HashMap::new();
        let mut graph_edges: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        let mut terminals: HashMap<&str, HashSet<&str>> = HashMap::new();

        let entities = module.declarations.iter().filter_map(|d| match d {
            Decl::Block(b) if b.kind == BlockKind::Entity => Some(b),
            _ => None,
        });
        for entity in entities {
            let name = match &entity.name {
                Some(n) => n.name.as_str(),
                None => continue,
            };
            for item in &entity.items {
                match &item.kind {
                    BlockItemKind::Assignment { name: f, value } if f.name == "status" => {
                        let mut idents = Vec::new();
                        collect_pipe_idents(value, &mut idents);
                        if idents.len() >= 2
                            && !idents.iter().any(|id| starts_uppercase(&id.name))
                        {
                            let set: HashSet<&str> =
                                idents.iter().map(|id| id.name.as_str()).collect();
                            status_values.insert(name, (set, idents));
                        }
                    }
                    BlockItemKind::Assignment { name: f, value } => {
                        if let Some(t) = extract_field_entity_type(value) {
                            field_types.entry(name).or_default().insert(f.name.as_str(), t);
                        }
                    }
                    BlockItemKind::TransitionsBlock(graph) => {
                        let edges: Vec<(&str, &str)> = graph
                            .edges
                            .iter()
                            .map(|e| (e.from.name.as_str(), e.to.name.as_str()))
                            .collect();
                        graph_edges.insert(name, edges);
                        let terms: HashSet<&str> =
                            graph.terminal.iter().map(|t| t.name.as_str()).collect();
                        if !terms.is_empty() {
                            terminals.insert(name, terms);
                        }
                    }
                    _ => {}
                }
            }
        }

        Self { status_values, field_types, graph_edges, terminals }
    }

    /// Status values as a simple entity → set map (without idents).
    fn status_by_entity(&self) -> HashMap<&'a str, HashSet<&'a str>> {
        self.status_values
            .iter()
            .map(|(k, (set, _))| (*k, set.clone()))
            .collect()
    }
}

/// Compute process-level findings: data flow, reachability, conflicts, invariants.
fn find_process_issues(module: &Module) -> Vec<crate::diagnostic::Finding> {
    let mut ctx = Ctx::new(module);
    let info = EntityInfo::from_module(module);
    ctx.collect_process_findings(&info);
    ctx.collect_conflict_findings(&info);
    ctx.collect_invariant_findings(&info);
    std::mem::take(&mut ctx.findings)
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
    findings: Vec<crate::diagnostic::Finding>,
}

impl<'a> Ctx<'a> {
    fn new(module: &'a Module) -> Self {
        Self {
            module,
            diagnostics: Vec::new(),
            findings: Vec::new(),
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

    fn push_finding(&mut self, finding: Finding) {
        self.findings.push(finding);
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
            let base_name = expr_as_ident(&v.base).or_else(|| {
                // Parser may represent `variant X : Base { ... }` as JoinLookup
                if let Expr::JoinLookup { entity, .. } = &v.base {
                    expr_as_ident(entity)
                } else {
                    None
                }
            });
            if let Some(base_name) = base_name {
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

            // Only check facing bindings for unused if surface has provides
            let has_provides = surface
                .items
                .iter()
                .any(|i| matches!(&i.kind, BlockItemKind::Clause { keyword, .. } if keyword == "provides"));

            let mut bindings: Vec<(&str, Span, bool)> = Vec::new(); // name, span, is_facing
            for item in &surface.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "facing" && keyword != "context" {
                    continue;
                }
                if let Expr::Binding { name, .. } = value {
                    bindings.push((&name.name, name.span, keyword == "facing"));
                }
            }

            for (name, span, is_facing) in &bindings {
                if *name == "_" {
                    continue;
                }
                // Facing bindings are only meaningful in surfaces with provides
                if *is_facing && !has_provides {
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
        let mut terminal_by_entity: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut has_transitions: HashSet<&str> = HashSet::new();
        let mut declared_edges: HashMap<&str, HashSet<(&str, &str)>> = HashMap::new();
        let mut field_entity_types: HashMap<&str, HashMap<&str, &str>> = HashMap::new();
        for entity in self.blocks(BlockKind::Entity) {
            let entity_name = match &entity.name {
                Some(n) => n.name.as_str(),
                None => continue,
            };
            for item in &entity.items {
                match &item.kind {
                    BlockItemKind::Assignment { name, value } if name.name == "status" => {
                        let mut idents = Vec::new();
                        collect_pipe_idents(value, &mut idents);
                        if idents.len() < 2 {
                            continue;
                        }
                        if idents.iter().any(|id| starts_uppercase(&id.name)) {
                            continue;
                        }
                        let set: HashSet<&str> =
                            idents.iter().map(|id| id.name.as_str()).collect();
                        status_by_entity.insert(entity_name, (idents, set));
                    }
                    BlockItemKind::Assignment { name, value } => {
                        // Collect field → entity type mappings for nested access
                        if let Some(type_name) = extract_field_entity_type(value) {
                            field_entity_types
                                .entry(entity_name)
                                .or_default()
                                .insert(name.name.as_str(), type_name);
                        }
                    }
                    BlockItemKind::TransitionsBlock(graph) => {
                        has_transitions.insert(entity_name);
                        let terminals: HashSet<&str> =
                            graph.terminal.iter().map(|t| t.name.as_str()).collect();
                        if !terminals.is_empty() {
                            terminal_by_entity.insert(entity_name, terminals);
                        }
                        let edges: HashSet<(&str, &str)> = graph
                            .edges
                            .iter()
                            .map(|e| (e.from.name.as_str(), e.to.name.as_str()))
                            .collect();
                        declared_edges.insert(entity_name, edges);
                    }
                    _ => {}
                }
            }
        }
        // Prune field_entity_types to only include fields whose type has a status enum
        for fields in field_entity_types.values_mut() {
            fields.retain(|_, type_name| status_by_entity.contains_key(type_name));
        }

        if status_by_entity.is_empty() {
            return;
        }

        let mut assigned_by_entity: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut transitions_by_entity: HashMap<&str, HashMap<&str, HashSet<&str>>> =
            HashMap::new();
        let mut created_issues: Vec<Diagnostic> = Vec::new();

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
                    &field_entity_types,
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
                    &field_entity_types,
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
                visit_created_calls(
                    value,
                    &status_by_entity,
                    &has_transitions,
                    &mut |entity, status| {
                        assigned_by_entity
                            .entry(entity)
                            .or_default()
                            .insert(status);
                    },
                    &mut created_issues,
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

                let is_terminal = terminal_by_entity
                    .get(entity_name)
                    .map_or_else(
                        || is_likely_terminal(&id.name),
                        |terminals| terminals.contains(id.name.as_str()),
                    );
                if is_terminal {
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

        // Check rule-produced transitions against declared graph edges
        for (entity_name, transition_map) in &transitions_by_entity {
            if let Some(edges) = declared_edges.get(entity_name) {
                if let Some((idents, _)) = status_by_entity.get(entity_name) {
                    for (from, targets) in transition_map {
                        for to in targets {
                            if from != to && !edges.contains(&(*from, *to)) {
                                // Find the span for the source status in the declaration
                                let span = idents
                                    .iter()
                                    .find(|id| id.name == *from)
                                    .map(|id| id.span)
                                    .unwrap_or(idents[0].span);
                                self.push(
                                    Diagnostic::warning(
                                        span,
                                        format!(
                                            "Rule produces transition '{from}' → '{to}' on entity '{entity_name}', but this edge is not in the declared transition graph.",
                                        ),
                                    )
                                    .with_code("allium.status.undeclaredTransition"),
                                );
                            }
                        }
                    }
                }
            }
        }

        for issue in created_issues {
            self.push(issue);
        }
    }
}

// ---------------------------------------------------------------------------
// Finding-producing methods (parallel to the check_* methods above)
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn collect_process_findings(&mut self, info: &EntityInfo<'_>) {
        let status_values = &info.status_values;
        let field_types = &info.field_types;
        let graph_edges = &info.graph_edges;
        let terminals = &info.terminals;

        if status_values.is_empty() {
            return;
        }

        // 2. Collect triggers provided by surfaces (and surface names)
        let mut surface_triggers: HashSet<&str> = HashSet::new();
        let mut surface_names: Vec<String> = Vec::new();
        for surface in self.blocks(BlockKind::Surface) {
            if let Some(n) = &surface.name {
                surface_names.push(n.name.clone());
            }
            for item in &surface.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword == "provides" {
                    collect_call_names(value, &mut surface_triggers);
                }
            }
        }

        // 3. Collect emitted triggers from rule ensures
        let mut emitted_triggers: HashSet<&str> = HashSet::new();
        for rule in self.blocks(BlockKind::Rule) {
            for item in &rule.items {
                collect_emitted_trigger_from_item(&item.kind, &mut emitted_triggers);
            }
        }

        // 4. Collect per-rule info (with per-rule field assignments for searched evidence)
        let mut assigned_fields: HashSet<String> = HashSet::new();

        struct RuleData<'b> {
            name: &'b str,
            trigger_reachable: bool,
            requires_fields: Vec<(String, String, String)>,
            transitions: Vec<(String, String, String)>,
            field_assignments: HashSet<String>,
            entity_bindings: Vec<String>,
        }
        let mut rules: Vec<RuleData> = Vec::new();

        for rule in self.blocks(BlockKind::Rule) {
            let rule_name = match &rule.name {
                Some(n) => n.name.as_str(),
                None => continue,
            };
            let mut trigger_name: Option<&str> = None;
            let mut requires_statuses: HashMap<&str, HashSet<&str>> = HashMap::new();
            let mut requires_fields: Vec<(String, String, String)> = Vec::new();
            let mut ensures_statuses: Vec<(&str, &str)> = Vec::new();
            let mut rule_assigned: HashSet<String> = HashSet::new();

            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword == "when" {
                    let names = extract_trigger_names(value);
                    if let Some((name, _)) = names.first() {
                        trigger_name = Some(*name);
                    }
                }
            }

            let trigger_reachable = trigger_name.map_or(true, |t| {
                surface_triggers.contains(t) || emitted_triggers.contains(t)
            });

            let binding_types = collect_rule_binding_types(rule, &status_values_for_binding(&status_values));

            // Collect entity bindings for unreachable_trigger affected_entities
            let entity_bindings: Vec<String> = binding_types
                .values()
                .map(|v| v.to_string())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "requires" {
                    continue;
                }
                collect_requires_conditions(
                    value,
                    &binding_types,
                    status_values,
                    &mut |binding, field, val| {
                        if field == "status" {
                            requires_statuses
                                .entry(binding)
                                .or_default()
                                .insert(val);
                        } else {
                            let entity = resolve_binding_entity_from_status(
                                binding, None, &binding_types, &status_values,
                            );
                            if let Some(e) = entity {
                                requires_fields.push((
                                    e.to_string(),
                                    field.to_string(),
                                    val.to_string(),
                                ));
                            }
                        }
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
                collect_field_assignments(
                    value,
                    &binding_types,
                    &status_values,
                    &field_types,
                    &mut |entity, field, value| {
                        let key = format!("{entity}.{field}");
                        assigned_fields.insert(key.clone());
                        rule_assigned.insert(key);
                        if field == "status" && value != "_variable_" {
                            assigned_fields.insert(format!("{entity}.status.{value}"));
                        }
                    },
                );
                collect_ensures_status(
                    value,
                    &binding_types,
                    &status_values,
                    &field_types,
                    &mut |binding, target| {
                        ensures_statuses.push((binding, target));
                    },
                );
            }

            let mut transitions = Vec::new();
            for (binding, target) in &ensures_statuses {
                let entity = resolve_binding_entity_from_status(
                    binding,
                    Some(target),
                    &binding_types,
                    &status_values,
                );
                if let Some(e) = entity {
                    if let Some(sources) = requires_statuses.get(binding) {
                        for source in sources {
                            transitions.push((
                                e.to_string(),
                                source.to_string(),
                                target.to_string(),
                            ));
                        }
                    }
                }
            }

            rules.push(RuleData {
                name: rule_name,
                trigger_reachable,
                requires_fields,
                transitions,
                field_assignments: rule_assigned,
                entity_bindings,
            });
        }

        // Track .created() status fields as assigned (and per-rule created tracking)
        let mut created_fields: HashSet<String> = HashSet::new();
        for rule in self.blocks(BlockKind::Rule) {
            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword != "ensures" {
                    continue;
                }
                collect_created_field_assignments(value, &status_values, &mut assigned_fields);
                collect_created_field_assignments(value, &status_values, &mut created_fields);
            }
        }

        // Collect surface-provided fields
        let mut surface_provided_fields: HashSet<String> = HashSet::new();
        for surface in self.blocks(BlockKind::Surface) {
            for item in &surface.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                if keyword == "provides" {
                    collect_surface_provided_fields(value, &status_values, &mut surface_provided_fields);
                }
            }
        }

        // Helper: build searched evidence for a given entity.field
        let build_searched = |entity: &str, field: &str| -> Vec<serde_json::Value> {
            let key = format!("{entity}.{field}");
            let mut searched = Vec::new();

            // Check rule_ensures
            let matching_rule: Option<&RuleData> = rules.iter().find(|r| {
                r.field_assignments.contains(&key)
            });
            if let Some(r) = matching_rule {
                if !r.trigger_reachable {
                    searched.push(serde_json::json!({
                        "kind": "rule_ensures",
                        "found": r.name,
                        "but": "trigger has no providing surface"
                    }));
                } else {
                    searched.push(serde_json::json!({
                        "kind": "rule_ensures",
                        "found": r.name
                    }));
                }
            } else {
                searched.push(serde_json::json!({
                    "kind": "rule_ensures",
                    "found": false
                }));
            }

            // Check surface_provides
            searched.push(serde_json::json!({
                "kind": "surface_provides",
                "found": surface_provided_fields.contains(&key)
            }));

            // Check created_calls
            searched.push(serde_json::json!({
                "kind": "created_calls",
                "found": created_fields.contains(&key)
            }));

            searched
        };

        // Dead transition findings
        for (entity, edges) in graph_edges {
            let _statuses = match status_values.get(entity) {
                Some(v) => v,
                None => continue,
            };

            for (from, to) in edges {
                let witnesses: Vec<&RuleData> = rules
                    .iter()
                    .filter(|r| {
                        r.transitions
                            .iter()
                            .any(|(e, f, t)| e == *entity && f == *from && t == *to)
                    })
                    .collect();

                if witnesses.is_empty() {
                    continue;
                }

                let any_achievable = witnesses.iter().any(|r| {
                    r.requires_fields.iter().all(|(e, f, _v)| {
                        assigned_fields.contains(&format!("{e}.{f}"))
                    })
                });

                if !any_achievable {
                    let witness_names: Vec<String> =
                        witnesses.iter().map(|r| r.name.to_string()).collect();
                    let unsatisfiable: Vec<serde_json::Value> = witnesses
                        .iter()
                        .flat_map(|r| {
                            r.requires_fields.iter().filter(|(e, f, _)| {
                                !assigned_fields.contains(&format!("{e}.{f}"))
                            })
                        })
                        .map(|(e, f, v)| {
                            serde_json::json!({
                                "entity": e,
                                "field": f,
                                "value": v,
                                "searched": build_searched(e, f),
                            })
                        })
                        .collect();

                    self.push_finding(serde_json::json!({
                        "type": "dead_transition",
                        "summary": format!(
                            "Transition '{from}' → '{to}' on entity '{entity}' is declared but unachievable"
                        ),
                        "edge": {"entity": entity, "from": from, "to": to},
                        "witnessing_rules": witness_names,
                        "unsatisfiable_requires": unsatisfiable,
                        "affected_entities": [entity],
                    }));
                }
            }
        }

        // Missing producer findings
        for r in &rules {
            for (entity, field, value) in &r.requires_fields {
                let key = format!("{entity}.{field}");
                if !assigned_fields.contains(&key) {
                    self.push_finding(serde_json::json!({
                        "type": "missing_producer",
                        "summary": format!("Nothing establishes {entity}.{field} = {value}"),
                        "requires": {"rule": r.name, "field": field, "value": value},
                        "searched": build_searched(entity, field),
                        "affected_entities": [entity],
                    }));
                }
            }
        }

        // Deadlock findings
        for (entity, edges) in graph_edges {
            let entity_terminals = match terminals.get(entity) {
                Some(t) => t,
                None => continue,
            };
            let (statuses, _idents) = match status_values.get(entity) {
                Some(v) => v,
                None => continue,
            };

            let achievable_edges: HashSet<(&str, &str)> = edges
                .iter()
                .filter(|(_from, to)| {
                    let producers: Vec<&RuleData> = rules
                        .iter()
                        .filter(|r| {
                            r.transitions
                                .iter()
                                .any(|(e, _f, t)| e == *entity && t == *to)
                        })
                        .collect();
                    if producers.is_empty() {
                        return assigned_fields.contains(&format!("{entity}.status.{to}"));
                    }
                    producers.iter().any(|r| {
                        r.requires_fields.iter().all(|(e, f, _v)| {
                            assigned_fields.contains(&format!("{e}.{f}"))
                        })
                    })
                })
                .copied()
                .collect();

            for status in statuses {
                if entity_terminals.contains(status) {
                    continue;
                }
                let mut visited = HashSet::new();
                let mut queue = vec![*status];
                let mut found_terminal = false;
                while let Some(current) = queue.pop() {
                    if !visited.insert(current) {
                        continue;
                    }
                    if entity_terminals.contains(current) {
                        found_terminal = true;
                        break;
                    }
                    for (from, to) in &achievable_edges {
                        if *from == current {
                            queue.push(to);
                        }
                    }
                }
                if !found_terminal {
                    let has_inbound = achievable_edges
                        .iter()
                        .any(|(_, to)| *to == *status);

                    if has_inbound || statuses.len() <= 6 {
                        // Build outbound edges with per-edge reasons
                        let outbound: Vec<serde_json::Value> = edges
                            .iter()
                            .filter(|(f, _)| *f == *status)
                            .map(|(f, t)| {
                                let witness_rules: Vec<(&str, &[(String, String, String)])> =
                                    rules
                                        .iter()
                                        .filter(|r| {
                                            r.transitions.iter().any(|(e, _ef, et)| {
                                                e == *entity && et == *t
                                            })
                                        })
                                        .map(|r| {
                                            (r.name, r.requires_fields.as_slice())
                                        })
                                        .collect();
                                let reason = edge_blocked_reason(
                                    &witness_rules, &assigned_fields,
                                );
                                serde_json::json!({
                                    "from": f,
                                    "to": t,
                                    "reason": reason,
                                })
                            })
                            .collect();

                        // Detect cycles via DFS through achievable edges
                        let cycle = detect_cycle(*status, &achievable_edges);

                        self.push_finding(serde_json::json!({
                            "type": "deadlock",
                            "summary": format!(
                                "Entity '{entity}' can reach state '{status}' but has no achievable path to any terminal state"
                            ),
                            "state": status,
                            "outbound_edges": outbound,
                            "cycle": cycle,
                            "affected_entities": [entity],
                        }));
                    }
                }
            }
        }

        // Unreachable trigger findings — aggregate per trigger
        let mut unreachable_by_trigger: HashMap<&str, Vec<(&str, Vec<String>)>> = HashMap::new();
        for rule in self.blocks(BlockKind::Rule) {
            let rule_name = match &rule.name {
                Some(n) => n.name.as_str(),
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
                for (name, _span) in trigger_names {
                    if !surface_triggers.contains(name) && !emitted_triggers.contains(name) {
                        // Find entity bindings for this rule
                        let rule_data = rules.iter().find(|r| r.name == rule_name);
                        let bindings = rule_data
                            .map(|r| r.entity_bindings.clone())
                            .unwrap_or_default();
                        unreachable_by_trigger
                            .entry(name)
                            .or_default()
                            .push((rule_name, bindings));
                    }
                }
            }
        }
        for (trigger, rule_entries) in &unreachable_by_trigger {
            let listening_rules: Vec<&str> = rule_entries.iter().map(|(n, _)| *n).collect();
            let affected_entities: Vec<String> = rule_entries
                .iter()
                .flat_map(|(_, bindings)| bindings.iter().cloned())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            self.push_finding(serde_json::json!({
                "type": "unreachable_trigger",
                "summary": format!(
                    "Trigger '{trigger}' is not provided by any surface"
                ),
                "trigger": trigger,
                "listening_rules": listening_rules,
                "surfaces_checked": surface_names,
                "affected_entities": affected_entities,
            }));
        }
    }

    fn collect_conflict_findings(&mut self, info: &EntityInfo<'_>) {
        let status_by_entity = info.status_by_entity();

        if status_by_entity.is_empty() {
            return;
        }

        struct ConflictRule<'b> {
            name: &'b str,
            trigger_kind: ConflictTriggerKind<'b>,
            requires_statuses: HashMap<String, HashSet<String>>,
            ensures_statuses: HashMap<String, String>,
        }

        let mut conflict_rules: Vec<ConflictRule> = Vec::new();

        for rule in self.blocks(BlockKind::Rule) {
            let rule_name = match &rule.name {
                Some(n) => n.name.as_str(),
                None => continue,
            };
            // Conflict detection resolves entities by name matching against
            // status_by_entity, not through binding types from when clauses.
            let binding_types = collect_rule_binding_types(rule, &HashMap::new());

            let mut trigger_kind = ConflictTriggerKind::Unknown;
            let mut requires_statuses: HashMap<String, HashSet<String>> = HashMap::new();
            let mut ensures_statuses: HashMap<String, String> = HashMap::new();

            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                match keyword.as_str() {
                    "when" => {
                        trigger_kind = classify_trigger(value);
                    }
                    "requires" => {
                        collect_requires_statuses_for_conflict(
                            value,
                            &binding_types,
                            &status_by_entity,
                            &mut requires_statuses,
                        );
                    }
                    "ensures" => {
                        collect_ensures_statuses_for_conflict(
                            value,
                            &binding_types,
                            &status_by_entity,
                            &mut ensures_statuses,
                        );
                    }
                    _ => {}
                }
            }

            conflict_rules.push(ConflictRule {
                name: rule_name,
                trigger_kind,
                requires_statuses,
                ensures_statuses,
            });
        }

        // Pairwise comparison
        let mut reported: HashSet<(usize, usize)> = HashSet::new();
        for i in 0..conflict_rules.len() {
            for j in (i + 1)..conflict_rules.len() {
                let a = &conflict_rules[i];
                let b = &conflict_rules[j];

                if matches!(
                    (&a.trigger_kind, &b.trigger_kind),
                    (ConflictTriggerKind::Call(_), ConflictTriggerKind::Call(_))
                ) {
                    continue;
                }

                // Find the overlapping state for the finding
                let mut overlap_state: Option<(&str, &str)> = None;
                let mut compatible = false;
                for (entity, a_statuses) in &a.requires_statuses {
                    if let Some(b_statuses) = b.requires_statuses.get(entity) {
                        let intersection: Vec<&String> =
                            a_statuses.intersection(b_statuses).collect();
                        if !intersection.is_empty() {
                            compatible = true;
                            overlap_state = Some((entity.as_str(), intersection[0].as_str()));
                            break;
                        }
                    }
                }
                if !compatible {
                    continue;
                }

                for (entity, a_target) in &a.ensures_statuses {
                    if let Some(b_target) = b.ensures_statuses.get(entity) {
                        if a_target != b_target && !reported.contains(&(i, j)) {
                            reported.insert((i, j));
                            let state = overlap_state
                                .map(|(_, s)| s.to_string())
                                .unwrap_or_default();
                            let mut values = serde_json::Map::new();
                            values.insert(a.name.to_string(), serde_json::json!(a_target));
                            values.insert(b.name.to_string(), serde_json::json!(b_target));

                            self.push_finding(serde_json::json!({
                                "type": "conflict",
                                "summary": format!(
                                    "Rules '{}' and '{}' can both fire when entity '{entity}' is in state '{state}', setting status to conflicting values",
                                    a.name, b.name,
                                ),
                                "rule_a": a.name,
                                "rule_b": b.name,
                                "field": "status",
                                "state": state,
                                "values": values,
                                "affected_entities": [entity],
                            }));
                        }
                    }
                }
            }
        }
    }

    fn collect_invariant_findings(&mut self, info: &EntityInfo<'_>) {
        let status_by_entity = info.status_by_entity();
        let field_types = &info.field_types;

        struct RuleEffect<'b> {
            name: &'b str,
            status_sets: Vec<(String, String)>,
            field_sets: HashSet<String>,
            requires: Vec<(String, String, String)>,
        }

        let binding_map: HashMap<&str, (HashSet<&str>, Vec<&Ident>)> = status_by_entity
            .iter()
            .map(|(k, v)| (*k, (v.clone(), Vec::new())))
            .collect();
        let binding_map_for_types: HashMap<&str, (Vec<&Ident>, HashSet<&str>)> = status_by_entity
            .iter()
            .map(|(k, v)| (*k, (Vec::new(), v.clone())))
            .collect();
        let mut rule_effects: Vec<RuleEffect> = Vec::new();

        for rule in self.blocks(BlockKind::Rule) {
            let rule_name = match &rule.name {
                Some(n) => n.name.as_str(),
                None => continue,
            };
            let binding_types = collect_rule_binding_types(rule, &binding_map_for_types);
            let mut status_sets = Vec::new();
            let mut field_sets = HashSet::new();
            let mut requires = Vec::new();

            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else {
                    continue;
                };
                match keyword.as_str() {
                    "ensures" => {
                        collect_rule_effects(
                            value,
                            &binding_types,
                            &status_by_entity,
                            &field_types,
                            &mut status_sets,
                            &mut field_sets,
                        );
                    }
                    "requires" => {
                        collect_requires_conditions(
                            value,
                            &binding_types,
                            &binding_map,
                            &mut |binding, field, val| {
                                let entity = resolve_binding_entity(
                                    binding,
                                    None,
                                    &binding_types,
                                    &binding_map_for_types,
                                );
                                if let Some(e) = entity {
                                    requires.push((
                                        e.to_string(),
                                        field.to_string(),
                                        val.to_string(),
                                    ));
                                }
                            },
                        );
                    }
                    _ => {}
                }
            }

            rule_effects.push(RuleEffect {
                name: rule_name,
                status_sets,
                field_sets,
                requires,
            });
        }

        // Check top-level invariants
        for decl in &self.module.declarations {
            let Decl::Invariant(inv) = decl else {
                continue;
            };

            if let Some(pattern) = extract_uniqueness_invariant(&inv.body) {
                let key_entity_type: Option<&str> = status_by_entity
                    .keys()
                    .find_map(|entity_name| {
                        field_types
                            .get(entity_name)
                            .and_then(|fields| fields.get(pattern.key_field).copied())
                    });

                for effect in &rule_effects {
                    for (entity, target) in &effect.status_sets {
                        if target == pattern.prohibited_status {
                            let has_guard = key_entity_type.map_or(false, |ket| {
                                effect.field_sets.iter().any(|f| {
                                    f.starts_with(&format!("{ket}."))
                                }) || effect.requires.iter().any(|(e, _f, _v)| {
                                    e == ket
                                })
                            });

                            if !has_guard {
                                let needed = format!(
                                    "Rule should set {}.status to prevent concurrent {} states",
                                    key_entity_type.unwrap_or("related entity"),
                                    pattern.prohibited_status,
                                );
                                self.push_finding(serde_json::json!({
                                    "type": "invariant_risk",
                                    "summary": format!(
                                        "Rule '{}' could violate invariant '{}'",
                                        effect.name, inv.name.name,
                                    ),
                                    "rule": effect.name,
                                    "invariant": inv.name.name,
                                    "mechanism": format!(
                                        "Sets {entity}.status to '{target}' without preventing concurrent instances"
                                    ),
                                    "guard_analysis": {
                                        "has_guard": false,
                                        "needed": needed,
                                    },
                                    "affected_entities": [entity],
                                }));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Compute a human-readable reason why a graph edge is blocked.
///
/// `witness_rules` contains `(rule_name, requires_fields)` for each rule
/// that witnesses the transition to `to` on `entity`.
fn edge_blocked_reason(
    witness_rules: &[(&str, &[(String, String, String)])],
    assigned_fields: &HashSet<String>,
) -> String {
    if witness_rules.is_empty() {
        return "no witnessing rule".to_string();
    }

    for (name, requires_fields) in witness_rules {
        for (e, f, v) in *requires_fields {
            if !assigned_fields.contains(&format!("{e}.{f}")) {
                return format!(
                    "rule {name} requires {e}.{f} = {v}, never established",
                );
            }
        }
    }

    "no achievable witnessing rule".to_string()
}

/// Detect a cycle in the achievable-edge graph starting from `start`.
/// Returns the cycle as a list of states, or `None` if no cycle exists.
fn detect_cycle<'a>(
    start: &'a str,
    edges: &HashSet<(&'a str, &'a str)>,
) -> Option<Vec<&'a str>> {
    // DFS with back-edge detection
    let mut stack: Vec<(&str, Vec<&str>)> = vec![(start, vec![start])];
    let mut visited: HashSet<&str> = HashSet::new();

    while let Some((current, path)) = stack.pop() {
        if !visited.insert(current) {
            continue;
        }
        for (from, to) in edges {
            if *from != current {
                continue;
            }
            if let Some(pos) = path.iter().position(|s| *s == *to) {
                // Found a back-edge — extract the cycle
                let mut cycle: Vec<&str> = path[pos..].to_vec();
                cycle.push(to);
                return Some(cycle);
            }
            let mut next_path = path.clone();
            next_path.push(to);
            // Re-insert current so it can be visited on this new path
            visited.remove(to);
            stack.push((to, next_path));
        }
    }
    None
}

/// Collect fields that surfaces provide via trigger call bindings.
fn collect_surface_provided_fields(
    expr: &Expr,
    status_values: &HashMap<&str, (HashSet<&str>, Vec<&Ident>)>,
    out: &mut HashSet<String>,
) {
    match expr {
        Expr::Call { function, args, .. } => {
            if let Expr::Ident(fn_name) = function.as_ref() {
                // Surface provides a trigger like TriggerName(entity) — the entity
                // bindings' fields are "provided" by this surface
                for arg in args {
                    if let CallArg::Positional(Expr::Ident(binding)) = arg {
                        // Check if this binding matches a known entity
                        if status_values.contains_key(binding.name.as_str()) {
                            // Mark all fields of this entity as surface-provided
                            out.insert(format!("{}.status", binding.name));
                        }
                    }
                    if let CallArg::Named(named) = arg {
                        if let Expr::Ident(val) = &named.value {
                            if status_values.contains_key(val.name.as_str()) {
                                out.insert(format!("{}.{}", val.name, named.name.name));
                            }
                        }
                    }
                }
                // Also just note that this trigger name is surface-provided
                let _ = fn_name;
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_surface_provided_fields(item, status_values, out);
            }
        }
        Expr::WhenGuard { action, .. } => {
            collect_surface_provided_fields(action, status_values, out);
        }
        Expr::Conditional { branches, else_body, .. } => {
            for b in branches {
                collect_surface_provided_fields(&b.body, status_values, out);
            }
            if let Some(body) = else_body {
                collect_surface_provided_fields(body, status_values, out);
            }
        }
        _ => {}
    }
}

/// Extract status assignments and field assignments from a rule's ensures clause.
fn collect_rule_effects(
    expr: &Expr,
    binding_types: &HashMap<&str, &str>,
    status_by_entity: &HashMap<&str, HashSet<&str>>,
    field_types: &HashMap<&str, HashMap<&str, &str>>,
    status_sets: &mut Vec<(String, String)>,
    field_sets: &mut HashSet<String>,
) {
    match expr {
        Expr::Comparison {
            left,
            op: ComparisonOp::Eq,
            right,
            ..
        } => {
            if let Some(target) = expr_as_ident(right) {
                if let Some((binding, field)) = expr_as_member_access(left) {
                    let entity = resolve_binding_entity(
                        binding,
                        if field == "status" { Some(target) } else { None },
                        binding_types,
                        &status_by_entity
                            .iter()
                            .map(|(k, v)| (*k, (Vec::new(), v.clone())))
                            .collect(),
                    );
                    if let Some(e) = entity {
                        if field == "status" {
                            status_sets.push((e.to_string(), target.to_string()));
                        }
                        field_sets.insert(format!("{e}.{field}"));
                    }
                }
                // Nested: binding.field.subfield = value
                if let Some((root, mid, field)) = expr_as_nested_member_access(left) {
                    let root_entity = resolve_binding_entity(
                        root,
                        None,
                        binding_types,
                        &status_by_entity
                            .iter()
                            .map(|(k, v)| (*k, (Vec::new(), v.clone())))
                            .collect(),
                    );
                    if let Some(re) = root_entity {
                        if let Some(nested) =
                            field_types.get(re).and_then(|f| f.get(mid).copied())
                        {
                            if field == "status" {
                                status_sets.push((nested.to_string(), target.to_string()));
                            }
                            field_sets.insert(format!("{nested}.{field}"));
                        }
                    }
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_rule_effects(
                    item, binding_types, status_by_entity, field_types, status_sets, field_sets,
                );
            }
        }
        Expr::Conditional {
            branches,
            else_body,
            ..
        } => {
            for branch in branches {
                collect_rule_effects(
                    &branch.body, binding_types, status_by_entity, field_types, status_sets,
                    field_sets,
                );
            }
            if let Some(body) = else_body {
                collect_rule_effects(
                    body, binding_types, status_by_entity, field_types, status_sets, field_sets,
                );
            }
        }
        _ => {}
    }
}

/// A uniqueness invariant pattern:
/// `for a in X: for b in X: a != b and a.key = b.key implies not (a.status = V and b.status = V)`
struct UniquenessPattern<'a> {
    prohibited_status: &'a str,
    key_field: &'a str,
}

/// Try to extract a uniqueness invariant pattern from an invariant body.
fn extract_uniqueness_invariant<'a>(expr: &'a Expr) -> Option<UniquenessPattern<'a>> {
    // Match: for a in X: for b in X: ... implies not (... and ...)
    let Expr::For { body, .. } = expr else {
        return None;
    };
    let Expr::For { body: inner_body, .. } = body.as_ref() else {
        return None;
    };

    // The inner body should be an implies expression
    let Expr::LogicalOp {
        op: LogicalOp::Implies,
        left: premise,
        right: conclusion,
        ..
    } = inner_body.as_ref()
    else {
        return None;
    };

    // The conclusion should be `not (a.status = V and b.status = V)`
    let Expr::Not { operand, .. } = conclusion.as_ref() else {
        return None;
    };

    // Extract the prohibited status from the negated conjunction
    let prohibited = extract_prohibited_status(operand)?;

    // Extract the key field from the premise (a.key = b.key)
    let key_field = extract_key_field(premise)?;

    Some(UniquenessPattern {
        prohibited_status: prohibited,
        key_field,
    })
}

/// Extract the prohibited status value from `a.status = V and b.status = V`.
fn extract_prohibited_status(expr: &Expr) -> Option<&str> {
    let Expr::LogicalOp {
        op: LogicalOp::And,
        left,
        right,
        ..
    } = expr
    else {
        return None;
    };

    // Both sides should be status comparisons with the same value
    let l_status = extract_status_value(left)?;
    let r_status = extract_status_value(right)?;

    if l_status == r_status {
        Some(l_status)
    } else {
        None
    }
}

fn extract_status_value(expr: &Expr) -> Option<&str> {
    if let Expr::Comparison {
        left,
        op: ComparisonOp::Eq,
        right,
        ..
    } = expr
    {
        if let Some((_, "status")) = expr_as_member_access(left) {
            return expr_as_ident(right);
        }
    }
    None
}

/// Extract the key entity type from `a != b and a.key = b.key`.
fn extract_key_field(expr: &Expr) -> Option<&str> {
    let Expr::LogicalOp {
        op: LogicalOp::And,
        left: _,
        right,
        ..
    } = expr
    else {
        return None;
    };

    // right should be a.key = b.key where key is a relationship to an entity
    if let Expr::Comparison {
        left,
        op: ComparisonOp::Eq,
        right: _,
        ..
    } = right.as_ref()
    {
        if let Some((_, field)) = expr_as_member_access(left) {
            // The field name is the relationship name. We need the entity type.
            // For simplicity, use the field name capitalised as the entity type.
            // A more robust approach would look up the field type.
            return Some(field);
        }
    }
    None
}

#[derive(PartialEq)]
enum ConflictTriggerKind<'a> {
    Call(&'a str),
    Temporal,
    Unknown,
}

fn classify_trigger(expr: &Expr) -> ConflictTriggerKind<'_> {
    match expr {
        Expr::Call { function, .. } => {
            if let Expr::Ident(id) = function.as_ref() {
                return ConflictTriggerKind::Call(&id.name);
            }
            ConflictTriggerKind::Unknown
        }
        Expr::Binding { value, .. } => classify_trigger(value),
        Expr::Comparison { .. }
        | Expr::Becomes { .. }
        | Expr::TransitionsTo { .. } => ConflictTriggerKind::Temporal,
        _ => ConflictTriggerKind::Unknown,
    }
}

fn collect_requires_statuses_for_conflict(
    expr: &Expr,
    binding_types: &HashMap<&str, &str>,
    status_by_entity: &HashMap<&str, HashSet<&str>>,
    out: &mut HashMap<String, HashSet<String>>,
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
                let entity = resolve_binding_entity(
                    binding,
                    Some(target),
                    binding_types,
                    &status_by_entity
                        .iter()
                        .map(|(k, v)| (*k, (Vec::new(), v.clone())))
                        .collect(),
                );
                if let Some(e) = entity {
                    out.entry(e.to_string()).or_default().insert(target.to_string());
                }
            }
        }
        Expr::LogicalOp { left, right, .. } => {
            collect_requires_statuses_for_conflict(left, binding_types, status_by_entity, out);
            collect_requires_statuses_for_conflict(right, binding_types, status_by_entity, out);
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_requires_statuses_for_conflict(item, binding_types, status_by_entity, out);
            }
        }
        _ => {}
    }
}

fn collect_ensures_statuses_for_conflict(
    expr: &Expr,
    binding_types: &HashMap<&str, &str>,
    status_by_entity: &HashMap<&str, HashSet<&str>>,
    out: &mut HashMap<String, String>,
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
                let entity = resolve_binding_entity(
                    binding,
                    Some(target),
                    binding_types,
                    &status_by_entity
                        .iter()
                        .map(|(k, v)| (*k, (Vec::new(), v.clone())))
                        .collect(),
                );
                if let Some(e) = entity {
                    out.insert(e.to_string(), target.to_string());
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_ensures_statuses_for_conflict(item, binding_types, status_by_entity, out);
            }
        }
        Expr::Conditional {
            branches,
            else_body,
            ..
        } => {
            for branch in branches {
                collect_ensures_statuses_for_conflict(
                    &branch.body, binding_types, status_by_entity, out,
                );
            }
            if let Some(body) = else_body {
                collect_ensures_statuses_for_conflict(body, binding_types, status_by_entity, out);
            }
        }
        _ => {}
    }
}

/// Convert status_values to the format expected by collect_rule_binding_types.
fn status_values_for_binding<'a>(
    status_values: &'a HashMap<&'a str, (HashSet<&'a str>, Vec<&'a Ident>)>,
) -> HashMap<&'a str, (Vec<&'a Ident>, HashSet<&'a str>)> {
    status_values
        .iter()
        .map(|(k, (set, idents))| (*k, (idents.clone(), set.clone())))
        .collect()
}

/// Resolve a binding to an entity name using binding_types, case-insensitive
/// match, and optionally target status inference.
fn resolve_binding_entity_from_status<'a>(
    binding: &str,
    target: Option<&str>,
    binding_types: &HashMap<&'a str, &'a str>,
    status_values: &HashMap<&'a str, (HashSet<&'a str>, Vec<&Ident>)>,
) -> Option<&'a str> {
    binding_types
        .get(binding)
        .copied()
        .or_else(|| {
            status_values
                .keys()
                .find(|name| name.eq_ignore_ascii_case(binding))
                .copied()
        })
        .or_else(|| {
            let target = target?;
            let mut candidates = status_values
                .iter()
                .filter(|(_, (values, _))| values.contains(target));
            let first = candidates.next()?;
            if candidates.next().is_none() {
                Some(first.0)
            } else {
                None
            }
        })
}

/// Collect requires conditions from a requires expression.
fn collect_requires_conditions<'a>(
    expr: &'a Expr,
    binding_types: &HashMap<&'a str, &'a str>,
    status_values: &HashMap<&str, (HashSet<&str>, Vec<&Ident>)>,
    cb: &mut impl FnMut(&'a str, &'a str, &'a str),
) {
    match expr {
        Expr::Comparison {
            left,
            op: ComparisonOp::Eq,
            right,
            ..
        } => {
            if let Some(target) = expr_as_ident(right) {
                if let Some((binding, field)) = expr_as_member_access(left) {
                    cb(binding, field, target);
                } else if let Some((root, _mid, field)) =
                    expr_as_nested_member_access(left)
                {
                    if field == "status" {
                        cb(root, "status", target);
                    }
                }
            }
            // Also handle literal true/false on the right
            if let Expr::BoolLiteral { value: true, .. } = right.as_ref() {
                if let Some((binding, field)) = expr_as_member_access(left) {
                    cb(binding, field, "true");
                }
            }
        }
        Expr::Comparison {
            op: ComparisonOp::GtEq,
            ..
        } => {
            // Comparisons like balance >= amount are not field-value conditions
        }
        Expr::LogicalOp { left, right, .. } => {
            collect_requires_conditions(left, binding_types, status_values, cb);
            collect_requires_conditions(right, binding_types, status_values, cb);
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_requires_conditions(item, binding_types, status_values, cb);
            }
        }
        _ => {}
    }
}

/// Collect field assignments from ensures expressions.
fn collect_field_assignments<'a>(
    expr: &'a Expr,
    binding_types: &HashMap<&'a str, &'a str>,
    status_values: &HashMap<&str, (HashSet<&str>, Vec<&Ident>)>,
    field_types: &HashMap<&str, HashMap<&str, &str>>,
    cb: &mut impl FnMut(&str, &str, &str),
) {
    match expr {
        Expr::Comparison {
            left,
            op: ComparisonOp::Eq,
            right,
            ..
        } => {
            if let Some((binding, field)) = expr_as_member_access(left) {
                let entity = resolve_binding_entity_from_status(
                    binding, None, binding_types, status_values,
                );
                if let Some(entity) = entity {
                    let val = expr_as_ident(right).unwrap_or("_variable_");
                    cb(entity, field, val);
                }
            }
            // Nested: binding.field.subfield = value
            if let Some((root, mid, field)) = expr_as_nested_member_access(left) {
                let root_entity = resolve_binding_entity_from_status(
                    root, None, binding_types, status_values,
                );
                if let Some(root_entity) = root_entity {
                    if let Some(nested) =
                        field_types.get(root_entity).and_then(|f| f.get(mid).copied())
                    {
                        let val = expr_as_ident(right).unwrap_or("_variable_");
                        cb(nested, field, val);
                    }
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_field_assignments(item, binding_types, status_values, field_types, cb);
            }
        }
        Expr::Conditional {
            branches,
            else_body,
            ..
        } => {
            for branch in branches {
                collect_field_assignments(
                    &branch.body, binding_types, status_values, field_types, cb,
                );
            }
            if let Some(body) = else_body {
                collect_field_assignments(body, binding_types, status_values, field_types, cb);
            }
        }
        _ => {}
    }
}

/// Collect status assignments from ensures (simplified version for transition building).
fn collect_ensures_status<'a>(
    expr: &'a Expr,
    binding_types: &HashMap<&'a str, &'a str>,
    status_values: &HashMap<&str, (HashSet<&str>, Vec<&Ident>)>,
    field_types: &HashMap<&str, HashMap<&str, &str>>,
    cb: &mut impl FnMut(&'a str, &'a str),
) {
    match expr {
        Expr::Comparison {
            left,
            op: ComparisonOp::Eq,
            right,
            ..
        } => {
            if let Some(target) = expr_as_ident(right) {
                // Only track direct binding.status (not nested) to avoid
                // cross-contamination when root binding accesses different entities
                if let Some((binding, "status")) = expr_as_member_access(left) {
                    cb(binding, target);
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_ensures_status(item, binding_types, status_values, field_types, cb);
            }
        }
        Expr::Conditional {
            branches,
            else_body,
            ..
        } => {
            for branch in branches {
                collect_ensures_status(
                    &branch.body, binding_types, status_values, field_types, cb,
                );
            }
            if let Some(body) = else_body {
                collect_ensures_status(body, binding_types, status_values, field_types, cb);
            }
        }
        _ => {}
    }
}

/// Collect field assignments from .created() calls.
fn collect_created_field_assignments<'a>(
    expr: &'a Expr,
    status_values: &HashMap<&str, (HashSet<&str>, Vec<&Ident>)>,
    assigned: &mut HashSet<String>,
) {
    match expr {
        Expr::Call { function, args, .. } => {
            if let Expr::MemberAccess { object, field, .. } = function.as_ref() {
                if field.name == "created" {
                    if let Expr::Ident(entity_id) = object.as_ref() {
                        let entity = entity_id.name.as_str();
                        if status_values.contains_key(entity) {
                            for arg in args {
                                if let CallArg::Named(named) = arg {
                                    assigned.insert(format!(
                                        "{entity}.{}", named.name.name
                                    ));
                                    // Track per-value for status assignments
                                    if named.name.name == "status" {
                                        if let Expr::Ident(val) = &named.value {
                                            assigned.insert(format!(
                                                "{entity}.status.{}", val.name
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                collect_created_field_assignments(item, status_values, assigned);
            }
        }
        Expr::Conditional {
            branches,
            else_body,
            ..
        } => {
            for branch in branches {
                collect_created_field_assignments(&branch.body, status_values, assigned);
            }
            if let Some(body) = else_body {
                collect_created_field_assignments(body, status_values, assigned);
            }
        }
        _ => {}
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
    field_entity_types: &HashMap<&'a str, HashMap<&'a str, &'a str>>,
    cb: &mut impl FnMut(&'a str, &'a str, &'a str),
) {
    match expr {
        Expr::Comparison {
            left,
            op: ComparisonOp::Eq,
            right,
            ..
        } => {
            if let Some(target) = expr_as_ident(right) {
                // Direct: binding.status = value
                if let Some((binding, "status")) = expr_as_member_access(left) {
                    let entity = resolve_binding_entity(
                        binding,
                        Some(target),
                        binding_types,
                        status_by_entity,
                    );
                    if let Some(entity) = entity {
                        cb(binding, target, entity);
                    }
                }
                // Nested: binding.field.status = value
                // Only add to assigned_by_entity, NOT to transitions
                // (using root binding for transitions causes cross-contamination)
                else if let Some((root, field, "status")) =
                    expr_as_nested_member_access(left)
                {
                    let root_entity = resolve_binding_entity(
                        root, None, binding_types, status_by_entity,
                    );
                    if let Some(root_entity) = root_entity {
                        if let Some(nested_entity) = field_entity_types
                            .get(root_entity)
                            .and_then(|fields| fields.get(field).copied())
                        {
                            // Only track assignment, skip transition building
                            // by using a sentinel binding key
                            cb("_nested_", target, nested_entity);
                        }
                    }
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                visit_status_assignments(
                    item,
                    binding_types,
                    status_by_entity,
                    field_entity_types,
                    cb,
                );
            }
        }
        Expr::Conditional {
            branches,
            else_body,
            ..
        } => {
            for branch in branches {
                visit_status_assignments(
                    &branch.body,
                    binding_types,
                    status_by_entity,
                    field_entity_types,
                    cb,
                );
            }
            if let Some(body) = else_body {
                visit_status_assignments(
                    body,
                    binding_types,
                    status_by_entity,
                    field_entity_types,
                    cb,
                );
            }
        }
        _ => {}
    }
}

/// Walk an ensures expression tree looking for `Entity.created(status: value)` calls.
/// Adds valid status values to the assigned set via `on_status`. Collects diagnostics
/// for missing or invalid status arguments into `issues`.
fn visit_created_calls<'a>(
    expr: &'a Expr,
    status_by_entity: &HashMap<&'a str, (Vec<&Ident>, HashSet<&'a str>)>,
    has_transitions: &HashSet<&'a str>,
    on_status: &mut impl FnMut(&'a str, &'a str),
    issues: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Call {
            function, args, span, ..
        } => {
            if let Expr::MemberAccess { object, field, .. } = function.as_ref() {
                if field.name == "created" {
                    if let Expr::Ident(entity_ident) = object.as_ref() {
                        let entity_name = entity_ident.name.as_str();
                        if let Some((_, values)) = status_by_entity.get(entity_name) {
                            let status_arg = args.iter().find_map(|arg| {
                                if let CallArg::Named(named) = arg {
                                    if named.name.name == "status" {
                                        return Some(named);
                                    }
                                }
                                None
                            });

                            match status_arg {
                                Some(named) => {
                                    if let Expr::Ident(status_ident) = &named.value {
                                        let status = status_ident.name.as_str();
                                        if values.contains(status) {
                                            on_status(entity_name, status);
                                        } else {
                                            issues.push(
                                                Diagnostic::error(
                                                    named.value.span(),
                                                    format!(
                                                        ".created() on entity '{entity_name}' sets status to '{status}', which is not a declared status value.",
                                                    ),
                                                )
                                                .with_code("allium.created.invalidStatus"),
                                            );
                                        }
                                    }
                                }
                                None => {
                                    if has_transitions.contains(entity_name) {
                                        issues.push(
                                            Diagnostic::warning(
                                                *span,
                                                format!(
                                                    ".created() on entity '{entity_name}' omits the status field, but the entity has a transition graph. The initial state is unspecified.",
                                                ),
                                            )
                                            .with_code("allium.created.missingStatus"),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Expr::Block { items, .. } => {
            for item in items {
                visit_created_calls(item, status_by_entity, has_transitions, on_status, issues);
            }
        }
        Expr::Conditional {
            branches,
            else_body,
            ..
        } => {
            for branch in branches {
                visit_created_calls(
                    &branch.body,
                    status_by_entity,
                    has_transitions,
                    on_status,
                    issues,
                );
            }
            if let Some(body) = else_body {
                visit_created_calls(body, status_by_entity, has_transitions, on_status, issues);
            }
        }
        _ => {}
    }
}

fn visit_status_comparisons<'a>(
    expr: &'a Expr,
    binding_types: &HashMap<&'a str, &'a str>,
    status_by_entity: &HashMap<&'a str, (Vec<&Ident>, HashSet<&'a str>)>,
    field_entity_types: &HashMap<&'a str, HashMap<&'a str, &'a str>>,
    cb: &mut impl FnMut(&'a str, &'a str),
) {
    match expr {
        Expr::Comparison {
            left,
            op: ComparisonOp::Eq,
            right,
            ..
        } => {
            if let Some(target) = expr_as_ident(right) {
                // Direct: binding.status = value
                if let Some((binding, "status")) = expr_as_member_access(left) {
                    let known = resolve_binding_entity(
                        binding,
                        Some(target),
                        binding_types,
                        status_by_entity,
                    )
                    .is_some();
                    if known {
                        cb(binding, target);
                    }
                }
                // Nested patterns (binding.field.status) are NOT tracked for
                // transition building to avoid cross-contamination when the
                // same root binding accesses different entities. Nested
                // assignments are still tracked for reachability.
            }
        }
        Expr::LogicalOp { left, right, .. } => {
            visit_status_comparisons(left, binding_types, status_by_entity, field_entity_types, cb);
            visit_status_comparisons(right, binding_types, status_by_entity, field_entity_types, cb);
        }
        Expr::Block { items, .. } => {
            for item in items {
                visit_status_comparisons(item, binding_types, status_by_entity, field_entity_types, cb);
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

/// Extract `binding.field.last` from a double-level member access.
fn expr_as_nested_member_access(expr: &Expr) -> Option<(&str, &str, &str)> {
    if let Expr::MemberAccess {
        object, field: last, ..
    } = expr
    {
        if let Expr::MemberAccess {
            object: root_obj,
            field: mid,
            ..
        } = object.as_ref()
        {
            if let Expr::Ident(root) = root_obj.as_ref() {
                return Some((&root.name, &mid.name, &last.name));
            }
        }
    }
    None
}

/// Resolve a binding name to an entity name using available strategies:
/// 1. Explicit binding type from when clause
/// 2. Case-insensitive match against entity names
/// 3. Infer from target status value (if unique to one entity)
fn resolve_binding_entity<'a>(
    binding: &str,
    target: Option<&str>,
    binding_types: &HashMap<&'a str, &'a str>,
    status_by_entity: &HashMap<&'a str, (Vec<&Ident>, HashSet<&'a str>)>,
) -> Option<&'a str> {
    binding_types
        .get(binding)
        .copied()
        .or_else(|| {
            status_by_entity
                .keys()
                .find(|name| name.eq_ignore_ascii_case(binding))
                .copied()
        })
        .or_else(|| {
            // Infer from target status: if the value belongs to exactly one entity, use it
            let target = target?;
            let mut candidates = status_by_entity
                .iter()
                .filter(|(_, (_, values))| values.contains(target));
            let first = candidates.next()?;
            if candidates.next().is_none() {
                Some(first.0)
            } else {
                None
            }
        })
}

/// Extract the entity type name from a field declaration value.
/// Handles `Payment`, `InterviewSlot with candidacy = this`, etc.
fn extract_field_entity_type(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(id) if starts_uppercase(&id.name) => Some(&id.name),
        Expr::JoinLookup { entity, .. } => {
            if let Expr::Ident(id) = entity.as_ref() {
                if starts_uppercase(&id.name) {
                    return Some(&id.name);
                }
            }
            None
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
            // Entity.created or Entity.field (in binding triggers)
            Expr::MemberAccess { object, .. } => {
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

        // Collect triggers emitted by rule ensures clauses.
        // Only collect the leading call in each ensures value, matching the
        // TS regex which captures only the first identifier after `ensures:`.
        let mut emitted: HashSet<&str> = HashSet::new();
        for rule in self.blocks(BlockKind::Rule) {
            for item in &rule.items {
                collect_emitted_trigger_from_item(&item.kind, &mut emitted);
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

/// Collect emitted triggers from block items, only looking at ensures clauses
/// and recursing into for/if blocks for nested ensures.
fn collect_emitted_trigger_from_item<'a>(kind: &'a BlockItemKind, out: &mut HashSet<&'a str>) {
    match kind {
        BlockItemKind::Clause { keyword, value } if keyword == "ensures" => {
            collect_leading_ensures_call(value, out);
        }
        BlockItemKind::ForBlock { items, .. } => {
            for item in items {
                collect_emitted_trigger_from_item(&item.kind, out);
            }
        }
        BlockItemKind::IfBlock { branches, else_items, .. } => {
            for b in branches {
                for item in &b.items {
                    collect_emitted_trigger_from_item(&item.kind, out);
                }
            }
            if let Some(items) = else_items {
                for item in items {
                    collect_emitted_trigger_from_item(&item.kind, out);
                }
            }
        }
        _ => {}
    }
}

/// Extract only the leading PascalCase call from an ensures expression,
/// matching the TS regex which captures only the first identifier followed
/// by `(` after `ensures:`.
fn collect_leading_ensures_call<'a>(expr: &'a Expr, out: &mut HashSet<&'a str>) {
    match expr {
        Expr::Call { function, .. } => {
            if let Expr::Ident(id) = function.as_ref() {
                if starts_uppercase(&id.name) {
                    out.insert(&id.name);
                }
            }
        }
        Expr::Block { items, .. } => {
            if let Some(first) = items.first() {
                collect_leading_ensures_call(first, out);
            }
        }
        _ => {}
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
                        BlockKind::Entity | BlockKind::ExternalEntity
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
        let mut all_idents = self.collect_all_referenced_idents();
        // Entities that serve as variant bases are "used"
        for v in self.variants() {
            let base = expr_as_ident(&v.base).or_else(|| {
                if let Expr::JoinLookup { entity, .. } = &v.base {
                    expr_as_ident(entity)
                } else {
                    None
                }
            });
            if let Some(name) = base {
                all_idents.insert(name);
            }
        }
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
        // binding: Entity.field becomes/transitions_to/created/comparison
        Expr::Binding { value, .. } => {
            matches!(
                value.as_ref(),
                Expr::Becomes { .. }
                    | Expr::TransitionsTo { .. }
                    | Expr::MemberAccess { .. }
                    | Expr::Comparison { .. }
            )
        }
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

            // Rules with bare entity bindings (e.g. `when: state: ClerkEventState`)
            // have an invalid trigger form. The binding name is syntactically present
            // but doesn't resolve to a meaningful type. Flag the first usage.
            for item in &rule.items {
                let BlockItemKind::Clause { keyword, value } = &item.kind else { continue };
                if keyword != "when" { continue }
                let Expr::Binding { name: binding_name, value: trigger_value, .. } = value else { continue };
                if !matches!(trigger_value.as_ref(), Expr::Ident(id) if starts_uppercase(&id.name)) {
                    continue;
                }
                // Find the first requires/ensures clause that references this binding
                let mut found = false;
                for check_item in &rule.items {
                    let BlockItemKind::Clause { keyword: kw, value: v } = &check_item.kind else { continue };
                    if kw != "requires" && kw != "ensures" { continue }
                    if expr_contains_ident(v, &binding_name.name) {
                        self.push(
                            Diagnostic::error(
                                check_item.span,
                                format!(
                                    "Rule '{rule_name}' references '{}' but no matching binding exists in context, trigger params, default instances, or local lets.",
                                    binding_name.name
                                ),
                            )
                            .with_code("allium.rule.undefinedBinding"),
                        );
                        found = true;
                        break;
                    }
                }
                if found { break; }
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
            let mut block_bound = bound.clone();
            for item in items {
                if let Expr::LetExpr { name, value, .. } = item {
                    check_unbound_roots(value, &block_bound, rule_name, diagnostics);
                    block_bound.insert(name.name.as_str());
                } else {
                    check_unbound_roots(item, &block_bound, rule_name, diagnostics);
                }
            }
        }
        Expr::For { binding, collection, body, .. } => {
            check_unbound_roots(collection, bound, rule_name, diagnostics);
            // Skip filter (where clause) — fields are implicitly scoped to the binding
            let mut inner = bound.clone();
            match binding {
                ForBinding::Single(id) => { inner.insert(id.name.as_str()); }
                ForBinding::Destructured(ids, _) => {
                    for id in ids {
                        inner.insert(id.name.as_str());
                    }
                }
            }
            check_unbound_roots(body, &inner, rule_name, diagnostics);
        }
        Expr::BinaryOp { left, right, .. } => {
            check_unbound_roots(left, bound, rule_name, diagnostics);
            check_unbound_roots(right, bound, rule_name, diagnostics);
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
            self.check_duplicate_lets_in_items(&rule.items, &mut seen);
        }
    }

    fn check_duplicate_lets_in_items<'b>(
        &mut self,
        items: &'b [BlockItem],
        seen: &mut HashMap<&'b str, Span>,
    ) {
        for item in items {
            match &item.kind {
                BlockItemKind::Let { name, .. } => {
                    if seen.contains_key(name.name.as_str()) {
                        self.push(
                            Diagnostic::error(
                                name.span,
                                format!("Duplicate let binding '{}' in this rule.", name.name),
                            )
                            .with_code("allium.let.duplicateBinding"),
                        );
                    } else {
                        seen.insert(&name.name, name.span);
                    }
                }
                BlockItemKind::ForBlock { items, .. } => {
                    self.check_duplicate_lets_in_items(items, seen);
                }
                BlockItemKind::IfBlock {
                    branches,
                    else_items,
                } => {
                    for b in branches {
                        self.check_duplicate_lets_in_items(&b.items, seen);
                    }
                    if let Some(items) = else_items {
                        self.check_duplicate_lets_in_items(items, seen);
                    }
                }
                BlockItemKind::Clause { value, .. } => {
                    self.check_duplicate_lets_in_expr(value, seen);
                }
                _ => {}
            }
        }
    }

    fn check_duplicate_lets_in_expr<'b>(
        &mut self,
        expr: &'b Expr,
        seen: &mut HashMap<&'b str, Span>,
    ) {
        match expr {
            Expr::LetExpr { name, value, .. } => {
                if seen.contains_key(name.name.as_str()) {
                    self.push(
                        Diagnostic::error(
                            name.span,
                            format!("Duplicate let binding '{}' in this rule.", name.name),
                        )
                        .with_code("allium.let.duplicateBinding"),
                    );
                } else {
                    seen.insert(&name.name, name.span);
                }
                self.check_duplicate_lets_in_expr(value, seen);
            }
            Expr::Block { items, .. } => {
                for item in items {
                    self.check_duplicate_lets_in_expr(item, seen);
                }
            }
            Expr::For { body, .. } => {
                self.check_duplicate_lets_in_expr(body, seen);
            }
            Expr::Conditional { branches, else_body, .. } => {
                for b in branches {
                    self.check_duplicate_lets_in_expr(&b.body, seen);
                }
                if let Some(body) = else_body {
                    self.check_duplicate_lets_in_expr(body, seen);
                }
            }
            _ => {}
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

        // Walk all expressions looking for config.field references
        for d in &self.module.declarations {
            match d {
                Decl::Block(b) => {
                    if b.kind == BlockKind::Config {
                        continue;
                    }
                    for item in &b.items {
                        self.check_config_refs_in_item(&item.kind, &config_params);
                    }
                }
                Decl::Invariant(inv) => {
                    self.check_config_refs_in_expr(&inv.body, &config_params);
                }
                _ => {}
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
                            Diagnostic::warning(
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
            Expr::For { collection, filter, body, .. } => {
                self.check_config_refs_in_expr(collection, params);
                if let Some(f) = filter {
                    self.check_config_refs_in_expr(f, params);
                }
                self.check_config_refs_in_expr(body, params);
            }
            Expr::LetExpr { value, .. } => {
                self.check_config_refs_in_expr(value, params);
            }
            Expr::Lambda { body, .. } => {
                self.check_config_refs_in_expr(body, params);
            }
            _ => {}
        }
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

    fn analyse_src(src: &str) -> crate::diagnostic::AnalyseResult {
        let input = if src.starts_with("-- allium:") {
            src.to_string()
        } else {
            format!("-- allium: 3\n{src}")
        };
        let result = parse(&input);
        analyse(&result.module, &input)
    }

    fn has_finding(result: &crate::diagnostic::AnalyseResult, finding_type: &str) -> bool {
        result.findings.iter().any(|f| f["type"] == finding_type)
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

    // -- .created() status tracing (enhancement 1) --

    #[test]
    fn created_with_status_suppresses_unreachable() {
        let ds = analyze_src(
            "entity Order {\n  status: pending | confirmed\n  customer: String\n  \
             transitions status {\n    pending -> confirmed\n    terminal: confirmed\n  }\n}\n\n\
             rule PlaceOrder {\n  when: CustomerPlacesOrder(customer)\n  ensures:\n    \
             Order.created(\n      status: pending,\n      customer: customer\n    )\n}\n\n\
             rule ConfirmOrder {\n  when: SellerConfirms(seller, order)\n  \
             requires: order.status = pending\n  ensures: order.status = confirmed\n}\n",
        );
        assert!(!has_code(&ds, "allium.status.unreachableValue"));
    }

    #[test]
    fn created_omitting_status_warns() {
        let ds = analyze_src(
            "entity Order {\n  status: pending | confirmed\n  customer: String\n  \
             transitions status {\n    pending -> confirmed\n    terminal: confirmed\n  }\n}\n\n\
             rule PlaceOrder {\n  when: CustomerPlacesOrder(customer)\n  ensures:\n    \
             Order.created(\n      customer: customer\n    )\n}\n",
        );
        assert!(has_code(&ds, "allium.created.missingStatus"));
    }

    #[test]
    fn created_multiple_initial_statuses() {
        let ds = analyze_src(
            "entity Proposal {\n  status: draft | submitted | reviewed\n  author: String\n  \
             transitions status {\n    draft -> submitted\n    submitted -> reviewed\n    \
             terminal: reviewed\n  }\n}\n\n\
             rule CreateDraft {\n  when: AuthorStarts(author)\n  ensures:\n    \
             Proposal.created(status: draft, author: author)\n}\n\n\
             rule SubmitDirectly {\n  when: AuthorSubmits(author)\n  ensures:\n    \
             Proposal.created(status: submitted, author: author)\n}\n\n\
             rule Review {\n  when: ReviewerReviews(proposal)\n  \
             requires: proposal.status = submitted\n  ensures: proposal.status = reviewed\n}\n",
        );
        // draft and submitted are set via .created(), reviewed via ensures — none should be unreachable
        assert!(!has_code(&ds, "allium.status.unreachableValue"));
    }

    #[test]
    fn created_invalid_status_errors() {
        let ds = analyze_src(
            "entity Task {\n  status: open | in_progress | done\n  title: String\n  \
             transitions status {\n    open -> in_progress\n    in_progress -> done\n    \
             terminal: done\n  }\n}\n\n\
             rule ImportTask {\n  when: SystemImports(title)\n  ensures:\n    \
             Task.created(status: archived, title: title)\n}\n",
        );
        assert!(has_code(&ds, "allium.created.invalidStatus"));
    }

    #[test]
    fn created_without_transitions_no_missing_status_warning() {
        // Entity without transition graph: .created() omitting status should not warn
        let ds = analyze_src(
            "entity Note {\n  status: draft | published\n  content: String\n}\n\n\
             rule CreateNote {\n  when: UserCreates(content)\n  ensures:\n    \
             Note.created(content: content)\n}\n",
        );
        assert!(!has_code(&ds, "allium.created.missingStatus"));
    }

    // -- Terminal state suppression (enhancement 2) --

    #[test]
    fn terminal_declared_suppresses_no_exit() {
        let ds = analyze_src(
            "entity Subscription {\n  status: active | paused | completed | cancelled\n  \
             transitions status {\n    active -> paused\n    paused -> active\n    \
             active -> completed\n    active -> cancelled\n    paused -> cancelled\n    \
             terminal: completed, cancelled\n  }\n}\n\n\
             rule Activate {\n  when: UserActivates(user, subscription)\n  \
             requires: subscription.status = paused\n  ensures: subscription.status = active\n}\n\n\
             rule Pause {\n  when: UserPauses(user, subscription)\n  \
             requires: subscription.status = active\n  ensures: subscription.status = paused\n}\n\n\
             rule Complete {\n  when: PeriodEnds(subscription)\n  \
             requires: subscription.status = active\n  ensures: subscription.status = completed\n}\n\n\
             rule Cancel {\n  when: UserCancels(user, subscription)\n  \
             requires: subscription.status = active\n  ensures: subscription.status = cancelled\n}\n",
        );
        assert!(!has_code(&ds, "allium.status.noExit"));
    }

    #[test]
    fn non_terminal_no_exit_still_warns() {
        let ds = analyze_src(
            "entity Ticket {\n  status: open | stuck | resolved\n  \
             transitions status {\n    open -> stuck\n    open -> resolved\n    \
             terminal: resolved\n  }\n}\n\n\
             rule Escalate {\n  when: AgentEscalates(agent, ticket)\n  \
             requires: ticket.status = open\n  ensures: ticket.status = stuck\n}\n\n\
             rule Resolve {\n  when: AgentResolves(agent, ticket)\n  \
             requires: ticket.status = open\n  ensures: ticket.status = resolved\n}\n",
        );
        // 'stuck' is not terminal and has no exit — should warn
        assert!(has_code(&ds, "allium.status.noExit"));
    }

    // -- Cross-entity rule matching (enhancement 3) --

    #[test]
    fn cross_entity_trigger_param_recognised() {
        let ds = analyze_src(
            "entity InterviewSlot {\n  status: scheduled | confirmed | completed\n  \
             transitions status {\n    scheduled -> confirmed\n    \
             confirmed -> completed\n    terminal: completed\n  }\n}\n\n\
             rule CreateSlot {\n  when: RecruiterSchedules(time)\n  ensures:\n    \
             InterviewSlot.created(status: scheduled)\n}\n\n\
             rule ConfirmSlot {\n  when: InterviewerConfirms(interviewer, slot)\n  \
             requires: slot.status = scheduled\n  ensures: slot.status = confirmed\n}\n\n\
             rule CompleteSlot {\n  when: InterviewerSubmits(interviewer, slot)\n  \
             requires: slot.status = confirmed\n  ensures: slot.status = completed\n}\n",
        );
        // Cross-entity rules should be recognised — no false positives on InterviewSlot
        assert!(!ds.iter().any(|d| {
            d.code == Some("allium.status.unreachableValue")
                && d.message.contains("InterviewSlot")
        }));
        assert!(!ds.iter().any(|d| {
            d.code == Some("allium.status.noExit") && d.message.contains("InterviewSlot")
        }));
    }

    #[test]
    fn cross_entity_undeclared_transition() {
        let ds = analyze_src(
            "entity InterviewSlot {\n  status: scheduled | confirmed | completed\n  \
             transitions status {\n    scheduled -> confirmed\n    \
             confirmed -> completed\n    terminal: completed\n  }\n}\n\n\
             rule ConfirmSlot {\n  when: InterviewerConfirms(interviewer, slot)\n  \
             requires: slot.status = completed\n  ensures: slot.status = confirmed\n}\n",
        );
        assert!(has_code(&ds, "allium.status.undeclaredTransition"));
    }

    #[test]
    fn nested_entity_status_recognised() {
        let ds = analyze_src(
            "entity Order {\n  status: placed | paid\n  payment: Payment\n  \
             transitions status {\n    placed -> paid\n    terminal: paid\n  }\n}\n\n\
             entity Payment {\n  status: pending | captured | failed\n  \
             transitions status {\n    pending -> captured\n    pending -> failed\n    \
             terminal: captured, failed\n  }\n}\n\n\
             rule CapturePayment {\n  when: GatewayConfirms(order, ref)\n  \
             requires: order.payment.status = pending\n  \
             ensures: order.payment.status = captured\n}\n",
        );
        // Nested access should be recognised — no false positives on Payment
        assert!(!ds.iter().any(|d| {
            (d.code == Some("allium.status.unreachableValue")
                || d.code == Some("allium.status.noExit"))
                && d.message.contains("'captured'")
        }));
    }

    // -- Process completeness (enhancements 4-6) --

    #[test]
    fn dead_transition_missing_producer() {
        let r = analyse_src(
            "entity App {\n  status: submitted | screening | approved | rejected\n  \
             verified: Boolean\n  \
             transitions status {\n    submitted -> screening\n    screening -> approved\n    \
             screening -> rejected\n    terminal: approved, rejected\n  }\n}\n\n\
             rule Begin {\n  when: ReviewerStarts(reviewer, app)\n  \
             requires: app.status = submitted\n  ensures: app.status = screening\n}\n\n\
             rule Approve {\n  when: ReviewerApproves(reviewer, app)\n  \
             requires:\n    app.status = screening\n    app.verified = true\n  \
             ensures: app.status = approved\n}\n\n\
             rule Reject {\n  when: ReviewerRejects(reviewer, app)\n  \
             requires: app.status = screening\n  ensures: app.status = rejected\n}\n",
        );
        assert!(has_finding(&r, "dead_transition"));
        assert!(has_finding(&r, "missing_producer"));
    }

    #[test]
    fn satisfied_requires_no_dead_transition() {
        let r = analyse_src(
            "entity App {\n  status: submitted | screening | approved | rejected\n  \
             verified: Boolean\n  \
             transitions status {\n    submitted -> screening\n    screening -> approved\n    \
             screening -> rejected\n    terminal: approved, rejected\n  }\n}\n\n\
             rule Begin {\n  when: ReviewerStarts(reviewer, app)\n  \
             requires: app.status = submitted\n  ensures: app.status = screening\n}\n\n\
             rule Verify {\n  when: SystemVerifies(app, result)\n  \
             requires: app.status = screening\n  ensures: app.verified = result\n}\n\n\
             rule Approve {\n  when: ReviewerApproves(reviewer, app)\n  \
             requires:\n    app.status = screening\n    app.verified = true\n  \
             ensures: app.status = approved\n}\n\n\
             rule Reject {\n  when: ReviewerRejects(reviewer, app)\n  \
             requires: app.status = screening\n  ensures: app.status = rejected\n}\n",
        );
        assert!(!has_finding(&r, "dead_transition"));
        assert!(!has_finding(&r, "missing_producer"));
    }

    #[test]
    fn deadlock_detected() {
        let r = analyse_src(
            "entity Doc {\n  status: submitted | review | approved | rejected\n  \
             reviewer_assigned: Boolean\n  \
             transitions status {\n    submitted -> review\n    review -> approved\n    \
             review -> rejected\n    terminal: approved, rejected\n  }\n}\n\n\
             rule Submit {\n  when: AuthorSubmits(author, doc)\n  \
             requires: doc.status = submitted\n  ensures: doc.status = review\n}\n\n\
             rule Approve {\n  when: ReviewerApproves(reviewer, doc)\n  \
             requires:\n    doc.status = review\n    doc.reviewer_assigned = true\n  \
             ensures: doc.status = approved\n}\n\n\
             rule Reject {\n  when: ReviewerRejects(reviewer, doc)\n  \
             requires:\n    doc.status = review\n    doc.reviewer_assigned = true\n  \
             ensures: doc.status = rejected\n}\n",
        );
        assert!(has_finding(&r, "deadlock"));
    }

    #[test]
    fn no_deadlock_when_paths_open() {
        let r = analyse_src(
            "entity Invoice {\n  status: draft | sent | paid | void\n  \
             transitions status {\n    draft -> sent\n    draft -> void\n    \
             sent -> paid\n    sent -> void\n    terminal: paid, void\n  }\n}\n\n\
             rule Send {\n  when: AccountantSends(accountant, invoice)\n  \
             requires: invoice.status = draft\n  ensures: invoice.status = sent\n}\n\n\
             rule Pay {\n  when: PaymentReceived(invoice)\n  \
             requires: invoice.status = sent\n  ensures: invoice.status = paid\n}\n\n\
             rule VoidDraft {\n  when: AccountantVoids(accountant, invoice)\n  \
             requires: invoice.status = draft\n  ensures: invoice.status = void\n}\n\n\
             rule VoidSent {\n  when: AccountantVoids(accountant, invoice)\n  \
             requires: invoice.status = sent\n  ensures: invoice.status = void\n}\n",
        );
        assert!(!has_finding(&r, "deadlock"));
    }

    // -- Conflict detection (enhancement 7) --

    #[test]
    fn conflict_temporal_vs_external() {
        let r = analyse_src(
            "entity Membership {\n  status: active | expired | extended\n  \
             expires_at: Timestamp\n  \
             transitions status {\n    active -> expired\n    active -> extended\n    \
             terminal: expired, extended\n  }\n}\n\n\
             rule AutoExpire {\n  when: m: Membership.expires_at <= now\n  \
             requires: m.status = active\n  ensures: m.status = expired\n}\n\n\
             rule ManualExtend {\n  when: AdminExtends(admin, membership)\n  \
             requires: membership.status = active\n  ensures: membership.status = extended\n}\n",
        );
        assert!(has_finding(&r, "conflict"));
    }

    #[test]
    fn no_conflict_actor_choice() {
        let r = analyse_src(
            "entity LeaveRequest {\n  status: pending | approved | denied\n  \
             transitions status {\n    pending -> approved\n    pending -> denied\n    \
             terminal: approved, denied\n  }\n}\n\n\
             rule Approve {\n  when: ManagerApproves(manager, request)\n  \
             requires: request.status = pending\n  ensures: request.status = approved\n}\n\n\
             rule Deny {\n  when: ManagerDenies(manager, request)\n  \
             requires: request.status = pending\n  ensures: request.status = denied\n}\n",
        );
        assert!(!has_finding(&r, "conflict"));
    }

    // -- Invariant verification (enhancement 8) --

    #[test]
    fn invariant_violation_detected() {
        let r = analyse_src(
            "entity JobRole {\n  status: open | filled\n  \
             candidacies: Candidacy with role = this\n  \
             transitions status {\n    open -> filled\n    terminal: filled\n  }\n}\n\n\
             entity Candidacy {\n  status: active | hired | rejected\n  \
             role: JobRole\n  \
             transitions status {\n    active -> hired\n    active -> rejected\n    \
             terminal: hired, rejected\n  }\n}\n\n\
             rule Hire {\n  when: ManagerHires(manager, candidacy)\n  \
             requires: candidacy.status = active\n  \
             ensures: candidacy.status = hired\n}\n\n\
             invariant OneHirePerRole {\n  for a in Candidacies:\n    for b in Candidacies:\n      \
             a != b and a.role = b.role implies not (a.status = hired and b.status = hired)\n}\n",
        );
        assert!(has_finding(&r, "invariant_risk"));
    }

    #[test]
    fn invariant_guarded_no_violation() {
        let r = analyse_src(
            "entity JobRole {\n  status: open | filled\n  \
             candidacies: Candidacy with role = this\n  \
             transitions status {\n    open -> filled\n    terminal: filled\n  }\n}\n\n\
             entity Candidacy {\n  status: active | hired | rejected\n  \
             role: JobRole\n  \
             transitions status {\n    active -> hired\n    active -> rejected\n    \
             terminal: hired, rejected\n  }\n}\n\n\
             rule Hire {\n  when: ManagerHires(manager, candidacy)\n  \
             requires:\n    candidacy.status = active\n    candidacy.role.status = open\n  \
             ensures:\n    candidacy.status = hired\n    candidacy.role.status = filled\n}\n\n\
             invariant OneHirePerRole {\n  for a in Candidacies:\n    for b in Candidacies:\n      \
             a != b and a.role = b.role implies not (a.status = hired and b.status = hired)\n}\n",
        );
        assert!(!has_finding(&r, "invariant_risk"));
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
