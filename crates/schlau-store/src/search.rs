//! Runs a parsed query against the replica: typo-tolerant words through FTS5, structured filters
//! in SQL, meaning through the caller's vector scores, fused and collapsed to conversations.

use std::collections::{HashMap, HashSet};

use rusqlite::types::Value;
use schlau_search::{
    ExpandOptions, ExpandedClause, ExpandedQuery, Field, Filter, Join, LexicalHit, MatchReason, Meaning, MeaningScale,
    Query, SemanticHit, Vocabulary, clause_to_fts5, expand_with, fuse, to_fts5_match,
};

use crate::Result;
use crate::model::ThreadRow;
use crate::read::Reader;

const CANDIDATES: usize = 2_000;

/// A conversation found by a search, best first.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub row: ThreadRow,
    pub score: f64,
    pub why: MatchReason,
    /// The best matching message in the conversation.
    pub message: i64,
    /// The attachment whose meaning matched, when that is why the conversation is here.
    pub attachment: Option<String>,
}

/// Meaning scores for the query, computed by the caller over every stored vector of each index.
pub struct MeaningInput<'a> {
    pub channels: Vec<MeaningChannel<'a>>,
}

/// One vector index's view of the query: message text, or attachments (another model, another scale).
pub struct MeaningChannel<'a> {
    pub scale: MeaningScale,
    /// The best vectors (id, cosine), unfiltered. Ids are message rowids, or packed attachment ids
    /// ([`crate::media_vector_id`]) for an attachment channel.
    pub top: &'a [(i64, f32)],
    pub attachments: bool,
}

/// Every channel's z-scores on one scale: a message's meaning is its best z in any channel.
const UNIT: MeaningScale = MeaningScale { mean: 0.0, deviation: 1.0 };

/// An attachment channel's best few messages that stand out at all are lifted by this many deviations.
/// Text against images gives compressed cosines, and a mailbox has far fewer attachments than
/// messages, so the best visual match of "blueish document" may stand only 1.5 deviations out; lifted,
/// it clears the gate the way an image search always shows its best results. Order is kept.
const ATTACHMENT_LIFT: f32 = 1.0;
const ATTACHMENT_LIFTED: usize = 3;
const ATTACHMENT_MIN_Z: f32 = 1.0;

/// One channel's hits as (message, z, attachment part), best first.
fn channel_hits(c: &MeaningChannel<'_>) -> Vec<(i64, f32, Option<i64>)> {
    let mut hits: Vec<(i64, f32, Option<i64>)> = c
        .top
        .iter()
        .map(|&(id, cosine)| {
            let z = c.scale.z(cosine) as f32;
            if c.attachments {
                let (message, part) = crate::media_vector_location(id);
                (message, z, Some(part))
            } else {
                (id, z, None)
            }
        })
        .collect();
    hits.sort_by(|a, b| b.1.total_cmp(&a.1));
    if c.attachments {
        let mut lifted = HashSet::new();
        hits.iter_mut().for_each(|(message, z, _)| {
            if *z >= ATTACHMENT_MIN_Z && (lifted.contains(message) || lifted.len() < ATTACHMENT_LIFTED) {
                lifted.insert(*message);
                *z += ATTACHMENT_LIFT;
            }
        });
    }
    hits
}

/// Per message, the best z-score over the channels and the attachment part it came from.
fn merge_channels(channels: &[MeaningChannel<'_>]) -> HashMap<i64, (f32, Option<i64>)> {
    channels
        .iter()
        .flat_map(channel_hits)
        .map(|(message, z, part)| (message, (z, part)))
        .fold(HashMap::new(), |mut best, (message, hit)| {
            best.entry(message).and_modify(|b: &mut (f32, Option<i64>)| if hit.0 > b.0 { *b = hit }).or_insert(hit);
            best
        })
}

fn columns(field: Field) -> &'static [&'static str] {
    match field {
        Field::Any => &[],
        Field::From => &["sender"],
        Field::To | Field::Cc => &["recipients"],
        Field::Subject => &["subject"],
        Field::Body => &["body"],
        Field::Attachment => &["attachments"],
    }
}

/// A SQL condition on `messages m`, with its parameters.
struct Predicate {
    sql: String,
    params: Vec<Value>,
}

