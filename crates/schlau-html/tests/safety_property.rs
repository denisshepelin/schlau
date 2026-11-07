mod common;

use std::sync::LazyLock;

use common::{FIXTURES, elements, fixture};
use regex::Regex;
use schlau_html::{Options, RemotePolicy, Theme, sanitize, wrap_document};

const FRAGMENTS: &[&str] = &[
    "<script>alert(1)</script>",
    "<SCRIPT SRC=//evil.example/x.js></SCRIPT>",
    "<scr<script>ipt>alert(1)</script>",
    "<img src=x onerror=alert(1)>",
    "<img src=\"https://evil.example/a.png\" width=600>",
    "<img src='//evil.example/b.png'>",
    "<img srcset=\"https://evil.example/1.png 1x, data:image/png;base64,AA 2x\">",
    "<img src=\"java\tscript:alert(1)\">",
    "<img src=\"&#106;avascript:alert(1)\">",
    "<a href=\"javascript:alert(1)\">x</a>",
    "<a href=\"&#x6A;avascript&#58;alert(1)\">x</a>",
    "<a href=\"https://www.google.com/url?q=javascript%3Aalert(1)\">x</a>",
    "<a href=\"https://example.com/?utm_source=x\" onclick=\"x()\">ok</a>",
    "<div style=\"background:url(https://evil.example/c.gif)\">",
    "<div style=\"background:url(&quot;//evil.example/d.gif&quot;)\">",
    "<div style=\"background:u&#x72;l(https://evil.example/e.gif)\">",
    "<div style=\"background:\\75 rl(https://evil.example/f.gif)\">",
    "<div style=\"width:expression(alert(1))\">",
    "<table background=\"https://evil.example/t.gif\"><tr><td background=//evil.example/u.gif>",
    "<style>@import 'https://evil.example/s.css'; p{background:url(https://evil.example/g.gif)}</style>",
    "<style>p{background:url(//evil.example/h.gif)}</style>",
    "<style>p::after{content:'</style><img src=https://evil.example/i.gif onerror=alert(1)>'}</style>",
    "<svg><style><img src=https://evil.example/j.gif></style></svg>",
    "<math><mi><style><img src=https://evil.example/k.gif onerror=alert(1)></style></mi></math>",
    "<noscript><p title=\"</noscript><img src=x onerror=alert(1)>\"></noscript>",
    "<iframe srcdoc=\"<script>alert(1)</script>\"></iframe>",
    "<object data=\"https://evil.example/o.swf\"></object>",
    "<meta http-equiv=refresh content=\"0;url=https://evil.example\">",
    "<base href=\"https://evil.example/\">",
    "<link rel=stylesheet href=\"https://evil.example/l.css\">",
    "<form action=\"https://evil.example\"><input type=submit formaction=\"https://evil.example\"></form>",
    "<video poster=\"https://evil.example/p.jpg\"></video>",
    "<body background=\"https://evil.example/body.gif\" onload=\"alert(1)\">",
    "<p>plain text https://evil.example/not-loaded</p>",
    "<!--<img src=https://evil.example/comment.gif>-->",
    "<![CDATA[<img src=https://evil.example/cdata.gif>]]>",
    "<p",
    "<div>",
    "</div></table>",
    "<table><tr><td>",
    "<blockquote type=cite>",
    "<img src=\"cid:part1@example\">",
    "<img src=\"data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=\">",
    "\u{0}<img src=x>",
    "<a href=\"//evil.example/proto-relative\">p</a>",
    "<a href=\"https://l.facebook.com/l.php?u=data%3Atext%2Fhtml%2C%3Cscript%3E\">fb</a>",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn random_document(rng: &mut Rng) -> String {
    (0..1 + rng.below(12))
        .map(|_| FRAGMENTS[rng.below(FRAGMENTS.len())])
        .collect()
}

fn spliced_fixture(rng: &mut Rng, corpus: &[String]) -> String {
    let source = &corpus[rng.below(corpus.len())];
    let boundaries: Vec<usize> = (0..4)
        .map(|_| {
            let mut at = rng.below(source.len().max(1));
            while !source.is_char_boundary(at) {
                at -= 1;
            }
            at
        })
        .collect();
    let (a, b) = (
        boundaries[0].min(boundaries[1]),
        boundaries[0].max(boundaries[1]),
    );
    let (c, d) = (
        boundaries[2].min(boundaries[3]),
        boundaries[2].max(boundaries[3]),
    );
    format!(
        "{}{}{}{}",
        &source[..a],
        FRAGMENTS[rng.below(FRAGMENTS.len())],
        &source[c..d],
        &source[b..]
    )
}

static EVENT_HANDLER_IN_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)<[a-z][^>]*[\s/]on[a-z]+\s*="#).expect("regex"));
static QUOTED_VALUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"="[^"]*""#).expect("regex"));
static CSS_STRING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""[^"]*"|'[^']*'"#).expect("regex"));
static CSS_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)url\(\s*(?:"([^"]*)"|'([^']*)'|([^)]*))"#).expect("regex"));

