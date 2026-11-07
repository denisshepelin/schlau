use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::{BezPath, Point, Rect};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use image::{RgbImage, RgbaImage};

use crate::image::{encode_jpeg, flatten_rgba};
use crate::text::finish;
use crate::{EncodedImage, ExtractError, Extracted, Limits};

const MIN_TEXT_CHARS: usize = 20;
const GLYPH_UNITS_PER_EM: f64 = 1000.0;
const BLANK_THRESHOLD: u8 = 250;

pub(crate) fn extract(bytes: &[u8], limits: &Limits) -> Result<Extracted, ExtractError> {
    let pdf =
        Pdf::new(bytes.to_vec()).map_err(|err| ExtractError::Malformed(format!("pdf: {err:?}")))?;
    let pages = pdf.pages();
    if pages.is_empty() {
        return Err(ExtractError::Malformed("pdf has no pages".to_owned()));
    }
    let settings = InterpreterSettings::default();
    let text = finish(
        &document_text(pages, &settings, limits),
        MIN_TEXT_CHARS,
        limits,
    );
    let images = render_pages(pages, &settings, limits)?;
    if text.is_none() && images.is_empty() {
        return Err(ExtractError::Malformed(
            "pdf has no text layer and renders blank".to_owned(),
        ));
    }
    Ok(Extracted { text, images })
}

fn document_text(pages: &[Page<'_>], settings: &InterpreterSettings, limits: &Limits) -> String {
    let cache = InterpreterCache::new();
    let budget = limits.max_text_chars.saturating_mul(2);
    pages
        .iter()
        .scan(0usize, |collected, page| {
            (*collected <= budget).then(|| {
                let text = page_text(page, &cache, settings);
                *collected += text.len();
                text
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn page_text<'a>(
    page: &Page<'a>,
    cache: &InterpreterCache<'a>,
    settings: &InterpreterSettings,
) -> String {
    let (width, height) = page.render_dimensions();
    let mut context = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        cache,
        page.xref(),
        settings.clone(),
    );
    let mut collector = TextCollector::default();
    interpret_page(page, &mut context, &mut collector);
    collector.into_text()
}

fn render_pages(
    pages: &[Page<'_>],
    settings: &InterpreterSettings,
    limits: &Limits,
) -> Result<Vec<EncodedImage>, ExtractError> {
    let cache = RenderCache::new();
    let render_settings = RenderSettings::default();
    let max_side = limits.max_side.min(u32::from(u16::MAX));
    pages
        .iter()
        .take(limits.max_pages)
        .map(|page| {
            let (width, height) = page.render_dimensions();
            let longest = width.max(height).max(1.0);
            let scale = max_side as f32 / longest;
            let pixmap_settings = PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
            };
            let pixmap = render(page, &cache, settings, &render_settings, &pixmap_settings);
            let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
            if w == 0 || h == 0 {
                return Err(ExtractError::Malformed("pdf page has no area".to_owned()));
            }
            let rgba = RgbaImage::from_raw(w, h, pixmap.take_rgba8(ImageAlphaType::Alpha))
                .ok_or_else(|| ExtractError::Malformed("pdf render buffer size".to_owned()))?;
            let rgb = flatten_rgba(&rgba);
            (!is_blank(&rgb)).then(|| encode_jpeg(&rgb)).transpose()
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|pages| pages.into_iter().flatten().collect())
}

/// True when every pixel is (near) white, as for pages hayro recovered without content.
fn is_blank(rgb: &RgbImage) -> bool {
    rgb.as_raw().iter().all(|&c| c >= BLANK_THRESHOLD)
}

struct PlacedGlyph {
    x: f64,
    y: f64,
    end_x: f64,
    size: f64,
    text: String,
}

#[derive(Default)]
struct TextCollector {
    glyphs: Vec<PlacedGlyph>,
}

fn glyph_text(glyph: &Glyph<'_>) -> Option<String> {
    match glyph.as_unicode()? {
        BfString::Char(c) => Some(c.to_string()),
        BfString::String(s) => Some(s),
    }
}

fn glyph_advance(glyph: &Glyph<'_>) -> Option<f32> {
    match glyph {
        Glyph::Outline(outline) => outline.advance_width(),
        Glyph::Type3(t3) => t3.advance_width(),
    }
}

impl TextCollector {
    fn separator(prev: &PlacedGlyph, next: &PlacedGlyph) -> &'static str {
        let size = prev.size.max(next.size).max(1e-3);
        if (next.y - prev.y).abs() > size * 0.5 || next.x < prev.x - size {
            "\n"
        } else if next.x - prev.end_x > size * 0.15 {
            " "
        } else {
            ""
        }
    }

    fn into_text(self) -> String {
        let separators = std::iter::once("").chain(
            self.glyphs
                .windows(2)
                .map(|pair| Self::separator(&pair[0], &pair[1])),
        );
        separators
            .zip(&self.glyphs)
            .flat_map(|(sep, glyph)| [sep, glyph.text.as_str()])
            .collect()
    }
}

impl<'a> Device<'a> for TextCollector {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}

    fn push_clip_path(&mut self, _: &ClipPath) {}

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        let placed = run.glyphs().iter().filter_map(|glyph| {
            let text = glyph_text(glyph)?;
            let transform = props.transform * glyph.transform();
            let origin = transform * Point::ORIGIN;
            let advance = glyph_advance(glyph).map_or(0.0, f64::from);
            let end = transform * Point::new(advance, 0.0);
            Some(PlacedGlyph {
                x: origin.x,
                y: origin.y,
                end_x: end.x,
                size: transform.determinant().abs().sqrt() * GLYPH_UNITS_PER_EM,
                text,
            })
        });
        self.glyphs.extend(placed);
    }

    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}

    fn pop_clip(&mut self) {}

    fn pop_transparency_group(&mut self) {}
}
