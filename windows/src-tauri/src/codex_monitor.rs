// Read-only adapter for local Codex Desktop/CLI rollout logs. These logs are an
// implementation detail, not an approval API. Only public messages and bounded,
// sanitized action summaries leave this module; reasoning and tool output do not.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

const MAX_SESSIONS: usize = 32;
const MAX_CANDIDATES: usize = MAX_SESSIONS * 4;
const MAX_ENTRIES: usize = 80;
const MAX_TEXT: usize = 240;
const MAX_DETAIL: usize = 1200;
const READ_BUDGET: u64 = 256 * 1024;
const BOOTSTRAP_BYTES: u64 = 512 * 1024;
const MAX_LINE: usize = 1024 * 1024;
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);
const STALE_AFTER: Duration = Duration::from_secs(15 * 60);
const FILE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
static RUNNING: AtomicBool = AtomicBool::new(false);
static SNAPSHOT_REQUESTED: AtomicBool = AtomicBool::new(false);
static ACTIVITY: LazyLock<Mutex<ActivitySnapshot>> =
    LazyLock::new(|| Mutex::new(ActivitySnapshot::default()));

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySnapshot {
    pub sessions: Vec<ActivitySession>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySession {
    pub id: String,
    pub cwd: String,
    pub title: String,
    pub state: ActivityState,
    pub updated_at: u64,
    pub entries: Vec<ActivityEntry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityState {
    Working,
    Thinking,
    Finished,
    Interrupted,
    #[default]
    Idle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub id: String,
    pub kind: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub timestamp: u64,
}

pub fn snapshot() -> ActivitySnapshot {
    ACTIVITY.lock().unwrap().clone()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub available: bool,
    pub sessions_path: String,
    pub mode: &'static str,
    pub approvals_supported: bool,
}

fn sessions_path() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".codex")
        })
        .join("sessions")
}

pub fn status() -> Status {
    let path = sessions_path();
    Status {
        available: fs::read_dir(&path).is_ok(),
        sessions_path: path.to_string_lossy().into_owned(),
        mode: "read-only",
        approvals_supported: false,
    }
}

