use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use ammonia::{Builder, UrlRelative};
use lol_html::html_content::{ContentType, Element};
use lol_html::{RewriteStrSettings, element, rewrite_str, text};

use crate::css::{CssContext, rewrite_css};
use crate::links::{clean_link, clean_link_counted};
use crate::options::{BodyKind, Options, Sanitized, Tally};
use crate::pixel::{ImageShape, is_invisible};
use crate::urls::{
    Resource, ResourceAction, classify_resource, decide_resource, decode_attr, escape_html,
    is_remote, record_pixel,
};

const TAGS: &[&str] = &[
    "a",
    "abbr",
    "acronym",
    "address",
    "area",
    "article",
    "aside",
    "b",
    "bdi",
    "bdo",
    "big",
    "blockquote",
    "br",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "font",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "main",
    "mark",
    "nav",
    "ol",
    "p",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "section",
    "small",
    "span",
    "strike",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "wbr",
];

const CLEAN_CONTENT_TAGS: &[&str] = &[
    "script", "title", "template", "object", "applet", "embed", "iframe", "frame", "frameset",
    "noembed", "noframes", "noscript", "select", "textarea", "math", "svg", "audio", "video",
    "canvas",
];

const GENERIC_ATTRIBUTES: &[&str] = &[
    "style",
    "class",
    "id",
    "dir",
    "lang",
    "title",
    "align",
    "valign",
    "width",
    "height",
    "bgcolor",
    "color",
    "border",
    "role",
    "hidden",
    "aria-label",
    "aria-hidden",
    "cellpadding",
    "cellspacing",
    "colspan",
    "rowspan",
    "nowrap",
];

const TAG_ATTRIBUTES: &[(&str, &[&str])] = &[
    ("a", &["href", "name", "hreflang"]),
    ("area", &["href", "alt", "shape", "coords"]),
    (
        "img",
        &[
            "src", "srcset", "alt", "border", "hspace", "vspace", "usemap",
        ],
    ),
    ("map", &["name"]),
    ("table", &["background", "summary", "frame", "rules"]),
    ("td", &["background", "headers", "scope", "abbr"]),
    ("th", &["background", "headers", "scope", "abbr"]),
    ("tr", &["background"]),
    ("tbody", &["background"]),
    ("thead", &["background"]),
    ("tfoot", &["background"]),
    ("font", &["face", "size"]),
    ("ol", &["start", "type", "reversed"]),
    ("ul", &["type"]),
    ("li", &["value", "type"]),
    ("blockquote", &["type"]),
    ("col", &["span"]),
    ("colgroup", &["span"]),
    ("time", &["datetime"]),
    ("details", &["open"]),
];

const URL_SCHEMES: &[&str] = &["http", "https", "mailto", "tel", "cid", "data"];

fn resource_is_valid(value: &str) -> bool {
    !matches!(classify_resource(value), Resource::Invalid)
}

fn srcset_candidates(value: &str) -> impl Iterator<Item = (&str, &str)> {
    value.split(',').filter_map(|candidate| {
        let candidate = candidate.trim();
        let (url, descriptor) = candidate
            .split_once(char::is_whitespace)
            .unwrap_or((candidate, ""));
        (!url.is_empty()).then_some((url, descriptor.trim()))
    })
}

