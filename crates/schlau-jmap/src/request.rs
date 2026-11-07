//! Batching several method calls into one API request, with result references.

use std::collections::{BTreeSet, HashMap};
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, MethodError, Result};
use crate::method::{Method, PropertiesArg};
use crate::session::capability;
use crate::types::{Id, IdsArg, ResultReference};

/// A handle to a call added to a [`RequestBuilder`], used to reference or read its result.
#[derive(Debug)]
pub struct Call<R> {
    id: String,
    name: String,
    _response: PhantomData<fn() -> R>,
}

impl<R> Clone for Call<R> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            name: self.name.clone(),
            _response: PhantomData,
        }
    }
}

impl<R> Call<R> {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Reference a JSON pointer into this call's result, e.g. `/ids` or `/list/*/threadId`.
    pub fn reference(&self, path: &str) -> ResultReference {
        ResultReference {
            result_of: self.id.clone(),
            name: self.name.clone(),
            path: path.to_owned(),
        }
    }

    pub fn ids_at(&self, path: &str) -> IdsArg {
        IdsArg::Ref(self.reference(path))
    }

    pub fn properties_at(&self, path: &str) -> PropertiesArg {
        PropertiesArg::Ref(self.reference(path))
    }
}

/// A raw `[name, arguments, callId]` invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invocation(pub String, pub Value, pub String);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub using: Vec<String>,
    pub method_calls: Vec<Invocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_ids: Option<HashMap<String, Id>>,
}

#[derive(Debug, Default)]
pub struct RequestBuilder {
    using: BTreeSet<&'static str>,
    calls: Vec<Invocation>,
    error: Option<serde_json::Error>,
}

impl RequestBuilder {
    pub fn new() -> Self {
        Self {
            using: BTreeSet::from([capability::CORE]),
            ..Self::default()
        }
    }

    pub fn call<M: Method>(&mut self, method: M) -> Call<M::Response> {
        let id = format!("c{}", self.calls.len());
        let name = method.name();
        self.using.extend(method.using());
        match serde_json::to_value(&method) {
            Ok(args) => self.calls.push(Invocation(name.clone(), args, id.clone())),
            Err(e) => {
                self.error.get_or_insert(e);
            }
        }
        Call {
            id,
            name,
            _response: PhantomData,
        }
    }

