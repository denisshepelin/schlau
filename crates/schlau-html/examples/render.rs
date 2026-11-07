use std::collections::HashMap;

use schlau_html::{Options, RemotePolicy, Theme, sanitize, wrap_document};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: render <email.html> [block|allow|allow-except-trackers]");
        std::process::exit(2);
    };
    let remote = match args.next().as_deref() {
        Some("allow") => RemotePolicy::Allow,
        Some("allow-except-trackers") => RemotePolicy::AllowExceptTrackers,
        _ => RemotePolicy::Block,
    };
    let html = std::fs::read_to_string(&path).expect("readable input file");
    let options = Options::with_remote(remote).with_cid_urls(HashMap::new());
    let sanitized = sanitize(&html, &options);
    eprintln!("{:#?}", sanitized.report);
    println!("{}", wrap_document(&sanitized, &Theme::default()));
}
