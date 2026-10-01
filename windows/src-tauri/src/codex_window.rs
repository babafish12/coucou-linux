//! Focus an existing Codex desktop window without opening a working folder.

use serde::Deserialize;
use std::cmp::Reverse;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Client {
    address: String,
    class: String,
    #[serde(default)]
    initial_class: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    pid: u32,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default = "unknown_focus_order", rename = "focusHistoryID")]
    focus_history_id: i64,
}

fn unknown_focus_order() -> i64 {
    i64::MAX
}

fn codex_class(class: &str) -> bool {
    matches!(
        class.to_ascii_lowercase().as_str(),
        "codex" | "com.openai.codex" | "chatgpt" | "com.openai.chatgpt"
    )
}

fn valid_address(address: &str) -> bool {
    address.strip_prefix("0x").is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn select_client<'a>(
    clients: &'a [Client],
    cwd: Option<&Path>,
    process_cwd: impl Fn(u32) -> Option<PathBuf>,
) -> Option<&'a Client> {
    clients
        .iter()
        .filter(|client| {
            client.mapped
                && !client.hidden
                && valid_address(&client.address)
                && (codex_class(&client.class) || codex_class(&client.initial_class))
        })
        .max_by_key(|client| {
            let match_score = cwd.map_or(0, |cwd| {
                if process_cwd(client.pid).as_deref() == Some(cwd) {
                    return 3;
                }
                let title = client.title.to_lowercase();
                let full = cwd.to_string_lossy().to_lowercase();
                if full.len() > 1 && title.contains(&full) {
                    return 2;
                }
                let name = cwd
                    .file_name()
                    .map(|name| name.to_string_lossy().to_lowercase());
                if name.is_some_and(|name| name.len() > 1 && title.contains(&name)) {
                    1
                } else {
                    0
                }
            });
            let specific = client.class.to_ascii_lowercase().contains("codex")
                || client.initial_class.to_ascii_lowercase().contains("codex");
            let order = if client.focus_history_id < 0 {
                i64::MAX
            } else {
                client.focus_history_id
            };
            (
                match_score,
                specific,
                Reverse(order),
                Reverse(client.address.as_str()),
            )
        })
}

fn request(command: &str) -> Result<String, String> {
    let runtime =
        std::env::var_os("XDG_RUNTIME_DIR").ok_or("Hyprland runtime directory is unavailable.")?;
    let instance = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| "Focusing Codex currently requires a Hyprland session.".to_string())?;
    if instance.is_empty() || instance.contains('/') || instance == "." || instance == ".." {
        return Err("Invalid Hyprland instance identifier.".into());
    }
    let socket = PathBuf::from(runtime)
        .join("hypr")
        .join(instance)
        .join(".socket.sock");
    let mut stream =
        UnixStream::connect(socket).map_err(|error| format!("Cannot reach Hyprland: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(command.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream
        .take(2 * 1024 * 1024)
        .read_to_string(&mut response)
        .map_err(|error| error.to_string())?;
    Ok(response)
}

pub fn focus(cwd: Option<&str>) -> Result<(), String> {
    let clients: Vec<Client> = serde_json::from_str(&request("j/clients")?)
        .map_err(|error| format!("Cannot read Hyprland windows: {error}"))?;
    let cwd = cwd.filter(|value| !value.is_empty()).map(PathBuf::from);
    let cwd = cwd.map(|path| path.canonicalize().unwrap_or(path));
    let client = select_client(&clients, cwd.as_deref(), |pid| {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    })
    .ok_or("No Codex desktop window is open. Open Codex, then try again.")?;
    let selector = format!("address:{}", client.address);
    // Current Hyprland uses Lua dispatchers; older releases use focuswindow.
    let lua = format!("/eval hl.dispatch(hl.dsp.focus({{window='{selector}'}}))");
    if request(&lua).is_ok_and(|response| response.trim() == "ok") {
        return Ok(());
    }
    let response = request(&format!("/dispatch focuswindow {selector}"))?;
    if response.trim() == "ok" {
        Ok(())
    } else {
        Err(format!(
            "Hyprland could not focus Codex: {}",
            response.trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clients(value: &str) -> Vec<Client> {
        serde_json::from_str(value).unwrap()
    }

    #[test]
    fn finds_the_combined_chatgpt_codex_desktop_by_application_class() {
        let windows = clients(
            r#"[
            {"address":"0x10","class":"firefox","title":"Codex","mapped":true},
            {"address":"0x20","class":"Chatgpt","title":"ChatGPT","mapped":true}
        ]"#,
        );
        assert_eq!(
            select_client(&windows, None, |_| None).unwrap().address,
            "0x20"
        );
    }

    #[test]
    fn project_match_precedes_app_class_and_recent_focus_breaks_ties() {
        let windows = clients(
            r#"[
            {"address":"0x10","class":"Codex","title":"Other","mapped":true,"focusHistoryID":0},
            {"address":"0x20","class":"Chatgpt","title":"coucou","mapped":true,"focusHistoryID":4},
            {"address":"0x30","class":"Codex","title":"coucou","mapped":true,"focusHistoryID":1}
        ]"#,
        );
        assert_eq!(
            select_client(&windows, Some(Path::new("/work/coucou")), |_| None)
                .unwrap()
                .address,
            "0x30"
        );
        assert_eq!(
            select_client(&windows, None, |_| None).unwrap().address,
            "0x10"
        );
    }

    #[test]
    fn process_working_directory_is_stronger_than_a_title_hint() {
        let windows = clients(
            r#"[
            {"address":"0x10","class":"Codex","title":"coucou","mapped":true,"pid":1},
            {"address":"0x20","class":"Chatgpt","title":"ChatGPT","mapped":true,"pid":2}
        ]"#,
        );
        assert_eq!(
            select_client(&windows, Some(Path::new("/work/coucou")), |pid| {
                (pid == 2).then(|| PathBuf::from("/work/coucou"))
            })
            .unwrap()
            .address,
            "0x20"
        );
    }

    #[test]
    fn excludes_unmapped_hidden_and_injected_window_addresses() {
        let windows = clients(
            r#"[
            {"address":"0x10","class":"Codex","mapped":false},
            {"address":"0x20","class":"Codex","mapped":true,"hidden":true},
            {"address":"0x30'; os.exit()","class":"Codex","mapped":true},
            {"address":"0x40","class":"terminal","title":"Codex","mapped":true}
        ]"#,
        );
        assert!(select_client(&windows, None, |_| None).is_none());
    }

    #[test]
    #[ignore = "explicitly focuses the real desktop window"]
    fn focus_existing_codex_window_live() {
        assert_eq!(std::env::var("COUCOU_TEST_CODEX_FOCUS").as_deref(), Ok("1"));
        focus(std::env::var("COUCOU_TEST_CODEX_CWD").ok().as_deref()).unwrap();
    }
}
