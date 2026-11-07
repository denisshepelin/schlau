use serde_json::{Value, json};

use crate::mock::{MockTransport, RecordedRequest};
use crate::transport::HttpResponse;
use crate::*;

const FASTMAIL_SESSION: &str = include_str!("../fixtures/session_fastmail.json");
const API: &str = "https://api.fastmail.com/jmap/api/";

fn session_with_limits(get: u64, calls: u64, set: u64) -> Session {
    let json = FASTMAIL_SESSION
        .replace(
            "\"maxObjectsInGet\": 4096",
            &format!("\"maxObjectsInGet\": {get}"),
        )
        .replace(
            "\"maxObjectsInSet\": 4096",
            &format!("\"maxObjectsInSet\": {set}"),
        )
        .replace(
            "\"maxCallsInRequest\": 50",
            &format!("\"maxCallsInRequest\": {calls}"),
        );
    Session::from_json(json.as_bytes()).unwrap()
}

fn client(mock: &MockTransport) -> Client<&MockTransport> {
    Client::with_session(
        mock,
        Session::from_json(FASTMAIL_SESSION.as_bytes()).unwrap(),
    )
}

fn response(calls: Value) -> String {
    json!({"methodResponses": calls, "sessionState": "cyrus-12046746;p-5;vfs-0"}).to_string()
}

fn posted(mock: &MockTransport, index: usize) -> Value {
    let req = &mock.requests()[index];
    assert_eq!(req.url(), API);
    req.json_body().unwrap()
}

fn ids(list: &[&str]) -> Vec<Id> {
    list.iter().map(|s| Id::from(*s)).collect()
}

#[test]
fn connects_by_fetching_session() {
    let mock = MockTransport::new();
    mock.push_json(FASTMAIL_SESSION);
    let c = Client::connect(&mock, FASTMAIL_SESSION_URL).unwrap();
    assert_eq!(c.mail_account_id().unwrap().as_str(), "u1a2b3c4d");
    assert_eq!(
        mock.requests(),
        vec![RecordedRequest::Get {
            url: FASTMAIL_SESSION_URL.into()
        }]
    );
}

#[test]
fn refresh_session_reports_state_change() {
    let mock = MockTransport::new();
    mock.push_json(FASTMAIL_SESSION);
    mock.push_json(&FASTMAIL_SESSION.replace("cyrus-12046746", "cyrus-999"));
    let mut c = Client::connect(&mock, FASTMAIL_SESSION_URL).unwrap();
    assert!(c.refresh_session().unwrap());
    assert_eq!(c.session().state, "cyrus-999;p-5;vfs-0");
    let mut detached = client(&mock);
    assert!(matches!(
        detached.refresh_session(),
        Err(Error::Invalid { .. })
    ));
}

#[test]
fn get_emails_chunks_by_max_objects_and_calls() {
    let mock = MockTransport::new();
    let c = Client::with_session(&mock, session_with_limits(2, 2, 10));
    let email = |id: &str| json!({"id": id, "subject": format!("s-{id}")});
    let get = |list: Value, nf: Value, call: &str| json!(["Email/get", {"accountId": "u1a2b3c4d", "state": "J1", "list": list, "notFound": nf}, call]);
    mock.push_json(&response(json!([
        get(json!([email("a"), email("b")]), json!([]), "c0"),
        get(json!([email("c")]), json!(["d"]), "c1"),
    ])));
    mock.push_json(&response(json!([get(
        json!([email("e")]),
        json!([]),
        "c0"
    )])));

    let resp = c
        .get_emails(
            &ids(&["a", "b", "c", "d", "e"]),
            Some(&["id", "subject"]),
            EmailGetArgs::default(),
        )
        .unwrap();
    let got: Vec<&str> = resp.list.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(got, ["a", "b", "c", "e"]);
    assert_eq!(resp.not_found, ids(&["d"]));

    let first = posted(&mock, 0);
    assert_eq!(first["methodCalls"].as_array().unwrap().len(), 2);
    assert_eq!(first["methodCalls"][0][1]["ids"], json!(["a", "b"]));
    assert_eq!(first["methodCalls"][1][1]["ids"], json!(["c", "d"]));
    assert_eq!(
        first["methodCalls"][1][1]["properties"],
        json!(["id", "subject"])
    );
    assert_eq!(posted(&mock, 1)["methodCalls"][0][1]["ids"], json!(["e"]));
}