impl Predicate {
    fn new(sql: impl Into<String>, params: Vec<Value>) -> Predicate {
        Predicate { sql: sql.into(), params }
    }

    fn join(parts: Vec<Predicate>, op: &str) -> Option<Predicate> {
        match parts.len() {
            0 => None,
            1 => parts.into_iter().next(),
            _ => {
                let sql = parts.iter().map(|p| format!("({})", p.sql)).collect::<Vec<_>>().join(op);
                Some(Predicate { sql, params: parts.into_iter().flat_map(|p| p.params).collect() })
            }
        }
    }

    fn not(self, negated: bool) -> Predicate {
        if negated { Predicate { sql: format!("NOT ({})", self.sql), params: self.params } } else { self }
    }
}

fn mailbox_role(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "spam" => "junk".into(),
        "draft" => "drafts".into(),
        "bin" | "deleted" => "trash".into(),
        other => other.into(),
    }
}

fn filter_predicate(filter: &Filter) -> Predicate {
    let flag = |bit: i64, set: bool| {
        Predicate::new(if set { "(m.flags & ?) != 0" } else { "(m.flags & ?) = 0" }, vec![Value::Integer(bit)])
    };
    match filter {
        Filter::Unread => flag(1, false),
        Filter::Read => flag(1, true),
        Filter::Flagged => flag(2, true),
        Filter::Answered => flag(4, true),
        Filter::Draft => flag(8, true),
        Filter::HasAttachment => Predicate::new("m.has_attachment = 1", vec![]),
        Filter::InMailbox(name) => Predicate::new(
            "EXISTS (SELECT 1 FROM message_mailboxes mm JOIN mailboxes b ON b.id = mm.mailbox_id
                     WHERE mm.message = m.rowid AND (b.role = ? OR b.name = ? COLLATE NOCASE OR b.id = ?))",
            vec![Value::Text(mailbox_role(name)), Value::Text(name.clone()), Value::Text(name.clone())],
        ),
        Filter::After(t) => Predicate::new("m.received_at >= ?", vec![Value::Integer(*t)]),
        Filter::Before(t) => Predicate::new("m.received_at < ?", vec![Value::Integer(*t)]),
        Filter::Larger(n) => Predicate::new("m.size > ?", vec![Value::Integer(*n as i64)]),
        Filter::Smaller(n) => Predicate::new("m.size < ?", vec![Value::Integer(*n as i64)]),
    }
}

fn fts_membership(expression: String) -> Predicate {
    Predicate::new("m.rowid IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)", vec![Value::Text(expression)])
}

fn positive(clause: &ExpandedClause) -> ExpandedClause {
    match clause.clone() {
        ExpandedClause::Terms { field, source, alternatives, .. } => {
            ExpandedClause::Terms { field, source, alternatives, negated: false }
        }
        ExpandedClause::Phrase { field, terms, prefix, .. } => ExpandedClause::Phrase { field, terms, prefix, negated: false },
        other => other,
    }
}

/// Any clause as a SQL condition: filters directly, text through an FTS5 subquery.
fn clause_predicate(clause: &ExpandedClause) -> Option<Predicate> {
    match clause {
        ExpandedClause::Filter { filter, negated } => Some(filter_predicate(filter).not(*negated)),
        ExpandedClause::Terms { negated, .. } | ExpandedClause::Phrase { negated, .. } => {
            clause_to_fts5(&positive(clause), columns).map(|e| fts_membership(e).not(*negated))
        }
        ExpandedClause::Any(cs) => Predicate::join(cs.iter().filter_map(clause_predicate).collect(), " OR "),
        ExpandedClause::All(cs) => Predicate::join(cs.iter().filter_map(clause_predicate).collect(), " AND "),
    }
}

impl Reader {
    /// Searches conversations. `meaning` carries the caller's vector scores, when it has a model.
    pub fn search(
        &self,
        query: &Query,
        vocabulary: &Vocabulary,
        meaning: Option<MeaningInput<'_>>,
        now: i64,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        self.search_with(query, vocabulary, meaning, now, limit, &ExpandOptions::default())
    }

