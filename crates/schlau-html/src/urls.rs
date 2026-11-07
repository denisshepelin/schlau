use std::borrow::Cow;

use percent_encoding::percent_decode_str;
use url::Url;

use crate::options::{Options, RemotePolicy, Tally};
use crate::trackers::{self, TrackerKind};

/// A parsed `http(s)` URL with its lowercase host.
#[derive(Debug, Clone)]
pub(crate) struct RemoteUrl {
    pub url: Url,
    pub host: String,
}

impl RemoteUrl {
    pub fn tracker_kind(&self) -> Option<TrackerKind> {
        trackers::classify(&self.host, &self.url.path().to_ascii_lowercase())
    }

    pub fn report_domain(&self) -> String {
        trackers::report_domain(&self.host)
    }
}

/// What a URL found in a resource position refers to.
#[derive(Debug, Clone)]
pub(crate) enum Resource {
    Cid(String),
    DataImage,
    Remote(RemoteUrl),
    Invalid,
}

/// Outcome for a resource reference (image `src`, `background`, CSS `url()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResourceAction {
    Keep(String),
    Pixel,
    Blocked,
    Invalid,
}

/// Browsers drop ASCII tab and newline anywhere in a URL and trim C0 controls and spaces.
pub(crate) fn normalize_url(raw: &str) -> Cow<'_, str> {
    let trimmed = raw.trim_matches(|c: char| c <= ' ');
    if trimmed.contains(['\t', '\n', '\r']) {
        Cow::Owned(
            trimmed
                .chars()
                .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
                .collect(),
        )
    } else {
        Cow::Borrowed(trimmed)
    }
}

pub(crate) fn has_scheme(url: &str, scheme: &str) -> bool {
    url.len() > scheme.len()
        && url.as_bytes()[scheme.len()] == b':'
        && url[..scheme.len()].eq_ignore_ascii_case(scheme)
}

pub(crate) fn parse_remote(url: &str) -> Option<RemoteUrl> {
    let parsed = if url.starts_with("//") {
        Url::parse(&format!("https:{url}")).ok()?
    } else if has_scheme(url, "http") || has_scheme(url, "https") {
        Url::parse(url).ok()?
    } else {
        return None;
    };
    let host = parsed
        .host_str()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    Some(RemoteUrl { url: parsed, host })
}

pub(crate) fn is_remote(url: &str) -> bool {
    let url = normalize_url(url);
    url.starts_with("//") || has_scheme(&url, "http") || has_scheme(&url, "https")
}

pub(crate) fn classify_resource(raw: &str) -> Resource {
    let url = normalize_url(raw);
    if has_scheme(&url, "cid") {
        Resource::Cid(
            percent_decode_str(&url[4..])
                .decode_utf8_lossy()
                .into_owned(),
        )
    } else if has_scheme(&url, "data") {
        if url[5..]
            .trim_start()
            .get(..6)
            .is_some_and(|mime| mime.eq_ignore_ascii_case("image/"))
        {
            Resource::DataImage
        } else {
            Resource::Invalid
        }
    } else {
        parse_remote(&url).map_or(Resource::Invalid, Resource::Remote)
    }
}

fn upgrade_to_https(mut url: Url) -> String {
    if url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
    url.into()
}

/// Applies the remote content policy to one resource reference and records what happened.
pub(crate) fn decide_resource(raw: &str, options: &Options, tally: &mut Tally) -> ResourceAction {
    match classify_resource(raw) {
        Resource::Cid(id) => options
            .resolve_cid(&id)
            .map_or(ResourceAction::Invalid, |local| {
                ResourceAction::Keep(local.to_owned())
            }),
        Resource::DataImage => ResourceAction::Keep(normalize_url(raw).into_owned()),
        Resource::Invalid => ResourceAction::Invalid,
        Resource::Remote(remote) => decide_remote(remote, options.remote, tally),
    }
}

pub(crate) fn decide_remote(
    remote: RemoteUrl,
    policy: RemotePolicy,
    tally: &mut Tally,
) -> ResourceAction {
    match (remote.tracker_kind(), policy) {
        (Some(TrackerKind::Tracking), _) => {
            tally.pixels_removed += 1;
            tally.domains.insert(remote.report_domain());
            ResourceAction::Pixel
        }
        (Some(TrackerKind::Sender), RemotePolicy::AllowExceptTrackers) => {
            tally.remote_blocked += 1;
            tally.domains.insert(remote.report_domain());
            ResourceAction::Blocked
        }
        (_, RemotePolicy::Block) => {
            tally.remote_blocked += 1;
            ResourceAction::Blocked
        }
        _ => ResourceAction::Keep(upgrade_to_https(remote.url)),
    }
}

/// Records a removed pixel that was detected by size or visibility rather than by URL.
pub(crate) fn record_pixel(raw: &str, tally: &mut Tally) {
    tally.pixels_removed += 1;
    if let Some(remote) = parse_remote(&normalize_url(raw)) {
        tally.domains.insert(remote.report_domain());
    }
}

/// Decodes the entities ammonia's serializer emits inside attribute values.
pub(crate) fn decode_attr(value: &str) -> Cow<'_, str> {
    if !value.contains('&') {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        let entity = rest[1..]
            .find(';')
            .filter(|end| *end <= 10)
            .map(|end| &rest[1..=end]);
        let decoded = entity.and_then(decode_entity);
        match (entity, decoded) {
            (Some(name), Some(ch)) => {
                out.push(ch);
                rest = &rest[name.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "quot" => Some('"'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "nbsp" => Some('\u{a0}'),
        "apos" => Some('\''),
        _ => {
            let digits = name.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Escapes text for a double-quoted attribute value or element content.
pub(crate) fn escape_html(value: &str) -> Cow<'_, str> {
    if !value.contains(['&', '"', '<', '>', '\'']) {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len() + 16);
    value.chars().for_each(|c| match c {
        '&' => out.push_str("&amp;"),
        '"' => out.push_str("&quot;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '\'' => out.push_str("&#39;"),
        other => out.push(other),
    });
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_round_trip() {
        assert_eq!(
            decode_attr("a&amp;b&quot;c&#x41;&nbsp;&bogus"),
            "a&b\"cA\u{a0}&bogus"
        );
        assert_eq!(
            escape_html("<a href=\"x\">&"),
            "&lt;a href=&quot;x&quot;&gt;&amp;"
        );
    }

    #[test]
    fn resource_classes() {
        assert!(matches!(classify_resource("cid:part1%40x"), Resource::Cid(id) if id == "part1@x"));
        assert!(matches!(
            classify_resource(" DATA:image/png;base64,AA"),
            Resource::DataImage
        ));
        assert!(matches!(
            classify_resource("data:text/html,<b>"),
            Resource::Invalid
        ));
        assert!(matches!(
            classify_resource("java\nscript:alert(1)"),
            Resource::Invalid
        ));
        assert!(matches!(
            classify_resource("//cdn.example.com/a.png"),
            Resource::Remote(_)
        ));
    }
}
