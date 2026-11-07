//! FTS5 MATCH expressions for expanded queries.
//!
//! FTS5's `NOT` is binary (`a NOT b`), so a negated word can only be expressed next to something positive,
//! and filters can't be expressed at all. `to_fts5_match` folds what it can into one expression and hands the
//! rest back for the store to apply in SQL.

use crate::expand::{ExpandedClause, ExpandedQuery};
use crate::query::Field;

/// How top-level positive text clauses combine in the MATCH expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join {
    /// Every word must match (`a AND b`): the query language's semantics.
    All,
    /// Any word may match (`a OR b`): for collecting candidates to rank by coverage (DigUp style), the store
    /// then keeps documents whose coverage passes `fuse`'s gate.
    Any,
}

/// What the store runs for an expanded query.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Fts5Match {
    /// The `fts MATCH ?` expression: positive text clauses, with negated ones folded in by `NOT`.
    /// `None` when the query has no positive text clause.
    pub expression: Option<String>,
    /// Expressions a document must NOT match, for when there is no positive expression to hang them on:
    /// apply each as `rowid NOT IN (SELECT rowid FROM fts WHERE fts MATCH ?)`.
    pub excluded: Vec<String>,
    /// Clauses FTS5 can't express, to apply in SQL: filters, and groups that mix filters with text or put
    /// a negation inside an OR. Text parts of these can still use `clause_to_fts5` per leaf.
    pub residual: Vec<ExpandedClause>,
}

/// Builds the MATCH expression for `query`. `columns` maps a field to the FTS5 columns it searches; an empty
/// slice means all columns (no column filter).
pub fn to_fts5_match<'c>(
    query: &ExpandedQuery,
    columns: impl Fn(Field) -> &'c [&'c str],
    join: Join,
) -> Fts5Match {
    let mut positive = Vec::new();
    let mut negative = Vec::new();
    let mut residual = Vec::new();
    query.clauses.iter().for_each(|clause| match clause {
        ExpandedClause::Terms { negated: true, .. }
        | ExpandedClause::Phrase { negated: true, .. } => {
            negative.push(leaf(clause, &columns));
        }
        ExpandedClause::Filter { .. } => residual.push(clause.clone()),
        _ => match clause_to_fts5(clause, &columns) {
            Some(e) => positive.push(e),
            None => residual.push(clause.clone()),
        },
    });
    let separator = match join {
        Join::All => " AND ",
        Join::Any => " OR ",
    };
    if positive.is_empty() {
        return Fts5Match {
            expression: None,
            excluded: negative,
            residual,
        };
    }
    let base = paren_join(&positive, separator);
    let expression = negative
        .iter()
        .fold(base, |acc, n| format!("{acc} NOT {n}"));
    Fts5Match {
        expression: Some(expression),
        excluded: Vec::new(),
        residual,
    }
}

/// The MATCH expression of one clause, or `None` if FTS5 can't express it on its own: a filter, a negated
/// clause, an OR with a negated or filter member, or an AND group with no positive text member. An AND group's
/// negated members are folded in with `NOT`.
pub fn clause_to_fts5<'c>(
    clause: &ExpandedClause,
    columns: impl Fn(Field) -> &'c [&'c str],
) -> Option<String> {
    expr(clause, &columns)
}

fn expr<'c>(clause: &ExpandedClause, columns: &impl Fn(Field) -> &'c [&'c str]) -> Option<String> {
    match clause {
        ExpandedClause::Terms { negated: false, .. }
        | ExpandedClause::Phrase { negated: false, .. } => Some(leaf(clause, columns)),
        ExpandedClause::Terms { .. }
        | ExpandedClause::Phrase { .. }
        | ExpandedClause::Filter { .. } => None,
        ExpandedClause::Any(cs) => {
            let parts: Option<Vec<String>> = cs.iter().map(|c| expr(c, columns)).collect();
            parts.map(|ps| paren_join(&ps, " OR "))
        }
        ExpandedClause::All(cs) => {
            let (neg, pos): (Vec<&ExpandedClause>, Vec<&ExpandedClause>) =
                cs.iter().partition(|c| is_negated_text(c));
            let pos: Option<Vec<String>> = pos.iter().map(|c| expr(c, columns)).collect();
            let pos = pos.filter(|p| !p.is_empty())?;
            let base = paren_join(&pos, " AND ");
            Some(
                neg.iter()
                    .fold(base, |acc, n| format!("{acc} NOT {}", leaf(n, columns))),
            )
        }
    }
}

fn is_negated_text(clause: &ExpandedClause) -> bool {
    matches!(
        clause,
        ExpandedClause::Terms { negated: true, .. } | ExpandedClause::Phrase { negated: true, .. }
    )
}

/// A word or phrase, ignoring its negation, with its column filter.
fn leaf<'c>(clause: &ExpandedClause, columns: &impl Fn(Field) -> &'c [&'c str]) -> String {
    let (field, body) = match clause {
        ExpandedClause::Terms {
            field,
            alternatives,
            ..
        } => {
            let alts: Vec<String> = alternatives
                .iter()
                .map(|a| term_pattern(&a.term, a.prefix))
                .collect();
            let body = if alts.len() == 1 {
                alts[0].clone()
            } else {
                format!("({})", alts.join(" OR "))
            };
            (*field, body)
        }
        ExpandedClause::Phrase {
            field,
            terms,
            prefix,
            ..
        } => (*field, term_pattern(&terms.join(" "), *prefix)),
        _ => unreachable!("leaf of a non-text clause"),
    };
    with_columns(columns(field), body)
}

fn with_columns(cols: &[&str], body: String) -> String {
    match cols {
        [] => body,
        cols => {
            let names: Vec<String> = cols.iter().map(|c| column_name(c)).collect();
            format!("{{{}}} : {body}", names.join(" "))
        }
    }
}

fn paren_join(parts: &[String], separator: &str) -> String {
    match parts {
        [one] => one.clone(),
        many => many
            .iter()
            .map(|p| format!("({p})"))
            .collect::<Vec<_>>()
            .join(separator),
    }
}

/// A term (or several space-separated terms, as a phrase) as an FTS5 string: double-quoted, inner quotes
/// doubled, and `*` after the closing quote for a prefix query. Quoting makes keywords (`AND`, `OR`, `NOT`,
/// `NEAR`) and punctuation literal.
pub fn quote_term(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

fn term_pattern(term: &str, prefix: bool) -> String {
    let quoted = quote_term(term);
    if prefix { quoted + "*" } else { quoted }
}

/// A column name for a column filter: bare if FTS5 accepts it as a bareword, quoted otherwise.
fn column_name(name: &str) -> String {
    let bare = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii())
        && !matches!(name, "AND" | "OR" | "NOT" | "NEAR");
    if bare {
        name.to_owned()
    } else {
        quote_term(name)
    }
}

#[cfg(test)]
mod tests;