    /// Add an untyped call; the result can be read with [`Response::get`] as any type.
    pub fn raw<R>(&mut self, name: &str, args: Value, using: &'static [&'static str]) -> Call<R> {
        let id = format!("c{}", self.calls.len());
        self.using.extend(using);
        self.calls
            .push(Invocation(name.to_owned(), args, id.clone()));
        Call {
            id,
            name: name.to_owned(),
            _response: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.calls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    pub fn build(self) -> Result<Request> {
        match self.error {
            Some(e) => Err(e.into()),
            None => Ok(Request {
                using: self.using.into_iter().map(str::to_owned).collect(),
                method_calls: self.calls,
                created_ids: None,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub method_responses: Vec<Invocation>,
    pub session_state: String,
    #[serde(default)]
    pub created_ids: Option<HashMap<String, Id>>,
}

impl Response {
    /// The typed result of `call`, or the method error the server returned for it.
    pub fn get<R: DeserializeOwned>(&self, call: &Call<R>) -> Result<R> {
        self.get_named(call.id(), call.name())
    }

    /// Look up a response by call id and method name. Needed for implicit calls, such as the
    /// `Email/set` that `EmailSubmission/set` with `onSuccessUpdateEmail` produces.
    pub fn get_named<R: DeserializeOwned>(&self, call_id: &str, name: &str) -> Result<R> {
        let found = self
            .method_responses
            .iter()
            .filter(|Invocation(_, _, id)| id == call_id)
            .find(|Invocation(n, _, _)| n == name || n == "error");
        match found {
            Some(Invocation(n, args, id)) if n == "error" => {
                Err(Error::Method(method_error(args, id)?))
            }
            Some(Invocation(_, args, _)) => Ok(R::deserialize(args)?),
            None => Err(Error::MissingResponse {
                call_id: call_id.to_owned(),
                method: name.to_owned(),
            }),
        }
    }

    /// Like [`Response::get_named`], but `None` when the server sent no such response.
    pub fn try_get_named<R: DeserializeOwned>(
        &self,
        call_id: &str,
        name: &str,
    ) -> Result<Option<R>> {
        match self.get_named(call_id, name) {
            Err(Error::MissingResponse { .. }) => Ok(None),
            other => other.map(Some),
        }
    }

    pub fn errors(&self) -> Vec<MethodError> {
        self.method_responses
            .iter()
            .filter(|Invocation(n, _, _)| n == "error")
            .filter_map(|Invocation(_, args, id)| method_error(args, id).ok())
            .collect()
    }
}

fn method_error(args: &Value, call_id: &str) -> Result<MethodError> {
    let mut e = MethodError::deserialize(args)?;
    e.call_id = call_id.to_owned();
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::{Email, EmailQueryArgs};
    use crate::email_filter::EmailFilterCondition;
    use crate::error::MethodErrorKind;
    use crate::method::{Get, GetResponse, Query, QueryResponse};
    use crate::types::Comparator;
    use serde_json::json;

    #[test]
    fn builds_query_then_get_with_back_reference() {
        let mut req = RequestBuilder::new();
        let q = req.call(
            Query::<Email>::new("A1")
                .filter(EmailFilterCondition::in_mailbox("inbox"))
                .sort([Comparator::received_at_desc()])
                .limit(10)
                .args(EmailQueryArgs {
                    collapse_threads: Some(true),
                }),
        );
        req.call(
            Get::<Email>::new("A1", q.ids_at("/ids"))
                .properties(PropertiesArg::list(["id", "subject"])),
        );
        let body = serde_json::to_value(req.build().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({
                "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
                "methodCalls": [
                    ["Email/query", {
                        "accountId": "A1",
                        "filter": {"inMailbox": "inbox"},
                        "sort": [{"property": "receivedAt", "isAscending": false}],
                        "limit": 10,
                        "collapseThreads": true
                    }, "c0"],
                    ["Email/get", {
                        "accountId": "A1",
                        "#ids": {"resultOf": "c0", "name": "Email/query", "path": "/ids"},
                        "properties": ["id", "subject"]
                    }, "c1"]
                ]
            })
        );
    }

    #[test]
    fn reads_typed_results_and_method_errors() {
        let resp: Response = serde_json::from_value(json!({
            "methodResponses": [
                ["Email/query", {
                    "accountId": "A1", "queryState": "q1", "canCalculateChanges": true,
                    "position": 0, "ids": ["e1", "e2"], "total": 2
                }, "c0"],
                ["error", {"type": "invalidResultReference"}, "c1"],
                ["error", {"type": "cannotCalculateChanges"}, "c2"]
            ],
            "sessionState": "s1"
        }))
        .unwrap();
        let q: Call<QueryResponse> = Call {
            id: "c0".into(),
            name: "Email/query".into(),
            _response: PhantomData,
        };
        let got = resp.get(&q).unwrap();
        assert_eq!(got.ids, vec![Id::from("e1"), Id::from("e2")]);

        let g: Call<GetResponse<Email>> = Call {
            id: "c2".into(),
            name: "Email/changes".into(),
            _response: PhantomData,
        };
        let err = resp.get(&g).unwrap_err();
        assert!(err.is_cannot_calculate_changes());
        match err {
            Error::Method(e) => assert_eq!(e.call_id, "c2"),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(resp.errors().len(), 2);
        assert_eq!(
            resp.errors()[0].kind,
            MethodErrorKind::InvalidResultReference
        );

        let missing = resp.get_named::<Value>("c9", "Email/get").unwrap_err();
        assert!(matches!(missing, Error::MissingResponse { .. }));
        assert!(
            resp.try_get_named::<Value>("c9", "Email/get")
                .unwrap()
                .is_none()
        );
    }
}
