//! v4 lexer. Hand-written, own token set. Keywords are NOT distinguished here:
//! they lex as `Ident` and the parser matches on text. That keeps keyword choices
//! (still provisional, human-owned) trivial to change without touching the lexer.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Str(String),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,      // :
    ColonColon, // ::
    Semi,       // ;
    Comma,      // ,
    Dot,        // .
    DotDot,     // ..
    Pipe,       // |
    Question,   // ?
    Eq,         // =
    NotEq,      // <>
    Lt,         // <
    Gt,         // >
    Le,         // <=
    Ge,         // >=
    Arrow,      // ->
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

/// Is this char a legal identifier continuation? Identifiers allow `_` and `/`
/// (the qualified-name separator seen in v3 carries over as a lexeme; the parser
/// decides its meaning).
fn is_ident_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

pub fn lex(src: &str) -> Vec<Token> {
    let bytes = src.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    let mut out = Vec::new();
    let push = |out: &mut Vec<Token>, tok: Tok, s: usize, e: usize| {
        out.push(Token { tok, span: Span::new(s, e) });
    };

    while i < n {
        let c = bytes[i] as char;

        // Whitespace.
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Line comment `-- ...` (also the version marker). Consume to newline.
        if c == '-' && i + 1 < n && bytes[i + 1] == b'-' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Identifier / keyword.
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < n && is_ident_part(bytes[i] as char) {
                i += 1;
            }
            let text = src[start..i].to_string();
            push(&mut out, Tok::Ident(text), start, i);
            continue;
        }

        // Integer.
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (bytes[i] as char).is_ascii_digit() {
                i += 1;
            }
            let val: i64 = src[start..i].parse().unwrap_or(0);
            push(&mut out, Tok::Int(val), start, i);
            continue;
        }

        // String literal.
        if c == '"' {
            let start = i;
            i += 1;
            while i < n && bytes[i] != b'"' {
                i += 1;
            }
            let text = src.get(start + 1..i).unwrap_or("").to_string();
            if i < n {
                i += 1; // closing quote
            }
            push(&mut out, Tok::Str(text), start, i);
            continue;
        }

        // Multi-char then single-char punctuation.
        let two = if i + 1 < n { &src[i..i + 2] } else { "" };
        let (tok, len) = match two {
            "::" => (Tok::ColonColon, 2),
            "<>" => (Tok::NotEq, 2),
            "<=" => (Tok::Le, 2),
            ">=" => (Tok::Ge, 2),
            ".." => (Tok::DotDot, 2),
            "->" => (Tok::Arrow, 2),
            _ => match c {
                '(' => (Tok::LParen, 1),
                ')' => (Tok::RParen, 1),
                '{' => (Tok::LBrace, 1),
                '}' => (Tok::RBrace, 1),
                '[' => (Tok::LBracket, 1),
                ']' => (Tok::RBracket, 1),
                ':' => (Tok::Colon, 1),
                ';' => (Tok::Semi, 1),
                ',' => (Tok::Comma, 1),
                '.' => (Tok::Dot, 1),
                '|' => (Tok::Pipe, 1),
                '?' => (Tok::Question, 1),
                '=' => (Tok::Eq, 1),
                '<' => (Tok::Lt, 1),
                '>' => (Tok::Gt, 1),
                // Unknown byte: skip it so lexing never wedges. The parser reports
                // structural problems; a stray glyph is not worth a lexer error.
                _ => {
                    i += 1;
                    continue;
                }
            },
        };
        push(&mut out, tok, i, i + len);
        i += len;
    }

    push(&mut out, Tok::Eof, n, n);
    out
}