fn join_srcset<'a>(candidates: impl Iterator<Item = (String, &'a str)>) -> Option<String> {
    let joined = candidates
        .map(|(url, descriptor)| {
            if descriptor.is_empty() {
                url
            } else {
                format!("{url} {descriptor}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    (!joined.is_empty()).then_some(joined)
}

fn filter_attribute<'a>(element: &str, attribute: &str, value: &'a str) -> Option<Cow<'a, str>> {
    match attribute {
        "src" => (element == "img" && resource_is_valid(value)).then_some(Cow::Borrowed(value)),
        "background" => resource_is_valid(value).then_some(Cow::Borrowed(value)),
        "srcset" => join_srcset(
            srcset_candidates(value)
                .filter(|(url, _)| resource_is_valid(url))
                .map(|(url, descriptor)| (url.to_owned(), descriptor)),
        )
        .map(Cow::Owned),
        "href" => clean_link(value).map(|_| Cow::Borrowed(value)),
        _ => Some(Cow::Borrowed(value)),
    }
}

fn keep_fragment(url: &str) -> Option<Cow<'_, str>> {
    url.starts_with('#').then_some(Cow::Borrowed(url))
}

static SANITIZER: LazyLock<Builder<'static>> = LazyLock::new(|| {
    let tag_attributes: HashMap<&str, HashSet<&str>> = TAG_ATTRIBUTES
        .iter()
        .map(|(tag, attributes)| (*tag, attributes.iter().copied().collect()))
        .collect();
    let mut builder = Builder::default();
    builder
        .tags(TAGS.iter().copied().collect())
        .clean_content_tags(CLEAN_CONTENT_TAGS.iter().copied().collect())
        .generic_attributes(GENERIC_ATTRIBUTES.iter().copied().collect())
        .generic_attribute_prefixes(HashSet::new())
        .tag_attributes(tag_attributes)
        .tag_attribute_values(HashMap::new())
        .set_tag_attribute_values(HashMap::new())
        .url_schemes(URL_SCHEMES.iter().copied().collect())
        .url_relative(UrlRelative::Custom(Box::new(keep_fragment)))
        .link_rel(None)
        .strip_comments(true)
        .attribute_filter(filter_attribute);
    builder
});

fn attr(el: &Element<'_, '_>, name: &str) -> Option<String> {
    el.get_attribute(name)
        .map(|value| decode_attr(&value).into_owned())
}

fn set_attr(el: &mut Element<'_, '_>, name: &str, value: &str) {
    el.set_attribute(name, &escape_html(value))
        .expect("attribute names used here are valid");
}

fn resolve_url(options: &Options, tally: &mut Tally) -> impl FnMut(&str) -> Option<String> {
    move |url| match decide_resource(url, options, tally) {
        ResourceAction::Keep(kept) => Some(kept),
        ResourceAction::Pixel | ResourceAction::Blocked | ResourceAction::Invalid => None,
    }
}

fn rewrite_srcset(el: &mut Element<'_, '_>, options: &Options, tally: &mut Tally) {
    let Some(srcset) = attr(el, "srcset") else {
        return;
    };
    let mut resolve = resolve_url(options, tally);
    let kept = join_srcset(
        srcset_candidates(&srcset)
            .filter_map(|(url, descriptor)| resolve(url).map(|kept| (kept, descriptor))),
    );
    match kept {
        Some(kept) => set_attr(el, "srcset", &kept),
        None => el.remove_attribute("srcset"),
    }
}

fn rewrite_image(el: &mut Element<'_, '_>, options: &Options, tally: &mut Tally) {
    let src = attr(el, "src");
    let (width, height, style) = (attr(el, "width"), attr(el, "height"), attr(el, "style"));
    let shape = ImageShape {
        width: width.as_deref(),
        height: height.as_deref(),
        style: style.as_deref(),
        hidden: el.has_attribute("hidden"),
    };
    if let Some(src) = src
        .as_deref()
        .filter(|src| is_remote(src) && is_invisible(shape))
    {
        record_pixel(src, tally);
        el.remove();
        return;
    }
    match src.map(|src| decide_resource(&src, options, tally)) {
        Some(ResourceAction::Keep(url)) => {
            set_attr(el, "src", &url);
            if is_remote(&url) {
                set_attr(el, "referrerpolicy", "no-referrer");
            }
        }
        Some(ResourceAction::Pixel) => {
            el.remove();
            return;
        }
        Some(ResourceAction::Blocked) => {
            el.remove_attribute("src");
            set_attr(el, "data-schlau-blocked", "");
        }
        Some(ResourceAction::Invalid) => el.remove_attribute("src"),
        None => {}
    }
    rewrite_srcset(el, options, tally);
    let renderable = ["src", "srcset", "data-schlau-blocked"]
        .iter()
        .any(|name| el.has_attribute(name));
    if !renderable {
        el.remove();
    }
}

