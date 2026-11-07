# schlau

A fast, keyboard-first mail client for Fastmail and any JMAP server, built on
[rune](https://github.com/anomalyco/rune). Everything you see comes from a local SQLite replica, so the UI never
waits on the network.

## Run

```sh
cargo run -p schlau-app --release -- --demo                # 20k synthetic messages, no account
cargo run -p schlau-app --release -- --demo --demo-size 100000
FASTMAIL_API_TOKEN=fmu1-… cargo run -p schlau-app --release  # your Fastmail account
cargo run -p schlau-app --release -- --fixtures fixtures/mailbox --index-attachments  # the fixture mailbox
```

Create a token under Fastmail → Settings → Privacy & Security → API tokens (mail access). Other JMAP servers:
set `JMAP_SESSION_URL` and `JMAP_TOKEN`. On macOS the token can live in the Keychain instead:

```sh
security add-generic-password -s schlau -a fastmail -w fmu1-…
```

Data lives in `~/Library/Application Support/schlau/` (`mail.sqlite`, `demo.sqlite`, `models/`). `--db PATH` uses
another database.

## Keys

| Key | Action |
|---|---|
| `⌘K` | Command palette (every command, every mailbox) |
| `/` | Search |
| `j` / `k`, `↓` / `↑` | Next / previous conversation |
| `↵`, `o`, `l` or `→` | Open |
| `esc`, `u`, `h` or `←` | Back |
| `g f` or `g l` | Go to folder (fuzzy, every folder with its unread count) |
| `space` | Quick look at the message's attachments (`←`/`→` between them, `↵` opens in the default app) |
| `e` | Done (archive) |
| `#` | Trash |
| `!` | Spam |
| `s` | Star |
| `⇧U` / `⇧I` | Mark unread / read |
| `z` | Undo the last action |
| `v` | View the original HTML in the secure web view |
| `⇧E` | Expand or collapse all messages |
| `g i`, `g e`, `g t`, `g d`, `g #`, `g !` | Go to Inbox, Archive, Sent, Drafts, Trash, Spam |
| `⌘R` | Sync now |

## Search

Words are typo-tolerant (`invocie` finds "invoice", `shcema` finds "schema") and the last word matches as a prefix
while you type. Operators: `from:` `to:` `cc:` `subject:` `body:` `filename:`, `in:inbox` / `label:x`,
`is:unread|read|starred|draft|answered`, `has:attachment`, `after:` / `before:` / `newer_than:7d` /
`older_than:1y`, `larger:5M` / `smaller:100k`, `"exact phrases"`, `-negation`, `a OR b`, `{a b}`.

Meaning search (run "Enable meaning search" from the palette, a one-time 530 MB download) adds local embeddings
(Model2Vec, potion-multilingual-128M, 101 languages): "where we talked about the offsite budget" finds the thread
even without those words. Word and meaning hits are fused the way DigUp does it: a message's meaning score is how far
it stands out from the query's distribution, and exact words still win lookups like invoice numbers.

## Image and attachment search

Attachments are indexed in the background: text from PDFs (text layer), Word, PowerPoint, Excel, OpenDocument,
calendar invites and text files joins the word index, so `nachzahlung` finds the German PDF. With the multimodal
model, photos and page images are embedded too, so "dog" finds the beach photos and "blueish document" finds a
scanned agenda; the search panel shows which file matched.

The model is EmbeddingGemma 2 (Apache 2.0): text, images and audio in one 768-dim space. Two ways to run it:

- **Built in** (default): a pure-Rust implementation on candle (`crates/schlau-gemma`), Metal on Apple Silicon.
  Run "Enable image and attachment search" from the palette, a one-time 1.5 GB download. About 290 ms per image
  and 17 ms per query on an M4 Pro. The query side keeps only the text tower loaded; the vision tower loads while
  there are attachments to index and unloads a minute after.
- **A server you already run**: anything that speaks OpenAI `/v1/embeddings`. With llama.cpp:

  ```sh
  llama-server -m embeddinggemma-2-Q8_0.gguf --mmproj mmproj-embeddinggemma-2-Q8_0.gguf \
    --embeddings --pooling mean --image-max-tokens 140 -c 4096 -ub 2048 -b 2048 --port 8080
  SCHLAU_EMBED_URL=http://127.0.0.1:8080 cargo run -p schlau-app --release
  ```

  (`ggml-org/embeddinggemma-2-GGUF`; `SCHLAU_EMBED_MODEL` and `SCHLAU_EMBED_KEY` for other servers.) At 140 image
  tokens its vectors match the built-in model's to a cosine of 0.9996, so switching keeps the index.

## Privacy

Message HTML never reaches a renderer unsanitized. The reader draws its own blocks; the web view (`v`) is a
WKWebView with JavaScript off, a non-persistent store, a CSP that allows no remote loads, and a WebKit rule list that
blocks every http(s) request as a second line. Tracking pixels are removed, remote images are blocked, link wrappers
(Google, Outlook safelinks, Proofpoint, Mandrill, …) are unwrapped and `utm_*`-style parameters stripped; the reader
says what it removed.

## Layout

See [ARCHITECTURE.md](ARCHITECTURE.md).

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
SCHLAU_TEST_DB=path/to/demo.sqlite cargo test --release -p schlau-app keystroke_latency -- --ignored --nocapture
cargo test --release -p schlau-sync --features metal --test attachments   # image search, with the model downloaded
FASTMAIL_API_TOKEN=… cargo run --release -p schlau-sync --example sync -- /tmp/live.sqlite 2 10  # live sync smoke test
```