    /// `search` with word-matching settings (typo tolerance, how the last word matches).
    pub fn search_with(
        &self,
        query: &Query,
        vocabulary: &Vocabulary,
        meaning: Option<MeaningInput<'_>>,
        now: i64,
        limit: usize,
        words: &ExpandOptions,
    ) -> Result<Vec<SearchHit>> {
        let expanded = expand_with(query, vocabulary, words);
        let matched = to_fts5_match(&expanded, columns, Join::All);
        let mut residual: Vec<Predicate> = matched.residual.iter().filter_map(clause_predicate).collect();
        residual.extend(matched.excluded.iter().map(|e| fts_membership(e.clone()).not(true)));
        let filters = Predicate::join(residual, " AND ");

        let wants_meaning = meaning.is_some() && !query.semantic_text.trim().is_empty();
        let lexical = match &matched.expression {
            Some(expression) => self.word_candidates(expression, filters.as_ref())?,
            None if !wants_meaning => return self.filtered_threads(filters.as_ref(), limit),
            None => Vec::new(),
        };
        let lexical = self.with_coverage(&expanded, lexical)?;

        let merged = match &meaning {
            Some(m) if wants_meaning => merge_channels(&m.channels),
            _ => HashMap::new(),
        };
        let semantic: Vec<SemanticHit>;
        let meaning = if merged.is_empty() {
            Meaning::None
        } else {
            let top: Vec<(i64, f32)> = merged.iter().map(|(&message, &(z, _))| (message, z)).collect();
            semantic = self.allowed_semantic(&top, filters.as_ref())?;
            Meaning::Summary { scale: UNIT, top: &semantic }
        };
        let hits = fuse(query, &lexical, meaning, now, CANDIDATES);
        let found = self.collapse(hits.iter().map(|h| (h.doc, h.score, h.why)), limit)?;
        found
            .into_iter()
            .map(|hit| {
                let part = match hit.why {
                    MatchReason::Words => None,
                    _ => merged.get(&hit.message).and_then(|(_, part)| *part),
                };
                let attachment = part.map(|p| self.attachment_name(hit.message, p)).transpose()?.flatten();
                Ok(SearchHit { attachment, ..hit })
            })
            .collect()
    }

