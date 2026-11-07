//! Remote pictures in message bodies, fetched only when asked for. Loading one tells its server that
//! the message was opened, and from where, so nothing loads by itself: the reader shows each picture's
//! description and host until it is asked for, one picture or a whole conversation at a time.
//! Tracking pixels never get this far; the sanitizer drops them.
//!
//! Fetches carry no cookies, no referrer and no account details. Pictures are decoded off the main
//! thread and kept for the session.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use rune::{Image, ImageCache, Waker};
use schlau_html::{Block, ImageSource};

/// Longest side a picture is decoded at, in pixels: wider than the reader at 2×.
const MAX_SIDE: u32 = 1800;
const MAX_BYTES: u64 = 15 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
const PARALLEL: usize = 4;
const USER_AGENT: &str = "Mozilla/5.0 (compatible; schlau)";

#[derive(Clone)]
pub enum RemoteImage {
    Loading,
    /// `size` is the picture's own size in pixels, when known before decoding.
    Ready { image: Image, size: Option<(u32, u32)> },
    Failed(String),
}

/// A fetched picture, ready for the image cache.
struct Fetched {
    bytes: Vec<u8>,
    size: Option<(u32, u32)>,
}

type Done = (String, Result<Fetched, String>);

pub struct RemoteImages {
    states: HashMap<String, RemoteImage>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    waker: Option<Waker>,
}

impl Default for RemoteImages {
    fn default() -> Self {
        let (done_tx, done_rx) = channel();
        RemoteImages { states: HashMap::new(), done_tx, done_rx, waker: None }
    }
}

impl RemoteImages {
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    pub fn get(&self, url: &str) -> Option<&RemoteImage> {
        self.states.get(url)
    }

    /// Whether `url` still needs asking for: never asked, or failed.
    pub fn wanted(&self, url: &str) -> bool {
        matches!(self.states.get(url), None | Some(RemoteImage::Failed(_)))
    }

    /// Starts fetching every picture in `urls` that isn't loaded or loading. Returns how many started.
    pub fn load(&mut self, urls: Vec<String>) -> usize {
        let mut todo: Vec<String> = urls.into_iter().filter(|u| self.wanted(u)).collect();
        todo.sort();
        todo.dedup();
        todo.iter().for_each(|u| {
            self.states.insert(u.clone(), RemoteImage::Loading);
        });
        let started = todo.len();
        if started > 0 {
            let (tx, waker) = (self.done_tx.clone(), self.waker.clone());
            std::thread::spawn(move || fetch_all(todo, tx, waker));
        }
        started
    }

    /// Takes in finished fetches. True when any arrived.
    pub fn poll(&mut self, images: &ImageCache) -> bool {
        let arrived: Vec<Done> = self.done_rx.try_iter().collect();
        let any = !arrived.is_empty();
        arrived.into_iter().for_each(|(url, result)| {
            let state = match result {
                Ok(f) => RemoteImage::Ready { image: images.load(f.bytes), size: f.size },
                Err(e) => RemoteImage::Failed(e),
            };
            self.states.insert(url, state);
        });
        any
    }
}

fn fetch_all(urls: Vec<String>, tx: Sender<Done>, waker: Option<Waker>) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent(USER_AGENT)
        .max_redirects(5)
        .build()
        .into();
    let per_thread = urls.len().div_ceil(PARALLEL).max(1);
    std::thread::scope(|scope| {
        urls.chunks(per_thread).for_each(|chunk| {
            let (agent, tx, waker) = (agent.clone(), tx.clone(), waker.clone());
            scope.spawn(move || {
                chunk.iter().for_each(|url| {
                    let _ = tx.send((url.clone(), fetch(&agent, url)));
                    if let Some(wake) = &waker {
                        wake();
                    }
                });
            });
        });
    });
}

/// `https` first, even for an `http` address; the address as written only if that fails, and its
/// error is the one reported.
fn candidates(url: &str) -> Vec<String> {
    match url.strip_prefix("http://") {
        Some(rest) => vec![format!("https://{rest}"), url.to_string()],
        None => vec![url.to_string()],
    }
}

