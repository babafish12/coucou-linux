// Small local log at $XDG_DATA_HOME/coucou/coucou.log. Nothing leaves the machine.

use std::io::Write;

use crate::{private_fs, settings};

/// API names and UI errors may contain control characters. Keep every entry on
/// one line and prevent terminal escape sequences when inspecting the log.
fn escaped_for_log(message: &str) -> String {
    use std::fmt::Write;
    let mut escaped = String::with_capacity(message.len());
    for ch in message.chars() {
        if ch.is_ascii_control() {
            let _ = write!(escaped, "\\u{:04X}", ch as u32);
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

pub fn line(message: impl AsRef<str>) {
    let stamp = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());
    let dir = settings::local_dir();
    if private_fs::ensure_private_dir(&dir).is_err() {
        return;
    }
    let path = dir.join("coucou.log");
    // Keep it from growing forever: start fresh past ~1 MB.
    if std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut file) = private_fs::open_private_file(&path, true) {
        let _ = writeln!(file, "{stamp} {}", escaped_for_log(message.as_ref()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_text_cannot_inject_log_lines_or_terminal_commands() {
        assert_eq!(
            escaped_for_log("Mochi 🐥\nforged\r\t\u{1b}[31m\u{7f}"),
            "Mochi 🐥\\u000Aforged\\u000D\\u0009\\u001B[31m\\u007F"
        );
    }
}
