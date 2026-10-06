//! Show an existing Codex desktop workspace without moving the pointer.

use crate::hyprland::{dispatch_preserving_cursor, request};
use serde::Deserialize;
use std::cmp::Reverse;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Workspace {
    #[serde(default)]
    id: i64,
    name: String,
    // Newer Hyprland separates the stable address from the display name.
    address: Option<String>,
}

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
    #[serde(default)]
    pinned: bool,
    workspace: Option<Workspace>,
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

fn workspace_selector(workspace: &Workspace) -> Result<String, String> {
    if let Some(address) = workspace.address.as_ref().filter(|value| !value.is_empty()) {
        return Ok(address.clone());
    }
    if workspace.id > 0 {
        return Ok(workspace.id.to_string());
    }
    if workspace.name.starts_with("special:") || workspace.name == "special" {
        return Ok(workspace.name.clone());
    }
    if !workspace.name.is_empty() {
        return Ok(format!("name:{}", workspace.name));
    }
    Err("The Codex window has no available workspace.".into())
}

fn workspace_visible(workspace: &Workspace, monitors: &serde_json::Value) -> bool {
    monitors.as_array().is_some_and(|monitors| {
        monitors.iter().any(|monitor| {
            ["activeWorkspace", "specialWorkspace"].iter().any(|key| {
                let active = &monitor[key];
                if let Some(address) = workspace.address.as_ref().filter(|value| !value.is_empty())
                {
                    active["address"].as_str() == Some(address.as_str())
                } else {
                    workspace.id != 0 && active["id"].as_i64() == Some(workspace.id)
                }
            })
        })
    })
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
    if client.pinned {
        return Ok(());
    }
    let workspace = client
        .workspace
        .as_ref()
        .ok_or("The Codex window has no available workspace.")?;
    let monitors = serde_json::from_str(&request("j/monitors")?)
        .map_err(|error| format!("Cannot read Hyprland monitors: {error}"))?;
    // Repeated clicks must not trigger workspace_back_and_forth, hide a special
    // workspace, or activate another window on an already visible workspace.
    if workspace_visible(workspace, &monitors) {
        return Ok(());
    }
    let target = workspace_selector(workspace)?;
    // Resolve the current workspace again inside Hyprland in case the window
    // moved since j/clients. Only the validated hex address enters Lua code.
    let lua = format!(
        "local w = hl.get_window('address:{}'); \
         assert(w and w.workspace, 'The Codex window has no available workspace.'); \
         if not w.workspace.active then \
         return hl.dispatch(hl.dsp.focus({{workspace=w.workspace}})) end",
        client.address
    );
    dispatch_preserving_cursor(&lua, &format!("workspace {target}"))
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
    fn workspace_targets_support_numbered_named_special_and_renamed_workspaces() {
        for (json, expected) in [
            (r#"{"id":3,"name":"Code"}"#, "3"),
            (
                r#"{"id":-1337,"name":"Code projects"}"#,
                "name:Code projects",
            ),
            (r#"{"id":-99,"name":"special:scratch"}"#, "special:scratch"),
            (r#"{"name":"Renamed","address":"name:Code"}"#, "name:Code"),
        ] {
            let workspace = serde_json::from_str(json).unwrap();
            assert_eq!(workspace_selector(&workspace).unwrap(), expected);
        }
        let missing = serde_json::from_str(r#"{"id":0,"name":""}"#).unwrap();
        assert!(workspace_selector(&missing).is_err());
    }

    #[test]
    fn visible_workspaces_are_unchanged_including_other_monitors_and_specials() {
        let monitors = serde_json::json!([
            {"activeWorkspace":{"id":1},"specialWorkspace":{"id":0}},
            {"activeWorkspace":{"id":3,"address":"3"},
             "specialWorkspace":{"id":-99,"address":"special:scratch"}}
        ]);
        for json in [
            r#"{"id":1,"name":"1"}"#,
            r#"{"id":3,"name":"Code","address":"3"}"#,
            r#"{"id":-99,"name":"special:scratch"}"#,
        ] {
            let workspace = serde_json::from_str(json).unwrap();
            assert!(workspace_visible(&workspace, &monitors));
        }
        for json in [
            r#"{"id":2,"name":"2"}"#,
            r#"{"id":0,"name":""}"#,
            r#"{"id":0,"name":"","address":""}"#,
            r#"{"id":3,"name":"Code","address":"name:Other"}"#,
        ] {
            let workspace = serde_json::from_str(json).unwrap();
            assert!(!workspace_visible(&workspace, &monitors));
        }
    }

    #[test]
    #[ignore = "explicitly switches the real desktop workspace"]
    fn focus_existing_codex_window_live() {
        assert_eq!(std::env::var("COUCOU_TEST_CODEX_FOCUS").as_deref(), Ok("1"));
        let cwd = std::env::var("COUCOU_TEST_CODEX_CWD").ok();
        let path = cwd.as_ref().map(PathBuf::from);
        let path = path.map(|path| path.canonicalize().unwrap_or(path));
        let windows = clients(&request("j/clients").unwrap());
        let target = select_client(&windows, path.as_deref(), |pid| {
            std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
        })
        .unwrap();
        let target_workspace = target.workspace.as_ref().unwrap();
        let cursor_options = || {
            [
                "no_warps",
                "warp_on_change_workspace",
                "warp_on_monitor_change",
                "warp_on_toggle_special",
            ]
            .map(|key| {
                let value: serde_json::Value =
                    serde_json::from_str(&request(&format!("j/getoption cursor:{key}")).unwrap())
                        .unwrap();
                (value["bool"].clone(), value["int"].clone())
            })
        };
        let options = cursor_options();
        let before = request("j/cursorpos").unwrap();
        if let Ok(workspace) = std::env::var("COUCOU_TEST_CODEX_FROM_WORKSPACE") {
            let workspace: u32 = workspace.parse().unwrap();
            assert!(workspace > 0);
            assert_ne!(
                workspace_selector(target_workspace).unwrap(),
                workspace.to_string()
            );
            dispatch_preserving_cursor(
                &format!("return hl.dispatch(hl.dsp.focus({{workspace={workspace}}}))"),
                &format!("workspace {workspace}"),
            )
            .unwrap();
            assert_eq!(request("j/cursorpos").unwrap(), before);
            let monitors = serde_json::from_str(&request("j/monitors").unwrap()).unwrap();
            assert!(!workspace_visible(target_workspace, &monitors));
        }
        focus(cwd.as_deref()).unwrap();
        let monitors = serde_json::from_str(&request("j/monitors").unwrap()).unwrap();
        assert!(workspace_visible(target_workspace, &monitors));
        assert_eq!(request("j/cursorpos").unwrap(), before);
        let workspace = request("j/activeworkspace").unwrap();
        focus(cwd.as_deref()).unwrap();
        assert_eq!(request("j/activeworkspace").unwrap(), workspace);
        assert_eq!(request("j/cursorpos").unwrap(), before);
        assert_eq!(cursor_options(), options);
    }
}