fn fetch(agent: &ureq::Agent, url: &str) -> Result<Fetched, String> {
    let mut errors = Vec::new();
    let found = candidates(url).iter().map(|c| download(agent, c)).find_map(|r| r.map_err(|e| errors.push(e)).ok());
    match found {
        Some(bytes) => decode(bytes),
        None => Err(errors.pop().unwrap_or_else(|| "no address".into())),
    }
}

fn download(agent: &ureq::Agent, url: &str) -> Result<(Vec<u8>, String), String> {
    let mut response = agent.get(url).header("Accept", "image/*").call().map_err(short_error)?;
    let mime = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = response.body_mut().with_config().limit(MAX_BYTES).read_to_vec().map_err(short_error)?;
    Ok((bytes, mime))
}

fn decode((bytes, mime): (Vec<u8>, String)) -> Result<Fetched, String> {
    let svg = mime.contains("svg") || bytes.trim_ascii_start().starts_with(b"<");
    match svg {
        true => Ok(Fetched { bytes, size: None }),
        false => schlau_extract::web_image(&bytes, MAX_SIDE)
            .map(|i| Fetched { size: Some((i.width, i.height)), bytes: i.bytes })
            .map_err(|_| "not a picture".to_string()),
    }
}

fn short_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(code) => format!("the server answered {code}"),
        ureq::Error::Timeout(_) => "it took too long".into(),
        ureq::Error::BodyExceedsLimit(_) => "it is too large".into(),
        ureq::Error::HostNotFound => "the server wasn't found".into(),
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed => "no connection".into(),
        other => other.to_string(),
    }
}

/// The remote pictures in `blocks`, in reading order, blocked or allowed.
pub fn remote_urls(blocks: &[Block]) -> Vec<String> {
    blocks
        .iter()
        .flat_map(|b| match b {
            Block::Image { src: ImageSource::Blocked(url) | ImageSource::Remote(url), .. } => vec![url.clone()],
            Block::Quote(inner) => remote_urls(inner),
            Block::List { items, .. } => items.iter().flat_map(|item| remote_urls(item)).collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// The host a picture comes from, as the reader names it.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.rsplit_once('@').map_or(host, |(_, h)| h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_candidates() {
        assert_eq!(host("https://cdn.example.com/a/b.png?x=1"), "cdn.example.com");
        assert_eq!(host("http://user@img.example.org"), "img.example.org");
        assert_eq!(candidates("http://a.example/x.png"), ["https://a.example/x.png", "http://a.example/x.png"]);
        assert_eq!(candidates("https://a.example/x.png"), ["https://a.example/x.png"]);
    }

    #[test]
    fn finds_pictures_inside_quotes_and_lists() {
        let html = r#"<p><img alt="Hero" src="https://cdn.example.com/hero.jpg" width="600" height="300"></p>
            <blockquote><img alt="Quoted" src="https://cdn.example.com/q.png" width="200" height="100"></blockquote>
            <ul><li><img alt="Listed" src="http://img.example.org/l.gif" width="120" height="60"></li></ul>"#;
        let urls = remote_urls(&schlau_html::to_blocks(html));
        assert_eq!(urls.len(), 3, "{urls:?}");
        assert!(urls[0].contains("hero.jpg") && urls[2].contains("l.gif"));
        let widths: Vec<Option<u32>> = schlau_html::to_blocks(html)
            .iter()
            .filter_map(|b| match b {
                Block::Image { width, .. } => Some(*width),
                _ => None,
            })
            .collect();
        assert_eq!(widths.first(), Some(&Some(600)), "the message's width is kept");
    }

    #[test]
    fn svg_passes_through_and_garbage_fails() {
        assert!(decode((b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(), String::new())).is_ok_and(|f| f.size.is_none()));
        assert!(decode((b"\x00\x01garbage".to_vec(), "image/png".into())).is_err());
    }
}
