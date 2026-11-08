//! The screener against canned JMAP responses: contacts sync into the replica, and a decision
//! replays as one `ContactCard/set` that makes the sender's card and adds it to the group.

use std::sync::Arc;

use schlau_jmap::mock::MockTransport;
use schlau_jmap::{Client, Session, Transport};
use schlau_store::{Action, Store};
use schlau_sync::{CONTACTS_AVAILABLE, Remote, SetupRequest};

const SESSION: &str = include_str!("../../schlau-jmap/fixtures/session_fastmail.json");
const ACCOUNT: &str = "u1a2b3c4d";

fn session(contacts: bool) -> Session {
    let json = match contacts {
        false => SESSION.to_string(),
        true => SESSION
            .replace(
                r#""urn:ietf:params:jmap:mail": {},"#,
                r#""urn:ietf:params:jmap:mail": {}, "urn:ietf:params:jmap:contacts": {},"#,
            )
            .replace(
                r#""urn:ietf:params:jmap:core": "u1a2b3c4d","#,
                r#""urn:ietf:params:jmap:core": "u1a2b3c4d", "urn:ietf:params:jmap:contacts": "u1a2b3c4d","#,
            ),
    };
    Session::from_json(json.as_bytes()).unwrap()
}

fn remote(mock: &Arc<MockTransport>, contacts: bool) -> Remote {
    let transport: Arc<dyn Transport> = mock.clone();
    Remote::new(Client::with_session(transport, session(contacts))).unwrap()
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("mail.sqlite")).unwrap();
    (dir, store)
}

