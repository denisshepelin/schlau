use std::io::{Cursor, Write};
use std::path::PathBuf;

use schlau_extract::{ExtractError, Extracted, Kind, Limits, extract, kind};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

fn fixture(name: &str) -> Vec<u8> {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "../../fixtures/mailbox/blobs",
        name,
    ]
    .iter()
    .collect();
    std::fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

fn run(name: &str, mime: &str) -> Extracted {
    extract(Some(name), mime, &fixture(name), &Limits::default())
        .unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn pdf_text(name: &str) -> String {
    run(name, "application/pdf")
        .text
        .unwrap_or_else(|| panic!("{name}: no text"))
}

#[test]
fn photos_are_downscaled_jpegs() {
    [
        "bruno-beach.jpg",
        "bruno-stick.jpg",
        "lx-factory-terrace.jpg",
        "lx-factory-room.jpg",
    ]
    .iter()
    .for_each(|name| {
        let out = run(name, "image/jpeg");
        assert_eq!(out.text, None, "{name}");
        assert_eq!(out.images.len(), 1, "{name}");
        let img = &out.images[0];
        assert_eq!(img.mime, "image/jpeg", "{name}");
        assert!(img.bytes.starts_with(&[0xFF, 0xD8, 0xFF]), "{name}");
        assert!(
            img.width.max(img.height) <= 1024,
            "{name}: {}x{}",
            img.width,
            img.height
        );
        assert!(img.width.min(img.height) >= 32, "{name}");
        let decoded = image::load_from_memory(&img.bytes).expect("decodes");
        assert_eq!((decoded.width(), decoded.height()), (img.width, img.height));
    });
}

#[test]
fn png_is_flattened_to_jpeg() {
    let out = run("logo-sketches-v2.png", "image/png");
    assert_eq!(out.images.len(), 1);
    assert_eq!(out.images[0].mime, "image/jpeg");
    assert!(out.images[0].width.max(out.images[0].height) <= 1024);
}

#[test]
fn small_images_are_skipped() {
    let mut png = Vec::new();
    image::RgbImage::new(16, 16)
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let out = extract(Some("spacer.png"), "image/png", &png, &Limits::default()).unwrap();
    assert_eq!(out, Extracted::default());
}

#[test]
fn images_are_never_upscaled() {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(300, 200, image::Rgba([0, 0, 0, 0]))
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let out = extract(None, "image/png", &png, &Limits::default()).unwrap();
    let img = &out.images[0];
    assert_eq!((img.width, img.height), (300, 200));
    let decoded = image::load_from_memory(&img.bytes).unwrap().to_rgb8();
    assert!(
        decoded.pixels().all(|p| p.0.iter().all(|&c| c > 245)),
        "alpha flattened onto white"
    );
}

#[test]
fn invoice_pdf_text() {
    let text = pdf_text("invoice-INV-2026-4821.pdf");
    assert!(text.contains("INV-2026-4821"), "{text}");
    assert!(text.contains("Lumen Labs"), "{text}");
}

#[test]
fn ticket_pdf_text() {
    assert!(pdf_text("e-ticket-K7Q2ZP.pdf").contains("K7Q2ZP"));
}

#[test]
fn german_pdf_text() {
    let text = pdf_text("Nebenkostenabrechnung-2025.pdf");
    assert!(text.contains("Nachzahlung"), "{text}");
    assert!(text.contains("Heizkosten"), "{text}");
}

#[test]
fn text_pdfs_also_render_a_page() {
    let out = run("invoice-INV-2026-4821.pdf", "application/pdf");
    assert_eq!(out.images.len(), 1);
    let img = &out.images[0];
    assert_eq!(img.mime, "image/jpeg");
    assert!(
        (1000..=1024).contains(&img.width.max(img.height)),
        "{}x{}",
        img.width,
        img.height
    );
}

#[test]
fn scanned_pdf_yields_one_image_and_no_text() {
    let out = run("planning-agenda.pdf", "application/pdf");
    assert_eq!(out.text, None);
    assert_eq!(out.images.len(), 1);
    assert!(out.images[0].width.max(out.images[0].height) <= 1024);
}

#[test]
fn truncated_and_garbage_pdfs_are_errors() {
    let pdf = fixture("invoice-INV-2026-4821.pdf");
    let inputs: [&[u8]; 4] = [
        &pdf[..pdf.len() / 3],
        &pdf[..40],
        b"%PDF-1.7\n this is not a pdf at all",
        &[0xFFu8; 4096],
    ];
    inputs.iter().for_each(|bytes| {
        let result = extract(Some("x.pdf"), "application/pdf", bytes, &Limits::default());
        assert!(
            matches!(result, Err(ExtractError::Malformed(_))),
            "{result:?}"
        );
    });
    assert!(matches!(
        extract(
            Some("x.pdf"),
            "application/pdf",
            b"garbage",
            &Limits::default()
        ),
        Err(ExtractError::Malformed(_))
    ));
}

#[test]
fn decompression_bomb_is_rejected() {
    let mut png = Vec::new();
    image::GrayImage::new(64, 64)
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    png[16..24].copy_from_slice(&[0, 0, 0xC3, 0x50, 0, 0, 0xC3, 0x50]);
    let crc = crc32(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    let result = extract(None, "image/png", &png, &Limits::default());
    assert!(
        matches!(result, Err(ExtractError::Malformed(_))),
        "{result:?}"
    );
}

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |crc, &b| {
        (0..8).fold(crc ^ u32::from(b), |c, _| {
            if c & 1 == 1 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            }
        })
    })
}

