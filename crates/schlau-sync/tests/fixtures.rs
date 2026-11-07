//! The fixture mailbox (`fixtures/mailbox`) loaded the way sync loads mail, then read and searched
//! the way the app does.

use std::path::PathBuf;

use schlau_search::{Vocabulary, parse};
use schlau_store::Store;

const NOW: i64 = 1_800_000_000;

fn loaded() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("mail.sqlite")).unwrap();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mailbox");
    let count = schlau_sync::load_fixtures(&mut store, &fixtures, NOW).unwrap();
    assert!(count >= 30, "fixtures hold {count} emails");
    (dir, store)
}

fn search(store: &Store, q: &str) -> Vec<String> {
    let reader = store.reader().unwrap();
    let vocab = Vocabulary::new(reader.vocabulary(1).unwrap().into_iter().map(|(t, d)| (t, d as u32)));
    reader.search(&parse(q, NOW), &vocab, None, NOW, 50).unwrap().into_iter().map(|h| h.row.thread_id).collect()
}

#[test]
fn threads_land_in_their_mailboxes_newest_first() {
    let (_dir, store) = loaded();
    let reader = store.reader().unwrap();
    let inbox = reader.thread_page("inbox", None, 100).unwrap();
    assert_eq!(inbox[0].thread_id, "t-issue", "the issue an hour ago is the newest");
    assert_eq!(inbox[1].thread_id, "t-offsite", "then Priya's reply two hours ago");
    let offsite = &inbox[1];
    assert_eq!(offsite.message_count, 7);
    assert_eq!(offsite.unread_count, 1);
    assert!(offsite.has_attachment);
    assert!(reader.thread_page("junk", None, 10).unwrap().iter().any(|r| r.thread_id == "t-phish"));
    assert!(inbox.iter().all(|r| r.thread_id != "t-phish"), "spam stays out of the inbox");
    assert_eq!(reader.thread_page("drafts", None, 10).unwrap().len(), 1);
}

#[test]
fn fixtures_are_searchable_with_typos_operators_and_other_languages() {
    let (_dir, store) = loaded();
    assert!(search(&store, "lisbn ").contains(&"t-offsite".to_string()));
    assert_eq!(search(&store, "from:paystream has:attachment").len(), 3);
    let mut landlord = search(&store, "nebenkostenabrechnung ");
    landlord.sort();
    assert_eq!(landlord, ["t-landlord", "t-sent-landlord"]);
    assert_eq!(search(&store, "heizung wartung"), ["t-landlord"]);
    assert!(search(&store, "kangroo ").contains(&"t-older-1".to_string()));
    assert!(search(&store, "is:unread in:inbox").contains(&"t-family".to_string()));
    assert_eq!(search(&store, "K7Q2ZP"), ["t-flight"]);
}

#[test]
fn hostile_html_renders_without_scripts_or_remote_loads() {
    let (_dir, store) = loaded();
    let body = store.reader().unwrap().body("025-phish").unwrap().unwrap();
    let html = body.html.unwrap();
    let clean = schlau_html::sanitize(&html, &schlau_html::Options::default());
    let out = clean.html.to_lowercase();
    assert!(!out.contains("<script") && !out.contains("javascript:") && !out.contains("onerror"));
    assert!(!out.contains("http-equiv") && !out.contains("<form"));
    assert!(!out.contains("evil.example/bg.png"));
}

#[test]
fn the_newsletter_reports_its_trackers() {
    let (_dir, store) = loaded();
    let html = store.reader().unwrap().body("014-newsletter").unwrap().unwrap().html.unwrap();
    let report = schlau_html::sanitize(&html, &schlau_html::Options::default()).report;
    assert!(report.pixels_removed >= 2, "{report:?}");
    assert!(report.params_stripped >= 1, "{report:?}");
    assert!(report.tracker_domains.iter().any(|d| d.contains("list-manage")), "{report:?}");
}
