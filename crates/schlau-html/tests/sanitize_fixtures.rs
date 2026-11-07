mod common;

use common::{anchors, elements, fixture, receipt_cids};
use schlau_html::{Options, RemotePolicy, sanitize};

#[test]
fn newsletter_blocks_remote_content_and_pixel() {
    let out = sanitize(&fixture("mailchimp_newsletter.html"), &Options::default());
    let report = &out.report;
    assert_eq!(report.pixels_removed, 1);
    assert!(
        report
            .tracker_domains
            .contains(&"list-manage.com".to_owned()),
        "{report:?}"
    );
    assert!(report.remote_blocked >= 9, "{report:?}");
    assert!(!out.html.contains("mcusercontent.com"));
    assert!(!out.html.contains("track/open.php"));
    assert!(!out.html.contains("@import"));
    assert!(!out.html.contains("fonts.googleapis.com"));

    let logo = elements(&out.html)
        .into_iter()
        .find(|e| e.tag == "img" && e.attr("alt") == Some("The Weekly Roast"))
        .expect("logo kept as placeholder");
    assert_eq!(logo.attr("width"), Some("564"));
    assert_eq!(logo.attr("height"), Some("120"));
    assert_eq!(logo.attr("src"), None);
    assert!(logo.attr("data-schlau-blocked").is_some());
}

#[test]
fn newsletter_links_are_cleaned_and_isolated() {
    let out = sanitize(&fixture("mailchimp_newsletter.html"), &Options::default());
    assert_eq!(out.report.params_stripped, 13);
    for param in ["utm_", "mc_cid", "mc_eid", "fbclid", "gclid"] {
        assert!(!out.html.contains(param), "{param} left in output");
    }
    let links = anchors(&out.html);
    assert!(
        links
            .iter()
            .any(|a| a.attr("href") == Some("https://shop.roastery.example/products/guji-hambela"))
    );
    assert!(
        links
            .iter()
            .any(|a| a.attr("href") == Some("https://roastery.example/guides/v60?ref=menu"))
    );
    for link in links.iter().filter(|a| a.attr("href").is_some()) {
        assert_eq!(link.attr("target"), Some("_blank"));
        assert_eq!(link.attr("rel"), Some("noopener noreferrer"));
    }
}

#[test]
fn newsletter_allow_keeps_images_but_not_pixels() {
    let out = sanitize(
        &fixture("mailchimp_newsletter.html"),
        &Options::with_remote(RemotePolicy::Allow),
    );
    assert_eq!(out.report.pixels_removed, 1);
    assert_eq!(out.report.remote_blocked, 0);
    assert!(
        out.html
            .contains("src=\"https://mcusercontent.com/abc123/images/logo.png\"")
    );
    assert!(out.html.contains("2x.jpg 2x"));
    assert!(
        out.html
            .contains("url(&quot;https://mcusercontent.com/abc123/images/cta-bg.png&quot;)")
    );
    assert!(!out.html.contains("list-manage.com/track/open"));
    assert!(!out.html.contains("http://cdn-images"));
}

#[test]
fn newsletter_allow_except_trackers_blocks_esp_hosting() {
    let out = sanitize(
        &fixture("mailchimp_newsletter.html"),
        &Options::with_remote(RemotePolicy::AllowExceptTrackers),
    );
    assert!(!out.html.contains("mcusercontent.com"));
    assert!(out.report.remote_blocked >= 9);
    assert!(
        out.report
            .tracker_domains
            .contains(&"mcusercontent.com".to_owned())
    );
    assert!(
        out.report
            .tracker_domains
            .contains(&"mailchimp.com".to_owned())
    );
}

#[test]
fn gmail_reply_unwraps_google_redirect() {
    let out = sanitize(&fixture("gmail_reply.html"), &Options::default());
    assert_eq!(out.report.links_unwrapped, 1);
    let hrefs: Vec<String> = anchors(&out.html)
        .iter()
        .filter_map(|a| a.attr("href").map(str::to_owned))
        .collect();
    assert!(
        hrefs.contains(&"https://calendar.example.com/event/123".to_owned()),
        "{hrefs:?}"
    );
    assert!(hrefs.contains(&"mailto:jonas@example.de".to_owned()));
    assert!(out.html.contains("class=\"gmail_quote\""));
}

#[test]
fn outlook_safelinks_and_proofpoint_are_unwrapped() {
    let out = sanitize(&fixture("outlook_safelinks.html"), &Options::default());
    assert_eq!(out.report.links_unwrapped, 2);
    assert_eq!(out.report.params_stripped, 1);
    let hrefs: Vec<String> = anchors(&out.html)
        .iter()
        .filter_map(|a| a.attr("href").map(str::to_owned))
        .collect();
    assert!(hrefs.contains(&"https://contoso.sharepoint.com/sites/Finance/Shared%20Documents/Q4-budget.xlsx?d=w1234".to_owned()), "{hrefs:?}");
    assert!(
        hrefs.contains(&"https://vendor.example.com/terms?id=9".to_owned()),
        "{hrefs:?}"
    );
    assert!(!out.html.contains("safelinks"));
    assert!(!out.html.contains("originalsrc"));
    assert!(out.report.tracker_domains.is_empty());
}

#[test]
fn receipt_rewrites_cid_references() {
    let options = Options::default().with_cid_urls(receipt_cids());
    let out = sanitize(&fixture("receipt_cid.html"), &options);
    assert!(out.html.contains("src=\"schlau-cid://blob-logo\""));
    assert!(out.html.contains("src=\"schlau-cid://blob-item-1\""));
    assert!(out.html.contains("src=\"schlau-cid://blob-item-2\""));
    assert!(out.html.contains("url(\"schlau-cid://blob-logo-bg\")"));
    assert!(out.html.contains("src=\"data:image/png;base64,"));
    assert!(
        !out.html.replace("schlau-cid:", "").contains("cid:"),
        "unresolved cid left: {}",
        out.html
    );
    assert!(!out.html.contains("Unknown attachment"));
    assert!(out.report.is_clean(), "{:?}", out.report);
}

#[test]
fn receipt_without_map_drops_cid_images() {
    let out = sanitize(&fixture("receipt_cid.html"), &Options::default());
    assert!(!out.html.to_ascii_lowercase().contains("cid:"));
    assert!(out.html.contains("$84.94"));
}

#[test]
fn hostile_email_is_neutralized() {
    let out = sanitize(&fixture("hostile.html"), &Options::default());
    let lower = out.html.to_ascii_lowercase();
    for needle in [
        "<script",
        "javascript:",
        "vbscript:",
        "<iframe",
        "<object",
        "<embed",
        "<form",
        "<input",
        "<meta",
        "<base",
        "<link",
        "<svg",
        "<math",
        "<video",
        "<audio",
        "expression(",
        "@import",
        "-moz-binding",
        "onerror",
        "onload",
        "onclick",
        "onmouseover",
        "ping=",
        "evil.example/bg",
        "escaped.gif",
        "set.png",
        "div-bg.gif",
        "quoted.gif",
        "table-bg.gif",
        "data:text",
    ] {
        assert!(!lower.contains(needle), "{needle} survived:\n{}", out.html);
    }
    assert_eq!(out.report.pixels_removed, 1);
    assert_eq!(out.report.tracker_domains, vec!["evil.example".to_owned()]);
    assert!(out.html.contains("is shown as text"));
    assert!(out.html.contains("href=\"https://evil.example/ok\""));
}
