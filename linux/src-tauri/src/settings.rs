// Preferences live in the platform config directory. Secrets use the system keyring.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ActivationMode {
    #[default]
    Hover,
    Click,
}

fn default_auto_hide_interval() -> f64 {
    30.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub sound_enabled: bool,
    pub sound_volume: f64,
    pub auto_close_interval: f64,
    #[serde(default)]
    pub activation_mode: ActivationMode,
    #[serde(default)]
    pub auto_hide_enabled: bool,
    #[serde(default = "default_auto_hide_interval")]
    pub auto_hide_interval: f64,
    pub absence_interval: f64,
    pub active_integrations: Vec<String>,
    /// "primary" = the main display, "cursor" = whichever display the mouse is on.
    pub screen: String,
    pub autostart: bool,
    /// Chat model override. Empty uses the Codex catalog default.
    /// Defaulted explicitly so a settings.json written by an older build still loads.
    #[serde(default = "default_model")]
    pub model: String,
    /// Empty selects the chosen model's default reasoning effort.
    #[serde(default)]
    pub reasoning_effort: String,
}

fn default_model() -> String {
    crate::codex::DEFAULT_MODEL.to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            sound_volume: 0.12,
            auto_close_interval: 15.0,
            activation_mode: ActivationMode::Hover,
            auto_hide_enabled: false,
            auto_hide_interval: default_auto_hide_interval(),
            absence_interval: 180.0,
            active_integrations: Vec::new(),
            screen: "primary".into(),
            autostart: false,
            model: default_model(),
            reasoning_effort: String::new(),
        }
    }
}

/// $XDG_CONFIG_HOME/coucou, falling back to ~/.config/coucou.
pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join("coucou")
}

/// $XDG_DATA_HOME/coucou, falling back to ~/.local/share/coucou.
pub fn local_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share").join("coucou")
}

fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(fallback)
        })
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    match std::fs::read(settings_path()) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    let dir = config_dir();
    crate::private_fs::ensure_private_dir(&dir)?;
    let json = serde_json::to_vec_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::private_fs::open_private_file(&settings_path(), false)?.write_all(&json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_preferences_keep_their_values_and_gain_default_reasoning() {
        let mut saved = serde_json::to_value(Settings::default()).unwrap();
        saved["model"] = serde_json::json!("saved-model");
        saved["soundVolume"] = serde_json::json!(0.37);
        saved["hooksInstalled"] = serde_json::json!(true);
        saved.as_object_mut().unwrap().remove("reasoningEffort");
        let loaded: Settings = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded.model, "saved-model");
        assert_eq!(loaded.sound_volume, 0.37);
        assert_eq!(loaded.reasoning_effort, "");
        let mut updated = loaded;
        updated.reasoning_effort = "high".into();
        let roundtrip: Settings = serde_json::from_slice(&serde_json::to_vec(&updated).unwrap()).unwrap();
        assert_eq!(roundtrip.reasoning_effort, "high");
        assert_eq!(roundtrip.sound_volume, 0.37);
        assert!(serde_json::to_value(roundtrip).unwrap().get("hooksInstalled").is_none());
    }

    #[test]
    fn older_preferences_keep_hover_and_visible_compact_defaults() {
        let mut saved = serde_json::to_value(Settings::default()).unwrap();
        for key in ["activationMode", "autoHideEnabled", "autoHideInterval"] {
            saved.as_object_mut().unwrap().remove(key);
        }
        saved["autoCloseInterval"] = serde_json::json!(7.0);
        let loaded: Settings = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded.activation_mode, ActivationMode::Hover);
        assert!(!loaded.auto_hide_enabled);
        assert_eq!(loaded.auto_hide_interval, 30.0);
        assert_eq!(loaded.auto_close_interval, 7.0);
    }

    #[test]
    fn click_and_auto_hide_preferences_survive_roundtrip() {
        let settings = Settings {
            activation_mode: ActivationMode::Click,
            auto_hide_enabled: true,
            auto_hide_interval: 12.0,
            ..Settings::default()
        };
        let loaded: Settings = serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(loaded.activation_mode, ActivationMode::Click);
        assert!(loaded.auto_hide_enabled);
        assert_eq!(loaded.auto_hide_interval, 12.0);
    }
}
