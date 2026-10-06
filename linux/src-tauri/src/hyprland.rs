//! Scoped Hyprland actions that never request pointer movement.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const LUA_PROBE: &str = "/eval assert(type(hl.get_config)=='function' and type(hl.config)=='function' and type(hl.dispatch)=='function', 'Cursor-safe dispatch is unavailable')";
const CURSOR_OPTIONS: [&str; 4] = [
    "no_warps",
    "warp_on_change_workspace",
    "warp_on_monitor_change",
    "warp_on_toggle_special",
];

pub(crate) fn request(command: &str) -> Result<String, String> {
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

// Execute the suppression, action and restoration in one compositor request.
// Restoring cursor coordinates afterwards would still cause a visible jump and
// could overwrite physical mouse movement that happened during the request.
fn guarded_lua(action: &str) -> String {
    format!(
        "/eval local keys={{'no_warps','warp_on_change_workspace','warp_on_monitor_change','warp_on_toggle_special'}}; \
        local previous,safe={{}},{{}}; for _,key in ipairs(keys) do \
        local value=hl.get_config('cursor.'..key); if value~=nil then \
        previous[key]=value; safe[key]=key=='no_warps' and true or 0 end end; \
        assert(previous.no_warps~=nil, 'Missing cursor option: no_warps'); \
        local ok,result=pcall(function() \
        hl.config({{cursor=safe}}); \
        for key,expected in pairs(safe) do \
        assert(hl.get_config('cursor.'..key)==expected, 'Could not disable cursor warping') end; \
        {action} \
        end); \
        hl.config({{cursor=previous}}); \
        for key,value in pairs(previous) do assert(hl.get_config('cursor.'..key)==value, 'Could not restore cursor option: '..key) end; \
        if not ok then error(result) end; \
        if type(result)=='table' and result.ok==false then error(result.error or 'Hyprland dispatch failed') end"
    )
}

pub(crate) fn dispatch_preserving_cursor(
    lua_action: &str,
    legacy_action: &str,
) -> Result<(), String> {
    dispatch_with(lua_action, legacy_action, request)
}

fn dispatch_with(
    lua_action: &str,
    legacy_action: &str,
    mut send: impl FnMut(&str) -> Result<String, String>,
) -> Result<(), String> {
    let probe = send(LUA_PROBE)?;
    match probe.trim() {
        "ok" => {
            let response = send(&guarded_lua(lua_action))?;
            if response.trim() == "ok" {
                Ok(())
            } else {
                Err(format!("Hyprland action failed: {}", response.trim()))
            }
        }
        // An action failure is never a reason to retry through another API.
        "unknown request" | "eval is only supported with the lua config manager" => {
            dispatch_legacy(legacy_action, &mut send)
        }
        _ => Err(format!(
            "Cannot safely disable Hyprland cursor warping: {}",
            probe.trim()
        )),
    }
}

fn dispatch_legacy(
    action: &str,
    send: &mut impl FnMut(&str) -> Result<String, String>,
) -> Result<(), String> {
    if action.is_empty()
        || action
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, ';' | '\\'))
        || action.contains("[[BATCH]]")
    {
        return Err("Invalid Hyprland action.".into());
    }
    let mut previous = Vec::new();
    for key in CURSOR_OPTIONS {
        let response = send(&format!("j/getoption cursor:{key}"))?;
        if key != "no_warps" && response.trim() == "no such option" {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&response)
            .map_err(|_| format!("Cannot read Hyprland cursor option {key}."))?;
        let value = value["int"]
            .as_i64()
            .or_else(|| value["bool"].as_bool().map(i64::from))
            .ok_or_else(|| format!("Invalid Hyprland cursor option {key}."))?;
        previous.push((key, value));
    }
    let mut commands = Vec::new();
    for (key, _) in &previous {
        let value = if *key == "no_warps" { 1 } else { 0 };
        commands.push(format!("/keyword cursor:{key} {value}"));
    }
    commands.push(format!("/dispatch {action}"));
    for (key, value) in &previous {
        commands.push(format!("/keyword cursor:{key} {value}"));
    }
    // Hyprland processes every batch command, including restores after an
    // unsuccessful dispatch, before replying or processing another IPC request.
    let response = send(&format!("[[BATCH]]{}", commands.join(";")))?;
    let replies: Vec<_> = response
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if replies.len() == commands.len() && replies.iter().all(|reply| *reply == "ok") {
        Ok(())
    } else {
        Err(format!("Hyprland action failed: {}", response.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_runtime_errors_never_retry_a_legacy_dispatch() {
        let mut requests = Vec::new();
        let result = dispatch_with("error('gone')", "workspace 2", |command| {
            requests.push(command.to_string());
            Ok(if command == LUA_PROBE { "ok" } else { "gone" }.into())
        });
        assert!(result.unwrap_err().contains("gone"));
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|command| command.starts_with("/eval ")));
    }

    #[test]
    fn transport_or_capability_errors_do_not_mutate_configuration() {
        for response in [Err("timeout".into()), Ok("Lua API missing".into())] {
            let mut calls = 0;
            assert!(dispatch_with("return nil", "workspace 2", |_| {
                calls += 1;
                response.clone()
            })
            .is_err());
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn legacy_batch_restores_original_values_even_when_dispatch_fails() {
        let mut batch = String::new();
        let result = dispatch_with("return nil", "workspace 2", |command| {
            if command == LUA_PROBE {
                return Ok("unknown request".into());
            }
            if command.starts_with("j/getoption ") {
                return Ok(if command.ends_with("no_warps") {
                    r#"{"bool":true}"#.into()
                } else if command.ends_with("warp_on_monitor_change") {
                    r#"{"int":-1}"#.into()
                } else {
                    r#"{"int":2}"#.into()
                });
            }
            batch = command.into();
            Ok("ok\nok\nok\nok\nInvalid workspace\nok\nok\nok\nok".into())
        });
        assert!(result.is_err());
        assert_eq!(batch, "[[BATCH]]/keyword cursor:no_warps 1;/keyword cursor:warp_on_change_workspace 0;/keyword cursor:warp_on_monitor_change 0;/keyword cursor:warp_on_toggle_special 0;/dispatch workspace 2;/keyword cursor:no_warps 1;/keyword cursor:warp_on_change_workspace 2;/keyword cursor:warp_on_monitor_change -1;/keyword cursor:warp_on_toggle_special 2");
    }

    #[test]
    fn legacy_supports_missing_optional_options_but_requires_no_warps() {
        for missing_required in [false, true] {
            let mut batch_sent = false;
            let result = dispatch_with("return nil", "workspace 2", |command| {
                if command == LUA_PROBE {
                    Ok("eval is only supported with the lua config manager".into())
                } else if command.ends_with("cursor:no_warps") && !missing_required {
                    Ok(r#"{"int":0}"#.into())
                } else if command.starts_with("j/getoption ") {
                    Ok("no such option".into())
                } else {
                    batch_sent = true;
                    assert_eq!(command, "[[BATCH]]/keyword cursor:no_warps 1;/dispatch workspace 2;/keyword cursor:no_warps 0");
                    Ok("ok\n\n\nok\n\n\nok".into())
                }
            });
            assert_eq!(result.is_ok(), !missing_required);
            assert_eq!(batch_sent, !missing_required);
        }
    }

    #[test]
    fn legacy_rejects_batch_injection_before_setting_options() {
        for action in [
            "workspace 2;exec bad",
            "workspace 2\nexec bad",
            "workspace \\2",
            "[[BATCH]]workspace 2",
        ] {
            let mut calls = 0;
            assert!(dispatch_with("return nil", action, |_| {
                calls += 1;
                Ok("unknown request".into())
            })
            .is_err());
            assert_eq!(calls, 1);
        }
    }
}