fn compact(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn resource_allowed(url: &str, remote: RemotePolicy) -> bool {
    let url = compact(url);
    url.starts_with("data:image/")
        || url.starts_with("schlau-cid:")
        || (remote != RemotePolicy::Block && url.starts_with("https://"))
}

fn assert_safe(input: &str, output: &str, remote: RemotePolicy) {
    let fail = |why: &str| panic!("{why}\n--- input ---\n{input}\n--- output ---\n{output}");
    let lower = output.to_ascii_lowercase();
    if lower.contains("<script") {
        fail("script tag");
    }
    if EVENT_HANDLER_IN_TAG.is_match(&QUOTED_VALUE.replace_all(output, "=\"\"")) {
        fail("event handler attribute");
    }
    for element in elements(output) {
        if [
            "script", "iframe", "object", "embed", "form", "input", "base", "meta", "link", "svg",
            "math", "video", "audio", "frame", "button",
        ]
        .contains(&element.tag.as_str())
        {
            fail(&format!("forbidden element {}", element.tag));
        }
        for (name, value) in &element.attrs {
            let value_compact = compact(value);
            if name.starts_with("on") {
                fail(&format!("attribute {name}"));
            }
            if value_compact.contains("javascript:") || value_compact.contains("vbscript:") {
                fail(&format!("script url in {name}"));
            }
            match name.as_str() {
                "src" | "background" | "poster" | "data" | "action" | "formaction" | "ping"
                | "xlink:href" => {
                    if !resource_allowed(value, remote) {
                        fail(&format!("resource {name}={value}"));
                    }
                }
                "srcset" => {
                    for candidate in value.split(',') {
                        let url = candidate.split_whitespace().next().unwrap_or_default();
                        if !resource_allowed(url, remote) {
                            fail(&format!("srcset candidate {url}"));
                        }
                    }
                }
                "href" => {
                    let ok = ["https://", "http://", "mailto:", "tel:", "#"]
                        .iter()
                        .any(|p| value_compact.starts_with(p));
                    if !ok {
                        fail(&format!("href {value}"));
                    }
                }
                _ => {}
            }
        }
        let css: Vec<&str> = element
            .attr("style")
            .into_iter()
            .chain((element.tag == "style").then_some(element.text.as_str()))
            .collect();
        for sheet in css {
            let sheet_lower = CSS_STRING.replace_all(sheet, "\"\"").to_ascii_lowercase();
            if sheet_lower.contains("expression(")
                || sheet_lower.contains("@import")
                || sheet_lower.contains("image-set(")
            {
                fail("dangerous css");
            }
            for capture in CSS_URL.captures_iter(sheet) {
                let url = capture
                    .iter()
                    .skip(1)
                    .flatten()
                    .next()
                    .map_or("", |m| m.as_str());
                if !resource_allowed(url, remote) {
                    fail(&format!("css url {url}"));
                }
            }
        }
    }
}

fn check(input: &str) {
    for remote in [
        RemotePolicy::Block,
        RemotePolicy::AllowExceptTrackers,
        RemotePolicy::Allow,
    ] {
        let out = sanitize(input, &Options::with_remote(remote));
        assert_safe(input, &out.html, remote);
        let document = wrap_document(&out, &Theme::default());
        let head = &document[..document.find("</head>").expect("head")];
        assert!(
            !head.contains("://") && !head.contains("url("),
            "document head references the network"
        );
    }
}

#[test]
fn fixtures_are_safe_under_every_policy() {
    FIXTURES.iter().for_each(|name| check(&fixture(name)));
}

#[test]
fn every_fragment_alone_is_safe() {
    FRAGMENTS.iter().for_each(|fragment| check(fragment));
}

#[test]
fn random_fragment_soup_is_safe() {
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    (0..1500).for_each(|_| check(&random_document(&mut rng)));
}

#[test]
fn spliced_fixtures_are_safe() {
    let corpus: Vec<String> = FIXTURES
        .iter()
        .filter(|name| !name.contains("100k"))
        .map(|name| fixture(name))
        .collect();
    let mut rng = Rng(0x0dd_ba11_cafe_f00d);
    (0..400).for_each(|_| check(&spliced_fixture(&mut rng, &corpus)));
}

#[test]
fn deep_nesting_does_not_overflow() {
    let deep = "<div>".repeat(20_000) + "text" + &"</div>".repeat(20_000);
    let out = sanitize(&deep, &Options::default());
    assert!(out.html.contains("text"));
    let _ = schlau_html::to_plain_text(&deep, true);
    let _ = schlau_html::to_blocks(&deep);
}
