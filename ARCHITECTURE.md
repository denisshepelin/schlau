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
| `schlau-embed` | `Embedder` trait and a pure-Rust static embedding model (Model2Vec), f16 vector packing, SIMD cosine. `MediaEmbedder` for text and images in one space: EmbeddingGemma 2 in process (feature `gemma`/`metal`) or any OpenAI-compatible server (llama-server). | gemma |
| `schlau-gemma` | EmbeddingGemma 2 on candle (text backbone, vision and audio towers, preprocessing), vendored from mofumail/embeddinggemma-2-candle with fused attention on Metal and a configurable image token budget. | – |
| `schlau-extract` | Attachment bytes to text and downscaled images: images, PDFs (text layer and rendered pages, pure Rust), Office/OpenDocument, calendar invites. Untrusted input: size caps, decoder limits, panics caught. | – |
| `schlau-store` | SQLite schema and migrations, read models the UI renders (thread list per mailbox, thread view), FTS5 index, vocabulary, vectors, outbox. Executes `schlau-search` queries. | search, embed |
| `schlau-sync` | Sync engine: first sync newest-first, delta sync with `/changes`, push, outbox replay, body fetch on demand and in background. The attachment worker (own thread, own connection) fetches blobs, extracts, embeds. | jmap, store, html, embed, extract |
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

## Settings and providers

Each setting is an enum of its choices (`Typos`, `LastWord` in `schlau-search`; `TextMeaning`, `AttachmentMeaning`
in `schlau-sync`; `Previewer` in the app), serialized kebab-case in `settings.json`. The app's `Setting` lists the
settings with their `Choice`s, which the settings pane (`settings_pane.rs`, `view/settings.rs`) draws as rows of
choices; commands stay in the palette. A search change goes to the engine as `Command::Configure(SearchSettings)`;
the engine loads the text provider (Model2Vec, or the multimodal model through `TextOf`) and hands the attachment
backend to the worker. `SharedMedia` holds one loaded query model per backend for the engine, the worker and the
UI. Vectors are stored per model id (`vectors.model`, `media_vectors.model`, `attachment_embedded`), so switching
keeps what each model indexed.

## Performance budget

| Interaction | Budget (p99) | Measured (Apple Silicon, release) |
|---|---|---|
| Open app to inbox on screen | 300 ms | not measured yet |
| Move selection, open a thread already cached | 16 ms | not measured yet (list page read: 0.15 ms) |
| Switch mailbox | 30 ms | page of 400 conversations from 100k messages: < 1 ms |
| Search, keystroke to results (100k messages) | 100 ms | p50 5.5 ms, p99 14.8 ms, max 44 ms (words + filters) |
| Command palette, keystroke to results | 8 ms | fuzzy rank of 500 commands: 0.04 ms p99 |

Image search (M4 Pro): query embedding 17 ms built in, 7 ms through llama-server; indexing 290 ms per image built in,
200 ms through llama-server.

Components: HTML sanitize of a 100 KB marketing email 2.3 ms; typo correction over 500k terms 0.4 ms p99;
scoring 200k × 256-dim vectors 1.3 ms; embedding 35k texts/s.

## Search pipeline

```
input ─► settled (drop a 1-letter word being typed) ─► parse (operators, dates, groups)
      ─► expand (each word ─► exact / prefix / 1–2 typo alternatives from the FTS5 vocabulary)
      ─► FTS5 MATCH (bm25) ∩ SQL filters (flags, mailboxes, dates, size)
      ─► meaning: query embedding · every vector ─► z-scores ─► top hits that pass the filters
      ─► attachments: EmbeddingGemma 2 query vector · every attachment vector ─► z-scores (own scale)
      ─► fuse (best z per message over both, word coverage, bm25, subject/sender bonus, recency)
      ─► collapse to conversations, remembering which attachment matched
```

Attachment text joins the `attachments` column of the word index, so lexical search reaches into PDFs without any
model. Text-to-image cosines are compressed and a mailbox holds far fewer attachments than messages, so the
attachment channel's best three clear standouts (z ≥ 1) are lifted one deviation, over the gate; words still win.

The vocabulary and the vector matrix are cached per worker and rebuilt at most every 5 s after the replica changes.
