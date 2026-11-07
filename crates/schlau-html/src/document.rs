use crate::options::{BodyKind, ColorScheme, Options, RemotePolicy, Sanitized, Theme};
use crate::urls::escape_html;

fn csp_for(remote: RemotePolicy) -> String {
    let remote_images = if remote.allows_remote() {
        " https:"
    } else {
        ""
    };
    [
        "default-src 'none'".to_owned(),
        "script-src 'none'".to_owned(),
        "object-src 'none'".to_owned(),
        "frame-src 'none'".to_owned(),
        "child-src 'none'".to_owned(),
        "connect-src 'none'".to_owned(),
        "media-src 'none'".to_owned(),
        "manifest-src 'none'".to_owned(),
        "worker-src 'none'".to_owned(),
        "base-uri 'none'".to_owned(),
        "form-action 'none'".to_owned(),
        "style-src 'unsafe-inline'".to_owned(),
        format!("img-src data: schlau-cid:{remote_images}"),
        "font-src data:".to_owned(),
    ]
    .join("; ")
}

/// The Content-Security-Policy to inject as a `<meta http-equiv>` tag: nothing executes, styles
/// are inline only, images come from `data:` and `schlau-cid:` (plus `https:` when remote
/// content is allowed), fonts from `data:`.
pub fn content_security_policy(options: &Options) -> String {
    csp_for(options.remote)
}

struct Palette<'a> {
    text: &'a str,
    background: &'a str,
    link: &'a str,
}

fn palette_rules(palette: &Palette<'_>) -> String {
    format!(
        "body{{color:{text};background:{background}}}a{{color:{link}}}",
        text = palette.text,
        background = palette.background,
        link = palette.link,
    )
}

fn css_value(value: &str) -> String {
    value
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | '{' | '}' | ';' | '\\' | '\n' | '\r'))
        .collect()
}

fn base_stylesheet(theme: &Theme, kind: BodyKind) -> String {
    let font = css_value(&theme.font_family);
    let mono = css_value(&theme.monospace_family);
    let light = Palette {
        text: &css_value(&theme.text_color),
        background: &css_value(&theme.background_color),
        link: &css_value(&theme.link_color),
    };
    let dark_text = css_value(&theme.dark_text_color);
    let dark_background = css_value(&theme.dark_background_color);
    let dark_link = css_value(&theme.dark_link_color);
    let dark = Palette {
        text: &dark_text,
        background: &dark_background,
        link: &dark_link,
    };
    let common = format!(
        "html{{-webkit-text-size-adjust:100%;text-size-adjust:100%}}\
         body{{margin:0;padding:16px;font-family:{font};font-size:{size}px;line-height:1.5;\
         overflow-wrap:break-word;word-wrap:break-word;overflow-x:auto}}\
         img{{max-width:100%;border:0}}\
         img:not([data-schlau-blocked]){{height:auto}}\
         img[data-schlau-blocked]{{display:inline-block;min-width:16px;min-height:16px;\
         background:repeating-linear-gradient(45deg,rgba(128,128,128,.12) 0 6px,rgba(128,128,128,.2) 6px 12px);\
         outline:1px dashed rgba(128,128,128,.4);outline-offset:-1px}}\
         pre,code,kbd,samp,tt{{font-family:{mono}}}\
         pre{{white-space:pre-wrap}}\
         blockquote[type=cite],.gmail_quote>blockquote{{margin:0 0 0 .4em;padding-left:.8em;\
         border-left:2px solid rgba(128,128,128,.45)}}",
        size = theme.font_size_px,
    );
    match kind {
        BodyKind::Html => format!(
            "{common}:root{{color-scheme:light}}{}",
            palette_rules(&light)
        ),
        BodyKind::PlainText => {
            let plain = ".schlau-plain{white-space:pre-wrap}\
                 .schlau-quote{margin:.25em 0 .25em .2em;padding-left:.8em;border-left:2px solid rgba(128,128,128,.5);opacity:.85}\
                 .schlau-signature{opacity:.65}";
            let scheme = match theme.color_scheme {
                ColorScheme::Light => {
                    format!(":root{{color-scheme:light}}{}", palette_rules(&light))
                }
                ColorScheme::Dark => format!(":root{{color-scheme:dark}}{}", palette_rules(&dark)),
                ColorScheme::Auto => format!(
                    ":root{{color-scheme:light dark}}{}@media (prefers-color-scheme:dark){{{}}}",
                    palette_rules(&light),
                    palette_rules(&dark)
                ),
            };
            format!("{common}{plain}{scheme}")
        }
    }
}

/// A complete HTML document for the web view: CSP and referrer meta tags first, then a base
/// stylesheet, then the sanitized body. It references no external resources.
pub fn wrap_document(sanitized: &Sanitized, theme: &Theme) -> String {
    let csp = csp_for(sanitized.remote);
    let color_scheme = match (sanitized.kind, theme.color_scheme) {
        (BodyKind::Html, _) | (BodyKind::PlainText, ColorScheme::Light) => "light",
        (BodyKind::PlainText, ColorScheme::Dark) => "dark",
        (BodyKind::PlainText, ColorScheme::Auto) => "light dark",
    };
    let body_class = match sanitized.kind {
        BodyKind::Html => "schlau-html",
        BodyKind::PlainText => "schlau-text",
    };
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"{color_scheme}\">\
         <style>{style}</style></head><body class=\"{body_class}\">{body}</body></html>",
        csp = escape_html(&csp),
        style = base_stylesheet(theme, sanitized.kind),
        body = sanitized.html,
    )
}
