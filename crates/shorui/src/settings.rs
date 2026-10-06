//! Small things remembered between runs. One JSON file in the user's config folder.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub dark: bool,
    pub sidebar_collapsed: bool,
    /// Tools the user starred. Older settings files call them `pinned`.
    #[serde(alias = "pinned")]
    pub favorites: Vec<String>,
    pub recent: Vec<String>,
    /// The folder last saved into. Save dialogs open there.
    pub save_dir: Option<PathBuf>,
    pub window: Option<(f32, f32)>,
    pub maximized: bool,
    /// Each tool's options as last set. Passwords are never kept.
    pub options: BTreeMap<String, Map<String, Value>>,
    /// Edit & Fill: the ink, text size and mark size last used.
    pub edit_ink: Option<[u8; 3]>,
    pub edit_text_size: Option<f32>,
    pub edit_mark_size: Option<f32>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            dark: true,
            sidebar_collapsed: false,
            favorites: Vec::new(),
            recent: Vec::new(),
            save_dir: None,
            window: None,
            maximized: false,
            options: BTreeMap::new(),
            edit_ink: None,
            edit_text_size: None,
            edit_mark_size: None,
        }
    }
}

fn file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("Shorui").join("settings.json"))
}

/// Options worth keeping between runs: not passwords, and not the per-run values the app
/// adds itself (their keys start with `@`).
pub fn keep_option(key: &str) -> bool {
    !key.starts_with('@') && !key.to_ascii_lowercase().contains("password")
}

impl Settings {
    /// Reads the settings file. A missing or unreadable file gives the defaults; this
    /// never fails, because it runs before the first frame.
    pub fn load() -> Self {
        file().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        // Tests must not touch the real settings file.
        if cfg!(test) {
            return;
        }
        if let Some(path) = file() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_vec_pretty(self) {
                // Write beside it and swap, so a crash mid-write cannot leave a broken file.
                let temp = path.with_extension("json.tmp");
                if std::fs::write(&temp, json).is_ok() {
                    let _ = std::fs::rename(&temp, &path);
                }
            }
        }
    }

    /// Copy the tools' options in, leaving out what must not be kept.
    pub fn remember_options(&mut self, options: &HashMap<&'static str, Map<String, Value>>) {
        self.options = options.iter().map(|(tool, map)| (tool.to_string(), map.iter().filter(|(k, _)| keep_option(k)).map(|(k, v)| (k.clone(), v.clone())).collect())).collect();
    }

    /// Move a tool to the front of the recent list.
    pub fn touch_recent(&mut self, id: &str) {
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_string());
        self.recent.truncate(5);
    }

    pub fn is_favorite(&self, id: &str) -> bool {
        self.favorites.iter().any(|f| f == id)
    }

    pub fn toggle_favorite(&mut self, id: &str) {
        if self.is_favorite(id) {
            self.favorites.retain(|f| f != id);
        } else {
            self.favorites.push(id.to_string());
        }
    }
}

/// A path shortened for display: the home folder becomes `~`.
pub fn display_path(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    match dirs::home_dir() {
        Some(home) => {
            let home = home.to_string_lossy().replace('\\', "/");
            match text.strip_prefix(&home) {
                Some(rest) => format!("~{rest}"),
                None => text,
            }
        }
        None => text,
    }
}

/// `8.4 MB`, `96 KB`, `512 B`.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(98_304), "96 KB");
        assert_eq!(human_size(8_808_038), "8.4 MB");
    }

    #[test]
    fn passwords_and_run_values_are_never_kept() {
        let mut options: HashMap<&'static str, Map<String, Value>> = HashMap::new();
        let mut protect = Map::new();
        protect.insert("user_password".into(), "secret".into());
        protect.insert("owner_password".into(), "secret".into());
        protect.insert("allow_print".into(), true.into());
        options.insert("protect", protect);
        let mut unlock = Map::new();
        unlock.insert("@password".into(), "secret".into());
        options.insert("unlock", unlock);
        let mut s = Settings::default();
        s.remember_options(&options);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("secret"), "{json}");
        assert_eq!(s.options["protect"].get("allow_print"), Some(&Value::Bool(true)));
    }

    #[test]
    fn old_pins_become_favorites() {
        let s: Settings = serde_json::from_str(r#"{"dark":false,"pinned":["ocr","merge"],"output_dir":"C:/x"}"#).unwrap();
        assert_eq!(s.favorites, vec!["ocr", "merge"]);
        assert!(!s.dark);
    }

    #[test]
    fn recent_is_deduplicated_and_capped() {
        let mut s = Settings::default();
        for id in ["merge", "split", "merge", "ocr", "crop", "nup", "pages"] {
            s.touch_recent(id);
        }
        assert_eq!(s.recent, vec!["pages", "nup", "crop", "ocr", "merge"]);
    }
}
