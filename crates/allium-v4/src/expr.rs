//! v4 predicate expression grammar. Turns a predicate's source text into an
//! expression tree. This is the shared enabler for 4b name resolution and 4c
//! discharge; item bodies are captured as raw spans by the declaration parser
//! (see `parser`/`ast`) and parsed here on demand via [`parse_predicate`].
//!
//! Reuses the v4 lexer. Keywords (`every`/`some`/`no`/`exists`/`and`/`or`/`not`/
//! `implies`/`in`/`old`/`means`…) are matched by text, so they stay easy to change.

use serde::Serialize;

use crate::diagnostic::Diagnostic;
use crate::lexer::{lex, Tok, Token};
use crate::span::Span;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum Quant {
    Every,
    Some,
    No,
    ExistsOne,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum BinOp {
    Implies,
    Or,
    And,
    Eq,
    Ne,
    In,
    Lt,
    Gt,
    Le,
    Ge,
    Add,
    Sub,
    Mul,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum UnOp {
    Not,
    Old,
}

#[derive(Debug, Clone, Serialize)]
pub enum Expr {
    /// `<q> x[, y] [: T] :: body`
    Quant { q: Quant, vars: Vec<String>, ty: Option<String>, body: Box<Expr> },
    /// `sum x[, y] [: T] :: term` — a bounded aggregate (SD-3). A numeric TERM, not a
    /// formula: its body binds tighter than any comparison, so `sum p :: f(p) = k`
    /// reads as `(sum p :: f(p)) = k`.
    Sum { vars: Vec<String>, ty: Option<String>, body: Box<Expr> },
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Unary { op: UnOp, e: Box<Expr> },
    /// `head(args)`
    App { head: Box<Expr>, args: Vec<Expr> },
    /// `base.name`
    Field { base: Box<Expr>, name: String },
    Name(String),
    Int(i64),
    /// `{ a | b | c }` enum/set literal — kept as source text for now.
    SetLit(String),
    Error,
}

/// Parse a predicate's source text into an expression tree, plus any diagnostics.
pub fn parse_predicate(text: &str) -> (Expr, Vec<Diagnostic>) {
    let tokens = lex(text);
    let mut p = ExprParser { src: text, tokens, pos: 0, diags: Vec::new() };
    let e = p.expr(0);
    if !matches!(p.cur().tok, Tok::Eof) {
        let sp = p.cur().span;
        p.diags.push(Diagnostic::warning(sp, format!("unexpected trailing tokens in predicate")));
    }
    (e, p.diags)
}

struct ExprParser<'s> {
    src: &'s str,
    tokens: Vec<Token>,
    pos: usize,
    diags: Vec<Diagnostic>,
}

impl<'s> ExprParser<'s> {
    fn cur(&self) -> &Token {
        &self.tokens[self.pos]
    }
    fn adv(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }
    fn is_kw(&self, s: &str) -> bool {
        matches!(&self.cur().tok, Tok::Ident(k) if k == s)
    }
    fn peek_kw(&self, ahead: usize) -> Option<&str> {
        match self.tokens.get(self.pos + ahead).map(|t| &t.tok) {
            Some(Tok::Ident(k)) => Some(k.as_str()),
            _ => None,
        }
    }

    /// Precedence-climbing binary parser.
    fn expr(&mut self, min_bp: u8) -> Expr {
        let mut lhs = self.prefix();
        loop {
            let (op, bp, right) = match self.infix_op() {
                Some(x) => x,
                None => break,
            };
            if bp < min_bp {
                break;
            }
            self.adv();
            let rhs = self.expr(if right { bp } else { bp + 1 });
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        lhs
    }

    /// The binary operator at the cursor, its binding power, and right-assoc flag.
    fn infix_op(&self) -> Option<(BinOp, u8, bool)> {
        let op = match &self.cur().tok {
            Tok::Ident(k) => match k.as_str() {
                "implies" => (BinOp::Implies, 1, true),
                "or" => (BinOp::Or, 2, false),
                "and" => (BinOp::And, 3, false),
                "in" => (BinOp::In, 4, false),
                _ => return None,
            },
            Tok::Eq => (BinOp::Eq, 4, false),
            Tok::NotEq => (BinOp::Ne, 4, false),
            Tok::Lt => (BinOp::Lt, 4, false),
            Tok::Gt => (BinOp::Gt, 4, false),
            Tok::Le => (BinOp::Le, 4, false),
            Tok::Ge => (BinOp::Ge, 4, false),
            Tok::Plus => (BinOp::Add, 5, false),
            Tok::Minus => (BinOp::Sub, 5, false),
            Tok::Star => (BinOp::Mul, 6, false),
            _ => return None,
        };
        Some(op)
    }

    fn prefix(&mut self) -> Expr {
        // Quantifiers: `some`/`no` are also value constructors when followed by `(`.
        if let Tok::Ident(k) = &self.cur().tok {
            let k = k.clone();
            let quant = match k.as_str() {
                "every" => Some(Quant::Every),
                "some" if !matches!(self.tokens.get(self.pos + 1).map(|t| &t.tok), Some(Tok::LParen)) => Some(Quant::Some),
                "no" if !matches!(self.tokens.get(self.pos + 1).map(|t| &t.tok), Some(Tok::LParen)) => Some(Quant::No),
                "exists" => Some(Quant::ExistsOne),
                _ => None,
            };
            if let Some(q) = quant {
                return self.quantifier(q);
            }
            if (k == "sum" || k == "total") && self.binder_follows() {
                return self.aggregate();
            }
            if k == "not" {
                self.adv();
                // `not` binds tighter than and(3)/or(2)/implies(1): its operand is a
                // comparison/atom, so `not a and b` is `(not a) and b`, not `not (a and b)`.
                let e = self.expr(4);
                return Expr::Unary { op: UnOp::Not, e: Box::new(e) };
            }
            if k == "old" {
                self.adv();
                let e = self.postfix_from_atom();
                return Expr::Unary { op: UnOp::Old, e: Box::new(e) };
            }
        }
        self.postfix_from_atom()
    }

    fn quantifier(&mut self, q: Quant) -> Expr {
        self.adv(); // the quantifier keyword
        if matches!(q, Quant::ExistsOne) {
            self.is_kw("one").then(|| self.adv()); // optional `one`
        }
        // Binder variables: comma-separated idents.
        let mut vars = Vec::new();
        loop {
            if let Tok::Ident(v) = &self.cur().tok {
                vars.push(v.clone());
                self.adv();
            }
            if matches!(self.cur().tok, Tok::Comma) {
                self.adv();
                continue;
            }
            break;
        }
        // Optional `: Type` before the body separator `::`. Disambiguated by
        // looking ahead for a `::` at the current bracket level: if one exists, a
        // single `:` introduces the type; otherwise `:` introduces the body.
        let mut ty = None;
        if matches!(self.cur().tok, Tok::Colon) && self.has_double_colon_ahead() {
            self.adv(); // `:`
            let start = self.cur().span.start;
            let mut end = start;
            while !matches!(self.cur().tok, Tok::ColonColon | Tok::Eof) {
                end = self.cur().span.end;
                self.adv();
            }
            ty = Some(self.src.get(start..end).unwrap_or("").trim().to_string());
        }
        // Body separator: `::` (every/some/no) or `:` (exists), accept either.
        if matches!(self.cur().tok, Tok::ColonColon | Tok::Colon) {
            self.adv();
        }
        let body = self.expr(0);
        Expr::Quant { q, vars, ty, body: Box::new(body) }
    }

    /// A `sum`/`total` is an aggregate (not a plain name) when an identifier binder
    /// and a `::` follow it.
    fn binder_follows(&self) -> bool {
        matches!(self.tokens.get(self.pos + 1).map(|t| &t.tok), Some(Tok::Ident(_)))
            && self.has_double_colon_ahead()
    }

    /// `sum x[, y] [: T] :: term`. The body parses at arithmetic precedence (bp 5), so
    /// it stops before a comparison: `sum p :: f(p) = k` is `(sum p :: f(p)) = k`.
    fn aggregate(&mut self) -> Expr {
        self.adv(); // 'sum' / 'total'
        let mut vars = Vec::new();
        loop {
            if let Tok::Ident(v) = &self.cur().tok {
                vars.push(v.clone());
                self.adv();
            }
            if matches!(self.cur().tok, Tok::Comma) {
                self.adv();
                continue;
            }
            break;
        }
        let mut ty = None;
        if matches!(self.cur().tok, Tok::Colon) && self.has_double_colon_ahead() {
            self.adv();
            let start = self.cur().span.start;
            let mut end = start;
            while !matches!(self.cur().tok, Tok::ColonColon | Tok::Eof) {
                end = self.cur().span.end;
                self.adv();
            }
            ty = Some(self.src.get(start..end).unwrap_or("").trim().to_string());
        }
        if matches!(self.cur().tok, Tok::ColonColon | Tok::Colon) {
            self.adv();
        }
        let body = self.expr(5);
        Expr::Sum { vars, ty, body: Box::new(body) }
    }

    fn has_double_colon_ahead(&self) -> bool {
        let mut i = self.pos;
        let mut depth = 0i32;
        while i < self.tokens.len() {
            match &self.tokens[i].tok {
                Tok::Eof => break,
                Tok::LParen | Tok::LBrace | Tok::LBracket => depth += 1,
                Tok::RParen | Tok::RBrace | Tok::RBracket => depth -= 1,
                Tok::ColonColon if depth == 0 => return true,
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn postfix_from_atom(&mut self) -> Expr {
        let atom = self.atom();
        self.postfix(atom)
    }

    fn postfix(&mut self, mut e: Expr) -> Expr {
        loop {
            match &self.cur().tok {
                Tok::Dot => {
                    self.adv();
                    if let Tok::Ident(n) = &self.cur().tok {
                        let n = n.clone();
                        self.adv();
                        e = Expr::Field { base: Box::new(e), name: n };
                    } else {
                        break;
                    }
                }
                Tok::LParen => {
                    self.adv();
                    let mut args = Vec::new();
                    while !matches!(self.cur().tok, Tok::RParen | Tok::Eof) {
                        args.push(self.expr(0));
                        if matches!(self.cur().tok, Tok::Comma) {
                            self.adv();
                        } else {
                            break;
                        }
                    }
                    let _ = self.eat(Tok::RParen);
                    e = Expr::App { head: Box::new(e), args };
                }
                _ => break,
            }
        }
        e
    }

    fn atom(&mut self) -> Expr {
        match self.cur().tok.clone() {
            Tok::LParen => {
                self.adv();
                let e = self.expr(0);
                let _ = self.eat(Tok::RParen);
                e
            }
            Tok::LBrace => {
                // `{ a | b | c }` — capture as source text.
                let start = self.cur().span.start;
                let mut depth = 0i32;
                let mut end = start;
                loop {
                    match &self.cur().tok {
                        Tok::Eof => break,
                        Tok::LBrace => depth += 1,
                        Tok::RBrace => {
                            depth -= 1;
                            end = self.cur().span.end;
                            self.adv();
                            if depth == 0 {
                                break;
                            }
                            continue;
                        }
                        _ => end = self.cur().span.end,
                    }
                    self.adv();
                }
                Expr::SetLit(self.src.get(start..end).unwrap_or("").to_string())
            }
            Tok::Ident(s) => {
                self.adv();
                Expr::Name(s)
            }
            Tok::Int(n) => {
                self.adv();
                Expr::Int(n)
            }
            _ => {
                let sp = self.cur().span;
                self.diags.push(Diagnostic::warning(sp, "expected a term in predicate".to_string()));
                self.adv();
                Expr::Error
            }
        }
    }

    fn eat(&mut self, t: Tok) -> bool {
        if std::mem::discriminant(&self.cur().tok) == std::mem::discriminant(&t) {
            self.adv();
            true
        } else {
            false
        }
    }
}

/// Collect the free names referenced in an expression (excluding quantifier-bound
/// variables). Used by 4b name resolution. Field accessors and call heads count.
pub fn free_names(e: &Expr, bound: &mut Vec<String>, out: &mut Vec<(String, Span)>) {
    match e {
        Expr::Quant { vars, body, .. } | Expr::Sum { vars, body, .. } => {
            let n = vars.len();
            for v in vars {
                bound.push(v.clone());
            }
            free_names(body, bound, out);
            for _ in 0..n {
                bound.pop();
            }
        }
        Expr::Binary { lhs, rhs, .. } => {
            free_names(lhs, bound, out);
            free_names(rhs, bound, out);
        }
        Expr::Unary { e, .. } => free_names(e, bound, out),
        Expr::App { head, args } => {
            free_names(head, bound, out);
            for a in args {
                free_names(a, bound, out);
            }
        }
        Expr::Field { base, .. } => free_names(base, bound, out),
        Expr::Name(s) => {
            if !bound.contains(s) {
                out.push((s.clone(), Span::new(0, 0)));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> Expr {
        let (e, d) = parse_predicate(text);
        let errs: Vec<_> = d.iter().filter(|x| x.is_error()).collect();
        assert!(errs.is_empty(), "diagnostics for {text:?}: {d:?}");
        e
    }

    #[test]
    fn simple_and_comparison() {
        ok("holder = none or exists one p : holder = some(p)");
    }

    #[test]
    fn quantifier_with_type_and_implies() {
        ok("every rm :: ac.rm_state(rm) = Committed implies tm_state = Committed");
    }

    #[test]
    fn multi_binder_and_dotted() {
        ok("every i, j : Proc :: i.id = j.id implies i = j");
    }

    #[test]
    fn nested_membership() {
        ok("no ea : InputEvent :: ea in a.events and some k : EntityKey :: k in ea.entity_keys");
    }

    #[test]
    fn no_type_binder() {
        ok("every x, y :: le(x, y) or le(y, x)");
    }

    #[test]
    fn old_and_call() {
        ok("balance(to) = old(balance(to)) + amt");
    }

    #[test]
    fn sum_aggregate_binds_below_comparison() {
        // `sum p :: f(p) = k` must read as `(sum p :: f(p)) = k`, i.e. Eq at the top
        // with a Sum on the left, not a sum of a boolean.
        let e = ok("sum p :: principal(p) = disbursed");
        match e {
            Expr::Binary { op: BinOp::Eq, lhs, .. } => {
                assert!(matches!(*lhs, Expr::Sum { .. }), "lhs should be a Sum, got {lhs:?}");
            }
            other => panic!("expected Eq at top, got {other:?}"),
        }
    }

    #[test]
    fn sum_as_bare_name_is_still_a_name() {
        // `sum` without a binder is an ordinary identifier, not an aggregate.
        let e = ok("sum = 0");
        assert!(matches!(e, Expr::Binary { op: BinOp::Eq, .. }));
    }

    #[test]
    fn not_binds_tighter_than_and() {
        let e = ok("not cleared(t) and platform_confirmed(t)");
        match e {
            Expr::Binary { op: BinOp::And, lhs, .. } => {
                assert!(matches!(*lhs, Expr::Unary { op: UnOp::Not, .. }), "lhs should be `not cleared(t)`");
            }
            other => panic!("expected And at top level, got {other:?}"),
        }
    }
}