#[test]
fn query_and_get_is_one_round_trip_with_clamped_limit() {
    let mock = MockTransport::new();
    let c = Client::with_session(&mock, session_with_limits(50, 16, 50));
    mock.push_json(&response(json!([
        ["Email/query", {"accountId": "u1a2b3c4d", "queryState": "q7", "canCalculateChanges": true,
                         "position": 0, "ids": ["M1a"], "total": 1}, "c0"],
        ["Email/get", serde_json::from_str::<Value>(include_str!("../fixtures/email_get_fastmail.json")).unwrap(), "c1"]
    ])));
    let query = Query::<Email>::new(c.mail_account_id().unwrap())
        .filter(EmailFilterCondition::in_mailbox("P-2a"))
        .sort([Comparator::received_at_desc()])
        .limit(500)
        .args(EmailQueryArgs {
            collapse_threads: Some(true),
        });
    let (q, g) = c
        .query_and_get_emails(
            query,
            Some(email_property::HEADERS),
            EmailGetArgs::default(),
        )
        .unwrap();
    assert_eq!(q.query_state.as_str(), "q7");
    assert_eq!(g.list[0].subject.as_deref(), Some("Your October invoice"));

    let body = posted(&mock, 0);
    assert_eq!(
        body["using"],
        json!(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"])
    );
    assert_eq!(body["methodCalls"][0][1]["limit"], 50);
    assert_eq!(body["methodCalls"][0][1]["collapseThreads"], true);
    assert_eq!(
        body["methodCalls"][1][1]["#ids"],
        json!({"resultOf": "c0", "name": "Email/query", "path": "/ids"})
    );
    assert_eq!(mock.requests().len(), 1);
}

#[test]
fn email_changes_and_get_fetches_created_and_updated() {
    let mock = MockTransport::new();
    let c = client(&mock);
    mock.push_json(&response(json!([
        ["Email/changes", {"accountId": "u1a2b3c4d", "oldState": "J1", "newState": "J2",
                           "hasMoreChanges": false, "created": ["M3"], "updated": ["M1a"], "destroyed": ["M0"]}, "c0"],
        ["Email/get", {"accountId": "u1a2b3c4d", "state": "J2", "list": [{"id": "M3", "subject": "new"}], "notFound": []}, "c1"],
        ["Email/get", {"accountId": "u1a2b3c4d", "state": "J2",
                       "list": [{"id": "M1a", "mailboxIds": {"P-2a": true}, "keywords": {"$seen": true}}], "notFound": []}, "c2"]
    ])));
    let delta = c
        .email_changes_and_get(
            &State::from("J1"),
            Some(10_000),
            email_property::HEADERS,
            EmailGetArgs::default(),
        )
        .unwrap();
    assert_eq!(delta.changes.destroyed, ids(&["M0"]));
    assert_eq!(delta.created[0].subject.as_deref(), Some("new"));
    assert!(!delta.updated[0].is_unread());

    let body = posted(&mock, 0);
    assert_eq!(body["methodCalls"][0][1]["maxChanges"], 4096);
    assert_eq!(body["methodCalls"][1][1]["#ids"]["path"], "/created");
    assert_eq!(body["methodCalls"][2][1]["#ids"]["path"], "/updated");
    assert_eq!(
        body["methodCalls"][2][1]["properties"],
        json!(["id", "mailboxIds", "keywords"])
    );
}

#[test]
fn cannot_calculate_changes_surfaces_from_changes_call() {
    let mock = MockTransport::new();
    let c = client(&mock);
    mock.push_json(&response(json!([
        ["error", {"type": "cannotCalculateChanges"}, "c0"],
        ["error", {"type": "invalidResultReference"}, "c1"],
        ["error", {"type": "invalidResultReference"}, "c2"]
    ])));
    let err = c
        .email_changes_and_get(
            &State::from("old"),
            None,
            email_property::HEADERS,
            EmailGetArgs::default(),
        )
        .unwrap_err();
    assert!(err.is_cannot_calculate_changes(), "{err}");
}

#[test]
fn mailbox_changes_uses_updated_properties_reference() {
    let mock = MockTransport::new();
    let c = client(&mock);
    mock.push_json(&response(json!([
        ["Mailbox/changes", {"accountId": "u1a2b3c4d", "oldState": "78541", "newState": "78542",
            "hasMoreChanges": false, "updatedProperties": ["totalEmails", "unreadEmails", "totalThreads", "unreadThreads"],
            "created": [], "updated": ["MB23cfa8094c0f41e6"], "destroyed": []}, "c0"],
        ["Mailbox/get", {"accountId": "u1a2b3c4d", "state": "78542", "list": [], "notFound": []}, "c1"],
        ["Mailbox/get", {"accountId": "u1a2b3c4d", "state": "78542", "list": [
            {"id": "MB23cfa8094c0f41e6", "totalEmails": 16307, "unreadEmails": 13903, "totalThreads": 5833, "unreadThreads": 5127}
        ], "notFound": []}, "c2"]
    ])));
    let changes = c.mailbox_changes(&State::from("78541"), None).unwrap();
    assert!(changes.changes.updated_properties.is_some());
    assert_eq!(changes.updated[0].unread_emails, Some(13903));
    assert_eq!(changes.updated[0].name, None);

    let body = posted(&mock, 0);
    assert_eq!(body["methodCalls"][1][1]["properties"], Value::Null);
    assert_eq!(
        body["methodCalls"][2][1]["#properties"],
        json!({"resultOf": "c0", "name": "Mailbox/changes", "path": "/updatedProperties"})
    );
}

#[test]
fn mark_read_chunks_by_max_objects_in_set_and_reports_failures() {
    let mock = MockTransport::new();
    let c = Client::with_session(&mock, session_with_limits(100, 16, 2));
    mock.push_json(&response(json!([
        ["Email/set", {"accountId": "u1a2b3c4d", "oldState": "J1", "newState": "J2",
                       "updated": {"a": null}, "notUpdated": {"b": {"type": "notFound"}}}, "c0"]
    ])));
    mock.push_json(&response(json!([
        ["Email/set", {"accountId": "u1a2b3c4d", "oldState": "J2", "newState": "J3",
                       "updated": {"c": null}, "created": null, "notCreated": null}, "c0"]
    ])));
    let resp = c.mark_read(&ids(&["a", "b", "c"]), true).unwrap();
    assert_eq!(resp.new_state.as_str(), "J3");
    assert_eq!(resp.updated.len(), 2);
    assert_eq!(
        resp.not_updated[&Id::from("b")].kind,
        SetErrorKind::NotFound
    );
    assert!(!resp.is_success());

    let first = posted(&mock, 0);
    assert_eq!(
        first["methodCalls"][0][1]["update"],
        json!({"a": {"keywords/$seen": true}, "b": {"keywords/$seen": true}})
    );
    assert_eq!(
        posted(&mock, 1)["methodCalls"][0][1]["update"],
        json!({"c": {"keywords/$seen": true}})
    );
}

#[test]
fn move_flag_and_destroy_patches() {
    let mock = MockTransport::new();
    let c = client(&mock);
    let ok = response(json!([["Email/set", {"accountId": "u1a2b3c4d", "newState": "J9"}, "c0"]]));
    (0..3).for_each(|_| {
        mock.push_json(&ok);
    });
    c.move_emails(&ids(&["a"]), &Id::from("inbox"), &Id::from("archive"))
        .unwrap();
    c.flag(&ids(&["a"]), false).unwrap();
    c.destroy_emails(&ids(&["a", "b"])).unwrap();
    assert_eq!(
        posted(&mock, 0)["methodCalls"][0][1]["update"]["a"],
        json!({"mailboxIds/inbox": null, "mailboxIds/archive": true})
    );
    assert_eq!(
        posted(&mock, 1)["methodCalls"][0][1]["update"]["a"],
        json!({"keywords/$flagged": null})
    );
    assert_eq!(
        posted(&mock, 2)["methodCalls"][0][1]["destroy"],
        json!(["a", "b"])
    );
    assert!(matches!(c.mark_read(&[], true), Err(Error::Invalid { .. })));
}

#[test]
fn create_draft_returns_id_or_set_error() {
    let mock = MockTransport::new();
    let c = client(&mock);
    mock.push_json(&response(json!([["Email/set", {"accountId": "u1a2b3c4d", "newState": "J2",
        "created": {"draft": {"id": "M77", "blobId": "G77", "threadId": "T77", "size": 512}}}, "c0"]])));
    mock.push_json(&response(json!([["Email/set", {"accountId": "u1a2b3c4d", "newState": "J2",
        "notCreated": {"draft": {"type": "invalidProperties", "properties": ["mailboxIds"]}}}, "c0"]])));
    let draft = || {
        EmailCreate::draft(
            "P-drafts",
            EmailAddress::new(None, "jane@fastmail.com"),
            "Hi",
            "Body",
        )
    };
    assert_eq!(c.create_draft(draft()).unwrap().as_str(), "M77");
    let err = c.create_draft(draft()).unwrap_err();
    assert!(err.to_string().contains("invalidProperties"), "{err}");
    assert_eq!(
        posted(&mock, 0)["methodCalls"][0][1]["create"]["draft"]["mailboxIds"],
        json!({"P-drafts": true})
    );
}

#[test]
fn send_draft_uses_on_success_update_email() {
    let mock = MockTransport::new();
    let c = client(&mock);
    mock.push_json(&response(json!([
        ["EmailSubmission/set", {"accountId": "u1a2b3c4d", "newState": "S2",
            "created": {"send": {"id": "ES1", "undoStatus": "final", "sendAt": "2026-11-07T10:00:00Z"}}}, "c0"],
        ["Email/set", {"accountId": "u1a2b3c4d", "oldState": "J1", "newState": "J2", "updated": {"M77": null}}, "c0"]
    ])));
    let boxes = SendMailboxes {
        drafts: Id::from("P-drafts"),
        sent: Id::from("P-sent"),
    };
    let sent = c
        .send_draft(&Id::from("M77"), &Id::from("I1"), &boxes, None)
        .unwrap();
    assert_eq!(sent.submission_id.as_str(), "ES1");
    assert_eq!(sent.email_update.unwrap().new_state.as_str(), "J2");

    let body = posted(&mock, 0);
    assert_eq!(
        body["using"],
        json!([
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:mail",
            "urn:ietf:params:jmap:submission"
        ])
    );
    let args = &body["methodCalls"][0][1];
    assert_eq!(
        args["create"]["send"],
        json!({"identityId": "I1", "emailId": "M77"})
    );
    assert_eq!(
        args["onSuccessUpdateEmail"],
        json!({"#send": {"mailboxIds/P-drafts": null, "mailboxIds/P-sent": true, "keywords/$draft": null}})
    );
}

#[test]
fn send_new_email_creates_and_submits_in_one_request() {
    let mock = MockTransport::new();
    let c = client(&mock);
    mock.push_json(&response(json!([
        ["Email/set", {"accountId": "u1a2b3c4d", "newState": "J2", "created": {"draft": {"id": "M8"}}}, "c0"],
        ["EmailSubmission/set", {"accountId": "u1a2b3c4d", "newState": "S2", "notCreated":
            {"send": {"type": "forbiddenFrom", "description": "not your address"}}}, "c1"]
    ])));
    let boxes = SendMailboxes {
        drafts: Id::from("P-drafts"),
        sent: Id::from("P-sent"),
    };
    let draft = EmailCreate::draft(
        "P-drafts",
        EmailAddress::new(None, "x@evil.example"),
        "Hi",
        "Body",
    );
    let err = c
        .send_new_email(draft, &Id::from("I1"), &boxes, None)
        .unwrap_err();
    assert!(err.to_string().contains("forbiddenFrom"));
    assert_eq!(
        posted(&mock, 0)["methodCalls"][1][1]["create"]["send"]["emailId"],
        "#draft"
    );
}

#[test]
fn identities_and_threads() {
    let mock = MockTransport::new();
    let c = client(&mock);
    mock.push_json(&response(json!([["Identity/get", {"accountId": "u1a2b3c4d", "state": "1",
        "list": [{"id": "I1", "name": "Jane Doe", "email": "jane@fastmail.com", "mayDelete": false}], "notFound": []}, "c0"]])));
    mock.push_json(&response(
        json!([["Thread/get", {"accountId": "u1a2b3c4d", "state": "T9",
        "list": [{"id": "T5f", "emailIds": ["M1a", "M2b"]}], "notFound": []}, "c0"]]),
    ));
    assert_eq!(
        c.identities().unwrap()[0].address().email,
        "jane@fastmail.com"
    );
    assert_eq!(
        c.threads(&ids(&["T5f"])).unwrap().list[0].email_ids.len(),
        2
    );
    assert_eq!(
        posted(&mock, 0)["using"][1],
        "urn:ietf:params:jmap:submission"
    );
    assert_eq!(posted(&mock, 0)["methodCalls"][0][1]["ids"], Value::Null);
}

#[test]
fn request_level_errors_are_typed() {
    let mock = MockTransport::new();
    let c = client(&mock);
    mock.push(HttpResponse {
        status: 400,
        content_type: Some("application/problem+json".into()),
        body: br#"{"type":"urn:ietf:params:jmap:error:limit","limit":"maxSizeRequest","status":400,"detail":"Request too large"}"#.to_vec(),
    });
    mock.push(HttpResponse {
        status: 503,
        content_type: Some("text/html".into()),
        body: b"<h1>down</h1>".to_vec(),
    });
    let err = c.mailboxes().unwrap_err();
    match &err {
        Error::Problem(p) => {
            assert_eq!(p.kind, ProblemKind::Limit);
            assert_eq!(p.limit.as_deref(), Some("maxSizeRequest"));
        }
        other => panic!("unexpected {other:?}"),
    }
    let err = c.mailboxes().unwrap_err();
    assert!(matches!(err, Error::Http { status: 503, .. }) && err.is_retryable());
}

#[test]
fn local_limits_are_checked_before_sending() {
    let mock = MockTransport::new();
    let c = Client::with_session(&mock, session_with_limits(10, 1, 10));
    let mut req = RequestBuilder::new();
    req.call(Get::<Mailbox>::new("u1a2b3c4d", IdsArg::all()));
    req.call(Get::<Thread>::new("u1a2b3c4d", IdsArg::all()));
    assert!(matches!(
        c.send(req),
        Err(Error::LocalLimit {
            limit: "maxCallsInRequest",
            actual: 2,
            max: 1
        })
    ));
    assert!(mock.requests().is_empty());
}

#[test]
fn download_and_upload_blobs() {
    let mock = MockTransport::new();
    let c = client(&mock);
    mock.push(HttpResponse {
        status: 200,
        content_type: Some("application/pdf".into()),
        body: b"%PDF-1.7".to_vec(),
    });
    mock.push_json(r#"{"accountId":"u1a2b3c4d","blobId":"Gnew","type":"image/png","size":4}"#);
    let account = c.mail_account_id().unwrap();
    let bytes = c
        .download(
            &account,
            &Id::from("G7f3e"),
            "application/pdf",
            "invoice 2026.pdf",
        )
        .unwrap();
    assert_eq!(bytes, b"%PDF-1.7");
    let up = c.upload(&account, "image/png", b"\x89PNG").unwrap();
    assert_eq!(up.blob_id.as_str(), "Gnew");
    let reqs = mock.requests();
    assert_eq!(
        reqs[0].url(),
        "https://www.fastmailusercontent.com/jmap/download/u1a2b3c4d/G7f3e/invoice%202026.pdf?type=application%2Fpdf"
    );
    assert!(
        matches!(&reqs[1], RecordedRequest::Post { url, content_type, .. }
        if url == "https://api.fastmail.com/jmap/upload/u1a2b3c4d/" && content_type == "image/png")
    );
}

#[test]
fn event_source_opens_stream_and_reports_http_errors() {
    let mock = MockTransport::new();
    let c = client(&mock);
    mock.push_stream([
        "event: state\ndata: {\"@type\":\"StateChange\",\"changed\":{\"u1a2b3c4d\":{\"Email\":\"J5\",\"Mailbox\":\"M5\"}}}\n",
        "id: abc\n\n",
    ]);
    mock.push(HttpResponse {
        status: 401,
        content_type: None,
        body: b"unauthorized".to_vec(),
    });
    let mut stream = c.event_source("*", 30, None).unwrap();
    let Some(Ok(PushEvent::StateChange(sc))) = stream.next() else {
        panic!("expected a state change")
    };
    let account = c.mail_account_id().unwrap();
    assert_eq!(sc.state_of(&account, "Email").unwrap().as_str(), "J5");
    assert!(stream.next().is_none());
    let last = stream.last_event_id().map(str::to_owned);

    let err = c.event_source("*", 30, last.as_deref()).err().unwrap();
    assert!(matches!(err, Error::Http { status: 401, .. }));
    assert_eq!(
        mock.requests()[1],
        RecordedRequest::Stream {
            url: "https://api.fastmail.com/jmap/event/?types=*&closeafter=no&ping=30".into(),
            last_event_id: Some("abc".into()),
        }
    );
}
