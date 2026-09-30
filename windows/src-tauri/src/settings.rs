// Preferences live in the platform config directory. Secrets use the system keyring.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub sound_enabled: bool,
    pub sound_volume: f64,
    pub auto_close_interval: f64,
    pub absence_interval: f64,
    pub active_integrations: Vec<String>,
    /// "primary" = the main display, "cursor" = whichever display the mouse is on.
    pub screen: String,
    pub autostart: bool,
    pub hooks_installed: bool,
    /// Chat model override. Empty on Linux to use the Codex catalog default.
    /// Defaulted explicitly so a settings.json written by an older build still loads.
    #[serde(default = "default_model")]
    pub model: String,
    /// Empty selects the chosen model's default reasoning effort.
    #[serde(default)]
    pub reasoning_effort: String,
}

fn default_model() -> String {
    #[cfg(windows)]
    {
        crate::claude::DEFAULT_MODEL.to_string()
    }
    #[cfg(target_os = "linux")]
    {
        crate::codex::DEFAULT_MODEL.to_string()
    }
}

fn default_integrations() -> Vec<String> {
    #[cfg(windows)]
    {
        vec![
            "integration_resend".into(),
            "integration_n8n".into(),
            "integration_vercel".into(),
            "integration_github".into(),
        ]
    }
    #[cfg(target_os = "linux")]
    {
        Vec::new()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            sound_volume: 0.12,
            auto_close_interval: 15.0,
            absence_interval: 180.0,
            active_integrations: default_integrations(),
            screen: "primary".into(),
            autostart: false,
            hooks_installed: false,
            model: default_model(),
            reasoning_effort: String::new(),
        }
    }
}

/// %APPDATA%\Coucou on Windows; $XDG_CONFIG_HOME/coucou on Linux.
pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        return xdg_dir("XDG_CONFIG_HOME", ".config").join("coucou");
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("Coucou")
    }
}

/// Windows local application data, or $XDG_DATA_HOME/coucou on Linux.
pub fn local_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        return xdg_dir("XDG_DATA_HOME", ".local/share").join("coucou");
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("Coucou")
    }
}

#[cfg(target_os = "linux")]
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

#[cfg(windows)]
pub fn hook_exe_path() -> PathBuf {
    local_dir().join("bin").join("coucou-hook.exe")
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
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(settings_path(), json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_preferences_keep_their_values_and_gain_default_reasoning() {
        let mut saved = serde_json::to_value(Settings::default()).unwrap();
        saved["model"] = serde_json::json!("saved-model");
        saved["soundVolume"] = serde_json::json!(0.37);
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
    }
}
