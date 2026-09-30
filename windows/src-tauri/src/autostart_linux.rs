// Match the user installer exactly, including XDG_CONFIG_HOME and filename case.
// The generic autostart dependency hardcodes ~/.config on Linux.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

fn entry_path(config_dir: &Path) -> PathBuf {
    config_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("autostart/Coucou.desktop")
}

pub fn enabled() -> bool {
    entry_path(&crate::settings::config_dir()).is_file()
}

pub fn set(enabled: bool) -> Result<(), String> {
    let path = entry_path(&crate::settings::config_dir());
    if enabled {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        write_entry(&path, &executable)
    } else {
        remove_entry(&path)
    }
}

fn remove_entry(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot remove Coucou's autostart entry: {error}")),
    }
}

fn exec_value(executable: &Path) -> Result<String, String> {
    let executable = executable
        .to_str()
        .ok_or_else(|| "Autostart requires a UTF-8 executable path.".to_string())?;
    if executable.chars().any(|c| c.is_control() || c == '=') {
        return Err("Autostart cannot represent this executable path in a desktop entry.".into());
    }
    // Desktop entries unescape string values first, then parse Exec quoting:
    // https://specifications.freedesktop.org/desktop-entry/latest/exec-variables.html
    let mut quoted = String::from("\"");
    for character in executable.chars() {
        match character {
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(character);
            }
            '%' => quoted.push_str("%%"),
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

fn write_entry(path: &Path, executable: &Path) -> Result<(), String> {
    let executable = exec_value(executable)?;
    let parent = path.parent().ok_or("Autostart directory is missing.")?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("Cannot create the autostart directory: {e}"))?;
    let temporary = parent.join(format!(
        ".Coucou.desktop.{}.{}.tmp",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed),
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&temporary)
        .map_err(|e| format!("Cannot create the autostart entry: {e}"))?;
    let result = (|| -> io::Result<()> {
        write!(file, "[Desktop Entry]\nType=Application\nName=Coucou\nComment=Mochi companion for local Codex sessions\nExec={executable}\nIcon=coucou\nTerminal=false\nCategories=Development;Utility;\nStartupNotify=false\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("Cannot save Coucou's autostart entry: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_uses_config_parent_and_installer_filename() {
        assert_eq!(
            entry_path(Path::new("/custom/config/coucou")),
            PathBuf::from("/custom/config/autostart/Coucou.desktop")
        );
    }

    #[test]
    fn executable_quotes_desktop_reserved_characters() {
        assert_eq!(
            exec_value(Path::new("/home/a b/coucou")).unwrap(),
            "\"/home/a b/coucou\""
        );
        assert_eq!(
            exec_value(Path::new("/a\\b\"$`%/coucou")).unwrap(),
            "\"/a\\\\\\\\b\\\\\"\\\\$\\\\`%%/coucou\""
        );
        assert!(exec_value(Path::new("/a\nHidden=true")).is_err());
        assert!(exec_value(Path::new("/a=b/coucou")).is_err());
    }

    #[test]
    fn enable_replaces_atomically_and_disable_only_removes_coucou() {
        let root = std::env::temp_dir().join(format!(
            "coucou-autostart-test-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("Coucou.desktop");
        let other = root.join("other.desktop");
        fs::write(&other, "preserved").unwrap();
        write_entry(&path, Path::new("/installed/coucou")).unwrap();
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("Exec=\"/installed/coucou\"\n"));
        write_entry(&path, Path::new("/updated/coucou")).unwrap();
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("Exec=\"/updated/coucou\"\n"));
        remove_entry(&path).unwrap();
        remove_entry(&path).unwrap();
        assert_eq!(fs::read_to_string(&other).unwrap(), "preserved");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_file(&other).unwrap();
        fs::remove_dir(&root).unwrap();
    }
}
