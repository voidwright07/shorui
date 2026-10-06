//! Bundled icons and fonts, layered over gpui-kit's own assets.

use anyhow::Result;
use gpui_kit::{AssetSource, SharedString};
use rust_embed::RustEmbed;
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/*.svg"]
#[include = "fonts/*.ttf"]
struct Embedded;

/// Paths under `icons/` and `fonts/` come from this crate; everything else from gpui-kit.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = Embedded::get(path) {
            return Ok(Some(file.data));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = Embedded::iter().filter(|name| name.starts_with(path)).map(|name| SharedString::from(name.to_string())).collect();
        out.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(out)
    }
}

/// The bundled font files, ready for `TextSystem::add_fonts`.
pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    Embedded::iter().filter(|name| name.starts_with("fonts/")).filter_map(|name| Embedded::get(&name)).map(|file| file.data).collect()
}

pub fn icon_path(name: &str) -> SharedString {
    SharedString::from(format!("icons/{name}.svg"))
}