pub fn start(app: AppHandle) {
    // The webview calls this after its hook listener is registered. Reloads
    // request a fresh snapshot rather than creating duplicate watcher threads.
    if RUNNING.swap(true, Ordering::Relaxed) {
        SNAPSHOT_REQUESTED.store(true, Ordering::Relaxed);
        return;
    }
    // File IO stays off Tauri's async executor and the UI thread.
    std::thread::spawn(move || {
        let mut monitor = Monitor::new(sessions_path());
        loop {
            if SNAPSHOT_REQUESTED.swap(false, Ordering::Relaxed) {
                monitor.foreground = None;
                // Completion may have arrived while the frontend was paused.
                // Clear that stale state even when no session is active now.
                let _ = app.emit_to(
                    crate::island::WINDOW_LABEL,
                    "hook",
                    json!({ "hook_event_name": "SessionEnd", "provider": "codex" }),
                );
            }
            for event in monitor.poll(SystemTime::now()) {
                let _ = app.emit_to(crate::island::WINDOW_LABEL, "hook", event);
            }
            if let Some(update) = monitor.activity_update() {
                let changed = {
                    let mut current = ACTIVITY.lock().unwrap();
                    if *current == update {
                        false
                    } else {
                        *current = update.clone();
                        true
                    }
                };
                if changed {
                    let _ = app.emit_to(crate::island::WINDOW_LABEL, "codex-activity", update);
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

fn shorten(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

// Redaction is deliberately conservative. Tool summaries additionally use an
// allowlist below: arbitrary arguments, scripts, headers and outputs stay local.
fn public_text(text: &str, limit: usize) -> String {
    let mut lines = Vec::new();
    for line in text.lines().take(40) {
        let line: String = line
            .chars()
            .filter(|c| !c.is_control())
            .take(4096)
            .collect();
        let lower = line.to_ascii_lowercase();
        if [
            "password",
            "passwd",
            "api_key",
            "api-key",
            "apikey",
            "authorization",
            "bearer ",
            "access_token",
            "refresh_token",
            "client_secret",
            "private key",
            "secret=",
            "secret:",
            "token=",
            "token:",
            "token\"",
            "secret\"",
            "credential",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            lines.push("[sensitive content omitted]".to_string());
            continue;
        }
        let words: Vec<_> = line
            .split_whitespace()
            .map(|word| {
                let lower = word.to_ascii_lowercase();
                let secret = [
                    "sk-",
                    "ghp_",
                    "gho_",
                    "github_pat_",
                    "xoxb-",
                    "xoxp-",
                    "akia",
                    "eyj",
                ]
                .iter()
                .any(|prefix| lower.contains(prefix));
                let opaque = word
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
                    .any(|part| part.len() >= 32 && part.chars().any(|c| c.is_ascii_digit()));
                if secret || opaque {
                    "[redacted]".to_string()
                } else if lower.contains("://") {
                    "[link]".to_string()
                } else {
                    word.to_string()
                }
            })
            .collect();
        lines.push(words.join(" "));
    }
    shorten(lines.join("\n").trim(), limit)
}

fn path_detail(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() || path.len() > 1024 || path.chars().any(|c| c.is_control()) {
        return None;
    }
    let mut shown = path.to_string();
    if let Some(home) = std::env::var_os("HOME").and_then(|value| value.into_string().ok()) {
        if let Some(relative) = path.strip_prefix(&format!("{home}/")) {
            shown = format!("~/{relative}");
        }
    }
    Some(public_text(&shown, MAX_TEXT))
}

fn request_title(text: &str) -> Option<String> {
    let text = text.trim();
    let text = if text.starts_with("# Files mentioned by the user:") {
        text.split_once("## My request:")?.1.trim()
    } else {
        text
    };
    if text.is_empty()
        || [
            "# AGENTS.md",
            "<environment_context>",
            "<INSTRUCTIONS>",
            "<user_instructions>",
            "<permissions instructions>",
            "<system_reminder>",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return None;
    }
    Some(public_text(
        text.lines().find(|line| !line.trim().is_empty())?,
        120,
    ))
}

fn epoch_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

// Rollouts use RFC3339 UTC timestamps. Parsing this fixed wire shape avoids a
// date/time dependency in the monitor; malformed times use the file timestamp.
fn timestamp_ms(value: &Value) -> Option<u64> {
    if let Some(milliseconds) = value.as_u64() {
        return Some(milliseconds);
    }
    let raw = value.as_str()?;
    let (date, time) = raw.split_once('T')?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    if date.next().is_some() || !(1970..=9999).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=days[month as usize - 1]).contains(&day) {
        return None;
    }
    let mut time = time.strip_suffix('Z')?.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let seconds = time.next()?;
    let (seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let second: i64 = seconds.parse().ok()?;
    if time.next().is_some()
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&second)
        || !fraction.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let mut millis = fraction.chars().take(3).collect::<String>();
    while millis.len() < 3 {
        millis.push('0');
    }
    let millis: i64 = millis.parse().ok()?;
    let year = year - i64::from(month <= 2);
    let era = year / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    u64::try_from(((days * 24 + hour) * 60 + minute) * 60 * 1000 + second * 1000 + millis).ok()
}

fn entry_id(kind: &str, text: &str, timestamp: u64) -> String {
    let hash = kind
        .bytes()
        .chain(text.bytes())
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    format!("{timestamp}-{hash:x}")
}

fn command_detail(command: &str) -> String {
    let line = command.lines().next().unwrap_or_default().trim();
    let mut tokens = line.split_whitespace();
    let program = tokens.next().unwrap_or("command");
    let program = if program.contains('=') {
        "shell"
    } else {
        program
    };
    let mut parts = vec![public_text(program, 80)];
    let safe = [
        "test",
        "build",
        "check",
        "run",
        "fmt",
        "clippy",
        "ci",
        "install",
        "tauri",
        "diff",
        "status",
        "log",
        "show",
        "ls-files",
        "rev-parse",
        "--workspace",
        "--release",
        "--no-bundle",
        "--noEmit",
        "--offline",
        "--lib",
        "--all-targets",
        "--all-features",
        "--stat",
        "--short",
        "--oneline",
        "--check",
        "--name-only",
        "--version",
        "-n",
        "--files",
    ];
    let safe_program = [
        "cargo", "npm", "pnpm", "yarn", "git", "rg", "cat", "sed", "ls", "pwd", "cd", "head",
        "tail", "wc",
    ]
    .contains(&program);
    let redacted = public_text(line, 2048).contains("[sensitive content omitted]");
    let mut omitted = command.lines().count() > 1;
    for token in tokens.take(40) {
        if token == "&&" || token == ";" || token == "|" || token.starts_with("<<") {
            omitted = true;
            break;
        }
        let path = (token.contains('/')
            || [
                ".rs", ".ts", ".tsx", ".js", ".mjs", ".json", ".md", ".toml", ".py", ".css",
                ".html",
            ]
            .iter()
            .any(|suffix| token.ends_with(suffix)))
            && token
                .chars()
                .all(|c| c.is_alphanumeric() || "/._-*~".contains(c));
        if safe_program && !redacted && (safe.contains(&token) || path) {
            parts.push(if path {
                path_detail(token).unwrap_or_default()
            } else {
                token.to_string()
            });
        } else {
            omitted = true;
        }
    }
    if omitted {
        parts.push("…".into());
    }
    shorten(&parts.join(" "), MAX_TEXT)
}

// Extract literal strings only; never evaluate the code-mode wrapper. Quoted
// scripts become command summaries and patch bodies contribute filenames only.
fn wrapper_literals(input: &str) -> Vec<(&str, String)> {
    let bytes = input.as_bytes();
    let mut literals = Vec::new();
    let mut at = 0;
    while at < bytes.len() && literals.len() < 80 {
        let quote = bytes[at];
        if quote != b'\'' && quote != b'"' && quote != b'`' {
            at += 1;
            continue;
        }
        let start = at;
        at += 1;
        while at < bytes.len() {
            if bytes[at] == b'\\' {
                at += 2;
            } else if bytes[at] == quote {
                break;
            } else {
                at += 1;
            }
        }
        if at >= bytes.len() {
            break;
        }
        let end = at;
        at += 1;
        if quote == b'`' || end - start > 16 * 1024 {
            continue;
        }
        let value = if quote == b'"' {
            serde_json::from_str::<String>(&input[start..=end]).ok()
        } else {
            let mut decoded = String::new();
            let mut chars = input[start + 1..end].chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(c) = chars.next() {
                        decoded.push(match c {
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            c => c,
                        });
                    }
                } else {
                    decoded.push(c);
                }
            }
            Some(decoded)
        };
        if let Some(value) = value {
            literals.push((&input[..start], value));
        }
    }
    literals
}

fn patch_paths(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|line| {
            [
                "*** Update File: ",
                "*** Add File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .and_then(path_detail)
        })
        .take(4)
        .collect()
}

fn tool_summary(payload: &Value) -> (String, Option<String>) {
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Tool");
    let short = name.rsplit('.').next().unwrap_or(name);
    let raw = payload.get("arguments").or_else(|| payload.get("input"));
    let parsed = raw
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let args = parsed.as_ref().or(raw);
    let mut details = Vec::new();
    let mut text = public_text(name, 80);
    if let Some(args) = args.filter(|value| value.is_object()) {
        if let Some(command) = args.get("cmd").or_else(|| args.get("command")) {
            if let Some(command) = command.as_str() {
                text = "Run command".into();
                details.push(command_detail(command));
            } else if let Some(parts) = command.as_array() {
                let parts: Vec<_> = parts.iter().filter_map(Value::as_str).collect();
                text = "Run command".into();
                if let Some(command) = parts
                    .last()
                    .filter(|_| parts.iter().any(|part| *part == "-lc" || *part == "-c"))
                {
                    details.push(command_detail(command));
                } else {
                    details.push(command_detail(&parts.join(" ")));
                }
            }
        }
        for key in [
            "file_path",
            "path",
            "filename",
            "target_file",
            "workdir",
            "cwd",
        ] {
            if let Some(path) = args.get(key).and_then(Value::as_str).and_then(path_detail) {
                details.push(path);
            }
        }
        if let Some(path) = args
            .get("target")
            .and_then(|target| target.get("path"))
            .and_then(Value::as_str)
            .and_then(path_detail)
        {
            details.push(path);
        }
    }
    if let Some(raw) = raw.and_then(Value::as_str) {
        if short == "apply_patch" {
            text = "Edit files".into();
            details.extend(patch_paths(raw));
        } else if short == "exec" {
            text = "Run tools".into();
            for (prefix, value) in wrapper_literals(raw) {
                let prefix = prefix.trim_end();
                if prefix.strip_suffix(':').is_some_and(|prefix| {
                    let prefix = prefix.trim_end();
                    [
                        "cmd",
                        "command",
                        "\"cmd\"",
                        "\"command\"",
                        "'cmd'",
                        "'command'",
                    ]
                    .iter()
                    .any(|key| prefix.ends_with(key))
                }) {
                    details.push(command_detail(&value));
                } else if value.starts_with("*** Begin Patch") {
                    text = "Edit files".into();
                    details.extend(patch_paths(&value));
                }
                if details.len() >= 4 {
                    break;
                }
            }
        }
    }
    details.dedup();
    let detail = (!details.is_empty()).then(|| {
        shorten(
            &details.into_iter().take(4).collect::<Vec<_>>().join("\n"),
            MAX_DETAIL,
        )
    });
    (text, detail)
}

#[derive(Default)]
struct Session {
    id: String,
    cwd: String,
    hidden: bool,
    active: bool,
    tool: Option<String>,
    terminal: Option<&'static str>,
    title: String,
    state: ActivityState,
    updated_at: u64,
    revision: u64,
    entries: VecDeque<ActivityEntry>,
}

impl Session {
    fn event(&self, name: &str) -> Value {
        let mut event = json!({
            "hook_event_name": name,
            "provider": "codex",
            "session_id": self.id,
            "cwd": self.cwd,
        });
        if name == "PreToolUse" {
            event["tool_name"] = json!(self.tool.as_deref().unwrap_or("Codex"));
        }
        event
    }

    fn snapshot(&self) -> Value {
        self.event(if self.tool.is_some() {
            "PreToolUse"
        } else {
            "UserPromptSubmit"
        })
    }

    fn activity(&self) -> ActivitySession {
        ActivitySession {
            id: self.id.clone(),
            cwd: self.cwd.clone(),
            title: if self.title.is_empty() {
                Path::new(&self.cwd)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .unwrap_or("Codex session")
                    .to_string()
            } else {
                self.title.clone()
            },
            state: self.state,
            updated_at: self.updated_at,
            entries: self.entries.iter().cloned().collect(),
        }
    }

    fn touch(&mut self, timestamp: u64) {
        self.updated_at = self.updated_at.max(timestamp);
        self.revision = self.revision.wrapping_add(1);
    }

    fn set_activity_state(&mut self, state: ActivityState, timestamp: u64) {
        if self.state != state {
            self.state = state;
            self.touch(timestamp);
        }
    }

    fn entry(
        &mut self,
        kind: &'static str,
        text: String,
        detail: Option<String>,
        timestamp: u64,
        id: Option<String>,
    ) {
        if text.is_empty() {
            return;
        }
        let id = id.unwrap_or_else(|| {
            entry_id(
                kind,
                &format!("{text}{}", detail.as_deref().unwrap_or_default()),
                timestamp,
            )
        });
        if self.entries.iter().any(|entry| entry.id == id) {
            return;
        }
        self.entries.push_back(ActivityEntry {
            id,
            kind,
            text,
            detail,
            timestamp,
        });
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.touch(timestamp);
    }

    fn message(&mut self, message: &str, timestamp: u64) {
        let message = public_text(message, MAX_DETAIL);
        if message.is_empty() {
            return;
        }
        let text = shorten(&message, MAX_TEXT);
        let detail = (message != text).then_some(message);
        // Public messages may be repeated as response_item, agent_message and
        // task_complete.last_agent_message. Keep one visible copy.
        if self.entries.iter().rev().take(8).any(|entry| {
            entry.kind == "message"
                && entry.text == text
                && entry.detail == detail
                && entry.timestamp.abs_diff(timestamp) <= 5000
        }) {
            return;
        }
        self.entry("message", text, detail, timestamp, None);
    }

    fn user_request(&mut self, text: &str, timestamp: u64) {
        if let Some(title) = request_title(text) {
            if title != self.title {
                self.title = title;
                self.touch(timestamp);
            }
        }
    }

    fn parse(&mut self, line: &[u8]) -> Option<Value> {
        let record: Value = serde_json::from_slice(line).ok()?;
        let payload = record.get("payload")?;
        let kind = record.get("type")?.as_str()?;
        let timestamp = record
            .get("timestamp")
            .and_then(timestamp_ms)
            .or_else(|| payload.get("timestamp").and_then(timestamp_ms))
            .unwrap_or(self.updated_at);
        if kind == "session_meta" {
            if let Some(id) = payload
                .get("id")
                .or_else(|| payload.get("session_id"))
                .and_then(Value::as_str)
            {
                self.id = id.to_owned();
            }
            if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
                self.cwd = public_text(cwd, 1024);
            }
            self.hidden = payload.get("thread_source").and_then(Value::as_str) == Some("subagent")
                || payload.get("source").and_then(|v| v.get("subagent")).is_some()
                // `exec` is also used by Coucou's own chat requests.
                || matches!(payload.get("source").and_then(Value::as_str), Some("exec" | "remote" | "cloud"));
            self.touch(timestamp);
            return None;
        }
        if self.hidden || self.id.is_empty() {
            return None;
        }
        if kind == "turn_context" {
            if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
                let cwd = public_text(cwd, 1024);
                if self.cwd != cwd {
                    self.cwd = cwd;
                    self.touch(timestamp);
                }
            }
            return None;
        }
        // Explicit allowlist: never inspect reasoning, analysis, developer
        // instructions, inter-agent messages, compaction text or tool outputs.
        if payload.get("channel").and_then(Value::as_str) == Some("analysis")
            || payload.get("phase").and_then(Value::as_str) == Some("analysis")
        {
            return None;
        }
        let event = match (kind, payload.get("type")?.as_str()?) {
            ("event_msg", "task_started") | ("event_msg", "user_message") => {
                if let Some(message) = payload.get("message").and_then(Value::as_str) {
                    self.user_request(message, timestamp);
                }
                self.active = true;
                self.tool = None;
                self.terminal = None;
                self.set_activity_state(ActivityState::Thinking, timestamp);
                self.entry("status", "Started working".into(), None, timestamp, None);
                "UserPromptSubmit"
            }
            ("response_item", "message") => {
                let role = payload.get("role").and_then(Value::as_str)?;
                let content = payload.get("content")?.as_array()?;
                if role == "user" {
                    for part in content {
                        if part.get("type").and_then(Value::as_str) == Some("input_text") {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                self.user_request(text, timestamp);
                            }
                        }
                    }
                    return None;
                }
                if role != "assistant" {
                    return None;
                }
                let phase = payload
                    .get("phase")
                    .or_else(|| payload.get("channel"))
                    .and_then(Value::as_str);
                if phase
                    .is_some_and(|phase| !matches!(phase, "commentary" | "final_answer" | "final"))
                {
                    return None;
                }
                let message = content
                    .iter()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                if message.is_empty() {
                    return None;
                }
                self.message(&message, timestamp);
                self.tool = None;
                if matches!(phase, Some("final_answer" | "final")) {
                    let already_finished = !self.active && self.terminal == Some("Stop");
                    self.active = false;
                    self.terminal = Some("Stop");
                    self.set_activity_state(ActivityState::Finished, timestamp);
                    if already_finished {
                        return None;
                    }
                    "Stop"
                } else {
                    self.active = true;
                    self.terminal = None;
                    self.set_activity_state(ActivityState::Thinking, timestamp);
                    "UserPromptSubmit"
                }
            }
            ("event_msg", "agent_message") => {
                self.message(payload.get("message")?.as_str()?, timestamp);
                return None;
            }
            ("response_item", "function_call" | "custom_tool_call") => {
                self.active = true;
                self.terminal = None;
                // The legacy hook retains names only; rich entries are sanitized.
                self.tool = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| name.chars().filter(|c| !c.is_control()).take(80).collect());
                self.set_activity_state(ActivityState::Working, timestamp);
                let (text, detail) = tool_summary(payload);
                let id = payload
                    .get("call_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .map(|id| entry_id("tool", id, 0));
                self.entry("tool", text, detail, timestamp, id);
                "PreToolUse"
            }
            ("response_item", "function_call_output" | "custom_tool_call_output")
                if self.active =>
            {
                self.tool = None;
                self.set_activity_state(ActivityState::Thinking, timestamp);
                "PostToolUse"
            }
            ("event_msg", "task_complete") => {
                if let Some(message) = payload.get("last_agent_message").and_then(Value::as_str) {
                    self.message(message, timestamp);
                }
                let already_finished = !self.active && self.terminal == Some("Stop");
                self.active = false;
                self.tool = None;
                self.terminal = Some("Stop");
                self.set_activity_state(ActivityState::Finished, timestamp);
                self.entry("status", "Finished".into(), None, timestamp, None);
                if already_finished {
                    return None;
                }
                "Stop"
            }
            ("event_msg", "turn_aborted") => {
                self.active = false;
                self.tool = None;
                self.terminal = Some("SessionEnd");
                self.set_activity_state(ActivityState::Interrupted, timestamp);
                self.entry("status", "Interrupted".into(), None, timestamp, None);
                "SessionEnd"
            }
            ("event_msg", "session_end") => {
                self.active = false;
                self.tool = None;
                self.terminal = Some("SessionEnd");
                if !matches!(
                    self.state,
                    ActivityState::Finished | ActivityState::Interrupted
                ) {
                    self.set_activity_state(ActivityState::Idle, timestamp);
                }
                "SessionEnd"
            }
            _ => return None,
        };
        Some(self.event(event))
    }
}

struct Tail {
    session: Session,
    offset: u64,
    partial: Vec<u8>,
    skip_line: bool,
    modified: SystemTime,
}

impl Tail {
    fn open(path: &Path, modified: SystemTime) -> io::Result<Self> {
        let mut tail = Self {
            session: Session::default(),
            offset: 0,
            partial: Vec::new(),
            skip_line: false,
            modified,
        };
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        // The metadata is at the start even when we only replay the bounded tail.
        let mut head = Vec::new();
        BufReader::new(file.by_ref().take(MAX_LINE as u64)).read_until(b'\n', &mut head)?;
        if let Some(end) = head.iter().position(|b| *b == b'\n') {
            tail.session.parse(&head[..end]);
        }
        if tail.session.hidden || tail.session.id.is_empty() {
            tail.offset = len;
            return Ok(tail);
        }
        if tail.session.updated_at == 0 {
            tail.session.updated_at = epoch_ms(modified);
        }
        tail.offset = len.saturating_sub(BOOTSTRAP_BYTES);
        if tail.offset > 0 {
            file.seek(SeekFrom::Start(tail.offset - 1))?;
            let mut previous = [0];
            file.read_exact(&mut previous)?;
            tail.skip_line = previous[0] != b'\n';
        }
        tail.read(path, BOOTSTRAP_BYTES)?;
        Ok(tail)
    }

    fn consume(&mut self, bytes: &[u8]) -> Vec<Value> {
        let mut events = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                if !self.skip_line {
                    if let Some(event) = self.session.parse(&self.partial) {
                        events.push(event);
                    }
                }
                self.partial.clear();
                self.skip_line = false;
            } else if !self.skip_line {
                if self.partial.len() < MAX_LINE {
                    self.partial.push(byte);
                } else {
                    // Skip oversized records without unbounded allocation; the
                    // next complete JSONL record still gets parsed normally.
                    self.partial.clear();
                    self.skip_line = true;
                }
            }
        }
        events
    }

    fn read(&mut self, path: &Path, budget: u64) -> io::Result<Vec<Value>> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        if metadata.len() < self.offset {
            // Truncation starts a new stream, including new session metadata.
            self.offset = 0;
            self.partial.clear();
            self.skip_line = false;
            self.session = Session {
                revision: self.session.revision.wrapping_add(1),
                ..Session::default()
            };
        }
        self.modified = metadata.modified().unwrap_or(self.modified);
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.take(budget).read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        Ok(self.consume(&bytes))
    }

    fn expire(&mut self, now: SystemTime) -> Option<Value> {
        if self.session.active && elapsed(now, self.modified) > STALE_AFTER {
            self.session.active = false;
            self.session.tool = None;
            self.session.terminal = Some("SessionEnd");
            self.session
                .set_activity_state(ActivityState::Idle, self.session.updated_at);
            return Some(self.session.event("SessionEnd"));
        }
        None
    }
}

