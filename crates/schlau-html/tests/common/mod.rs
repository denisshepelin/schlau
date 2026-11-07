#![allow(dead_code)]

use std::collections::HashMap;

use html5ever::tendril::TendrilSink;
use html5ever::{ParseOpts, parse_document};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

pub const FIXTURES: &[&str] = &[
    "mailchimp_newsletter.html",
    "gmail_reply.html",
    "outlook_safelinks.html",
    "receipt_cid.html",
    "hostile.html",
    "marketing_100k.html",
];

pub fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("fixture {path}: {err}"))
}

pub fn receipt_cids() -> HashMap<String, String> {
    [
        ("logo@example.books", "schlau-cid://blob-logo"),
        ("logo-bg@example.books", "schlau-cid://blob-logo-bg"),
        ("item-1@example.books", "schlau-cid://blob-item-1"),
        ("<item-2@example.books>", "schlau-cid://blob-item-2"),
    ]
    .into_iter()
    .map(|(cid, url)| (cid.to_owned(), url.to_owned()))
    .collect()
}

#[derive(Debug, Clone)]
pub struct Element {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn collect(node: &Handle, out: &mut Vec<Element>) {
    if let NodeData::Element { name, attrs, .. } = &node.data {
        let text = node
            .children
            .borrow()
            .iter()
            .filter_map(|child| match &child.data {
                NodeData::Text { contents } => Some(contents.borrow().to_string()),
                _ => None,
            })
            .collect();
        out.push(Element {
            tag: name.local.to_string(),
            attrs: attrs
                .borrow()
                .iter()
                .map(|a| (a.name.local.to_string(), a.value.to_string()))
                .collect(),
            text,
        });
    }
    node.children
        .borrow()
        .iter()
        .for_each(|child| collect(child, out));
}

/// Every element of `html` as a browser would parse it.
pub fn elements(html: &str) -> Vec<Element> {
    let dom = parse_document(RcDom::default(), ParseOpts::default()).one(html);
    let mut out = Vec::new();
    collect(&dom.document, &mut out);
    out
}

pub fn anchors(html: &str) -> Vec<Element> {
    elements(html)
        .into_iter()
        .filter(|e| e.tag == "a")
        .collect()
}
