//! AST for the Allium specification language.
//!
//! The parse tree uses a uniform block-item representation for declaration
//! bodies: every `name: value`, `keyword: value` and `let name = value` within
//! braces is a [`BlockItem`]. Semantic classification into entity fields vs
//! relationships vs derived values, or trigger types, happens in a later pass.
//!
//! Expressions are fully typed — the parser produces the rich [`Expr`] tree
//! directly.

use crate::Span;

// ---------------------------------------------------------------------------
// Top level
// ---------------------------------------------------------------------------

/// A parsed `.allium` file.
#[derive(Debug, Clone)]
pub struct Module {
    pub span: Span,
    /// Extracted from `-- allium: N` if present.
    pub version: Option<u32>,
    pub declarations: Vec<Decl>,
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Decl {
    Use(UseDecl),
    Block(BlockDecl),
    Default(DefaultDecl),
    Variant(VariantDecl),
    Deferred(DeferredDecl),
    OpenQuestion(OpenQuestionDecl),
}

/// `use "path" as alias`
#[derive(Debug, Clone)]
pub struct UseDecl {
    pub span: Span,
    pub path: StringLiteral,
    pub alias: Option<Ident>,
}

/// A named or anonymous block: `entity User { ... }`, `config { ... }`, etc.
#[derive(Debug, Clone)]
pub struct BlockDecl {
    pub span: Span,
    pub kind: BlockKind,
    /// `None` for `given` and local `config` blocks.
    pub name: Option<Ident>,
    pub items: Vec<BlockItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Entity,
    ExternalEntity,
    Value,
    Enum,
    Given,
    Config,
    Rule,
    Surface,
    Actor,
}

/// `default [Type] name = value`
#[derive(Debug, Clone)]
pub struct DefaultDecl {
    pub span: Span,
    pub type_name: Option<Ident>,
    pub name: Ident,
    pub value: Expr,
}

/// `variant Name : Type { ... }`
#[derive(Debug, Clone)]
pub struct VariantDecl {
    pub span: Span,
    pub name: Ident,
    pub base: Expr,
    pub items: Vec<BlockItem>,
}

/// `deferred path.expression`
#[derive(Debug, Clone)]
pub struct DeferredDecl {
    pub span: Span,
    pub path: Expr,
}

/// `open question "text"`
#[derive(Debug, Clone)]
pub struct OpenQuestionDecl {
    pub span: Span,
    pub text: StringLiteral,
}

// ---------------------------------------------------------------------------
// Block items — uniform representation for declaration bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BlockItem {
    pub span: Span,
    pub kind: BlockItemKind,
}

