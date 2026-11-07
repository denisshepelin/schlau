//! The sync steps against canned JMAP responses: first sync, a delta, outbox replay and bodies.

use std::sync::Arc;

use schlau_jmap::mock::MockTransport;
use schlau_jmap::{Client, Session, Transport};
use schlau_store::{Action, Flags, Store};
use schlau_sync::Remote;

const SESSION: &[u8] = include_bytes!("../../schlau-jmap/fixtures/session_fastmail.json");
const ACCOUNT: &str = "u1a2b3c4d";

fn remote(mock: &Arc<MockTransport>) -> Remote {
    let transport: Arc<dyn Transport> = mock.clone();
    let client = Client::with_session(transport, Session::from_json(SESSION).unwrap());
    Remote::new(client).unwrap()
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("mail.sqlite")).unwrap();
    (dir, store)
}

fn email(id: &str, thread: &str, at: &str, seen: bool, mailbox: &str, subject: &str) -> String {
    format!(
        r#"{{"id":"{id}","threadId":"{thread}","mailboxIds":{{"{mailbox}":true}},"keywords":{{"$seen":{seen}}},
            "receivedAt":"{at}","subject":"{subject}","from":[{{"name":"Jane","email":"jane@x.com"}}],
            "preview":"p","size":100}}"#
    )
}

fn mailboxes_response() -> String {
    format!(
        r#"{{"methodResponses":[["Mailbox/get",{{"accountId":"{ACCOUNT}","state":"mb1","list":[
            {{"id":"inbox","name":"Inbox","role":"inbox","totalEmails":2,"unreadEmails":1}},
            {{"id":"archive","name":"Archive","role":"archive"}}],"notFound":[]}},"c0"]],"sessionState":"s"}}"#
    )
}

fn first_page() -> String {
    format!(
        r#"{{"methodResponses":[
            ["Email/query",{{"accountId":"{ACCOUNT}","queryState":"q1","canCalculateChanges":true,"position":0,
              "ids":["m2","m1"],"total":2}},"c0"],
            ["Email/get",{{"accountId":"{ACCOUNT}","state":"e1","list":[{},{}],"notFound":[]}},"c1"]
        ],"sessionState":"s"}}"#,
        email("m2", "t1", "2026-03-02T10:00:00Z", false, "inbox", "Re: Plans"),
        email("m1", "t1", "2026-03-01T10:00:00Z", true, "inbox", "Plans"),
    )
}

fn first_sync(mock: &Arc<MockTransport>, store: &mut Store) -> Remote {
    let r = remote(mock);
    mock.push_json(&mailboxes_response());
    mock.push_json(&first_page());
    r.claim(store).unwrap();
    assert!(r.sync_mailboxes(store).unwrap());
    assert!(r.start_emails(store).unwrap());
    r
}

#[test]
fn the_first_sync_lists_the_newest_mail_and_finishes_backfill_when_it_has_everything() {
    let mock = Arc::new(MockTransport::new());
    let (_dir, mut store) = store();
    let r = first_sync(&mock, &mut store);
    let reader = store.reader().unwrap();
    let rows = reader.thread_page("inbox", None, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_count, 2);
    assert_eq!(rows[0].unread_count, 1);
    assert_eq!(rows[0].subject, "Plans");
    assert_eq!(reader.meta("jmap.email_state").unwrap().as_deref(), Some("e1"));
    assert!(!r.backfill(&mut store).unwrap(), "two of two messages: nothing older to fetch");
}

#[test]
fn a_delta_applies_new_updated_and_destroyed_mail() {
    let mock = Arc::new(MockTransport::new());
    let (_dir, mut store) = store();
    let r = first_sync(&mock, &mut store);
    mock.push_json(&format!(
        r#"{{"methodResponses":[
            ["Email/changes",{{"accountId":"{ACCOUNT}","oldState":"e1","newState":"e2","hasMoreChanges":false,
              "created":["m3"],"updated":["m2"],"destroyed":["m1"]}},"c0"],
            ["Email/get",{{"accountId":"{ACCOUNT}","state":"e2","list":[{}],"notFound":[]}},"c1"],
            ["Email/get",{{"accountId":"{ACCOUNT}","state":"e2","list":[
                {{"id":"m2","mailboxIds":{{"archive":true}},"keywords":{{"$seen":true}}}}],"notFound":[]}},"c2"]
        ],"sessionState":"s"}}"#,
        email("m3", "t2", "2026-03-03T10:00:00Z", false, "inbox", "New"),
    ));
    assert!(r.sync_emails(&mut store).unwrap());
    let reader = store.reader().unwrap();
    let inbox = reader.thread_page("inbox", None, 10).unwrap();
    assert_eq!(inbox.iter().map(|r| r.thread_id.as_str()).collect::<Vec<_>>(), ["t2"]);
    let archive = reader.thread_page("archive", None, 10).unwrap();
    assert_eq!(archive.len(), 1);
    assert_eq!(archive[0].unread_count, 0);
    assert_eq!(reader.meta("jmap.email_state").unwrap().as_deref(), Some("e2"));
}

