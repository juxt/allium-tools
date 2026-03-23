//! Recursive descent parser for Allium.
//!
//! Expressions use a Pratt parser (precedence climbing). Declarations and block
//! bodies use direct recursive descent. Multi-line clause values are detected
//! by comparing the line/column of the next token against the clause keyword.

use serde::Serialize;

use crate::ast::*;
use crate::diagnostic::Diagnostic;
use crate::lexer::{lex, SourceMap, Token, TokenKind};
use crate::Span;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ParseResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse an Allium source file into a [`Module`].
pub fn parse(source: &str) -> ParseResult {
    let tokens = lex(source);
    let source_map = SourceMap::new(source);
    let mut p = Parser {
        source,
        tokens,
        pos: 0,
        source_map,
        diagnostics: Vec::new(),
    };
    let module = p.parse_module();
    ParseResult {
        module,
        diagnostics: p.diagnostics,
    }
}

// ---------------------------------------------------------------------------
// Parser state
// ---------------------------------------------------------------------------

struct Parser<'s> {
    source: &'s str,
    tokens: Vec<Token>,
    pos: usize,
    source_map: SourceMap,
    diagnostics: Vec<Diagnostic>,
}

// ---------------------------------------------------------------------------
// Navigation helpers
// ---------------------------------------------------------------------------

impl<'s> Parser<'s> {
    fn peek(&self) -> Token {
        self.tokens[self.pos]
    }

    fn peek_kind(&self) -> TokenKind {
        self.tokens[self.pos].kind
    }

    fn peek_at(&self, offset: usize) -> Token {
        let idx = (self.pos + offset).min(self.tokens.len() - 1);
        self.tokens[idx]
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos];
        if tok.kind != TokenKind::Eof {
            self.pos += 1;
        }
        tok
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn at_eof(&self) -> bool {
        self.at(TokenKind::Eof)
    }

    fn eat(&mut self, kind: TokenKind) -> Option<Token> {
        if self.at(kind) {
            Some(self.advance())
        } else {
            None
        }
    }

    fn expect(&mut self, kind: TokenKind) -> Option<Token> {
        if self.at(kind) {
            Some(self.advance())
        } else {
            self.error(
                self.peek().span,
                format!("expected {kind}, found {}", self.peek_kind()),
            );
            None
        }
    }

    fn text(&self, span: Span) -> &'s str {
        &self.source[span.start..span.end]
    }

    fn line_of(&self, span: Span) -> u32 {
        self.source_map.line_col(span.start).0
    }

    fn col_of(&self, span: Span) -> u32 {
        self.source_map.line_col(span.start).1
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        let line = self.source_map.line_col(span.start).0;
        if let Some(last) = self.diagnostics.last() {
            if last.severity == crate::diagnostic::Severity::Error
                && self.source_map.line_col(last.span.start).0 == line
            {
                return;
            }
        }
        self.diagnostics.push(Diagnostic::error(span, msg));
    }

    /// Consume and return an [`Ident`] from any word token.
    fn parse_ident(&mut self) -> Option<Ident> {
        self.parse_ident_in("identifier")
    }

    /// Consume and return an [`Ident`] with a context-specific label for errors.
    fn parse_ident_in(&mut self, context: &str) -> Option<Ident> {
        let tok = self.peek();
        if tok.kind.is_word() {
            self.advance();
            Some(Ident {
                span: tok.span,
                name: self.text(tok.span).to_string(),
            })
        } else {
            self.error(
                tok.span,
                format!("expected {context}, found {}", tok.kind),
            );
            None
        }
    }

    /// Consume a string token and produce a [`StringLiteral`].
    fn parse_string(&mut self) -> Option<StringLiteral> {
        let tok = self.expect(TokenKind::String)?;
        let raw = self.text(tok.span);
        // Strip surrounding quotes
        let inner = &raw[1..raw.len() - 1];
        let parts = parse_string_parts(inner, tok.span.start + 1);
        Some(StringLiteral {
            span: tok.span,
            parts,
        })
    }
}

/// Split the inner content of a string literal into text and interpolation
/// parts. `base_offset` is the byte offset of the first character after the
/// opening quote in the source file.
fn parse_string_parts(inner: &str, base_offset: usize) -> Vec<StringPart> {
    let mut parts = Vec::new();
    let mut buf = String::new();
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            buf.push(bytes[i + 1] as char);
            i += 2;
        } else if bytes[i] == b'{' {
            if !buf.is_empty() {
                parts.push(StringPart::Text(std::mem::take(&mut buf)));
            }
            i += 1; // skip {
            let start = i;
            while i < bytes.len() && bytes[i] != b'}' {
                i += 1;
            }
            let name = std::str::from_utf8(&bytes[start..i]).unwrap_or("").to_string();
            let span_start = base_offset + start;
            let span_end = base_offset + i;
            parts.push(StringPart::Interpolation(Ident {
                span: Span::new(span_start, span_end),
                name,
            }));
            if i < bytes.len() {
                i += 1; // skip }
            }
        } else {
            buf.push(bytes[i] as char);
            i += 1;
        }
    }
    if !buf.is_empty() {
        parts.push(StringPart::Text(buf));
    }
    parts
}

// ---------------------------------------------------------------------------
// Clause-keyword recognition
// ---------------------------------------------------------------------------

/// Returns true for identifiers that act as clause keywords inside blocks.
/// These are parsed as `Clause` items rather than `Assignment` items.
fn is_clause_keyword(text: &str) -> bool {
    matches!(
        text,
        "when"
            | "requires"
            | "ensures"
            | "facing"
            | "context"
            | "exposes"
            | "provides"
            | "related"
            | "timeout"
            | "contracts"
            | "identified_by"
            | "within"
    )
}

/// True for clause keywords whose value can start with a `name: expr` binding.
fn clause_allows_binding(keyword: &str) -> bool {
    matches!(keyword, "when")
}

/// True for keywords that use `keyword name: value` syntax (no colon after the
/// keyword). These directly embed a binding.
fn is_binding_clause_keyword(text: &str) -> bool {
    matches!(text, "facing" | "context")
}

/// True if the current token is a keyword that begins a clause.
fn token_is_clause_keyword(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::When | TokenKind::Requires | TokenKind::Ensures | TokenKind::Within
            | TokenKind::Invariant
            | TokenKind::Transitions
    )
}

/// Extract a `when` clause from a field declaration expression.
///
/// If the expression is `WhenGuard { action: Type, condition: status = v1 | v2 }`,
/// decomposes it into the inner type expression and a `WhenClause`.
fn extract_when_clause(expr: &Expr) -> Option<(Expr, WhenClause)> {
    if let Expr::WhenGuard { action, condition, span } = expr {
        // condition should be `status = value1 | value2 | ...`
        if let Expr::Comparison {
            left,
            op: ComparisonOp::Eq,
            right,
            span: _cond_span,
        } = condition.as_ref()
        {
            if let Expr::Ident(status_field) = left.as_ref() {
                let mut qualifying_states = Vec::new();
                collect_pipe_idents(right, &mut qualifying_states);
                if !qualifying_states.is_empty() {
                    return Some((
                        *action.clone(),
                        WhenClause {
                            span: *span,
                            status_field: status_field.clone(),
                            qualifying_states,
                        },
                    ));
                }
            }
        }
        // Also handle single state: `when status = shipped` (no pipe)
        // Already handled above since a single Ident goes through collect_pipe_idents
    }
    // Also handle `TypeOptional` wrapping a WhenGuard: `Type? when status = ...`
    // The parser would parse `Type?` first (postfix), then `when` (infix on the TypeOptional).
    // Actually, `?` is postfix and `when` is infix with lower BP, so `Type? when cond`
    // parses as `WhenGuard { action: TypeOptional { Type }, condition: ... }`.
    // That case is handled by the WhenGuard branch above — action will be TypeOptional.
    None
}

