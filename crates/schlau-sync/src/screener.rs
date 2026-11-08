//! The screener's server side. Decisions live in Fastmail contact groups, which rules the user made
//! once in Fastmail route on. The engine keeps the groups in the replica, notes where new mail from
//! their members lands, and moves what a missing rule left behind.

use std::collections::BTreeMap;

use schlau_store::{Action, Reader, Store};
use serde::{Deserialize, Serialize};

/// What a destination does with a sender's mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DestinationKind {
    /// Let in: the Inbox.
    Inbox,
    /// File in a folder.
    File,
    /// Send to hell: spam.
    Out,
}

/// A place a screened sender's mail goes, and the contact group that says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    pub kind: DestinationKind,
    pub mailbox_id: String,
    /// The contact group's name in Fastmail, such as `Screener · Newsletters`.
    pub group: String,
}

/// The screener as the user set it up: the folder new senders land in, and the destinations.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScreenerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub destinations: Vec<Destination>,
}

/// The name of the contact group for a destination folder.
pub fn group_name(folder: &str) -> String {
    format!("Screener · {folder}")
}

/// What `Command::SetupScreener` asks the engine to make when missing: the screened folder, by
/// name at the top level, and contact groups by name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SetupRequest {
    pub triage: Option<String>,
    pub groups: Vec<String>,
}

impl SetupRequest {
    /// One request for both: a later folder name wins, groups are joined.
    pub fn merge(self, later: SetupRequest) -> SetupRequest {
        let groups = self.groups.into_iter().chain(later.groups).fold(Vec::new(), |mut acc, g| {
            if !acc.contains(&g) {
                acc.push(g);
            }
            acc
        });
        SetupRequest { triage: later.triage.or(self.triage), groups }
    }
}

/// A destination whose group is in the replica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub group_id: String,
    pub mailbox_id: String,
    pub kind: DestinationKind,
}

impl ScreenerConfig {
    pub fn is_active(&self) -> bool {
        self.triage.is_some() && !self.destinations.is_empty()
    }

    /// The destinations whose groups the replica has, in order.
    pub fn routes(&self, reader: &Reader) -> schlau_store::Result<Vec<Route>> {
        let groups = reader.contact_groups()?;
        Ok(self
            .destinations
            .iter()
            .filter_map(|d| {
                groups.iter().find(|g| g.name == d.group).map(|g| Route {
                    group_id: g.id.clone(),
                    mailbox_id: d.mailbox_id.clone(),
                    kind: d.kind,
                })
            })
            .collect())
    }
}

/// The actions that put `message_ids` from `from` where `route` sends them.
pub fn route_actions(message_ids: Vec<String>, from: &str, route: &Route) -> Vec<Action> {
    let moved = Action::Move { message_ids: message_ids.clone(), from: Some(from.to_string()), to: route.mailbox_id.clone() };
    match route.kind {
        DestinationKind::Out => vec![moved, Action::SetFlag { message_ids, flag: "$junk".into(), on: true }],
        DestinationKind::Inbox | DestinationKind::File => vec![moved],
    }
}

