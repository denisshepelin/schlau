use std::time::Instant;

use super::*;

fn mailbox(id: &str, role: Option<&str>) -> Mailbox {
    Mailbox {
        id: id.into(),
        name: id.into(),
        parent_id: None,
        role: role.map(Into::into),
        sort_order: 0,
        total_emails: 0,
        unread_emails: 0,
        total_threads: 0,
        unread_threads: 0,
        is_subscribed: true,
    }
}

fn message(id: &str, thread: &str, at: i64, from: (&str, &str), subject: &str, boxes: &[&str]) -> MessageRecord {
    MessageRecord {
        id: id.into(),
        thread_id: thread.into(),
        received_at: at,
        subject: subject.into(),
        addresses: Addresses {
            from: vec![Address { name: from.0.into(), email: from.1.into() }],
            to: vec![Address { name: "Me".into(), email: "me@example.com".into() }],
            ..Default::default()
        },
        preview: format!("preview of {id}"),
        mailbox_ids: boxes.iter().map(|b| b.to_string()).collect(),
        ..Default::default()
    }
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("mail.sqlite")).unwrap();
    store
        .write(|tx| {
            tx.upsert_mailbox(&mailbox("inbox", Some("inbox")))?;
            tx.upsert_mailbox(&mailbox("archive", Some("archive")))?;
            tx.upsert_mailbox(&mailbox("trash", Some("trash")))
        })
        .unwrap();
    (dir, store)
}

#[test]
fn reopening_keeps_the_schema_and_data() {
    let (dir, store) = store();
    drop(store);
    let store = Store::open(dir.path().join("mail.sqlite")).unwrap();
    assert_eq!(store.reader().unwrap().mailboxes().unwrap().len(), 3);
}

#[test]
fn a_mailbox_lists_conversations_newest_first_and_pages_by_cursor() {
    let (_dir, mut store) = store();
    store
        .write(|tx| {
            tx.upsert_message(&message("m1", "t1", 100, ("Jane", "jane@x.com"), "Lunch", &["inbox"]))?;
            tx.upsert_message(&message("m2", "t2", 300, ("Bob", "bob@x.com"), "Invoice", &["inbox"]))?;
            tx.upsert_message(&message("m3", "t1", 200, ("Bob", "bob@x.com"), "Re: Lunch", &["inbox"]))?;
            tx.upsert_message(&message("m4", "t3", 50, ("Ann", "ann@x.com"), "Old", &["inbox"]))
        })
        .unwrap();
    let reader = store.reader().unwrap();
    let first = reader.thread_page("inbox", None, 2).unwrap();
    assert_eq!(first.iter().map(|r| r.thread_id.as_str()).collect::<Vec<_>>(), ["t2", "t1"]);
    assert_eq!(first[1].message_count, 2);
    assert_eq!(first[1].participants, "Jane, Bob");
    assert_eq!(first[1].subject, "Lunch");
    let last = &first[1];
    let next = reader
        .thread_page("inbox", Some(PageCursor { latest_at: last.latest_at, thread_id: &last.thread_id }), 2)
        .unwrap();
    assert_eq!(next.iter().map(|r| r.thread_id.as_str()).collect::<Vec<_>>(), ["t3"]);
}

#[test]
fn archiving_moves_the_conversation_and_queues_the_change() {
    let (_dir, mut store) = store();
    store
        .write(|tx| tx.upsert_message(&message("m1", "t1", 100, ("Jane", "jane@x.com"), "Hi", &["inbox"])).map(|_| ()))
        .unwrap();
    let action = Action::Move { message_ids: vec!["m1".into()], from: Some("inbox".into()), to: "archive".into() };
    store.write(|tx| tx.perform(&action)).unwrap();
    let reader = store.reader().unwrap();
    assert!(reader.thread_page("inbox", None, 10).unwrap().is_empty());
    assert_eq!(reader.thread_page("archive", None, 10).unwrap().len(), 1);
    let outbox = reader.outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].action, action);
}

#[test]
fn marking_read_updates_the_unread_count() {
    let (_dir, mut store) = store();
    store
        .write(|tx| tx.upsert_message(&message("m1", "t1", 100, ("Jane", "jane@x.com"), "Hi", &["inbox"])).map(|_| ()))
        .unwrap();
    assert_eq!(store.reader().unwrap().thread_page("inbox", None, 1).unwrap()[0].unread_count, 1);
    store
        .write(|tx| tx.perform(&Action::SetFlag { message_ids: vec!["m1".into()], flag: "$seen".into(), on: true }))
        .unwrap();
    assert_eq!(store.reader().unwrap().thread_page("inbox", None, 1).unwrap()[0].unread_count, 0);
}

