//! The query language: Gmail/Fastmail-like operators, phrases, negation, `OR` and groups.
//!
//! Parsing never fails. Input that doesn't parse as an operator degrades to words, and every byte of the
//! input is covered by at most one `Token` for syntax highlighting.

mod value;

use std::ops::Range;

use crate::normalize::{normalize, tokenize};
use crate::query::{Clause, Field, Query};
use value::{Clock, Op};

/// What a span of the query input is, for syntax highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// An operator name with its colon: `from:`, `is:`.
    Operator,
    /// An operator's value: `jane` in `from:jane`, `unread` in `is:unread`, including quotes.
    Value,
    /// A free word.
    Word,
    /// A quoted phrase, including its quotes.
    Phrase,
    /// The `-` that negates what follows.
    Negation,
    /// `OR` (and `AND`, which is implied and ignored).
    Connective,
    /// A grouping bracket: `(`, `)`, `{`, `}`.
    Group,
    /// Something that looked like syntax but isn't valid: `is:banana`, `after:2026-13-45`, a stray `)`.
    /// Invalid operators still search for their text as words.
    Invalid,
}

/// A highlighted span of the query input. `range` is in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Byte range in the input.
    pub range: Range<usize>,
    /// What the span is.
    pub kind: TokenKind,
}

/// Parses a query, with dates (`today`, `2026-01-31`) taken in UTC. `now_unix` is the current time in seconds
/// since the Unix epoch, for relative dates.
pub fn parse(input: &str, now_unix: i64) -> Query {
    parse_in_zone(input, now_unix, 0)
}

/// Parses a query, with calendar dates taken in a time zone `utc_offset_seconds` east of UTC.
pub fn parse_in_zone(input: &str, now_unix: i64, utc_offset_seconds: i64) -> Query {
    parse_with_tokens(input, now_unix, utc_offset_seconds).0
}

/// The highlighting tokens of a query, in input order. Whitespace is not covered.
pub fn tokens(input: &str) -> Vec<Token> {
    parse_with_tokens(input, 0, 0).1
}

/// Parses a query and returns its highlighting tokens too, in one pass.
pub fn parse_with_tokens(
    input: &str,
    now_unix: i64,
    utc_offset_seconds: i64,
) -> (Query, Vec<Token>) {
    let mut p = Parser {
        src: input,
        pos: 0,
        clock: Clock {
            now: now_unix,
            utc_offset: utc_offset_seconds,
        },
        tokens: Vec::new(),
        last_text: None,
        text_count: 0,
        depth: 0,
    };
    let clauses = flatten_all(p.sequence(None, None));
    let ends_open = input.chars().last().is_some_and(|c| !c.is_whitespace());
    let last_is_prefix = ends_open
        && p.last_text
            .is_some_and(|(end, ok)| ok && end == input.len());
    let semantic_text = semantic_text(&clauses);
    let mut tokens = p.tokens;
    tokens.sort_by_key(|t| t.range.start);
    (
        Query {
            clauses,
            semantic_text,
            last_is_prefix,
        },
        tokens,
    )
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    clock: Clock,
    tokens: Vec<Token>,
    last_text: Option<(usize, bool)>,
    text_count: usize,
    depth: usize,
}

