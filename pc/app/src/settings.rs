//! Everything the user can set, saved to %APPDATA%\Tether\settings.json.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Source {
    Phone,
    /// Any phone (iPhone too) through the Tether web page, over Wi‑Fi.
    Browser,
    /// A webcam plugged into this PC, by its Windows device link (stable) and its name (for showing).
    Webcam { link: String, name: String },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Quality {
    Low,
    Normal,
    High,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
    /// Windows virtual-key code.
    pub vk: u32,
    /// What to show, e.g. "H".
    pub key: String,
}

impl Hotkey {
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.win {
            parts.push("Win");
        }
        parts.push(&self.key);
        parts.join(" + ")
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub source: Source,
    /// Phone camera id ("0" is usually the main back camera, "1" the front one).
    pub camera: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub quality: Quality,
    pub torch: bool,
    pub phone_preview: bool,
    pub phone_dim: bool,
    pub mirror: bool,
    /// Clockwise, 0/90/180/270.
    pub rotation: u32,
    pub hotkey: Option<Hotkey>,
    /// Image or GIF shown while the cover is on (and when the connection drops). None: the built-in card.
    pub cover_file: Option<PathBuf>,
    pub cover_on_disconnect: bool,
    pub look: crate::look::Look,
    /// Look for a new Tether on GitHub at start and every few hours.
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            source: Source::Phone,
            camera: "0".into(),
            width: 1920,
            height: 1080,
            fps: 30,
            quality: Quality::Normal,
            torch: false,
            phone_preview: true,
            phone_dim: true,
            mirror: false,
            rotation: 0,
            hotkey: Some(Hotkey { ctrl: true, shift: true, alt: false, win: false, vk: 0x48, key: "H".into() }),
            cover_file: None,
            cover_on_disconnect: true,
            look: crate::look::Look::default(),
            auto_update: true,
        }
    }
}

impl Settings {
    pub fn bitrate(&self) -> u32 {
        let base = (self.width * self.height) as f64 * self.fps as f64 * 0.13; // ~8 Mbit/s for 1080p30
        let factor = match self.quality {
            Quality::Low => 0.45,
            Quality::Normal => 1.0,
            Quality::High => 1.8,
        };
        (base * factor).clamp(1_000_000.0, 40_000_000.0) as u32
    }

    fn path() -> Option<PathBuf> {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Tether").join("settings.json"))
    }

    /// The saved settings, or None on the very first start.
    pub fn load_saved() -> Option<Settings> {
        Self::path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok())
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_vec_pretty(self) {
                let tmp = p.with_extension("json.tmp");
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(tmp, p);
                }
            }
        }
    }
}