#[test]
fn bodies_are_indexed_for_full_text_search_and_deletes_leave_the_index() {
    let (_dir, mut store) = store();
    store
        .write(|tx| {
            tx.upsert_message(&message("m1", "t1", 100, ("Jane Doe", "jane@acme.com"), "Quarterly report", &["inbox"]))?;
            tx.set_body(
                "m1",
                &Body { text: Some("The kangaroo budget is attached.".into()), ..Default::default() },
                &BodyIndex { text: "The kangaroo budget is attached.".into() },
            )
        })
        .unwrap();
    let reader = store.reader().unwrap();
    assert_eq!(reader.fts_match("kangaroo", 10).unwrap().len(), 1);
    assert_eq!(reader.fts_match("sender: acme", 10).unwrap().len(), 1);
    assert_eq!(reader.fts_match("quarter*", 10).unwrap().len(), 1);
    assert!(reader.has_term("kangaroo").unwrap());
    assert_eq!(reader.body("m1").unwrap().unwrap().text.as_deref(), Some("The kangaroo budget is attached."));
    assert!(reader.missing_bodies(10).unwrap().is_empty());

    store.write(|tx| tx.delete_message("m1")).unwrap();
    let reader = store.reader().unwrap();
    assert!(reader.fts_match("kangaroo", 10).unwrap().is_empty());
    assert!(reader.thread_page("inbox", None, 10).unwrap().is_empty());
}

#[test]
fn keyword_updates_keep_the_fetched_body() {
    let (_dir, mut store) = store();
    let m = message("m1", "t1", 100, ("Jane", "jane@x.com"), "Hi", &["inbox"]);
    store
        .write(|tx| {
            tx.upsert_message(&m)?;
            tx.set_body("m1", &Body { text: Some("zebra".into()), ..Default::default() }, &BodyIndex { text: "zebra".into() })
        })
        .unwrap();
    store.write(|tx| tx.upsert_message(&MessageRecord { flags: Flags::SEEN, ..m.clone() })).unwrap();
    let reader = store.reader().unwrap();
    assert_eq!(reader.fts_match("zebra", 10).unwrap().len(), 1);
    assert!(reader.thread_messages("t1").unwrap()[0].body_fetched);
}

/// The list's hot path stays fast at mailbox scale: 100k messages in 25k threads, then the first
/// page and a deep page of the inbox. Budgets are loose for debug builds; release is far below.
#[test]
fn paging_a_hundred_thousand_message_mailbox_is_fast() {
    let (_dir, mut store) = store();
    let started = Instant::now();
    store
        .write(|tx| {
            (0..100_000).try_for_each(|i| {
                let thread = format!("t{}", i / 4);
                let m = message(&format!("m{i}"), &thread, i, ("Sender", "s@x.com"), "Subject", &["inbox"]);
                tx.upsert_message(&m).map(|_| ())
            })
        })
        .unwrap();
    let load = started.elapsed();
    let reader = store.reader().unwrap();

    let started = Instant::now();
    let page = reader.thread_page("inbox", None, 100).unwrap();
    let first = started.elapsed();
    assert_eq!(page.len(), 100);
    assert_eq!(page[0].thread_id, "t24999");

    let started = Instant::now();
    let cursor = PageCursor { latest_at: 50_000, thread_id: "t12500" };
    let deep = reader.thread_page("inbox", Some(cursor), 100).unwrap();
    let deep_time = started.elapsed();
    assert_eq!(deep.len(), 100);

    eprintln!("load {load:?}, first page {first:?}, deep page {deep_time:?}");
    assert!(first.as_millis() < 20, "first page took {first:?}");
    assert!(deep_time.as_millis() < 20, "deep page took {deep_time:?}");
}

mod search {
    use super::*;
    use schlau_search::{MeaningScale, Vocabulary, parse};

    const NOW: i64 = 1_800_000_000;