fn collect_pipe_idents(expr: &Expr, out: &mut Vec<Ident>) {
    match expr {
        Expr::Ident(id) => out.push(id.clone()),
        Expr::Pipe { left, right, .. } => {
            collect_pipe_idents(left, out);
            collect_pipe_idents(right, out);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Module parsing
// ---------------------------------------------------------------------------

impl<'s> Parser<'s> {
    fn parse_module(&mut self) -> Module {
        let start = self.peek().span;
        // Version marker is a comment: `-- allium: N`. Detect it from the raw
        // source before the lexer strips it.
        let version = detect_version(self.source);

        match version {
            None => {
                self.diagnostics.push(Diagnostic::warning(
                    start,
                    "missing version marker; expected '-- allium: 1' as the first line",
                ));
            }
            Some(1) | Some(2) | Some(3) => {}
            Some(v) => {
                self.diagnostics.push(Diagnostic::error(
                    start,
                    format!("unsupported allium version {v}; this parser supports versions 1, 2 and 3"),
                ));
            }
        }

        let mut decls = Vec::new();
        while !self.at_eof() {
            if let Some(d) = self.parse_decl() {
                decls.push(d);
            } else {
                // Recovery: skip one token and try again
                self.advance();
            }
        }
        let end = self.peek().span;
        Module {
            span: start.merge(end),
            version,
            declarations: decls,
        }
    }
}

fn detect_version(source: &str) -> Option<u32> {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("--") {
            let rest = rest.trim();
            if let Some(ver) = rest.strip_prefix("allium:") {
                return ver.trim().parse().ok();
            }
        }
        break; // only check leading lines
    }
    None
}

// ---------------------------------------------------------------------------
// Declaration parsing
// ---------------------------------------------------------------------------

impl<'s> Parser<'s> {
    fn parse_decl(&mut self) -> Option<Decl> {
        match self.peek_kind() {
            TokenKind::Use => self.parse_use_decl().map(Decl::Use),
            TokenKind::Rule => self.parse_block(BlockKind::Rule).map(Decl::Block),
            TokenKind::Entity => self.parse_block(BlockKind::Entity).map(Decl::Block),
            TokenKind::External => {
                let start = self.advance().span;
                if self.at(TokenKind::Entity) {
                    self.parse_block_from(start, BlockKind::ExternalEntity)
                        .map(Decl::Block)
                } else {
                    self.error(self.peek().span, "expected 'entity' after 'external'");
                    None
                }
            }
            TokenKind::Value => self.parse_block(BlockKind::Value).map(Decl::Block),
            TokenKind::Enum => self.parse_block(BlockKind::Enum).map(Decl::Block),
            TokenKind::Given => self.parse_anonymous_block(BlockKind::Given).map(Decl::Block),
            TokenKind::Config => self.parse_anonymous_block(BlockKind::Config).map(Decl::Block),
            TokenKind::Surface => self.parse_block(BlockKind::Surface).map(Decl::Block),
            TokenKind::Actor => self.parse_block(BlockKind::Actor).map(Decl::Block),
            TokenKind::Contract => self.parse_contract_decl().map(Decl::Block),
            TokenKind::Invariant => self.parse_invariant_decl().map(Decl::Invariant),
            TokenKind::Default => self.parse_default_decl().map(Decl::Default),
            TokenKind::Variant => self.parse_variant_decl().map(Decl::Variant),
            TokenKind::Deferred => self.parse_deferred_decl().map(Decl::Deferred),
            TokenKind::Open => self.parse_open_question_decl().map(Decl::OpenQuestion),
            // Qualified config: `alias/config { ... }`
            TokenKind::Ident
                if self.peek_at(1).kind == TokenKind::Slash
                    && self.text(self.peek_at(2).span) == "config" =>
            {
                self.parse_qualified_config().map(Decl::Block)
            }
            _ => {
                self.error(
                    self.peek().span,
                    format!(
                        "expected declaration (entity, rule, enum, value, config, surface, actor, \
                         given, default, variant, deferred, use, open question, contract, invariant), found {}",
                        self.peek_kind(),
                    ),
                );
                None
            }
        }
    }

    // -- module declaration -----------------------------------------------

    // -- use declaration ------------------------------------------------

    fn parse_use_decl(&mut self) -> Option<UseDecl> {
        let start = self.expect(TokenKind::Use)?.span;
        let path = self.parse_string()?;
        let alias = if self.eat(TokenKind::As).is_some() {
            Some(self.parse_ident_in("import alias")?)
        } else {
            None
        };
        let end = alias
            .as_ref()
            .map(|a| a.span)
            .unwrap_or(path.span);
        Some(UseDecl {
            span: start.merge(end),
            path,
            alias,
        })
    }

    // -- named block: `keyword Name { ... }` ----------------------------

    fn parse_block(&mut self, kind: BlockKind) -> Option<BlockDecl> {
        let start = self.advance().span; // consume keyword
        self.parse_block_from(start, kind)
    }

    fn parse_block_from(&mut self, start: Span, kind: BlockKind) -> Option<BlockDecl> {
        // For ExternalEntity the keyword was already consumed by the caller;
        // here we consume Entity.
        if kind == BlockKind::ExternalEntity {
            self.expect(TokenKind::Entity)?;
        }
        let context = match kind {
            BlockKind::Entity | BlockKind::ExternalEntity => "entity name",
            BlockKind::Rule => "rule name",
            BlockKind::Surface => "surface name",
            BlockKind::Actor => "actor name",
            BlockKind::Value => "value type name",
            BlockKind::Enum => "enum name",
            _ => "block name",
        };
        let name = Some(self.parse_ident_in(context)?);
        self.expect(TokenKind::LBrace)?;
        let items = if kind == BlockKind::Enum {
            self.parse_enum_body()
        } else {
            self.parse_block_items(kind)
        };
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(BlockDecl {
            span: start.merge(end),
            kind,
            name,
            items,
        })
    }

    // -- anonymous block: `keyword { ... }` -----------------------------

    /// Parse enum body: pipe-separated variant names.
    /// `{ pending | shipped | delivered }` or `` { en | `de-CH-1996` } ``
    fn parse_enum_body(&mut self) -> Vec<BlockItem> {
        let mut items = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            if self.eat(TokenKind::Pipe).is_some() {
                continue;
            }
            if self.at(TokenKind::BacktickLiteral) {
                let t = self.advance();
                let raw = self.text(t.span);
                let value = raw[1..raw.len() - 1].to_string();
                items.push(BlockItem {
                    span: t.span,
                    kind: BlockItemKind::EnumVariant {
                        name: Ident { span: t.span, name: value },
                        backtick_quoted: true,
                    },
                });
            } else if let Some(ident) = self.parse_ident_in("enum variant") {
                items.push(BlockItem {
                    span: ident.span,
                    kind: BlockItemKind::EnumVariant { name: ident, backtick_quoted: false },
                });
            } else {
                self.advance(); // skip unrecognised token
            }
        }
        items
    }

    fn parse_anonymous_block(&mut self, kind: BlockKind) -> Option<BlockDecl> {
        let start = self.advance().span;
        self.expect(TokenKind::LBrace)?;
        let items = self.parse_block_items(kind);
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(BlockDecl {
            span: start.merge(end),
            kind,
            name: None,
            items,
        })
    }

    // -- qualified config: `alias/config { ... }` -----------------------

    fn parse_qualified_config(&mut self) -> Option<BlockDecl> {
        let alias = self.parse_ident_in("config qualifier")?;
        let start = alias.span;
        self.expect(TokenKind::Slash)?;
        self.advance(); // consume "config" ident
        self.expect(TokenKind::LBrace)?;
        let items = self.parse_block_items(BlockKind::Config);
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(BlockDecl {
            span: start.merge(end),
            kind: BlockKind::Config,
            name: Some(alias),
            items,
        })
    }

    // -- default declaration -------------------------------------------

    fn parse_default_decl(&mut self) -> Option<DefaultDecl> {
        let start = self.expect(TokenKind::Default)?.span;

        // `default [TypeName] instanceName = value`
        // The type name is optional. If the next two tokens are both words
        // and the second is followed by `=`, the first is the type.
        let (type_name, name) = if self.peek_kind().is_word()
            && self.peek_at(1).kind.is_word()
            && self.peek_at(2).kind == TokenKind::Eq
        {
            let t = self.parse_ident_in("type name")?;
            let n = self.parse_ident_in("default name")?;
            (Some(t), n)
        } else {
            (None, self.parse_ident_in("default name")?)
        };

        self.expect(TokenKind::Eq)?;
        let value = self.parse_expr(0)?;
        Some(DefaultDecl {
            span: start.merge(value.span()),
            type_name,
            name,
            value,
        })
    }

    // -- variant declaration -------------------------------------------

    fn parse_variant_decl(&mut self) -> Option<VariantDecl> {
        let start = self.expect(TokenKind::Variant)?.span;
        let name = self.parse_ident_in("variant name")?;
        self.expect(TokenKind::Colon)?;
        let base = self.parse_expr(0)?;

        let items = if self.eat(TokenKind::LBrace).is_some() {
            let items = self.parse_block_items(BlockKind::Entity);
            self.expect(TokenKind::RBrace)?;
            items
        } else {
            Vec::new()
        };

        let end = if let Some(last) = items.last() {
            last.span
        } else {
            base.span()
        };
        Some(VariantDecl {
            span: start.merge(end),
            name,
            base,
            items,
        })
    }

    // -- deferred declaration ------------------------------------------

    fn parse_deferred_decl(&mut self) -> Option<DeferredDecl> {
        let start = self.expect(TokenKind::Deferred)?.span;
        let path = self.parse_expr(0)?;
        Some(DeferredDecl {
            span: start.merge(path.span()),
            path,
        })
    }

    // -- open question --------------------------------------------------

    fn parse_open_question_decl(&mut self) -> Option<OpenQuestionDecl> {
        let start = self.expect(TokenKind::Open)?.span;
        self.expect(TokenKind::Question)?;
        let text = self.parse_string()?;
        Some(OpenQuestionDecl {
            span: start.merge(text.span),
            text,
        })
    }

    // -- contract declaration -------------------------------------------

    fn parse_contract_decl(&mut self) -> Option<BlockDecl> {
        let start = self.advance().span; // consume `contract`
        let name = self.parse_ident_in("contract name")?;

        // Reject lowercase contract names
        if name.name.chars().next().is_some_and(|c| c.is_lowercase()) {
            self.diagnostics.push(Diagnostic::error(
                name.span,
                "contract name must start with an uppercase letter",
            ));
        }

        // Reject colon-delimited body
        if self.at(TokenKind::Colon) {
            self.error(
                self.peek().span,
                "contract body must use braces { }, not a colon",
            );
            return None;
        }

        self.expect(TokenKind::LBrace)?;
        let items = self.parse_block_items(BlockKind::Contract);
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(BlockDecl {
            span: start.merge(end),
            kind: BlockKind::Contract,
            name: Some(name),
            items,
        })
    }

    // -- invariant declaration ------------------------------------------

    fn parse_invariant_decl(&mut self) -> Option<InvariantDecl> {
        let start = self.advance().span; // consume `invariant`
        let name = self.parse_ident_in("invariant name")?;

        // Reject lowercase invariant names
        if name.name.chars().next().is_some_and(|c| c.is_lowercase()) {
            self.diagnostics.push(Diagnostic::error(
                name.span,
                "invariant name must start with an uppercase letter",
            ));
        }

        self.expect(TokenKind::LBrace)?;
        let body = self.parse_invariant_body()?;
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(InvariantDecl {
            span: start.merge(end),
            name,
            body,
        })
    }

    /// Parse the body of an invariant block — a sequence of expressions and
    /// let bindings, similar to a clause value block.
    fn parse_invariant_body(&mut self) -> Option<Expr> {
        let start = self.peek().span;
        let mut items = Vec::new();

        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            if self.at(TokenKind::Let) {
                let let_start = self.advance().span;
                let name = self.parse_ident_in("binding name")?;
                self.expect(TokenKind::Eq)?;
                let value = self.parse_expr(0)?;
                items.push(Expr::LetExpr {
                    span: let_start.merge(value.span()),
                    name,
                    value: Box::new(value),
                });
            } else if let Some(expr) = self.parse_expr(0) {
                items.push(expr);
            } else {
                self.advance();
                break;
            }
        }

        if items.len() == 1 {
            Some(items.pop().unwrap())
        } else {
            let end = items.last().map(|e| e.span()).unwrap_or(start);
            Some(Expr::Block {
                span: start.merge(end),
                items,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Block item parsing
// ---------------------------------------------------------------------------

impl<'s> Parser<'s> {
    fn parse_block_items(&mut self, block_kind: BlockKind) -> Vec<BlockItem> {
        let mut items = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            if let Some(item) = self.parse_block_item(block_kind) {
                items.push(item);
                self.eat(TokenKind::Comma);
            } else {
                // Recovery: skip one token
                self.advance();
            }
        }
        items
    }

    fn parse_block_item(&mut self, block_kind: BlockKind) -> Option<BlockItem> {
        let start = self.peek().span;

        // `let name = value`
        if self.at(TokenKind::Let) {
            return self.parse_let_item(start);
        }

        // `for binding in collection [where filter]: ...` at block level
        if self.at(TokenKind::For) {
            return self.parse_for_block_item(start);
        }

        // `if condition: ... [else if ...: ...] [else: ...]` at block level
        if self.at(TokenKind::If) {
            return self.parse_if_block_item(start);
        }

        // `@invariant`, `@guidance`, `@guarantee` — prose annotations
        if self.at(TokenKind::At) {
            return self.parse_annotation(start);
        }

        // `invariant Name { expr }` — expression-bearing invariant inside a block
        if self.at(TokenKind::Invariant) && self.peek_at(1).kind.is_word()
            && self.peek_at(2).kind != TokenKind::Colon
        {
            return self.parse_invariant_block_item(start);
        }

        // `open question "text"` (inside a block)
        if self.at(TokenKind::Open) && self.peek_at(1).kind == TokenKind::Question {
            self.advance(); // open
            self.advance(); // question
            let text = self.parse_string()?;
            return Some(BlockItem {
                span: start.merge(text.span),
                kind: BlockItemKind::OpenQuestion { text },
            });
        }

        // `transitions field { ... }` — transition graph (v3)
        if self.at(TokenKind::Transitions)
            && self.peek_at(1).kind.is_word()
            && self.peek_at(2).kind == TokenKind::LBrace
        {
            return self.parse_transitions_block(start);
        }

        // Migration diagnostics: old colon-form prose constructs
        if self.peek_kind() == TokenKind::Ident {
            let word = self.text(self.peek().span);
            if (word == "guidance" || word == "guarantee")
                && self.peek_at(1).kind == TokenKind::Colon
            {
                let kw = word.to_string();
                self.error(
                    self.peek().span,
                    format!(
                        "`{kw}:` syntax was replaced by `@{kw}`. Use `@{kw}` followed by indented comment lines."
                    ),
                );
                // Fall through to normal clause parsing so we don't lose the rest
            }
        }

        // Migration diagnostic: old `invariant:` colon form
        if self.at(TokenKind::Invariant) && self.peek_at(1).kind == TokenKind::Colon {
            self.error(
                self.peek().span,
                "`invariant:` syntax was replaced by `@invariant`. Use `@invariant Name` followed by indented comment lines.",
            );
            // Fall through to normal clause parsing
        }

        // Everything else: `name: value` or `keyword: value` or
        // `name(params): value`
        if self.peek_kind().is_word() {
            // `contracts:` clause — dispatch before generic clause/assignment
            if self.text(self.peek().span) == "contracts"
                && self.peek_at(1).kind == TokenKind::Colon
            {
                return self.parse_contracts_clause(start);
            }

            // `facing name: Type` / `context name: Type [where ...]` — binding
            // clause keywords that don't use `:` after the keyword itself.
            if is_binding_clause_keyword(self.text(self.peek().span))
                && self.peek_at(1).kind.is_word()
                && self.peek_at(2).kind == TokenKind::Colon
            {
                return self.parse_binding_clause_item(start);
            }

            // Check for `Name.field:` — dot-path reverse relationship
            if self.peek_at(1).kind == TokenKind::Dot
                && self.peek_at(2).kind.is_word()
                && self.peek_at(3).kind == TokenKind::Colon
            {
                return self.parse_path_assignment_item(start);
            }

            // Check for `name(` — potential parameterised assignment
            if self.peek_at(1).kind == TokenKind::LParen {
                return self.parse_param_or_clause_item(start);
            }

            // `produces:` and `consumes:` are removed in v3 — emit migration diagnostic
            if block_kind == BlockKind::Rule
                && (self.at(TokenKind::Produces) || self.at(TokenKind::Consumes))
                && self.peek_at(1).kind == TokenKind::Colon
            {
                return self.parse_legacy_field_list_clause(start);
            }

            // Check for `name:` — assignment or clause
            if self.peek_at(1).kind == TokenKind::Colon {
                return self.parse_assign_or_clause_item(start);
            }
        }

        // For clauses whose keyword is a separate TokenKind (when, requires, etc.)
        if token_is_clause_keyword(self.peek_kind()) && self.peek_at(1).kind == TokenKind::Colon {
            return self.parse_assign_or_clause_item(start);
        }

        self.error(
            start,
            format!(
                "expected block item (name: value, let name = value, when:/requires:/ensures: clause, \
                 for ... in ...:, or open question), found {}",
                self.peek_kind(),
            ),
        );
        None
    }

    /// Parse `transitions field { edges..., terminal: states }`.
    fn parse_transitions_block(&mut self, start: Span) -> Option<BlockItem> {
        self.advance(); // consume `transitions`
        let field = self.parse_ident_in("transition field name")?;
        self.expect(TokenKind::LBrace)?;

        let mut edges = Vec::new();
        let mut terminal = Vec::new();

        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            // `terminal: state1, state2`
            if self.at(TokenKind::Terminal) && self.peek_at(1).kind == TokenKind::Colon {
                self.advance(); // consume `terminal`
                self.advance(); // consume `:`
                loop {
                    let state = self.parse_ident_in("terminal state")?;
                    terminal.push(state);
                    if self.eat(TokenKind::Comma).is_none() {
                        break;
                    }
                    // Allow trailing comma before `}`
                    if self.at(TokenKind::RBrace) {
                        break;
                    }
                }
                continue;
            }

            // `from -> to`
            let from = self.parse_ident_in("source state")?;
            if self.expect(TokenKind::ThinArrow).is_none() {
                // Recovery: skip to next line or `}`
                while !self.at(TokenKind::RBrace) && !self.at_eof() {
                    let cur_line = self.line_of(self.peek().span);
                    self.advance();
                    if self.line_of(self.peek().span) != cur_line {
                        break;
                    }
                }
                continue;
            }
            let to = self.parse_ident_in("target state")?;
            let edge_span = from.span.merge(to.span);
            edges.push(TransitionEdge {
                span: edge_span,
                from,
                to,
            });

            // Optional comma between edges
            self.eat(TokenKind::Comma);
        }

        let end = self.expect(TokenKind::RBrace)?.span;

        Some(BlockItem {
            span: start.merge(end),
            kind: BlockItemKind::TransitionsBlock(TransitionGraph {
                span: start.merge(end),
                field,
                edges,
                terminal,
            }),
        })
    }

    /// Parse legacy `produces:` / `consumes:` clauses, emitting a migration warning
    /// and skipping the clause body. Returns `None` so the item is dropped from the AST.
    fn parse_legacy_field_list_clause(&mut self, start: Span) -> Option<BlockItem> {
        let keyword_tok = self.advance(); // consume `produces` or `consumes`
        let keyword = self.text(keyword_tok.span).to_string();
        self.advance(); // consume `:`

        // Skip the comma-separated field names
        let clause_line = self.line_of(start);
        loop {
            if self.at(TokenKind::RBrace) || self.at_eof() {
                break;
            }
            if self.line_of(self.peek().span) > clause_line {
                break;
            }
            self.advance();
        }

        self.diagnostics.push(Diagnostic::warning(
            start.merge(keyword_tok.span),
            format!(
                "`{keyword}:` clauses are removed in v3; use `when` clauses on entity fields instead"
            ),
        ));

        // Return None to drop this item — try the next item in the caller's loop
        self.parse_block_item(BlockKind::Rule)
    }

    fn parse_let_item(&mut self, start: Span) -> Option<BlockItem> {
        self.advance(); // consume `let`
        let name = self.parse_ident_in("binding name")?;
        self.expect(TokenKind::Eq)?;
        let value = self.parse_clause_value(start)?;
        Some(BlockItem {
            span: start.merge(value.span()),
            kind: BlockItemKind::Let { name, value },
        })
    }

    /// Parse `facing name: Type` or `context name: Type [where ...]`.
    /// These keywords don't take `:` after the keyword — they embed a binding directly.
    fn parse_binding_clause_item(&mut self, start: Span) -> Option<BlockItem> {
        let keyword_tok = self.advance(); // consume facing/context
        let keyword = self.text(keyword_tok.span).to_string();
        let binding_name = self.parse_ident_in(&format!("{keyword} binding name"))?;
        self.advance(); // consume ':'
        let type_expr = self.parse_clause_value(start)?;
        let value_span = type_expr.span();
        let value = Expr::Binding {
            span: binding_name.span.merge(value_span),
            name: binding_name,
            value: Box::new(type_expr),
        };
        Some(BlockItem {
            span: start.merge(value_span),
            kind: BlockItemKind::Clause { keyword, value },
        })
    }

    /// Parse `for binding in collection [where filter]:` at block level.
    /// The body is a set of nested block items (let, requires, ensures, etc.).
    fn parse_for_block_item(&mut self, start: Span) -> Option<BlockItem> {
        self.advance(); // consume `for`
        let binding = self.parse_for_binding()?;
        self.expect(TokenKind::In)?;

        let collection = self.parse_expr(BP_WITH_WHERE + 1)?;

        let filter = if self.eat(TokenKind::Where).is_some() {
            // Parse filter at min_bp 0 — colon terminates naturally since
            // it's not an expression operator.
            Some(self.parse_expr(0)?)
        } else {
            None
        };

        self.expect(TokenKind::Colon)?;

        // The body contains nested block items at higher indentation.
        let for_line = self.line_of(start);
        let next_line = self.line_of(self.peek().span);

        let items = if next_line > for_line {
            let base_col = self.col_of(self.peek().span);
            self.parse_indented_block_items(base_col)
        } else {
            // Single-line for: parse one block item
            let mut items = Vec::new();
            if let Some(item) = self.parse_block_item(BlockKind::Entity) {
                items.push(item);
            }
            items
        };

        let end = items
            .last()
            .map(|i| i.span)
            .unwrap_or(start);

        Some(BlockItem {
            span: start.merge(end),
            kind: BlockItemKind::ForBlock {
                binding,
                collection,
                filter,
                items,
            },
        })
    }

    /// Collect block items at column >= `base_col` (for indented for-block bodies).
    fn parse_indented_block_items(&mut self, base_col: u32) -> Vec<BlockItem> {
        let mut items = Vec::new();
        while !self.at_eof()
            && !self.at(TokenKind::RBrace)
            && self.col_of(self.peek().span) >= base_col
        {
            if let Some(item) = self.parse_block_item(BlockKind::Entity) {
                items.push(item);
            } else {
                self.advance();
                break;
            }
        }
        items
    }

    /// Parse `if condition: ... [else if ...: ...] [else: ...]` at block level.
    fn parse_if_block_item(&mut self, start: Span) -> Option<BlockItem> {
        self.advance(); // consume `if`
        let mut branches = Vec::new();

        // First branch
        let condition = self.parse_expr(0)?;
        self.expect(TokenKind::Colon)?;
        let if_line = self.line_of(start);
        let items = self.parse_if_block_body(if_line);
        branches.push(CondBlockBranch {
            span: start.merge(items.last().map(|i| i.span).unwrap_or(start)),
            condition,
            items,
        });

        // else if / else
        let mut else_items = None;
        while self.at(TokenKind::Else) {
            let else_tok = self.advance();
            if self.at(TokenKind::If) {
                let if_start = self.advance().span;
                let cond = self.parse_expr(0)?;
                self.expect(TokenKind::Colon)?;
                let body_items = self.parse_if_block_body(self.line_of(else_tok.span));
                branches.push(CondBlockBranch {
                    span: if_start.merge(body_items.last().map(|i| i.span).unwrap_or(if_start)),
                    condition: cond,
                    items: body_items,
                });
            } else {
                self.expect(TokenKind::Colon)?;
                let body_items = self.parse_if_block_body(self.line_of(else_tok.span));
                else_items = Some(body_items);
                break;
            }
        }

        let end = else_items
            .as_ref()
            .and_then(|items| items.last().map(|i| i.span))
            .or_else(|| branches.last().and_then(|b| b.items.last().map(|i| i.span)))
            .unwrap_or(start);

        Some(BlockItem {
            span: start.merge(end),
            kind: BlockItemKind::IfBlock {
                branches,
                else_items,
            },
        })
    }

    /// Parse the body of an if/else if/else block branch.
    fn parse_if_block_body(&mut self, keyword_line: u32) -> Vec<BlockItem> {
        let next_line = self.line_of(self.peek().span);
        if next_line > keyword_line {
            let base_col = self.col_of(self.peek().span);
            self.parse_indented_block_items(base_col)
        } else {
            // Single-line: parse one block item
            let mut items = Vec::new();
            if let Some(item) = self.parse_block_item(BlockKind::Entity) {
                items.push(item);
            }
            items
        }
    }

    /// Parse `contracts:` clause with indented `demands`/`fulfils` entries.
    fn parse_contracts_clause(&mut self, start: Span) -> Option<BlockItem> {
        self.advance(); // consume `contracts`
        self.advance(); // consume `:`

        let contracts_col = self.col_of(start);
        let mut entries = Vec::new();

        while !self.at_eof()
            && !self.at(TokenKind::RBrace)
            && self.col_of(self.peek().span) > contracts_col
        {
            if !self.peek_kind().is_word() {
                break;
            }

            let entry_start = self.peek().span;
            let direction_tok = self.advance();
            let direction_text = self.text(direction_tok.span);

            let direction = match direction_text {
                "demands" => ContractDirection::Demands,
                "fulfils" => ContractDirection::Fulfils,
                other => {
                    self.error(
                        direction_tok.span,
                        format!(
                            "Unknown direction '{other}' in contracts clause. Use `demands` or `fulfils`."
                        ),
                    );
                    // Skip the rest of this entry
                    if self.peek_kind().is_word() {
                        self.advance();
                    }
                    continue;
                }
            };

            let name = self.parse_ident_in("contract name")?;

            // Reject inline braced blocks
            if self.at(TokenKind::LBrace) {
                self.error(
                    self.peek().span,
                    "Inline contract blocks are not allowed in `contracts:`. Declare the contract at module level.",
                );
                return None;
            }

            let end = name.span;
            entries.push(ContractBinding {
                direction,
                name,
                span: entry_start.merge(end),
            });
        }

        if entries.is_empty() {
            self.error(
                start,
                "Empty `contracts:` clause. Add at least one `demands` or `fulfils` entry.",
            );
            return None;
        }

        let end = entries.last().unwrap().span;
        Some(BlockItem {
            span: start.merge(end),
            kind: BlockItemKind::ContractsClause { entries },
        })
    }

    /// Parse `@invariant Name`, `@guidance`, or `@guarantee Name` with comment body.
    fn parse_annotation(&mut self, start: Span) -> Option<BlockItem> {
        let at_tok = self.advance(); // consume `@`
        let at_col = self.col_of(at_tok.span);

        if !self.peek_kind().is_word() {
            self.error(
                self.peek().span,
                format!("expected annotation keyword after `@`, found {}", self.peek_kind()),
            );
            return None;
        }

        let keyword_tok = self.advance();
        let keyword_text = self.text(keyword_tok.span);

        let kind = match keyword_text {
            "invariant" => AnnotationKind::Invariant,
            "guidance" => AnnotationKind::Guidance,
            "guarantee" => AnnotationKind::Guarantee,
            other => {
                self.error(
                    keyword_tok.span,
                    format!(
                        "Unknown annotation `@{other}`. Use `@invariant`, `@guidance` or `@guarantee`."
                    ),
                );
                return None;
            }
        };

        // Parse optional name
        let name = match &kind {
            AnnotationKind::Invariant | AnnotationKind::Guarantee => {
                let n = self.parse_ident_in("annotation name")?;
                if n.name.chars().next().is_some_and(|c| c.is_lowercase()) {
                    self.diagnostics.push(Diagnostic::error(
                        n.span,
                        "Annotation names must be PascalCase.",
                    ));
                }
                Some(n)
            }
            AnnotationKind::Guidance => {
                // Reject name after @guidance
                if self.peek_kind().is_word()
                    && self.line_of(self.peek().span) == self.line_of(keyword_tok.span)
                {
                    self.error(
                        self.peek().span,
                        "`@guidance` does not take a name. Remove the name after `@guidance`.",
                    );
                    return None;
                }
                None
            }
        };

        // Parse comment body from source lines.
        // The last consumed token tells us which line the annotation header is on.
        let last_header_span = name.as_ref().map(|n| n.span).unwrap_or(keyword_tok.span);
        let header_line = self.line_of(last_header_span);
        let body = self.parse_annotation_body(at_col, header_line);

        if body.is_empty() {
            self.error(
                last_header_span,
                "Annotations must be followed by at least one indented comment line.",
            );
            return None;
        }

        Some(BlockItem {
            span: start.merge(last_header_span),
            kind: BlockItemKind::Annotation(Annotation {
                kind,
                name,
                body,
                span: start.merge(last_header_span),
            }),
        })
    }

    /// Scan source lines for indented `-- ` comment lines forming an annotation body.
    /// Starts from the line after `header_line` and collects lines indented deeper
    /// than `at_col`.
    fn parse_annotation_body(&self, at_col: u32, header_line: u32) -> Vec<String> {
        let mut body = Vec::new();
        let lines: Vec<&str> = self.source.lines().collect();
        let mut line_idx = (header_line + 1) as usize;

        while line_idx < lines.len() {
            let line = lines[line_idx];
            let trimmed = line.trim_start();

            if trimmed.is_empty() {
                if !body.is_empty() {
                    body.push(String::new());
                }
                line_idx += 1;
                continue;
            }

            let indent = (line.len() - trimmed.len()) as u32;
            if indent <= at_col {
                break;
            }

            if let Some(comment) = trimmed.strip_prefix("-- ") {
                body.push(comment.to_string());
            } else if trimmed == "--" {
                body.push(String::new());
            } else {
                break;
            }

            line_idx += 1;
        }

        // Trim trailing blank lines
        while body.last().is_some_and(|l| l.is_empty()) {
            body.pop();
        }

        body
    }

    /// Parse `invariant Name { expr }` inside a block (entity-level).
    fn parse_invariant_block_item(&mut self, start: Span) -> Option<BlockItem> {
        self.advance(); // consume `invariant`
        let name = self.parse_ident_in("invariant name")?;

        // Reject lowercase invariant names
        if name.name.chars().next().is_some_and(|c| c.is_lowercase()) {
            self.diagnostics.push(Diagnostic::error(
                name.span,
                "invariant name must start with an uppercase letter",
            ));
        }

        self.expect(TokenKind::LBrace)?;
        let body = self.parse_invariant_body()?;
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(BlockItem {
            span: start.merge(end),
            kind: BlockItemKind::InvariantBlock { name, body },
        })
    }

    fn parse_assign_or_clause_item(&mut self, start: Span) -> Option<BlockItem> {
        let name_tok = self.advance(); // consume name/keyword
        let name_text = self.text(name_tok.span).to_string();
        self.advance(); // consume ':'

        let allows_binding = clause_allows_binding(&name_text);
        let value = self.parse_clause_value_maybe_binding(start, allows_binding)?;
        let value_span = value.span();

        let kind = if is_clause_keyword(&name_text) {
            BlockItemKind::Clause {
                keyword: name_text,
                value,
            }
        } else if let Some((inner_value, when_clause)) = extract_when_clause(&value) {
            BlockItemKind::FieldWithWhen {
                name: Ident {
                    span: name_tok.span,
                    name: name_text,
                },
                value: inner_value,
                when_clause,
            }
        } else {
            BlockItemKind::Assignment {
                name: Ident {
                    span: name_tok.span,
                    name: name_text,
                },
                value,
            }
        };

        Some(BlockItem {
            span: start.merge(value_span),
            kind,
        })
    }

    /// Parse `Entity.field: value` — a dot-path reverse relationship declaration.
    fn parse_path_assignment_item(&mut self, start: Span) -> Option<BlockItem> {
        let obj_tok = self.advance(); // consume first ident
        self.advance(); // consume '.'
        let field = self.parse_ident_in("field name")?;
        self.advance(); // consume ':'

        let path = Expr::MemberAccess {
            span: obj_tok.span.merge(field.span),
            object: Box::new(Expr::Ident(Ident {
                span: obj_tok.span,
                name: self.text(obj_tok.span).to_string(),
            })),
            field,
        };

        let value = self.parse_clause_value(start)?;
        let value_span = value.span();
        Some(BlockItem {
            span: start.merge(value_span),
            kind: BlockItemKind::PathAssignment { path, value },
        })
    }

    fn parse_param_or_clause_item(&mut self, start: Span) -> Option<BlockItem> {
        // Could be `name(params): value` (param assignment) or
        // `name(args)` which is an expression that happens to start a clause
        // value. Peek far enough to see if `)` is followed by `:`.
        let saved_pos = self.pos;
        let _name_tok = self.advance();
        self.advance(); // (

        // Try to scan past balanced parens
        let mut depth = 1u32;
        while !self.at_eof() && depth > 0 {
            match self.peek_kind() {
                TokenKind::LParen => {
                    depth += 1;
                    self.advance();
                }
                TokenKind::RParen => {
                    depth -= 1;
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }

        if self.at(TokenKind::Colon) {
            // It's a parameterised assignment: restore and parse properly
            self.pos = saved_pos;
            let name = self.parse_ident_in("derived value name")?;
            self.expect(TokenKind::LParen)?;
            let params = self.parse_ident_list()?;
            self.expect(TokenKind::RParen)?;
            self.expect(TokenKind::Colon)?;
            let value = self.parse_clause_value(start)?;
            Some(BlockItem {
                span: start.merge(value.span()),
                kind: BlockItemKind::ParamAssignment {
                    name,
                    params,
                    value,
                },
            })
        } else {
            // Not a param assignment — restore and fall through to assignment
            self.pos = saved_pos;
            // Check for regular `name: value`
            if self.peek_at(1).kind == TokenKind::Colon {
                // Nope, the (1) is LParen not Colon. Re-examine.
            }
            // Fall back: treat as `name: value` where value starts with a call
            self.parse_assign_or_clause_item(start)
        }
    }

    fn parse_ident_list(&mut self) -> Option<Vec<Ident>> {
        let mut params = Vec::new();
        if !self.at(TokenKind::RParen) {
            params.push(self.parse_ident_in("parameter name")?);
            while self.eat(TokenKind::Comma).is_some() {
                params.push(self.parse_ident_in("parameter name")?);
            }
        }
        Some(params)
    }

    /// Parse a for-loop binding: either a single ident or `(a, b)` destructuring.
    fn parse_for_binding(&mut self) -> Option<ForBinding> {
        if self.at(TokenKind::LParen) {
            let start = self.advance().span; // consume '('
            let mut idents = Vec::new();
            idents.push(self.parse_ident_in("loop variable")?);
            while self.eat(TokenKind::Comma).is_some() {
                idents.push(self.parse_ident_in("loop variable")?);
            }
            let end = self.expect(TokenKind::RParen)?.span;
            Some(ForBinding::Destructured(idents, start.merge(end)))
        } else {
            let ident = self.parse_ident_in("loop variable")?;
            Some(ForBinding::Single(ident))
        }
    }

    /// Parse a clause value, optionally checking for a `name: expr` binding
    /// pattern at the start. Used for when, facing and context clauses where
    /// the first `ident:` is a binding rather than a nested assignment.
    fn parse_clause_value_maybe_binding(
        &mut self,
        clause_start: Span,
        allow_binding: bool,
    ) -> Option<Expr> {
        if allow_binding
            && self.peek_kind().is_word()
            && self.peek_at(1).kind == TokenKind::Colon
        {
            // Check this isn't at the start of a new block item on the next line.
            // Bindings only apply on the same line or the immediate indented value.
            let clause_line = self.line_of(clause_start);
            let next_line = self.line_of(self.peek().span);
            let colon_is_block_item = next_line > clause_line
                && self.peek_at(2).kind != TokenKind::Eof
                && self.line_of(self.peek_at(2).span) == next_line;

            if next_line == clause_line || colon_is_block_item {
                let name = self.parse_ident_in("binding name")?;
                self.advance(); // consume ':'
                let inner = self.parse_clause_value(clause_start)?;
                return Some(Expr::Binding {
                    span: name.span.merge(inner.span()),
                    name,
                    value: Box::new(inner),
                });
            }
        }
        self.parse_clause_value(clause_start)
    }

    /// Parse a clause value. If the next token is on a new line (indented),
    /// collect a multi-line block. Otherwise parse a single expression.
    fn parse_clause_value(&mut self, clause_start: Span) -> Option<Expr> {
        let clause_line = self.line_of(clause_start);
        let next = self.peek();
        let next_line = self.line_of(next.span);

        if next_line > clause_line {
            // Multi-line block — but only if the next token is actually
            // indented past the clause keyword. When a clause has only a
            // comment as its value (stripped by the lexer), the next visible
            // token is a sibling at the same indentation.
            let base_col = self.col_of(next.span);
            let clause_col = self.col_of(clause_start);
            if base_col <= clause_col {
                return Some(Expr::Block {
                    span: clause_start,
                    items: Vec::new(),
                });
            }
            self.parse_indented_block(base_col)
        } else {
            // Single-line clause value
            self.parse_expr(0)
        }
    }

    /// Collect expressions that start at column >= `base_col` into a block.
    /// Also handles `let name = value` bindings inside clause value blocks.
    fn parse_indented_block(&mut self, base_col: u32) -> Option<Expr> {
        let start = self.peek().span;
        let mut items = Vec::new();

        while !self.at_eof()
            && !self.at(TokenKind::RBrace)
            && self.col_of(self.peek().span) >= base_col
        {
            // Handle `let name = value` inside expression blocks
            if self.at(TokenKind::Let) {
                let let_start = self.advance().span;
                if let Some(name) = self.parse_ident_in("binding name") {
                    if self.expect(TokenKind::Eq).is_some() {
                        if let Some(value) = self.parse_expr(0) {
                            items.push(Expr::LetExpr {
                                span: let_start.merge(value.span()),
                                name,
                                value: Box::new(value),
                            });
                            continue;
                        }
                    }
                }
                break;
            }

            if let Some(expr) = self.parse_expr(0) {
                items.push(expr);
            } else {
                self.advance();
                break;
            }
        }

        if items.len() == 1 {
            Some(items.pop().unwrap())
        } else {
            let end = items.last().map(|e| e.span()).unwrap_or(start);
            Some(Expr::Block {
                span: start.merge(end),
                items,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Expression parsing — Pratt parser
// ---------------------------------------------------------------------------

// Binding powers (even = left, odd = right for right-associative)
const BP_LAMBDA: u8 = 4;
const BP_WHEN_GUARD: u8 = 5;
const BP_PROJECTION: u8 = 6;
const BP_WITH_WHERE: u8 = 7;
const BP_IMPLIES: u8 = 8;
const BP_OR: u8 = 10;
const BP_AND: u8 = 20;
const BP_COMPARE: u8 = 30;
const BP_TRANSITION: u8 = 32;
const BP_NULL_COALESCE: u8 = 40;
const BP_ADD: u8 = 50;
const BP_MUL: u8 = 60;
const BP_PIPE: u8 = 65;
const BP_PREFIX: u8 = 70;
const BP_POSTFIX: u8 = 80;

impl<'s> Parser<'s> {
    pub fn parse_expr(&mut self, min_bp: u8) -> Option<Expr> {
        let mut lhs = self.parse_prefix()?;

        loop {
            if let Some((l_bp, r_bp)) = self.infix_bp() {
                if l_bp < min_bp {
                    break;
                }
                lhs = self.parse_infix(lhs, r_bp)?;
            } else if let Some(l_bp) = self.postfix_bp() {
                if l_bp < min_bp {
                    break;
                }
                lhs = self.parse_postfix(lhs)?;
            } else {
                break;
            }
        }

        Some(lhs)
    }

    // -- prefix ---------------------------------------------------------

    fn parse_prefix(&mut self) -> Option<Expr> {
        match self.peek_kind() {
            TokenKind::Not => {
                let start = self.advance().span;
                if self.at(TokenKind::Exists) {
                    self.advance();
                    let operand = self.parse_expr(BP_PREFIX)?;
                    Some(Expr::NotExists {
                        span: start.merge(operand.span()),
                        operand: Box::new(operand),
                    })
                } else {
                    let operand = self.parse_expr(BP_PREFIX)?;
                    Some(Expr::Not {
                        span: start.merge(operand.span()),
                        operand: Box::new(operand),
                    })
                }
            }
            TokenKind::Exists => {
                // When `exists` is not followed by an expression-start token,
                // treat it as a plain identifier (e.g. `label: exists`).
                let next = self.peek_at(1).kind;
                if matches!(
                    next,
                    TokenKind::RParen
                        | TokenKind::RBrace
                        | TokenKind::RBracket
                        | TokenKind::Comma
                        | TokenKind::Eof
                ) {
                    let id = self.parse_ident()?;
                    return Some(Expr::Ident(id));
                }
                let start = self.advance().span;
                let operand = self.parse_expr(BP_PREFIX)?;
                Some(Expr::Exists {
                    span: start.merge(operand.span()),
                    operand: Box::new(operand),
                })
            }
            TokenKind::If => self.parse_if_expr(),
            TokenKind::For => self.parse_for_expr(),
            TokenKind::LBrace => self.parse_brace_expr(),
            TokenKind::LBracket => {
                let t = self.advance();
                self.error(t.span, "list literals `[...]` are not supported; use `Set<T>` type annotation or `{...}` set literal");
                None
            }
            TokenKind::LParen => self.parse_paren_expr(),
            TokenKind::Number => {
                let t = self.advance();
                Some(Expr::NumberLiteral {
                    span: t.span,
                    value: self.text(t.span).to_string(),
                })
            }
            TokenKind::Duration => {
                let t = self.advance();
                Some(Expr::DurationLiteral {
                    span: t.span,
                    value: self.text(t.span).to_string(),
                })
            }
            TokenKind::String => {
                let sl = self.parse_string()?;
                Some(Expr::StringLiteral(sl))
            }
            TokenKind::BacktickLiteral => {
                let t = self.advance();
                let raw = self.text(t.span);
                // Strip surrounding backticks
                let value = raw[1..raw.len() - 1].to_string();
                Some(Expr::BacktickLiteral {
                    span: t.span,
                    value,
                })
            }
            TokenKind::True => {
                let t = self.advance();
                Some(Expr::BoolLiteral {
                    span: t.span,
                    value: true,
                })
            }
            TokenKind::False => {
                let t = self.advance();
                Some(Expr::BoolLiteral {
                    span: t.span,
                    value: false,
                })
            }
            TokenKind::Null => {
                let t = self.advance();
                Some(Expr::Null { span: t.span })
            }
            TokenKind::Now => {
                let t = self.advance();
                Some(Expr::Now { span: t.span })
            }
            TokenKind::This => {
                let t = self.advance();
                Some(Expr::This { span: t.span })
            }
            TokenKind::Within => {
                let t = self.advance();
                Some(Expr::Within { span: t.span })
            }
            k if k.is_word() => {
                let id = self.parse_ident()?;
                Some(Expr::Ident(id))
            }
            TokenKind::Star => {
                // Wildcard `*` in type position (e.g. `Codec<*>`)
                let t = self.advance();
                Some(Expr::Ident(Ident {
                    span: t.span,
                    name: "*".into(),
                }))
            }
            TokenKind::Minus => {
                // Unary minus: -expr → BinaryOp(0, Sub, expr)
                let start = self.advance().span;
                let operand = self.parse_expr(BP_PREFIX)?;
                Some(Expr::BinaryOp {
                    span: start.merge(operand.span()),
                    left: Box::new(Expr::NumberLiteral {
                        span: start,
                        value: "0".into(),
                    }),
                    op: BinaryOp::Sub,
                    right: Box::new(operand),
                })
            }
            _ => {
                self.error(
                    self.peek().span,
                    format!(
                        "expected expression (identifier, number, string, true/false, null, \
                         if/for/not/exists, '(', '{{', '['), found {}",
                        self.peek_kind(),
                    ),
                );
                None
            }
        }
    }

    // -- infix binding powers -------------------------------------------

    fn infix_bp(&self) -> Option<(u8, u8)> {
        match self.peek_kind() {
            TokenKind::FatArrow => Some((BP_LAMBDA, BP_LAMBDA - 1)), // right-assoc
            // `when` as an inline guard on provides/related items
            TokenKind::When => Some((BP_WHEN_GUARD, BP_WHEN_GUARD + 1)),
            TokenKind::Pipe => Some((BP_PIPE, BP_PIPE + 1)),
            TokenKind::Implies => Some((BP_IMPLIES, BP_IMPLIES - 1)), // right-assoc
            TokenKind::Or => Some((BP_OR, BP_OR + 1)),
            TokenKind::And => Some((BP_AND, BP_AND + 1)),
            TokenKind::Eq | TokenKind::BangEq => {
                Some((BP_COMPARE, BP_COMPARE + 1))
            }
            TokenKind::Lt => {
                // If `<` is immediately adjacent to a word token (no space),
                // treat as generic type postfix, not comparison infix.
                if self.pos > 0 {
                    let prev = self.tokens[self.pos - 1];
                    if prev.span.end == self.peek().span.start && prev.kind.is_word() {
                        return None;
                    }
                }
                Some((BP_COMPARE, BP_COMPARE + 1))
            }
            TokenKind::LtEq | TokenKind::Gt | TokenKind::GtEq => {
                Some((BP_COMPARE, BP_COMPARE + 1))
            }
            TokenKind::In => Some((BP_COMPARE, BP_COMPARE + 1)),
            // `not in` — only when followed by `in`
            TokenKind::Not if self.peek_at(1).kind == TokenKind::In => {
                Some((BP_COMPARE, BP_COMPARE + 1))
            }
            TokenKind::TransitionsTo => Some((BP_TRANSITION, BP_TRANSITION + 1)),
            TokenKind::Becomes => Some((BP_TRANSITION, BP_TRANSITION + 1)),
            TokenKind::Where => Some((BP_WITH_WHERE, BP_WITH_WHERE + 1)),
            TokenKind::With => Some((BP_WITH_WHERE, BP_WITH_WHERE + 1)),
            TokenKind::ThinArrow => Some((BP_PROJECTION, BP_PROJECTION + 1)),
            TokenKind::QuestionQuestion => Some((BP_NULL_COALESCE, BP_NULL_COALESCE + 1)),
            TokenKind::Plus | TokenKind::Minus => Some((BP_ADD, BP_ADD + 1)),
            TokenKind::Star | TokenKind::Slash => Some((BP_MUL, BP_MUL + 1)),
            _ => None,
        }
    }

    fn parse_infix(&mut self, lhs: Expr, r_bp: u8) -> Option<Expr> {
        let op_tok = self.advance();
        match op_tok.kind {
            TokenKind::FatArrow => {
                let body = self.parse_expr(r_bp)?;
                Some(Expr::Lambda {
                    span: lhs.span().merge(body.span()),
                    param: Box::new(lhs),
                    body: Box::new(body),
                })
            }
            TokenKind::Pipe => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::Pipe {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                })
            }
            TokenKind::Implies => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::LogicalOp {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: LogicalOp::Implies,
                    right: Box::new(rhs),
                })
            }
            TokenKind::Or => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::LogicalOp {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: LogicalOp::Or,
                    right: Box::new(rhs),
                })
            }
            TokenKind::And => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::LogicalOp {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: LogicalOp::And,
                    right: Box::new(rhs),
                })
            }
            TokenKind::Eq => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::Comparison {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: ComparisonOp::Eq,
                    right: Box::new(rhs),
                })
            }
            TokenKind::BangEq => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::Comparison {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: ComparisonOp::NotEq,
                    right: Box::new(rhs),
                })
            }
            TokenKind::Lt => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::Comparison {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: ComparisonOp::Lt,
                    right: Box::new(rhs),
                })
            }
            TokenKind::LtEq => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::Comparison {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: ComparisonOp::LtEq,
                    right: Box::new(rhs),
                })
            }
            TokenKind::Gt => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::Comparison {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: ComparisonOp::Gt,
                    right: Box::new(rhs),
                })
            }
            TokenKind::GtEq => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::Comparison {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: ComparisonOp::GtEq,
                    right: Box::new(rhs),
                })
            }
            TokenKind::In => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::In {
                    span: lhs.span().merge(rhs.span()),
                    element: Box::new(lhs),
                    collection: Box::new(rhs),
                })
            }
            TokenKind::Not => {
                // `not in`
                self.expect(TokenKind::In)?;
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::NotIn {
                    span: lhs.span().merge(rhs.span()),
                    element: Box::new(lhs),
                    collection: Box::new(rhs),
                })
            }
            TokenKind::Where => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::Where {
                    span: lhs.span().merge(rhs.span()),
                    source: Box::new(lhs),
                    condition: Box::new(rhs),
                })
            }
            TokenKind::With => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::With {
                    span: lhs.span().merge(rhs.span()),
                    source: Box::new(lhs),
                    predicate: Box::new(rhs),
                })
            }
            TokenKind::QuestionQuestion => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::NullCoalesce {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                })
            }
            TokenKind::Plus => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::BinaryOp {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: BinaryOp::Add,
                    right: Box::new(rhs),
                })
            }
            TokenKind::Minus => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::BinaryOp {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: BinaryOp::Sub,
                    right: Box::new(rhs),
                })
            }
            TokenKind::Star => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::BinaryOp {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: BinaryOp::Mul,
                    right: Box::new(rhs),
                })
            }
            TokenKind::Slash => {
                // Check for qualified name: `alias/Name` or `alias/config`
                // Qualified if the LHS is a bare identifier and the RHS is a
                // word that either starts with uppercase or is a block keyword
                // (like `config`).
                if let Expr::Ident(ref id) = lhs {
                    if self.peek_kind().is_word() {
                        let next_text = self.text(self.peek().span);
                        let is_qualified = next_text
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_uppercase())
                            || matches!(
                                self.peek_kind(),
                                TokenKind::Config | TokenKind::Entity | TokenKind::Value
                            );
                        if is_qualified {
                            let name_tok = self.advance();
                            return Some(Expr::QualifiedName(QualifiedName {
                                span: lhs.span().merge(name_tok.span),
                                qualifier: Some(id.name.clone()),
                                name: self.text(name_tok.span).to_string(),
                            }));
                        }
                    }
                }
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::BinaryOp {
                    span: lhs.span().merge(rhs.span()),
                    left: Box::new(lhs),
                    op: BinaryOp::Div,
                    right: Box::new(rhs),
                })
            }
            TokenKind::ThinArrow => {
                let field = self.parse_ident_in("projection field")?;
                Some(Expr::ProjectionMap {
                    span: lhs.span().merge(field.span),
                    source: Box::new(lhs),
                    field,
                })
            }
            TokenKind::TransitionsTo => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::TransitionsTo {
                    span: lhs.span().merge(rhs.span()),
                    subject: Box::new(lhs),
                    new_state: Box::new(rhs),
                })
            }
            TokenKind::Becomes => {
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::Becomes {
                    span: lhs.span().merge(rhs.span()),
                    subject: Box::new(lhs),
                    new_state: Box::new(rhs),
                })
            }
            TokenKind::When => {
                // Inline guard: `action when condition`
                let rhs = self.parse_expr(r_bp)?;
                Some(Expr::WhenGuard {
                    span: lhs.span().merge(rhs.span()),
                    action: Box::new(lhs),
                    condition: Box::new(rhs),
                })
            }
            _ => {
                self.error(
                    op_tok.span,
                    format!("unexpected infix operator {}", op_tok.kind),
                );
                None
            }
        }
    }

    // -- postfix --------------------------------------------------------

    fn postfix_bp(&self) -> Option<u8> {
        match self.peek_kind() {
            TokenKind::Dot | TokenKind::QuestionDot => Some(BP_POSTFIX),
            TokenKind::QuestionMark => Some(BP_POSTFIX),
            // `<` for generic types like `Set<T>`, `List<T>` — only treated
            // as postfix when it immediately follows a word with no space.
            TokenKind::Lt => {
                if self.pos > 0 {
                    let prev = self.tokens[self.pos - 1];
                    // Only if `<` starts immediately after the previous token
                    // (no whitespace gap) to distinguish from comparisons.
                    if prev.span.end == self.peek().span.start && prev.kind.is_word() {
                        return Some(BP_POSTFIX);
                    }
                }
                None
            }
            TokenKind::LParen => Some(BP_POSTFIX),
            TokenKind::LBrace => {
                // Join lookup: only when preceded by something that looks
                // like an entity name (handled generically — any expr can
                // be followed by { for join lookup in expression position).
                // But only if the { is on the same line to avoid consuming
                // a block body.
                let next = self.peek();
                let prev_end = if self.pos > 0 {
                    self.tokens[self.pos - 1].span.end
                } else {
                    0
                };
                // Same line check
                if self.line_of(Span::new(prev_end, prev_end))
                    == self.line_of(next.span)
                {
                    Some(BP_POSTFIX)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn parse_postfix(&mut self, lhs: Expr) -> Option<Expr> {
        match self.peek_kind() {
            TokenKind::QuestionMark => {
                let end = self.advance().span;
                Some(Expr::TypeOptional {
                    span: lhs.span().merge(end),
                    inner: Box::new(lhs),
                })
            }
            TokenKind::Lt => {
                // Generic type: `Set<T>`, `List<Node?>`
                self.advance(); // consume <
                let mut args = Vec::new();
                // Parse args above comparison BP so `>` isn't consumed as infix
                while !self.at(TokenKind::Gt) && !self.at_eof() {
                    args.push(self.parse_expr(BP_COMPARE + 1)?);
                    self.eat(TokenKind::Comma);
                }
                let end = self.expect(TokenKind::Gt)?.span;
                Some(Expr::GenericType {
                    span: lhs.span().merge(end),
                    name: Box::new(lhs),
                    args,
                })
            }
            TokenKind::Dot => {
                self.advance();
                let field = self.parse_ident_in("field name")?;
                Some(Expr::MemberAccess {
                    span: lhs.span().merge(field.span),
                    object: Box::new(lhs),
                    field,
                })
            }
            TokenKind::QuestionDot => {
                self.advance();
                let field = self.parse_ident_in("field name")?;
                Some(Expr::OptionalAccess {
                    span: lhs.span().merge(field.span),
                    object: Box::new(lhs),
                    field,
                })
            }
            TokenKind::LParen => {
                self.advance();
                let args = self.parse_call_args()?;
                let end = self.expect(TokenKind::RParen)?.span;
                Some(Expr::Call {
                    span: lhs.span().merge(end),
                    function: Box::new(lhs),
                    args,
                })
            }
            TokenKind::LBrace => {
                self.advance();
                let fields = self.parse_join_fields()?;
                let end = self.expect(TokenKind::RBrace)?.span;
                Some(Expr::JoinLookup {
                    span: lhs.span().merge(end),
                    entity: Box::new(lhs),
                    fields,
                })
            }
            _ => None,
        }
    }

    // -- call arguments -------------------------------------------------

    fn parse_call_args(&mut self) -> Option<Vec<CallArg>> {
        let mut args = Vec::new();
        while !self.at(TokenKind::RParen) && !self.at_eof() {
            // Check for named argument: `name: value`
            if self.peek_kind().is_word() && self.peek_at(1).kind == TokenKind::Colon {
                let name = self.parse_ident_in("argument name")?;
                self.advance(); // :
                let value = self.parse_expr(0)?;
                args.push(CallArg::Named(NamedArg {
                    span: name.span.merge(value.span()),
                    name,
                    value,
                }));
            } else {
                let expr = self.parse_expr(0)?;
                args.push(CallArg::Positional(expr));
            }
            self.eat(TokenKind::Comma);
        }
        Some(args)
    }

    // -- join fields ----------------------------------------------------

    fn parse_join_fields(&mut self) -> Option<Vec<JoinField>> {
        let mut fields = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            let field = self.parse_ident_in("join field name")?;
            let value = if self.eat(TokenKind::Colon).is_some() {
                Some(self.parse_expr(0)?)
            } else {
                None
            };
            fields.push(JoinField {
                span: field.span.merge(
                    value
                        .as_ref()
                        .map(|v| v.span())
                        .unwrap_or(field.span),
                ),
                field,
                value,
            });
            self.eat(TokenKind::Comma);
        }
        Some(fields)
    }

    // -- if expression --------------------------------------------------

    fn parse_if_expr(&mut self) -> Option<Expr> {
        let start = self.advance().span; // consume `if`
        let mut branches = Vec::new();

        // First branch
        let condition = self.parse_expr(0)?;
        self.expect(TokenKind::Colon)?;
        let body = self.parse_branch_body(start)?;
        branches.push(CondBranch {
            span: start.merge(body.span()),
            condition,
            body,
        });

        // else if / else
        let mut else_body = None;
        while self.at(TokenKind::Else) {
            let else_tok = self.advance();
            if self.at(TokenKind::If) {
                let if_start = self.advance().span;
                let cond = self.parse_expr(0)?;
                self.expect(TokenKind::Colon)?;
                let body = self.parse_branch_body(else_tok.span)?;
                branches.push(CondBranch {
                    span: if_start.merge(body.span()),
                    condition: cond,
                    body,
                });
            } else {
                self.expect(TokenKind::Colon)?;
                let body = self.parse_branch_body(else_tok.span)?;
                else_body = Some(Box::new(body));
                break;
            }
        }

        let end = else_body
            .as_ref()
            .map(|b| b.span())
            .or_else(|| branches.last().map(|b| b.body.span()))
            .unwrap_or(start);

        Some(Expr::Conditional {
            span: start.merge(end),
            branches,
            else_body,
        })
    }

    fn parse_branch_body(&mut self, keyword_span: Span) -> Option<Expr> {
        let keyword_line = self.line_of(keyword_span);
        let next_line = self.line_of(self.peek().span);

        if next_line > keyword_line {
            let base_col = self.col_of(self.peek().span);
            self.parse_indented_block(base_col)
        } else {
            self.parse_expr(0)
        }
    }

    // -- for expression -------------------------------------------------

    fn parse_for_expr(&mut self) -> Option<Expr> {
        let start = self.advance().span; // consume `for`
        let binding = self.parse_for_binding()?;
        self.expect(TokenKind::In)?;

        // Parse collection, stopping before `where` and `:`
        let collection = self.parse_expr(BP_WITH_WHERE + 1)?;

        let filter = if self.eat(TokenKind::Where).is_some() {
            // Parse filter at min_bp 0 — colon terminates naturally.
            Some(Box::new(self.parse_expr(0)?))
        } else {
            None
        };

        self.expect(TokenKind::Colon)?;
        let body = self.parse_branch_body(start)?;

        Some(Expr::For {
            span: start.merge(body.span()),
            binding,
            collection: Box::new(collection),
            filter,
            body: Box::new(body),
        })
    }

    // -- brace expressions: set literal or object literal ---------------

    fn parse_brace_expr(&mut self) -> Option<Expr> {
        let start = self.advance().span; // consume {

        if self.at(TokenKind::RBrace) {
            let end = self.advance().span;
            return Some(Expr::SetLiteral {
                span: start.merge(end),
                elements: Vec::new(),
            });
        }

        // Peek: if first item is `ident:`, it's an object literal
        if self.peek_kind().is_word() && self.peek_at(1).kind == TokenKind::Colon {
            return self.parse_object_literal(start);
        }

        // Otherwise set literal
        self.parse_set_literal(start)
    }


    fn parse_object_literal(&mut self, start: Span) -> Option<Expr> {
        let mut fields = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            let name = self.parse_ident_in("field name")?;
            self.expect(TokenKind::Colon)?;
            let value = self.parse_expr(0)?;
            fields.push(NamedArg {
                span: name.span.merge(value.span()),
                name,
                value,
            });
            self.eat(TokenKind::Comma);
        }
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(Expr::ObjectLiteral {
            span: start.merge(end),
            fields,
        })
    }

    fn parse_set_literal(&mut self, start: Span) -> Option<Expr> {
        let mut elements = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            elements.push(self.parse_expr(0)?);
            self.eat(TokenKind::Comma);
        }
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(Expr::SetLiteral {
            span: start.merge(end),
            elements,
        })
    }

    // -- parenthesised expression ---------------------------------------

    fn parse_paren_expr(&mut self) -> Option<Expr> {
        let start = self.advance().span; // (

        // Detect `(name: expr, ...)` — typed signature parameters.
        if self.peek_kind().is_word() && self.peek_at(1).kind == TokenKind::Colon {
            let mut bindings = Vec::new();
            while !self.at(TokenKind::RParen) && !self.at_eof() {
                let name = self.parse_ident_in("parameter name")?;
                self.expect(TokenKind::Colon)?;
                let value = self.parse_expr(0)?;
                bindings.push(Expr::Binding {
                    span: name.span.merge(value.span()),
                    name,
                    value: Box::new(value),
                });
                self.eat(TokenKind::Comma);
            }
            self.expect(TokenKind::RParen)?;
            if bindings.len() == 1 {
                return Some(bindings.into_iter().next().unwrap());
            }
            let span = start.merge(bindings.last().unwrap().span());
            return Some(Expr::Block {
                span,
                items: bindings,
            });
        }

        let expr = self.parse_expr(0)?;
        self.expect(TokenKind::RParen)?;
        Some(expr)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;

    fn parse_ok(src: &str) -> ParseResult {
        // Prefix with version marker if not already present, to avoid
        // spurious "missing version marker" warnings in every test.
        let owned;
        let input = if src.starts_with("-- allium:") {
            src
        } else {
            owned = format!("-- allium: 1\n{src}");
            &owned
        };
        let result = parse(input);
        if !result.diagnostics.is_empty() {
            for d in &result.diagnostics {
                eprintln!(
                    "  [{:?}] {} ({}..{})",
                    d.severity, d.message, d.span.start, d.span.end
                );
            }
        }
        result
    }

    #[test]
    fn version_marker() {
        let r = parse_ok("-- allium: 1\n");
        assert_eq!(r.module.version, Some(1));
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn version_missing_warns() {
        let r = parse("entity User {}");
        assert_eq!(r.module.version, None);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Warning);
        assert!(r.diagnostics[0].message.contains("missing version marker"), "got: {}", r.diagnostics[0].message);
    }

    #[test]
    fn version_unsupported_errors() {
        let r = parse("-- allium: 99\nentity User {}");
        assert_eq!(r.module.version, Some(99));
        assert!(r.diagnostics.iter().any(|d|
            d.severity == Severity::Error && d.message.contains("unsupported allium version 99")
        ), "expected unsupported version error, got: {:?}", r.diagnostics);
    }

    #[test]
    fn empty_entity() {
        let r = parse_ok("entity User {}");
        assert_eq!(r.diagnostics.len(), 0);
        assert_eq!(r.module.declarations.len(), 1);
        match &r.module.declarations[0] {
            Decl::Block(b) => {
                assert_eq!(b.kind, BlockKind::Entity);
                assert_eq!(b.name.as_ref().unwrap().name, "User");
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn entity_with_fields() {
        let src = r#"entity Order {
    customer: Customer
    status: pending | active | completed
    total: Decimal
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        match &r.module.declarations[0] {
            Decl::Block(b) => {
                assert_eq!(b.items.len(), 3);
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn use_declaration() {
        let r = parse_ok(r#"use "github.com/specs/oauth/abc123" as oauth"#);
        assert_eq!(r.diagnostics.len(), 0);
        match &r.module.declarations[0] {
            Decl::Use(u) => {
                assert_eq!(u.alias.as_ref().unwrap().name, "oauth");
            }
            other => panic!("expected Use, got {other:?}"),
        }
    }

    #[test]
    fn enum_declaration() {
        let src = "enum OrderStatus { pending | shipped | delivered }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_block() {
        let src = r#"config {
    max_retries: Integer = 3
    timeout: Duration = 24.hours
}"#;
        // Config entries are `name: Type = default`. The parser sees
        // `name: Type = default` as an assignment where the value is
        // `Type = default` (comparison with Eq). That's fine for the
        // parse tree — semantic pass separates type from default.
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn rule_declaration() {
        let src = r#"rule PlaceOrder {
    when: CustomerPlacesOrder(customer, items, total)
    requires: total > 0
    ensures: Order.created(customer: customer, status: pending, total: total)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        match &r.module.declarations[0] {
            Decl::Block(b) => {
                assert_eq!(b.kind, BlockKind::Rule);
                assert_eq!(b.items.len(), 3);
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn expression_precedence() {
        let r = parse_ok("rule T { v: a + b * c }");
        // The value should be Add(a, Mul(b, c))
        match &r.module.declarations[0] {
            Decl::Block(b) => match &b.items[0].kind {
                BlockItemKind::Assignment { value, .. } => match value {
                    Expr::BinaryOp { op, right, .. } => {
                        assert_eq!(*op, BinaryOp::Add);
                        assert!(matches!(**right, Expr::BinaryOp { op: BinaryOp::Mul, .. }));
                    }
                    other => panic!("expected BinaryOp, got {other:?}"),
                },
                other => panic!("expected Assignment, got {other:?}"),
            },
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn default_declaration() {
        let src = r#"default Role admin = { name: "admin", permissions: { "read" } }"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn open_question() {
        let src = r#"open question "Should admins be role-specific?""#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn external_entity() {
        let src = "external entity Customer { email: String }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        match &r.module.declarations[0] {
            Decl::Block(b) => assert_eq!(b.kind, BlockKind::ExternalEntity),
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn where_expression() {
        let src = "entity E { active: items where status = active }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn with_expression() {
        let src = "entity E { slots: InterviewSlot with candidacy = this }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn lambda_expression() {
        let src = "entity E { v: items.any(i => i.active) }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn deferred() {
        let src = "deferred InterviewerMatching.suggest";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn variant_declaration() {
        let src = "variant Email : Notification { subject: String }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- projection mapping -----------------------------------------------

    #[test]
    fn projection_arrow() {
        let src = "entity E { confirmed: confirmations where status = confirmed -> interviewer }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- transitions_to / becomes ------------------------------------------

    #[test]
    fn transitions_to_trigger() {
        let src = "rule R { when: Interview.status transitions_to scheduled\n    ensures: Notification.created() }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn becomes_trigger() {
        let src = "rule R { when: Interview.status becomes scheduled\n    ensures: Notification.created() }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- binding colon in clause values ------------------------------------

    #[test]
    fn when_binding() {
        let src = "rule R {\n    when: interview: Interview.status transitions_to scheduled\n    ensures: Notification.created()\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        // The when clause value should be a Binding wrapping a TransitionsTo
        let decl = &r.module.declarations[0];
        if let Decl::Block(b) = decl {
            if let BlockItemKind::Clause { keyword, value } = &b.items[0].kind {
                assert_eq!(keyword, "when");
                assert!(matches!(value, Expr::Binding { .. }));
            } else {
                panic!("expected clause");
            }
        } else {
            panic!("expected block decl");
        }
    }

    #[test]
    fn when_binding_temporal() {
        let src = "rule R {\n    when: invitation: Invitation.expires_at <= now\n    ensures: Invitation.expired()\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn when_binding_created() {
        let src = "rule R {\n    when: batch: DigestBatch.created\n    ensures: Email.created()\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn facing_binding() {
        let src = "surface S {\n    facing viewer: Interviewer\n    exposes: InterviewList\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn context_binding() {
        let src = "surface S {\n    facing viewer: Interviewer\n    context assignment: SlotConfirmation where interviewer = viewer\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- rule-level for block item -----------------------------------------

    #[test]
    fn rule_level_for() {
        let src = r#"rule ProcessDigests {
    when: schedule: DigestSchedule.next_run_at <= now
    for user in Users where notification_setting.digest_enabled:
        ensures: DigestBatch.created(user: user)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        if let Decl::Block(b) = &r.module.declarations[0] {
            // Should have when clause + for block item
            assert!(b.items.len() >= 2);
            assert!(matches!(b.items[1].kind, BlockItemKind::ForBlock { .. }));
        } else {
            panic!("expected block decl");
        }
    }

    // -- let inside ensures blocks -----------------------------------------

    #[test]
    fn let_in_ensures_block() {
        let src = r#"rule R {
    when: ScheduleInterview(candidacy, time, interviewers)
    ensures:
        let slot = InterviewSlot.created(time: time, candidacy: candidacy)
        for interviewer in interviewers:
            SlotConfirmation.created(slot: slot, interviewer: interviewer)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- when guard on provides items --------------------------------------

    #[test]
    fn provides_when_guard() {
        let src = "surface S {\n    facing viewer: Interviewer\n    provides: ConfirmSlot(viewer, slot) when slot.status = pending\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- optional type suffix ----------------------------------------------

    #[test]
    fn optional_type_suffix() {
        let src = "entity E { locked_until: Timestamp? }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn optional_trigger_param() {
        let src = "rule R { when: Report(interviewer, interview, reason, details?)\n    ensures: Done() }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- qualified name with config ----------------------------------------

    #[test]
    fn qualified_config_access() {
        let src = "entity E { duration: oauth/config.session_duration }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- comprehensive integration test ------------------------------------

    #[test]
    fn realistic_spec() {
        let src = r#"-- allium: 1

enum OrderStatus { pending | shipped | delivered }

external entity Customer {
    email: String
    name: String
}

entity Order {
    customer: Customer
    status: OrderStatus
    total: Decimal
    items: OrderItem with order = this
    shipped_items: items where status = shipped
    confirmed_items: items where status = confirmed -> item
    is_complete: status = delivered
    locked_until: Timestamp?
}

config {
    max_retries: Integer = 3
    timeout: Duration = 24.hours
}

rule PlaceOrder {
    when: CustomerPlacesOrder(customer, items, total)
    requires: total > 0
    ensures: Order.created(customer: customer, status: pending, total: total)
}

rule ShipOrder {
    when: order: Order.status transitions_to shipped
    ensures: Email.created(to: order.customer.email, template: order_shipped)
}

open question "How do we handle partial shipments?"
"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "expected no errors");
        assert_eq!(r.module.version, Some(1));
        assert_eq!(r.module.declarations.len(), 7);
    }

    #[test]
    fn extension_behaviour_excerpt() {
        // Exercises: inline enums, generic types, or-triggers, named call
        // args, config with typed defaults, module declaration.
        let src = r#"value Document {
    uri: String
    text: String
}

entity Finding {
    code: String
    severity: error | warning | info
    range: FindingRange
}

entity DiagnosticsMode {
    value: strict | relaxed
}

config {
    duplicateKey: String = "allium.config.duplicateKey"
}

rule RefreshDiagnostics {
    when: DocumentOpened(document) or DocumentChanged(document)
    requires: document.language_id = "allium"
    ensures: FindingsComputed(document)
}

surface DiagnosticsDashboard {
    facing viewer: Developer
    context doc: Document where viewer.active_document = doc
    provides: RunChecks(viewer) when doc.language_id = "allium"
    exposes: FindingList
}

rule ProcessDigests {
    when: schedule: DigestSchedule.next_run_at <= now
    for user in Users where notification_setting.digest_enabled:
        let settings = user.notification_setting
        ensures: DigestBatch.created(user: user)
}
"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "expected no errors");
        // value + entity + entity + config + rule + surface + rule = 7
        assert_eq!(r.module.declarations.len(), 7);
    }

    #[test]
    fn exists_as_identifier() {
        let src = r#"rule R {
    when: X()
    ensures: CompletionItemAvailable(label: exists)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- pipe precedence: tighter than boolean ops ----------------------------

    #[test]
    fn pipe_binds_tighter_than_or() {
        // `a or b | c` should parse as `a or (b | c)`, not `(a or b) | c`
        let src = "entity E { v: a or b | c }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        // Top-level should be LogicalOp(Or)
        let Expr::LogicalOp { op, right, .. } = value else {
            panic!("expected LogicalOp, got {value:?}");
        };
        assert_eq!(*op, LogicalOp::Or);
        // Right side should be Pipe(b, c)
        assert!(matches!(right.as_ref(), Expr::Pipe { .. }));
    }

    // -- variant with expression base -----------------------------------------

    #[test]
    fn variant_with_pipe_base() {
        let src = "variant Mixed : TypeA | TypeB";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Variant(v) = &r.module.declarations[0] else { panic!() };
        assert!(matches!(v.base, Expr::Pipe { .. }));
    }

    // -- for-block with comparison in where filter ----------------------------

    #[test]
    fn for_block_where_comparison() {
        let src = r#"rule R {
    when: X()
    for item in Items where item.status = active:
        ensures: Processed(item: item)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::ForBlock { filter, .. } = &b.items[1].kind else { panic!() };
        assert!(filter.is_some());
        assert!(matches!(filter.as_ref().unwrap(), Expr::Comparison { .. }));
    }

    // -- for-expression with comparison in where filter -----------------------

    #[test]
    fn for_expr_where_comparison() {
        let src = r#"rule R {
    when: X()
    ensures:
        for item in Items where item.active = true:
            Processed(item: item)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- if/else if/else chain ------------------------------------------------

    #[test]
    fn if_else_if_else() {
        let src = r#"rule R {
    when: X(v)
    ensures:
        if v < 10: Small()
        else if v < 100: Medium()
        else: Large()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- null coalescing and optional chaining --------------------------------

    #[test]
    fn null_coalesce_and_optional_chain() {
        let src = "entity E { v: a?.b ?? fallback }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        // Top-level should be NullCoalesce
        assert!(matches!(value, Expr::NullCoalesce { .. }));
    }

    // -- generic types --------------------------------------------------------

    #[test]
    fn generic_type_nested() {
        let src = "entity E { v: List<Set<String>> }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- set literal, list literal, object literal ----------------------------

    #[test]
    fn collection_literals() {
        let src = r#"rule R {
    when: X()
    ensures:
        let s = {a, b, c}
        let o = {name: "test", count: 42}
        Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn spec_reject_list_literal() {
        // The spec does not define `[...]` list literal syntax.
        let src = r#"rule R {
    when: X()
    ensures:
        let l = [1, 2, 3]
        Done()
}"#;
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `[...]` list literal (not in spec), but parsed without errors"
        );
    }

    // -- given block ----------------------------------------------------------

    #[test]
    fn given_block() {
        let src = "given { viewer: User\n    time: Timestamp }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.kind, BlockKind::Given);
        assert!(b.name.is_none());
    }

    // -- actor block ----------------------------------------------------------

    #[test]
    fn actor_block() {
        let src = "actor Admin { identified_by: User where role = admin }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.kind, BlockKind::Actor);
    }

    // -- join lookup ----------------------------------------------------------

    #[test]
    fn join_lookup() {
        let src = "entity E { match: Other{field_a, field_b: value} }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        assert!(matches!(value, Expr::JoinLookup { .. }));
    }

    // -- in / not in with set literal -----------------------------------------

    #[test]
    fn in_not_in_set() {
        let src = r#"rule R {
    when: X(s)
    requires: s in {a, b, c}
    requires: s not in {d, e}
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- comprehensive fixture file -------------------------------------------

    #[test]
    fn comprehensive_fixture() {
        let src = include_str!("../tests/fixtures/comprehensive-edge-cases.allium");
        let r = parse(src);
        assert_eq!(
            r.diagnostics.len(),
            0,
            "expected no errors in comprehensive fixture, got: {:?}",
            r.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );
        assert!(r.module.declarations.len() > 30, "expected many declarations");
    }

    // -- error message quality ------------------------------------------------

    #[test]
    fn error_expected_declaration() {
        let r = parse("-- allium: 1\n+ invalid");
        assert!(r.diagnostics.len() >= 1);
        let msg = &r.diagnostics[0].message;
        assert!(msg.contains("expected declaration"), "got: {msg}");
        assert!(msg.contains("entity"), "should list valid options, got: {msg}");
        assert!(msg.contains("rule"), "should list valid options, got: {msg}");
    }

    #[test]
    fn error_expected_expression() {
        let r = parse("-- allium: 1\nentity E { v: }");
        assert!(r.diagnostics.len() >= 1);
        let msg = &r.diagnostics[0].message;
        assert!(msg.contains("expected expression"), "got: {msg}");
        assert!(msg.contains("identifier"), "should list valid starters, got: {msg}");
    }

    #[test]
    fn error_expected_block_item() {
        let r = parse("-- allium: 1\nentity E { + }");
        assert!(r.diagnostics.len() >= 1);
        let msg = &r.diagnostics[0].message;
        assert!(msg.contains("expected block item"), "got: {msg}");
    }

    #[test]
    fn error_expected_identifier() {
        let r = parse("-- allium: 1\nentity 123 {}");
        assert!(r.diagnostics.len() >= 1);
        let msg = &r.diagnostics[0].message;
        // Context-aware: says "entity name" not generic "identifier"
        assert!(msg.contains("expected entity name"), "got: {msg}");
        // Human-friendly: says "number" not "Number" or "TokenKind::Number"
        assert!(msg.contains("number"), "should say what was found, got: {msg}");
    }

    #[test]
    fn error_missing_brace() {
        let r = parse("entity E {");
        assert!(r.diagnostics.len() >= 1);
        let msg = &r.diagnostics[0].message;
        assert!(msg.contains("expected"), "got: {msg}");
    }

    #[test]
    fn error_recovery_multiple() {
        // Parser should recover and report multiple errors (on separate lines)
        let r = parse("entity E { + }\nentity F { - }");
        assert!(r.diagnostics.len() >= 2, "expected at least 2 errors, got {}", r.diagnostics.len());
    }

    #[test]
    fn error_dedup_same_line() {
        // Multiple bad tokens on a single line should produce only one error
        let r = parse("-- allium: 1\n+ - * /");
        let errors: Vec<_> = r.diagnostics.iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "expected 1 error for same-line bad tokens, got {}", errors.len());
    }

    #[test]
    fn for_block() {
        let src = r#"rule R {
    when: X()
    for user in Users where user.active:
        ensures: Notified(user: user)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert!(matches!(b.items[1].kind, BlockItemKind::ForBlock { .. }));
    }

    #[test]
    fn for_expr() {
        let src = r#"rule R {
    when: X(project)
    ensures:
        let total = for task in project.tasks: task.effort
        Done(total: total)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn for_where() {
        let src = r#"rule R {
    when: X()
    for item in Items where item.active:
        ensures: Processed(item: item)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn spec_reject_for_with_filter() {
        // The spec uses `where` for iteration filtering; `with` is for
        // relationship declarations only.
        let src = r#"rule R {
    when: X()
    for slot in Slot with slot.role = reviewer:
        ensures: Reviewed(slot: slot)
}"#;
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `for ... with` (spec uses `where`), but parsed without errors"
        );
    }

    #[test]
    fn block_level_if() {
        let src = r#"rule R {
    when: X(task)
    if task.priority = high:
        ensures: Escalated(task: task)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::IfBlock { branches, else_items } = &b.items[1].kind else {
            panic!("expected IfBlock, got {:?}", b.items[1].kind);
        };
        assert_eq!(branches.len(), 1);
        assert!(else_items.is_none());
    }

    #[test]
    fn block_level_if_else() {
        let src = r#"rule R {
    when: X(score)
    if score > 80:
        ensures: High()
    else if score > 40:
        ensures: Medium()
    else:
        ensures: Low()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::IfBlock { branches, else_items } = &b.items[1].kind else {
            panic!("expected IfBlock, got {:?}", b.items[1].kind);
        };
        assert_eq!(branches.len(), 2);
        assert!(else_items.is_some());
    }

    #[test]
    fn wildcard_type_parameter() {
        let src = "entity E { codec: Codec<*> }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        if let Expr::GenericType { args, .. } = value {
            assert_eq!(args.len(), 1);
            if let Expr::Ident(id) = &args[0] {
                assert_eq!(id.name, "*");
            } else {
                panic!("expected wildcard ident, got {:?}", args[0]);
            }
        } else {
            panic!("expected GenericType, got {:?}", value);
        }
    }

    #[test]
    fn guidance_clause_comment_only_value_migration() {
        // Old `guidance:` colon form should emit a migration diagnostic
        let src = "-- allium: 1\nrule R {\n    ensures: Done()\n    guidance: -- just a comment\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("`guidance:` syntax was replaced")),
            "expected migration diagnostic, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn spec_reject_for_expr_with_filter() {
        // Expression-level `for` also only accepts `where`, not `with`.
        let src = r#"rule R {
    when: X(project)
    ensures:
        let total = for task in project.tasks with task.active: task.effort
        Done(total: total)
}"#;
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `for ... with` in expression (spec uses `where`), but parsed without errors"
        );
    }

    #[test]
    fn for_destructured_binding() {
        let src = r#"rule R {
    when: X()
    for (key, value) in Pairs where key != null:
        ensures: Processed(key: key, value: value)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::ForBlock { binding, .. } = &b.items[1].kind else { panic!() };
        assert!(matches!(binding, ForBinding::Destructured(ids, _) if ids.len() == 2));
    }

    #[test]
    fn dot_path_assignment() {
        let src = r#"entity Shard {
    ShardGroup.shard_cache: Shard with group = this
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::PathAssignment { path, .. } = &b.items[0].kind else {
            panic!("expected PathAssignment, got {:?}", b.items[0].kind);
        };
        assert!(matches!(path, Expr::MemberAccess { .. }));
    }

    #[test]
    fn language_reference_fixture() {
        let src = include_str!("../tests/fixtures/language-reference-constructs.allium");
        let r = parse(src);
        let errors: Vec<_> = r.diagnostics.iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            0,
            "expected no errors in language-reference fixture, got: {:?}",
            errors.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );
    }

    // =====================================================================
    // V1 SPEC CONFORMANCE TESTS
    //
    // These tests verify that the parser conforms to the Allium V1 language
    // reference (docs/allium-v1-language-reference.md). Each test is tagged
    // with the finding number from the audit.
    //
    // Tests marked "should reject" are expected to FAIL until the parser
    // is updated to reject non-spec constructs.
    //
    // Tests marked "should parse" are expected to FAIL until the parser
    // is updated to handle spec-defined constructs.
    // =====================================================================

    // -- Finding 1: spec uses `for`, not `for each` ---------------------------

    #[test]
    fn spec_for_bare_form() {
        // The spec uses bare `for` exclusively. This must parse cleanly.
        let src = r#"rule ProcessDigests {
    when: schedule: DigestSchedule.next_run_at <= now
    for user in Users where notification_setting.digest_enabled:
        let settings = user.notification_setting
        ensures: DigestBatch.created(user: user)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn spec_reject_for_each() {
        // `for each` is not in the spec. The parser should reject it.
        let src = r#"rule R {
    when: X()
    for each user in Users where user.active:
        ensures: Notified(user: user)
}"#;
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `for each` (not in spec), but parsed without errors"
        );
    }

    // -- Finding 2: spec uses `=`, not `==` -----------------------------------

    #[test]
    fn spec_reject_double_equals() {
        // The spec uses `=` for equality. `==` should not be accepted.
        let src = "rule R { when: X(a)\n    requires: a.status == active\n    ensures: Done() }";
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `==` (not in spec), but parsed without errors"
        );
    }

    // -- Finding 3: `system` blocks are not in the spec -----------------------

    #[test]
    fn spec_reject_system_block() {
        // `system` is not a declaration type in the V1 spec.
        let src = "system PaymentGateway {\n    timeout: 30.seconds\n}";
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `system` block (not in spec), but parsed without errors"
        );
    }

    // -- Finding 6: `tags` clause is not in the spec --------------------------

    #[test]
    fn spec_reject_tags_clause() {
        // `tags:` is not a clause keyword in the V1 spec.
        let src = r#"rule R {
    when: MigrationTriggered()
    tags: infrastructure, migration
    ensures: MigrationComplete()
}"#;
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `tags:` clause (not in spec), but parsed without errors"
        );
    }

    // -- Finding 7: `includes`/`excludes` are not in the spec -----------------

    #[test]
    fn spec_reject_includes_operator() {
        // The spec uses `x in collection`, not `collection includes x`.
        let src = r#"rule R {
    when: X(a, b)
    requires: a.items includes b
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `includes` operator (not in spec), but parsed without errors"
        );
    }

    #[test]
    fn spec_reject_excludes_operator() {
        // The spec uses `x not in collection`, not `collection excludes x`.
        let src = r#"rule R {
    when: X(a, b)
    requires: a.items excludes b
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `excludes` operator (not in spec), but parsed without errors"
        );
    }

    // -- Finding 8: range literals (`..`) are not in the spec -----------------

    #[test]
    fn spec_reject_range_literal() {
        // The `..` range operator is not defined in the V1 spec.
        let src = r#"rule R {
    when: X(v)
    requires: v in [1..100]
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `..` range (not in spec), but parsed without errors"
        );
    }

    // -- Finding 9: `within` is only for actors, not rules --------------------

    #[test]
    fn spec_within_in_actor() {
        // The spec defines `within:` as an actor clause.
        let src = r#"actor WorkspaceAdmin {
    within: Workspace
    identified_by: User where role = admin
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "within: in actor should parse cleanly");
    }

    // -- Finding 10: `module` declaration is not in the spec ------------------

    #[test]
    fn spec_reject_module_declaration() {
        // `module Name` is not a declaration in the V1 spec.
        let src = "module my_spec";
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for `module` declaration (not in spec), but parsed without errors"
        );
    }

    // -- Finding 11: `guidance` at module level is not in the spec ------------

    #[test]
    fn spec_reject_module_level_guidance() {
        // The spec shows `guidance:` only as a surface clause.
        let src = r#"guidance: "All rules must be idempotent""#;
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for module-level `guidance:` (not in spec), but parsed without errors"
        );
    }

    // -- Finding 12: `guarantee`/`timeout` in rules is not in the spec --------

    #[test]
    fn spec_guarantee_in_surface_migration() {
        // Old `guarantee:` colon form should emit a migration diagnostic
        let src = "-- allium: 1\nsurface S {\n    facing viewer: User\n    guarantee: DataIntegrity\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("`guarantee:` syntax was replaced")),
            "expected migration diagnostic, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn spec_timeout_in_surface() {
        // The spec defines `timeout:` as a surface clause with rule name syntax.
        let src = r#"surface InvitationView {
    facing recipient: Candidate
    context invitation: ResourceInvitation where email = recipient.email
    timeout: InvitationExpires
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "timeout: in surface should parse cleanly");
    }

    #[test]
    fn spec_timeout_in_surface_with_when() {
        // The spec shows `timeout: RuleName when condition`.
        let src = r#"surface InvitationView {
    facing recipient: Candidate
    context invitation: ResourceInvitation where email = recipient.email
    timeout: InvitationExpires when invitation.expires_at <= now
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "timeout: with when guard should parse cleanly");
    }

    // -- Finding 15: suffix predicates are not in the spec --------------------

    #[test]
    fn spec_reject_suffix_predicate() {
        // The spec does not define suffix predicate syntax like `starts_with`.
        let src = r#"rule R {
    when: X()
    requires: finding.code starts_with "allium."
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for suffix predicate (not in spec), but parsed without errors"
        );
    }

    // -- Finding 17: `.add()`/`.remove()` are in the spec ---------------------

    #[test]
    fn spec_add_remove_in_ensures() {
        // The spec documents `.add()` and `.remove()` as ensures-only mutations.
        // These parse as regular method calls, which is correct.
        let src = r#"rule R {
    when: AssignInterviewer(interview, new_interviewer)
    ensures:
        interview.interviewers.add(new_interviewer)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, ".add() should parse cleanly");
    }

    #[test]
    fn spec_remove_in_ensures() {
        let src = r#"rule R {
    when: RemoveInterviewer(interview, leaving)
    ensures:
        interview.interviewers.remove(leaving)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, ".remove() should parse cleanly");
    }

    // -- Finding 18: `.first`/`.last` are in the spec -------------------------

    #[test]
    fn spec_first_last_access() {
        // The spec documents `.first` and `.last` for ordered collections.
        let src = "entity E { latest: attempts.last\n    earliest: attempts.first }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, ".first/.last should parse cleanly");
    }

    // -- Finding 19: set arithmetic is in the spec ----------------------------

    #[test]
    fn spec_set_arithmetic() {
        // The spec documents `+` and `-` on collections as set arithmetic.
        let src = r#"entity Role {
    permissions: Set<String>
    inherited: Set<String>
    all_permissions: permissions + inherited
    removed: old_mentions - new_mentions
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "set arithmetic should parse cleanly");
    }

    // -- Finding 20: discard binding `_` is in the spec -----------------------

    #[test]
    fn spec_discard_binding_in_trigger() {
        // The spec shows `when: _: LogProcessor.last_flush_check + ...`
        let src = r#"rule R {
    when: _: LogProcessor.last_flush_check <= now
    ensures: Flushed()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "discard binding _ in trigger should parse cleanly");
    }

    #[test]
    fn spec_discard_in_trigger_params() {
        // The spec shows `when: SomeEvent(_, slot)`
        let src = r#"rule R {
    when: SomeEvent(_, slot)
    ensures: Processed(slot: slot)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "discard _ in trigger params should parse cleanly");
    }

    #[test]
    fn spec_discard_in_for() {
        // The spec shows `for _ in items: Counted(batch)`
        let src = r#"rule R {
    when: X(items)
    ensures:
        for _ in items: Counted()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "discard _ in for should parse cleanly");
    }

    // -- Finding 21: default with object literal is in the spec ---------------

    #[test]
    fn spec_default_with_object_literal() {
        // The spec shows: default InterviewType all_in_one = { name: "All in one", duration: 75.minutes }
        let src = r#"default InterviewType all_in_one = { name: "All in one", duration: 75.minutes }"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "default with object literal should parse cleanly");
    }

    #[test]
    fn spec_default_multiline_object() {
        // The spec shows multi-line defaults with object literals.
        let src = r#"default Role viewer = {
    name: "viewer",
    permissions: { "documents.read" }
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "multi-line default with object literal should parse cleanly");
    }

    // -- Spec surface features: related, let, guarantee, timeout --------------

    #[test]
    fn spec_surface_related_clause() {
        // The spec shows `related:` with surface references.
        let src = r#"surface InterviewerDashboard {
    facing viewer: Interviewer
    context assignment: SlotConfirmation where interviewer = viewer
    related: InterviewDetail(assignment.slot.interview) when assignment.slot.interview != null
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "related: in surface should parse cleanly");
    }

    #[test]
    fn spec_surface_let_binding() {
        // The spec shows `let` bindings inside surfaces.
        let src = r#"surface S {
    facing viewer: User
    let comments = Comments where parent = viewer
    exposes: CommentList
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "let in surface should parse cleanly");
    }

    #[test]
    fn spec_surface_multiline_context_where() {
        // The spec shows context with where on a continuation line.
        let src = r#"surface InterviewerPendingAssignments {
    facing viewer: Interviewer
    context assignment: InterviewAssignment
        where interviewer = viewer and status = pending
    exposes: AssignmentList
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "multi-line context where should parse cleanly");
    }

    // -- Spec: `for` inside surfaces ------------------------------------------

    #[test]
    fn spec_for_in_surface_provides() {
        // The spec shows for iteration inside surface provides.
        let src = r#"surface TaskBoard {
    facing viewer: User
    for task in Task where task.assignee = viewer:
        provides: CompleteTask(viewer, task) when task.status = in_progress
    exposes: KanbanBoard
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "for in surface provides should parse cleanly");
    }

    // -- Spec: `use` without alias --------------------------------------------

    #[test]
    fn spec_use_without_alias() {
        // The spec shows `use` both with and without `as alias`.
        let src = r#"use "github.com/specs/notifications/def456""#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "use without alias should parse cleanly");
    }

    // -- Spec: empty external entity ------------------------------------------

    #[test]
    fn spec_empty_external_entity() {
        // The spec shows external entities with empty bodies as type placeholders.
        let src = "external entity Commentable {}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "empty external entity should parse cleanly");
    }

    // -- Spec: multi-line provides block in surface ---------------------------

    #[test]
    fn spec_surface_multiline_provides() {
        // The spec shows provides as a multi-line block.
        let src = r#"surface ProjectDashboard {
    facing viewer: ProjectManager
    context project: Project where owner = viewer
    provides:
        CreateTask(viewer, project) when project.status = active
        ArchiveProject(viewer, project) when project.tasks.all(t => t.status = completed)
    exposes: TaskList
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "multi-line provides should parse cleanly");
    }

    // -- Spec: multi-line exposes block in surface ----------------------------

    #[test]
    fn spec_surface_multiline_exposes() {
        // The spec shows exposes as a multi-line block.
        let src = r#"surface InterviewerDashboard {
    facing viewer: Interviewer
    context assignment: SlotConfirmation where interviewer = viewer
    exposes:
        assignment.slot.time
        assignment.status
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0, "multi-line exposes should parse cleanly");
    }

    // =====================================================================
    // COVERAGE GAP TESTS
    //
    // Dedicated unit tests for spec constructs that previously only had
    // fixture-file coverage.
    // =====================================================================

    // -- Composite or-triggers ------------------------------------------------

    #[test]
    fn composite_or_trigger() {
        let src = r#"rule R {
    when: EventA(x) or EventB(x) or EventC(x)
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Clause { keyword, value } = &b.items[0].kind else { panic!() };
        assert_eq!(keyword, "when");
        // Top-level should be LogicalOp(Or) wrapping another Or
        let Expr::LogicalOp { op, left, .. } = value else {
            panic!("expected LogicalOp, got {value:?}");
        };
        assert_eq!(*op, LogicalOp::Or);
        assert!(matches!(left.as_ref(), Expr::LogicalOp { op: LogicalOp::Or, .. }));
    }

    // -- Value type declaration -----------------------------------------------

    #[test]
    fn value_type_declaration() {
        let src = r#"value TimeRange {
    start: Timestamp
    end: Timestamp
    duration: end - start
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.kind, BlockKind::Value);
        assert_eq!(b.name.as_ref().unwrap().name, "TimeRange");
        assert_eq!(b.items.len(), 3);
    }

    // -- Qualified config block -----------------------------------------------

    #[test]
    fn qualified_config_block() {
        let src = r#"use "github.com/specs/oauth/abc123" as oauth
oauth/config {
    session_duration: Duration = 24.hours
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        assert_eq!(r.module.declarations.len(), 2);
    }

    // -- String interpolation -------------------------------------------------

    #[test]
    fn string_interpolation_parts() {
        let src = r#"rule R {
    when: X(name, action)
    ensures: Log.created(message: "User {name} did {action}")
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        // Dig into the message arg to verify interpolation parts
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Clause { value, .. } = &b.items[1].kind else { panic!() };
        let Expr::Call { args, .. } = value else { panic!() };
        let CallArg::Named(arg) = &args[0] else { panic!() };
        let Expr::StringLiteral(s) = &arg.value else { panic!() };
        assert_eq!(s.parts.len(), 4, "expected 4 string parts: text, interp, text, interp");
        assert!(matches!(&s.parts[0], StringPart::Text(t) if t == "User "));
        assert!(matches!(&s.parts[1], StringPart::Interpolation(id) if id.name == "name"));
        assert!(matches!(&s.parts[2], StringPart::Text(t) if t == " did "));
        assert!(matches!(&s.parts[3], StringPart::Interpolation(id) if id.name == "action"));
    }

    // -- `this` keyword as expression -----------------------------------------

    #[test]
    fn this_keyword_expression() {
        // `Item with parent = this` parses as With(Item, Eq(parent, this))
        // because `with` binds looser than `=`, capturing the full predicate.
        let src = "entity E { items: Item with parent = this }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::With { predicate, .. } = value else {
            panic!("expected With, got {value:?}");
        };
        let Expr::Comparison { op, right, .. } = predicate.as_ref() else {
            panic!("expected Comparison in with predicate, got {predicate:?}");
        };
        assert_eq!(*op, ComparisonOp::Eq);
        assert!(matches!(right.as_ref(), Expr::This { .. }));
    }

    // -- `not` prefix operator (standalone) -----------------------------------

    #[test]
    fn not_prefix_standalone() {
        let src = r#"rule R {
    when: X(user)
    requires: not user.is_locked
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Clause { keyword, value } = &b.items[1].kind else { panic!() };
        assert_eq!(keyword, "requires");
        assert!(matches!(value, Expr::Not { .. }));
    }

    // -- Unary minus ----------------------------------------------------------

    #[test]
    fn unary_minus() {
        let src = "entity E { offset: -1 }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        assert!(matches!(value, Expr::BinaryOp { op: BinaryOp::Sub, .. }
                        | Expr::NumberLiteral { .. }), "expected negation, got {value:?}");
    }

    // -- Parenthesised expression grouping ------------------------------------

    #[test]
    fn parenthesised_expression() {
        let src = "entity E { v: (a + b) * c }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        // Top level should be Mul, with left being Add (grouped by parens)
        let Expr::BinaryOp { op, left, .. } = value else {
            panic!("expected BinaryOp, got {value:?}");
        };
        assert_eq!(*op, BinaryOp::Mul);
        assert!(matches!(left.as_ref(), Expr::BinaryOp { op: BinaryOp::Add, .. }));
    }

    // -- Boolean literals -----------------------------------------------------

    #[test]
    fn boolean_literals() {
        let src = r#"rule R {
    when: X(item)
    ensures:
        item.active = true
        item.deleted = false
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- `null` literal -------------------------------------------------------

    #[test]
    fn null_literal() {
        let src = "entity E { v: parent ?? null }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::NullCoalesce { right, .. } = value else { panic!() };
        assert!(matches!(right.as_ref(), Expr::Null { .. }));
    }

    // -- Empty set literal ----------------------------------------------------

    #[test]
    fn empty_set_literal() {
        let src = "entity E { tags: Set<String>\n    default_tags: {} }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[1].kind else { panic!() };
        let Expr::SetLiteral { elements, .. } = value else { panic!("expected SetLiteral, got {value:?}") };
        assert!(elements.is_empty());
    }

    // =====================================================================
    // PRE-1.0 COVERAGE TESTS
    //
    // Additional tests addressing gaps identified during the pre-release
    // review: parameterised derived values, operator precedence matrix,
    // indentation-based multi-line detection, and set/object literal
    // disambiguation.
    // =====================================================================

    // -- Parameterised derived values (ParamAssignment) --------------------

    #[test]
    fn param_assignment_single() {
        let src = "entity Plan { can_use(feature): feature in features }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::ParamAssignment { name, params, value } = &b.items[0].kind else {
            panic!("expected ParamAssignment, got {:?}", b.items[0].kind);
        };
        assert_eq!(name.name, "can_use");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "feature");
        assert!(matches!(value, Expr::In { .. }));
    }

    #[test]
    fn param_assignment_multiple() {
        let src = "entity E { distance(x, y): (x * x + y * y) }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::ParamAssignment { name, params, .. } = &b.items[0].kind else {
            panic!("expected ParamAssignment, got {:?}", b.items[0].kind);
        };
        assert_eq!(name.name, "distance");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "x");
        assert_eq!(params[1].name, "y");
    }

    #[test]
    fn param_assignment_simple_expression() {
        let src = "entity Task { remaining_effort(total): total - effort }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::ParamAssignment { name, params, value } = &b.items[0].kind else {
            panic!("expected ParamAssignment, got {:?}", b.items[0].kind);
        };
        assert_eq!(name.name, "remaining_effort");
        assert_eq!(params.len(), 1);
        assert!(matches!(value, Expr::BinaryOp { op: BinaryOp::Sub, .. }));
    }

    // -- Operator precedence matrix ----------------------------------------

    #[test]
    fn precedence_logical_and_binds_tighter_than_or() {
        // `a or b and c` => Or(a, And(b, c))
        let src = "entity E { v: a or b and c }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::LogicalOp { op, right, .. } = value else {
            panic!("expected LogicalOp, got {value:?}");
        };
        assert_eq!(*op, LogicalOp::Or);
        assert!(matches!(right.as_ref(), Expr::LogicalOp { op: LogicalOp::And, .. }));
    }

    #[test]
    fn precedence_comparison_binds_tighter_than_and() {
        // `a = b and c != d` => And(Eq(a, b), NotEq(c, d))
        let src = "entity E { v: a = b and c != d }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::LogicalOp { op, left, right, .. } = value else {
            panic!("expected LogicalOp, got {value:?}");
        };
        assert_eq!(*op, LogicalOp::And);
        assert!(matches!(left.as_ref(), Expr::Comparison { op: ComparisonOp::Eq, .. }));
        assert!(matches!(right.as_ref(), Expr::Comparison { op: ComparisonOp::NotEq, .. }));
    }

    #[test]
    fn precedence_arithmetic_binds_tighter_than_comparison() {
        // `a + b > c * d` => Gt(Add(a, b), Mul(c, d))
        let src = "entity E { v: a + b > c * d }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::Comparison { op, left, right, .. } = value else {
            panic!("expected Comparison, got {value:?}");
        };
        assert_eq!(*op, ComparisonOp::Gt);
        assert!(matches!(left.as_ref(), Expr::BinaryOp { op: BinaryOp::Add, .. }));
        assert!(matches!(right.as_ref(), Expr::BinaryOp { op: BinaryOp::Mul, .. }));
    }

    #[test]
    fn precedence_null_coalesce_binds_tighter_than_comparison() {
        // `a ?? b = c` => Eq(NullCoalesce(a, b), c)
        let src = "entity E { v: a ?? b = c }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::Comparison { op, left, .. } = value else {
            panic!("expected Comparison, got {value:?}");
        };
        assert_eq!(*op, ComparisonOp::Eq);
        assert!(matches!(left.as_ref(), Expr::NullCoalesce { .. }));
    }

    #[test]
    fn precedence_not_binds_tighter_than_and() {
        // `not a and b` => And(Not(a), b)
        let src = r#"rule R {
    when: X(a, b)
    requires: not a and b
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Clause { value, .. } = &b.items[1].kind else { panic!() };
        let Expr::LogicalOp { op, left, .. } = value else {
            panic!("expected LogicalOp, got {value:?}");
        };
        assert_eq!(*op, LogicalOp::And);
        assert!(matches!(left.as_ref(), Expr::Not { .. }));
    }

    #[test]
    fn precedence_where_captures_full_condition() {
        // `items where status = active` => Where(items, Eq(status, active))
        // where (BP 7) binds looser than comparison (BP 30), so the full
        // condition `status = active` is captured as the where predicate.
        let src = "entity E { v: items where status = active }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::Where { condition, .. } = value else {
            panic!("expected Where, got {value:?}");
        };
        assert!(matches!(condition.as_ref(), Expr::Comparison { op: ComparisonOp::Eq, .. }));
    }

    #[test]
    fn precedence_where_captures_and_or_conditions() {
        // `items where status = active and count > 0` =>
        //   Where(items, And(Eq(status, active), Gt(count, 0)))
        let src = "entity E { v: items where status = active and count > 0 }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::Where { condition, .. } = value else {
            panic!("expected Where, got {value:?}");
        };
        assert!(matches!(condition.as_ref(), Expr::LogicalOp { op: LogicalOp::And, .. }));
    }

    #[test]
    fn precedence_projection_applies_to_where_result() {
        // `items where status = confirmed -> interviewer` =>
        //   ProjectionMap(Where(items, Eq(status, confirmed)), interviewer)
        let src = "entity E { v: items where status = confirmed -> interviewer }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::ProjectionMap { source, field, .. } = value else {
            panic!("expected ProjectionMap, got {value:?}");
        };
        assert_eq!(field.name, "interviewer");
        assert!(matches!(source.as_ref(), Expr::Where { .. }));
    }

    #[test]
    fn precedence_lambda_binds_loosest() {
        // `items.any(i => i.active and i.valid)` => Lambda(i, And(active, valid))
        let src = "entity E { v: items.any(i => i.active and i.valid) }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::Call { args, .. } = value else { panic!() };
        let CallArg::Positional(Expr::Lambda { body, .. }) = &args[0] else { panic!() };
        assert!(matches!(body.as_ref(), Expr::LogicalOp { op: LogicalOp::And, .. }));
    }

    #[test]
    fn precedence_in_binds_at_comparison_level() {
        // `x in {a, b} and y not in {c}` => And(In(x, {a,b}), NotIn(y, {c}))
        let src = r#"rule R {
    when: X(x, y)
    requires: x in {a, b} and y not in {c}
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Clause { value, .. } = &b.items[1].kind else { panic!() };
        let Expr::LogicalOp { op, left, right, .. } = value else {
            panic!("expected LogicalOp, got {value:?}");
        };
        assert_eq!(*op, LogicalOp::And);
        assert!(matches!(left.as_ref(), Expr::In { .. }));
        assert!(matches!(right.as_ref(), Expr::NotIn { .. }));
    }

    // -- Multi-line clause value detection ----------------------------------

    #[test]
    fn multiline_ensures_block() {
        let src = r#"rule R {
    when: X(doc)
    ensures:
        doc.status = published
        Notification.created(to: doc.author)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Clause { keyword, value } = &b.items[1].kind else { panic!() };
        assert_eq!(keyword, "ensures");
        let Expr::Block { items, .. } = value else {
            panic!("expected Block for multi-line ensures, got {value:?}");
        };
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn singleline_ensures_value() {
        let src = r#"rule R {
    when: X(doc)
    ensures: doc.status = published
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Clause { keyword, value } = &b.items[1].kind else { panic!() };
        assert_eq!(keyword, "ensures");
        // Single-line value should NOT be wrapped in a Block
        assert!(!matches!(value, Expr::Block { .. }), "single-line ensures should not be Block");
    }

    #[test]
    fn multiline_requires_with_continuation() {
        let src = r#"rule R {
    when: X(a)
    requires:
        a.count >= 2
        or a.items.any(i => i.can_solo)
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- Set vs object literal disambiguation ------------------------------

    #[test]
    fn object_literal_single_field() {
        let src = r#"rule R {
    when: X()
    ensures:
        let o = {name: "test"}
        Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Clause { value, .. } = &b.items[1].kind else { panic!() };
        let Expr::Block { items, .. } = value else { panic!() };
        let Expr::LetExpr { value: let_val, .. } = &items[0] else { panic!() };
        assert!(matches!(let_val.as_ref(), Expr::ObjectLiteral { .. }));
    }

    #[test]
    fn set_literal_single_element() {
        let src = r#"rule R {
    when: X()
    ensures:
        let s = {active}
        Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Clause { value, .. } = &b.items[1].kind else { panic!() };
        let Expr::Block { items, .. } = value else { panic!() };
        let Expr::LetExpr { value: let_val, .. } = &items[0] else { panic!() };
        assert!(matches!(let_val.as_ref(), Expr::SetLiteral { .. }),
            "bare {{ident}} should parse as set literal, got {:?}", let_val);
    }

    // -- Lambda variations -------------------------------------------------

    #[test]
    fn lambda_with_chained_access() {
        let src = "entity E { v: items.all(t => t.item.status = active) }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn nested_lambda() {
        let src = "entity E { v: groups.any(g => g.items.all(i => i.valid)) }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- Qualified name variations -----------------------------------------

    #[test]
    fn qualified_name_with_member_access() {
        let src = "entity E { v: shared/Validator.check }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::MemberAccess { object, field, .. } = value else {
            panic!("expected MemberAccess, got {value:?}");
        };
        assert!(matches!(object.as_ref(), Expr::QualifiedName(_)));
        assert_eq!(field.name, "check");
    }

    #[test]
    fn qualified_name_in_call() {
        let src = r#"rule R {
    when: X(item)
    requires: shared/Validator.check(item: item)
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -- Nested control flow -----------------------------------------------

    #[test]
    fn nested_if_inside_for() {
        let src = r#"rule R {
    when: X()
    for user in Users where user.active:
        if user.role = admin:
            ensures: AdminNotified(user: user)
        else:
            ensures: UserNotified(user: user)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::ForBlock { items, .. } = &b.items[1].kind else { panic!() };
        assert!(matches!(items[0].kind, BlockItemKind::IfBlock { .. }));
    }

    #[test]
    fn for_with_let_before_ensures() {
        let src = r#"rule R {
    when: schedule: DigestSchedule.next_run_at <= now
    for user in Users where user.active:
        let pending = user.tasks where status = pending
        ensures: DigestEmail.created(to: user.email, tasks: pending)
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::ForBlock { items, .. } = &b.items[1].kind else { panic!() };
        assert_eq!(items.len(), 2, "for body should have let + ensures");
        assert!(matches!(items[0].kind, BlockItemKind::Let { .. }));
        assert!(matches!(items[1].kind, BlockItemKind::Clause { .. }));
    }

    // -- Join lookup variations --------------------------------------------

    #[test]
    fn join_lookup_all_unnamed() {
        let src = "entity E { match: Other{a, b, c} }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::JoinLookup { fields, .. } = value else { panic!() };
        assert_eq!(fields.len(), 3);
        assert!(fields.iter().all(|f| f.value.is_none()));
    }

    #[test]
    fn join_lookup_all_named() {
        let src = "entity E { match: Membership{user: actor, workspace: ws} }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::JoinLookup { fields, .. } = value else { panic!() };
        assert_eq!(fields.len(), 2);
        assert!(fields.iter().all(|f| f.value.is_some()));
    }

    #[test]
    fn join_lookup_in_requires() {
        let src = r#"rule R {
    when: X(user, workspace)
    requires: exists WorkspaceMembership{user: user, workspace: workspace}
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn join_lookup_negated_in_requires() {
        let src = r#"rule R {
    when: X(email)
    requires: not exists User{email: email}
    ensures: Done()
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -----------------------------------------------------------------------
    // ALP-11: implies operator
    // -----------------------------------------------------------------------

    #[test]
    fn implies_basic() {
        let src = "rule R { requires: a implies b }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Clause { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::LogicalOp { op, .. } = value else { panic!("expected LogicalOp, got {value:?}") };
        assert_eq!(*op, LogicalOp::Implies);
    }

    #[test]
    fn implies_precedence_and_binds_tighter() {
        // `a and b implies c` → `(a and b) implies c`
        let src = "rule R { v: a and b implies c }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::LogicalOp { op, left, .. } = value else { panic!() };
        assert_eq!(*op, LogicalOp::Implies);
        assert!(matches!(left.as_ref(), Expr::LogicalOp { op: LogicalOp::And, .. }));
    }

    #[test]
    fn implies_precedence_or_binds_tighter() {
        // `a or b implies c` → `(a or b) implies c`
        let src = "rule R { v: a or b implies c }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::LogicalOp { op, left, .. } = value else { panic!() };
        assert_eq!(*op, LogicalOp::Implies);
        assert!(matches!(left.as_ref(), Expr::LogicalOp { op: LogicalOp::Or, .. }));
    }

    #[test]
    fn implies_precedence_implies_above_or() {
        // `a implies b or c` → `a implies (b or c)`
        let src = "rule R { v: a implies b or c }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::LogicalOp { op, right, .. } = value else { panic!() };
        assert_eq!(*op, LogicalOp::Implies);
        assert!(matches!(right.as_ref(), Expr::LogicalOp { op: LogicalOp::Or, .. }));
    }

    #[test]
    fn implies_precedence_not_binds_tighter() {
        // `not a implies b` → `(not a) implies b`
        let src = "rule R { v: not a implies b }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::LogicalOp { op, left, .. } = value else { panic!() };
        assert_eq!(*op, LogicalOp::Implies);
        assert!(matches!(left.as_ref(), Expr::Not { .. }));
    }

    #[test]
    fn implies_right_associative() {
        // `a implies b implies c` → `a implies (b implies c)`
        let src = "rule R { v: a implies b implies c }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        let Expr::LogicalOp { op, right, .. } = value else { panic!() };
        assert_eq!(*op, LogicalOp::Implies);
        assert!(matches!(right.as_ref(), Expr::LogicalOp { op: LogicalOp::Implies, .. }));
    }

    #[test]
    fn implies_is_keyword_parsed_as_operator() {
        // `implies` is a keyword — in infix position it's the operator, not an ident.
        // `a implies b` parses as LogicalOp, never as two adjacent identifiers.
        let src = "entity E { v: a implies b }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        assert!(matches!(value, Expr::LogicalOp { op: LogicalOp::Implies, .. }));
    }

    #[test]
    fn implies_in_ensures() {
        let src = r#"rule R {
    when: X()
    ensures: a implies b
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn implies_in_derived_value() {
        let src = "entity E { v: a implies b }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        assert!(matches!(value, Expr::LogicalOp { op: LogicalOp::Implies, .. }));
    }

    // -----------------------------------------------------------------------
    // ALP-7: guidance clause ordering
    // -----------------------------------------------------------------------

    #[test]
    fn guidance_ordering_tests_removed() {
        // Guidance ordering validation was moved to the structural validator.
        // The old `guidance:` colon form now emits migration diagnostics.
        // See guidance_colon_form_migration and annotation_guidance_in_rule tests.
    }

    // -----------------------------------------------------------------------
    // ALP-9: contract declarations
    // -----------------------------------------------------------------------

    #[test]
    fn contract_signatures_only() {
        let src = r#"contract Auditable {
    last_modified_by: Actor
    last_modified_at: Timestamp
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.kind, BlockKind::Contract);
        assert_eq!(b.name.as_ref().unwrap().name, "Auditable");
        assert_eq!(b.items.len(), 2);
    }

    #[test]
    fn contract_with_annotations() {
        let src = r#"contract Versioned {
    version: Integer
    @invariant Monotonic
        -- versions must increase
    @guidance
        -- use semantic versioning
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.kind, BlockKind::Contract);
        assert_eq!(b.items.len(), 3);
    }

    #[test]
    fn contract_with_any_type() {
        let src = r#"contract Identifiable {
    id: Any
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn contract_lowercase_name_rejected() {
        let src = "-- allium: 1\ncontract bad {}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("uppercase")),
            "expected uppercase error, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn contract_colon_body_rejected() {
        let src = "-- allium: 1\ncontract Bad: something";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("braces")),
            "expected braces error, got: {:?}",
            r.diagnostics
        );
    }

    // -----------------------------------------------------------------------
    // ALP-15: contracts clause
    // -----------------------------------------------------------------------

    #[test]
    fn contracts_clause_single_demands() {
        let src = "surface S {\n    contracts:\n        demands Auditable\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::ContractsClause { entries } = &b.items[0].kind else {
            panic!("expected ContractsClause, got {:?}", b.items[0].kind)
        };
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].direction, ContractDirection::Demands));
        assert_eq!(entries[0].name.name, "Auditable");
    }

    #[test]
    fn contracts_clause_single_fulfils() {
        let src = "surface S {\n    contracts:\n        fulfils EventSubmitter\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::ContractsClause { entries } = &b.items[0].kind else {
            panic!("expected ContractsClause")
        };
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].direction, ContractDirection::Fulfils));
        assert_eq!(entries[0].name.name, "EventSubmitter");
    }

    #[test]
    fn contracts_clause_mixed() {
        let src = "surface S {\n    contracts:\n        demands Auditable\n        fulfils EventSubmitter\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::ContractsClause { entries } = &b.items[0].kind else {
            panic!("expected ContractsClause")
        };
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0].direction, ContractDirection::Demands));
        assert!(matches!(entries[1].direction, ContractDirection::Fulfils));
    }

    #[test]
    fn contracts_with_other_clauses() {
        let src = r#"surface S {
    facing user: User
    contracts:
        demands Auditable
    exposes:
        user.name
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.items.len(), 3);
    }

    #[test]
    fn contracts_only_surface() {
        let src = "surface S {\n    contracts:\n        demands Foo\n        fulfils Bar\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.items.len(), 1);
    }

    #[test]
    fn contracts_empty_rejected() {
        let src = "-- allium: 1\nsurface S {\n    contracts:\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("Empty `contracts:`")),
            "expected empty contracts error, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn contracts_inline_block_rejected() {
        let src = "-- allium: 1\nsurface S {\n    contracts:\n        demands Foo {\n        }\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("Inline contract blocks")),
            "expected inline block error, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn contracts_unknown_direction_rejected() {
        let src = "-- allium: 1\nsurface S {\n    contracts:\n        requires Foo\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("Unknown direction")),
            "expected unknown direction error, got: {:?}",
            r.diagnostics
        );
    }

    // -----------------------------------------------------------------------
    // ALP-16: annotations
    // -----------------------------------------------------------------------

    #[test]
    fn annotation_invariant() {
        let src = "contract C {\n    @invariant Determinism\n        -- all evaluations must be deterministic\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Annotation(ann) = &b.items[0].kind else {
            panic!("expected Annotation, got {:?}", b.items[0].kind)
        };
        assert!(matches!(ann.kind, AnnotationKind::Invariant));
        assert_eq!(ann.name.as_ref().unwrap().name, "Determinism");
        assert_eq!(ann.body.len(), 1);
        assert_eq!(ann.body[0], "all evaluations must be deterministic");
    }

    #[test]
    fn annotation_multiple_invariants() {
        let src = "contract C {\n    @invariant A\n        -- first\n    @invariant B\n        -- second\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.items.len(), 2);
        assert!(matches!(&b.items[0].kind, BlockItemKind::Annotation(_)));
        assert!(matches!(&b.items[1].kind, BlockItemKind::Annotation(_)));
    }

    #[test]
    fn annotation_invariant_then_guidance() {
        let src = "contract C {\n    @invariant Safety\n        -- must be safe\n    @guidance\n        -- implementation notes\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.items.len(), 2);
    }

    #[test]
    fn annotation_guidance_in_rule() {
        let src = "rule R {\n    when: Event.created\n    ensures: something\n    @guidance\n        -- do it this way\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let last = b.items.last().unwrap();
        let BlockItemKind::Annotation(ann) = &last.kind else { panic!() };
        assert!(matches!(ann.kind, AnnotationKind::Guidance));
        assert!(ann.name.is_none());
    }

    #[test]
    fn annotation_guarantee() {
        let src = "surface S {\n    @guarantee ResponseTime\n        -- must respond within 100ms\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Annotation(ann) = &b.items[0].kind else { panic!() };
        assert!(matches!(ann.kind, AnnotationKind::Guarantee));
        assert_eq!(ann.name.as_ref().unwrap().name, "ResponseTime");
    }

    #[test]
    fn annotation_guarantee_then_guidance() {
        let src = "surface S {\n    @guarantee Fast\n        -- sub-second\n    @guidance\n        -- cache aggressively\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.items.len(), 2);
    }

    #[test]
    fn annotation_contracts_guarantee_guidance() {
        let src = r#"surface S {
    contracts:
        demands Auditable
    @guarantee ResponseTime
        -- fast
    @guidance
        -- notes
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.items.len(), 3);
    }

    #[test]
    fn annotation_multiline_body() {
        let src = "contract C {\n    @invariant Multi\n        -- line one\n        -- line two\n        -- line three\n}";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Annotation(ann) = &b.items[0].kind else { panic!() };
        assert_eq!(ann.body.len(), 3);
        assert_eq!(ann.body[0], "line one");
        assert_eq!(ann.body[2], "line three");
    }

    #[test]
    fn annotation_empty_body_rejected() {
        let src = "-- allium: 1\ncontract C {\n    @invariant NoBody\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("at least one indented comment line")),
            "expected empty body error, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn annotation_unknown_keyword_rejected() {
        let src = "-- allium: 1\ncontract C {\n    @note Something\n        -- text\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("Unknown annotation")),
            "expected unknown annotation error, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn expression_invariant_still_works() {
        let src = r#"entity E {
    status: pending | active
    invariant AllValid {
        this.status = active
    }
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        // Find the invariant block item
        let inv = b.items.iter().find(|i| matches!(&i.kind, BlockItemKind::InvariantBlock { .. }));
        assert!(inv.is_some(), "expression-bearing invariant should still parse");
    }

    #[test]
    fn invariant_colon_form_migration() {
        let src = "-- allium: 1\ncontract C {\n    invariant: SomeName\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("`invariant:` syntax was replaced")),
            "expected migration diagnostic, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn guidance_colon_form_migration() {
        let src = "-- allium: 1\nrule R {\n    when: Event.created\n    ensures: something\n    guidance: \"do it\"\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("`guidance:` syntax was replaced")),
            "expected migration diagnostic, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn guarantee_colon_form_migration() {
        let src = "-- allium: 1\nsurface S {\n    guarantee: \"fast\"\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("`guarantee:` syntax was replaced")),
            "expected migration diagnostic, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn annotation_guidance_with_name_rejected() {
        let src = "-- allium: 1\ncontract C {\n    @guidance Named\n        -- text\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("does not take a name")),
            "expected guidance name error, got: {:?}",
            r.diagnostics
        );
    }

    // -----------------------------------------------------------------------
    // ALP-11 part 2: expression-bearing invariants
    // -----------------------------------------------------------------------

    #[test]
    fn invariant_top_level_simple() {
        let src = r#"invariant PositiveBalance {
    this.balance > 0
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Invariant(inv) = &r.module.declarations[0] else {
            panic!("expected Invariant, got {:?}", r.module.declarations[0])
        };
        assert_eq!(inv.name.name, "PositiveBalance");
    }

    #[test]
    fn invariant_top_level_for_quantifier() {
        let src = r#"invariant AllPositive {
    for item in items: item.value > 0
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Invariant(inv) = &r.module.declarations[0] else { panic!() };
        assert!(matches!(inv.body, Expr::For { .. }));
    }

    #[test]
    fn invariant_top_level_nested_for() {
        let src = r#"invariant NestedFor {
    for a in items: for b in a.children: b.valid = true
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn invariant_top_level_implies() {
        let src = r#"invariant ImpliesTest {
    this.active implies this.balance > 0
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Invariant(inv) = &r.module.declarations[0] else { panic!() };
        assert!(matches!(inv.body, Expr::LogicalOp { op: LogicalOp::Implies, .. }));
    }

    #[test]
    fn invariant_top_level_let_binding() {
        let src = r#"invariant WithLet {
    let total = this.items.count()
    total > 0
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn invariant_top_level_collection_ops() {
        let src = r#"invariant CollectionOps {
    this.items where active = true
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn invariant_top_level_exists() {
        let src = r#"invariant ExistsCheck {
    exists this.primary_contact
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn invariant_top_level_not_exists() {
        let src = r#"invariant NotExistsCheck {
    not exists this.deleted_at
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn invariant_top_level_optional_navigation() {
        let src = r#"invariant OptionalNav {
    this.owner?.email ?? "none" != "none"
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn invariant_top_level_lowercase_rejected() {
        let src = "-- allium: 1\ninvariant bad { true }";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("uppercase")),
            "expected uppercase error, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn invariant_entity_level() {
        let src = r#"entity Account {
    balance: Decimal
    invariant NonNegative { this.balance >= 0 }
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::InvariantBlock { name, body: _ } = &b.items[1].kind else {
            panic!("expected InvariantBlock, got {:?}", b.items[1].kind)
        };
        assert_eq!(name.name, "NonNegative");
    }

    #[test]
    fn invariant_entity_level_this_ref() {
        let src = r#"entity Order {
    total: Decimal
    invariant PositiveTotal { this.total > 0 }
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn invariant_entity_level_implies() {
        let src = r#"entity Subscription {
    active: Boolean
    balance: Decimal
    invariant ActiveMeansPositive { this.active implies this.balance > 0 }
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn invariant_entity_level_lowercase_rejected() {
        let src = "-- allium: 1\nentity E { invariant bad { true } }";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("uppercase")),
            "expected uppercase error, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn invariant_colon_form_in_entity_migration() {
        // Old `invariant:` colon form should emit migration diagnostic
        let src = "-- allium: 1\nentity E {\n    invariant: -- must be valid\n}";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("`invariant:` syntax was replaced")),
            "expected migration diagnostic, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn invariant_top_level_colon_rejected() {
        // Colon-delimited body at top level is wrong syntax
        let src = "-- allium: 1\ninvariant Bad: some text";
        let r = parse(src);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for colon-delimited invariant at top level, got: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn invariant_same_name_different_scopes() {
        // Top-level and entity-level invariants with same name are valid (parser doesn't check)
        let src = r#"invariant SameName { true }
entity E {
    invariant SameName { true }
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -----------------------------------------------------------------------
    // ALP-10: cross-module config references
    // -----------------------------------------------------------------------

    #[test]
    fn config_qualified_reference() {
        // `core/config.max_batch_size` should parse as MemberAccess(QualifiedName, field)
        let src = r#"config {
    param: Integer = core/config.max_batch_size
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_multiple_qualified_refs() {
        let src = r#"config {
    param_a: Integer = core/config.max_batch_size
    param_b: Duration = core/config.default_delay
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_qualified_ref_with_type() {
        let src = r#"config {
    publish_delay: Duration = core/config.default_delay
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_qualified_chain() {
        // Parameter with qualified default that itself could have a qualified default
        let src = r#"config {
    first: Integer = core/config.base
    second: Integer = first
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_renamed_param_with_qualified_ref() {
        let src = r#"config {
    my_timeout: Duration = core/config.base_timeout
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -----------------------------------------------------------------------
    // ALP-13: expression-form config defaults
    // -----------------------------------------------------------------------

    #[test]
    fn config_default_arithmetic() {
        let src = r#"config {
    param: Integer = other_param + 1
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_default_qualified_arithmetic() {
        let src = r#"config {
    param: Duration = core/config.timeout * 2
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_default_parenthesised() {
        let src = r#"config {
    param: Integer = (base + 1) * factor
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_default_two_qualified_refs() {
        let src = r#"config {
    param: Duration = core/config.a + core/config.b
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_default_literal_only() {
        let src = r#"config {
    param: Integer = 5
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_default_decimal_literal() {
        let src = r#"config {
    param: Decimal = price * 1.5
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_default_mixed_operators() {
        let src = r#"config {
    param: Duration = timeout * 2 + 1.minute
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn config_default_operator_precedence() {
        // a + b * c should be Add(a, Mul(b, c))
        let src = r#"config {
    param: Integer = a + b * c
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    // -----------------------------------------------------------------------
    // Version 2 marker
    // -----------------------------------------------------------------------

    #[test]
    fn version_2_accepted() {
        let r = parse("-- allium: 2\nentity User {}");
        assert_eq!(r.module.version, Some(2));
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn version_99_still_rejected() {
        let r = parse("-- allium: 99\nentity User {}");
        assert!(r.diagnostics.iter().any(|d|
            d.severity == Severity::Error && d.message.contains("unsupported")
        ));
    }

    #[test]
    fn contract_typed_signature() {
        let src = r#"contract Codec {
    serialize: (value: Any) -> ByteArray
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.kind, BlockKind::Contract);
        let BlockItemKind::Assignment { name, value } = &b.items[0].kind else { panic!() };
        assert_eq!(name.name, "serialize");
        assert!(matches!(value, Expr::ProjectionMap { .. }));
    }

    #[test]
    fn contract_multi_param_signature() {
        let src = r#"contract Codec {
    serialize: (value: Any, format: String) -> ByteArray
}"#;
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn comma_separated_entity_fields() {
        let src = "entity Point { x: Decimal, y: Decimal }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.items.len(), 2);
        assert!(matches!(&b.items[0].kind, BlockItemKind::Assignment { name, .. } if name.name == "x"));
        assert!(matches!(&b.items[1].kind, BlockItemKind::Assignment { name, .. } if name.name == "y"));
    }

    #[test]
    fn comma_separated_value_fields() {
        let src = "value Coord { x: Integer, y: Integer }";
        let r = parse_ok(src);
        assert_eq!(r.diagnostics.len(), 0);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.items.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Version 3: transitions, produces, consumes
    // -----------------------------------------------------------------------

    #[test]
    fn version_3_accepted() {
        let r = parse("-- allium: 3\nentity User {}");
        assert_eq!(r.module.version, Some(3));
        assert_eq!(r.diagnostics.len(), 0);
    }

    #[test]
    fn transitions_block_basic() {
        let src = r#"-- allium: 3
entity Order {
    status: pending | confirmed | shipped | delivered | cancelled

    transitions status {
        pending -> confirmed
        confirmed -> shipped
        shipped -> delivered
        pending -> cancelled
        confirmed -> cancelled
        terminal: delivered, cancelled
    }
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        // items[0] is the status field, items[1] is the transitions block
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else {
            panic!("expected TransitionsBlock, got {:?}", b.items[1].kind)
        };
        assert_eq!(graph.field.name, "status");
        assert_eq!(graph.edges.len(), 5);
        assert_eq!(graph.edges[0].from.name, "pending");
        assert_eq!(graph.edges[0].to.name, "confirmed");
        assert_eq!(graph.terminal.len(), 2);
        assert_eq!(graph.terminal[0].name, "delivered");
        assert_eq!(graph.terminal[1].name, "cancelled");
    }

    #[test]
    fn transitions_block_no_terminal() {
        let src = r#"-- allium: 3
entity Task {
    status: open | closed
    transitions status {
        open -> closed
    }
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else {
            panic!("expected TransitionsBlock")
        };
        assert_eq!(graph.edges.len(), 1);
        assert!(graph.terminal.is_empty());
    }

    #[test]
    fn produces_emits_migration_warning() {
        let src = r#"-- allium: 3
rule ShipOrder {
    when: ShipOrder(order, tracking)
    requires: order.status = picking
    produces: tracking_number, shipped_at
    ensures: order.status = shipped
}"#;
        let r = parse(src);
        let warnings: Vec<_> = r.diagnostics.iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert!(
            warnings.iter().any(|d| d.message.contains("`produces:` clauses are removed")),
            "expected migration warning for produces, got: {:?}", warnings
        );
    }

    #[test]
    fn consumes_emits_migration_warning() {
        let src = r#"-- allium: 3
rule ReadOrder {
    when: Check(order)
    consumes: warehouse_assignment
    ensures: order.verified = true
}"#;
        let r = parse(src);
        let warnings: Vec<_> = r.diagnostics.iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert!(
            warnings.iter().any(|d| d.message.contains("`consumes:` clauses are removed")),
            "expected migration warning for consumes, got: {:?}", warnings
        );
    }

    #[test]
    fn when_clause_on_field() {
        let src = r#"-- allium: 3
entity Order {
    status: pending | shipped | delivered
    tracking_number: String when status = shipped | delivered
    transitions status {
        pending -> shipped
        shipped -> delivered
        terminal: delivered
    }
}"#;
        let r = parse(src);
        let errors: Vec<_> = r.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 0, "unexpected errors: {:?}", errors);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let field_with_when = b.items.iter().find(|i| matches!(&i.kind, BlockItemKind::FieldWithWhen { .. }));
        assert!(field_with_when.is_some(), "expected FieldWithWhen item");
        if let BlockItemKind::FieldWithWhen { name, when_clause, .. } = &field_with_when.unwrap().kind {
            assert_eq!(name.name, "tracking_number");
            assert_eq!(when_clause.status_field.name, "status");
            assert_eq!(when_clause.qualifying_states.len(), 2);
            assert_eq!(when_clause.qualifying_states[0].name, "shipped");
            assert_eq!(when_clause.qualifying_states[1].name, "delivered");
        }
    }

    #[test]
    fn when_clause_single_state() {
        let src = r#"-- allium: 3
entity Order {
    status: active | cancelled
    cancelled_at: Timestamp when status = cancelled
    transitions status {
        active -> cancelled
        terminal: cancelled
    }
}"#;
        let r = parse(src);
        let errors: Vec<_> = r.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 0, "unexpected errors: {:?}", errors);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        if let BlockItemKind::FieldWithWhen { name, when_clause, .. } = &b.items[1].kind {
            assert_eq!(name.name, "cancelled_at");
            assert_eq!(when_clause.qualifying_states.len(), 1);
            assert_eq!(when_clause.qualifying_states[0].name, "cancelled");
        } else {
            panic!("expected FieldWithWhen, got {:?}", b.items[1].kind);
        }
    }

    #[test]
    fn when_clause_with_optional() {
        let src = r#"-- allium: 3
entity Order {
    status: active | cancelled
    notes: String? when status = cancelled
    transitions status {
        active -> cancelled
        terminal: cancelled
    }
}"#;
        let r = parse(src);
        let errors: Vec<_> = r.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 0, "unexpected errors: {:?}", errors);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        if let BlockItemKind::FieldWithWhen { name, value, when_clause } = &b.items[1].kind {
            assert_eq!(name.name, "notes");
            assert!(matches!(value, Expr::TypeOptional { .. }), "expected TypeOptional");
            assert_eq!(when_clause.qualifying_states.len(), 1);
        } else {
            panic!("expected FieldWithWhen, got {:?}", b.items[1].kind);
        }
    }

    #[test]
    fn transitions_in_json_output() {
        let src = r#"-- allium: 3
entity Order {
    status: pending | done
    transitions status {
        pending -> done
        terminal: done
    }
}"#;
        let r = parse(src);
        let json = serde_json::to_string(&r.module).unwrap();
        assert!(json.contains("TransitionsBlock"), "JSON should contain TransitionsBlock: {}", json);
        assert!(json.contains("pending"), "JSON should contain 'pending'");
    }

    #[test]
    fn transitions_block_with_commas() {
        let src = r#"-- allium: 3
entity Order {
    status: a | b | c
    transitions status {
        a -> b,
        b -> c,
        terminal: c,
    }
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else {
            panic!("expected TransitionsBlock")
        };
        assert_eq!(graph.edges.len(), 2);
        assert_eq!(graph.terminal.len(), 1);
    }

    #[test]
    fn v3_full_entity_with_transitions_and_rule() {
        let src = r#"-- allium: 3
entity Order {
    status: pending | shipped | delivered
    tracking: String when status = shipped | delivered
    shipped_at: Timestamp when status = shipped | delivered

    transitions status {
        pending -> shipped
        shipped -> delivered
        terminal: delivered
    }
}

rule ShipOrder {
    when: ShipOrder(order, tracking)
    requires: order.status = pending
    ensures:
        order.status = shipped
        order.tracking = tracking
        order.shipped_at = now
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        assert_eq!(r.module.declarations.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Transition graph: edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn transitions_empty_block() {
        let src = "-- allium: 3\nentity E {\n    status: a | b\n    transitions status {}\n}";
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else {
            panic!("expected TransitionsBlock, got {:?}", b.items[1].kind)
        };
        assert!(graph.edges.is_empty());
        assert!(graph.terminal.is_empty());
    }

    #[test]
    fn transitions_terminal_only() {
        let src = r#"-- allium: 3
entity E {
    status: done
    transitions status {
        terminal: done
    }
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else { panic!() };
        assert!(graph.edges.is_empty());
        assert_eq!(graph.terminal.len(), 1);
        assert_eq!(graph.terminal[0].name, "done");
    }

    #[test]
    fn transitions_terminal_before_edges() {
        let src = r#"-- allium: 3
entity E {
    status: a | b | c
    transitions status {
        terminal: c
        a -> b
        b -> c
    }
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else { panic!() };
        assert_eq!(graph.edges.len(), 2);
        assert_eq!(graph.terminal.len(), 1);
        assert_eq!(graph.terminal[0].name, "c");
    }

    #[test]
    fn transitions_self_loop() {
        let src = r#"-- allium: 3
entity E {
    status: running | stopped
    transitions status {
        running -> running
        running -> stopped
        terminal: stopped
    }
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else { panic!() };
        assert_eq!(graph.edges.len(), 2);
        assert_eq!(graph.edges[0].from.name, "running");
        assert_eq!(graph.edges[0].to.name, "running");
    }

    #[test]
    fn transitions_single_edge() {
        let src = "-- allium: 3\nentity E {\n    s: a | b\n    transitions s { a -> b }\n}";
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else { panic!() };
        assert_eq!(graph.field.name, "s");
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn transitions_multiple_terminal_values() {
        let src = r#"-- allium: 3
entity E {
    status: a | b | c | d | e
    transitions status {
        a -> b
        b -> c
        terminal: c, d, e
    }
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else { panic!() };
        assert_eq!(graph.terminal.len(), 3);
    }

    #[test]
    fn transitions_trailing_comma_in_terminal() {
        let src = "-- allium: 3\nentity E {\n    s: a | b\n    transitions s {\n        a -> b\n        terminal: b,\n    }\n}";
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else { panic!() };
        assert_eq!(graph.terminal.len(), 1);
    }

    #[test]
    fn transitions_among_other_entity_items() {
        // Transitions block between fields, relationships, invariants
        let src = r#"-- allium: 3
entity Order {
    status: pending | shipped | delivered
    customer: Customer
    tracking: String?

    transitions status {
        pending -> shipped
        shipped -> delivered
        terminal: delivered
    }

    active_items: items where status = active
    invariant Positive { this.total > 0 }
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        // Should have: status, customer, tracking, transitions, active_items, invariant
        assert_eq!(b.items.len(), 6);
        assert!(matches!(&b.items[3].kind, BlockItemKind::TransitionsBlock(_)));
        assert!(matches!(&b.items[5].kind, BlockItemKind::InvariantBlock { .. }));
    }

    #[test]
    fn transitions_error_recovery_missing_arrow() {
        let src = r#"-- allium: 3
entity E {
    status: a | b | c
    transitions status {
        a b
        b -> c
    }
}"#;
        let r = parse(src);
        // Should get a diagnostic about missing `->`
        assert!(r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "expected error for missing arrow, got: {:?}", r.diagnostics);
        // But should still parse the valid edge
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else {
            panic!("expected TransitionsBlock")
        };
        assert_eq!(graph.edges.len(), 1, "should recover and parse second edge");
        assert_eq!(graph.edges[0].from.name, "b");
    }

    #[test]
    fn transitions_field_name_preserved() {
        let src = "-- allium: 3\nentity E {\n    phase: x | y\n    transitions phase { x -> y }\n}";
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else { panic!() };
        assert_eq!(graph.field.name, "phase");
    }

    #[test]
    fn transitions_diamond_topology() {
        // Common pattern: multiple paths converge on a state
        let src = r#"-- allium: 3
entity E {
    status: new | path_a | path_b | done
    transitions status {
        new -> path_a
        new -> path_b
        path_a -> done
        path_b -> done
        terminal: done
    }
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else { panic!() };
        assert_eq!(graph.edges.len(), 4);
    }

    #[test]
    fn transitions_edge_span_is_from_to_range() {
        let src = "-- allium: 3\nentity E {\n    s: a | b\n    transitions s {\n        a -> b\n    }\n}";
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::TransitionsBlock(graph) = &b.items[1].kind else { panic!() };
        let edge = &graph.edges[0];
        // Span should cover from `a` through `b`, not include the arrow
        assert!(edge.span.start <= edge.from.span.start);
        assert!(edge.span.end >= edge.to.span.end);
    }

    // -----------------------------------------------------------------------
    // When clause: edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn when_clause_multiple_fields() {
        let src = r#"-- allium: 3
entity Order {
    status: pending | shipped | delivered
    tracking: String when status = shipped | delivered
    shipped_at: Timestamp when status = shipped | delivered
    delivered_at: Timestamp when status = delivered
    transitions status {
        pending -> shipped
        shipped -> delivered
        terminal: delivered
    }
}"#;
        let r = parse(src);
        let errors: Vec<_> = r.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
        assert_eq!(errors.len(), 0, "unexpected errors: {:?}", errors);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let when_count = b.items.iter()
            .filter(|i| matches!(&i.kind, BlockItemKind::FieldWithWhen { .. }))
            .count();
        assert_eq!(when_count, 3);
    }

    #[test]
    fn legacy_produces_consumes_skipped_with_warnings() {
        let src = r#"-- allium: 3
rule R {
    when: Go(x)
    produces: field_a
    consumes: field_b
    ensures: x.done = true
}"#;
        let r = parse(src);
        let warnings: Vec<_> = r.diagnostics.iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert!(warnings.len() >= 2, "expected at least 2 migration warnings, got {}", warnings.len());
        // The produces/consumes items should be dropped from the AST
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert!(
            !b.items.iter().any(|i| matches!(&i.kind, BlockItemKind::FieldWithWhen { .. })),
            "legacy produces/consumes should not become FieldWithWhen"
        );
    }

    // -----------------------------------------------------------------------
    // v3 interactions: transitions + produces/consumes + other constructs
    // -----------------------------------------------------------------------

    #[test]
    fn v3_entity_with_transitions_and_invariant() {
        let src = r#"-- allium: 3
entity Account {
    status: open | frozen | closed
    balance: Decimal

    transitions status {
        open -> frozen
        frozen -> open
        open -> closed
        frozen -> closed
        terminal: closed
    }

    invariant NonNegative { this.balance >= 0 }
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert!(b.items.iter().any(|i| matches!(&i.kind, BlockItemKind::TransitionsBlock(_))));
        assert!(b.items.iter().any(|i| matches!(&i.kind, BlockItemKind::InvariantBlock { .. })));
    }

    #[test]
    fn v3_rule_with_multiple_ensures() {
        let src = r#"-- allium: 3
rule CompleteOrder {
    when: Complete(order)
    requires: order.status = shipped
    ensures: order.status = delivered
    ensures: order.completed_at = now
    ensures: order.receipt_number = generate_receipt()
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let ensures_count = b.items.iter()
            .filter(|i| matches!(&i.kind, BlockItemKind::Clause { keyword, .. } if keyword == "ensures"))
            .count();
        assert_eq!(ensures_count, 3);
    }

    #[test]
    fn v3_rule_with_if_block() {
        let src = r#"-- allium: 3
rule Cancel {
    when: Cancel(order, reason)
    requires: order.status != delivered
    ensures:
        order.status = cancelled
        order.cancelled_at = now
        if reason = customer_request:
            order.cancelled_by = order.customer
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
    }

    #[test]
    fn v3_complete_lifecycle_spec() {
        let src = r#"-- allium: 3

entity Subscription {
    status: trial | active | past_due | cancelled
    started_at: Timestamp when status = active | past_due | cancelled
    cancelled_at: Timestamp when status = cancelled
    balance: Decimal

    transitions status {
        trial -> active
        active -> past_due
        past_due -> active
        active -> cancelled
        past_due -> cancelled
        terminal: cancelled
    }

    invariant NonNegative { this.balance >= 0 }
}

config {
    trial_period: Duration = 14.days
}

rule ActivateSubscription {
    when: Activate(sub)
    requires: sub.status = trial
    ensures:
        sub.status = active
        sub.started_at = now
}

rule CancelSubscription {
    when: Cancel(sub)
    requires: sub.status != cancelled
    ensures:
        sub.status = cancelled
        sub.cancelled_at = now
}

invariant AllCancelledHaveTimestamp {
    for sub in Subscriptions where status = cancelled:
        sub.cancelled_at != null
}

surface SubscriptionDashboard {
    facing user: User
    context sub: Subscription where owner = user
    exposes:
        sub.status
        sub.balance
    provides:
        Cancel(sub) when sub.status != cancelled
}
"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        // entity, config, 2 rules, invariant, surface = 6 declarations
        assert_eq!(r.module.declarations.len(), 6);
    }

    #[test]
    fn v3_produces_consumes_are_field_names_in_entities() {
        // In non-rule blocks, `produces` and `consumes` are just field names
        let src = r#"-- allium: 3
entity Factory {
    produces: widget_a
    consumes: raw_material
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert!(matches!(&b.items[0].kind, BlockItemKind::Assignment { name, .. } if name.name == "produces"));
        assert!(matches!(&b.items[1].kind, BlockItemKind::Assignment { name, .. } if name.name == "consumes"));
    }

    #[test]
    fn v3_legacy_produces_consumes_emit_warnings_in_rules() {
        let src = r#"-- allium: 3
rule Ship {
    when: Ship(order)
    produces: tracking_number
    consumes: warehouse
    ensures: order.status = shipped
}"#;
        let r = parse(src);
        let warnings: Vec<_> = r.diagnostics.iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect();
        assert!(warnings.len() >= 2, "expected migration warnings, got {:?}", warnings);
    }

    #[test]
    fn v3_version_preserved_in_module() {
        let src = "-- allium: 3\nentity E {}";
        let r = parse(src);
        assert_eq!(r.module.version, Some(3));
    }

    #[test]
    fn v3_version_4_still_rejected() {
        let src = "-- allium: 4\nentity E {}";
        let r = parse(src);
        assert!(r.diagnostics.iter().any(|d| d.severity == Severity::Error
            && d.message.contains("unsupported")));
    }

    // -----------------------------------------------------------------------
    // Backtick-quoted enum literals
    // -----------------------------------------------------------------------

    #[test]
    fn backtick_in_named_enum() {
        let src = "-- allium: 3\nenum Locale { en | fr | `de-CH-1996` | `no-cache` }";
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.items.len(), 4);
        // First two are unquoted
        let BlockItemKind::EnumVariant { name, backtick_quoted } = &b.items[0].kind else { panic!() };
        assert_eq!(name.name, "en");
        assert!(!backtick_quoted);
        // Third is backtick-quoted
        let BlockItemKind::EnumVariant { name, backtick_quoted } = &b.items[2].kind else { panic!() };
        assert_eq!(name.name, "de-CH-1996");
        assert!(backtick_quoted);
        // Fourth is backtick-quoted
        let BlockItemKind::EnumVariant { name, backtick_quoted } = &b.items[3].kind else { panic!() };
        assert_eq!(name.name, "no-cache");
        assert!(backtick_quoted);
    }

    #[test]
    fn backtick_in_inline_enum() {
        let src = "-- allium: 3\nentity E { cache: `no-cache` | `no-store` | `public` }";
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        let BlockItemKind::Assignment { value, .. } = &b.items[0].kind else { panic!() };
        // Top-level should be Pipe chains of BacktickLiteral
        assert!(matches!(value, Expr::Pipe { .. }));
    }

    #[test]
    fn backtick_in_comparison() {
        let src = r#"-- allium: 3
rule R {
    when: Check(item)
    requires: item.locale = `de-CH-1996`
    ensures: Done()
}"#;
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
    }

    #[test]
    fn backtick_mixed_with_unquoted() {
        let src = "-- allium: 3\nenum CacheDirective { `no-cache` | `no-store` | public | private }";
        let r = parse(src);
        assert_eq!(r.diagnostics.len(), 0, "unexpected diagnostics: {:?}", r.diagnostics);
        let Decl::Block(b) = &r.module.declarations[0] else { panic!() };
        assert_eq!(b.items.len(), 4);
        let BlockItemKind::EnumVariant { backtick_quoted, .. } = &b.items[0].kind else { panic!() };
        assert!(backtick_quoted);
        let BlockItemKind::EnumVariant { backtick_quoted, .. } = &b.items[2].kind else { panic!() };
        assert!(!backtick_quoted);
    }

    // -----------------------------------------------------------------------
    // V3 fixture file
    // -----------------------------------------------------------------------

    #[test]
    fn v3_lifecycle_fixture() {
        let src = include_str!("../tests/fixtures/v3-lifecycle.allium");
        let r = parse(src);
        let errors: Vec<_> = r.diagnostics.iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert_eq!(
            errors.len(),
            0,
            "expected no errors in v3 lifecycle fixture, got: {:?}",
            errors.iter().map(|d| &d.message).collect::<Vec<_>>(),
        );
    }
}
