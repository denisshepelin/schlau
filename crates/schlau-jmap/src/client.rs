use std::io::Read;

use serde::Deserialize;

use crate::error::{Error, ProblemDetails, Result};
use crate::eventsource::EventStream;
use crate::request::{Request, RequestBuilder, Response};
use crate::session::{FASTMAIL_SESSION_URL, Session};
use crate::transport::{HttpResponse, Transport, UreqTransport};
use crate::types::Id;

/// A JMAP client bound to one session. Cheap to share across threads by reference.
#[derive(Debug)]
pub struct Client<T = UreqTransport> {
    transport: T,
    session: Session,
    session_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResponse {
    pub account_id: Id,
    pub blob_id: Id,
    #[serde(rename = "type")]
    pub mime_type: String,
    pub size: u64,
}

impl Client<UreqTransport> {
    /// Connect to Fastmail with an API token.
    pub fn fastmail(token: &str) -> Result<Self> {
        Self::connect(UreqTransport::bearer(token), FASTMAIL_SESSION_URL)
    }
}

impl<T: Transport> Client<T> {
    /// Fetch the session resource and build a client from it.
    pub fn connect(transport: T, session_url: &str) -> Result<Self> {
        let session = fetch_session(&transport, session_url)?;
        Ok(Self {
            transport,
            session,
            session_url: Some(session_url.to_owned()),
        })
    }

    /// Build a client from a session obtained elsewhere (e.g. cached on disk).
    pub fn with_session(transport: T, session: Session) -> Self {
        Self {
            transport,
            session,
            session_url: None,
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Refetch the session. Returns whether its `state` changed.
    pub fn refresh_session(&mut self) -> Result<bool> {
        let url = self.session_url.as_deref().ok_or(Error::Invalid {
            what: "session",
            detail: "client was built without a session URL".into(),
        })?;
        let fresh = fetch_session(&self.transport, url)?;
        let changed = fresh.state != self.session.state;
        self.session = fresh;
        Ok(changed)
    }

    /// True when a response reports a session state other than the one this client holds.
    pub fn is_session_stale(&self, response: &Response) -> bool {
        response.session_state != self.session.state
    }

    pub fn mail_account_id(&self) -> Result<Id> {
        self.session.mail_account_id().cloned()
    }

    pub fn submission_account_id(&self) -> Result<Id> {
        self.session.submission_account_id().cloned()
    }

    pub fn send(&self, builder: RequestBuilder) -> Result<Response> {
        self.send_request(&builder.build()?)
    }

    pub fn send_request(&self, request: &Request) -> Result<Response> {
        let core = self.session.core();
        let calls = request.method_calls.len() as u64;
        if calls > core.max_calls_in_request {
            return Err(Error::LocalLimit {
                limit: "maxCallsInRequest",
                actual: calls,
                max: core.max_calls_in_request,
            });
        }
        let body = serde_json::to_vec(request)?;
        if body.len() as u64 > core.max_size_request {
            return Err(Error::LocalLimit {
                limit: "maxSizeRequest",
                actual: body.len() as u64,
                max: core.max_size_request,
            });
        }
        let url = &self.session.api_url;
        let resp = self.transport.post(url, "application/json", &body)?;
        Ok(serde_json::from_slice(&check_status(url, resp)?)?)
    }

    pub fn download(
        &self,
        account_id: &Id,
        blob_id: &Id,
        mime_type: &str,
        name: &str,
    ) -> Result<Vec<u8>> {
        let url = self
            .session
            .download_url_for(account_id, blob_id, mime_type, name);
        let resp = self.transport.get(&url, "*/*")?;
        check_status(&url, resp)
    }

    pub fn upload(&self, account_id: &Id, mime_type: &str, data: &[u8]) -> Result<UploadResponse> {
        let max = self.session.core().max_size_upload;
        if data.len() as u64 > max {
            return Err(Error::LocalLimit {
                limit: "maxSizeUpload",
                actual: data.len() as u64,
                max,
            });
        }
        let url = self.session.upload_url_for(account_id);
        let resp = self.transport.post(&url, mime_type, data)?;
        Ok(serde_json::from_slice(&check_status(&url, resp)?)?)
    }

    /// Open the push stream with `closeafter=no`. `types` is `*` or e.g. `Email,Mailbox`.
    /// Pass the previous stream's `last_event_id` when reconnecting.
    pub fn event_source(
        &self,
        types: &str,
        ping_seconds: u32,
        last_event_id: Option<&str>,
    ) -> Result<EventStream<Box<dyn Read + Send>>> {
        let url = self
            .session
            .event_source_url_for(types, false, ping_seconds);
        let reader = self.transport.open_stream(&url, last_event_id)?;
        Ok(EventStream::new(reader))
    }
}

fn fetch_session<T: Transport>(transport: &T, url: &str) -> Result<Session> {
    let resp = transport.get(url, "application/json")?;
    Session::from_json(&check_status(url, resp)?)
}

/// Map non-2xx responses to `Problem` (RFC 7807 body) or `Http`.
pub(crate) fn check_status(url: &str, resp: HttpResponse) -> Result<Vec<u8>> {
    if resp.is_success() {
        return Ok(resp.body);
    }
    let looks_like_problem = resp
        .content_type
        .as_deref()
        .is_some_and(|ct| ct.contains("problem+json") || ct.contains("application/json"));
    match looks_like_problem
        .then(|| serde_json::from_slice::<ProblemDetails>(&resp.body).ok())
        .flatten()
    {
        Some(problem) => Err(Error::Problem(problem)),
        None => Err(Error::Http {
            status: resp.status,
            url: url.to_owned(),
            body: String::from_utf8_lossy(&resp.body).into_owned(),
        }),
    }
}
