//! An offline [`Transport`] that replays canned responses and records requests.

use std::collections::VecDeque;
use std::io::{Cursor, Read};
use std::sync::Mutex;

use crate::error::{Error, Result};
use crate::transport::{HttpResponse, Transport};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedRequest {
    Get {
        url: String,
    },
    Post {
        url: String,
        content_type: String,
        body: Vec<u8>,
    },
    Stream {
        url: String,
        last_event_id: Option<String>,
    },
}

impl RecordedRequest {
    pub fn url(&self) -> &str {
        match self {
            Self::Get { url } | Self::Post { url, .. } | Self::Stream { url, .. } => url,
        }
    }

    pub fn json_body(&self) -> Option<serde_json::Value> {
        match self {
            Self::Post { body, .. } => serde_json::from_slice(body).ok(),
            _ => None,
        }
    }
}

enum Canned {
    Response(HttpResponse),
    Stream(Vec<Vec<u8>>),
}

/// Responses are returned in the order they were queued, regardless of URL.
#[derive(Default)]
pub struct MockTransport {
    queue: Mutex<VecDeque<Canned>>,
    requests: Mutex<Vec<RecordedRequest>>,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, response: HttpResponse) -> &Self {
        self.queue
            .lock()
            .expect("mock poisoned")
            .push_back(Canned::Response(response));
        self
    }

    pub fn push_json(&self, body: &str) -> &Self {
        self.push(HttpResponse::ok_json(body.as_bytes()))
    }

    /// Queue an event stream delivered to the reader in exactly these chunks.
    pub fn push_stream<I, C>(&self, chunks: I) -> &Self
    where
        I: IntoIterator<Item = C>,
        C: Into<Vec<u8>>,
    {
        self.queue
            .lock()
            .expect("mock poisoned")
            .push_back(Canned::Stream(chunks.into_iter().map(Into::into).collect()));
        self
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("mock poisoned").clone()
    }

    fn record(&self, request: RecordedRequest) {
        self.requests.lock().expect("mock poisoned").push(request);
    }

    fn next(&self) -> Result<Canned> {
        self.queue
            .lock()
            .expect("mock poisoned")
            .pop_front()
            .ok_or_else(|| Error::Transport("mock transport has no queued response".into()))
    }

    fn next_response(&self) -> Result<HttpResponse> {
        match self.next()? {
            Canned::Response(r) => Ok(r),
            Canned::Stream(_) => Err(Error::Transport("expected response, found stream".into())),
        }
    }
}

impl Transport for MockTransport {
    fn get(&self, url: &str, _accept: &str) -> Result<HttpResponse> {
        self.record(RecordedRequest::Get { url: url.into() });
        self.next_response()
    }

    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<HttpResponse> {
        self.record(RecordedRequest::Post {
            url: url.into(),
            content_type: content_type.into(),
            body: body.to_vec(),
        });
        self.next_response()
    }

    fn open_stream(&self, url: &str, last_event_id: Option<&str>) -> Result<Box<dyn Read + Send>> {
        self.record(RecordedRequest::Stream {
            url: url.into(),
            last_event_id: last_event_id.map(str::to_owned),
        });
        match self.next()? {
            Canned::Stream(chunks) => Ok(Box::new(ChunkedReader {
                chunks: chunks.into(),
            })),
            Canned::Response(r) if r.is_success() => Ok(Box::new(Cursor::new(r.body))),
            Canned::Response(r) => Err(Error::Http {
                status: r.status,
                url: url.into(),
                body: String::from_utf8_lossy(&r.body).into_owned(),
            }),
        }
    }
}

/// Returns at most one queued chunk per `read`, to exercise partial-line handling.
struct ChunkedReader {
    chunks: VecDeque<Vec<u8>>,
}

impl Read for ChunkedReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Some(mut chunk) = self.chunks.pop_front() else {
            return Ok(0);
        };
        let n = chunk.len().min(buf.len());
        buf[..n].copy_from_slice(&chunk[..n]);
        if n < chunk.len() {
            self.chunks.push_front(chunk.split_off(n));
        }
        Ok(n)
    }
}