const MAX_DEPTH: usize = 32;

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek2(&self) -> Option<char> {
        self.src[self.pos..].chars().nth(1)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn token(&mut self, range: Range<usize>, kind: TokenKind) {
        if !range.is_empty() {
            self.tokens.push(Token { range, kind });
        }
    }

    fn bareword(&mut self) -> Range<usize> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| !c.is_whitespace() && !matches!(c, '(' | ')' | '{' | '}' | '"'))
        {
            self.bump();
        }
        start..self.pos
    }

    fn quoted(&mut self) -> (Range<usize>, Range<usize>, bool) {
        let start = self.pos;
        self.bump();
        let inner_start = self.pos;
        while self.peek().is_some_and(|c| c != '"') {
            self.bump();
        }
        let inner = inner_start..self.pos;
        let closed = self.bump().is_some();
        (start..self.pos, inner, closed)
    }

    /// A conjunction up to `close` (consumed) or the end of input.
    fn sequence(&mut self, close: Option<char>, ctx: Option<Op>) -> Vec<Clause> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(c) if Some(c) == close => {
                    let at = self.pos;
                    self.bump();
                    self.token(at..self.pos, TokenKind::Group);
                    break;
                }
                Some(')' | '}') => {
                    let at = self.pos;
                    self.bump();
                    self.token(at..self.pos, TokenKind::Invalid);
                }
                Some(_) => out.extend(self.disjunction(close, ctx)),
            }
        }
        out
    }

    fn at_connective(&self, word: &str) -> bool {
        let rest = &self.src[self.pos..];
        rest.starts_with(word)
            && rest[word.len()..]
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '{' | '"' | '-'))
    }

    fn connective(&mut self, word: &str) {
        let at = self.pos;
        self.pos += word.len();
        self.token(at..self.pos, TokenKind::Connective);
    }

    fn disjunction(&mut self, close: Option<char>, ctx: Option<Op>) -> Option<Clause> {
        if self.at_connective("OR") || self.at_connective("AND") {
            let word = if self.at_connective("OR") {
                "OR"
            } else {
                "AND"
            };
            self.connective(word);
            return None;
        }
        let first = self.unary(ctx);
        let mut alternatives: Vec<Clause> = first.into_iter().collect();
        loop {
            let save = self.pos;
            self.skip_ws();
            if !self.at_connective("OR") {
                self.pos = save;
                break;
            }
            self.connective("OR");
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(c) if Some(c) == close => break,
                Some(_) if self.at_connective("OR") || self.at_connective("AND") => break,
                Some(_) => alternatives.extend(self.unary(ctx)),
            }
        }
        match alternatives.len() {
            0 => None,
            1 => alternatives.pop(),
            _ => Some(Clause::Any(alternatives)),
        }
    }

    fn unary(&mut self, ctx: Option<Op>) -> Option<Clause> {
        let negated = self.peek() == Some('-')
            && self.peek2().is_some_and(|c| !c.is_whitespace() && c != '-');
        if !negated {
            return self.primary(ctx);
        }
        let at = self.pos;
        self.bump();
        self.token(at..self.pos, TokenKind::Negation);
        let before = self.text_count;
        let clause = self.primary(ctx).map(negate);
        if self.text_count != before {
            self.last_text = self.last_text.map(|(end, _)| (end, false));
        }
        clause
    }

    fn primary(&mut self, ctx: Option<Op>) -> Option<Clause> {
        match self.peek()? {
            '(' => self.group(')', ctx, Clause::All),
            '{' => self.group('}', ctx, Clause::Any),
            '"' => self.phrase(ctx),
            _ => self.word_or_operator(ctx),
        }
    }

    fn group(
        &mut self,
        close: char,
        ctx: Option<Op>,
        wrap: fn(Vec<Clause>) -> Clause,
    ) -> Option<Clause> {
        let at = self.pos;
        self.bump();
        if self.depth >= MAX_DEPTH {
            self.token(at..self.pos, TokenKind::Invalid);
            return None;
        }
        self.token(at..self.pos, TokenKind::Group);
        self.depth += 1;
        let mut inner = self.sequence(Some(close), ctx);
        self.depth -= 1;
        match inner.len() {
            0 => None,
            1 => inner.pop(),
            _ => Some(wrap(inner)),
        }
    }

    fn phrase(&mut self, ctx: Option<Op>) -> Option<Clause> {
        let (outer, inner, closed) = self.quoted();
        let text = &self.src[inner.clone()];
        let kind = if ctx.is_some() {
            TokenKind::Value
        } else {
            TokenKind::Phrase
        };
        match ctx {
            None | Some(Op::Text(_)) => {
                self.token(outer, kind);
                let field = match ctx {
                    Some(Op::Text(f)) => f,
                    _ => Field::Any,
                };
                has_terms(text).then(|| {
                    self.note_text(inner.end, !closed && !text.ends_with(char::is_whitespace));
                    Clause::Phrase {
                        field,
                        text: text.to_owned(),
                        negated: false,
                    }
                })
            }
            Some(op) => self.apply_op(op, text, outer.clone(), outer),
        }
    }

    fn word_or_operator(&mut self, ctx: Option<Op>) -> Option<Clause> {
        let range = self.bareword();
        if range.is_empty() {
            let at = self.pos;
            self.bump();
            self.token(at..self.pos, TokenKind::Invalid);
            return None;
        }
        let word = &self.src[range.clone()];
        let op = word
            .find(':')
            .filter(|&i| i > 0)
            .and_then(|i| Op::from_name(&word[..i]).map(|op| (op, i)));
        match (op, ctx) {
            (Some((op, colon)), _) => self.operator(op, range.start, range.start + colon + 1),
            (None, Some(op)) => self.apply_op(op, word, range.clone(), range),
            (None, None) => {
                self.token(range.clone(), TokenKind::Word);
                self.text_word(Field::Any, range)
            }
        }
    }

    fn text_word(&mut self, field: Field, range: Range<usize>) -> Option<Clause> {
        let text = &self.src[range.clone()];
        has_terms(text).then(|| {
            self.note_text(range.end, true);
            Clause::Word {
                field,
                text: text.to_owned(),
                negated: false,
            }
        })
    }

    fn note_text(&mut self, end: usize, prefix_ok: bool) {
        self.text_count += 1;
        self.last_text = Some((end, prefix_ok));
    }

    /// `name:` has been read up to `value_start`; parse the value.
    fn operator(&mut self, op: Op, start: usize, value_start: usize) -> Option<Clause> {
        self.pos = value_start;
        self.token(start..value_start, TokenKind::Operator);
        match self.peek() {
            None => None,
            Some(c) if c.is_whitespace() || c == ')' || c == '}' => None,
            Some('"') => {
                let (outer, inner, _) = self.quoted();
                let text = &self.src[inner.clone()];
                match op {
                    Op::Text(field) => {
                        self.token(outer, TokenKind::Value);
                        has_terms(text).then(|| {
                            self.note_text(inner.end, false);
                            Clause::Phrase {
                                field,
                                text: text.to_owned(),
                                negated: false,
                            }
                        })
                    }
                    _ => self.apply_op(op, text, start..outer.end, outer),
                }
            }
            Some('(') => self.group(')', Some(op), Clause::All),
            Some('{') => self.group('}', Some(op), Clause::Any),
            Some(_) => {
                let value = self.bareword();
                self.apply_op(op, &self.src[value.clone()], start..value.end, value)
            }
        }
    }

    /// Applies `op` to `value` (unquoted). `whole` is the source span to search as words if the value is
    /// invalid; `value_range` is what gets highlighted as the value.
    fn apply_op(
        &mut self,
        op: Op,
        value: &str,
        whole: Range<usize>,
        value_range: Range<usize>,
    ) -> Option<Clause> {
        if let Op::Text(field) = op {
            self.token(value_range.clone(), TokenKind::Value);
            return self.text_word(field, value_range);
        }
        match op.filter(value, self.clock) {
            Some(filter) => {
                self.token(value_range, TokenKind::Value);
                Some(Clause::Filter {
                    filter,
                    negated: false,
                })
            }
            None => {
                self.tokens
                    .retain(|t| !(t.range.start >= whole.start && t.range.end <= whole.end));
                self.token(
                    whole.start.min(value_range.start)..whole.end.max(value_range.end),
                    TokenKind::Invalid,
                );
                let text = &self.src[whole.clone()];
                has_terms(text).then(|| {
                    self.note_text(whole.end, false);
                    Clause::Word {
                        field: Field::Any,
                        text: text.to_owned(),
                        negated: false,
                    }
                })
            }
        }
    }
}

