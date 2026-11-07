//! The search query AST shared by the parser (`parse`) and the executor in `schlau-store`.
//!
//! A query is a conjunction of clauses. Free words and phrases are matched lexically (typo-tolerant)
//! and, when the query reads like a description, semantically too.
//!
//! Clauses keep source order (left to right, depth first), which `Query::last_is_prefix` relies on.

/// A parsed search query. All clauses must hold (AND); `Clause::Any` expresses OR.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    /// The clauses, all of which must hold, in source order.
    pub clauses: Vec<Clause>,
    /// The free text of the query with operators removed, as typed. Fed to the embedder.
    ///
    /// Holds the non-negated words and phrases of any field, subject and body, in source order and joined by
    /// single spaces. Sender and recipient values are left out: they name people, not what a message is about.
    pub semantic_text: String,
    /// Whether the last word is still being typed (no trailing space): match it as a prefix.
    ///
    /// Applies to the last `Clause::Word` (or unterminated `Clause::Phrase`) in source order, and is only set
    /// when that clause ends the input and is not negated.
    pub last_is_prefix: bool,
}

/// One condition of a query.
#[derive(Debug, Clone, PartialEq)]
pub enum Clause {
    /// A word to match in any text field (or one field), typo-tolerant.
    ///
    /// `text` is the word as typed (not normalized). It can tokenize into several terms (`jane.doe`,
    /// `e-mail`), which then match as a phrase.
    Word {
        /// The field the word must appear in.
        field: Field,
        /// The word as typed.
        text: String,
        /// The word must not appear.
        negated: bool,
    },
    /// An exact phrase in quotes. Not typo-tolerant.
    Phrase {
        /// The field the phrase must appear in.
        field: Field,
        /// The phrase as typed, without the quotes.
        text: String,
        /// The phrase must not appear.
        negated: bool,
    },
    /// A structured filter (`is:unread`, `has:attachment`, `in:inbox`, dates, size).
    Filter {
        /// The condition.
        filter: Filter,
        /// The condition must not hold.
        negated: bool,
    },
    /// `a OR b`, `{a b}`: at least one of the alternatives holds.
    Any(Vec<Clause>),
    /// `(a b)` inside an OR: all of the clauses hold. At the top level groups are flattened into
    /// `Query::clauses` instead.
    All(Vec<Clause>),
}

/// Where a word or phrase must appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// Any text field.
    Any,
    /// Sender name and address (`from:`).
    From,
    /// Recipient names and addresses (`to:`).
    To,
    /// Carbon-copy names and addresses (`cc:`).
    Cc,
    /// The subject line (`subject:`).
    Subject,
    /// The message body (`body:`).
    Body,
    /// Attachment file names (`filename:`, `attachment:`).
    Attachment,
}

/// A structured condition the store evaluates in SQL.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    /// `is:unread`: the `$seen` keyword is not set.
    Unread,
    /// `is:read`: the `$seen` keyword is set.
    Read,
    /// `is:flagged`, `is:starred`: the `$flagged` keyword is set.
    Flagged,
    /// `is:draft`: the `$draft` keyword is set.
    Draft,
    /// `is:answered`, `is:replied`: the `$answered` keyword is set.
    Answered,
    /// `has:attachment`.
    HasAttachment,
    /// `in:inbox`, `in:archive`, `in:"Receipts/2026"`: a mailbox by role or by name.
    /// `label:x` is an alias of `in:x` (Fastmail labels are mailboxes).
    InMailbox(String),
    /// Seconds since the Unix epoch, UTC. `after:` is inclusive, `before:` exclusive.
    After(i64),
    /// Seconds since the Unix epoch, UTC; exclusive.
    Before(i64),
    /// Bytes. `larger:5M`, `smaller:100k`.
    Larger(u64),
    /// Bytes, exclusive.
    Smaller(u64),
}

impl Query {
    /// Whether the query has no clauses at all.
    pub fn is_empty(&self) -> bool {
        self.clauses.is_empty()
    }

    /// Whether any clause, at any depth, matches text (a word or a phrase, negated or not).
    pub fn has_text(&self) -> bool {
        fn any_text(c: &Clause) -> bool {
            match c {
                Clause::Word { .. } | Clause::Phrase { .. } => true,
                Clause::Filter { .. } => false,
                Clause::Any(cs) | Clause::All(cs) => cs.iter().any(any_text),
            }
        }
        self.clauses.iter().any(any_text)
    }
}
