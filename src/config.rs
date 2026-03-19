use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_width")]
    pub window_width: i32,
    #[serde(default = "default_height")]
    pub window_height: i32,
    #[serde(default)]
    pub window_maximized: bool,
    #[serde(default)]
    pub info_panel_visible: bool,
    #[serde(default)]
    pub last_directory: Option<String>,
}

fn default_width() -> i32 {
    1200
}
fn default_height() -> i32 {
    800
}

impl Default for Config {
    fn default() -> Self {
        Self {
            window_width: 1200,
            window_height: 800,
            window_maximized: false,
            info_panel_visible: false,
            last_directory: None,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(contents) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, contents);
        }
    }

    fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("iris")
            .join("config.toml")
    }
}
