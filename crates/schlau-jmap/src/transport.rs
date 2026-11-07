use std::io::Read;
use std::time::Duration;

use crate::error::{Error, Result};

/// A complete HTTP response as seen by the JMAP layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn ok_json(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            content_type: Some("application/json".into()),
            body: body.into(),
        }
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The blocking HTTP operations the JMAP client needs. Implementations own authentication.
///
/// Non-2xx statuses must be returned as `Ok(HttpResponse)`, not errors, so the client can
/// interpret JMAP problem details. `Err` is reserved for connection-level failures.
pub trait Transport: Send + Sync {
    fn get(&self, url: &str, accept: &str) -> Result<HttpResponse>;

    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<HttpResponse>;

    /// Open a long-lived GET (for `text/event-stream`) and return the body as a reader.
    /// A non-2xx status must be reported as `Error::Http`.
    fn open_stream(&self, url: &str, last_event_id: Option<&str>) -> Result<Box<dyn Read + Send>>;
}

impl<T: Transport + ?Sized> Transport for &T {
    fn get(&self, url: &str, accept: &str) -> Result<HttpResponse> {
        (**self).get(url, accept)
    }

    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<HttpResponse> {
        (**self).post(url, content_type, body)
    }

    fn open_stream(&self, url: &str, last_event_id: Option<&str>) -> Result<Box<dyn Read + Send>> {
        (**self).open_stream(url, last_event_id)
    }
}

impl<T: Transport + ?Sized> Transport for std::sync::Arc<T> {
    fn get(&self, url: &str, accept: &str) -> Result<HttpResponse> {
        (**self).get(url, accept)
    }

    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<HttpResponse> {
        (**self).post(url, content_type, body)
    }

    fn open_stream(&self, url: &str, last_event_id: Option<&str>) -> Result<Box<dyn Read + Send>> {
        (**self).open_stream(url, last_event_id)
    }
}

/// Timeouts for [`UreqTransport`].
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    /// Whole-call limit for API, download and upload requests.
    pub call: Duration,
    /// Max silence on an event stream before the read fails. Keep it above the ping interval.
    pub stream_read: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            call: Duration::from_secs(120),
            stream_read: Duration::from_secs(75),
        }
    }
}

/// [`Transport`] over `ureq` with `Authorization: Bearer <token>`.
#[derive(Clone)]
pub struct UreqTransport {
    agent: ureq::Agent,
    stream_agent: ureq::Agent,
    authorization: String,
}

impl std::fmt::Debug for UreqTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UreqTransport").finish_non_exhaustive()
    }
}

const USER_AGENT: &str = concat!("schlau-jmap/", env!("CARGO_PKG_VERSION"));

impl UreqTransport {
    pub fn bearer(token: &str) -> Self {
        Self::with_timeouts(token, Timeouts::default())
    }

    pub fn with_timeouts(token: &str, timeouts: Timeouts) -> Self {
        let base = || {
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .user_agent(USER_AGENT)
                .timeout_connect(Some(timeouts.connect))
        };
        Self {
            agent: base().timeout_global(Some(timeouts.call)).build().into(),
            stream_agent: base()
                .timeout_per_read(Some(timeouts.stream_read))
                .build()
                .into(),
            authorization: format!("Bearer {token}"),
        }
    }
}

fn transport_err(e: ureq::Error) -> Error {
    match e {
        ureq::Error::Io(io) => Error::Io(io),
        other => Error::Transport(other.to_string()),
    }
}

fn into_http_response(mut resp: ureq::http::Response<ureq::Body>) -> Result<HttpResponse> {
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = resp
        .body_mut()
        .with_config()
        .limit(u64::MAX)
        .read_to_vec()
        .map_err(transport_err)?;
    Ok(HttpResponse {
        status,
        content_type,
        body,
    })
}

impl Transport for UreqTransport {
    fn get(&self, url: &str, accept: &str) -> Result<HttpResponse> {
        self.agent
            .get(url)
            .header("Authorization", &self.authorization)
            .header("Accept", accept)
            .call()
            .map_err(transport_err)
            .and_then(into_http_response)
    }

    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<HttpResponse> {
        self.agent
            .post(url)
            .header("Authorization", &self.authorization)
            .header("Content-Type", content_type)
            .header("Accept", "application/json")
            .send(body)
            .map_err(transport_err)
            .and_then(into_http_response)
    }

    fn open_stream(&self, url: &str, last_event_id: Option<&str>) -> Result<Box<dyn Read + Send>> {
        let request = self
            .stream_agent
            .get(url)
            .header("Authorization", &self.authorization)
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache");
        let request = match last_event_id {
            Some(id) => request.header("Last-Event-ID", id),
            None => request,
        };
        let resp = request.call().map_err(transport_err)?;
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            Ok(Box::new(resp.into_body().into_reader()))
        } else {
            let failed = into_http_response(resp)?;
            Err(Error::Http {
                status,
                url: url.to_owned(),
                body: String::from_utf8_lossy(&failed.body).into_owned(),
            })
        }
    }
}