#[test]
fn garbage_image_is_an_error() {
    assert!(matches!(
        extract(
            Some("x.jpg"),
            "image/jpeg",
            &[0xFF, 0xD8, 0xFF, 0, 1, 2, 3],
            &Limits::default()
        ),
        Err(ExtractError::Malformed(_))
    ));
}

#[test]
fn too_large_input_is_rejected() {
    let limits = Limits {
        max_bytes: 10,
        ..Limits::default()
    };
    assert!(matches!(
        extract(Some("a.txt"), "text/plain", &[b'a'; 11], &limits),
        Err(ExtractError::TooLarge(11))
    ));
}

#[test]
fn unsupported_type_is_rejected() {
    assert!(matches!(
        extract(
            Some("a.exe"),
            "application/x-msdownload",
            b"MZ",
            &Limits::default()
        ),
        Err(ExtractError::Unsupported(_))
    ));
}

#[test]
fn kind_from_mime_and_extension() {
    let cases = [
        (None, "image/JPEG", Some(Kind::Image)),
        (None, "image/webp", Some(Kind::Image)),
        (None, "application/pdf", Some(Kind::Pdf)),
        (None, "text/plain; charset=utf-8", Some(Kind::Text)),
        (None, "text/calendar; method=REQUEST", Some(Kind::Text)),
        (
            Some("Report.PDF"),
            "application/octet-stream",
            Some(Kind::Pdf),
        ),
        (Some("scan.pdf"), "", Some(Kind::Pdf)),
        (
            Some("photo.JPG"),
            "application/octet-stream",
            Some(Kind::Image),
        ),
        (
            Some("notes.md"),
            "application/octet-stream",
            Some(Kind::Text),
        ),
        (Some("invite.ics"), "", Some(Kind::Text)),
        (
            Some("letter.docx"),
            "application/octet-stream",
            Some(Kind::Office),
        ),
        (Some("deck.pptx"), "application/zip", Some(Kind::Office)),
        (
            None,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Some(Kind::Office),
        ),
        (
            None,
            "application/vnd.oasis.opendocument.text",
            Some(Kind::Office),
        ),
        (Some("blob.bin"), "application/octet-stream", None),
        (None, "application/octet-stream", None),
        (Some("a.pdf"), "text/html", None),
        (None, "video/mp4", None),
    ];
    cases.iter().for_each(|(name, mime, expected)| {
        assert_eq!(kind(*name, mime), *expected, "{name:?} {mime}");
    });
}

fn zip(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    entries.iter().for_each(|(name, body)| {
        writer
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(body.as_bytes()).unwrap();
    });
    writer.finish().unwrap().into_inner()
}