#[derive(Debug, Clone)]
pub enum BlockItemKind {
    /// `keyword: value` — when:, requires:, ensures:, facing:, etc.
    Clause { keyword: String, value: Expr },
    /// `name: value` — field, relationship, projection, derived value.
    Assignment { name: Ident, value: Expr },
    /// `name(params): value` — parameterised derived value.
    ParamAssignment {
        name: Ident,
        params: Vec<Ident>,
        value: Expr,
    },
    /// `let name = value`
    Let { name: Ident, value: Expr },
    /// Bare name inside an enum body — `pending`, `shipped`, etc.
    EnumVariant { name: Ident },
    /// `for binding in collection [where filter]: ...` at block level (rule iteration)
    ForBlock {
        binding: ForBinding,
        collection: Expr,
        filter: Option<Expr>,
        items: Vec<BlockItem>,
    },
    /// `if condition: ... else if ...: ... else: ...` at block level
    IfBlock {
        branches: Vec<CondBlockBranch>,
        else_items: Option<Vec<BlockItem>>,
    },
    /// `Shard.shard_cache: value` — dot-path reverse relationship
    PathAssignment { path: Expr, value: Expr },
    /// `open question "text"` (nested within a block)
    OpenQuestion { text: StringLiteral },
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Expr {
    /// `identifier` or `_`
    Ident(Ident),

    /// `"text"` possibly with `{interpolation}`
    StringLiteral(StringLiteral),

    /// `42`, `100_000`, `3.14`
    NumberLiteral { span: Span, value: String },

    /// `true`, `false`
    BoolLiteral { span: Span, value: bool },

    /// `null`
    Null { span: Span },

    /// `now`
    Now { span: Span },

    /// `this`
    This { span: Span },

    /// `within`
    Within { span: Span },

    /// `24.hours`, `7.days`
    DurationLiteral { span: Span, value: String },

    /// `{ a, b, c }` — set literal
    SetLiteral { span: Span, elements: Vec<Expr> },

    /// `[a, b, c]` — list literal
    ListLiteral { span: Span, elements: Vec<Expr> },

    /// `{ key: value, ... }` — object literal
    ObjectLiteral { span: Span, fields: Vec<NamedArg> },

    /// `Set<T>`, `List<T>` — generic type annotation
    GenericType {
        span: Span,
        name: Box<Expr>,
        args: Vec<Expr>,
    },

    /// `a.b`
    MemberAccess {
        span: Span,
        object: Box<Expr>,
        field: Ident,
    },

    /// `a?.b`
    OptionalAccess {
        span: Span,
        object: Box<Expr>,
        field: Ident,
    },

    /// `a ?? b`
    NullCoalesce {
        span: Span,
        left: Box<Expr>,
        right: Box<Expr>,
    },

    /// `func(args)` or `entity.method(args)`
    Call {
        span: Span,
        function: Box<Expr>,
        args: Vec<CallArg>,
    },

    /// `Entity{field1, field2}` or `Entity{field: value}`
    JoinLookup {
        span: Span,
        entity: Box<Expr>,
        fields: Vec<JoinField>,
    },

    /// `a + b`, `a - b`, `a * b`, `a / b`
    BinaryOp {
        span: Span,
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },

    /// `a = b`, `a != b`, `a < b`, `a <= b`, `a > b`, `a >= b`
    Comparison {
        span: Span,
        left: Box<Expr>,
        op: ComparisonOp,
        right: Box<Expr>,
    },

    /// `a and b`, `a or b`
    LogicalOp {
        span: Span,
        left: Box<Expr>,
        op: LogicalOp,
        right: Box<Expr>,
    },

    /// `not expr`
    Not { span: Span, operand: Box<Expr> },

    /// `x in collection`
    In {
        span: Span,
        element: Box<Expr>,
        collection: Box<Expr>,
    },

    /// `x not in collection`
    NotIn {
        span: Span,
        element: Box<Expr>,
        collection: Box<Expr>,
    },

    /// `exists expr`
    Exists { span: Span, operand: Box<Expr> },

    /// `not exists expr`
    NotExists { span: Span, operand: Box<Expr> },

    /// `collection where condition`
    Where {
        span: Span,
        source: Box<Expr>,
        condition: Box<Expr>,
    },

    /// `collection with predicate` (in relationship declarations)
    With {
        span: Span,
        source: Box<Expr>,
        predicate: Box<Expr>,
    },

    /// `a | b` — pipe, used for inline enums and sum type discriminators
    Pipe {
        span: Span,
        left: Box<Expr>,
        right: Box<Expr>,
    },

    /// `x => body`
    Lambda {
        span: Span,
        param: Box<Expr>,
        body: Box<Expr>,
    },

    /// `if cond: a else if cond: b else: c`
    Conditional {
        span: Span,
        branches: Vec<CondBranch>,
        else_body: Option<Box<Expr>>,
    },

    /// `for x in collection [where cond]: body`
    For {
        span: Span,
        binding: ForBinding,
        collection: Box<Expr>,
        filter: Option<Box<Expr>>,
        body: Box<Expr>,
    },

    /// `collection where cond -> field` — projection mapping
    ProjectionMap {
        span: Span,
        source: Box<Expr>,
        field: Ident,
    },

    /// `Entity.status transitions_to state`
    TransitionsTo {
        span: Span,
        subject: Box<Expr>,
        new_state: Box<Expr>,
    },

    /// `Entity.status becomes state`
    Becomes {
        span: Span,
        subject: Box<Expr>,
        new_state: Box<Expr>,
    },

    /// `name: expr` — binding inside a clause value (when triggers, facing, context)
    Binding {
        span: Span,
        name: Ident,
        value: Box<Expr>,
    },

    /// `action when condition` — guard on a provides/related item
    WhenGuard {
        span: Span,
        action: Box<Expr>,
        condition: Box<Expr>,
    },

    /// `T?` — optional type annotation
    TypeOptional {
        span: Span,
        inner: Box<Expr>,
    },

    /// `let name = value` inside an expression block (ensures, provides, etc.)
    LetExpr {
        span: Span,
        name: Ident,
        value: Box<Expr>,
    },

    /// `oauth/Session` — qualified name with module prefix
    QualifiedName(QualifiedName),

    /// A sequence of expressions from a multi-line block.
    Block { span: Span, items: Vec<Expr> },

}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Ident(id) => id.span,
            Expr::StringLiteral(s) => s.span,
            Expr::NumberLiteral { span, .. }
            | Expr::BoolLiteral { span, .. }
            | Expr::Null { span }
            | Expr::Now { span }
            | Expr::This { span }
            | Expr::Within { span }
            | Expr::DurationLiteral { span, .. }
            | Expr::SetLiteral { span, .. }
            | Expr::ListLiteral { span, .. }
            | Expr::ObjectLiteral { span, .. }
            | Expr::GenericType { span, .. }
            | Expr::MemberAccess { span, .. }
            | Expr::OptionalAccess { span, .. }
            | Expr::NullCoalesce { span, .. }
            | Expr::Call { span, .. }
            | Expr::JoinLookup { span, .. }
            | Expr::BinaryOp { span, .. }
            | Expr::Comparison { span, .. }
            | Expr::LogicalOp { span, .. }
            | Expr::Not { span, .. }
            | Expr::In { span, .. }
            | Expr::NotIn { span, .. }
            | Expr::Exists { span, .. }
            | Expr::NotExists { span, .. }
            | Expr::Where { span, .. }
            | Expr::With { span, .. }
            | Expr::Pipe { span, .. }
            | Expr::Lambda { span, .. }
            | Expr::Conditional { span, .. }
            | Expr::For { span, .. }
            | Expr::ProjectionMap { span, .. }
            | Expr::TransitionsTo { span, .. }
            | Expr::Becomes { span, .. }
            | Expr::Binding { span, .. }
            | Expr::WhenGuard { span, .. }
            | Expr::TypeOptional { span, .. }
            | Expr::LetExpr { span, .. }
            | Expr::Block { span, .. } => *span,
            Expr::QualifiedName(q) => q.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CondBranch {
    pub span: Span,
    pub condition: Expr,
    pub body: Expr,
}

/// A branch of a block-level `if`/`else if` chain.
#[derive(Debug, Clone)]
pub struct CondBlockBranch {
    pub span: Span,
    pub condition: Expr,
    pub items: Vec<BlockItem>,
}

/// Binding in a `for` loop — either a single identifier or a
/// destructured tuple like `(a, b)`.
#[derive(Debug, Clone)]
pub enum ForBinding {
    Single(Ident),
    Destructured(Vec<Ident>, Span),
}

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Ident {
    pub span: Span,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct QualifiedName {
    pub span: Span,
    pub qualifier: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct StringLiteral {
    pub span: Span,
    pub parts: Vec<StringPart>,
}

#[derive(Debug, Clone)]
pub enum StringPart {
    Text(String),
    Interpolation(Ident),
}

#[derive(Debug, Clone)]
pub struct NamedArg {
    pub span: Span,
    pub name: Ident,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub enum CallArg {
    Positional(Expr),
    Named(NamedArg),
}

#[derive(Debug, Clone)]
pub struct JoinField {
    pub span: Span,
    pub field: Ident,
    /// If absent, matches a local variable with the same name.
    pub value: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonOp {
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp {
    And,
    Or,
}
