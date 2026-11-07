use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::method::JmapObject;
use crate::session::capability;
use crate::types::{Id, NoArgs, NoCondition};

/// Thread (RFC 8621 section 3): email ids sorted by `receivedAt`, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Thread {
    pub id: Id,
    pub email_ids: Vec<Id>,
}

impl JmapObject for Thread {
    const TYPE: &'static str = "Thread";
    const USING: &'static [&'static str] = &[capability::MAIL];
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
    fn parses_rfc8621_thread_get() {
        let resp: GetResponse<Thread> = serde_json::from_str(
            r#"{"accountId":"acme","state":"f6a7e214",
                "list":[{"id":"f123u4","emailIds":["eaa623","f782cbb"]},
                        {"id":"f41b213","emailIds":["1112ccc"]}],
                "notFound":[]}"#,
        )
        .unwrap();
        assert_eq!(resp.list[0].email_ids.len(), 2);
        assert_eq!(resp.list[1].email_ids[0].as_str(), "1112ccc");
    }
}
