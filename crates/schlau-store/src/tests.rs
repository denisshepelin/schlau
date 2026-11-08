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
    reader.media_vectors("gemma", |id, _| ids.push(id)).unwrap();
    let mut other = Vec::new();
    reader.media_vectors("another", |id, _| other.push(id)).unwrap();
    assert!(other.is_empty(), "vectors belong to their model");
    assert_eq!(reader.pending_attachments(Some("another"), 10).unwrap().len(), 1, "a second model still has work");
    assert_eq!(ids.iter().map(|&id| media_vector_location(id)).collect::<Vec<_>>(), [(pdf.message, 0), (pdf.message, 0)]);
    assert_eq!(reader.attachment_name(pdf.message, 0).unwrap().as_deref(), Some("Nebenkosten.pdf"));

    store.write(|tx| tx.delete_message("m1")).unwrap();
    assert_eq!(store.reader().unwrap().media_vector_count("gemma").unwrap(), 0);
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

#[test]
fn message_vectors_are_kept_per_model() {
    let (_dir, mut store) = store();
    store
        .write(|tx| {
            tx.upsert_message(&message("m1", "t1", 100, ("Jane", "jane@example.com"), "Hello", &["inbox"]))?;
            tx.set_body("m1", &Body { text: Some("hi".into()), ..Default::default() }, &BodyIndex { text: "hi".into() })?;
            tx.put_vectors("potion", "m1", &[vec![1u8, 2]])
        })
        .unwrap();
    let reader = store.reader().unwrap();
    assert!(reader.messages_without_vectors("potion", 10).unwrap().is_empty());
    assert_eq!(reader.messages_without_vectors("gemma", 10).unwrap().len(), 1, "another model starts its own index");
    store.write(|tx| tx.put_vectors("gemma", "m1", &[vec![3u8, 4], vec![5, 6]])).unwrap();
    let reader = store.reader().unwrap();
    let count = |model: &str| {
        let mut n = 0;
        reader.vectors(model, |_, _| n += 1).unwrap();
        n
    };
    assert_eq!((count("potion"), count("gemma")), (1, 2), "switching back finds the first model's vectors");
    assert_eq!(reader.vector_progress("gemma").unwrap(), (1, 1));
    store.write(|tx| tx.forget_model("potion")).unwrap();
    assert_eq!(store.reader().unwrap().vector_progress("potion").unwrap().0, 0);
}

fn group(id: &str, name: &str, members: &[&str]) -> ContactRecord {
    ContactRecord {
        id: id.into(),
        uid: format!("uid-{id}"),
        kind: "group".into(),
        name: name.into(),
        members: members.iter().map(|m| m.to_string()).collect(),
        ..Default::default()
    }
}

fn person(id: &str, emails: &[&str]) -> ContactRecord {
    ContactRecord {
        id: id.into(),
        uid: format!("uid-{id}"),
        kind: "individual".into(),
        name: id.into(),
        emails: emails.iter().map(|e| e.to_string()).collect(),
        ..Default::default()
    }
}

fn screener_store() -> (tempfile::TempDir, Store) {
    let (dir, mut store) = store();
    store
        .write(|tx| {
            tx.upsert_mailbox(&mailbox("triage", None))?;
            tx.upsert_mailbox(&mailbox("news", None))?;
            tx.upsert_mailbox(&mailbox("sent", Some("sent")))?;
            tx.upsert_contact(&group("g-news", "Screener · News", &["uid-p-billing"]))?;
            tx.upsert_contact(&group("g-out", "Screener · Out", &[]))?;
            tx.upsert_contact(&person("p-billing", &["Billing@Shop.example"]))?;
            tx.upsert_contact(&person("p-friend", &["friend@x.com"]))?;
            tx.upsert_message(&message("m1", "t1", 100, ("Shop", "Receipts@shop.example"), "Receipt 1", &["triage"]))?;
            tx.upsert_message(&message("m2", "t2", 300, ("Shop", "receipts@shop.example"), "Receipt 2", &["triage"]))?;
            tx.upsert_message(&message("m3", "t3", 200, ("Ann", "ann@x.com"), "Hello", &["triage"]))?;
            tx.upsert_message(&message("m4", "t4", 50, ("Shop", "receipts@shop.example"), "Old receipt", &["archive"]))?;
            let mut sent = message("m5", "t3", 250, ("Me", "me@example.com"), "Re: Hello", &["sent"]);
            sent.addresses.to = vec![Address { name: "Ann".into(), email: "Ann@x.com".into() }];
            tx.upsert_message(&sent).map(|_| ())
        })
        .unwrap();
    (dir, store)
}

