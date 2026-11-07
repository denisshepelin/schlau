//! JMAP push over `text/event-stream` (RFC 8620 section 7.3).

use std::collections::{HashMap, VecDeque};
use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::{Id, State};

/// One dispatched server-sent event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
    pub id: Option<String>,
}

/// Incremental `text/event-stream` parser that accepts arbitrary byte chunk boundaries.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    after_cr: bool,
    started: bool,
    event: Option<String>,
    data: Option<String>,
    last_event_id: Option<String>,
    retry_ms: Option<u64>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// The `id` the caller should send as `Last-Event-ID` when reconnecting.
    pub fn last_event_id(&self) -> Option<&str> {
        self.last_event_id.as_deref()
    }

    /// Server-requested reconnection delay, if any.
    pub fn retry_ms(&self) -> Option<u64> {
        self.retry_ms
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        for &b in bytes {
            match b {
                b'\n' if self.after_cr => self.after_cr = false,
                b'\n' | b'\r' => {
                    self.after_cr = b == b'\r';
                    let line = std::mem::take(&mut self.line);
                    out.extend(self.process_line(&line));
                }
                _ => {
                    self.after_cr = false;
                    self.line.push(b);
                }
            }
        }
        out
    }

    fn process_line(&mut self, raw: &[u8]) -> Option<SseEvent> {
        let raw = if self.started {
            raw
        } else {
            self.started = true;
            raw.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(raw)
        };
        if raw.is_empty() {
            return self.dispatch();
        }
        let line = String::from_utf8_lossy(raw);
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line.as_ref(), ""),
        };
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => {
                let data = self.data.get_or_insert_with(String::new);
                data.push_str(value);
                data.push('\n');
            }
            "id" if !value.contains('\0') => self.last_event_id = Some(value.to_owned()),
            "retry" => {
                if let Ok(ms) = value.parse() {
                    self.retry_ms = Some(ms);
                }
            }
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        let event = self.event.take();
        let mut data = self.data.take()?;
        data.pop();
        Some(SseEvent {
            event: event
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "message".into()),
            data,
            id: self.last_event_id.clone(),
        })
    }
}

/// `{"@type":"StateChange","changed":{accountId:{TypeName:state}}}` (RFC 8620 section 7.1).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StateChange {
    #[serde(rename = "@type")]
    pub kind: String,
    pub changed: HashMap<Id, HashMap<String, State>>,
}

impl StateChange {
    pub fn for_account(&self, account_id: &Id) -> Option<&HashMap<String, State>> {
        self.changed.get(account_id)
    }

    /// The new state of `type_name` (e.g. `Email`, `Mailbox`, `Thread`, `EmailDelivery`).
    pub fn state_of(&self, account_id: &Id, type_name: &str) -> Option<&State> {
        self.for_account(account_id)?.get(type_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushEvent {
    StateChange(StateChange),
    /// Keep-alive; `interval` is the ping period in seconds when the server reports it.
    Ping {
        interval: Option<u64>,
    },
    /// Any other event the server sent, unparsed.
    Other(SseEvent),
}

impl PushEvent {
    pub fn from_sse(event: SseEvent) -> Self {
        #[derive(Deserialize)]
        struct Ping {
            interval: Option<u64>,
        }
        match event.event.as_str() {
            "ping" => PushEvent::Ping {
                interval: serde_json::from_str::<Ping>(&event.data)
                    .ok()
                    .and_then(|p| p.interval),
            },
            _ => match serde_json::from_str::<StateChange>(&event.data) {
                Ok(sc) if sc.kind == "StateChange" => PushEvent::StateChange(sc),
                _ => PushEvent::Other(event),
            },
        }
    }
}

/// A blocking iterator over push events. It ends (`None`) when the server closes the stream;
/// reconnecting, with [`EventStream::last_event_id`] and backoff, is up to the caller.
pub struct EventStream<R> {
    reader: R,
    parser: SseParser,
    pending: VecDeque<PushEvent>,
    done: bool,
}

impl<R: Read> EventStream<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            parser: SseParser::new(),
            pending: VecDeque::new(),
            done: false,
        }
    }

    pub fn last_event_id(&self) -> Option<&str> {
        self.parser.last_event_id()
    }

    pub fn retry_ms(&self) -> Option<u64> {
        self.parser.retry_ms()
    }
}

