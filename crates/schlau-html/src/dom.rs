use html5ever::tendril::TendrilSink;
use html5ever::{ParseOpts, parse_document};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::css::declarations;

/// Nesting beyond this depth is ignored, which bounds recursion on hostile input.
pub(crate) const MAX_DEPTH: usize = 200;

pub(crate) fn parse(html: &str) -> RcDom {
    parse_document(RcDom::default(), ParseOpts::default()).one(html)
}

pub(crate) fn tag_name(node: &Handle) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(&*name.local),
        _ => None,
    }
}

pub(crate) fn attribute(node: &Handle, attribute: &str) -> Option<String> {
    match &node.data {
        NodeData::Element { attrs, .. } => attrs
            .borrow()
            .iter()
            .find(|attr| &*attr.name.local == attribute)
            .map(|attr| attr.value.to_string()),
        _ => None,
    }
}

pub(crate) fn has_class(node: &Handle, class: &str) -> bool {
    attribute(node, "class")
        .is_some_and(|classes| classes.split_ascii_whitespace().any(|c| c == class))
}

const SKIPPED: &[&str] = &[
    "head", "style", "script", "title", "template", "noscript", "meta", "link", "object", "embed",
    "iframe", "frame", "frameset", "svg", "math", "select", "textarea", "audio", "video", "canvas",
    "map",
];

/// Elements whose content is never shown to a reader.
pub(crate) fn is_skipped(node: &Handle) -> bool {
    tag_name(node).is_some_and(|tag| SKIPPED.contains(&tag))
        || attribute(node, "hidden").is_some()
        || attribute(node, "style").is_some_and(|style| {
            declarations(&style).any(|(name, value)| {
                (name == "display" && value == "none")
                    || (name == "visibility" && value == "hidden")
            })
        })
}

pub(crate) fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "body"
            | "caption"
            | "center"
            | "dd"
            | "details"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "html"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "summary"
            | "table"
            | "tbody"
            | "td"
            | "tfoot"
            | "th"
            | "thead"
            | "tr"
            | "ul"
    )
}

/// Characters email templates use for invisible padding (preheaders, spacing hacks).
pub(crate) fn is_invisible_char(c: char) -> bool {
    matches!(
        c,
        '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}' | '\u{034f}' | '\u{00ad}' | '\u{180e}'
    )
}

/// Text of a subtree, up to `limit` bytes, for quick heuristics.
pub(crate) fn text_prefix(node: &Handle, limit: usize) -> String {
    fn collect(node: &Handle, out: &mut String, limit: usize, depth: usize) {
        if out.len() >= limit || depth > MAX_DEPTH {
            return;
        }
        match &node.data {
            NodeData::Text { contents } => out.push_str(&contents.borrow()),
            NodeData::Element { .. } if is_skipped(node) => {}
            _ => node
                .children
                .borrow()
                .iter()
                .for_each(|child| collect(child, out, limit, depth + 1)),
        }
    }
    let mut out = String::new();
    collect(node, &mut out, limit, 0);
    out
}
