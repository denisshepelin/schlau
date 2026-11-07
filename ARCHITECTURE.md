# schlau

A keyboard-first mail client for Fastmail (and any JMAP server), built to be fast:
every interaction is answered from a local SQLite replica, with p99 under 100 ms.

## Principles

1. **Local first.** The UI never waits on the network. It reads from SQLite. Sync writes
   to SQLite and tells the UI which rows changed. User actions (archive, read, label) apply
   locally at once and go into an outbox that a worker replays against JMAP.
2. **No UI invention.** Superhuman's model: one list, one reading view, a command palette
   (`mod+k`) that can run every action, and single-key shortcuts for the common ones.
3. **Private by construction.** Email HTML is sanitized before anything renders it. Remote
   content is blocked by default, tracking pixels and link wrappers are removed, and the
   reader reports what it blocked.
4. **Search is a feature, not a box.** One query language combines structured filters
   (`from:`, `is:unread`, `after:`), typo-tolerant words, and meaning (embeddings), fused
   the way DigUp fuses vector and keyword hits.

## Crates

| Crate | Responsibility | Depends on |
|---|---|---|
| `schlau-jmap` | JMAP client (RFC 8620 core, RFC 8621 mail): session, Mailbox, Email, Thread, Identity, EmailSubmission, `/changes`, `/queryChanges`, blobs, EventSource push. Blocking HTTP (`ureq`), no async runtime. | – |
| `schlau-html` | Sanitize email HTML for display, strip trackers, unwrap tracking redirects, strip tracking query params, produce plain text and preview snippets. | – |
| `schlau-search` | Query language (AST + parser), typo-tolerant term correction over a vocabulary (`neo_frizbee` from fff), fuzzy scoring for short fields, hybrid fusion of word and meaning hits (DigUp's z-score fusion). Pure: no I/O. | – |
| `schlau-embed` | `Embedder` trait and a pure-Rust static embedding model (Model2Vec), f16 vector packing, SIMD cosine. | – |
| `schlau-store` | SQLite schema and migrations, read models the UI renders (thread list per mailbox, thread view), FTS5 index, vocabulary, vectors, outbox. Executes `schlau-search` queries. | search, embed |
| `schlau-sync` | Sync engine: first sync newest-first, delta sync with `/changes`, push, outbox replay, body fetch on demand and in background. | jmap, store, html, embed |
| `schlau-app` | The rune UI. | store, sync, search, html |

## Data flow

```
JMAP ──(worker thread)──► schlau-sync ──► SQLite (WAL) ◄── schlau-store reads ◄── UI (rune, main thread)
                                 ▲                                   │
                                 └──────── outbox ◄── user actions ──┘
```

- One writer connection (sync thread), many reader connections. WAL mode, `synchronous=NORMAL`,
  mmap, prepared statements cached.
- The UI asks for data through `rune::Latest` workers, so a query never blocks a frame.
- The thread list for a mailbox is a covering index scan: `(mailbox_id, received_at DESC)`.

## Performance budget

| Interaction | Budget (p99) |
|---|---|
| Open app to inbox on screen | 300 ms |
| Move selection, open a thread already cached | 16 ms |
| Switch mailbox | 30 ms |
| Search, keystroke to results (100k messages) | 100 ms |
| Command palette, keystroke to results | 8 ms |
