//! v4 AST. Phase 4a is structural: declarations and body items are parsed into
//! this tree, but predicate bodies (invariant/guarantee/establish/init and
//! action requires/ensures) are kept as raw source spans. The predicate
//! expression grammar lands in 4c (analyse), when the discharge path needs it.

use serde::Serialize;

use crate::span::Span;

#[derive(Debug, Clone, Serialize)]
pub struct Module {
    pub version: Option<u32>,
    pub span: Span,
    pub decls: Vec<Decl>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum DeclKind {
    Contract,
    Component,
    /// `use <target> as <alias>` — a banked import (translate-and-sum). v4
    /// cross-module resolution semantics are parked (SEAL-5); this is the surface.
    Import,
}

#[derive(Debug, Clone, Serialize)]
pub struct Decl {
    pub span: Span,
    pub kind: DeclKind,
    /// Declaration name, or the import target for `Import`.
    pub name: String,
    /// Import alias for `use … as <alias>`.
    pub alias: Option<String>,
    pub params: Vec<Param>,
    /// `satisfies (x : Contract)` on a component.
    pub satisfies: Vec<Param>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Param {
    pub span: Span,
    pub name: String,
    /// Raw type text, e.g. `Currency`, `Vault(gbp)`, `AtomicCommit`.
    pub ty: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum ItemKind {
    Entity,
    State,
    Given,
    Action,
    // Role-tagged named predicates (CONSTRUCTS.md P2: one predicate primitive, many
    // roles). All share the `<role> <name> [means <pred>]` shape.
    Invariant,
    Guarantee,
    Fault,
    Requirement,
    Axiom,
    Rely,
    Establish,
    Init,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub span: Span,
    pub kind: ItemKind,
    /// Declared name where the item has one (entity/state/given/action/invariant/guarantee).
    pub name: Option<String>,
    /// Parameter/argument names in the item's `(…)`, e.g. an action's params or a
    /// relation's arguments. In scope for this item's predicate bodies.
    pub params: Vec<String>,
    /// Modifiers seen before the item keyword: `pub`, `abstract`, `readable`.
    pub modifiers: Vec<String>,
    /// Raw predicate span for `means`/`init`/`establish` bodies.
    pub body: Option<Span>,
    /// Raw `requires` span (actions).
    pub requires: Option<Span>,
    /// Raw `ensures` span (actions).
    pub ensures: Option<Span>,
    /// `establish … by a, b` witnesses.
    pub witnesses: Vec<String>,
}

impl Item {
    pub fn new(kind: ItemKind, span: Span) -> Self {
        Self {
            span,
            kind,
            name: None,
            params: Vec::new(),
            modifiers: Vec::new(),
            body: None,
            requires: None,
            ensures: None,
            witnesses: Vec::new(),
        }
    }
}
