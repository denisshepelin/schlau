use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::method::JmapObject;
use crate::session::capability;
use crate::types::{Id, PatchObject, UtcDate};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Address {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Map<String, Value>>,
}

impl Address {
    pub fn new(email: &str) -> Self {
        Self {
            email: email.to_owned(),
            parameters: None,
        }
    }
}

/// SMTP envelope. When omitted on create, the server derives it from the message headers.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub mail_from: Address,
    pub rcpt_to: Vec<Address>,
}

/// EmailSubmission (RFC 8621 section 7).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EmailSubmission {
    pub id: Id,
    pub identity_id: Option<Id>,
    pub email_id: Option<Id>,
    pub thread_id: Option<Id>,
    pub envelope: Option<Envelope>,
    pub send_at: Option<UtcDate>,
    /// `pending`, `final` or `canceled`.
    pub undo_status: Option<String>,
    pub delivery_status: Option<Map<String, Value>>,
    pub dsn_blob_ids: Option<Vec<Id>>,
    pub mdn_blob_ids: Option<Vec<Id>>,
}

/// A submission to create. `email_id` may be `#creationId` to reference an Email created
/// earlier in the same request.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSubmissionCreate {
    pub identity_id: Id,
    pub email_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Envelope>,
}

/// `onSuccessUpdateEmail` keys are `#<submission creation id>` or an EmailSubmission id.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSubmissionSetArgs {
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub on_success_update_email: HashMap<String, PatchObject>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub on_success_destroy_email: Vec<String>,
}

impl JmapObject for EmailSubmission {
    const TYPE: &'static str = "EmailSubmission";
    const USING: &'static [&'static str] = &[capability::MAIL, capability::SUBMISSION];
    type Condition = Value;
    type Create = EmailSubmissionCreate;
    type GetArgs = crate::types::NoArgs;
    type QueryArgs = crate::types::NoArgs;
    type SetArgs = EmailSubmissionSetArgs;
}