fn has_terms(text: &str) -> bool {
    tokenize(&normalize(text)).next().is_some()
}

/// Negates a clause, pushing negation down to the leaves (De Morgan).
fn negate(clause: Clause) -> Clause {
    match clause {
        Clause::Word {
            field,
            text,
            negated,
        } => Clause::Word {
            field,
            text,
            negated: !negated,
        },
        Clause::Phrase {
            field,
            text,
            negated,
        } => Clause::Phrase {
            field,
            text,
            negated: !negated,
        },
        Clause::Filter { filter, negated } => Clause::Filter {
            filter,
            negated: !negated,
        },
        Clause::Any(cs) => Clause::All(cs.into_iter().map(negate).collect()),
        Clause::All(cs) => Clause::Any(cs.into_iter().map(negate).collect()),
    }
}

fn flatten_all(clauses: Vec<Clause>) -> Vec<Clause> {
    clauses
        .into_iter()
        .flat_map(|c| match c {
            Clause::All(cs) => flatten_all(cs),
            other => vec![other],
        })
        .collect()
}

fn semantic_text(clauses: &[Clause]) -> String {
    fn collect<'c>(c: &'c Clause, out: &mut Vec<&'c str>) {
        match c {
            Clause::Word {
                field,
                text,
                negated: false,
            }
            | Clause::Phrase {
                field,
                text,
                negated: false,
            } if matches!(field, Field::Any | Field::Subject | Field::Body) => {
                out.push(text.trim());
            }
            Clause::Any(cs) | Clause::All(cs) => cs.iter().for_each(|c| collect(c, out)),
            _ => {}
        }
    }
    let mut parts = Vec::new();
    clauses.iter().for_each(|c| collect(c, &mut parts));
    parts.join(" ")
}

#[cfg(test)]
mod tests;