#[test]
fn the_screener_groups_a_folder_by_sender_with_what_the_replica_knows() {
    let (_dir, store) = screener_store();
    let senders = store.reader().unwrap().screener_senders("triage").unwrap();
    assert_eq!(senders.iter().map(|s| s.email.as_str()).collect::<Vec<_>>(), ["receipts@shop.example", "ann@x.com"]);
    let shop = &senders[0];
    assert_eq!(shop.message_ids, ["m2", "m1"]);
    assert_eq!((shop.unread, shop.latest_subject.as_str(), shop.contact, shop.replied), (2, "Receipt 2", false, false));
    assert_eq!(shop.elsewhere, [("archive".to_string(), 1)]);
    assert_eq!(shop.domain_groups, [("g-news".to_string(), 1)], "billing@ at the same domain is filed");
    assert!(senders[1].replied, "Sent has a reply to Ann");
}

#[test]
fn adding_a_sender_to_a_group_makes_a_pending_card_until_the_server_has_one() {
    let (_dir, mut store) = screener_store();
    let add = Action::AddToGroup {
        group_id: "g-news".into(),
        email: "Receipts@shop.example".into(),
        name: "Shop".into(),
        uid: "urn:uuid:new".into(),
    };
    store.write(|tx| tx.perform(&add)).unwrap();
    let reader = store.reader().unwrap();
    assert_eq!(reader.card_for_email("receipts@shop.example").unwrap(), Some(("urn:uuid:new".into(), true)));
    assert_eq!(reader.groups_of("receipts@shop.example").unwrap(), ["g-news"]);
    assert_eq!(reader.member_uids("g-news", "receipts@shop.example").unwrap(), ["urn:uuid:new"]);

    store.write(|tx| tx.perform(&Action::AddToGroup {
        group_id: "g-out".into(),
        email: "friend@x.com".into(),
        name: String::new(),
        uid: "urn:uuid:unused".into(),
    })).unwrap();
    assert_eq!(store.reader().unwrap().member_uids("g-out", "friend@x.com").unwrap(), ["uid-p-friend"], "an existing card is reused");

    let remove = Action::RemoveFromGroup {
        group_id: "g-news".into(),
        email: "receipts@shop.example".into(),
        name: "Shop".into(),
        uid: "urn:uuid:new".into(),
        delete_card: true,
    };
    store.write(|tx| tx.perform(&remove)).unwrap();
    let reader = store.reader().unwrap();
    assert!(reader.groups_of("receipts@shop.example").unwrap().is_empty());
    assert_eq!(reader.card_by_uid("urn:uuid:new").unwrap(), Some(("pending:urn:uuid:new".into(), 0)), "kept for the replay to delete");
    assert_eq!(reader.outbox().unwrap().len(), 3);
}

#[test]
fn a_group_not_in_the_replica_is_left_alone() {
    let (_dir, mut store) = screener_store();
    let add = Action::AddToGroup { group_id: "g-missing".into(), email: "a@b.c".into(), name: String::new(), uid: "u".into() };
    store.write(|tx| tx.apply_locally(&add)).unwrap();
    assert_eq!(store.reader().unwrap().card_for_email("a@b.c").unwrap(), None);
}