    /// Word candidates: the FTS5 match, filtered, best BM25 first.
    fn word_candidates(&self, expression: &str, filters: Option<&Predicate>) -> Result<Vec<LexicalHit>> {
        let (filter_sql, filter_params) =
            filters.map_or((String::new(), Vec::new()), |p| (format!(" AND ({})", p.sql), p.params.clone()));
        let sql = format!(
            "SELECT f.rowid, bm25(messages_fts, 6.0, 4.0, 2.0, 1.0, 3.0) AS score, m.received_at
             FROM messages_fts f JOIN messages m ON m.rowid = f.rowid
             WHERE messages_fts MATCH ?{filter_sql} ORDER BY score LIMIT {CANDIDATES}"
        );
        let params: Vec<Value> = std::iter::once(Value::Text(expression.to_string())).chain(filter_params).collect();
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(LexicalHit { doc: r.get(0)?, bm25: r.get(1)?, coverage: 1.0, field: Field::Any, received_at: r.get(2)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Share of the query's text clauses each candidate matches, and whether the subject or sender
    /// holds the words. Every candidate of an AND match covers everything; the share matters for
    /// meaning-only candidates and future OR joins.
    fn with_coverage(&self, expanded: &ExpandedQuery, hits: Vec<LexicalHit>) -> Result<Vec<LexicalHit>> {
        if hits.is_empty() {
            return Ok(hits);
        }
        let ids: HashSet<i64> = hits.iter().map(|h| h.doc).collect();
        let header = ExpandedQuery {
            clauses: expanded
                .positive_text_clauses()
                .map(|c| match c.clone() {
                    ExpandedClause::Terms { source, alternatives, negated, .. } => {
                        ExpandedClause::Terms { field: Field::Subject, source, alternatives, negated }
                    }
                    ExpandedClause::Phrase { terms, prefix, negated, .. } => {
                        ExpandedClause::Phrase { field: Field::Subject, terms, prefix, negated }
                    }
                    other => other,
                })
                .collect(),
        };
        let in_subject: HashSet<i64> =
            match to_fts5_match(&header, |_| &["subject", "sender"][..], Join::All).expression {
                Some(e) => self.matching(&e, &ids)?,
                None => HashSet::new(),
            };
        Ok(hits
            .into_iter()
            .map(|h| LexicalHit { field: if in_subject.contains(&h.doc) { Field::Subject } else { Field::Body }, ..h })
            .collect())
    }

    /// Rows matching `expression` among `candidates`. FTS5 answers the match alone far faster than
    /// it checks a long `rowid IN (…)` list, so the intersection happens here.
    fn matching(&self, expression: &str, candidates: &HashSet<i64>) -> Result<HashSet<i64>> {
        let mut stmt = self.conn.prepare_cached("SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1")?;
        let rows = stmt.query_map([expression], |r| r.get::<_, i64>(0))?;
        rows.filter(|r| r.as_ref().map_or(true, |id| candidates.contains(id))).map(|r| r.map_err(Into::into)).collect()
    }

    fn allowed_semantic(&self, top: &[(i64, f32)], filters: Option<&Predicate>) -> Result<Vec<SemanticHit>> {
        if top.is_empty() {
            return Ok(Vec::new());
        }
        let ids = top.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>().join(",");
        let (filter_sql, params) =
            filters.map_or((String::new(), Vec::new()), |p| (format!(" AND ({})", p.sql), p.params.clone()));
        let sql = format!("SELECT m.rowid, m.received_at FROM messages m WHERE m.rowid IN ({ids}){filter_sql}");
        let mut stmt = self.conn.prepare(&sql)?;
        let allowed: HashMap<i64, i64> = stmt
            .query_map(rusqlite::params_from_iter(params), |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(top
            .iter()
            .filter_map(|(doc, cosine)| {
                allowed.get(doc).map(|received_at| SemanticHit { doc: *doc, cosine: *cosine, received_at: *received_at })
            })
            .collect())
    }

    /// A query of filters only: the newest matching conversations.
    fn filtered_threads(&self, filters: Option<&Predicate>, limit: usize) -> Result<Vec<SearchHit>> {
        let Some(filters) = filters else { return Ok(Vec::new()) };
        let sql = format!(
            "SELECT m.rowid FROM messages m WHERE {} ORDER BY m.received_at DESC LIMIT {}",
            filters.sql,
            limit * 4
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let ids: Vec<i64> =
            stmt.query_map(rusqlite::params_from_iter(filters.params.clone()), |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        self.collapse(ids.into_iter().map(|id| (id, 0.0, MatchReason::Words)), limit)
    }

    /// Message hits to conversations, keeping each conversation's best hit and the order.
    fn collapse(&self, hits: impl Iterator<Item = (i64, f64, MatchReason)>, limit: usize) -> Result<Vec<SearchHit>> {
        let mut thread_of = self.conn.prepare_cached("SELECT thread_id FROM messages WHERE rowid = ?1")?;
        let mut row_of = self.conn.prepare_cached(
            "SELECT r.thread_id, r.latest_at, r.latest_message, r.subject, r.participants, r.preview,
                    r.message_count, r.unread_count, r.flagged, r.has_attachment
             FROM thread_rows r LEFT JOIN mailboxes b ON b.id = r.mailbox_id
             WHERE r.thread_id = ?1
             ORDER BY (b.role IN ('trash', 'junk')), r.latest_at DESC LIMIT 1",
        )?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (message, score, why) in hits {
            if out.len() >= limit {
                break;
            }
            let thread: String = match thread_of.query_row([message], |r| r.get(0)) {
                Ok(t) => t,
                Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                Err(e) => return Err(e.into()),
            };
            if !seen.insert(thread.clone()) {
                continue;
            }
            let row = row_of.query_row([&thread], |r| {
                Ok(ThreadRow {
                    thread_id: r.get(0)?,
                    latest_at: r.get(1)?,
                    latest_message: r.get(2)?,
                    subject: r.get(3)?,
                    participants: r.get(4)?,
                    preview: r.get(5)?,
                    message_count: r.get(6)?,
                    unread_count: r.get(7)?,
                    flagged: r.get(8)?,
                    has_attachment: r.get(9)?,
                })
            });
            match row {
                Ok(row) => out.push(SearchHit { row, score, why, message, attachment: None }),
                Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(out)
    }
}
