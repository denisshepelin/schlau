//! Quick look: an attachment shown over the conversation without leaving the keyboard. Files come
//! from the same place the attachment worker gets them (the account, or the fixture folder), are kept
//! in a cache folder so a second look is instant and ↵ can hand them to the default app, and are
//! drawn as pictures (photos, PDF pages) or text (documents) through `schlau-extract`, never by a
//! renderer that could run them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune::{El, ImageCache, Key, KeyEvent, ScrollSpec, SurfaceFit, TextStyle, col, el, row, spacer, text};
use rune_kit::Kit;
use schlau_extract::{Kind, Limits};
use schlau_store::Attachment;
use schlau_sync::{BlobError, BlobSource};

use crate::reader::human_size;
use crate::state::Ui;

/// The attachment being looked at, and its siblings in the same message for ← and →.
pub struct Preview {
    pub attachments: Vec<Attachment>,
    pub index: usize,
    pub content: Content,
    /// The file on disk, once fetched, for opening in the default app.
    pub path: Option<PathBuf>,
}

pub enum Content {
    Loading,
    /// Pictures with their width over height: a photo, or a document's pages.
    Pictures(Vec<(rune::Image, f32)>),
    Text(String),
    Nothing(String),
}

/// A fetched and extracted attachment, made on a worker.
pub struct Loaded {
    pub blob_id: String,
    pub path: Option<PathBuf>,
    pub body: Result<Body, String>,
}

pub enum Body {
    /// Encoded pictures (JPEG) and their width over height.
    Pictures(Vec<(Vec<u8>, f32)>),
    Text(String),
    Nothing(String),
}

pub struct Request {
    pub attachment: Attachment,
    pub blobs: Arc<dyn BlobSource>,
    pub cache: PathBuf,
}

const LIMITS: Limits = Limits { max_bytes: 50_000_000, max_text_chars: 200_000, max_pages: 12, max_side: 2048 };

/// Where an attachment is kept: one folder per blob, the file under its own (sanitized) name.
fn cached_path(cache: &Path, a: &Attachment) -> PathBuf {
    let safe = |s: &str| s.chars().map(|c| if c.is_alphanumeric() || "._- ".contains(c) { c } else { '_' }).collect::<String>();
    let name = a.name.as_deref().and_then(|n| Path::new(n).file_name()).and_then(|n| n.to_str()).map(safe);
    cache.join(safe(&a.blob_id)).join(name.filter(|n| !n.trim_matches('.').is_empty()).unwrap_or_else(|| "attachment".into()))
}

fn fetch(r: &Request) -> Result<(Vec<u8>, Option<PathBuf>), String> {
    let path = cached_path(&r.cache, &r.attachment);
    if let Ok(bytes) = std::fs::read(&path) {
        return Ok((bytes, Some(path)));
    }
    let bytes = r.blobs.fetch(&r.attachment).map_err(|e| match e {
        BlobError::Missing(why) => format!("The file is not available: {why}"),
        BlobError::Unavailable(why) => format!("Couldn't download it: {why}"),
    })?;
    let saved = path.parent().is_some_and(|dir| std::fs::create_dir_all(dir).is_ok()) && std::fs::write(&path, &bytes).is_ok();
    Ok((bytes, saved.then_some(path)))
}

pub fn load(r: Request) -> Loaded {
    let a = &r.attachment;
    let blob_id = a.blob_id.clone();
    let (bytes, path) = match fetch(&r) {
        Ok(found) => found,
        Err(e) => return Loaded { blob_id, path: None, body: Err(e) },
    };
    let body = match schlau_extract::kind(a.name.as_deref(), &a.mime) {
        None => Ok(Body::Nothing("No preview for this kind of file".into())),
        Some(kind) => schlau_extract::extract(a.name.as_deref(), &a.mime, &bytes, &LIMITS).map_err(|e| e.to_string()).map(|x| {
            match (kind, x.images.is_empty(), x.text) {
                (Kind::Image | Kind::Pdf, false, _) => Body::Pictures(
                    x.images.into_iter().map(|i| (i.bytes, i.width as f32 / i.height.max(1) as f32)).collect(),
                ),
                (_, _, Some(text)) => Body::Text(text),
                _ => Body::Nothing("Nothing to show in this file".into()),
            }
        }),
    };
    Loaded { blob_id, path, body }
}

/// The loaded attachment as something to draw, its pictures handed to the image cache to decode.
pub fn content(body: Result<Body, String>, images: &ImageCache) -> Content {
    match body {
        Ok(Body::Pictures(pictures)) => {
            Content::Pictures(pictures.into_iter().map(|(bytes, ratio)| (images.load(bytes), ratio)).collect())
        }
        Ok(Body::Text(t)) => Content::Text(t),
        Ok(Body::Nothing(why)) | Err(why) => Content::Nothing(why),
    }
}

impl Ui {
    /// Quick-looks attachment `index` of the open conversation's focused message.
    pub fn quick_look(&mut self, attachments: Vec<Attachment>, index: usize) {
        if attachments.is_empty() {
            self.toast("No attachments in this message");
            return;
        }
        let index = index.min(attachments.len() - 1);
        self.preview = Some(Preview { attachments, index, content: Content::Loading, path: None });
        self.load_preview();
    }

