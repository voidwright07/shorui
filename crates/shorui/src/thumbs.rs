//! Page images for the UI, rendered off the main thread and cached.

use crate::shell::Shell;
use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use gpui_kit::*;
use image::{Frame, RgbaImage};
use smallvec::smallvec;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub enum Thumb {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

/// `(file, 0-based page, longest side in pixels)`.
pub type ThumbKey = (PathBuf, usize, u32);

#[derive(Default)]
pub struct Thumbs {
    map: HashMap<ThumbKey, Thumb>,
    /// Rotated copies, made on demand for the page grid.
    turned: HashMap<(ThumbKey, i32), Arc<RenderImage>>,
}

/// gpui wants BGRA.
pub fn to_render_image(mut image: RgbaImage) -> Arc<RenderImage> {
    for px in image.pixels_mut() {
        px.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(smallvec![Frame::new(image)]))
}

impl Thumbs {
    pub fn get(&self, key: &ThumbKey) -> Option<&Thumb> {
        self.map.get(key)
    }

    /// The image turned clockwise by `degrees` (a multiple of 90).
    pub fn turned(&mut self, key: &ThumbKey, degrees: i32, image: Arc<RenderImage>) -> Arc<RenderImage> {
        let degrees = degrees.rem_euclid(360);
        if degrees == 0 {
            return image;
        }
        if let Some(done) = self.turned.get(&(key.clone(), degrees)) {
            return done.clone();
        }
        let size = image.size(0);
        let (w, h) = (size.width.0 as u32, size.height.0 as u32);
        let Some(pixels) = image.as_bytes(0).and_then(|bytes| RgbaImage::from_raw(w, h, bytes.to_vec())) else { return image };
        // The bytes are already BGRA; rotating does not care about channel order.
        let rotated = match degrees {
            90 => image::imageops::rotate90(&pixels),
            180 => image::imageops::rotate180(&pixels),
            _ => image::imageops::rotate270(&pixels),
        };
        let out = Arc::new(RenderImage::new(smallvec![Frame::new(rotated)]));
        self.turned.insert((key.clone(), degrees), out.clone());
        out
    }

    /// Forget the images of files that are no longer loaded. Returns them so their
    /// textures can be released: dropping the `Arc` alone does not free a texture.
    pub fn evict(&mut self, keep: &dyn Fn(&Path) -> bool) -> Vec<Arc<RenderImage>> {
        let mut gone = Vec::new();
        self.map.retain(|k, v| {
            let stay = keep(&k.0);
            if !stay {
                if let Thumb::Ready(image) = v {
                    gone.push(image.clone());
                }
            }
            stay
        });
        self.turned.retain(|k, v| {
            let stay = keep(&k.0.0);
            if !stay {
                gone.push(v.clone());
            }
            stay
        });
        gone
    }

    /// Forget the large renderings made for zooming in, except `keep`. Returns them so
    /// their textures can be released.
    pub fn evict_large(&mut self, keep: &ThumbKey) -> Vec<Arc<RenderImage>> {
        let mut gone = Vec::new();
        self.map.retain(|k, v| {
            let stay = k.2 <= 1400 || k == keep;
            if !stay {
                if let Thumb::Ready(image) = v {
                    gone.push(image.clone());
                }
            }
            stay
        });
        gone
    }
}

impl Shell {
    /// The image for a page if it is ready. Starts rendering it (and its neighbours in
    /// `pages`) when it is not. `pages` are 0-based. For an image file, page 0 is the
    /// picture itself.
    pub fn page_image(&mut self, path: &Path, page: usize, max_px: u32, batch: &[usize], cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let key = (path.to_path_buf(), page, max_px);
        match self.thumbs.get(&key) {
            Some(Thumb::Ready(image)) => return Some(image.clone()),
            Some(_) => return None,
            None => {}
        }
        let mut wanted: Vec<usize> = batch.iter().copied().filter(|pg| self.thumbs.get(&(path.to_path_buf(), *pg, max_px)).is_none()).collect();
        if !wanted.contains(&page) {
            wanted.insert(0, page);
        }
        for pg in &wanted {
            self.thumbs.map.insert((path.to_path_buf(), *pg, max_px), Thumb::Loading);
        }
        let password = self.state.options.get("unlock").and_then(|o| o.get("@password")).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string);
        let (tx, mut rx) = unbounded::<(usize, Option<RgbaImage>)>();
        let file = path.to_path_buf();
        let is_pdf = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        cx.background_spawn({
            let file = file.clone();
            async move {
                if !is_pdf {
                    for pg in wanted {
                        let image = if pg == 0 { shorui_core::img::thumbnail(&file, max_px).ok() } else { None };
                        if tx.unbounded_send((pg, image)).is_err() {
                            break;
                        }
                    }
                    return;
                }
                match shorui_core::render::Renderer::open_path(&file, password.as_deref()) {
                    Ok(renderer) => {
                        for pg in wanted {
                            let image = renderer.thumbnail(pg, max_px).ok();
                            if tx.unbounded_send((pg, image)).is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        for pg in wanted {
                            let _ = tx.unbounded_send((pg, None));
                        }
                    }
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Some((pg, image)) = rx.next().await {
                let thumb = match image {
                    Some(image) => Thumb::Ready(to_render_image(image)),
                    None => Thumb::Failed,
                };
                let ok = this.update(cx, |this, cx| {
                    this.thumbs.map.insert((file.clone(), pg, max_px), thumb);
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
        None
    }
}