fn respond(mock: &MockTransport, name: &str, args: &str) {
    mock.push_json(&format!(r#"{{"methodResponses":[["{name}",{args},"c0"]],"sessionState":"s"}}"#));
}

const GROUP: &str = r#"{"id":"g1","uid":"urn:uuid:g1","kind":"group","name":{"full":"Screener · News"},"members":{}}"#;

/// The first contacts sync: the default address book, then every card.
fn first_contacts_sync(mock: &Arc<MockTransport>, store: &mut Store) -> Remote {
    let r = remote(mock, true);
    respond(mock, "AddressBook/get", &format!(r#"{{"accountId":"{ACCOUNT}","state":"a1","list":[{{"id":"ab1","name":"Personal","isDefault":true}}],"notFound":[]}}"#));
    respond(mock, "ContactCard/query", &format!(r#"{{"accountId":"{ACCOUNT}","queryState":"q","canCalculateChanges":false,"position":0,"ids":["g1"],"total":1}}"#));
    respond(mock, "ContactCard/get", &format!(r#"{{"accountId":"{ACCOUNT}","state":"c1","list":[{GROUP}],"notFound":[]}}"#));
    assert!(r.sync_contacts(store).unwrap());
    r
}

#[test]
fn without_contacts_access_the_replica_says_so() {
    let mock = Arc::new(MockTransport::new());
    let (_dir, mut store) = store();
    assert!(!remote(&mock, false).sync_contacts(&mut store).unwrap());
    assert_eq!(store.reader().unwrap().meta(CONTACTS_AVAILABLE).unwrap().as_deref(), Some("0"));
    assert!(mock.requests().is_empty());
    let err = remote(&mock, false)
        .setup_screener(&mut store, &SetupRequest { triage: None, groups: vec!["Screener · Out".into()] })
        .unwrap_err();
    assert!(err.to_string().contains("contacts"), "{err}");
}

#[test]
fn a_decision_makes_the_card_and_adds_it_to_the_group_in_one_set() {
    let mock = Arc::new(MockTransport::new());
    let (_dir, mut store) = store();
    let r = first_contacts_sync(&mock, &mut store);
    let reader = store.reader().unwrap();
    assert_eq!(reader.meta(CONTACTS_AVAILABLE).unwrap().as_deref(), Some("1"));
    assert_eq!(reader.contact_groups().unwrap()[0].name, "Screener · News");

    let add = Action::AddToGroup { group_id: "g1".into(), email: "news@shop.example".into(), name: "Shop".into(), uid: "urn:uuid:n".into() };
    store.write(|tx| tx.perform(&add)).unwrap();
    assert_eq!(store.reader().unwrap().groups_of("news@shop.example").unwrap(), ["g1"], "applied locally at once");

    respond(&mock, "ContactCard/set", &format!(r#"{{"accountId":"{ACCOUNT}","oldState":"c1","newState":"c2","created":{{"card":{{"id":"p1"}}}},"updated":{{"g1":null}}}}"#));
    respond(&mock, "ContactCard/changes", &format!(r#"{{"accountId":"{ACCOUNT}","oldState":"c1","newState":"c2","hasMoreChanges":false,"created":["p1"],"updated":["g1"],"destroyed":[]}}"#));
    respond(&mock, "ContactCard/get", &format!(
        r#"{{"accountId":"{ACCOUNT}","state":"c2","list":[
            {{"id":"p1","uid":"urn:uuid:n","kind":"individual","emails":{{"e1":{{"address":"news@shop.example"}}}}}},
            {{"id":"g1","uid":"urn:uuid:g1","kind":"group","name":{{"full":"Screener · News"}},"members":{{"urn:uuid:n":true}}}}],"notFound":[]}}"#
    ));
    assert_eq!(r.replay_outbox(&mut store).unwrap(), 1);

    let set = mock.requests()[3].json_body().unwrap();
    let args = &set["methodCalls"][0][1];
    assert_eq!(set["methodCalls"][0][0], "ContactCard/set");
    assert_eq!(set["using"].as_array().unwrap().iter().filter(|u| *u == "urn:ietf:params:jmap:contacts").count(), 1);
    assert_eq!(args["create"]["card"]["uid"], "urn:uuid:n");
    assert_eq!(args["create"]["card"]["addressBookIds"]["ab1"], true);
    assert_eq!(args["update"]["g1"]["members/urn:uuid:n"], true);

    let reader = store.reader().unwrap();
    assert!(reader.outbox().unwrap().is_empty());
    assert_eq!(reader.card_for_email("news@shop.example").unwrap(), Some(("urn:uuid:n".into(), false)), "the server's card replaced the pending one");
    assert_eq!(reader.groups_of("news@shop.example").unwrap(), ["g1"]);
}

#[test]
fn undoing_a_decision_removes_the_member_and_deletes_the_card_it_made() {
    let mock = Arc::new(MockTransport::new());
    let (_dir, mut store) = store();
    let r = first_contacts_sync(&mock, &mut store);
    store
        .write(|tx| {
            tx.upsert_contact(&schlau_store::ContactRecord {
                id: "p1".into(),
                uid: "urn:uuid:n".into(),
                kind: "individual".into(),
                emails: vec!["news@shop.example".into()],
                ..Default::default()
            })?;
            tx.apply_locally(&Action::AddToGroup { group_id: "g1".into(), email: "news@shop.example".into(), name: String::new(), uid: "urn:uuid:n".into() })
        })
        .unwrap();
    let remove = Action::RemoveFromGroup {
        group_id: "g1".into(),
        email: "news@shop.example".into(),
        name: String::new(),
        uid: "urn:uuid:n".into(),
        delete_card: true,
    };
    store.write(|tx| tx.perform(&remove)).unwrap();
    respond(&mock, "ContactCard/set", &format!(r#"{{"accountId":"{ACCOUNT}","oldState":"c1","newState":"c3","updated":{{"g1":null}},"destroyed":["p1"]}}"#));
    respond(&mock, "ContactCard/changes", &format!(r#"{{"accountId":"{ACCOUNT}","oldState":"c1","newState":"c3","hasMoreChanges":false,"created":[],"updated":[],"destroyed":["p1"]}}"#));
    assert_eq!(r.replay_outbox(&mut store).unwrap(), 1);
    let args = &mock.requests()[3].json_body().unwrap()["methodCalls"][0][1];
    assert_eq!(args["update"]["g1"]["members/urn:uuid:n"], serde_json::Value::Null);
    assert_eq!(args["destroy"], serde_json::json!(["p1"]));
    assert_eq!(store.reader().unwrap().card_for_email("news@shop.example").unwrap(), None);
}

#[test]
fn setup_makes_the_missing_folder_and_groups() {
    let mock = Arc::new(MockTransport::new());
    let (_dir, mut store) = store();
    let r = first_contacts_sync(&mock, &mut store);
    respond(&mock, "Mailbox/set", &format!(r#"{{"accountId":"{ACCOUNT}","newState":"m2","created":{{"new":{{"id":"mb-t"}}}}}}"#));
    respond(&mock, "Mailbox/get", &format!(r#"{{"accountId":"{ACCOUNT}","state":"m2","list":[{{"id":"mb-t","name":"Triage"}}],"notFound":[]}}"#));
    respond(&mock, "ContactCard/changes", &format!(r#"{{"accountId":"{ACCOUNT}","oldState":"c1","newState":"c1","hasMoreChanges":false,"created":[],"updated":[],"destroyed":[]}}"#));
    respond(&mock, "ContactCard/set", &format!(r#"{{"accountId":"{ACCOUNT}","newState":"c2","created":{{"g0":{{"id":"g2"}}}}}}"#));
    respond(&mock, "ContactCard/changes", &format!(r#"{{"accountId":"{ACCOUNT}","oldState":"c1","newState":"c2","hasMoreChanges":false,"created":["g2"],"updated":[],"destroyed":[]}}"#));
    respond(&mock, "ContactCard/get", &format!(r#"{{"accountId":"{ACCOUNT}","state":"c2","list":[{{"id":"g2","uid":"urn:uuid:g2","kind":"group","name":{{"full":"Screener · Out"}}}}],"notFound":[]}}"#));
    let request = SetupRequest { triage: Some("Triage".into()), groups: vec!["Screener · News".into(), "Screener · Out".into()] };
    r.setup_screener(&mut store, &request).unwrap();
    let reader = store.reader().unwrap();
    assert!(reader.mailboxes().unwrap().iter().any(|m| m.name == "Triage"));
    assert_eq!(reader.contact_groups().unwrap().len(), 2);
    let create = &mock.requests()[3].json_body().unwrap()["methodCalls"][0][1]["create"];
    assert_eq!(create["new"]["name"], "Triage");
    let groups = &mock.requests()[6].json_body().unwrap()["methodCalls"][0][1]["create"];
    assert_eq!(groups.as_object().unwrap().len(), 1, "only the missing group: {groups}");
    assert_eq!(groups["g0"]["name"]["full"], "Screener · Out");
    assert_eq!(groups["g0"]["kind"], "group");
}
