use markup5ever_rcdom::{Handle, NodeData};

use crate::css::declarations;
use crate::dom::{MAX_DEPTH, attribute, is_block, is_invisible_char, is_skipped, parse, tag_name};
use crate::links::clean_link;
use crate::options::{Options, Tally};
use crate::pixel::{ImageShape, is_invisible};
use crate::urls::{
    Resource, ResourceAction, classify_resource, decide_resource, is_remote, normalize_url,
};

/// A run of inline content inside a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text {
        text: String,
        bold: bool,
        italic: bool,
        code: bool,
    },
    /// `href` is already cleaned (unwrapped, tracking parameters removed, safe scheme).
    Link {
        text: String,
        href: String,
    },
    LineBreak,
}

/// Where a reader image comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// A `cid:` reference resolved through [`Options::cid_urls`].
    Local(String),
    /// A `cid:` reference with no entry in [`Options::cid_urls`].
    Cid(String),
    /// A `data:image/*` URI.
    Inline(String),
    /// A remote image the policy allows (`https` only).
    Remote(String),
    /// A remote image the policy blocks; the URL is kept so the reader can offer to load it.
    Blocked(String),
}

/// Simple block structure for a native reader. Layout tables are flattened into paragraphs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Heading(u8, Vec<Inline>),
    Quote(Vec<Block>),
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    Image {
        alt: String,
        src: ImageSource,
        /// The width the message asks for, in CSS pixels (`width="600"`), when it gives one.
        width: Option<u32>,
    },
    Rule,
    Pre(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
    /// Line breaks in text are kept (`white-space: pre-wrap`, and the plain-text wrapper
    /// `text_to_html` writes): a single newline breaks the line, a blank line ends the paragraph.
    lines: bool,
}

impl Style {
    fn apply(self, node: &Handle) -> Style {
        let tag = tag_name(node).unwrap_or_default();
        let from_tag = Style {
            bold: self.bold || matches!(tag, "b" | "strong" | "th"),
            italic: self.italic || matches!(tag, "i" | "em" | "cite" | "var" | "dfn"),
            code: self.code || matches!(tag, "code" | "tt" | "kbd" | "samp"),
            lines: self.lines
                || attribute(node, "class").is_some_and(|c| c.split_whitespace().any(|c| c.starts_with("schlau-plain"))),
        };
        attribute(node, "style").map_or(from_tag, |style| {
            declarations(&style).fold(from_tag, |acc, (name, value)| match name.as_str() {
                "font-weight" => Style {
                    bold: value == "bold"
                        || value == "bolder"
                        || value.parse::<u16>().is_ok_and(|w| w >= 600),
                    ..acc
                },
                "font-style" => Style {
                    italic: value == "italic" || value == "oblique",
                    ..acc
                },
                "white-space" => Style {
                    lines: matches!(value.as_str(), "pre" | "pre-wrap" | "pre-line" | "break-spaces"),
                    ..acc
                },
                _ => acc,
            })
        })
    }
}

struct Builder<'o> {
    options: &'o Options,
    blocks: Vec<Block>,
    inlines: Vec<Inline>,
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    text.chars()
        .filter(|c| !is_invisible_char(*c))
        .for_each(|c| {
            if c.is_whitespace() {
                space = true;
            } else {
                if space {
                    out.push(' ');
                }
                space = false;
                out.push(c);
            }
        });
    if space {
        out.push(' ');
    }
    out
}

fn ends_with_space(inlines: &[Inline]) -> bool {
    match inlines.last() {
        None | Some(Inline::LineBreak) => true,
        Some(Inline::Text { text, .. } | Inline::Link { text, .. }) => text.ends_with(' '),
    }
}

fn trim_inlines(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while matches!(inlines.last(), Some(Inline::LineBreak)) {
        inlines.pop();
    }
    while matches!(inlines.first(), Some(Inline::LineBreak)) {
        inlines.remove(0);
    }
    if let Some(Inline::Text { text, .. }) = inlines.first_mut() {
        *text = text.trim_start().to_owned();
    }
    if let Some(Inline::Text { text, .. }) = inlines.last_mut() {
        *text = text.trim_end().to_owned();
    }
    let trimmed: Vec<Inline> = inlines
        .into_iter()
        .filter(|inline| !matches!(inline, Inline::Text { text, .. } if text.is_empty()))
        .collect();
    let has_content = trimmed
        .iter()
        .any(|inline| !matches!(inline, Inline::LineBreak));
    if has_content { trimmed } else { Vec::new() }
}