fn rewrite_background(el: &mut Element<'_, '_>, options: &Options, tally: &mut Tally) {
    let kept = attr(el, "background").and_then(|url| resolve_url(options, tally)(&url));
    match kept {
        Some(url) => set_attr(el, "background", &url),
        None => el.remove_attribute("background"),
    }
}

fn rewrite_style_attribute(el: &mut Element<'_, '_>, options: &Options, tally: &mut Tally) {
    if el.removed() {
        return;
    }
    if let Some(style) = attr(el, "style") {
        let rewritten = rewrite_css(&style, CssContext::Inline, &mut resolve_url(options, tally));
        set_attr(el, "style", &rewritten);
    }
}

fn rewrite_link(el: &mut Element<'_, '_>, tally: &mut Tally) {
    match attr(el, "href").and_then(|href| clean_link_counted(&href, tally)) {
        Some(href) if href.starts_with('#') => {
            set_attr(el, "href", &href);
            el.remove_attribute("target");
        }
        Some(href) => {
            set_attr(el, "href", &href);
            set_attr(el, "target", "_blank");
        }
        None => {
            el.remove_attribute("href");
            el.remove_attribute("target");
        }
    }
    set_attr(el, "rel", "noopener noreferrer");
}

fn rewrite_for_privacy(html: &str, options: &Options) -> Option<(String, Tally)> {
    let tally = RefCell::new(Tally::default());
    let stylesheet = RefCell::new(String::new());
    let settings = RewriteStrSettings::new()
        .append_element_content_handler(element!("img", |el| {
            rewrite_image(el, options, &mut tally.borrow_mut());
            Ok(())
        }))
        .append_element_content_handler(element!("[background]", |el| {
            rewrite_background(el, options, &mut tally.borrow_mut());
            Ok(())
        }))
        .append_element_content_handler(element!("[style]", |el| {
            rewrite_style_attribute(el, options, &mut tally.borrow_mut());
            Ok(())
        }))
        .append_element_content_handler(element!("a, area", |el| {
            rewrite_link(el, &mut tally.borrow_mut());
            Ok(())
        }))
        .append_element_content_handler(text!("style", |chunk| {
            stylesheet.borrow_mut().push_str(chunk.as_str());
            if chunk.last_in_text_node() {
                let css = std::mem::take(&mut *stylesheet.borrow_mut());
                let rewritten = rewrite_css(
                    &css,
                    CssContext::Sheet,
                    &mut resolve_url(options, &mut tally.borrow_mut()),
                );
                chunk.replace(&rewritten, ContentType::Html);
            } else {
                chunk.remove();
            }
            Ok(())
        }));
    let output = rewrite_str(html, settings).ok()?;
    Some((output, tally.into_inner()))
}

/// Sanitizes untrusted email HTML for display and reports what was removed.
///
/// Scripts, event handlers, forms, frames, embedded objects, `<meta>`, `<base>`, `<link>` and
/// every URL scheme except `http(s)`, `mailto`, `tel`, `cid` and `data:image/*` (images only)
/// are removed. Remote resources follow [`Options::remote`], tracking pixels are always
/// removed, and links are unwrapped, stripped of tracking parameters and opened in a new
/// window without a referrer.
pub fn sanitize(html: &str, options: &Options) -> Sanitized {
    let cleaned = SANITIZER.clean(html).to_string();
    let (html, tally) = rewrite_for_privacy(&cleaned, options).unwrap_or_else(|| {
        let text = crate::plain::to_plain_text(&cleaned, false);
        (
            format!("<pre>{}</pre>", escape_html(&text)),
            Tally::default(),
        )
    });
    Sanitized {
        html,
        report: tally.into_report(),
        remote: options.remote,
        kind: BodyKind::Html,
    }
}
