use std::process::Stdio;
use std::time::Duration;

fn session_url(session_id: &str) -> Result<String, String> {
    let valid = session_id.len() == 36
        && session_id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    if !valid {
        return Err("This session has no valid Codex chat ID.".into());
    }
    Ok(format!("codex://threads/{session_id}"))
}

pub async fn open_session(session_id: &str) -> Result<(), String> {
    let url = session_url(session_id)?;
    // Codex Desktop registers this URI scheme and focuses the requested chat.
    // Pass the URL as one argument; rollout metadata must never become shell code.
    let mut command = tokio::process::Command::new("xdg-open");
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(10), command.status())
        .await
        .map_err(|_| {
            "Opening the Codex chat timed out. Check that Codex Desktop is installed.".to_string()
        })?
        .map_err(|_| "Could not launch xdg-open to open the Codex chat.".to_string())?;
    if !status.success() {
        return Err(
            "Could not open the Codex chat. Install Codex Desktop with its codex:// link handler."
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_the_exact_session() {
        let id = "01957a3b-832b-7000-9bdc-13d031ff6a42";
        assert_eq!(session_url(id).unwrap(), format!("codex://threads/{id}"));
    }

    #[test]
    fn rejects_missing_ids_and_url_or_shell_injection() {
        for id in [
            "",
            "new",
            "../settings",
            "01957a3b-832b-7000-9bdc-13d031ff6a42?prompt=run",
            "$(touch /tmp/injected)",
            "01957a3b_832b_7000_9bdc_13d031ff6a42",
            "01957a3b-832b-7000-9bdc-13d031ff6a4z",
        ] {
            assert!(session_url(id).is_err(), "accepted invalid ID {id:?}");
        }
    }
}