fn subtree_text(node: &Handle, depth: usize) -> String {
    if depth > MAX_DEPTH {
        return String::new();
    }
    match &node.data {
        NodeData::Text { contents } => contents.borrow().to_string(),
        NodeData::Element { .. } if is_skipped(node) => String::new(),
        NodeData::Element { .. } if tag_name(node) == Some("br") => "\n".into(),
        _ => node
            .children
            .borrow()
            .iter()
            .map(|child| subtree_text(child, depth + 1))
            .collect(),
    }
}

fn contains_image(node: &Handle, depth: usize) -> bool {
    depth <= MAX_DEPTH
        && node
            .children
            .borrow()
            .iter()
            .any(|child| tag_name(child) == Some("img") || contains_image(child, depth + 1))
}

fn heading_level(tag: &str) -> Option<u8> {
    match tag {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

impl<'o> Builder<'o> {
    fn new(options: &'o Options) -> Self {
        Builder {
            options,
            blocks: Vec::new(),
            inlines: Vec::new(),
        }
    }

    fn flush(&mut self) {
        let inlines = trim_inlines(std::mem::take(&mut self.inlines));
        if !inlines.is_empty() {
            self.blocks.push(Block::Paragraph(inlines));
        }
    }

    fn push_block(&mut self, block: Block) {
        self.flush();
        self.blocks.push(block);
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush();
        self.blocks
    }

    fn push_text(&mut self, raw: &str, style: Style) {
        if style.lines && raw.contains('\n') {
            return self.push_lines(raw, style);
        }
        let collapsed = collapse_whitespace(raw);
        let text = if ends_with_space(&self.inlines) {
            collapsed.trim_start()
        } else {
            collapsed.as_str()
        };
        if text.is_empty() {
            return;
        }
        match self.inlines.last_mut() {
            Some(Inline::Text {
                text: last,
                bold,
                italic,
                code,
            }) if (*bold, *italic, *code) == (style.bold, style.italic, style.code) => {
                last.push_str(text);
            }
            _ => self.inlines.push(Inline::Text {
                text: text.to_owned(),
                bold: style.bold,
                italic: style.italic,
                code: style.code,
            }),
        }
    }

    /// Text whose line breaks count: lines become line breaks, blank lines end paragraphs.
    fn push_lines(&mut self, raw: &str, style: Style) {
        let plain = Style { lines: false, ..style };
        let mut blank_run = 0;
        raw.split('\n').enumerate().for_each(|(i, line)| {
            if i > 0 {
                if line.trim().is_empty() {
                    blank_run += 1;
                } else {
                    match blank_run {
                        0 => self.inlines.push(Inline::LineBreak),
                        _ => self.flush(),
                    }
                    blank_run = 0;
                }
            }
            if !line.trim().is_empty() {
                self.push_text(line, plain);
            }
        });
        if blank_run > 0 {
            self.flush();
        }
    }

    fn children_blocks(&self, node: &Handle, style: Style, depth: usize) -> Vec<Block> {
        let mut inner = Builder::new(self.options);
        inner.children(node, style, depth);
        inner.finish()
    }

    fn children(&mut self, node: &Handle, style: Style, depth: usize) {
        node.children
            .borrow()
            .iter()
            .for_each(|child| self.node(child, style, depth + 1));
    }

    fn node(&mut self, node: &Handle, style: Style, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match &node.data {
            NodeData::Text { contents } => self.push_text(&contents.borrow(), style),
            NodeData::Element { .. } if !is_skipped(node) => self.element(node, style, depth),
            NodeData::Document => self.children(node, style, depth),
            _ => {}
        }
    }

    fn element(&mut self, node: &Handle, style: Style, depth: usize) {
        let tag = tag_name(node).unwrap_or_default();
        let style = style.apply(node);
        if let Some(level) = heading_level(tag) {
            self.heading(node, level, style, depth);
            return;
        }
        match tag {
            "br" => self.inlines.push(Inline::LineBreak),
            "hr" => self.push_block(Block::Rule),
            "img" => self.image(node),
            "pre" => {
                let text = subtree_text(node, depth);
                let text = text.trim_matches('\n');
                if !text.trim().is_empty() {
                    self.push_block(Block::Pre(text.to_owned()));
                }
            }
            "blockquote" => {
                let inner = self.children_blocks(node, style, depth);
                if !inner.is_empty() {
                    self.push_block(Block::Quote(inner));
                }
            }
            "ul" | "ol" => self.list(node, tag == "ol", style, depth),
            "a" => self.link(node, style, depth),
            "tr" => {
                self.flush();
                self.children(node, style, depth);
                self.flush();
            }
            "td" | "th" => {
                let before = self.inlines.len();
                if before > 0 {
                    self.inlines.push(Inline::Text { text: " · ".into(), bold: false, italic: false, code: false });
                }
                self.children(node, style, depth);
                let only_separator = self.inlines.len() == before + 1
                    && matches!(self.inlines.last(), Some(Inline::Text { text, .. }) if text == " · ");
                if before > 0 && only_separator {
                    self.inlines.pop();
                }
            }
            _ if is_block(tag) => {
                self.flush();
                self.children(node, style, depth);
                self.flush();
            }
            _ => self.children(node, style, depth),
        }
    }

    fn heading(&mut self, node: &Handle, level: u8, style: Style, depth: usize) {
        let inner = self.children_blocks(
            node,
            Style {
                bold: false,
                ..style
            },
            depth,
        );
        self.flush();
        let (text_blocks, other): (Vec<Block>, Vec<Block>) = inner
            .into_iter()
            .partition(|block| matches!(block, Block::Paragraph(_)));
        let inlines: Vec<Inline> = text_blocks
            .into_iter()
            .flat_map(|block| match block {
                Block::Paragraph(inlines) => inlines,
                _ => Vec::new(),
            })
            .collect();
        if !inlines.is_empty() {
            self.blocks.push(Block::Heading(level, inlines));
        }
        self.blocks.extend(other);
    }

    fn list(&mut self, node: &Handle, ordered: bool, style: Style, depth: usize) {
        let items: Vec<Vec<Block>> = node
            .children
            .borrow()
            .iter()
            .filter(|child| tag_name(child) == Some("li") && !is_skipped(child))
            .map(|item| self.children_blocks(item, style.apply(item), depth + 1))
            .filter(|blocks| !blocks.is_empty())
            .collect();
        if !items.is_empty() {
            self.push_block(Block::List { ordered, items });
        }
    }

    fn link(&mut self, node: &Handle, style: Style, depth: usize) {
        let text = collapse_whitespace(&subtree_text(node, depth))
            .trim()
            .to_owned();
        let href = attribute(node, "href")
            .and_then(|href| clean_link(&href))
            .map(|link| link.href);
        match href {
            Some(href) if !text.is_empty() && !contains_image(node, depth) => {
                self.inlines.push(Inline::Link { text, href });
            }
            _ => self.children(node, style, depth),
        }
    }

    fn image(&mut self, node: &Handle) {
        let Some(src) = attribute(node, "src") else {
            return;
        };
        let (width, height, css) = (
            attribute(node, "width"),
            attribute(node, "height"),
            attribute(node, "style"),
        );
        let shape = ImageShape {
            width: width.as_deref(),
            height: height.as_deref(),
            style: css.as_deref(),
            hidden: false,
        };
        if is_remote(&src) && is_invisible(shape) {
            return;
        }
        let alt = attribute(node, "alt")
            .map(|alt| collapse_whitespace(&alt).trim().to_owned())
            .unwrap_or_default();
        let source = match classify_resource(&src) {
            Resource::Cid(id) => match self.options.resolve_cid(&id) {
                Some(local) => Some(ImageSource::Local(local.to_owned())),
                None => Some(ImageSource::Cid(id)),
            },
            Resource::DataImage => Some(ImageSource::Inline(normalize_url(&src).into_owned())),
            Resource::Invalid => None,
            Resource::Remote(remote) => {
                let original = remote.url.as_str().to_owned();
                match decide_resource(&src, self.options, &mut Tally::default()) {
                    ResourceAction::Keep(url) => Some(ImageSource::Remote(url)),
                    ResourceAction::Blocked => Some(ImageSource::Blocked(original)),
                    ResourceAction::Pixel | ResourceAction::Invalid => None,
                }
            }
        };
        let width = width.as_deref().and_then(css_pixels);
        if let Some(src) = source {
            self.push_block(Block::Image { alt, src, width });
        }
    }
}

/// A length attribute in pixels: `600` or `600px`; percentages and other units give nothing.
fn css_pixels(value: &str) -> Option<u32> {
    let value = value.trim();
    let number = value.strip_suffix("px").unwrap_or(value).trim();
    number.parse::<f32>().ok().filter(|n| n.is_finite() && *n >= 1.0).map(|n| n.round() as u32)
}

/// Reader blocks with default [`Options`]: remote images blocked, `cid:` left unresolved.
pub fn to_blocks(html: &str) -> Vec<Block> {
    to_blocks_with(html, &Options::default())
}

/// Reader blocks using `options` for the remote policy and `cid:` resolution. Tracking
/// pixels are dropped, links are cleaned, and table layouts become a flat list of paragraphs.
pub fn to_blocks_with(html: &str, options: &Options) -> Vec<Block> {
    let dom = parse(html);
    let mut builder = Builder::new(options);
    builder.node(&dom.document, Style::default(), 0);
    builder.finish()
}
