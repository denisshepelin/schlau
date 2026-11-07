use schlau_html::{Block, Inline, text_to_html, to_blocks};

fn texts(inlines: &[Inline]) -> Vec<String> {
    inlines
        .iter()
        .map(|i| match i {
            Inline::Text { text, .. } => text.clone(),
            Inline::Link { text, .. } => format!("[{text}]"),
            Inline::LineBreak => "/".into(),
        })
        .collect()
}

#[test]
fn plain_text_keeps_its_paragraphs_line_breaks_and_quotes() {
    let html = text_to_html("Hello Denis,\n\nfirst paragraph\nsecond line\n\n> quoted\n> more\n\nBye").html;
    let blocks = to_blocks(&html);
    let Block::Paragraph(first) = &blocks[0] else { panic!("{blocks:?}") };
    assert_eq!(texts(first), ["Hello Denis,"]);
    let Block::Paragraph(second) = &blocks[1] else { panic!("{blocks:?}") };
    assert_eq!(texts(second), ["first paragraph", "/", "second line"]);
    assert!(matches!(&blocks[2], Block::Quote(_)), "{blocks:?}");
    let Block::Paragraph(last) = blocks.last().unwrap() else { panic!("{blocks:?}") };
    assert_eq!(texts(last), ["Bye"]);
}

#[test]
fn html_whitespace_still_collapses() {
    let blocks = to_blocks("<p>one\n   two</p>");
    let Block::Paragraph(p) = &blocks[0] else { panic!("{blocks:?}") };
    assert_eq!(texts(p), ["one two"]);
}

#[test]
fn a_table_row_of_short_cells_reads_as_one_line() {
    let blocks = to_blocks("<table><tr><td>Amount paid</td><td><b>$49.00</b></td></tr><tr><td>Invoice</td><td>INV-1</td></tr></table>");
    let lines: Vec<String> = blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph(p) => texts(p).concat(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(lines, ["Amount paid · $49.00", "Invoice · INV-1"]);
}

#[test]
fn layout_tables_with_blocks_inside_keep_their_blocks() {
    let blocks = to_blocks("<table><tr><td><h1>Title</h1><p>Body text</p></td></tr></table>");
    assert!(matches!(blocks[0], Block::Heading(1, _)), "{blocks:?}");
    assert!(matches!(&blocks[1], Block::Paragraph(p) if texts(p) == ["Body text"]), "{blocks:?}");
}