#[test]
fn docx_text() {
    let document = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Quarterly</w:t></w:r><w:r><w:t xml:space="preserve"> report for Lumen &amp; Co</w:t></w:r></w:p>
    <w:p><w:r><w:t>Second paragraph</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
    let bytes = zip(&[
        ("[Content_Types].xml", "<Types/>"),
        ("word/document.xml", document),
    ]);
    let out = extract(
        Some("report.docx"),
        "application/octet-stream",
        &bytes,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(
        out.text.as_deref(),
        Some("Quarterly report for Lumen & Co\nSecond paragraph")
    );
    assert!(out.images.is_empty());
}

#[test]
fn pptx_slides_in_order() {
    let slide = |text: &str| {
        format!(
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
        )
    };
    let (s1, s2, s10) = (slide("first"), slide("second"), slide("tenth"));
    let bytes = zip(&[
        ("ppt/presentation.xml", "<p:presentation xmlns:p=\"p\"/>"),
        ("ppt/slides/slide10.xml", &s10),
        ("ppt/slides/slide2.xml", &s2),
        ("ppt/slides/slide1.xml", &s1),
    ]);
    let out = extract(Some("deck.pptx"), "", &bytes, &Limits::default()).unwrap();
    assert_eq!(out.text.as_deref(), Some("first\nsecond\ntenth"));
}

#[test]
fn xlsx_shared_and_inline_strings() {
    let bytes = zip(&[
        ("xl/workbook.xml", "<workbook/>"),
        (
            "xl/sharedStrings.xml",
            r#"<sst><si><t>Revenue</t></si><si><r><t>Net </t></r><r><t>profit</t></r></si></sst>"#,
        ),
        (
            "xl/worksheets/sheet1.xml",
            r#"<worksheet><sheetData><row><c t="s"><v>0</v></c><c t="inlineStr"><is><t>inline note</t></is></c></row></sheetData></worksheet>"#,
        ),
    ]);
    let out = extract(Some("book.xlsx"), "", &bytes, &Limits::default()).unwrap();
    assert_eq!(
        out.text.as_deref(),
        Some("Revenue\nNet profit\ninline note")
    );
}

#[test]
fn odt_text() {
    let bytes = zip(&[
        ("mimetype", "application/vnd.oasis.opendocument.text"),
        (
            "content.xml",
            r#"<office:document-content xmlns:office="o" xmlns:text="t"><office:body><office:text><text:h>Title</text:h><text:p>Body<text:s/>text</text:p></office:text></office:body></office:document-content>"#,
        ),
    ]);
    let out = extract(Some("letter.odt"), "", &bytes, &Limits::default()).unwrap();
    assert_eq!(out.text.as_deref(), Some("Title\nBody text"));
}

#[test]
fn non_office_zip_is_unsupported() {
    let bytes = zip(&[("readme.txt", "hello")]);
    assert!(matches!(
        extract(Some("x.docx"), "", &bytes, &Limits::default()),
        Err(ExtractError::Unsupported(_))
    ));
}

#[test]
fn corrupt_docx_is_malformed() {
    assert!(matches!(
        extract(Some("x.docx"), "", b"PK\x03\x04garbage", &Limits::default()),
        Err(ExtractError::Malformed(_))
    ));
}

#[test]
fn ics_keeps_summary_description_location() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:123@example.com\r\nDTSTART:20260310T090000Z\r\nSUMMARY:Planning offsite with the Lisbon te\r\n am\r\nDESCRIPTION:Agenda:\\n1. Roadmap\\, Q2\r\nLOCATION:LX Factory\\, Lisbon\r\nORGANIZER;CN=Alex:mailto:alex@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let out = extract(
        Some("invite.ics"),
        "text/calendar",
        ics.as_bytes(),
        &Limits::default(),
    )
    .unwrap();
    let text = out.text.unwrap();
    assert!(
        text.contains("Planning offsite with the Lisbon team"),
        "{text}"
    );
    assert!(text.contains("1. Roadmap, Q2"), "{text}");
    assert!(text.contains("LX Factory, Lisbon"), "{text}");
    assert!(!text.contains("mailto"), "{text}");
    assert!(!text.contains("DTSTART"), "{text}");
}

#[test]
fn plain_text_is_normalized_and_truncated() {
    let limits = Limits {
        max_text_chars: 5,
        ..Limits::default()
    };
    let out = extract(
        Some("a.txt"),
        "text/plain",
        "  äö \t ü\n\n\nßxyz".as_bytes(),
        &limits,
    )
    .unwrap();
    assert_eq!(out.text.as_deref(), Some("äö ü"));
    let empty = extract(Some("a.txt"), "text/plain", b" \n\t ", &Limits::default()).unwrap();
    assert_eq!(empty.text, None);
}

#[test]
fn invalid_utf8_text_is_decoded_lossily() {
    let out = extract(None, "text/plain", b"caf\xE9 ok", &Limits::default()).unwrap();
    assert_eq!(out.text.as_deref(), Some("caf\u{FFFD} ok"));
}
