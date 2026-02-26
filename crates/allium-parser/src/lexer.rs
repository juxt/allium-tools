use crate::Span;

// ---------------------------------------------------------------------------
// Token types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    // Identifiers and literals
    Ident,
    Number,
    Duration,
    String,
    True,
    False,
    Null,

    // Block-level keywords
    Rule,
    Entity,
    External,
    Value,
    Enum,
    Given,
    Config,
    Surface,
    Actor,
    Default,
    Variant,
    Deferred,
    Open,
    Question,
    Use,
    As,
    Module,

    // Clause / expression keywords
    When,
    Requires,
    Ensures,
    Let,
    For,
    In,
    If,
    Else,
    Where,
    With,
    Not,
    And,
    Or,
    Exists,

    // Trigger / predicate keywords
    TransitionsTo,
    Becomes,
    Includes,
    Excludes,

    // Context-sensitive identifiers treated as keywords
    Now,
    This,
    Within,

    // Operators
    Eq,              // =
    EqEq,            // ==
    BangEq,          // !=
    Lt,              // <
    LtEq,            // <=
    Gt,              // >
    GtEq,            // >=
    Plus,            // +
    Minus,           // -
    Star,            // *
    Slash,           // /
    Pipe,            // |
    FatArrow,        // =>
    ThinArrow,       // ->
    QuestionQuestion, // ??
    QuestionDot,     // ?.
    Dot,             // .
    DotDot,          // ..

    // Delimiters
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,        // [
    RBracket,        // ]
    Colon,
    Comma,
    QuestionMark,    // standalone ?

    // End of file
    Eof,

    // Lexer error (unrecognised character or unterminated string)
    Error,
}