#[test]
fn a_pending_action_survives_a_delta_that_predates_it() {
    let mock = Arc::new(MockTransport::new());
    let (_dir, mut store) = store();
    let r = first_sync(&mock, &mut store);
    let archive = Action::Move { message_ids: vec!["m2".into()], from: Some("inbox".into()), to: "archive".into() };
    store.write(|tx| tx.perform(&archive)).unwrap();
    mock.push_json(&format!(
        r#"{{"methodResponses":[
            ["Email/changes",{{"accountId":"{ACCOUNT}","oldState":"e1","newState":"e2","hasMoreChanges":false,
              "created":[],"updated":["m2"],"destroyed":[]}},"c0"],
            ["Email/get",{{"accountId":"{ACCOUNT}","state":"e2","list":[],"notFound":[]}},"c1"],
            ["Email/get",{{"accountId":"{ACCOUNT}","state":"e2","list":[
                {{"id":"m2","mailboxIds":{{"inbox":true}},"keywords":{{"$seen":true}}}}],"notFound":[]}},"c2"]
        ],"sessionState":"s"}}"#
    ));
    r.sync_emails(&mut store).unwrap();
    let thread = store.reader().unwrap().thread_messages("t1").unwrap();
    let m2 = thread.iter().find(|m| m.id == "m2").unwrap();
    assert_eq!(m2.mailbox_ids, vec!["archive".to_string()]);
    assert!(m2.flags.contains(Flags::SEEN));
}

#[test]
fn the_outbox_replays_as_email_set_and_empties() {
    let mock = Arc::new(MockTransport::new());
    let (_dir, mut store) = store();
    let r = first_sync(&mock, &mut store);
    let star = Action::SetFlag { message_ids: vec!["m1".into()], flag: "$flagged".into(), on: true };
    store.write(|tx| tx.perform(&star)).unwrap();
    mock.push_json(&format!(
        r#"{{"methodResponses":[["Email/set",{{"accountId":"{ACCOUNT}","oldState":"e1","newState":"e2",
            "updated":{{"m1":null}}}},"c0"]],"sessionState":"s"}}"#
    ));
    assert_eq!(r.replay_outbox(&mut store).unwrap(), 1);
    assert!(store.reader().unwrap().outbox().unwrap().is_empty());
    let sent = mock.requests().last().unwrap().json_body().unwrap();
    let update = &sent["methodCalls"][0][1]["update"]["m1"];
    assert_eq!(update["keywords/$flagged"], serde_json::json!(true));
}

#[test]
fn fetched_bodies_are_stored_and_searchable() {
    let mock = Arc::new(MockTransport::new());
    let (_dir, mut store) = store();
    let r = first_sync(&mock, &mut store);
    mock.push_json(&format!(
        r#"{{"methodResponses":[["Email/get",{{"accountId":"{ACCOUNT}","state":"e1","list":[
            {{"id":"m1","bodyValues":{{"1":{{"value":"Meet at the aquarium"}}}},
              "textBody":[{{"partId":"1","type":"text/plain"}}],"htmlBody":[],"attachments":[]}}],
            "notFound":[]}},"c0"]],"sessionState":"s"}}"#
    ));
    assert_eq!(r.fetch_bodies(&mut store, &["m1".to_string()]).unwrap(), vec!["m1".to_string()]);
    let reader = store.reader().unwrap();
    assert_eq!(reader.body("m1").unwrap().unwrap().text.as_deref(), Some("Meet at the aquarium"));
    assert_eq!(reader.fts_match("aquarium", 5).unwrap().len(), 1);
}