    fn seeded() -> (tempfile::TempDir, Store) {
        let (dir, mut store) = store();
        store
            .write(|tx| {
                let mut a = message("m1", "t1", NOW - 100, ("Jane Cooper", "jane@acme.com"), "Invoice for March", &["inbox"]);
                a.has_attachment = true;
                tx.upsert_message(&a)?;
                tx.set_body("m1", &Body::default(), &BodyIndex { text: "Please receive the attached invoice.".into() })?;
                let mut b = message("m2", "t2", NOW - 50, ("Bob Stone", "bob@lumen.dev"), "Lunch plans", &["archive"]);
                b.flags = Flags::SEEN;
                tx.upsert_message(&b)?;
                tx.set_body("m2", &Body::default(), &BodyIndex { text: "Ramen near the office on Thursday?".into() })?;
                tx.upsert_message(&message("m3", "t3", NOW - 10, ("Jane Cooper", "jane@acme.com"), "Schema review", &["inbox"]))
                    .map(|_| ())
            })
            .unwrap();
        (dir, store)
    }

    fn find(store: &Store, q: &str) -> Vec<String> {
        let reader = store.reader().unwrap();
        let vocab = Vocabulary::new(reader.vocabulary(1).unwrap().into_iter().map(|(t, d)| (t, d as u32)));
        reader
            .search(&parse(q, NOW), &vocab, None, NOW, 20)
            .unwrap()
            .into_iter()
            .map(|h| h.row.thread_id)
            .collect()
    }

    #[test]
    fn typos_still_find_the_word() {
        let (_dir, store) = seeded();
        assert_eq!(find(&store, "invocie "), ["t1"]);
        assert_eq!(find(&store, "recieve "), ["t1"]);
        assert_eq!(find(&store, "shcema "), ["t3"]);
    }

    #[test]
    fn the_last_word_matches_as_a_prefix_while_typing() {
        let (_dir, store) = seeded();
        assert_eq!(find(&store, "ram"), ["t2"]);
    }

    #[test]
    fn operators_and_filters_narrow_the_results() {
        let (_dir, store) = seeded();
        assert_eq!(find(&store, "from:jane"), ["t3", "t1"]);
        assert_eq!(find(&store, "from:jane has:attachment"), ["t1"]);
        assert_eq!(find(&store, "is:unread in:inbox"), ["t3", "t1"]);
        assert_eq!(find(&store, "in:archive"), ["t2"]);
        assert_eq!(find(&store, "-from:jane"), ["t2"]);
        assert!(find(&store, "subject:ramen").is_empty());
    }

    #[test]
    fn meaning_alone_finds_a_message_without_the_words() {
        let (_dir, store) = seeded();
        let reader = store.reader().unwrap();
        let vocab = Vocabulary::new(reader.vocabulary(1).unwrap().into_iter().map(|(t, d)| (t, d as u32)));
        let rowid = |id: &str| reader.thread_messages(&format!("t{}", &id[1..])).unwrap()[0].rowid;
        let top = [(rowid("m2"), 0.9_f32), (rowid("m1"), 0.1)];
        let meaning = MeaningInput {
            channels: vec![MeaningChannel { scale: MeaningScale { mean: 0.1, deviation: 0.1 }, top: &top, attachments: false }],
        };
        let hits = reader.search(&parse("where to eat noodles", NOW), &vocab, Some(meaning), NOW, 10).unwrap();
        assert_eq!(hits.iter().map(|h| h.row.thread_id.as_str()).collect::<Vec<_>>(), ["t2"]);
    }
}

fn attachment(blob: &str, name: &str, mime: &str) -> Attachment {
    Attachment { blob_id: blob.into(), name: Some(name.into()), mime: mime.into(), size: 1000, cid: None, inline: false }
}

