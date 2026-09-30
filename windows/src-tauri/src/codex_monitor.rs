// Read-only adapter for local Codex Desktop/CLI rollout logs. These logs are an
// implementation detail, not an approval API: never modify them, copy prompt or
// tool arguments into the UI, or synthesize a PermissionRequest from a log.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

const MAX_SESSIONS: usize = 32;
const READ_BUDGET: u64 = 256 * 1024;
const BOOTSTRAP_BYTES: u64 = 512 * 1024;
const MAX_LINE: usize = 1024 * 1024;
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);
const STALE_AFTER: Duration = Duration::from_secs(15 * 60);
const FILE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
static RUNNING: AtomicBool = AtomicBool::new(false);
static SNAPSHOT_REQUESTED: AtomicBool = AtomicBool::new(false);

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
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

#[derive(Default)]
struct Session {
    id: String,
    cwd: String,
    hidden: bool,
    active: bool,
    tool: Option<String>,
    terminal: Option<&'static str>,
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

    fn parse(&mut self, line: &[u8]) -> Option<Value> {
        let record: Value = serde_json::from_slice(line).ok()?;
        let payload = record.get("payload")?;
        let kind = record.get("type")?.as_str()?;
        if kind == "session_meta" {
            if let Some(id) = payload
                .get("id")
                .or_else(|| payload.get("session_id"))
                .and_then(Value::as_str)
            {
                self.id = id.to_owned();
            }
            if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
                self.cwd = cwd.to_owned();
            }
            self.hidden = payload.get("thread_source").and_then(Value::as_str) == Some("subagent")
                || payload.get("source").and_then(|v| v.get("subagent")).is_some()
                // `exec` is also used by Coucou's own chat requests.
                || payload.get("source").and_then(Value::as_str) == Some("exec");
            return None;
        }
        if kind == "turn_context" {
            if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
                self.cwd = cwd.to_owned();
            }
            return None;
        }
        if self.hidden || self.id.is_empty() {
            return None;
        }
        let event = match (kind, payload.get("type")?.as_str()?) {
            ("event_msg", "task_started") | ("event_msg", "user_message") => {
                self.active = true;
                self.tool = None;
                self.terminal = None;
                "UserPromptSubmit"
            }
            ("response_item", "function_call" | "custom_tool_call") => {
                self.active = true;
                self.terminal = None;
                // Tool names only. Arguments, commands and output may contain secrets.
                self.tool = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| name.chars().filter(|c| !c.is_control()).take(80).collect());
                "PreToolUse"
            }
            ("response_item", "function_call_output" | "custom_tool_call_output")
                if self.active =>
            {
                "PostToolUse"
            }
            ("event_msg", "task_complete") => {
                self.active = false;
                self.terminal = Some("Stop");
                "Stop"
            }
            ("event_msg", "turn_aborted") => {
                self.active = false;
                self.terminal = Some("SessionEnd");
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
        file.by_ref().take(MAX_LINE as u64).read_to_end(&mut head)?;
        if let Some(end) = head.iter().position(|b| *b == b'\n') {
            tail.session.parse(&head[..end]);
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
            self.session = Session::default();
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
            self.session.terminal = Some("SessionEnd");
            return Some(self.session.event("SessionEnd"));
        }
        None
    }
}

struct Monitor {
    root: PathBuf,
    tails: HashMap<PathBuf, Tail>,
    foreground: Option<PathBuf>,
    discovered_at: Option<SystemTime>,
}

impl Monitor {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            tails: HashMap::new(),
            foreground: None,
            discovered_at: None,
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
            self.tails
                .retain(|path, _| recent.iter().any(|(_, p)| p == path));
            for (modified, path) in recent {
                if self.tails.contains_key(&path) {
                    continue;
                }
                if let Ok(mut tail) = Tail::open(&path, modified) {
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
                    self.tails.insert(path, tail);
                }
            }
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
                    if files.len() > MAX_SESSIONS * 2 {
                        files.sort_unstable_by(|a, b| b.0.cmp(&a.0));
                        files.truncate(MAX_SESSIONS);
                    }
                }
            }
        }
    }
    let mut files = Vec::new();
    visit(root, 0, now, &mut files);
    files.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    files.truncate(MAX_SESSIONS);
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
        for n in 0..MAX_SESSIONS + 4 {
            fixture.file(&format!("{n}.jsonl"), &meta(&n.to_string()));
        }
        assert_eq!(recent_files(&fixture.0, now).len(), MAX_SESSIONS);
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
}
