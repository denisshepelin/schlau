use std::io::{Cursor, Read};

use quick_xml::Reader;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::Event;
use zip::ZipArchive;

use crate::text::finish;
use crate::{ExtractError, Extracted, Limits};

const MAX_ENTRY_BYTES: u64 = 20 * 1024 * 1024;
const MAX_ENTRIES: usize = 2_000;

/// Which XML elements carry text and which ones break lines.
struct XmlRules {
    /// Text is only collected inside these elements; empty means everywhere.
    text: &'static [&'static str],
    /// Closing one of these emits a newline.
    block: &'static [&'static str],
    /// An empty or opening one of these emits a space.
    space: &'static [&'static str],
    /// An empty one of these emits a newline.
    line_break: &'static [&'static str],
}

const DOCX: XmlRules = XmlRules {
    text: &["t"],
    block: &["p", "tr"],
    space: &["tab", "tc"],
    line_break: &["br", "cr"],
};

const PPTX: XmlRules = XmlRules {
    text: &["t"],
    block: &["p"],
    space: &[],
    line_break: &["br"],
};

const XLSX: XmlRules = XmlRules {
    text: &["t"],
    block: &["si", "is"],
    space: &[],
    line_break: &[],
};

const ODT: XmlRules = XmlRules {
    text: &[],
    block: &["p", "h", "list-item", "table-row"],
    space: &["s", "tab", "table-cell"],
    line_break: &["line-break"],
};

enum Format {
    Docx,
    Pptx,
    Xlsx,
    Odt,
}

type Archive<'b> = ZipArchive<Cursor<&'b [u8]>>;

fn malformed(err: impl std::fmt::Display) -> ExtractError {
    ExtractError::Malformed(err.to_string())
}

pub(crate) fn extract(bytes: &[u8], limits: &Limits) -> Result<Extracted, ExtractError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(malformed)?;
    if archive.len() > MAX_ENTRIES {
        return Err(ExtractError::Malformed(format!(
            "archive has {} entries",
            archive.len()
        )));
    }
    let names: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .map(|name| name.into_owned())
        .collect();
    let has = |name: &str| names.iter().any(|n| n == name);
    let format = if has("word/document.xml") {
        Format::Docx
    } else if has("ppt/presentation.xml") {
        Format::Pptx
    } else if has("xl/workbook.xml") {
        Format::Xlsx
    } else if has("content.xml") {
        Format::Odt
    } else {
        return Err(ExtractError::Unsupported(
            "zip archive is not a known office document".to_owned(),
        ));
    };
    let budget = limits.max_text_chars.saturating_mul(2);
    let raw = match format {
        Format::Docx => entries_text(
            &mut archive,
            &["word/document.xml".to_owned()],
            &DOCX,
            budget,
        ),
        Format::Pptx => entries_text(
            &mut archive,
            &numbered(&names, "ppt/slides/slide"),
            &PPTX,
            budget,
        ),
        Format::Xlsx => {
            let entries: Vec<String> = std::iter::once("xl/sharedStrings.xml".to_owned())
                .filter(|n| has(n))
                .chain(numbered(&names, "xl/worksheets/sheet"))
                .collect();
            entries_text(&mut archive, &entries, &XLSX, budget)
        }
        Format::Odt => entries_text(&mut archive, &["content.xml".to_owned()], &ODT, budget),
    }?;
    Ok(Extracted {
        text: finish(&raw, 1, limits),
        images: Vec::new(),
    })
}

/// Entries named `{prefix}{N}.xml`, ordered by `N`.
fn numbered(names: &[String], prefix: &str) -> Vec<String> {
    let mut found: Vec<(u32, &String)> = names
        .iter()
        .filter_map(|name| {
            let n = name
                .strip_prefix(prefix)?
                .strip_suffix(".xml")?
                .parse()
                .ok()?;
            Some((n, name))
        })
        .collect();
    found.sort_by_key(|(n, _)| *n);
    found.into_iter().map(|(_, name)| name.clone()).collect()
}

