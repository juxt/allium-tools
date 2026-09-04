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
    /// `terminal <state-condition>` — declares a lifecycle end state. Desugars to a finality invariant
    /// (`old(cond) implies cond`, the state is never left) and marks the state as an intended dead-end so
    /// deadlock detection does not flag it.
    Terminal,
    /// `transitions <obs>(<var>) <edges> [terminal: <tag>]` — a declarative lifecycle edge block (the v3
    /// idiom). Captured as a whole so it is not silently shredded into bogus items; not yet expanded to
    /// guarded actions (that needs a synthesised-predicate body mechanism), so a pass warns it is inert.
    Transitions,
    /// `objective <goal> within <bound> [measure <obs> decreasing] [under <env>]` — a bounded liveness
    /// obligation (the anti-vacuity FLOOR: what the system must ACHIEVE, dual to an invariant's ceiling).
    /// PROVISIONAL name. Body captured raw for now; deeper checking (bound-as-safety, measure
    /// well-foundedness) is a later increment. Its presence is what the ceiling-without-floor check reads.
    Objective,
    /// `budget <metric> <cmp> <threshold> over <window>` — a statistical/SLA obligation (latency
    /// percentile, error rate) over a cohort window, monitored-never-proved. PROVISIONAL name. Body raw.
    Budget,
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
    /// Raw `ensures` spans (actions). Multiple `ensures` clauses read as their conjunction; see
    /// [`Item::ensures_expr`]. Empty for a non-action or an action with no postcondition.
    pub ensures: Vec<Span>,
    /// `establish … by a, b` witnesses.
    pub witnesses: Vec<String>,
    /// A `where <pred>` refinement clause on a typed declaration (`state balance : Money where balance >= 0`):
    /// the value is constrained by the predicate. Desugars to an invariant over the observable.
    pub where_pred: Option<Span>,
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
            ensures: Vec::new(),
            witnesses: Vec::new(),
            where_pred: None,
        }
    }

    /// The action's postcondition as one predicate: the conjunction of its `ensures` clauses, or `None`
    /// if it has none. Multiple clauses `ensures a` / `ensures b` read as `a and b`.
    pub fn ensures_expr(&self, src: &str) -> Option<crate::expr::Expr> {
        let mut it = self.ensures.iter().map(|sp| crate::expr::parse_predicate(sp.slice(src)).0);
        let first = it.next()?;
        Some(it.fold(first, |acc, e| crate::expr::Expr::Binary {
            op: crate::expr::BinOp::And,
            lhs: Box::new(acc),
            rhs: Box::new(e),
        }))
    }
}