#[test]
fn messages_from_group_members_outside_their_folder_are_misrouted() {
    let (_dir, mut store) = screener_store();
    store
        .write(|tx| {
            tx.upsert_message(&message("m6", "t6", 400, ("Billing", "billing@shop.example"), "Invoice", &["triage"]))?;
            tx.upsert_message(&message("m7", "t7", 410, ("Billing", "billing@shop.example"), "Invoice 2", &["news"]))?;
            tx.upsert_message(&message("m8", "t8", 420, ("Billing", "billing@shop.example"), "Invoice 3", &["inbox"]))
                .map(|_| ())
        })
        .unwrap();
    let reader = store.reader().unwrap();
    let groups = [("g-out".to_string(), "junk".to_string()), ("g-news".to_string(), "news".to_string())];
    let found = reader.misrouted(&["triage".into(), "inbox".into()], &groups).unwrap();
    assert_eq!(
        found,
        [
            Misrouted { message_id: "m6".into(), mailbox_id: "triage".into(), group_id: "g-news".into() },
            Misrouted { message_id: "m8".into(), mailbox_id: "inbox".into(), group_id: "g-news".into() },
        ]
    );
    assert_eq!(reader.group_of_message("m7", &["g-news".into()]).unwrap().as_deref(), Some("g-news"));
    assert_eq!(reader.group_of_message("m1", &["g-news".into()]).unwrap(), None);
}

#[test]
fn routing_events_count_once_per_message() {
    let (_dir, mut store) = screener_store();
    store
        .write(|tx| {
            tx.record_screener_event("m1", "g-news", true)?;
            tx.record_screener_event("m1", "g-news", false)?;
            tx.record_screener_event("m2", "g-news", false)
        })
        .unwrap();
    let stats = store.reader().unwrap().routing_stats("g-news").unwrap();
    assert_eq!((stats.routed, stats.missed), (1, 1));
    assert!(stats.last_missed_at.is_some());
    assert_eq!(store.reader().unwrap().routing_stats("g-out").unwrap(), RoutingStats::default());
}

#[test]
fn a_full_contact_sync_replaces_cards_and_drops_pending_ones() {
    let (_dir, mut store) = screener_store();
    store
        .write(|tx| tx.perform(&Action::AddToGroup { group_id: "g-news".into(), email: "n@x.com".into(), name: String::new(), uid: "u-n".into() }))
        .unwrap();
    store
        .write(|tx| {
            tx.replace_contacts(&[group("g-news", "Screener · News", &[]), person("p-n", &["n@x.com"])])?;
            tx.drop_pending_contacts()
        })
        .unwrap();
    let reader = store.reader().unwrap();
    assert_eq!(reader.contact_count().unwrap(), 2);
    assert_eq!(reader.contact_groups().unwrap(), [ContactGroup { id: "g-news".into(), name: "Screener · News".into(), members: 0 }]);
    assert_eq!(reader.card_for_email("n@x.com").unwrap(), Some(("uid-p-n".into(), false)));
}

/// Opens (and so migrates) the database in `SCHLAU_MIGRATE_DB`: run it on a copy of a real replica before a
/// schema change ships. `SCHLAU_MIGRATE_DB=… cargo test -p schlau-store migrates_a_copy -- --ignored --nocapture`.
#[test]
#[ignore = "needs SCHLAU_MIGRATE_DB pointing at a copy of a database"]
fn migrates_a_copy() {
    let path = std::path::PathBuf::from(std::env::var("SCHLAU_MIGRATE_DB").unwrap());
    let started = Instant::now();
    let store = Store::open(&path).unwrap();
    let reader = store.reader().unwrap();
    let version: i64 = reader.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    let count = |sql: &str| -> i64 { reader.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    eprintln!(
        "migrated to v{version} in {:.0} ms: {} messages, {} media vectors, {} attachments embedded, {} pending",
        started.elapsed().as_secs_f64() * 1000.0,
        reader.message_count().unwrap(),
        count("SELECT count(*) FROM media_vectors"),
        count("SELECT count(*) FROM attachment_embedded"),
        reader.pending_attachments(Some("embeddinggemma-2/768"), 100_000).unwrap().len(),
    );
}
