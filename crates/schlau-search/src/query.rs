//! The search query AST shared by the parser (`parse`) and the executor in `schlau-store`.
//!
//! A query is a conjunction of clauses. Free words and phrases are matched lexically (typo-tolerant)
//! and, when the query reads like a description, semantically too.

/// A parsed search query. All clauses must hold (AND); `Clause::Any` expresses OR.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub clauses: Vec<Clause>,
    /// The free text of the query with operators removed, as typed. Fed to the embedder.
    pub semantic_text: String,
    /// Whether the last word is still being typed (no trailing space): match it as a prefix.
    pub last_is_prefix: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Clause {
    /// A word to match in any text field (or one field), typo-tolerant.
    Word { field: Field, text: String, negated: bool },
    /// An exact phrase in quotes. Not typo-tolerant.
    Phrase { field: Field, text: String, negated: bool },
    /// A structured filter (`is:unread`, `has:attachment`, `in:inbox`, dates, size).
    Filter { filter: Filter, negated: bool },
    /// `a OR b`, `{a b}`: at least one of the alternatives holds.
    Any(Vec<Clause>),
}

/// Where a word or phrase must appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Any,
    From,
    To,
    Cc,
    Subject,
    Body,
    Attachment,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    Unread,
    Read,
    Flagged,
    Draft,
    Answered,
    HasAttachment,
    /// `in:inbox`, `in:archive`, `in:"Receipts/2026"`: a mailbox by role or by name.
    /// `label:x` is an alias of `in:x` (Fastmail labels are mailboxes).
    InMailbox(String),
    /// Seconds since the Unix epoch, UTC. `after:` is inclusive, `before:` exclusive.
    After(i64),
    Before(i64),
    /// Bytes. `larger:5M`, `smaller:100k`.
    Larger(u64),
    Smaller(u64),
}
