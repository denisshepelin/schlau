use crate::Result;
use crate::read::Reader;

/// A full-text match: the message and its BM25 rank (lower is better, as SQLite reports it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FtsHit {
    pub message: i64,
    pub rank: f64,
}

/// Column weights for BM25: subject and sender matter most, then attachment names, recipients, body.
const BM25: &str = "bm25(messages_fts, 6.0, 4.0, 2.0, 1.0, 3.0)";

impl Reader {
    /// Runs an FTS5 MATCH expression, best first.
    pub fn fts_match(&self, expression: &str, limit: usize) -> Result<Vec<FtsHit>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT rowid, {BM25} AS rank FROM messages_fts WHERE messages_fts MATCH ?1 ORDER BY rank LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![expression, limit as i64], |r| {
            Ok(FtsHit { message: r.get(0)?, rank: r.get(1)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every indexed term with the number of messages containing it, for typo correction.
    pub fn vocabulary(&self, min_documents: i64) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare_cached("SELECT term, doc FROM messages_vocab WHERE doc >= ?1")?;
        let rows = stmt.query_map([min_documents], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Whether a term is in the index exactly.
    pub fn has_term(&self, term: &str) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached("SELECT 1 FROM messages_vocab WHERE term = ?1")?
            .exists([term])?)
    }
}
