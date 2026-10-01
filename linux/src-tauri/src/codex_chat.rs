// Codex app-server JSON-RPC transport. The CLI owns auth, tools and sandboxing.
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{MAX_HISTORY, MAX_OUTPUT, MAX_QUERY};
use crate::codex_models::Selection;

const START_TIMEOUT: Duration = Duration::from_secs(25);
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(600);
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(test))]
const CANCEL_START_TIMEOUT: Duration = INTERRUPT_TIMEOUT;
#[cfg(test)]
const CANCEL_START_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEvent {
    pub request_id: String,
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

impl ChatEvent {
    pub(super) fn new(request_id: &str, kind: &str, text: &str) -> Self {
        Self {
            request_id: request_id.into(),
            kind: kind.into(),
            text: text.into(),
            item_id: None,
            approval_id: None,
            command: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
        }
    }
}

pub(super) enum Control {
    Cancel,
    Approval { approval_id: String, allow: bool },
}

type ApprovalSlot = Arc<Mutex<Option<(String, String)>>>;

pub(super) struct Session {
    child: Child,
    connection: Connection<BufReader<ChildStdout>, ChildStdin>,
    pub generation: u64,
    pub has_turns: bool,
}

fn command(working_dir: &Path) -> Command {
    let mut command = super::cli();
    command
        .current_dir(working_dir)
        .args([
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            "model_provider=\"openai\"",
            "-c",
            "features.shell_tool=true",
            "-c",
            "features.apps=false",
            "-c",
            "features.plugins=false",
            "-c",
            "features.hooks=false",
            "-c",
            "features.browser_use=false",
            "-c",
            "features.computer_use=false",
            "-c",
            "features.multi_agent=false",
            "-c",
            "web_search=\"disabled\"",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

impl Session {
    #[cfg(test)]
    pub fn process_id(&self) -> Option<u32> {
        self.child.id()
    }

    pub async fn start(
        working_dir: &Path,
        generation: u64,
        selection: &Selection,
    ) -> Result<Self, String> {
        let mut child = command(working_dir)
            .spawn()
            .map_err(|e| format!("Cannot start Codex. Check that Codex CLI is installed: {e}"))?;
        let mut connection = Connection {
            reader: BufReader::new(child.stdout.take().ok_or("Cannot open Codex output.")?),
            writer: child.stdin.take().ok_or("Cannot open Codex input.")?,
            next_id: 1,
            thread_id: String::new(),
            pending_line: Vec::new(),
        };
        tokio::time::timeout(START_TIMEOUT, connection.initialize(working_dir, selection))
            .await
            .map_err(|_| "Connecting to Codex timed out. Please retry.".to_string())??;
        Ok(Self {
            child,
            connection,
            generation,
            has_turns: false,
        })
    }

    pub async fn turn(
        &mut self,
        selection: &Selection,
        prompt: String,
        image: Option<&Path>,
        request_id: &str,
        controls: &mut UnboundedReceiver<Control>,
        approval: &ApprovalSlot,
        on_event: &(impl Fn(ChatEvent) + Send + Sync),
    ) -> Result<String, String> {
        let result = self
            .connection
            .turn(
                selection, prompt, image, request_id, controls, approval, on_event,
            )
            .await;
        self.has_turns = result.is_ok();
        if result.is_err() {
            // A protocol error or cancellation must never reuse a possibly active turn.
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
        result
    }
}

struct Connection<R, W> {
    reader: R,
    writer: W,
    next_id: u64,
    thread_id: String,
    pending_line: Vec<u8>,
}

impl<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin> Connection<R, W> {
    async fn write(&mut self, message: Value) -> Result<(), String> {
        let mut data = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
        data.push(b'\n');
        self.writer
            .write_all(&data)
            .await
            .map_err(|e| format!("Cannot write to Codex: {e}"))?;
        self.writer
            .flush()
            .await
            .map_err(|e| format!("Cannot flush Codex input: {e}"))
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(json!({"id":id,"method":method,"params":params}))
            .await?;
        Ok(id)
    }

    async fn response(&mut self, id: u64, budget: &mut usize) -> Result<Value, String> {
        loop {
            let message = read(&mut self.reader, budget, &mut self.pending_line).await?;
            if message.get("method").is_some() && message.get("id").is_some() {
                self.reject_unknown(&message).await?;
                continue;
            }
            if message["id"].as_u64() != Some(id) {
                continue;
            }
            return response_result(&message);
        }
    }

    async fn initialize(
        &mut self,
        working_dir: &Path,
        selection: &Selection,
    ) -> Result<(), String> {
        let mut budget = MAX_OUTPUT as usize;
        let id = self.request("initialize", json!({
            "clientInfo":{"name":"coucou","title":"Coucou","version":env!("CARGO_PKG_VERSION")}
        })).await?;
        self.response(id, &mut budget).await?;
        self.write(json!({"method":"initialized","params":{}}))
            .await?;
        let id = self
            .request(
                "thread/start",
                json!({
                    "model":selection.model,"modelProvider":"openai","cwd":working_dir,
                    "ephemeral":true,"sandbox":"workspace-write","approvalPolicy":"on-request",
                    "approvalsReviewer":"user","developerInstructions":super::INSTRUCTIONS,
                    "config":{"sandbox_workspace_write":{"writable_roots":[],"network_access":false,
                        "exclude_tmpdir_env_var":true,"exclude_slash_tmp":true}}
                }),
            )
            .await?;
        self.thread_id = self.response(id, &mut budget).await?["thread"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("Codex returned no chat identifier.")?
            .into();
        Ok(())
    }

    async fn reject_unknown(&mut self, message: &Value) -> Result<(), String> {
        self.write(json!({"id":message["id"],"error":{"code":-32601,
            "message":"This Coucou client does not support this request."}}))
            .await
    }

    async fn interrupt(&mut self, turn_id: &str, budget: &mut usize) {
        if turn_id.is_empty() {
            return;
        }
        let result = tokio::time::timeout(INTERRUPT_TIMEOUT, async {
            self.request(
                "turn/interrupt",
                json!({"threadId":self.thread_id,"turnId":turn_id}),
            )
            .await?;
            loop {
                let message = read(&mut self.reader, budget, &mut self.pending_line).await?;
                if message["method"] == "turn/completed"
                    && message["params"]["threadId"] == self.thread_id
                    && message["params"]["turn"]["id"] == turn_id
                {
                    return Ok::<_, String>(());
                }
                if message.get("id").is_some() && message.get("method").is_some() {
                    self.reject_unknown(&message).await?;
                }
            }
        })
        .await;
        let _ = result;
    }

    async fn turn(
        &mut self,
        selection: &Selection,
        prompt: String,
        image: Option<&Path>,
        request_id: &str,
        controls: &mut UnboundedReceiver<Control>,
        approval: &ApprovalSlot,
        on_event: &(impl Fn(ChatEvent) + Send + Sync),
    ) -> Result<String, String> {
        let mut input = vec![json!({"type":"text","text":prompt})];
        if let Some(path) = image {
            input.push(json!({"type":"localImage","path":path}));
        }
        let mut params = json!({"threadId":self.thread_id,"input":input,
            "model":selection.model,"approvalPolicy":"on-request","approvalsReviewer":"user",
            "sandboxPolicy":{"type":"workspaceWrite","writableRoots":[],"networkAccess":false,
                "excludeTmpdirEnvVar":true,"excludeSlashTmp":true}});
        if !selection.reasoning_effort.is_empty() {
            params["effort"] = json!(selection.reasoning_effort);
        }
        let start_id = self.request("turn/start", params).await?;
        let mut budget = MAX_OUTPUT as usize;
        let mut turn_id = String::new();
        let mut last_reply = String::new();
        let mut items: HashMap<String, Value> = HashMap::new();
        let mut active_approval: Option<(String, Value)> = None;
        let mut canceled = false;
        let mut cancel_deadline = None;
        loop {
            let timeout = if let Some(deadline) = cancel_deadline {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err("The Codex reply was stopped.".into());
                }
                deadline - now
            } else if active_approval.is_some() {
                APPROVAL_TIMEOUT
            } else {
                IDLE_TIMEOUT
            };
            let incoming = tokio::select! {
                biased;
                control = controls.recv(), if !canceled => {
                    match control {
                        Some(Control::Cancel) | None => {
                            if turn_id.is_empty() {
                                canceled = true;
                                cancel_deadline = Some(tokio::time::Instant::now() + CANCEL_START_TIMEOUT);
                            } else {
                                self.interrupt(&turn_id, &mut budget).await;
                                return Err("The Codex reply was stopped.".into());
                            }
                        }
                        Some(Control::Approval { approval_id, allow }) => {
                            if active_approval.as_ref().is_some_and(|(id,_)| id == &approval_id) {
                                let (_, message) = active_approval.take().unwrap();
                                self.write(approval_response(&message, allow)).await?;
                                *approval.lock().unwrap() = None;
                                let mut event = ChatEvent::new(request_id, "approvalResolved", "");
                                event.approval_id = Some(approval_id);
                                on_event(event);
                            }
                        }
                    }
                    None
                }
                message = tokio::time::timeout(timeout, read(&mut self.reader, &mut budget, &mut self.pending_line)) => {
                    match message {
                        Ok(Ok(message)) => Some(message),
                        error => {
                            self.interrupt(&turn_id, &mut budget).await;
                            if canceled { return Err("The Codex reply was stopped.".into()); }
                            return Err(match error { Ok(Err(e)) => e,
                                _ => "Codex did not respond within the time limit. Please retry.".into() });
                        }
                    }
                }
            };
            let Some(message) = incoming else {
                continue;
            };
            if message.get("method").is_none() {
                if message["id"].as_u64() == Some(start_id) {
                    let result = response_result(&message)?;
                    turn_id = result["turn"]["id"].as_str().unwrap_or_default().into();
                    if turn_id.is_empty() {
                        return Err("Codex returned no turn identifier.".into());
                    }
                    if canceled {
                        self.interrupt(&turn_id, &mut budget).await;
                        return Err("The Codex reply was stopped.".into());
                    }
                }
                continue;
            }
            let method = message["method"].as_str().unwrap_or_default();
            let params = &message["params"];
            if params["threadId"]
                .as_str()
                .is_some_and(|id| id != self.thread_id)
            {
                continue;
            }
            if params["turnId"]
                .as_str()
                .is_some_and(|id| !turn_id.is_empty() && id != turn_id)
            {
                continue;
            }
            if message.get("id").is_some() {
                if matches!(
                    method,
                    "item/commandExecution/requestApproval"
                        | "item/fileChange/requestApproval"
                        | "item/permissions/requestApproval"
                ) {
                    if active_approval.is_some() {
                        // Never replace a prompt the user is currently reviewing.
                        self.write(approval_response(&message, false)).await?;
                        continue;
                    }
                    let id = format!("{}:{}", request_id, message["id"]);
                    let event = approval_event(request_id, &id, &message, &items);
                    if !reviewable_approval(&message, &event, &items) {
                        self.write(approval_response(&message, false)).await?;
                        on_event(ChatEvent::new(
                            request_id,
                            "status",
                            "The action was declined because Codex supplied no reviewable preview.",
                        ));
                        continue;
                    }
                    *approval.lock().unwrap() = Some((request_id.into(), id.clone()));
                    active_approval = Some((id, message.clone()));
                    on_event(event);
                } else {
                    self.reject_unknown(&message).await?;
                }
                continue;
            }
            match method {
                "serverRequest/resolved" => {
                    if active_approval
                        .as_ref()
                        .is_some_and(|(_, request)| request["id"] == params["requestId"])
                    {
                        let (id, _) = active_approval.take().unwrap();
                        *approval.lock().unwrap() = None;
                        let mut event = ChatEvent::new(request_id, "approvalResolved", "");
                        event.approval_id = Some(id);
                        on_event(event);
                    }
                }
                "turn/started" => {
                    turn_id = params["turn"]["id"].as_str().unwrap_or_default().into();
                    on_event(ChatEvent::new(request_id, "status", "Codex is replying…"));
                    if canceled {
                        self.interrupt(&turn_id, &mut budget).await;
                        return Err("The Codex reply was stopped.".into());
                    }
                }
                "item/agentMessage/delta" => {
                    let text = params["delta"].as_str().unwrap_or_default();
                    let mut event = ChatEvent::new(request_id, "delta", text);
                    event.item_id = params["itemId"].as_str().map(str::to_owned);
                    on_event(event);
                }
                "item/started" | "item/completed" => {
                    let item = &params["item"];
                    let id = item["id"].as_str().unwrap_or_default();
                    items.insert(id.into(), item.clone());
                    if item["type"] == "agentMessage" && method == "item/completed" {
                        last_reply = item["text"].as_str().unwrap_or_default().into();
                        let mut event = ChatEvent::new(request_id, "message", &last_reply);
                        event.item_id = Some(id.into());
                        on_event(event);
                    } else if method == "item/started" {
                        let status = match item["type"].as_str() {
                            Some("commandExecution") => Some(
                                item["command"]
                                    .as_str()
                                    .unwrap_or("Running a local command…"),
                            ),
                            Some("fileChange") => Some("Editing files…"),
                            Some("webSearch") => Some("Searching the web…"),
                            _ => None,
                        };
                        if let Some(status) = status {
                            on_event(ChatEvent::new(request_id, "status", status));
                        }
                    }
                }
                "turn/completed" => {
                    if !turn_id.is_empty() && params["turn"]["id"] != turn_id {
                        continue;
                    }
                    let turn = &params["turn"];
                    if turn["status"] != "completed" {
                        return Err(turn["error"]["message"]
                            .as_str()
                            .unwrap_or("The Codex reply was interrupted.")
                            .into());
                    }
                    // Some versions include the definitive items only on completion.
                    if let Some(turn_items) = turn["items"].as_array() {
                        if let Some(item) = turn_items
                            .iter()
                            .rev()
                            .find(|item| item["type"] == "agentMessage")
                        {
                            last_reply = item["text"].as_str().unwrap_or_default().into();
                        }
                    }
                    let text = last_reply.trim();
                    if text.is_empty() {
                        return Err("Codex returned no response text.".into());
                    }
                    if text.len() > MAX_HISTORY - MAX_QUERY {
                        return Err("Codex's answer was too long. Ask for a shorter answer.".into());
                    }
                    return Ok(text.into());
                }
                "error" => {
                    if params["willRetry"] != true {
                        return Err(params["error"]["message"]
                            .as_str()
                            .unwrap_or("Codex could not complete this request.")
                            .into());
                    }
                    on_event(ChatEvent::new(
                        request_id,
                        "status",
                        "Reconnecting to Codex…",
                    ));
                }
                _ => {}
            }
        }
    }
}

async fn read(
    reader: &mut (impl AsyncBufRead + Unpin),
    budget: &mut usize,
    line: &mut Vec<u8>,
) -> Result<Value, String> {
    // Keep the buffer across select! cancellation: a control can arrive mid-line.
    let remaining = (*budget + 1).saturating_sub(line.len());
    let count = (&mut *reader)
        .take(remaining as u64)
        .read_until(b'\n', line)
        .await
        .map_err(|e| format!("Cannot read Codex output: {e}"))?;
    if count == 0 && line.is_empty() {
        return Err("The Codex connection closed. Please retry.".into());
    }
    if line.len() > *budget {
        return Err("Codex produced too much output. Start a new chat.".into());
    }
    *budget -= line.len();
    let result = serde_json::from_slice(line)
        .map_err(|_| "Codex returned an invalid event. Update Codex CLI and retry.".into());
    line.clear();
    result
}

fn response_result(message: &Value) -> Result<Value, String> {
    if let Some(error) = message.get("error") {
        return Err(error["message"]
            .as_str()
            .unwrap_or("Codex rejected the request.")
            .into());
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| "Codex returned no result.".into())
}

fn approval_event(
    request_id: &str,
    approval_id: &str,
    message: &Value,
    items: &HashMap<String, Value>,
) -> ChatEvent {
    let params = &message["params"];
    let mut text = params["reason"]
        .as_str()
        .unwrap_or("Codex needs permission for this action.")
        .to_string();
    match message["method"].as_str() {
        Some("item/fileChange/requestApproval") => {
            if let Some(item) = params["itemId"].as_str().and_then(|id| items.get(id)) {
                if let Some(changes) = item["changes"].as_array() {
                    for change in changes {
                        text.push_str(&format!(
                            "\n\n{}: {}\n{}",
                            change["kind"]["type"].as_str().unwrap_or("change"),
                            change["path"].as_str().unwrap_or("File"),
                            change["diff"]
                                .as_str()
                                .unwrap_or("Change details unavailable.")
                        ));
                    }
                }
            }
            if let Some(root) = params["grantRoot"].as_str() {
                text.push_str(&format!("\nRequested writable folder: {root}"));
            }
        }
        Some("item/permissions/requestApproval") => {
            text.push_str("\n\n");
            text.push_str(
                &serde_json::to_string_pretty(&params["permissions"]).unwrap_or_default(),
            );
        }
        _ => {}
    }
    for (field, label) in [
        ("networkApprovalContext", "Requested network access"),
        ("additionalPermissions", "Requested permissions"),
    ] {
        if let Some(value) = params.get(field).filter(|value| !value.is_null()) {
            text.push_str(&format!(
                "\n\n{label}:\n{}",
                serde_json::to_string_pretty(value).unwrap_or_default()
            ));
        }
    }
    let mut event = ChatEvent::new(request_id, "approval", &text);
    event.approval_id = Some(approval_id.into());
    event.item_id = params["itemId"].as_str().map(str::to_owned);
    event.command = params["command"]
        .as_str()
        .filter(|command| !command.trim().is_empty())
        .or_else(|| {
            params["itemId"]
                .as_str()
                .and_then(|id| items.get(id))
                .and_then(|item| item["command"].as_str())
        })
        .map(str::to_owned);
    event.cwd = params["cwd"]
        .as_str()
        .or_else(|| {
            params["itemId"]
                .as_str()
                .and_then(|id| items.get(id))
                .and_then(|item| item["cwd"].as_str())
        })
        .map(str::to_owned);
    event
}

fn reviewable_approval(message: &Value, event: &ChatEvent, items: &HashMap<String, Value>) -> bool {
    let params = &message["params"];
    let nonempty = |value: &Value| value.as_str().is_some_and(|text| !text.trim().is_empty());
    match message["method"].as_str() {
        Some("item/commandExecution/requestApproval") => {
            event
                .command
                .as_deref()
                .is_some_and(|command| !command.trim().is_empty())
                || (nonempty(&params["networkApprovalContext"]["host"])
                    && nonempty(&params["networkApprovalContext"]["protocol"]))
        }
        Some("item/fileChange/requestApproval") => params["itemId"]
            .as_str()
            .and_then(|id| items.get(id))
            .and_then(|item| item["changes"].as_array())
            .is_some_and(|changes| {
                !changes.is_empty()
                    && changes.iter().all(|change| {
                        nonempty(&change["path"])
                            && (nonempty(&change["diff"]) || change["kind"]["type"] == "delete")
                    })
            }),
        _ => true,
    }
}

fn approval_response(message: &Value, allow: bool) -> Value {
    let result = if message["method"] == "item/permissions/requestApproval" {
        json!({"permissions":if allow { message["params"]["permissions"].clone() } else { json!({}) },"scope":"turn"})
    } else {
        json!({"decision":if allow { "accept" } else { "decline" }})
    };
    json!({"id":message["id"],"result":result})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn selection() -> Selection {
        Selection {
            model: "test-model".into(),
            reasoning_effort: "low".into(),
        }
    }

    fn lines(messages: Vec<Value>) -> String {
        messages
            .into_iter()
            .map(|value| format!("{value}\n"))
            .collect()
    }

    fn completion(turn: &str, text: &str) -> Value {
        json!({"method":"turn/completed","params":{"threadId":"thread","turn":{
            "id":turn,"status":"completed","items":[{"id":"answer","type":"agentMessage","text":text}]}}})
    }

    #[test]
    fn streaming_delivers_deltas_before_completion_and_reuses_thread() {
        runtime().block_on(async {
            let data = lines(vec![
                json!({"id":1,"result":{"turn":{"id":"turn-1"}}}),
                json!({"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"turn-1","itemId":"commentary","delta":"Working"}}),
                json!({"method":"item/completed","params":{"threadId":"thread","turnId":"turn-1","item":{"id":"commentary","type":"agentMessage","text":"Working"}}}),
                json!({"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"turn-1","itemId":"answer","delta":"Done"}}),
                completion("turn-1", "Done"),
                json!({"id":2,"result":{"turn":{"id":"turn-2"}}}),
                completion("turn-2", "Again"),
            ]);
            let mut connection = Connection { reader: BufReader::new(data.as_bytes()), writer: Vec::new(),
                next_id: 1, thread_id: "thread".into(), pending_line: Vec::new() };
            let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let events = Mutex::new(Vec::new());
            let approval = Arc::new(Mutex::new(None));
            let answer = connection.turn(&selection(), "first".into(), None, "request-1", &mut rx, &approval,
                &|event| events.lock().unwrap().push(event)).await.unwrap();
            assert_eq!(answer, "Done");
            let events = events.into_inner().unwrap();
            assert_eq!(events.iter().map(|event| event.kind.as_str()).collect::<Vec<_>>(), ["delta","message","delta"]);
            assert_eq!(events[0].item_id.as_deref(), Some("commentary"));
            assert_eq!(events[2].item_id.as_deref(), Some("answer"));
            assert_eq!(connection.turn(&selection(), "second".into(), None, "request-2", &mut rx, &approval, &|_| {}).await.unwrap(), "Again");
            let requests: Vec<Value> = String::from_utf8(connection.writer).unwrap().lines()
                .map(|line| serde_json::from_str(line).unwrap()).collect();
            assert_eq!(requests.len(), 2);
            assert!(requests.iter().all(|request| request["method"] == "turn/start" && request["params"]["threadId"] == "thread"));
            assert_eq!(requests[1]["params"]["input"][0]["text"], "second");
            assert_eq!(requests[1]["params"]["model"], "test-model");
            assert_eq!(requests[1]["params"]["effort"], "low");
            assert_eq!(requests[1]["params"]["sandboxPolicy"]["type"], "workspaceWrite");
            assert_eq!(requests[1]["params"]["approvalPolicy"], "on-request");
        });
    }

    #[test]
    fn approval_waits_for_scoped_decision_and_preserves_opaque_rpc_identifier() {
        runtime().block_on(async {
            let (client, server) = tokio::io::duplex(8192);
            let (reader, writer) = tokio::io::split(client);
            let (server_reader, mut server_writer) = tokio::io::split(server);
            let peer = tokio::spawn(async move {
                let mut reader = BufReader::new(server_reader);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let notifications = lines(vec![
                    json!({"id":1,"result":{"turn":{"id":"turn"}}}),
                    json!({"id":"rpc-approval","method":"item/commandExecution/requestApproval","params":{"threadId":"thread","turnId":"turn","itemId":"command","command":"touch /outside/file","cwd":"/work","reason":"Write outside workspace"}}),
                ]);
                server_writer.write_all(notifications.as_bytes()).await.unwrap();
                line.clear(); reader.read_line(&mut line).await.unwrap();
                let response: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(response, json!({"id":"rpc-approval","result":{"decision":"decline"}}));
                server_writer.write_all(format!("{}\n",completion("turn","Denied")).as_bytes()).await.unwrap();
            });
            let mut connection = Connection { reader: BufReader::new(reader), writer,
                next_id: 1, thread_id: "thread".into(), pending_line: Vec::new() };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let approval = Arc::new(Mutex::new(None));
            let events = Mutex::new(Vec::new());
            let reply = connection.turn(&selection(), "query".into(), None, "request", &mut rx, &approval, &|event| {
                if event.kind == "approval" {
                    assert_eq!(event.command.as_deref(), Some("touch /outside/file"));
                    let id = event.approval_id.as_ref().unwrap();
                    assert_eq!(approval.lock().unwrap().as_ref().unwrap(), &("request".into(), id.clone()));
                    tx.send(Control::Approval { approval_id: id.clone(), allow: false }).unwrap();
                }
                events.lock().unwrap().push(event);
            }).await.unwrap();
            peer.await.unwrap();
            assert_eq!(reply, "Denied");
            assert!(approval.lock().unwrap().is_none());
            assert!(events.lock().unwrap().iter().any(|event| event.kind == "approvalResolved"));
        });
    }

    #[test]
    fn server_resolution_removes_an_approval_without_a_late_reply() {
        runtime().block_on(async {
            let data = lines(vec![
                json!({"id":1,"result":{"turn":{"id":"turn"}}}),
                json!({"id":"approval","method":"item/commandExecution/requestApproval","params":{"threadId":"thread","turnId":"turn","itemId":"command","command":"example"}}),
                json!({"method":"serverRequest/resolved","params":{"threadId":"thread","requestId":"approval"}}),
                completion("turn", "Resolved"),
            ]);
            let mut connection = Connection { reader: BufReader::new(data.as_bytes()), writer: Vec::new(),
                next_id: 1, thread_id: "thread".into(), pending_line: Vec::new() };
            let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let approval = Arc::new(Mutex::new(None));
            let events = Mutex::new(Vec::new());
            connection.turn(&selection(), "query".into(), None, "request", &mut rx, &approval,
                &|event| events.lock().unwrap().push(event)).await.unwrap();
            assert!(approval.lock().unwrap().is_none());
            let events = events.into_inner().unwrap();
            assert_eq!(events.len(), 2);
            assert_eq!(events[1].kind, "approvalResolved");
            assert_eq!(events[0].approval_id, events[1].approval_id);
            assert_eq!(String::from_utf8(connection.writer).unwrap().lines().count(), 1);
        });
    }

    #[test]
    fn cancel_interrupts_the_active_turn_before_returning() {
        runtime().block_on(async {
            let data = lines(vec![
                json!({"id":1,"result":{"turn":{"id":"turn"}}}),
                json!({"method":"turn/started","params":{"threadId":"thread","turn":{"id":"turn"}}}),
                json!({"id":2,"result":{}}),
                json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"interrupted","items":[]}}}),
            ]);
            let mut connection = Connection { reader: BufReader::new(data.as_bytes()), writer: Vec::new(),
                next_id: 1, thread_id: "thread".into(), pending_line: Vec::new() };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tx.send(Control::Cancel).unwrap();
            let err = connection.turn(&selection(), "query".into(), None, "request", &mut rx,
                &Arc::new(Mutex::new(None)), &|_| {}).await.unwrap_err();
            assert!(err.contains("stopped"));
            let requests: Vec<Value> = String::from_utf8(connection.writer).unwrap().lines()
                .map(|line| serde_json::from_str(line).unwrap()).collect();
            assert_eq!(requests[1]["method"], "turn/interrupt");
            assert_eq!(requests[1]["params"], json!({"threadId":"thread","turnId":"turn"}));
        });
    }

    #[test]
    fn cancellation_without_a_turn_identifier_has_a_fixed_deadline() {
        runtime().block_on(async {
            let (client, mut server) = tokio::io::duplex(16_384);
            let (reader, writer) = tokio::io::split(client);
            // Keep sending unrelated notifications: they must not extend cancellation.
            let peer = tokio::spawn(async move {
                loop {
                    if server
                        .write_all(b"{\"method\":\"notice\"}\n")
                        .await
                        .is_err()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });
            let mut connection = Connection {
                reader: BufReader::new(reader),
                writer,
                next_id: 1,
                thread_id: "thread".into(),
                pending_line: Vec::new(),
            };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tx.send(Control::Cancel).unwrap();
            let started = std::time::Instant::now();
            let result = tokio::time::timeout(
                Duration::from_secs(1),
                connection.turn(
                    &selection(),
                    "query".into(),
                    None,
                    "request",
                    &mut rx,
                    &Arc::new(Mutex::new(None)),
                    &|_| {},
                ),
            )
            .await
            .unwrap();
            assert!(result.unwrap_err().contains("stopped"));
            assert!(started.elapsed() < Duration::from_secs(1));
            peer.abort();
        });
    }

    #[test]
    fn interrupted_partial_line_is_preserved_and_output_is_bounded() {
        runtime().block_on(async {
            let (reader, mut writer) = tokio::io::duplex(1024);
            let mut reader = BufReader::new(reader);
            let mut budget = 100;
            let mut partial = Vec::new();
            writer.write_all(b"{\"id\":").await.unwrap();
            assert!(tokio::time::timeout(
                Duration::from_millis(5),
                read(&mut reader, &mut budget, &mut partial)
            )
            .await
            .is_err());
            assert_eq!(partial, b"{\"id\":");
            writer.write_all(b"1}\n").await.unwrap();
            assert_eq!(
                read(&mut reader, &mut budget, &mut partial).await.unwrap(),
                json!({"id":1})
            );
            assert_eq!(budget, 91);
            let mut reader = BufReader::new(&b"0123456789\n"[..]);
            assert!(read(&mut reader, &mut 5, &mut Vec::new())
                .await
                .unwrap_err()
                .contains("too much"));
        });
    }

    #[test]
    fn failures_and_malformed_protocol_do_not_return_partial_success() {
        runtime().block_on(async {
            for data in [
                "not-json\n".to_string(),
                lines(vec![json!({"id":1,"error":{"message":"Unknown model"}})]),
                lines(vec![json!({"id":1,"result":{"turn":{"id":"turn"}}}),
                    json!({"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"turn","itemId":"answer","delta":"Partial"}}),
                    json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"failed","error":{"message":"Quota exhausted"},"items":[]}}})]),
            ] {
                let mut connection = Connection { reader: BufReader::new(data.as_bytes()), writer: Vec::new(),
                    next_id: 1, thread_id: "thread".into(), pending_line: Vec::new() };
                let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                assert!(connection.turn(&selection(), "query".into(), None, "request", &mut rx,
                    &Arc::new(Mutex::new(None)), &|_| {}).await.is_err());
            }
        });
    }

    #[test]
    fn file_and_permission_approvals_show_details_and_never_grant_session_access() {
        let mut items = HashMap::new();
        items.insert(
            "patch".into(),
            json!({"changes":[{"path":"/notes.txt","diff":"-old\n+new"}]}),
        );
        let request =
            json!({"id":1,"method":"item/fileChange/requestApproval","params":{"itemId":"patch"}});
        let event = approval_event("request", "approval", &request, &items);
        assert!(event.text.contains("/notes.txt\n-old\n+new"));
        assert_eq!(
            approval_response(&request, true)["result"]["decision"],
            "accept"
        );
        let request = json!({"id":2,"method":"item/permissions/requestApproval","params":{"permissions":{"network":{"enabled":true}}}});
        assert!(approval_event("request", "approval", &request, &items)
            .text
            .contains("network"));
        assert_eq!(
            approval_response(&request, false)["result"],
            json!({"permissions":{},"scope":"turn"})
        );
        assert_eq!(approval_response(&request, true)["result"]["scope"], "turn");
    }

    #[test]
    fn approvals_require_a_command_network_target_or_concrete_file_change() {
        let mut items = HashMap::new();
        let mut request =
            json!({"method":"item/commandExecution/requestApproval","params":{"itemId":"cmd"}});
        assert!(!reviewable_approval(
            &request,
            &approval_event("r", "a", &request, &items),
            &items
        ));
        request["params"]["networkApprovalContext"] =
            json!({"host":"example.com","protocol":"https"});
        assert!(reviewable_approval(
            &request,
            &approval_event("r", "a", &request, &items),
            &items
        ));
        request["params"]["networkApprovalContext"] = json!({"host":""});
        assert!(!reviewable_approval(
            &request,
            &approval_event("r", "a", &request, &items),
            &items
        ));
        items.insert(
            "cmd".into(),
            json!({"command":"printf example", "cwd":"/work"}),
        );
        request["params"]["command"] = json!(" ");
        let fallback = approval_event("r", "a", &request, &items);
        assert_eq!(fallback.command.as_deref(), Some("printf example"));
        assert_eq!(fallback.cwd.as_deref(), Some("/work"));
        assert!(reviewable_approval(
            &request,
            &approval_event("r", "a", &request, &items),
            &items
        ));
        request = json!({"method":"item/fileChange/requestApproval","params":{"itemId":"patch"}});
        for invalid in [
            json!([]),
            json!([{"path":"/note","diff":""}]),
            json!([{"path":"","diff":"+new"}]),
        ] {
            items.insert("patch".into(), json!({"changes":invalid}));
            assert!(!reviewable_approval(
                &request,
                &approval_event("r", "a", &request, &items),
                &items
            ));
        }
        items.insert(
            "patch".into(),
            json!({"changes":[{"path":"/note","kind":{"type":"delete"},"diff":""}]}),
        );
        let event = approval_event("r", "a", &request, &items);
        assert!(reviewable_approval(&request, &event, &items));
        assert!(event.text.contains("delete: /note"));
    }

    #[test]
    fn command_enables_local_tools_without_bypassing_sandbox_or_approvals() {
        let command = command(Path::new("."));
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert!(args.iter().any(|arg| arg == "features.shell_tool=true"));
        assert!(!args
            .iter()
            .any(|arg| arg.contains("dangerously") || arg.contains("danger-full-access")));
        assert!(args.windows(2).any(|pair| pair == ["--listen", "stdio://"]));
        let removed: Vec<_> = command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| value.is_none().then_some(key.to_string_lossy().to_string()))
            .collect();
        assert!(removed.iter().any(|key| key == "OPENAI_API_KEY"));
        assert!(removed.iter().any(|key| key == "CODEX_API_KEY"));
    }
}