    fn load_preview(&mut self) {
        let (Some(p), Some(blobs)) = (self.preview.as_mut(), self.blobs.clone()) else {
            if let Some(p) = self.preview.as_mut() {
                p.content = Content::Nothing("Attachments can't be fetched in this mode".into());
            }
            return;
        };
        p.content = Content::Loading;
        p.path = None;
        let request = Request { attachment: p.attachments[p.index].clone(), blobs, cache: self.attachment_cache.clone() };
        self.preview_loads.request(request, load);
    }

    /// A finished load, if it is for the attachment still on screen.
    pub fn preview_loaded(&mut self, loaded: Loaded) {
        let (Some(p), Some(images)) = (self.preview.as_mut(), self.images.as_ref()) else { return };
        if p.attachments.get(p.index).is_some_and(|a| a.blob_id == loaded.blob_id) {
            p.path = loaded.path;
            p.content = content(loaded.body, images);
        }
    }

    fn step_preview(&mut self, delta: isize) {
        let Some(p) = self.preview.as_mut() else { return };
        let n = p.attachments.len() as isize;
        let next = (p.index as isize + delta).rem_euclid(n.max(1)) as usize;
        if next != p.index {
            p.index = next;
            self.load_preview();
        }
    }

    fn open_preview_externally(&mut self) {
        match self.preview.as_ref().and_then(|p| p.path.clone()) {
            Some(path) => {
                if let Err(e) = std::process::Command::new("open").arg(&path).spawn() {
                    self.toast(format!("Couldn't open it: {e}"));
                }
            }
            None => self.toast("Still fetching the file"),
        }
    }
}

/// Keys while quick look is open: it takes all of them.
pub fn on_key(s: &mut Ui, ev: &KeyEvent) -> bool {
    match &ev.key {
        Key::Escape => s.preview = None,
        Key::Text(t) if t == " " || t == "q" => s.preview = None,
        Key::Left => s.step_preview(-1),
        Key::Right => s.step_preview(1),
        Key::Text(t) if t == "h" => s.step_preview(-1),
        Key::Text(t) if t == "l" => s.step_preview(1),
        Key::Enter => s.open_preview_externally(),
        Key::Text(t) if t == "o" => s.open_preview_externally(),
        _ => {}
    }
    true
}

pub fn view(s: &Ui, k: &Kit) -> Option<El> {
    let p = s.preview.as_ref()?;
    let a = &p.attachments[p.index];
    let name = a.name.clone().unwrap_or_else(|| "attachment".into());
    let position = if p.attachments.len() > 1 { format!("{} of {} · ←→ attachments · ", p.index + 1, p.attachments.len()) } else { String::new() };
    let header = row()
        .w_full()
        .h(44.0)
        .px(18.0)
        .gap(12.0)
        .center_y()
        .child(text(name).font(TextStyle { weight: 600.0, ..k.ty.body }).color(k.c.text_base).truncate().min_w(0.0))
        .child(text(human_size(a.size)).font(k.ty.small).color(k.c.text_faint))
        .child(spacer())
        .child(text(format!("{position}↵ open in default app · esc close")).font(k.ty.small).color(k.c.text_faint));
    let body = match &p.content {
        Content::Loading => centered(text("Loading…").font(k.ty.body).color(k.c.text_faint)),
        Content::Nothing(why) => centered(
            col()
                .gap(6.0)
                .items_center()
                .child(text(why.clone()).font(k.ty.body).color(k.c.text_muted))
                .child(text("↵ opens it in the default app").font(k.ty.small).color(k.c.text_faint)),
        ),
        Content::Pictures(pictures) if pictures.len() == 1 => {
            let (image, ratio) = &pictures[0];
            centered(el().aspect_fit(*ratio).pixels_fit(image.pixels(), SurfaceFit::Fit).radius(4.0))
        }
        Content::Pictures(pages) => col()
            .key(format!("preview-pages:{}", a.blob_id))
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(col().w_full().padding(18.0).gap(14.0).items_center().children(pages.iter().map(|(image, ratio)| {
                el().w_full().max_w(900.0).aspect_ratio(*ratio).pixels_fit(image.pixels(), SurfaceFit::Fit).radius(2.0)
            }))),
        Content::Text(t) => col()
            .key(format!("preview-text:{}", a.blob_id))
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(
                col().w_full().padding(24.0).child(
                    text(t.clone())
                        .font(k.ty.body.lh(21.0))
                        .color(k.c.text_base)
                        .selectable(true)
                        .selection_color(k.selection())
                        .wrap_anywhere()
                        .w_full(),
                ),
            ),
    };
    let panel = col()
        .key("quick-look")
        .w_full()
        .h_full()
        .radius(12.0)
        .clip_rounded(12.0)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_base)
        .child(header)
        .child(el().w_full().h(1.0).fill(k.c.border_muted))
        .child(body);
    Some(col().absolute(0.0, 0.0).w_full().h_full().padding(36.0).fill(k.c.scrim.alpha(0.55)).child(panel))
}

fn centered(child: El) -> El {
    col().w_full().grow(1.0).min_h(0.0).padding(18.0).center().child(child)
}