fn entries_text(
    archive: &mut Archive<'_>,
    entries: &[String],
    rules: &XmlRules,
    budget: usize,
) -> Result<String, ExtractError> {
    entries.iter().try_fold(String::new(), |mut acc, name| {
        if acc.len() > budget {
            return Ok(acc);
        }
        let xml = read_entry(archive, name)?;
        acc.push_str(&xml_text(&xml, rules, budget.saturating_sub(acc.len()))?);
        acc.push('\n');
        Ok(acc)
    })
}

fn read_entry(archive: &mut Archive<'_>, name: &str) -> Result<String, ExtractError> {
    let entry = archive.by_name(name).map_err(malformed)?;
    let mut buf = Vec::new();
    entry
        .take(MAX_ENTRY_BYTES)
        .read_to_end(&mut buf)
        .map_err(malformed)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, local)| local)
}

fn is_in(name: &str, set: &[&str]) -> bool {
    set.contains(&name)
}

/// Collects text from an OOXML/ODF part, stopping once `budget` bytes are gathered.
fn xml_text(xml: &str, rules: &XmlRules, budget: usize) -> Result<String, ExtractError> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut text_depth = 0usize;
    let collect_all = rules.text.is_empty();
    while out.len() <= budget {
        let event = match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(event) => event,
            Err(err) if out.is_empty() => return Err(malformed(err)),
            Err(_) => break,
        };
        let capturing = collect_all || text_depth > 0;
        match event {
            Event::Start(e) => {
                let name = local(e.name().as_ref()).to_owned();
                if is_in(&name, rules.text) {
                    text_depth += 1;
                }
                if is_in(&name, rules.space) {
                    out.push(' ');
                }
            }
            Event::End(e) => {
                let name = local(e.name().as_ref()).to_owned();
                if is_in(&name, rules.text) {
                    text_depth = text_depth.saturating_sub(1);
                }
                if is_in(&name, rules.block) {
                    out.push('\n');
                }
            }
            Event::Empty(e) => {
                let name = local(e.name().as_ref()).to_owned();
                if is_in(&name, rules.space) {
                    out.push(' ');
                } else if is_in(&name, rules.line_break) || is_in(&name, rules.block) {
                    out.push('\n');
                }
            }
            Event::Text(t) if capturing => out.push_str(&t.xml10_content()),
            Event::CData(t) if capturing => out.push_str(&t),
            Event::GeneralRef(r) if capturing => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    out.push(c);
                } else if let Some(s) = resolve_predefined_entity(&r) {
                    out.push_str(s);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docx_runs_and_paragraphs() {
        let xml = r#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:t xml:space="preserve"> world &amp; co</w:t></w:r></w:p><w:p><w:r><w:t>Second</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(
            xml_text(xml, &DOCX, usize::MAX).unwrap(),
            "Hello world & co\nSecond\n"
        );
    }

    #[test]
    fn odt_spaces() {
        let xml = r#"<office:document-content xmlns:office="o" xmlns:text="t"><office:body><office:text><text:p>A<text:s/>B</text:p><text:h>Title</text:h></office:text></office:body></office:document-content>"#;
        assert_eq!(xml_text(xml, &ODT, usize::MAX).unwrap(), "A B\nTitle\n");
    }

    #[test]
    fn numbered_sorts_numerically() {
        let names = [
            "ppt/slides/slide10.xml",
            "ppt/slides/slide2.xml",
            "ppt/slides/_rels/slide1.xml.rels",
            "ppt/slides/slide1.xml",
        ]
        .map(String::from);
        assert_eq!(
            numbered(&names, "ppt/slides/slide"),
            [
                "ppt/slides/slide1.xml",
                "ppt/slides/slide2.xml",
                "ppt/slides/slide10.xml"
            ]
        );
    }
}