struct Monitor {
    root: PathBuf,
    tails: HashMap<PathBuf, Tail>,
    excluded: HashSet<PathBuf>,
    foreground: Option<PathBuf>,
    discovered_at: Option<SystemTime>,
    activity_revisions: Vec<(String, u64)>,
}

impl Monitor {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            tails: HashMap::new(),
            excluded: HashSet::new(),
            foreground: None,
            discovered_at: None,
            activity_revisions: Vec::new(),
        }
    }

    fn poll(&mut self, now: SystemTime) -> Vec<Value> {
        let mut pending: HashMap<PathBuf, Vec<Value>> = HashMap::new();
        if self
            .discovered_at
            .map_or(true, |last| elapsed(now, last) >= DISCOVERY_INTERVAL)
        {
            let initial = self.discovered_at.is_none();
            let recent = recent_files(&self.root, now);
            self.excluded
                .retain(|path| recent.iter().any(|(_, candidate)| candidate == path));
            let mut selected = HashSet::new();
            for (modified, path) in recent {
                if selected.len() == MAX_SESSIONS {
                    break;
                }
                if self.tails.contains_key(&path) {
                    selected.insert(path);
                    continue;
                }
                if self.excluded.contains(&path) {
                    continue;
                }
                if let Ok(mut tail) = Tail::open(&path, modified) {
                    // A rollout's source is immutable. Remember excluded paths
                    // so busy subagents never get reparsed every discovery tick.
                    if tail.session.hidden {
                        self.excluded.insert(path);
                        continue;
                    }
                    if tail.session.id.is_empty() {
                        continue;
                    }
                    // Expired state reconstructed from disk is not a live event.
                    tail.expire(now);
                    // Reconstruct existing state silently. Never announce an old
                    // task's completion when Coucou itself starts up.
                    if !initial
                        && elapsed(now, modified) <= DISCOVERY_INTERVAL * 2
                        && !tail.session.hidden
                        && !tail.session.id.is_empty()
                    {
                        if let Some(terminal) = tail.session.terminal {
                            pending.insert(path.clone(), vec![tail.session.event(terminal)]);
                        }
                    }
                    selected.insert(path.clone());
                    self.tails.insert(path, tail);
                }
            }
            self.tails.retain(|path, _| selected.contains(path));
            self.discovered_at = Some(now);
        }
        for (path, tail) in &mut self.tails {
            let events = pending.entry(path.clone()).or_default();
            if let Ok(fresh) = tail.read(path, READ_BUDGET) {
                events.extend(fresh);
            }
            if let Some(event) = tail.expire(now) {
                events.push(event);
            }
            if tail.session.active {
                // A fast follow-up turn can start in the same read as completion
                // of the previous turn. Report its final active state.
                events.retain(|event| {
                    event["hook_event_name"] != "Stop" && event["hook_event_name"] != "SessionEnd"
                });
            }
        }

        // One frontend pill represents Codex. A completion in another session
        // must never put the pill to sleep while any observed session is active.
        let active = self
            .tails
            .iter()
            .filter(|(_, t)| t.session.active && !t.session.hidden && !t.session.id.is_empty())
            .max_by_key(|(_, t)| t.modified)
            .map(|(path, _)| path.clone());
        let latest_event = self
            .tails
            .iter()
            .filter(|(path, _)| pending.get(*path).is_some_and(|events| !events.is_empty()))
            .max_by_key(|(_, t)| t.modified)
            .map(|(path, _)| path.clone());
        let selected = active.or(latest_event).or_else(|| self.foreground.clone());
        let Some(path) = selected else {
            return Vec::new();
        };
        let Some(tail) = self.tails.get(&path) else {
            self.foreground = None;
            return vec![json!({ "hook_event_name": "SessionEnd", "provider": "codex" })];
        };
        let changed = self.foreground.as_ref() != Some(&path);
        self.foreground = Some(path.clone());
        let events = if changed {
            let mut events = vec![tail.session.event("SessionStart")];
            if tail.session.active {
                events.push(tail.session.snapshot());
            } else {
                events.extend(pending.remove(&path).unwrap_or_default());
            }
            events
        } else {
            pending.remove(&path).unwrap_or_default()
        };
        if events.iter().any(|e| e["hook_event_name"] == "SessionEnd") {
            self.foreground = None;
        }
        events
    }

    fn activity_update(&mut self) -> Option<ActivitySnapshot> {
        let mut revisions: Vec<_> = self
            .tails
            .values()
            .filter(|tail| !tail.session.hidden && !tail.session.id.is_empty())
            .map(|tail| (tail.session.id.clone(), tail.session.revision))
            .collect();
        revisions.sort_unstable();
        if revisions == self.activity_revisions {
            return None;
        }
        self.activity_revisions = revisions;
        let mut sessions: Vec<_> = self
            .tails
            .values()
            .filter(|tail| !tail.session.hidden && !tail.session.id.is_empty())
            .map(|tail| tail.session.activity())
            .collect();
        sessions.sort_unstable_by(|a, b| {
            let active = |session: &ActivitySession| {
                matches!(
                    session.state,
                    ActivityState::Working | ActivityState::Thinking
                )
            };
            active(b)
                .cmp(&active(a))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        Some(ActivitySnapshot { sessions })
    }
}

fn elapsed(now: SystemTime, before: SystemTime) -> Duration {
    now.duration_since(before).unwrap_or_default()
}

fn recent_files(root: &Path, now: SystemTime) -> Vec<(SystemTime, PathBuf)> {
    fn visit(dir: &Path, depth: u8, now: SystemTime, files: &mut Vec<(SystemTime, PathBuf)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() && depth < 3 {
                visit(&path, depth + 1, now, files);
            } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
                let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
                    continue;
                };
                if elapsed(now, modified) <= FILE_MAX_AGE {
                    files.push((modified, path));
                    if files.len() > MAX_CANDIDATES * 2 {
                        files.sort_unstable_by(|a, b| b.0.cmp(&a.0));
                        files.truncate(MAX_CANDIDATES);
                    }
                }
            }
        }
    }
    let mut files = Vec::new();
    visit(root, 0, now, &mut files);
    files.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    files.truncate(MAX_CANDIDATES);
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "coucou-monitor-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str, content: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, content).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn meta(id: &str) -> String {
        json!({"type":"session_meta","payload":{"id":id,"cwd":"/work/project","source":"cli"}})
            .to_string()
            + "\n"
    }
    fn event(name: &str) -> String {
        json!({"type":"event_msg","payload":{"type":name}}).to_string() + "\n"
    }
    fn append(path: &Path, text: &str) {
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    fn names(events: &[Value]) -> Vec<&str> {
        events
            .iter()
            .map(|event| event["hook_event_name"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn translates_lifecycle_without_exposing_content_or_approvals() {
        let mut session = Session::default();
        session.parse(meta("session-1").as_bytes());
        assert_eq!(
            session.parse(event("task_started").as_bytes()).unwrap()["hook_event_name"],
            "UserPromptSubmit"
        );
        let tool = br#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"SECRET"}}"#;
        let output = session.parse(tool).unwrap();
        assert_eq!(output["tool_name"], "exec_command");
        assert!(!output.to_string().contains("SECRET"));
        assert!(session.parse(br#"{"type":"event_msg","payload":{"type":"permission_request","message":"SECRET"}}"#).is_none());
        assert_eq!(
            session.parse(event("task_complete").as_bytes()).unwrap()["hook_event_name"],
            "Stop"
        );
        assert!(!session.active);
    }

    #[test]
    fn ignores_subagents_and_exec_sessions() {
        for source in [json!({"subagent":{"thread_spawn":{}}}), json!("exec")] {
            let mut session = Session::default();
            session.parse(
                json!({"type":"session_meta","payload":{"id":"hidden","source":source}})
                    .to_string()
                    .as_bytes(),
            );
            assert!(session.parse(event("task_started").as_bytes()).is_none());
            assert!(!session.active);
        }
    }

    #[test]
    fn incremental_reader_waits_for_newline_and_does_not_repeat_events() {
        let fixture = Fixture::new();
        let path = fixture.file("session.jsonl", &meta("a"));
        let mut tail = Tail::open(&path, SystemTime::now()).unwrap();
        let start = event("task_started");
        append(&path, &start[..start.len() - 1]);
        assert!(tail.read(&path, READ_BUDGET).unwrap().is_empty());
        append(&path, "\n");
        assert_eq!(
            names(&tail.read(&path, READ_BUDGET).unwrap()),
            ["UserPromptSubmit"]
        );
        assert!(tail.read(&path, READ_BUDGET).unwrap().is_empty());
    }

    #[test]
    fn recovers_after_truncation_and_oversized_or_invalid_records() {
        let fixture = Fixture::new();
        let path = fixture.file(
            "session.jsonl",
            &(meta("old") + &event("task_started") + &" ".repeat(256)),
        );
        let mut tail = Tail::open(&path, SystemTime::now()).unwrap();
        fs::write(&path, meta("new") + &event("task_complete")).unwrap();
        assert_eq!(names(&tail.read(&path, READ_BUDGET).unwrap()), ["Stop"]);
        assert_eq!(tail.session.id, "new");
        let invalid = [
            vec![b'x'; MAX_LINE + 1],
            b"\nnot json\n".to_vec(),
            event("task_started").into_bytes(),
        ]
        .concat();
        assert_eq!(names(&tail.consume(&invalid)), ["UserPromptSubmit"]);
        assert!(tail.partial.is_empty());
    }

    #[test]
    fn startup_does_not_replay_finished_tasks_and_expiry_is_neutral() {
        let fixture = Fixture::new();
        let path = fixture.file(
            "session.jsonl",
            &(meta("a") + &event("task_started") + &event("task_complete")),
        );
        let mut monitor = Monitor::new(fixture.0.clone());
        let now = SystemTime::now();
        assert!(monitor.poll(now).is_empty());
        append(&path, &event("task_started"));
        assert_eq!(
            names(&monitor.poll(now)),
            ["SessionStart", "UserPromptSubmit"]
        );
        assert_eq!(
            names(&monitor.poll(now + STALE_AFTER + Duration::from_secs(1))),
            ["SessionEnd"]
        );
        assert!(monitor
            .poll(now + STALE_AFTER + Duration::from_secs(2))
            .is_empty());
    }

    #[test]
    fn another_active_session_wins_over_completion() {
        let fixture = Fixture::new();
        fixture.file("a.jsonl", &(meta("a") + &event("task_started")));
        let b = fixture.file("b.jsonl", &(meta("b") + &event("task_started")));
        let mut monitor = Monitor::new(fixture.0.clone());
        let now = SystemTime::now();
        monitor.poll(now);
        append(&b, &event("task_complete"));
        let events = monitor.poll(now);
        assert!(!names(&events).contains(&"Stop"));
        assert!(
            monitor
                .tails
                .get(monitor.foreground.as_ref().unwrap())
                .unwrap()
                .session
                .active
        );
    }

    #[test]
    fn discovers_new_sessions_and_bounds_file_count() {
        let fixture = Fixture::new();
        let now = SystemTime::now();
        let mut monitor = Monitor::new(fixture.0.clone());
        assert!(monitor.poll(now).is_empty());
        fixture.file("new.jsonl", &(meta("new") + &event("task_started")));
        assert_eq!(
            names(&monitor.poll(now + DISCOVERY_INTERVAL)),
            ["SessionStart", "UserPromptSubmit"]
        );
        for n in 0..MAX_CANDIDATES + 4 {
            fixture.file(&format!("{n}.jsonl"), &meta(&n.to_string()));
        }
        assert_eq!(recent_files(&fixture.0, now).len(), MAX_CANDIDATES);
        monitor.poll(now + DISCOVERY_INTERVAL * 2);
        assert_eq!(monitor.tails.len(), MAX_SESSIONS);
    }

    #[test]
    fn stale_state_at_startup_is_not_announced() {
        let fixture = Fixture::new();
        fixture.file("old.jsonl", &(meta("old") + &event("task_started")));
        let mut monitor = Monitor::new(fixture.0.clone());
        assert!(monitor
            .poll(SystemTime::now() + STALE_AFTER + Duration::from_secs(1))
            .is_empty());
    }

    #[test]
    fn immediate_follow_up_turn_does_not_emit_previous_completion() {
        let fixture = Fixture::new();
        let path = fixture.file("session.jsonl", &(meta("a") + &event("task_started")));
        let mut monitor = Monitor::new(fixture.0.clone());
        let now = SystemTime::now();
        monitor.poll(now);
        append(&path, &(event("task_complete") + &event("task_started")));
        assert_eq!(names(&monitor.poll(now)), ["UserPromptSubmit"]);
    }

    #[test]
    fn bounded_bootstrap_preserves_metadata_and_recovers_after_large_record() {
        let fixture = Fixture::new();
        let content = meta("large")
            + &" ".repeat(BOOTSTRAP_BYTES as usize * 2)
            + "\n"
            + &event("task_started");
        let path = fixture.file("large.jsonl", &content);
        let tail = Tail::open(&path, SystemTime::now()).unwrap();
        assert_eq!(tail.session.id, "large");
        assert!(tail.session.active);
        assert_eq!(tail.offset, content.len() as u64);
        assert!(tail.partial.is_empty());
    }

    fn record(timestamp: u64, kind: &str, payload: Value) -> String {
        json!({"timestamp":timestamp,"type":kind,"payload":payload}).to_string() + "\n"
    }

    fn message(timestamp: u64, role: &str, phase: &str, text: &str) -> String {
        record(
            timestamp,
            "response_item",
            json!({"type":"message","role":role,"phase":phase,
            "content":[{"type":if role == "user" {"input_text"} else {"output_text"},"text":text}]}),
        )
    }

    #[test]
    fn public_progress_and_final_are_retained_without_reasoning_or_outputs() {
        let mut session = Session::default();
        session.parse(meta("main").as_bytes());
        session.parse(message(1000, "user", "", "# Files mentioned by the user:\n- screenshot.png\n\n## My request:\nShow useful activity").as_bytes());
        assert_eq!(session.title, "Show useful activity");
        session
            .parse(message(1001, "user", "", "# AGENTS.md instructions\nPrivate setup").as_bytes());
        assert_eq!(session.title, "Show useful activity");
        session.parse(event("task_started").as_bytes());
        session.parse(
            message(
                2000,
                "assistant",
                "commentary",
                "I am checking the activity parser.",
            )
            .as_bytes(),
        );
        let revision = session.revision;
        for payload in [
            json!({"type":"reasoning","summary":"PRIVATE_REASONING"}),
            json!({"type":"agent_reasoning","text":"PRIVATE_REASONING"}),
            json!({"type":"agent_message","text":"PRIVATE_INTER_AGENT"}),
        ] {
            session.parse(record(2100, "response_item", payload).as_bytes());
        }
        session.parse(message(2200, "assistant", "analysis", "PRIVATE_ANALYSIS").as_bytes());
        session.parse(message(2200, "developer", "", "PRIVATE_INSTRUCTIONS").as_bytes());
        assert_eq!(session.revision, revision);
        session.parse(
            record(
                2500,
                "response_item",
                json!({"type":"function_call_output","output":"PRIVATE_OUTPUT"}),
            )
            .as_bytes(),
        );
        let final_text = format!("Done. {}", "Useful public explanation. ".repeat(16));
        assert_eq!(
            session
                .parse(message(3000, "assistant", "final_answer", &final_text).as_bytes())
                .unwrap()["hook_event_name"],
            "Stop"
        );
        assert!(session
            .parse(
                record(
                    3001,
                    "event_msg",
                    json!({"type":"task_complete","last_agent_message":final_text})
                )
                .as_bytes()
            )
            .is_none());
        assert_eq!(session.state, ActivityState::Finished);
        let messages: Vec<_> = session
            .entries
            .iter()
            .filter(|entry| entry.kind == "message")
            .collect();
        assert_eq!(messages.len(), 2);
        assert!(messages[1].detail.as_ref().unwrap().len() > MAX_TEXT);
        let serialized = serde_json::to_string(&session.activity()).unwrap();
        assert!(!serialized.contains("PRIVATE_"));
        assert!(serialized.contains("\"updatedAt\":3001"));
    }

    #[test]
    fn tool_summaries_cover_commands_paths_and_custom_wrappers_without_raw_bodies() {
        let (text, detail) = tool_summary(
            &json!({"name":"exec_command","arguments":json!({"cmd":"cargo test --workspace --offline", "workdir":"/work/project"}).to_string()}),
        );
        assert_eq!(text, "Run command");
        assert_eq!(
            detail.unwrap(),
            "cargo test --workspace --offline\n/work/project"
        );
        let (_, detail) = tool_summary(
            &json!({"name":"shell","arguments":{"command":["bash","-lc","git diff --stat"]}}),
        );
        assert_eq!(detail.unwrap(), "git diff --stat");
        let patch = "*** Begin Patch\n*** Update File: src/activity.ts\n@@\n+PRIVATE_PATCH_BODY\n*** End Patch";
        for payload in [
            json!({"name":"apply_patch","input":patch}),
            json!({"name":"exec","input":format!("await tools.apply_patch({});",serde_json::to_string(patch).unwrap())}),
        ] {
            let (text, detail) = tool_summary(&payload);
            assert_eq!(text, "Edit files");
            assert_eq!(detail.unwrap(), "src/activity.ts");
        }
        let (_, detail) = tool_summary(
            &json!({"name":"exec","input":"const result = await tools.exec_command({cmd: 'npm run build', workdir: '/work'}); text(result);"}),
        );
        assert_eq!(detail.unwrap(), "npm run build");
        let (_, detail) = tool_summary(
            &json!({"name":"exec","input":"await tools.exec_command({\"cmd\": \"curl -H 'Authorization: Bearer PRIVATE_TOKEN' https://example.com\"});"}),
        );
        assert_eq!(detail.unwrap(), "curl …");
        let (_, detail) = tool_summary(
            &json!({"name":"send_message","arguments":{"message":"PRIVATE_AGENT_TASK"}}),
        );
        assert!(detail.is_none());
        let cleaned = public_text("Progress done\napi_key=PRIVATE_KEY\nToken sk-example-secret\nhttps://example.com/?token=PRIVATE_URL", MAX_DETAIL);
        assert!(!cleaned.contains("PRIVATE_") && !cleaned.contains("sk-example"));
    }

    #[test]
    fn activity_restores_completed_and_interrupted_sessions_and_emits_only_changes() {
        let fixture = Fixture::new();
        let metadata = |id: &str| {
            record(
                1,
                "session_meta",
                json!({"id":id,"cwd":"/work/project","source":"cli"}),
            )
        };
        let a = fixture.file(
            "a.jsonl",
            &(metadata("a")
                + &message(1000, "assistant", "commentary", "Checking files")
                + &message(2000, "assistant", "final_answer", "Complete")),
        );
        fixture.file(
            "b.jsonl",
            &(metadata("b")
                + &record(3000, "event_msg", json!({"type":"task_started"}))
                + &record(4000, "event_msg", json!({"type":"turn_aborted"}))),
        );
        let mut monitor = Monitor::new(fixture.0.clone());
        let now = SystemTime::now();
        assert!(monitor.poll(now).is_empty());
        let restored = monitor.activity_update().unwrap();
        assert_eq!(restored.sessions.len(), 2);
        assert_eq!(
            restored
                .sessions
                .iter()
                .find(|s| s.id == "a")
                .unwrap()
                .state,
            ActivityState::Finished
        );
        assert_eq!(
            restored
                .sessions
                .iter()
                .find(|s| s.id == "b")
                .unwrap()
                .state,
            ActivityState::Interrupted
        );
        assert!(monitor.activity_update().is_none());
        append(
            &a,
            &record(
                5000,
                "event_msg",
                json!({"type":"token_count","info":"PRIVATE"}),
            ),
        );
        append(
            &a,
            &record(
                5100,
                "response_item",
                json!({"type":"reasoning","summary":"PRIVATE"}),
            ),
        );
        monitor.poll(now);
        assert!(monitor.activity_update().is_none());
        append(
            &a,
            &record(6000, "event_msg", json!({"type":"task_started"})),
        );
        monitor.poll(now);
        let active = monitor.activity_update().unwrap();
        assert_eq!(active.sessions[0].id, "a");
        assert_eq!(active.sessions[0].state, ActivityState::Thinking);
        append(
            &a,
            &record(7000, "event_msg", json!({"type":"session_end"})),
        );
        monitor.poll(now);
        let ended = monitor.activity_update().unwrap();
        assert_eq!(ended.sessions.len(), 2);
        assert_eq!(
            ended.sessions.iter().find(|s| s.id == "a").unwrap().state,
            ActivityState::Idle
        );
        let mut fresh = Monitor::new(fixture.0.clone());
        fresh.poll(now);
        assert_eq!(fresh.activity_update().unwrap(), ended);
    }

    #[test]
    fn main_sessions_have_bounded_history_and_are_not_crowded_out_by_subagents() {
        let fixture = Fixture::new();
        fixture.file("main.jsonl", &(meta("main") + &event("task_started")));
        for n in 0..MAX_SESSIONS + 4 {
            fixture.file(&format!("hidden-{n}.jsonl"), &(json!({"type":"session_meta","payload":{"id":format!("hidden-{n}"),"source":{"subagent":{"thread_spawn":{}}}}}).to_string() + "\n" + &event("task_started")));
        }
        let mut monitor = Monitor::new(fixture.0.clone());
        monitor.poll(SystemTime::now());
        let snapshot = monitor.activity_update().unwrap();
        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.sessions[0].id, "main");
        assert_eq!(monitor.excluded.len(), MAX_SESSIONS + 4);
        let mut session = Session::default();
        session.parse(meta("bounded").as_bytes());
        for n in 0..MAX_ENTRIES + 10 {
            session.parse(
                message(
                    n as u64 * 10000,
                    "assistant",
                    "commentary",
                    &format!("Public action {n}"),
                )
                .as_bytes(),
            );
        }
        assert_eq!(session.entries.len(), MAX_ENTRIES);
        assert_eq!(session.entries.front().unwrap().text, "Public action 10");
    }

    #[test]
    fn timestamps_parse_wire_format_and_reject_invalid_dates() {
        assert_eq!(timestamp_ms(&json!("1970-01-01T00:00:00Z")), Some(0));
        assert_eq!(
            timestamp_ms(&json!("2026-10-01T12:34:56.123456Z")),
            Some(1790858096123)
        );
        assert_eq!(
            timestamp_ms(&json!("2024-02-29T00:00:00.1Z")),
            Some(1709164800100)
        );
        assert!(timestamp_ms(&json!("2026-02-29T00:00:00Z")).is_none());
        assert!(timestamp_ms(&json!("2026-10-01T25:00:00Z")).is_none());
    }

    #[test]
    #[ignore = "Opt-in read-only local rollout smoke test; emits counts only"]
    fn local_activity_smoke() {
        if std::env::var("COUCOU_CODEX_ACTIVITY_SMOKE").as_deref() != Ok("1") {
            return;
        }
        let mut monitor = Monitor::new(sessions_path());
        monitor.poll(SystemTime::now());
        let snapshot = monitor.activity_update().unwrap_or_default();
        let sessions = snapshot.sessions.len();
        let entries: usize = snapshot
            .sessions
            .iter()
            .map(|session| session.entries.len())
            .sum();
        let messages = snapshot
            .sessions
            .iter()
            .flat_map(|session| &session.entries)
            .filter(|entry| entry.kind == "message")
            .count();
        let tools = snapshot
            .sessions
            .iter()
            .flat_map(|session| &session.entries)
            .filter(|entry| entry.kind == "tool")
            .count();
        let active = snapshot
            .sessions
            .iter()
            .filter(|session| {
                matches!(
                    session.state,
                    ActivityState::Thinking | ActivityState::Working
                )
            })
            .count();
        println!("local activity: sessions={sessions}, active={active}, entries={entries}, messages={messages}, tools={tools}");
        assert!(sessions <= MAX_SESSIONS);
        assert!(snapshot
            .sessions
            .iter()
            .all(|session| session.entries.len() <= MAX_ENTRIES));
        monitor.poll(SystemTime::now());
    }
}