impl<R: Read> Iterator for EventStream<R> {
    type Item = Result<PushEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut buf = [0u8; 8192];
        loop {
            if let Some(ev) = self.pending.pop_front() {
                return Some(Ok(ev));
            }
            if self.done {
                return None;
            }
            match self.reader.read(&mut buf) {
                Ok(0) => self.done = true,
                Ok(n) => self.pending.extend(
                    self.parser
                        .feed(&buf[..n])
                        .into_iter()
                        .map(PushEvent::from_sse),
                ),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.done = true;
                    return Some(Err(e.into()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut p = SseParser::new();
        chunks.iter().flat_map(|c| p.feed(c)).collect()
    }

    #[test]
    fn parses_events_split_mid_line_and_mid_utf8() {
        let whole = "event: state\ndata: {\"@type\":\"StateChange\",\"changed\":{\"u1\":{\"Email\":\"s\u{e9}1\"}}}\n\n";
        let bytes = whole.as_bytes();
        let one = feed_all(&[bytes]);
        for split in 1..bytes.len() {
            let (a, b) = bytes.split_at(split);
            assert_eq!(feed_all(&[a, b]), one, "split at {split}");
        }
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].event, "state");
        assert!(one[0].data.contains("s\u{e9}1"));
    }

    #[test]
    fn handles_crlf_cr_comments_multiline_data_and_ids() {
        let events = feed_all(&[
            b"\xEF\xBB\xBF: hello\r\nid: 7\r",
            b"\ndata: a\r\ndata:b\r\rdata\n\nevent: ping\ndata: {\"interval\":30}\n\n",
            b"retry: 5000\nevent: state\n\n",
        ]);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].data, "a\nb");
        assert_eq!(events[0].event, "message");
        assert_eq!(events[0].id.as_deref(), Some("7"));
        assert_eq!(events[1].data, "");
        assert_eq!(events[2].event, "ping");
    }

    #[test]
    fn event_without_data_is_not_dispatched() {
        let mut p = SseParser::new();
        assert!(p.feed(b"event: state\n\nretry: 2500\n\n").is_empty());
        assert_eq!(p.retry_ms(), Some(2500));
    }

    #[test]
    fn classifies_push_events() {
        let sc = PushEvent::from_sse(SseEvent {
            event: "state".into(),
            data: r#"{"@type":"StateChange","changed":{"a3123":{"Email":"d35ecb040aab","EmailDelivery":"428d565f2440","CalendarEvent":"87accfac587a"},"a43461d":{"Mailbox":"0af7a512ce70","CalendarEvent":"7a4297cecd76"}}}"#.into(),
            id: None,
        });
        let PushEvent::StateChange(sc) = sc else {
            panic!("expected state change")
        };
        assert_eq!(
            sc.state_of(&Id::from("a3123"), "Email").unwrap().as_str(),
            "d35ecb040aab"
        );
        assert_eq!(sc.state_of(&Id::from("a43461d"), "Email"), None);

        let ping = PushEvent::from_sse(SseEvent {
            event: "ping".into(),
            data: r#"{"interval":30}"#.into(),
            id: None,
        });
        assert_eq!(ping, PushEvent::Ping { interval: Some(30) });

        let other = PushEvent::from_sse(SseEvent {
            event: "message".into(),
            data: "hi".into(),
            id: None,
        });
        assert!(matches!(other, PushEvent::Other(_)));
    }

    #[test]
    fn stream_iterates_over_chunked_reader_and_ends_on_eof() {
        let mock = crate::mock::MockTransport::new();
        mock.push_stream([
            ":ok\n\nevent: ping\nda".as_bytes(),
            b"ta: {\"interval\":30}\n\nevent: st",
            b"ate\nid: 42\ndata: {\"@type\":\"StateChange\",",
            b"\"changed\":{\"u1\":{\"Mailbox\":\"m9\"}}}\n\nevent: state\ndata: {\"trunc",
        ]);
        use crate::transport::Transport;
        let reader = mock.open_stream("https://x/event", None).unwrap();
        let mut stream = EventStream::new(reader);
        assert_eq!(
            stream.next().unwrap().unwrap(),
            PushEvent::Ping { interval: Some(30) }
        );
        let PushEvent::StateChange(sc) = stream.next().unwrap().unwrap() else {
            panic!("expected state change")
        };
        assert_eq!(
            sc.state_of(&Id::from("u1"), "Mailbox").unwrap().as_str(),
            "m9"
        );
        assert!(stream.next().is_none());
        assert_eq!(stream.last_event_id(), Some("42"));
    }
}
