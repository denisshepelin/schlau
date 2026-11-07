use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::email::EmailAddress;
use crate::method::JmapObject;
use crate::session::capability;
use crate::types::{Id, NoArgs, NoCondition};

/// Identity (RFC 8621 section 6): a From address the user may send as.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Identity {
    pub id: Id,
    pub name: Option<String>,
    /// May be a wildcard such as `*@example.com`.
    pub email: Option<String>,
    pub reply_to: Option<Vec<EmailAddress>>,
    pub bcc: Option<Vec<EmailAddress>>,
    pub text_signature: Option<String>,
    pub html_signature: Option<String>,
    pub may_delete: Option<bool>,
}

impl Identity {
    pub fn address(&self) -> EmailAddress {
        EmailAddress {
            name: self.name.clone().filter(|n| !n.is_empty()),
            email: self.email.clone().unwrap_or_default(),
        }
    }
}

impl JmapObject for Identity {
    const TYPE: &'static str = "Identity";
    const USING: &'static [&'static str] = &[capability::SUBMISSION];
    type Condition = NoCondition;
    type Create = Value;
    type GetArgs = NoArgs;
    type QueryArgs = NoArgs;
    type SetArgs = NoArgs;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::method::GetResponse;

    #[test]
    fn parses_rfc8621_identity_get() {
        let resp: GetResponse<Identity> = serde_json::from_str(
            r#"{"accountId":"acme","state":"99401312","list":[
                {"id":"XD-3301-222-11_22AAz","name":"Joe Bloggs","email":"joe@example.com",
                 "replyTo":null,"bcc":[{"name":null,"email":"joe+archive@example.com"}],
                 "textSignature":"-- \nJoe Bloggs\nMaster of Email","htmlSignature":"<div><b>Joe Bloggs</b></div>",
                 "mayDelete":false},
                {"id":"XD-9911312-11_22AAz","name":"Joe B","email":"*@example.com","replyTo":null,
                 "bcc":null,"textSignature":"","htmlSignature":"","mayDelete":true}],
                "notFound":["xyz"]}"#,
        )
        .unwrap();
        assert_eq!(resp.list.len(), 2);
        assert_eq!(
            resp.list[0].address(),
            EmailAddress::new(Some("Joe Bloggs"), "joe@example.com")
        );
        assert_eq!(
            resp.list[0].bcc.as_ref().unwrap()[0].email,
            "joe+archive@example.com"
        );
        assert_eq!(resp.list[1].email.as_deref(), Some("*@example.com"));
    }
}