/// Notes where new mail from group members landed, and moves what landed in the screened folder or
/// the Inbox instead of its group's folder, then anything else in the screened folder that a group
/// claims. Returns how many messages it moved.
pub fn after_sync(store: &mut Store, config: &ScreenerConfig, created: &[String]) -> schlau_store::Result<usize> {
    let Some(triage) = config.triage.clone().filter(|_| config.is_active()) else { return Ok(0) };
    let reader = store.reader()?;
    let routes = config.routes(&reader)?;
    if routes.is_empty() {
        return Ok(0);
    }
    let group_ids: Vec<String> = routes.iter().map(|r| r.group_id.clone()).collect();
    let inbox = reader.mailbox_by_role("inbox")?.map(|m| m.id);
    let arrivals: Vec<(String, Route, Vec<String>)> = created
        .iter()
        .filter_map(|id| {
            let group = reader.group_of_message(id, &group_ids).ok().flatten()?;
            let route = routes.iter().find(|r| r.group_id == group)?.clone();
            let boxes = reader.message_mailboxes(id).ok()?;
            Some((id.clone(), route, boxes))
        })
        .collect();
    let mut moves: BTreeMap<String, (String, Route)> = arrivals
        .iter()
        .filter(|(_, route, boxes)| !boxes.contains(&route.mailbox_id))
        .filter_map(|(id, route, boxes)| {
            let from = boxes.iter().find(|b| **b == triage || Some(*b) == inbox.as_ref())?;
            Some((id.clone(), (from.clone(), route.clone())))
        })
        .collect();
    let targets: Vec<(String, String)> = routes.iter().map(|r| (r.group_id.clone(), r.mailbox_id.clone())).collect();
    reader.misrouted(std::slice::from_ref(&triage), &targets)?.into_iter().for_each(|m| {
        if let Some(route) = routes.iter().find(|r| r.group_id == m.group_id) {
            moves.entry(m.message_id).or_insert((m.mailbox_id, route.clone()));
        }
    });
    let batches: BTreeMap<(String, String), (Route, Vec<String>)> =
        moves.into_iter().fold(BTreeMap::new(), |mut acc, (id, (from, route))| {
            acc.entry((from, route.group_id.clone())).or_insert_with(|| (route, Vec::new())).1.push(id);
            acc
        });
    let moved = batches.values().map(|(_, ids)| ids.len()).sum();
    store.write(|tx| {
        arrivals
            .iter()
            .try_for_each(|(id, route, boxes)| tx.record_screener_event(id, &route.group_id, boxes.contains(&route.mailbox_id)))?;
        batches
            .iter()
            .flat_map(|((from, _), (route, ids))| route_actions(ids.clone(), from, route))
            .try_for_each(|action| tx.perform(&action).map(|_| ()))
    })?;
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spam_also_teaches_the_spam_filter() {
        let route = Route { group_id: "g".into(), mailbox_id: "junk".into(), kind: DestinationKind::Out };
        let actions = route_actions(vec!["m1".into()], "triage", &route);
        assert_eq!(actions.len(), 2);
        assert!(matches!(&actions[1], Action::SetFlag { flag, on: true, .. } if flag == "$junk"));
        let file = Route { kind: DestinationKind::File, ..route };
        assert_eq!(route_actions(vec!["m1".into()], "triage", &file).len(), 1);
    }

    fn mailbox(id: &str, role: Option<&str>) -> schlau_store::Mailbox {
        schlau_store::Mailbox {
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

    fn message(id: &str, from: &str, mailbox: &str) -> schlau_store::MessageRecord {
        schlau_store::MessageRecord {
            id: id.into(),
            thread_id: format!("t-{id}"),
            received_at: 100,
            addresses: schlau_store::Addresses {
                from: vec![schlau_store::Address { name: String::new(), email: from.into() }],
                ..Default::default()
            },
            mailbox_ids: vec![mailbox.into()],
            ..Default::default()
        }
    }

    #[test]
    fn new_mail_a_rule_missed_is_moved_and_noted() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("mail.sqlite")).unwrap();
        store
            .write(|tx| {
                ["inbox", "triage", "news", "junk"].iter().try_for_each(|b| {
                    let role = match *b {
                        "inbox" | "junk" => Some(*b),
                        _ => None,
                    };
                    tx.upsert_mailbox(&mailbox(b, role))
                })?;
                let group = |id: &str, name: &str, members: &[&str]| schlau_store::ContactRecord {
                    id: id.into(),
                    uid: id.into(),
                    kind: "group".into(),
                    name: name.into(),
                    members: members.iter().map(|m| m.to_string()).collect(),
                    ..Default::default()
                };
                let person = |id: &str, email: &str| schlau_store::ContactRecord {
                    id: id.into(),
                    uid: id.into(),
                    kind: "individual".into(),
                    emails: vec![email.into()],
                    ..Default::default()
                };
                tx.upsert_contact(&group("g-news", &group_name("News"), &["p-news"]))?;
                tx.upsert_contact(&group("g-out", &group_name("Out"), &["p-spam"]))?;
                tx.upsert_contact(&person("p-news", "news@shop.example"))?;
                tx.upsert_contact(&person("p-spam", "deals@spam.example"))?;
                tx.upsert_message(&message("routed", "news@shop.example", "news"))?;
                tx.upsert_message(&message("missed", "News@shop.example", "inbox"))?;
                tx.upsert_message(&message("old-inbox", "news@shop.example", "inbox"))?;
                tx.upsert_message(&message("left", "deals@spam.example", "triage"))?;
                tx.upsert_message(&message("stranger", "who@else.example", "triage")).map(|_| ())
            })
            .unwrap();
        let config = ScreenerConfig {
            triage: Some("triage".into()),
            destinations: vec![
                Destination { kind: DestinationKind::File, mailbox_id: "news".into(), group: group_name("News") },
                Destination { kind: DestinationKind::Out, mailbox_id: "junk".into(), group: group_name("Out") },
            ],
        };
        let moved = after_sync(&mut store, &config, &["routed".into(), "missed".into()]).unwrap();
        assert_eq!(moved, 2, "the new inbox message and what waits in triage; older inbox mail stays");
        let reader = store.reader().unwrap();
        assert_eq!(reader.message_mailboxes("missed").unwrap(), ["news"]);
        assert_eq!(reader.message_mailboxes("left").unwrap(), ["junk"]);
        assert_eq!(reader.message_mailboxes("old-inbox").unwrap(), ["inbox"]);
        assert_eq!(reader.message_mailboxes("stranger").unwrap(), ["triage"]);
        let news = reader.routing_stats("g-news").unwrap();
        assert_eq!((news.routed, news.missed), (1, 1));
        let queued: Vec<_> = reader.outbox().unwrap().into_iter().map(|i| i.action).collect();
        assert!(queued.iter().any(|a| matches!(a, Action::SetFlag { flag, .. } if flag == "$junk")));
        assert_eq!(after_sync(&mut store, &ScreenerConfig::default(), &[]).unwrap(), 0, "off until set up");
    }

    #[test]
    fn setup_requests_merge_instead_of_replacing() {
        let first = SetupRequest { triage: Some("Triage".into()), groups: vec!["A".into()] };
        let merged = first.merge(SetupRequest { triage: None, groups: vec!["A".into(), "B".into()] });
        assert_eq!(merged, SetupRequest { triage: Some("Triage".into()), groups: vec!["A".into(), "B".into()] });
    }

    #[test]
    fn the_config_round_trips_through_json() {
        let config = ScreenerConfig {
            triage: Some("mb-triage".into()),
            destinations: vec![Destination { kind: DestinationKind::Out, mailbox_id: "junk".into(), group: group_name("Out") }],
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"kind\":\"out\""), "{json}");
        assert_eq!(serde_json::from_str::<ScreenerConfig>(&json).unwrap(), config);
        assert_eq!(serde_json::from_str::<ScreenerConfig>("{}").unwrap(), ScreenerConfig::default());
    }
}
