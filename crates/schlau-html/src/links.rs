use std::borrow::Cow;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use percent_encoding::percent_decode_str;
use url::Url;

use crate::options::Tally;
use crate::trackers::{self, TrackerKind};
use crate::urls::{RemoteUrl, has_scheme, normalize_url, parse_remote};

/// Result of cleaning one link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanedLink {
    /// Safe destination: `http(s)`, `mailto:`, `tel:`, or a `#fragment`.
    pub href: String,
    /// How many redirect wrappers were removed.
    pub unwrapped: usize,
    /// How many tracking query parameters were removed.
    pub params_stripped: usize,
    /// Tracker or email-service domain the original link went through, if any.
    pub tracker_domain: Option<String>,
}

const TRACKING_PARAMS: &[&str] = &[
    "fbclid",
    "gclid",
    "gclsrc",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "yclid",
    "igshid",
    "twclid",
    "ttclid",
    "li_fat_id",
    "epik",
    "mc_cid",
    "mc_eid",
    "_hsenc",
    "_hsmi",
    "hsctatracking",
    "mkt_tok",
    "_ga",
    "_gl",
    "_kx",
    "ck_subscriber_id",
    "s_cid",
    "s_kwcid",
    "sc_cid",
    "wickedid",
    "ml_subscriber",
    "ml_subscriber_hash",
    "rb_clickid",
    "ss_campaign_id",
    "ss_campaign_name",
    "ss_campaign_sent_date",
    "ss_source",
    "__s",
    "trk",
    "trkcampaign",
    "trkinfo",
    "elqtrackid",
    "elqtrack",
    "elq",
    "elqaid",
    "elqat",
    "elqcampaignid",
    "_bta_tid",
    "_bta_c",
    "ref_src",
    "xtor",
    "_openstat",
    "_branch_match_id",
    "_branch_referrer",
    "__twitter_impression",
    "soc_src",
    "soc_trk",
    "cmpid_email",
    "sfmc_id",
    "sfmc_activityid",
    "et_rid",
    "et_cid",
    "j",
    "jb",
    "sfmc_sub",
    "l",
    "u",
    "mid",
    "pk_vid",
    "_ke",
    "_bhlid",
];

/// Params from [`TRACKING_PARAMS`] that are only stripped for hosts where they are known to be
/// tracking, because they are ordinary parameter names elsewhere.
const AMBIGUOUS_PARAMS: &[&str] = &["j", "jb", "l", "u", "mid", "elq"];

const TRACKING_PREFIXES: &[&str] = &[
    "utm_", "vero_", "oly_", "__hs", "hsa_", "pk_", "mtm_", "hmb_", "mc_",
];

const HOST_SPECIFIC_PARAMS: &[(&str, &[&str])] = &[
    ("youtube.com", &["si", "feature", "pp"]),
    ("youtu.be", &["si", "feature"]),
    ("spotify.com", &["si", "nd"]),
    ("instagram.com", &["igsh", "igshid", "img_index"]),
    ("twitter.com", &["s", "t", "ref_url"]),
    ("x.com", &["s", "t", "ref_url"]),
    (
        "linkedin.com",
        &[
            "trackingid",
            "lipi",
            "midtoken",
            "midsig",
            "trk",
            "trkemail",
            "eid",
            "otptoken",
        ],
    ),
    (
        "amazon.com",
        &[
            "ref_", "pd_rd_r", "pd_rd_w", "pd_rd_wg", "pf_rd_p", "pf_rd_r", "pd_rd_i",
        ],
    ),
    ("exacttarget.com", &["j", "jb", "l", "u", "mid"]),
];

const MAX_UNWRAP_DEPTH: usize = 5;

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn query_param(url: &Url, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    })
}

fn is_google_host(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    matches!(labels.as_slice(), ["google", ..] | ["www", "google", ..])
}

