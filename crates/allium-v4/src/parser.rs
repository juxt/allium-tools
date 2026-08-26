//! v4 parser. Recursive descent, wholly separate from the v3 parser. Phase 4a
//! parses the declaration and body-item structure; predicate bodies are captured
//! as raw spans (see `ast`). Keywords are matched by text so they stay easy to
//! change while the surface is provisional.

use crate::ast::*;
use crate::diagnostic::Diagnostic;
use crate::lexer::{lex, Tok, Token};
use crate::span::Span;

pub struct ParseResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

/// Item keywords that terminate a raw predicate / type at bracket depth 0.
const ITEM_STARTERS: &[&str] = &[
    "entity", "observable", "state", "given", "action", "init", "invariant",
    "guarantee", "establish", "pub", "abstract", "readable", "contract",
    "component", "end", "satisfies",
];

fn is_starter(s: &str) -> bool {
    ITEM_STARTERS.contains(&s)
}

pub fn parse(source: &str) -> ParseResult {
    let tokens = lex(source);
    let mut p = Parser { source, tokens, pos: 0, diagnostics: Vec::new() };
    let module = p.parse_module();
    ParseResult { module, diagnostics: p.diagnostics }
}

struct Parser<'s> {
    source: &'s str,
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'s> Parser<'s> {
    fn cur(&self) -> &Token {
        &self.tokens[self.pos]
    }
    fn span(&self) -> Span {
        self.cur().span
    }
    fn at_eof(&self) -> bool {
        matches!(self.cur().tok, Tok::Eof)
    }
    fn advance(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }
    /// Text if the current token is an identifier.
    fn cur_ident(&self) -> Option<String> {
        match &self.cur().tok {
            Tok::Ident(s) => Some(s.clone()),
            _ => None,
        }
    }
    fn is_ident(&self, kw: &str) -> bool {
        matches!(&self.cur().tok, Tok::Ident(s) if s == kw)
    }
    fn eat_ident(&mut self, kw: &str) -> bool {
        if self.is_ident(kw) {
            self.advance();
            true
        } else {
            false
        }
    }
    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(span, msg));
    }

    fn parse_module(&mut self) -> Module {
        let start = self.span();
        let version = detect_version(self.source);
        let mut decls = Vec::new();
        while !self.at_eof() {
            if self.is_ident("contract") || self.is_ident("component") {
                decls.push(self.parse_decl());
            } else {
                let found = self.token_desc();
                self.error(self.span(), format!("expected a `contract` or `component` declaration, found {found}"));
                self.advance();
            }
        }
        let end = self.span();
        Module { version, span: start.merge(end), decls }
    }

    fn token_desc(&self) -> String {
        match &self.cur().tok {
            Tok::Ident(s) => format!("`{s}`"),
            Tok::Eof => "end of file".into(),
            other => format!("{other:?}"),
        }
    }

    fn parse_decl(&mut self) -> Decl {
        let start = self.span();
        let kind = if self.eat_ident("contract") {
            DeclKind::Contract
        } else {
            self.eat_ident("component");
            DeclKind::Component
        };
        let name = self.cur_ident().unwrap_or_default();
        if name.is_empty() {
            self.error(self.span(), "expected a name after the declaration keyword");
        } else {
            self.advance();
        }
        let params = if matches!(self.cur().tok, Tok::LParen) {
            self.parse_params()
        } else {
            Vec::new()
        };
        let satisfies = if self.eat_ident("satisfies") {
            self.parse_params()
        } else {
            Vec::new()
        };

        let mut items = Vec::new();
        while !self.at_eof() && !self.is_ident("end") {
            let before = self.pos;
            if let Some(item) = self.parse_item() {
                items.push(item);
            }
            if self.pos == before {
                // No progress: skip a token to guarantee termination.
                self.advance();
            }
        }
        let end = self.span();
        if !self.eat_ident("end") {
            self.error(end, format!("expected `end` to close `{name}`, found {}", self.token_desc()));
        }
        Decl { span: start.merge(end), kind, name, params, satisfies, items }
    }

    /// `( name [: type] (, name [: type])* )`. Types captured as raw text.
    fn parse_params(&mut self) -> Vec<Param> {
        let mut out = Vec::new();
        if !matches!(self.cur().tok, Tok::LParen) {
            return out;
        }
        self.advance(); // (
        while !matches!(self.cur().tok, Tok::RParen | Tok::Eof) {
            let start = self.span();
            let name = self.cur_ident().unwrap_or_default();
            if name.is_empty() {
                self.advance();
                continue;
            }
            self.advance();
            let mut ty = String::new();
            if matches!(self.cur().tok, Tok::Colon) {
                self.advance();
                if let Some(sp) = self.read_raw(&[], /*stop_paren*/ true) {
                    ty = sp.slice(self.source).trim().to_string();
                }
            }
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            out.push(Param { span: start.merge(end), name, ty });
            if matches!(self.cur().tok, Tok::Comma) {
                self.advance();
            }
        }
        self.eat(Tok::RParen);
        out
    }

    fn eat(&mut self, t: Tok) -> bool {
        if std::mem::discriminant(&self.cur().tok) == std::mem::discriminant(&t) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn parse_item(&mut self) -> Option<Item> {
        let start = self.span();
        let mut modifiers = Vec::new();
        while self.is_ident("pub") || self.is_ident("abstract") || self.is_ident("readable") {
            modifiers.push(self.cur_ident().unwrap());
            self.advance();
        }

        let kw = self.cur_ident();
        let mut item = match kw.as_deref() {
            Some("entity") => {
                self.advance();
                let mut it = Item::new(ItemKind::Entity, start);
                it.name = self.take_name();
                if matches!(self.cur().tok, Tok::LBrace) {
                    self.skip_balanced(); // record body { ... }
                }
                it
            }
            Some("observable") => {
                self.advance();
                self.eat_ident("state");
                self.parse_named_typed(ItemKind::State, start)
            }
            Some("state") => {
                self.advance();
                self.parse_named_typed(ItemKind::State, start)
            }
            Some("given") => {
                self.advance();
                self.parse_named_typed(ItemKind::Given, start)
            }
            Some("action") => {
                self.advance();
                self.parse_action(start)
            }
            Some("init") => {
                self.advance();
                let mut it = Item::new(ItemKind::Init, start);
                it.body = self.read_raw(ITEM_STARTERS, false);
                it
            }
            Some("invariant") => {
                self.advance();
                self.parse_named_pred(ItemKind::Invariant, start)
            }
            Some("guarantee") => {
                self.advance();
                self.parse_named_pred(ItemKind::Guarantee, start)
            }
            Some("establish") => {
                self.advance();
                self.parse_establish(start)
            }
            Some(_) => {
                // A bare projection: `name(args) : type`, e.g. `readable balance(Handle) : Cur.Amount`.
                self.parse_named_typed(ItemKind::Given, start)
            }
            None => {
                self.error(start, format!("expected a body item, found {}", self.token_desc()));
                return None;
            }
        };
        item.modifiers = modifiers;
        item.span = start.merge(self.tokens[self.pos.saturating_sub(1)].span);
        Some(item)
    }

    fn take_name(&mut self) -> Option<String> {
        if let Some(n) = self.cur_ident() {
            self.advance();
            Some(n)
        } else {
            None
        }
    }

    /// `name [(args)] [: type]`.
    fn parse_named_typed(&mut self, kind: ItemKind, start: Span) -> Item {
        let mut it = Item::new(kind, start);
        it.name = self.take_name();
        if matches!(self.cur().tok, Tok::LParen) {
            self.skip_balanced();
        }
        if matches!(self.cur().tok, Tok::Colon) {
            self.advance();
            it.body = self.read_raw(ITEM_STARTERS, false); // raw type text
        }
        it
    }

    /// `name means <pred>`.
    fn parse_named_pred(&mut self, kind: ItemKind, start: Span) -> Item {
        let word = kind_word(&kind);
        let mut it = Item::new(kind, start);
        it.name = self.take_name();
        if !self.eat_ident("means") {
            self.error(self.span(), format!("expected `means` after the {word} name, found {}", self.token_desc()));
        }
        it.body = self.read_raw(ITEM_STARTERS, false);
        it
    }

    /// `name [(params)] [requires <pred>] [;] [ensures <pred>]`.
    fn parse_action(&mut self, start: Span) -> Item {
        let mut it = Item::new(ItemKind::Action, start);
        it.name = self.take_name();
        if matches!(self.cur().tok, Tok::LParen) {
            self.skip_balanced();
        }
        if self.eat_ident("requires") {
            it.requires = self.read_raw(ITEM_STARTERS_PLUS_ENSURES, true);
        }
        self.eat(Tok::Semi);
        if self.eat_ident("ensures") {
            it.ensures = self.read_raw(ITEM_STARTERS, false);
        }
        it
    }

    /// `<pred> [by a, b, ...]`.
    fn parse_establish(&mut self, start: Span) -> Item {
        let mut it = Item::new(ItemKind::Establish, start);
        it.body = self.read_raw(ITEM_STARTERS_PLUS_BY, false);
        if self.eat_ident("by") {
            while let Some(n) = self.cur_ident() {
                if is_starter(&n) {
                    break;
                }
                it.witnesses.push(n);
                self.advance();
                if !self.eat(Tok::Comma) {
                    break;
                }
            }
        }
        it
    }

    /// Consume tokens until a stop word (identifier in `stops`) at bracket depth 0,
    /// or `;` if `stop_semi`, or a closing bracket at depth 0, or EOF. Returns the
    /// covering span, or None if nothing was consumed.
    fn read_raw(&mut self, stops: &[&str], stop_semi: bool) -> Option<Span> {
        let start = self.span().start;
        let mut end = start;
        let mut depth: i32 = 0;
        let mut consumed = false;
        loop {
            match &self.cur().tok {
                Tok::Eof => break,
                Tok::LParen | Tok::LBrace | Tok::LBracket => depth += 1,
                Tok::RParen | Tok::RBrace | Tok::RBracket => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Tok::Semi if stop_semi && depth == 0 => break,
                Tok::Ident(s) if depth == 0 && stops.contains(&s.as_str()) => break,
                _ => {}
            }
            end = self.cur().span.end;
            self.advance();
            consumed = true;
        }
        if consumed {
            Some(Span::new(start, end))
        } else {
            None
        }
    }

    /// Skip a balanced `(...)`, `{...}` or `[...]` starting at the current token.
    fn skip_balanced(&mut self) {
        let open = self.cur().tok.clone();
        let close = match open {
            Tok::LParen => Tok::RParen,
            Tok::LBrace => Tok::RBrace,
            Tok::LBracket => Tok::RBracket,
            _ => return,
        };
        let mut depth = 0i32;
        loop {
            match &self.cur().tok {
                Tok::Eof => break,
                t if *t == open => depth += 1,
                t if std::mem::discriminant(t) == std::mem::discriminant(&close) => {
                    depth -= 1;
                    self.advance();
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                _ => {}
            }
            self.advance();
        }
    }
}

const ITEM_STARTERS_PLUS_ENSURES: &[&str] = &[
    "entity", "observable", "state", "given", "action", "init", "invariant",
    "guarantee", "establish", "pub", "abstract", "readable", "contract",
    "component", "end", "satisfies", "ensures",
];

const ITEM_STARTERS_PLUS_BY: &[&str] = &[
    "entity", "observable", "state", "given", "action", "init", "invariant",
    "guarantee", "establish", "pub", "abstract", "readable", "contract",
    "component", "end", "satisfies", "by",
];

fn kind_word(k: &ItemKind) -> &'static str {
    match k {
        ItemKind::Invariant => "invariant",
        ItemKind::Guarantee => "guarantee",
        _ => "item",
    }
}

/// Own version detection over the `-- allium: N` marker (no shared call to v3).
pub fn detect_version(source: &str) -> Option<u32> {
    for line in source.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("--") {
            if let Some(v) = rest.trim().strip_prefix("allium:") {
                return v.trim().parse().ok();
            }
            continue; // other comment before the marker
        }
        break; // first non-comment line
    }
    None
}