impl TokenKind {
    /// True for any keyword or identifier — tokens that look like words.
    pub fn is_word(self) -> bool {
        matches!(
            self,
            TokenKind::Ident
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Null
                | TokenKind::Rule
                | TokenKind::Entity
                | TokenKind::External
                | TokenKind::Value
                | TokenKind::Enum
                | TokenKind::Given
                | TokenKind::Config
                | TokenKind::Surface
                | TokenKind::Actor
                | TokenKind::Default
                | TokenKind::Variant
                | TokenKind::Deferred
                | TokenKind::Open
                | TokenKind::Question
                | TokenKind::Use
                | TokenKind::As
                | TokenKind::Module
                | TokenKind::When
                | TokenKind::Requires
                | TokenKind::Ensures
                | TokenKind::Let
                | TokenKind::For
                | TokenKind::In
                | TokenKind::If
                | TokenKind::Else
                | TokenKind::Where
                | TokenKind::With
                | TokenKind::Not
                | TokenKind::And
                | TokenKind::Or
                | TokenKind::Exists
                | TokenKind::TransitionsTo
                | TokenKind::Becomes
                | TokenKind::Includes
                | TokenKind::Excludes
                | TokenKind::Now
                | TokenKind::This
                | TokenKind::Within
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Source map — precomputed line start offsets for O(1) line/col lookup
// ---------------------------------------------------------------------------

pub struct SourceMap {
    line_starts: Vec<usize>,
}

impl SourceMap {
    pub fn new(source: &str) -> Self {
        let mut starts = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self { line_starts: starts }
    }

    pub fn line_col(&self, offset: usize) -> (u32, u32) {
        let line = self
            .line_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let col = offset - self.line_starts[line];
        (line as u32, col as u32)
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

/// Tokenise `source` into a flat list of tokens, ending with `Eof`.
/// Whitespace and comments are skipped. The special comment `-- allium: N`
/// at the very start of the file is preserved as an `Ident` token followed
/// by normal tokens (version detection happens in the parser by inspecting
/// the raw source before tokenising).
pub fn lex(source: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();
    loop {
        let tok = lexer.next_token();
        let done = tok.kind == TokenKind::Eof;
        tokens.push(tok);
        if done {
            break;
        }
    }
    tokens
}

struct Lexer<'s> {
    src: &'s [u8],
    pos: usize,
}

impl<'s> Lexer<'s> {
    fn new(source: &'s str) -> Self {
        Self {
            src: source.as_bytes(),
            pos: 0,
        }
    }

    fn next_token(&mut self) -> Token {
        self.skip_whitespace_and_comments();

        if self.pos >= self.src.len() {
            return Token {
                kind: TokenKind::Eof,
                span: Span::new(self.pos, self.pos),
            };
        }

        let start = self.pos;
        let b = self.src[self.pos];

        if b == b'"' {
            return self.lex_string(start);
        }
        if b.is_ascii_digit() {
            return self.lex_number(start);
        }
        if is_ident_start(b) {
            return self.lex_ident(start);
        }

        self.lex_operator(start)
    }

    // -- whitespace / comments ------------------------------------------

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            while self.pos < self.src.len()
                && matches!(self.src[self.pos], b' ' | b'\t' | b'\n' | b'\r')
            {
                self.pos += 1;
            }
            if self.pos + 1 < self.src.len()
                && self.src[self.pos] == b'-'
                && self.src[self.pos + 1] == b'-'
            {
                while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
    }

    // -- string literals ------------------------------------------------

    fn lex_string(&mut self, start: usize) -> Token {
        self.pos += 1; // opening "
        while self.pos < self.src.len() {
            match self.src[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return Token {
                        kind: TokenKind::String,
                        span: Span::new(start, self.pos),
                    };
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos < self.src.len() {
                        self.pos += 1;
                    }
                }
                b'\n' => {
                    return Token {
                        kind: TokenKind::Error,
                        span: Span::new(start, self.pos),
                    };
                }
                _ => self.pos += 1,
            }
        }
        Token {
            kind: TokenKind::Error,
            span: Span::new(start, self.pos),
        }
    }

    // -- numbers and durations ------------------------------------------

    fn lex_number(&mut self, start: usize) -> Token {
        self.consume_digits();

        if self.pos < self.src.len() && self.src[self.pos] == b'.' {
            let after_dot = self.pos + 1;
            if after_dot < self.src.len() && self.src[after_dot].is_ascii_digit() {
                // Decimal part
                self.pos += 1;
                self.consume_digits();
                // Check for .unit (e.g. 3.14.hours — unusual but valid)
                if self.check_duration_suffix() {
                    return Token {
                        kind: TokenKind::Duration,
                        span: Span::new(start, self.pos),
                    };
                }
                return Token {
                    kind: TokenKind::Number,
                    span: Span::new(start, self.pos),
                };
            }
            if self.peek_duration_unit(after_dot).is_some() {
                let unit_len = self.peek_duration_unit(after_dot).unwrap();
                self.pos = after_dot + unit_len;
                return Token {
                    kind: TokenKind::Duration,
                    span: Span::new(start, self.pos),
                };
            }
        }

        Token {
            kind: TokenKind::Number,
            span: Span::new(start, self.pos),
        }
    }

    fn consume_digits(&mut self) {
        while self.pos < self.src.len()
            && (self.src[self.pos].is_ascii_digit() || self.src[self.pos] == b'_')
        {
            self.pos += 1;
        }
    }

    /// After consuming a decimal number, check for `.unit` suffix.
    fn check_duration_suffix(&mut self) -> bool {
        if self.pos < self.src.len() && self.src[self.pos] == b'.' {
            if let Some(unit_len) = self.peek_duration_unit(self.pos + 1) {
                self.pos += 1 + unit_len;
                return true;
            }
        }
        false
    }

    fn peek_duration_unit(&self, from: usize) -> Option<usize> {
        const UNITS: &[&str] = &[
            "seconds", "second", "minutes", "minute", "hours", "hour", "days", "day", "weeks",
            "week", "months", "month", "years", "year",
        ];
        for unit in UNITS {
            let end = from + unit.len();
            if end <= self.src.len()
                && &self.src[from..end] == unit.as_bytes()
                && (end >= self.src.len() || !is_ident_continue(self.src[end]))
            {
                return Some(unit.len());
            }
        }
        None
    }

    // -- identifiers and keywords ---------------------------------------

    fn lex_ident(&mut self, start: usize) -> Token {
        while self.pos < self.src.len() && is_ident_continue(self.src[self.pos]) {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        Token {
            kind: classify_keyword(text),
            span: Span::new(start, self.pos),
        }
    }

    // -- operators and punctuation --------------------------------------

    fn lex_operator(&mut self, start: usize) -> Token {
        let b = self.src[self.pos];
        let next = if self.pos + 1 < self.src.len() {
            self.src[self.pos + 1]
        } else {
            0
        };

        let (kind, len) = match (b, next) {
            (b'=', b'>') => (TokenKind::FatArrow, 2),
            (b'=', b'=') => (TokenKind::EqEq, 2),
            (b'=', _) => (TokenKind::Eq, 1),
            (b'!', b'=') => (TokenKind::BangEq, 2),
            (b'<', b'=') => (TokenKind::LtEq, 2),
            (b'<', _) => (TokenKind::Lt, 1),
            (b'>', b'=') => (TokenKind::GtEq, 2),
            (b'>', _) => (TokenKind::Gt, 1),
            (b'+', _) => (TokenKind::Plus, 1),
            (b'-', b'>') => (TokenKind::ThinArrow, 2),
            (b'-', _) => (TokenKind::Minus, 1),
            (b'*', _) => (TokenKind::Star, 1),
            (b'/', _) => (TokenKind::Slash, 1),
            (b'|', _) => (TokenKind::Pipe, 1),
            (b'?', b'?') => (TokenKind::QuestionQuestion, 2),
            (b'?', b'.') => (TokenKind::QuestionDot, 2),
            (b'?', _) => (TokenKind::QuestionMark, 1),
            (b'.', b'.') => (TokenKind::DotDot, 2),
            (b'.', _) => (TokenKind::Dot, 1),
            (b'{', _) => (TokenKind::LBrace, 1),
            (b'}', _) => (TokenKind::RBrace, 1),
            (b'(', _) => (TokenKind::LParen, 1),
            (b')', _) => (TokenKind::RParen, 1),
            (b'[', _) => (TokenKind::LBracket, 1),
            (b']', _) => (TokenKind::RBracket, 1),
            (b':', _) => (TokenKind::Colon, 1),
            (b',', _) => (TokenKind::Comma, 1),
            _ => (TokenKind::Error, 1),
        };

        self.pos += len;
        Token {
            kind,
            span: Span::new(start, self.pos),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn classify_keyword(text: &str) -> TokenKind {
    match text {
        "rule" => TokenKind::Rule,
        "entity" => TokenKind::Entity,
        "external" => TokenKind::External,
        "value" => TokenKind::Value,
        "enum" => TokenKind::Enum,
        "given" => TokenKind::Given,
        "config" => TokenKind::Config,
        "surface" => TokenKind::Surface,
        "actor" => TokenKind::Actor,
        "default" => TokenKind::Default,
        "variant" => TokenKind::Variant,
        "deferred" => TokenKind::Deferred,
        "open" => TokenKind::Open,
        "question" => TokenKind::Question,
        "use" => TokenKind::Use,
        "as" => TokenKind::As,
        "module" => TokenKind::Module,
        "when" => TokenKind::When,
        "requires" => TokenKind::Requires,
        "ensures" => TokenKind::Ensures,
        "let" => TokenKind::Let,
        "for" => TokenKind::For,
        "in" => TokenKind::In,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "where" => TokenKind::Where,
        "with" => TokenKind::With,
        "not" => TokenKind::Not,
        "and" => TokenKind::And,
        "or" => TokenKind::Or,
        "exists" => TokenKind::Exists,
        "transitions_to" => TokenKind::TransitionsTo,
        "becomes" => TokenKind::Becomes,
        "includes" => TokenKind::Includes,
        "excludes" => TokenKind::Excludes,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "null" => TokenKind::Null,
        "now" => TokenKind::Now,
        "this" => TokenKind::This,
        "within" => TokenKind::Within,
        _ => TokenKind::Ident,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).into_iter().map(|t| t.kind).collect()
    }

    fn text_of(src: &str) -> Vec<&str> {
        lex(src)
            .into_iter()
            .map(|t| &src[t.span.start..t.span.end])
            .collect()
    }

    #[test]
    fn keywords() {
        assert_eq!(
            kinds("rule entity enum"),
            vec![TokenKind::Rule, TokenKind::Entity, TokenKind::Enum, TokenKind::Eof]
        );
    }

    #[test]
    fn identifiers() {
        assert_eq!(
            kinds("my_var User"),
            vec![TokenKind::Ident, TokenKind::Ident, TokenKind::Eof]
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(kinds("42"), vec![TokenKind::Number, TokenKind::Eof]);
        assert_eq!(kinds("3.14"), vec![TokenKind::Number, TokenKind::Eof]);
        assert_eq!(kinds("100_000"), vec![TokenKind::Number, TokenKind::Eof]);
    }

    #[test]
    fn durations() {
        assert_eq!(kinds("24.hours"), vec![TokenKind::Duration, TokenKind::Eof]);
        assert_eq!(kinds("7.days"), vec![TokenKind::Duration, TokenKind::Eof]);
        assert_eq!(kinds("1.second"), vec![TokenKind::Duration, TokenKind::Eof]);
        assert_eq!(kinds("3.5.minutes"), vec![TokenKind::Duration, TokenKind::Eof]);
    }

    #[test]
    fn duration_vs_member_access() {
        // 42.count is number + dot + ident, not a duration
        assert_eq!(
            kinds("42.count"),
            vec![TokenKind::Number, TokenKind::Dot, TokenKind::Ident, TokenKind::Eof]
        );
    }

    #[test]
    fn strings() {
        assert_eq!(kinds(r#""hello""#), vec![TokenKind::String, TokenKind::Eof]);
        assert_eq!(
            kinds(r#""hello {name}""#),
            vec![TokenKind::String, TokenKind::Eof]
        );
    }

    #[test]
    fn operators() {
        assert_eq!(
            kinds("=> -> ?? ?. != <= >="),
            vec![
                TokenKind::FatArrow,
                TokenKind::ThinArrow,
                TokenKind::QuestionQuestion,
                TokenKind::QuestionDot,
                TokenKind::BangEq,
                TokenKind::LtEq,
                TokenKind::GtEq,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn comments_skipped() {
        assert_eq!(
            kinds("rule -- this is a comment\nentity"),
            vec![TokenKind::Rule, TokenKind::Entity, TokenKind::Eof]
        );
    }

    #[test]
    fn delimiters() {
        assert_eq!(
            kinds("{ } ( ) : ,"),
            vec![
                TokenKind::LBrace, TokenKind::RBrace,
                TokenKind::LParen, TokenKind::RParen,
                TokenKind::Colon, TokenKind::Comma,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn full_line() {
        let src = "status: pending | active | completed";
        assert_eq!(
            text_of(src),
            vec!["status", ":", "pending", "|", "active", "|", "completed", ""]
        );
    }

    #[test]
    fn source_map_line_col() {
        let src = "abc\ndef\nghi";
        let map = SourceMap::new(src);
        assert_eq!(map.line_col(0), (0, 0)); // 'a'
        assert_eq!(map.line_col(3), (0, 3)); // '\n'
        assert_eq!(map.line_col(4), (1, 0)); // 'd'
        assert_eq!(map.line_col(8), (2, 0)); // 'g'
    }
}