fn proofpoint_v2(encoded: &str) -> String {
    let swapped: String = encoded
        .chars()
        .map(|c| match c {
            '-' => '%',
            '_' => '/',
            other => other,
        })
        .collect();
    percent_decode_str(&swapped)
        .decode_utf8_lossy()
        .into_owned()
}

fn proofpoint_v3(raw: &str) -> Option<String> {
    let start = raw.find("/v3/__")? + "/v3/__".len();
    let rest = &raw[start..];
    let inner = &rest[..rest.find("__;")?];
    (!inner.contains('*')).then(|| inner.to_owned())
}

fn decode_base64_text(value: &str) -> Option<String> {
    let trimmed = value.trim_end_matches('=');
    URL_SAFE_NO_PAD
        .decode(trimmed)
        .or_else(|_| STANDARD_NO_PAD.decode(trimmed))
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

fn mandrill_destination(url: &Url) -> Option<String> {
    let payload = decode_base64_text(&query_param(url, &["p"])?)?;
    let outer: serde_json::Value = serde_json::from_str(&payload).ok()?;
    let inner = match outer.get("p") {
        Some(serde_json::Value::String(text)) => serde_json::from_str(text).ok()?,
        _ => outer,
    };
    inner.get("url")?.as_str().map(str::to_owned)
}

fn looks_like_destination(text: &str) -> bool {
    (has_scheme(text, "https") || has_scheme(text, "http")) && parse_remote(text).is_some()
}

/// Email-service click trackers sometimes carry the destination as a plain or base64 value.
fn embedded_destination(url: &Url) -> Option<String> {
    let from_query = url.query_pairs().find_map(|(_, value)| {
        let value = value.into_owned();
        if looks_like_destination(&value) {
            Some(value)
        } else {
            decode_base64_text(&value).filter(|text| looks_like_destination(text))
        }
    });
    from_query.or_else(|| {
        url.path_segments()?
            .filter(|segment| segment.len() >= 16)
            .find_map(|segment| {
                decode_base64_text(segment).filter(|text| looks_like_destination(text))
            })
    })
}

fn unwrap_once(raw: &str, url: &Url, host: &str) -> Option<String> {
    let path = url.path();
    if is_google_host(host) && path == "/url" {
        query_param(url, &["q", "url"])
    } else if host.ends_with(".safelinks.protection.outlook.com") {
        query_param(url, &["url"])
    } else if host_matches(host, "urldefense.proofpoint.com") && path == "/v2/url" {
        query_param(url, &["u"]).map(|value| proofpoint_v2(&value))
    } else if host_matches(host, "urldefense.proofpoint.com") && path == "/v1/url" {
        query_param(url, &["u"])
    } else if host_matches(host, "urldefense.com") {
        proofpoint_v3(raw)
    } else if [
        "l.facebook.com",
        "lm.facebook.com",
        "l.messenger.com",
        "l.instagram.com",
    ]
    .contains(&host)
    {
        query_param(url, &["u"])
    } else if host_matches(host, "youtube.com") && path == "/redirect" {
        query_param(url, &["q"])
    } else if (host_matches(host, "linkedin.com") && path.starts_with("/redir/"))
        || host == "slack-redir.net"
        || host == "out.reddit.com"
    {
        query_param(url, &["url"])
    } else if host == "t.umblr.com" {
        query_param(url, &["z"])
    } else if host == "vk.com" && path == "/away.php" {
        query_param(url, &["to"])
    } else if host_matches(host, "steamcommunity.com") && path == "/linkfilter/" {
        query_param(url, &["url", "u"])
    } else if host_matches(host, "mandrillapp.com") && path.starts_with("/track/click") {
        mandrill_destination(url)
    } else if trackers::host_kind(host).is_some() {
        embedded_destination(url)
    } else {
        None
    }
}

fn is_tracking_param(key: &str, host: &str) -> bool {
    let generic = TRACKING_PARAMS.contains(&key) && !AMBIGUOUS_PARAMS.contains(&key);
    generic
        || TRACKING_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
        || HOST_SPECIFIC_PARAMS
            .iter()
            .any(|(domain, params)| host_matches(host, domain) && params.contains(&key))
}

fn strip_params(url: &mut Url, host: &str) -> usize {
    let Some(query) = url.query().map(str::to_owned) else {
        return 0;
    };
    let (kept, removed): (Vec<&str>, Vec<&str>) = query.split('&').partition(|pair| {
        let key = pair.split('=').next().unwrap_or_default();
        let key = percent_decode_str(key)
            .decode_utf8_lossy()
            .to_ascii_lowercase();
        key.is_empty() || !is_tracking_param(&key, host)
    });
    let kept: Vec<&str> = kept.into_iter().filter(|pair| !pair.is_empty()).collect();
    if removed.is_empty() {
        return 0;
    }
    let joined = kept.join("&");
    url.set_query((!joined.is_empty()).then_some(joined.as_str()));
    removed.len()
}

enum Destination {
    Remote(RemoteUrl),
    Mail(String),
    Unsafe,
}

fn unwrap_chain(raw: &str, remote: RemoteUrl, depth: usize) -> (Destination, usize) {
    if depth == MAX_UNWRAP_DEPTH {
        return (Destination::Remote(remote), depth);
    }
    match unwrap_once(raw, &remote.url, &remote.host).map(|dest| normalize_url(&dest).into_owned())
    {
        Some(dest) if has_scheme(&dest, "mailto") => (Destination::Mail(dest), depth + 1),
        Some(dest) => match parse_remote(&dest) {
            Some(next) => unwrap_chain(&dest, next, depth + 1),
            None if dest.contains(':') && !dest.starts_with('/') => {
                (Destination::Unsafe, depth + 1)
            }
            None => (Destination::Remote(remote), depth),
        },
        None => (Destination::Remote(remote), depth),
    }
}

fn clean_remote(raw: &str) -> Option<CleanedLink> {
    let first = parse_remote(raw)?;
    let wrapper_kind = first.tracker_kind();
    let wrapper_report = first.report_domain();
    let (destination, unwrapped) = unwrap_chain(raw, first, 0);
    let wrapper_domain = match wrapper_kind {
        Some(TrackerKind::Tracking) => Some(wrapper_report),
        Some(TrackerKind::Sender) if unwrapped > 0 => Some(wrapper_report),
        _ => None,
    };
    Some(match destination {
        Destination::Unsafe => return None,
        Destination::Mail(href) => CleanedLink {
            href,
            unwrapped,
            params_stripped: 0,
            tracker_domain: wrapper_domain,
        },
        Destination::Remote(mut remote) => {
            let params_stripped = strip_params(&mut remote.url, &remote.host);
            let destination_domain = matches!(remote.tracker_kind(), Some(TrackerKind::Tracking))
                .then(|| remote.report_domain());
            CleanedLink {
                href: remote.url.as_str().to_owned(),
                unwrapped,
                params_stripped,
                tracker_domain: wrapper_domain.or(destination_domain),
            }
        }
    })
}

fn plain(href: Cow<'_, str>) -> CleanedLink {
    CleanedLink {
        href: href.into_owned(),
        unwrapped: 0,
        params_stripped: 0,
        tracker_domain: None,
    }
}

/// Cleans a link destination: unwraps known redirect wrappers, strips tracking query
/// parameters and rejects every scheme other than `http`, `https`, `mailto` and `tel`.
/// Returns `None` when the link must not be followed at all.
pub fn clean_link(raw: &str) -> Option<CleanedLink> {
    let url = normalize_url(raw);
    if url.starts_with('#') || has_scheme(&url, "mailto") || has_scheme(&url, "tel") {
        Some(plain(url))
    } else if url.starts_with("//") || has_scheme(&url, "http") || has_scheme(&url, "https") {
        clean_remote(&url)
    } else {
        None
    }
}

/// [`clean_link`] that also adds the outcome to a running tally.
pub(crate) fn clean_link_counted(raw: &str, tally: &mut Tally) -> Option<String> {
    clean_link(raw).map(|link| {
        tally.links_unwrapped += link.unwrapped;
        tally.params_stripped += link.params_stripped;
        tally.domains.extend(link.tracker_domain);
        link.href
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn href(raw: &str) -> String {
        clean_link(raw).map(|link| link.href).unwrap_or_default()
    }

    #[test]
    fn unwraps_common_wrappers() {
        assert_eq!(
            href("https://www.google.com/url?q=https://example.com/a%3Fb%3D1&sa=D&ust=1"),
            "https://example.com/a?b=1"
        );
        assert_eq!(
            href(
                "https://nam12.safelinks.protection.outlook.com/?url=https%3A%2F%2Fexample.org%2Fdoc%3Futm_source%3Dx%26id%3D7&data=05%7C01&reserved=0"
            ),
            "https://example.org/doc?id=7"
        );
        assert_eq!(
            href(
                "https://urldefense.proofpoint.com/v2/url?u=https-3A__example.com_path&d=DwMF&c=x"
            ),
            "https://example.com/path"
        );
        assert_eq!(
            href("https://urldefense.com/v3/__https://example.com/page__;!!ABC!def$"),
            "https://example.com/page"
        );
        assert_eq!(
            href("https://l.facebook.com/l.php?u=https%3A%2F%2Fexample.net%2F&h=AT0"),
            "https://example.net/"
        );
    }

    #[test]
    fn decodes_mandrill_payload() {
        let inner =
            r#"{"u":1,"v":1,"url":"https:\/\/shop.example.com\/sale?utm_campaign=x","id":"a"}"#;
        let outer = serde_json::json!({ "p": inner }).to_string();
        let encoded = URL_SAFE_NO_PAD.encode(outer);
        let link = clean_link(&format!(
            "https://mandrillapp.com/track/click/123/shop.example.com?p={encoded}"
        ))
        .unwrap();
        assert_eq!(link.href, "https://shop.example.com/sale");
        assert_eq!(link.unwrapped, 1);
        assert_eq!(link.params_stripped, 1);
        assert_eq!(link.tracker_domain.as_deref(), Some("mandrillapp.com"));
    }

    #[test]
    fn strips_params_and_keeps_others_verbatim() {
        let link = clean_link(
            "https://example.com/p?id=1&utm_source=news&utm_medium=email&q=a%20b&fbclid=xyz",
        )
        .unwrap();
        assert_eq!(link.href, "https://example.com/p?id=1&q=a%20b");
        assert_eq!(link.params_stripped, 3);
        assert_eq!(
            href("https://example.com/?utm_source=a"),
            "https://example.com/"
        );
        assert_eq!(href("https://example.com/?u=1"), "https://example.com/?u=1");
        assert_eq!(
            href("https://youtu.be/abc?si=track"),
            "https://youtu.be/abc"
        );
    }

    #[test]
    fn rejects_dangerous_schemes() {
        assert_eq!(clean_link("javascript:alert(1)"), None);
        assert_eq!(clean_link(" JaVa\tScRiPt:alert(1)"), None);
        assert_eq!(clean_link("data:text/html,x"), None);
        assert_eq!(clean_link("vbscript:x"), None);
        assert_eq!(
            clean_link("https://www.google.com/url?q=javascript:alert(1)"),
            None
        );
        assert_eq!(href("mailto:a@example.com"), "mailto:a@example.com");
    }

    #[test]
    fn records_tracker_domain_for_unrecoverable_clicks() {
        let link = clean_link("https://example.us1.list-manage.com/track/click?u=abc&id=def&e=123")
            .unwrap();
        assert_eq!(link.unwrapped, 0);
        assert_eq!(link.tracker_domain.as_deref(), Some("list-manage.com"));
    }
}