#[test]
fn attachments_wait_for_extraction_then_for_vectors_and_their_text_is_searchable() {
    let (_dir, mut store) = store();
    let body = Body {
        text: Some("See the attached bill.".into()),
        attachments: vec![
            attachment("b1", "Nebenkosten.pdf", "application/pdf"),
            Attachment { inline: true, cid: Some("logo".into()), ..attachment("b2", "logo.png", "image/png") },
            attachment("b3", "beach.jpg", "image/jpeg"),
        ],
        ..Default::default()
    };
    store
        .write(|tx| {
            tx.upsert_message(&message("m1", "t1", 100, ("Krüger", "info@krueger.example"), "Abrechnung", &["inbox"]))?;
            tx.set_body("m1", &body, &BodyIndex { text: "See the attached bill.".into() })
        })
        .unwrap();
    let reader = store.reader().unwrap();
    let pending = reader.pending_attachments(None, 10).unwrap();
    assert_eq!(pending.iter().map(|p| p.part).collect::<Vec<_>>(), [0, 2], "the inline logo is not indexed");
    let (pdf, photo) = (pending[0].clone(), pending[1].clone());
    assert_eq!(reader.attachment_progress(None).unwrap(), (0, 2));

    let pdf_text = AttachmentOutcome { text: Some("Nachzahlung 177,80 EUR".into()), ..Default::default() };
    let skipped = AttachmentOutcome { skipped: Some("no decoder".into()), ..Default::default() };
    store
        .write(|tx| {
            tx.index_attachment(pdf.message, pdf.part, &pdf_text, "See the attached bill.")?;
            tx.index_attachment(photo.message, photo.part, &skipped, "See the attached bill.")
        })
        .unwrap();
    let reader = store.reader().unwrap();
    assert_eq!(reader.fts_match("nachzahlung", 10).unwrap().len(), 1, "attachment text joins the word index");
    assert_eq!(reader.fts_match("attached", 10).unwrap().len(), 1, "the body stays indexed");
    assert_eq!(reader.fts_match("nebenkosten", 10).unwrap().len(), 1, "and the file name");
    assert!(reader.pending_attachments(None, 10).unwrap().is_empty());
    assert_eq!(reader.attachment_progress(None).unwrap(), (2, 2));

    let again = reader.pending_attachments(Some("gemma"), 10).unwrap();
    assert_eq!(again.len(), 1, "a model brings back extracted attachments, not skipped ones");
    assert!(again[0].extracted);
    let vectors = AttachmentOutcome { vectors: vec![vec![0u8; 4], vec![1u8; 4]], model: Some("gemma".into()), ..Default::default() };
    store.write(|tx| tx.index_attachment(pdf.message, pdf.part, &vectors, "See the attached bill.")).unwrap();
    let reader = store.reader().unwrap();
    assert!(reader.pending_attachments(Some("gemma"), 10).unwrap().is_empty());
    assert_eq!(reader.fts_match("nachzahlung", 10).unwrap().len(), 1, "a vectors-only pass keeps the text");
    let mut ids = Vec::new();
    reader.media_vectors(|id, _| ids.push(id)).unwrap();
    assert_eq!(ids.iter().map(|&id| media_vector_location(id)).collect::<Vec<_>>(), [(pdf.message, 0), (pdf.message, 0)]);
    assert_eq!(reader.attachment_name(pdf.message, 0).unwrap().as_deref(), Some("Nebenkosten.pdf"));

    store.write(|tx| tx.delete_message("m1")).unwrap();
    assert_eq!(store.reader().unwrap().media_vector_count().unwrap(), 0);
}

#[test]
fn a_photo_that_means_the_query_finds_its_conversation_and_says_which_file() {
    use schlau_search::{MeaningScale, Vocabulary, parse};
    let (_dir, mut store) = store();
    let body = Body { text: Some("Look who found the sea.".into()), attachments: vec![attachment("b1", "bruno-beach.jpg", "image/jpeg")], ..Default::default() };
    store
        .write(|tx| {
            tx.upsert_message(&message("m1", "t1", 100, ("Mum", "mum@example.com"), "Sunday", &["inbox"]))?;
            tx.upsert_message(&message("m2", "t2", 200, ("Bank", "bank@example.com"), "Statement", &["inbox"]))?;
            tx.set_body("m1", &body, &BodyIndex { text: "Look who found the sea.".into() })
        })
        .unwrap();
    let reader = store.reader().unwrap();
    let m1 = reader.thread_messages("t1").unwrap()[0].rowid;
    let m2 = reader.thread_messages("t2").unwrap()[0].rowid;
    let vocab = Vocabulary::new(reader.vocabulary(1).unwrap().into_iter().map(|(t, d)| (t, d as u32)));
    let text = [(m2, 0.31_f32), (m1, 0.30)];
    let media = [(media_vector_id(m1, 0, 0), 0.71_f32)];
    let meaning = MeaningInput {
        channels: vec![
            MeaningChannel { scale: MeaningScale { mean: 0.3, deviation: 0.05 }, top: &text, attachments: false },
            MeaningChannel { scale: MeaningScale { mean: 0.55, deviation: 0.04 }, top: &media, attachments: true },
        ],
    };
    let hits = reader.search(&parse("dog", 1_000), &vocab, Some(meaning), 1_000, 10).unwrap();
    assert_eq!(hits[0].row.thread_id, "t1");
    assert_eq!(hits[0].attachment.as_deref(), Some("bruno-beach.jpg"));
}
