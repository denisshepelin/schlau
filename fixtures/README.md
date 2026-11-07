# Fixture mailbox

A small, realistic mailbox as JMAP objects on disk. The app loads it with
`cargo run -p schlau-app --release -- --fixtures fixtures/mailbox`, through the same conversion and indexing code as
sync. `scripts/screenshots.sh` renders every scene in `scripts/scenes/` over it, and `crates/schlau-sync/tests/fixtures.rs`
checks threads, search and sanitizing against it.

Brands and people are fictional (`*.example` domains).

## Layout

- `mailbox/mailboxes.json`: `{"mailboxes": [<JMAP Mailbox>, …]}`
- `mailbox/emails/<stem>.json`: a JMAP Email with its headers, keywords, mailboxes and attachments. `"ago": "2h"`
  (`m`, `h` or `d`) places it relative to now, so screenshots always look current.
- `mailbox/emails/<stem>.txt` / `<stem>.html`: its text and HTML bodies, both optional.
- `mailbox/blobs/<attachment name>`: the attachment files. Photos are from Wikimedia Commons (see
  `blobs/CREDITS.md`); the PDFs and the logo sketch come from `scripts/fixture-attachments.py`
  (`uv run scripts/fixture-attachments.py fixtures/mailbox/blobs`). `planning-agenda.pdf` is a scan without a text
  layer, so only image search finds it ("blueish document").

## What's in it

| Case | Emails |
|---|---|
| A long thread with photo attachments, one unread reply | Offsite in Lisbon (7 messages) |
| Notifications with tracking pixels and `utm_` links | Codehub pull request, Tracklane issue |
| Receipts with PDF invoices, tracked click links, a hidden pixel | Paystream × 3 (Receipts) |
| A newsletter with a Mailchimp-style pixel, a Facebook pixel, a Google redirect link | Brightline Weekly |
| Travel: booking reference lookup, Portuguese hotel mail | Northwind Air, Casa do Rio (Travel) |
| Family photos (dogs, for image search), a quoted reply | Bruno at the beach (Family) |
| German letter with a PDF, Japanese meeting note | Hausverwaltung Krüger, Aiko Tanaka |
| Calendar invite (`.ics`), plain-text mailing list with quotes and a signature | Sam Okafor, sqlite-users |
| Hostile HTML: script, meta refresh, `javascript:` link, `onerror`, a form, CSS `url()` | Account Security (Spam) |
| Draft, sent mail, older archived mail for typo searches | Drafts, Sent, Archive |

Regenerating it is not needed; edit the files directly.
