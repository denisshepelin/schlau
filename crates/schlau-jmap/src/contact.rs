//! JMAP for Contacts (RFC 9610): address books and contact cards, which include contact groups
//! (cards of kind `group` whose `members` are the `uid`s of other cards).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::Client;
use crate::error::{Error, Result};
use crate::method::{ChangesResponse, GetResponse, JmapObject, Query, Set, SetResponse};
use crate::request::RequestBuilder;
use crate::session::capability;
use crate::transport::Transport;
use crate::types::{Id, NoArgs, NoCondition, PatchObject, SetError, State, pointer_segment};

/// An address book (RFC 9610 section 2). Every card belongs to at least one.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AddressBook {
    pub id: Id,
    pub name: Option<String>,
    pub is_default: bool,
}

impl JmapObject for AddressBook {
    const TYPE: &'static str = "AddressBook";
    const USING: &'static [&'static str] = &[capability::CONTACTS];
    type Condition = NoCondition;
    type Create = Value;
    type GetArgs = NoArgs;
    type QueryArgs = NoArgs;
    type SetArgs = NoArgs;
}

/// A JSContact card (RFC 9553) as JMAP serves it, with the properties schlau reads.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContactCard {
    pub id: Id,
    pub uid: Option<String>,
    /// `individual`, `group`, `org`, … Absent means `individual`.
    pub kind: Option<String>,
    pub address_book_ids: Option<HashMap<Id, bool>>,
    pub name: Option<ContactName>,
    pub emails: Option<HashMap<String, ContactEmail>>,
    /// For a group: the `uid`s of its members.
    pub members: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContactName {
    pub full: Option<String>,
    pub components: Option<Vec<NameComponent>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NameComponent {
    pub kind: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContactEmail {
    pub address: String,
}

impl ContactCard {
    pub fn is_group(&self) -> bool {
        self.kind.as_deref() == Some("group")
    }

    /// The name to show: `full`, else the name components joined by spaces.
    pub fn display_name(&self) -> String {
        let Some(name) = &self.name else { return String::new() };
        match &name.full {
            Some(full) if !full.trim().is_empty() => full.trim().to_string(),
            _ => name
                .components
                .iter()
                .flatten()
                .filter(|c| c.kind.as_deref() != Some("separator"))
                .map(|c| c.value.trim())
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    pub fn email_addresses(&self) -> Vec<String> {
        let mut found: Vec<String> =
            self.emails.iter().flatten().map(|(_, e)| e.address.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
        found.sort();
        found.dedup();
        found
    }

    pub fn member_uids(&self) -> Vec<String> {
        let mut found: Vec<String> = self.members.iter().flatten().filter(|(_, on)| **on).map(|(uid, _)| uid.clone()).collect();
        found.sort();
        found
    }
}

impl JmapObject for ContactCard {
    const TYPE: &'static str = "ContactCard";
    const USING: &'static [&'static str] = &[capability::CONTACTS];
    type Condition = NoCondition;
    type Create = Value;
    type GetArgs = NoArgs;
    type QueryArgs = NoArgs;
    type SetArgs = NoArgs;
}

/// A new individual card holding one email address.
pub fn individual_card(uid: &str, name: &str, email: &str, address_book: &Id) -> Value {
    let mut card = json!({
        "@type": "Card",
        "version": "1.0",
        "uid": uid,
        "kind": "individual",
        "addressBookIds": { address_book.as_str(): true },
        "emails": { "e1": { "@type": "EmailAddress", "address": email } },
    });
    if !name.trim().is_empty() {
        card["name"] = json!({ "@type": "Name", "full": name.trim() });
    }
    card
}

/// A new, empty group card.
pub fn group_card(uid: &str, name: &str, address_book: &Id) -> Value {
    json!({
        "@type": "Card",
        "version": "1.0",
        "uid": uid,
        "kind": "group",
        "addressBookIds": { address_book.as_str(): true },
        "name": { "@type": "Name", "full": name },
        "members": {},
    })
}

/// The patch that adds (`on`) or removes a member `uid` from a group.
pub fn member_patch(uid: &str, on: bool) -> PatchObject {
    PatchObject::from_iter([(format!("members/{}", pointer_segment(uid)), if on { Value::Bool(true) } else { Value::Null })])
}

fn rejected(what: &str, key: &str, error: &SetError) -> Error {
    Error::Invalid {
        what: "set",
        detail: format!("{what} {key}: {} {}", error.kind, error.description.as_deref().unwrap_or("")),
    }
}

/// Fails when the server refused any part of a `/set`.
pub fn require_success<O>(resp: SetResponse<O>) -> Result<SetResponse<O>> {
    let failure = resp
        .not_created
        .iter()
        .map(|(k, e)| rejected("not created", k, e))
        .chain(resp.not_updated.iter().map(|(k, e)| rejected("not updated", k.as_str(), e)))
        .chain(resp.not_destroyed.iter().map(|(k, e)| rejected("not destroyed", k.as_str(), e)))
        .next();
    match failure {
        Some(e) => Err(e),
        None => Ok(resp),
    }
}

impl<T: Transport> Client<T> {
    pub fn has_contacts(&self) -> bool {
        self.session().has_capability(capability::CONTACTS)
    }

    pub fn contacts_account_id(&self) -> Result<Id> {
        self.session().primary_account(capability::CONTACTS).cloned()
    }

    pub fn address_books(&self) -> Result<Vec<AddressBook>> {
        Ok(self.get_all_objects::<AddressBook>(&self.contacts_account_id()?, None, NoArgs {})?.list)
    }

    /// The address book new cards go into: the default one, else the first.
    pub fn default_address_book(&self) -> Result<Id> {
        let books = self.address_books()?;
        books
            .iter()
            .find(|b| b.is_default)
            .or_else(|| books.first())
            .map(|b| b.id.clone())
            .ok_or_else(|| Error::Invalid { what: "AddressBook", detail: "the account has no address book".into() })
    }

    /// Every card in the account, paged through `ContactCard/query`. The state is the first page's.
    pub fn all_contact_cards(&self) -> Result<GetResponse<ContactCard>> {
        let account = self.contacts_account_id()?;
        let page = self.session().core().max_objects_in_get.clamp(1, 1000);
        let mut ids: Vec<Id> = Vec::new();
        loop {
            let mut req = RequestBuilder::new();
            let call = req.call(Query::<ContactCard>::new(account.clone()).position(ids.len() as i64).limit(page));
            let resp = self.send(req)?.get(&call)?;
            let got = resp.ids.len();
            ids.extend(resp.ids);
            if got == 0 || got < page as usize || resp.total.is_some_and(|t| ids.len() as u64 >= t) {
                break;
            }
        }
        self.get_objects::<ContactCard>(&account, &ids, None, NoArgs {})
    }

    pub fn contact_card_changes(&self, since: &State) -> Result<(ChangesResponse, Vec<ContactCard>)> {
        let account = self.contacts_account_id()?;
        let changes = self.changes::<ContactCard>(&account, since, None)?;
        let wanted: Vec<Id> = changes.created.iter().chain(&changes.updated).cloned().collect();
        let cards = match wanted.is_empty() {
            true => Vec::new(),
            false => self.get_objects::<ContactCard>(&account, &wanted, None, NoArgs {})?.list,
        };
        Ok((changes, cards))
    }

    /// One `ContactCard/set`; fails when the server refused any part of it.
    pub fn set_contact_cards(&self, set: Set<ContactCard>) -> Result<SetResponse<ContactCard>> {
        let mut req = RequestBuilder::new();
        let call = req.call(set);
        require_success(self.send(req)?.get(&call)?)
    }

    pub fn contact_set(&self) -> Result<Set<ContactCard>> {
        Ok(Set::<ContactCard>::new(self.contacts_account_id()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_groups_and_individuals() {
        let resp: GetResponse<ContactCard> = serde_json::from_str(
            r#"{"accountId":"u1","state":"s1","list":[
                {"id":"c1","uid":"urn:uuid:a","kind":"individual","name":{"full":" Jane Doe "},
                 "emails":{"e1":{"address":"Jane@Example.com"},"e2":{"address":"jane@example.com"}}},
                {"id":"c2","uid":"urn:uuid:g","kind":"group","name":{"components":[{"kind":"given","value":"Screener"},{"kind":"separator","value":" · "},{"kind":"surname","value":"Out"}]},
                 "members":{"urn:uuid:a":true,"urn:uuid:b":false}},
                {"id":"c3","uid":"urn:uuid:c"}
            ],"notFound":null}"#,
        )
        .unwrap();
        let [jane, group, bare] = &resp.list[..] else { panic!("three cards") };
        assert_eq!(jane.display_name(), "Jane Doe");
        assert_eq!(jane.email_addresses(), ["jane@example.com"]);
        assert!(!jane.is_group());
        assert!(group.is_group());
        assert_eq!(group.display_name(), "Screener Out");
        assert_eq!(group.member_uids(), ["urn:uuid:a"]);
        assert_eq!(bare.display_name(), "");
        assert!(bare.email_addresses().is_empty());
    }

    #[test]
    fn member_patches_escape_the_uid() {
        let add = member_patch("urn:uuid:a/b", true);
        assert_eq!(add.get("members/urn:uuid:a~1b"), Some(&Value::Bool(true)));
        assert_eq!(member_patch("x", false).get("members/x"), Some(&Value::Null));
    }

    #[test]
    fn new_cards_carry_uid_book_and_address() {
        let card = individual_card("urn:uuid:n", "", "news@example.com", &Id::from("ab1"));
        assert_eq!(card["addressBookIds"]["ab1"], true);
        assert_eq!(card["emails"]["e1"]["address"], "news@example.com");
        assert!(card.get("name").is_none());
        let group = group_card("urn:uuid:g", "Screener · Out", &Id::from("ab1"));
        assert_eq!(group["kind"], "group");
        assert_eq!(group["name"]["full"], "Screener · Out");
    }

    #[test]
    fn refused_sets_are_errors() {
        let resp: SetResponse<ContactCard> = serde_json::from_str(
            r#"{"accountId":"u1","newState":"s2","notUpdated":{"c2":{"type":"invalidPatch"}}}"#,
        )
        .unwrap();
        assert!(require_success(resp).is_err());
        let ok: SetResponse<ContactCard> =
            serde_json::from_str(r#"{"accountId":"u1","newState":"s2","updated":{"c2":null}}"#).unwrap();
        assert!(require_success(ok).is_ok());
    }
}
