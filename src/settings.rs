use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CaptureMode {
    #[default]
    Rectangle,
    Freeform,
}

impl CaptureMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rectangle => "▭ Rectangle",
            Self::Freeform => "〰 Freeform (Lasso)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveFormat {
    Png,
    Jpeg,
}

impl Default for SaveFormat {
    fn default() -> Self {
        Self::Png
    }
}

impl SaveFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    pub fn filter_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Png => &["png"],
            Self::Jpeg => &["jpg", "jpeg"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG (Lossless / Sharp)",
            Self::Jpeg => "JPEG (Compact + Explorer Details)",
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub default_author: String,
    pub default_format: SaveFormat,
    #[serde(default)]
    pub default_capture_mode: CaptureMode,
    pub jpeg_quality: u8,
    pub last_save_dir: Option<PathBuf>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_author: String::new(),
            default_format: SaveFormat::Png,
            default_capture_mode: CaptureMode::Rectangle,
            jpeg_quality: 95,
            last_save_dir: None,
        }
    }
}

impl AppSettings {
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("skrinshotter").join("settings.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut settings: Self = serde_json::from_str(&raw).unwrap_or_default();
        settings.jpeg_quality = settings.jpeg_quality.clamp(10, 100);
        settings
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path()
            .ok_or_else(|| "Could not find system configuration directory.".to_string())?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create configuration directory: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?;
        fs::write(&path, json).map_err(|e| format!("Failed to save settings: {e}"))
    }

    pub fn preferred_save_dir(&self) -> PathBuf {
        if let Some(dir) = &self.last_save_dir {
            if dir.is_dir() {
                return dir.clone();
            }
        }
        dirs::picture_dir()
            .or_else(dirs::desktop_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

