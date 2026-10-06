//! Personal Telegram accounts through TDLib. One worker owns the client and
//! applies responses and updates in receive order; IPC never exposes credentials.

use base64::Engine;
use libloading::Library;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::State;
use tokio::sync::oneshot;

use crate::{integrations, secrets, settings};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const SEND_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_CHATS: usize = 100;
const MAX_CACHED_CHATS: usize = 500;
const MAX_CACHED_USERS: usize = 2_000;
const MAX_NOTIFICATIONS: usize = 8;
const NOTIFICATION_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    state: String,
    configured: bool,
    runtime_available: bool,
    error: Option<String>,
    message: Option<String>,
    account_name: Option<String>,
    paused: bool,
    unread_count: Option<i64>,
    unread_chat_count: Option<i64>,
    notifications: Vec<Notification>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: "disconnected".into(),
            configured: false,
            runtime_available: false,
            error: None,
            message: None,
            account_name: None,
            paused: false,
            unread_count: None,
            unread_chat_count: None,
            notifications: Vec::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    id: String,
    title: String,
    last_message: String,
    unread_count: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    id: String,
    text: String,
    date: i64,
    outgoing: bool,
    sender_name: String,
}

#[derive(Clone, Serialize)]
pub struct Notification {
    id: String,
    chat: Chat,
    message: Message,
    #[serde(skip)]
    notification_id: i64,
    #[serde(skip)]
    received_at: Instant,
}

struct NotificationState {
    generation: u64,
    baseline_received: bool,
    baseline_id: i64,
    pending_unreceived: bool,
    pending_delayed: bool,
    armed_at: Option<i64>,
    seen: HashMap<i64, Instant>,
}

impl Default for NotificationState {
    fn default() -> Self {
        Self {
            generation: 0,
            baseline_received: false,
            baseline_id: 0,
            pending_unreceived: true,
            pending_delayed: true,
            armed_at: None,
            seen: HashMap::new(),
        }
    }
}

enum Action {
    Configure(i32, String),
    Connect,
    SetHomeEnabled(bool),
    Authenticate(String, String),
    Chats,
    History(i64, i64),
    Send(i64, String),
    Disconnect,
    Logout,
}

struct Work {
    action: Action,
    reply: Option<oneshot::Sender<Result<Value, String>>>,
}

pub struct Telegram {
    sender: mpsc::SyncSender<Work>,
    status: Arc<Mutex<Status>>,
    interrupt: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    stopped: Arc<(Mutex<bool>, Condvar)>,
}

impl Default for Telegram {
    fn default() -> Self {
        Self::new(false)
    }
}

impl Telegram {
    pub fn new(home_enabled: bool) -> Self {
        let (sender, receiver) = mpsc::sync_channel(16);
        let status = Arc::new(Mutex::new(Status::default()));
        let interrupt = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_status = status.clone();
        let worker_interrupt = interrupt.clone();
        let worker_shutdown = shutdown.clone();
        let worker_stopped = stopped.clone();
        std::thread::Builder::new()
            .name("coucou-telegram".into())
            .spawn(move || {
                let mut worker = Worker::new(worker_status, worker_interrupt);
                worker.shutdown = worker_shutdown;
                worker.run(receiver, home_enabled);
                *worker_stopped.0.lock().unwrap() = true;
                worker_stopped.1.notify_all();
            })
            .expect("could not start Telegram worker");
        Self {
            sender,
            status,
            interrupt,
            shutdown,
            stopped,
        }
    }
    /// Apply changes to the Home integration without making cached summary reads
    /// reconnect a session the user explicitly disconnected.
    pub fn set_home_enabled(&self, enabled: bool) -> Result<(), String> {
        if !enabled {
            self.interrupt.store(true, Ordering::Relaxed);
        }
        self.sender
            .try_send(Work {
                action: Action::SetHomeEnabled(enabled),
                reply: None,
            })
            .map_err(|_| "Telegram is busy. Try changing its Home setting again.".to_string())
    }

    /// Called by the app's exit handler so TDLib flushes and closes its database.
    pub fn shutdown(&self) -> bool {
        self.shutdown.store(true, Ordering::Relaxed);
        self.interrupt.store(true, Ordering::Relaxed);
        let (done, _) = self
            .stopped
            .1
            .wait_timeout_while(
                self.stopped.0.lock().unwrap(),
                Duration::from_secs(10),
                |done| !*done,
            )
            .unwrap();
        *done
    }

    async fn request(&self, action: Action) -> Result<Value, String> {
        let (reply, receive) = oneshot::channel();
        self.sender
            .try_send(Work {
                action,
                reply: Some(reply),
            })
            .map_err(|_| "Telegram is busy. Wait for the current request to finish.".to_string())?;
        tokio::time::timeout(Duration::from_secs(90), receive)
            .await
            .map_err(|_| {
                "Telegram did not finish the request. Refresh before retrying a message."
                    .to_string()
            })?
            .map_err(|_| "Telegram worker is unavailable. Restart Coucou.".to_string())?
    }
}

#[tauri::command]
pub fn telegram_status(telegram: State<'_, Telegram>) -> Status {
    cached_status(&telegram)
}

#[tauri::command]
pub fn telegram_summary(telegram: State<'_, Telegram>) -> Status {
    cached_status(&telegram)
}

fn cached_status(telegram: &Telegram) -> Status {
    let mut status = telegram.status.lock().unwrap().clone();
    status.paused = integrations::PAUSED.load(Ordering::Relaxed);
    status.notifications.retain(|notification| {
        !status.paused
            && !telegram.interrupt.load(Ordering::Relaxed)
            && notification.received_at.elapsed() < NOTIFICATION_TTL
    });
    status
}

#[tauri::command]
pub async fn telegram_configure(
    telegram: State<'_, Telegram>,
    api_id: i32,
    api_hash: String,
) -> Result<(), String> {
    validate_credentials(api_id, &api_hash)?;
    telegram
        .request(Action::Configure(api_id, api_hash))
        .await
        .map(|_| ())
}

#[tauri::command]
pub async fn telegram_connect(telegram: State<'_, Telegram>) -> Result<(), String> {
    telegram.request(Action::Connect).await.map(|_| ())
}

#[tauri::command]
pub async fn telegram_authenticate(
    telegram: State<'_, Telegram>,
    kind: String,
    value: String,
) -> Result<(), String> {
    if value.is_empty() || value.len() > 1024 {
        return Err("Enter a valid sign-in value (at most 1024 bytes).".into());
    }
    telegram
        .request(Action::Authenticate(kind, value))
        .await
        .map(|_| ())
}

#[tauri::command]
pub async fn telegram_chats(telegram: State<'_, Telegram>) -> Result<Vec<Chat>, String> {
    serde_json::from_value(telegram.request(Action::Chats).await?)
        .map_err(|_| "Invalid Telegram chat response.".into())
}

#[tauri::command]
pub async fn telegram_history(
    telegram: State<'_, Telegram>,
    chat_id: String,
    from_message_id: Option<String>,
) -> Result<Vec<Message>, String> {
    let chat_id = parse_id(&chat_id, false)?;
    let from = parse_id(from_message_id.as_deref().unwrap_or("0"), true)?;
    serde_json::from_value(telegram.request(Action::History(chat_id, from)).await?)
        .map_err(|_| "Invalid Telegram history response.".into())
}

#[tauri::command]
pub async fn telegram_send(
    telegram: State<'_, Telegram>,
    chat_id: String,
    text: String,
) -> Result<Message, String> {
    let chat_id = parse_id(&chat_id, false)?;
    validate_message(&text)?;
    serde_json::from_value(telegram.request(Action::Send(chat_id, text)).await?)
        .map_err(|_| "Invalid Telegram message response.".into())
}

#[tauri::command]
pub async fn telegram_disconnect(telegram: State<'_, Telegram>) -> Result<(), String> {
    telegram.interrupt.store(true, Ordering::Relaxed);
    telegram.request(Action::Disconnect).await.map(|_| ())
}

#[tauri::command]
pub async fn telegram_logout(telegram: State<'_, Telegram>) -> Result<(), String> {
    telegram.request(Action::Logout).await.map(|_| ())
}

// IDs cross the JavaScript boundary as strings, and must fit TDLib's int53.
fn parse_id(value: &str, allow_zero: bool) -> Result<i64, String> {
    let id = value
        .parse::<i64>()
        .map_err(|_| "Invalid Telegram identifier.".to_string())?;
    if id.unsigned_abs() > 9_007_199_254_740_991
        || (!allow_zero && id == 0)
        || (allow_zero && id < 0)
    {
        return Err("Invalid Telegram identifier.".into());
    }
    Ok(id)
}

fn validate_credentials(api_id: i32, api_hash: &str) -> Result<(), String> {
    if api_id <= 0 || api_hash.len() != 32 || !api_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Enter the API ID and 32-character API hash from my.telegram.org.".into());
    }
    Ok(())
}

fn validate_message(text: &str) -> Result<(), String> {
    if text.trim().is_empty() || text.chars().count() > 4096 {
        return Err("A Telegram message must contain between 1 and 4096 characters.".into());
    }
    Ok(())
}

type Create = unsafe extern "C" fn() -> *mut c_void;
type Send = unsafe extern "C" fn(*mut c_void, *const c_char);
type Receive = unsafe extern "C" fn(*mut c_void, f64) -> *const c_char;
type Execute = unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char;
type Destroy = unsafe extern "C" fn(*mut c_void);

struct TdLib {
    // Keep the library loaded for the complete lifetime of every function pointer.
    _library: Library,
    create: Create,
    send: Send,
    receive: Receive,
    destroy: Destroy,
}

impl TdLib {
    fn load() -> Result<Self, String> {
        let mut candidates = Vec::new();
        if let Ok(executable) = std::env::current_exe() {
            if let Some(parent) = executable.parent() {
                candidates.push(parent.join("libtdjson.so"));
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join(".local/lib/coucou/libtdjson.so"));
        }
        candidates.extend([
            PathBuf::from("libtdjson.so"),
            PathBuf::from("libtdjson.so.1.8.67"),
        ]);
        for path in candidates {
            // Symbols and their signatures are the published TDLib JSON C API.
            let loaded = unsafe {
                (|| -> Result<Self, libloading::Error> {
                    let library = Library::new(path)?;
                    let create = *library.get::<Create>(b"td_json_client_create\0")?;
                    let send = *library.get::<Send>(b"td_json_client_send\0")?;
                    let receive = *library.get::<Receive>(b"td_json_client_receive\0")?;
                    let execute = *library.get::<Execute>(b"td_json_client_execute\0")?;
                    let destroy = *library.get::<Destroy>(b"td_json_client_destroy\0")?;
                    // TDLib can log credentials at verbose levels; disable its log stream.
                    let quiet = CString::new(
                        r#"{"@type":"setLogStream","log_stream":{"@type":"logStreamEmpty"}}"#,
                    )
                    .unwrap();
                    execute(std::ptr::null_mut(), quiet.as_ptr());
                    Ok(Self {
                        _library: library,
                        create,
                        send,
                        receive,
                        destroy,
                    })
                })()
            };
            if let Ok(library) = loaded {
                return Ok(library);
            }
        }
        Err("Telegram runtime is missing or could not be loaded. Run scripts/install-telegram-linux.sh and restart Coucou.".into())
    }
}

struct Worker {
    library: Option<TdLib>,
    client: *mut c_void,
    status: Arc<Mutex<Status>>,
    interrupt: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    parameters: Option<Value>,
    next_id: u64,
    chats: HashMap<i64, Chat>,
    users: HashMap<i64, String>,
    send_results: HashMap<(i64, i64), Result<Value, String>>,
    was_paused: bool,
    resume_after_pause: bool,
    home_enabled: bool,
    notifications: NotificationState,
}

impl Worker {
    fn new(status: Arc<Mutex<Status>>, interrupt: Arc<AtomicBool>) -> Self {
        Self {
            library: None,
            client: std::ptr::null_mut(),
            status,
            interrupt,
            shutdown: Arc::new(AtomicBool::new(false)),
            parameters: None,
            next_id: 0,
            chats: HashMap::new(),
            users: HashMap::new(),
            send_results: HashMap::new(),
            was_paused: false,
            resume_after_pause: false,
            home_enabled: false,
            notifications: NotificationState::default(),
        }
    }

    fn run(mut self, receiver: mpsc::Receiver<Work>, home_enabled: bool) {
        self.home_enabled = home_enabled;
        self.load_runtime();
        let configured = credentials().is_ok();
        self.status.lock().unwrap().configured = configured;
        self.connect_saved(home_enabled);
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                self.close();
                break;
            }
            let paused = integrations::PAUSED.load(Ordering::Relaxed);
            if paused && !self.was_paused {
                self.resume_after_pause |= !self.client.is_null();
                self.close();
            } else if !paused && self.was_paused && self.resume_after_pause {
                if let Err(error) = self.connect() {
                    self.status.lock().unwrap().error = Some(error);
                }
                self.resume_after_pause = false;
            }
            self.was_paused = paused;
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(work) => {
                    // A caller that timed out or closed its webview must not cause a
                    // queued message to be sent later without feedback.
                    if work.reply.as_ref().is_some_and(|reply| reply.is_closed()) {
                        continue;
                    }
                    let result = self.handle(work.action);
                    if let Some(reply) = work.reply {
                        let _ = reply.send(result);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.close();
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            for _ in 0..64 {
                let Some(value) = self.receive(0.0) else {
                    break;
                };
                self.update(&value);
            }
        }
    }

    fn load_runtime(&mut self) {
        match TdLib::load() {
            Ok(library) => {
                self.library = Some(library);
                let mut status = self.status.lock().unwrap();
                status.runtime_available = true;
                status.error = None;
            }
            Err(error) => {
                let mut status = self.status.lock().unwrap();
                status.runtime_available = false;
                status.error = Some(error);
            }
        }
    }

    fn handle(&mut self, action: Action) -> Result<Value, String> {
        match action {
            Action::Configure(api_id, api_hash) => {
                validate_credentials(api_id, &api_hash)?;
                if !self.client.is_null() {
                    return Err(
                        "Disconnect Telegram before changing application credentials.".into(),
                    );
                }
                let credentials = json!({"apiId":api_id,"apiHash":api_hash}).to_string();
                secrets::set("telegram-api-credentials", &credentials).map_err(|e| {
                    format!("Could not save Telegram credentials in Secret Service: {e}")
                })?;
                self.status.lock().unwrap().configured = true;
                Ok(Value::Null)
            }
            Action::Connect => {
                self.connect()?;
                Ok(Value::Null)
            }
            Action::SetHomeEnabled(enabled) => {
                self.home_enabled = enabled;
                self.notifications.armed_at = None;
                self.status.lock().unwrap().notifications.clear();
                if enabled {
                    self.connect_saved(true);
                    self.arm_notifications();
                } else {
                    self.resume_after_pause = false;
                    self.close();
                }
                Ok(Value::Null)
            }
            Action::Disconnect => {
                self.resume_after_pause = false;
                self.close();
                Ok(Value::Null)
            }
            Action::Logout => {
                self.require_ready()?;
                self.reset_notifications();
                self.rpc(json!({"@type":"logOut"}))?;
                self.resume_after_pause = false;
                self.clear_account_summary();
                Ok(Value::Null)
            }
            Action::Authenticate(kind, value) => {
                let state = self.status.lock().unwrap().state.clone();
                if state != kind {
                    return Err(
                        "Telegram sign-in has changed. Refresh and use the current sign-in step."
                            .into(),
                    );
                }
                let request = authentication_request(&kind, &value)?;
                self.rpc(request)?;
                self.status.lock().unwrap().error = None;
                Ok(Value::Null)
            }
            Action::Chats => {
                self.require_ready()?;
                // 404 means that all chats in this list have already been loaded.
                let loaded = self.rpc_raw(json!({"@type":"loadChats","chat_list":{"@type":"chatListMain"},"limit":MAX_CHATS}))?;
                if loaded["@type"] == "error" && loaded["code"] != 404 {
                    return Err(td_error(&loaded));
                }
                let chats = self.rpc(json!({"@type":"getChats","chat_list":{"@type":"chatListMain"},"limit":MAX_CHATS}))?;
                let ids = chats["chat_ids"]
                    .as_array()
                    .ok_or("Telegram returned an invalid chat list.")?;
                let mut result = Vec::new();
                for id in ids.iter().take(MAX_CHATS).filter_map(Value::as_i64) {
                    if !self.chats.contains_key(&id) {
                        let chat = self.rpc(json!({"@type":"getChat","chat_id":id}))?;
                        self.cache_chat(&chat);
                    }
                    if let Some(chat) = self.chats.get(&id) {
                        result.push(chat.clone());
                    }
                }
                Ok(serde_json::to_value(result).unwrap())
            }
            Action::History(chat_id, from_message_id) => {
                self.require_ready()?;
                let result = self.rpc(json!({"@type":"getChatHistory","chat_id":chat_id,"from_message_id":from_message_id,"offset":0,"limit":50,"only_local":false}))?;
                let messages = result["messages"]
                    .as_array()
                    .ok_or("Telegram returned invalid history.")?;
                let mut result: Vec<Message> = messages
                    .iter()
                    .take(50)
                    .map(|message| self.message(message))
                    .collect();
                result.reverse();
                Ok(serde_json::to_value(result).unwrap())
            }
            Action::Send(chat_id, text) => {
                self.require_ready()?;
                validate_message(&text)?;
                self.send_results.clear();
                let sent = self.rpc(json!({
                    "@type":"sendMessage","chat_id":chat_id,
                    "input_message_content":{"@type":"inputMessageText","text":{"@type":"formattedText","text":text,"entities":[]},"clear_draft":false,"link_preview_options":{"@type":"linkPreviewOptions","is_disabled":true}}
                }))?;
                let confirmed = self.wait_sent(chat_id, sent)?;
                Ok(serde_json::to_value(self.message(&confirmed)).unwrap())
            }
        }
    }

    fn connect_saved(&mut self, home_enabled: bool) {
        // An enabled card does not initiate account setup. Only a previously
        // configured local session is eligible for background connection.
        let configured = self.status.lock().unwrap().configured;
        match saved_connection(
            home_enabled,
            configured,
            has_session(&settings::local_dir().join("telegram")),
            integrations::PAUSED.load(Ordering::Relaxed),
        ) {
            SavedConnection::None => {}
            SavedConnection::AfterPause => self.resume_after_pause = true,
            SavedConnection::Now => {
                if let Err(error) = self.connect() {
                    self.status.lock().unwrap().error = Some(error);
                }
            }
        }
    }

    fn connect(&mut self) -> Result<(), String> {
        if integrations::PAUSED.load(Ordering::Relaxed) {
            return Err(
                "Coucou is paused. Resume it from the tray before connecting Telegram.".into(),
            );
        }
        if !self.client.is_null() {
            return Ok(());
        }
        if self.library.is_none() {
            self.load_runtime();
        }
        if self.library.is_none() {
            return Err(self.status.lock().unwrap().error.clone().unwrap());
        }
        self.parameters = Some(parameters()?);
        if integrations::PAUSED.load(Ordering::Relaxed) || self.shutdown.load(Ordering::Relaxed) {
            self.parameters = None;
            return Err("Coucou is paused or closing. Telegram was not connected.".into());
        }
        self.interrupt.store(false, Ordering::Relaxed);
        self.reset_notifications();
        self.client = unsafe { (self.library.as_ref().unwrap().create)() };
        if self.client.is_null() {
            return Err("TDLib could not create a Telegram client.".into());
        }
        {
            let mut status = self.status.lock().unwrap();
            status.state = "connecting".into();
            status.configured = true;
            status.error = None;
            status.message = None;
        }
        // Pre-authentication options are applied before TDLib's notification
        // manager loads its database, so its initial snapshot is a baseline.
        self.send_raw(json!({"@type":"setOption","name":"notification_group_count_max","value":{"@type":"optionValueInteger","value":25}}))?;
        self.send_raw(json!({"@type":"setOption","name":"notification_group_size_max","value":{"@type":"optionValueInteger","value":10}}))?;
        self.send_raw(json!({"@type":"getAuthorizationState"}))?;
        Ok(())
    }

    fn require_ready(&self) -> Result<(), String> {
        self.check_interrupt()?;
        if integrations::PAUSED.load(Ordering::Relaxed) {
            return Err("Coucou is paused. Resume it from the tray to use Telegram.".into());
        }
        if self.client.is_null() || self.status.lock().unwrap().state != "ready" {
            return Err("Connect and sign in to Telegram first.".into());
        }
        Ok(())
    }

    fn send_raw(&self, request: Value) -> Result<(), String> {
        if self.client.is_null() {
            return Err("Telegram is disconnected.".into());
        }
        let data = CString::new(request.to_string()).map_err(|_| "Invalid Telegram request.")?;
        unsafe {
            (self.library.as_ref().unwrap().send)(self.client, data.as_ptr());
        }
        Ok(())
    }

    fn receive(&self, timeout: f64) -> Option<Value> {
        if self.client.is_null() {
            return None;
        }
        let pointer = unsafe { (self.library.as_ref().unwrap().receive)(self.client, timeout) };
        if pointer.is_null() {
            return None;
        }
        // Copy/parse before the next receive invalidates TDLib's response buffer.
        let bytes = unsafe { CStr::from_ptr(pointer) }.to_bytes();
        if bytes.len() > 8 * 1024 * 1024 {
            return None;
        }
        serde_json::from_slice(bytes).ok()
    }

    fn rpc_raw(&mut self, mut request: Value) -> Result<Value, String> {
        // Disconnect/pause can arrive while this request is waiting in the queue.
        // Check before transmitting, not only while waiting for its response.
        self.check_interrupt()?;
        self.next_id += 1;
        let id = format!("coucou-{}", self.next_id);
        request["@extra"] = json!(id);
        self.send_raw(request)?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        while Instant::now() < deadline {
            self.check_interrupt()?;
            if self.client.is_null() {
                return Err("Telegram disconnected before completing the request.".into());
            }
            if let Some(value) = self.receive(0.1) {
                self.update(&value);
                if value["@extra"] == id {
                    return Ok(value);
                }
            }
        }
        Err("Telegram request timed out. Refresh before retrying; a message may still be delivered.".into())
    }

    fn rpc(&mut self, request: Value) -> Result<Value, String> {
        let value = self.rpc_raw(request)?;
        if value["@type"] == "error" {
            Err(td_error(&value))
        } else {
            Ok(value)
        }
    }

    fn check_interrupt(&self) -> Result<(), String> {
        if self.client.is_null() {
            return Err("Telegram is disconnected.".into());
        }
        if self.interrupt.load(Ordering::Relaxed) || integrations::PAUSED.load(Ordering::Relaxed) {
            return Err("Telegram was disconnected or paused. Refresh before retrying a message; delivery may already have happened.".into());
        }
        Ok(())
    }

    fn wait_sent(&mut self, chat_id: i64, sent: Value) -> Result<Value, String> {
        let Some(sending_state) = sent.get("sending_state").filter(|s| !s.is_null()) else {
            return Ok(sent);
        };
        if sending_state["@type"] == "messageSendingStateFailed" {
            return Err(td_error(&sending_state["error"]));
        }
        let message_id = sent["id"]
            .as_i64()
            .ok_or("Telegram returned an invalid sent message.")?;
        let deadline = Instant::now() + SEND_TIMEOUT;
        loop {
            if let Some(result) = self.send_results.remove(&(chat_id, message_id)) {
                return result;
            }
            self.check_interrupt()?;
            if Instant::now() >= deadline {
                return Err("Telegram has not confirmed delivery yet. Refresh this chat before retrying to avoid a duplicate message.".into());
            }
            if let Some(update) = self.receive(0.1) {
                self.update(&update);
            }
        }
    }

    fn update(&mut self, value: &Value) {
        let kind = value["@type"].as_str().unwrap_or("");
        if value["@extra"] == "initialize" && kind == "error" {
            self.status.lock().unwrap().error = Some(td_error(value));
        }
        if value["@extra"] == "account" && kind == "user" {
            self.status.lock().unwrap().account_name = Some(user_name(value));
        }
        match kind {
            "updateAuthorizationState" => self.authorization(&value["authorization_state"]),
            "updateActiveNotifications" => {
                // TDLib restores these from earlier launches. Never announce them.
                if let Some(groups) = value["groups"].as_array() {
                    for group in groups {
                        if let Some(notifications) = group["notifications"].as_array() {
                            for notification in notifications {
                                self.notifications.baseline_id = self
                                    .notifications
                                    .baseline_id
                                    .max(notification["id"].as_i64().unwrap_or(0));
                            }
                        }
                    }
                }
                self.notifications.baseline_received = true;
                self.arm_notifications();
            }
            "updateHavePendingNotifications" => {
                self.notifications.pending_unreceived = value["have_unreceived_notifications"]
                    .as_bool()
                    .unwrap_or(true);
                self.notifications.pending_delayed = value["have_delayed_notifications"]
                    .as_bool()
                    .unwrap_or(true);
                // This also toggles during normal live delivery. Only wait for
                // the initial sync; resetting here drops messages being fetched.
                self.arm_notifications();
            }
            "updateNotificationGroup" => self.notification_group(value, unix_time()),
            "updateNotification" => self.update_notification(&value["notification"]),
            "updateUnreadMessageCount" | "updateUnreadChatCount" => {
                // These totals cover the whole main list, including chats that
                // have not been loaded into our bounded history-window cache.
                if value["chat_list"]["@type"] == "chatListMain" {
                    if let Some(count) = value["unread_count"].as_i64().filter(|n| *n >= 0) {
                        let mut status = self.status.lock().unwrap();
                        if kind == "updateUnreadMessageCount" {
                            status.unread_count = Some(count);
                        } else {
                            status.unread_chat_count = Some(count);
                        }
                    }
                }
            }
            "updateUser" => {
                let user = &value["user"];
                if let Some(id) = user["id"].as_i64() {
                    trim_cache(&mut self.users, MAX_CACHED_USERS);
                    self.users.insert(id, user_name(user));
                }
            }
            "updateNewChat" => self.cache_chat(&value["chat"]),
            "updateChatTitle" | "updateChatLastMessage" | "updateChatReadInbox" => {
                if let Some(chat) = value["chat_id"]
                    .as_i64()
                    .and_then(|id| self.chats.get_mut(&id))
                {
                    match kind {
                        "updateChatTitle" => {
                            chat.title = value["title"].as_str().unwrap_or("").into()
                        }
                        "updateChatLastMessage" => {
                            chat.last_message = message_text(&value["last_message"])
                        }
                        _ => chat.unread_count = value["unread_count"].as_i64().unwrap_or(0),
                    }
                }
            }
            "updateMessageSendSucceeded" | "updateMessageSendFailed" => {
                if let (Some(chat), Some(old)) = (
                    value["message"]["chat_id"].as_i64(),
                    value["old_message_id"].as_i64(),
                ) {
                    trim_cache(&mut self.send_results, 32);
                    let result = if kind == "updateMessageSendSucceeded" {
                        Ok(value["message"].clone())
                    } else {
                        Err(td_error(&value["error"]))
                    };
                    self.send_results.insert((chat, old), result);
                }
            }
            _ => {}
        }
    }

    fn authorization(&mut self, auth: &Value) {
        let (state, message) = authorization_status(auth);
        {
            let mut status = self.status.lock().unwrap();
            status.state = state.into();
            status.message = message;
            status.error = None;
            if matches!(state, "closed" | "loggingOut") {
                status.account_name = None;
                status.unread_count = None;
                status.unread_chat_count = None;
            }
        }
        if state == "parameters" {
            if integrations::PAUSED.load(Ordering::Relaxed)
                || self.interrupt.load(Ordering::Relaxed)
            {
                return;
            }
            if let Some(mut parameters) = self.parameters.take() {
                parameters["@extra"] = json!("initialize");
                if let Err(error) = self.send_raw(parameters) {
                    self.status.lock().unwrap().error = Some(error);
                }
            }
        } else if state == "ready" {
            let _ = self.send_raw(json!({"@type":"getMe","@extra":"account"}));
            self.arm_notifications();
        } else if state == "closed" {
            // Destroy only after authorizationStateClosed, as required by TDLib.
            if !self.client.is_null() {
                unsafe {
                    (self.library.as_ref().unwrap().destroy)(self.client);
                }
                self.client = std::ptr::null_mut();
            }
            self.chats.clear();
            self.users.clear();
            self.send_results.clear();
        }
        if matches!(state, "closed" | "loggingOut" | "closing") {
            self.reset_notifications();
        }
    }

    fn reset_notifications(&mut self) {
        self.notifications = NotificationState {
            generation: self.notifications.generation.wrapping_add(1),
            ..NotificationState::default()
        };
        self.status.lock().unwrap().notifications.clear();
    }

    fn arm_notifications(&mut self) {
        if self.home_enabled
            && self.notifications.baseline_received
            && !self.notifications.pending_unreceived
            && !self.notifications.pending_delayed
            && self.notifications.armed_at.is_none()
            && !integrations::PAUSED.load(Ordering::Relaxed)
            && !self.interrupt.load(Ordering::Relaxed)
            && self.status.lock().unwrap().state == "ready"
        {
            self.notifications.armed_at = Some(unix_time());
        }
    }

    fn notification_group(&mut self, value: &Value, now: i64) {
        if let Some(removed) = value["removed_notification_ids"].as_array() {
            self.status
                .lock()
                .unwrap()
                .notifications
                .retain(|notification| {
                    !removed
                        .iter()
                        .any(|id| id.as_i64() == Some(notification.notification_id))
                });
        }
        let Some(added) = value["added_notifications"].as_array() else {
            return;
        };
        let ready = self.home_enabled
            && !integrations::PAUSED.load(Ordering::Relaxed)
            && !self.interrupt.load(Ordering::Relaxed)
            && self.status.lock().unwrap().state == "ready";
        self.notifications
            .seen
            .retain(|_, received| received.elapsed() < NOTIFICATION_TTL);
        for notification in added {
            let Some(id) = notification["id"].as_i64().filter(|id| *id > 0) else {
                continue;
            };
            if self.notifications.armed_at.is_none() {
                self.notifications.baseline_id = self.notifications.baseline_id.max(id);
            }
            if id <= self.notifications.baseline_id || self.notifications.seen.contains_key(&id) {
                continue;
            }
            // IDs can arrive out of order across delayed notification groups.
            // Keep a bounded set rather than dropping anything below a high-water mark.
            if self.notifications.seen.len() >= 2_048 {
                if let Some(oldest) = self
                    .notifications
                    .seen
                    .iter()
                    .min_by_key(|(_, time)| **time)
                    .map(|(id, _)| *id)
                {
                    self.notifications.seen.remove(&oldest);
                }
            }
            self.notifications.seen.insert(id, Instant::now());
            let Some(armed_at) = self.notifications.armed_at.filter(|_| ready) else {
                continue;
            };
            let content = &notification["type"];
            let raw_message = &content["message"];
            let date = raw_message["date"].as_i64().unwrap_or(0);
            // NotificationGroup is TDLib's filtered stream: muted chats are
            // suppressed there. A zero sound id means silent, not muted.
            if content["@type"] != "notificationTypeNewMessage"
                || raw_message["is_outgoing"].as_bool().unwrap_or(true)
                || date < armed_at
                || now.saturating_sub(date) >= NOTIFICATION_TTL.as_secs() as i64
                || date > now.saturating_add(5)
            {
                continue;
            }
            let Some(chat_id) = value["chat_id"].as_i64().filter(|id| *id != 0) else {
                continue;
            };
            let mut message = self.message(raw_message);
            if message.id == "0" {
                continue;
            }
            let mut chat = self.chats.get(&chat_id).cloned().unwrap_or(Chat {
                id: chat_id.to_string(),
                title: "Telegram chat".into(),
                last_message: String::new(),
                unread_count: 0,
            });
            if content["show_preview"].as_bool() != Some(true) {
                message.text = "New message".into();
                message.sender_name.clear();
            }
            chat.last_message = message.text.clone();
            let mut status = self.status.lock().unwrap();
            status
                .notifications
                .retain(|notification| notification.received_at.elapsed() < NOTIFICATION_TTL);
            if status.notifications.len() >= MAX_NOTIFICATIONS {
                status.notifications.remove(0);
            }
            status.notifications.push(Notification {
                id: format!("{}:{id}", self.notifications.generation),
                chat,
                message,
                notification_id: id,
                received_at: Instant::now(),
            });
        }
    }

    fn update_notification(&mut self, notification: &Value) {
        let Some(id) = notification["id"].as_i64() else {
            return;
        };
        let content = &notification["type"];
        if content["@type"] != "notificationTypeNewMessage" {
            return;
        }
        let mut message = self.message(&content["message"]);
        if message.id == "0" || message.outgoing {
            return;
        }
        if content["show_preview"].as_bool() != Some(true) {
            message.text = "New message".into();
            message.sender_name.clear();
        }
        // Edits and preview-privacy changes replace an existing alert in place;
        // they never resurrect an expired or startup notification.
        if let Some(existing) = self
            .status
            .lock()
            .unwrap()
            .notifications
            .iter_mut()
            .find(|item| item.notification_id == id)
        {
            existing.chat.last_message = message.text.clone();
            existing.message = message;
        }
    }

    fn cache_chat(&mut self, value: &Value) {
        if let Some(id) = value["id"].as_i64() {
            trim_cache(&mut self.chats, MAX_CACHED_CHATS);
            self.chats.insert(
                id,
                Chat {
                    id: id.to_string(),
                    title: value["title"].as_str().unwrap_or("Telegram chat").into(),
                    last_message: message_text(&value["last_message"]),
                    unread_count: value["unread_count"].as_i64().unwrap_or(0),
                },
            );
        }
    }

    fn message(&self, value: &Value) -> Message {
        let sender = &value["sender_id"];
        let sender_name = match sender["@type"].as_str() {
            Some("messageSenderUser") => sender["user_id"]
                .as_i64()
                .and_then(|id| self.users.get(&id))
                .cloned(),
            Some("messageSenderChat") => sender["chat_id"]
                .as_i64()
                .and_then(|id| self.chats.get(&id))
                .map(|chat| chat.title.clone()),
            _ => None,
        }
        .unwrap_or_default();
        Message {
            id: value["id"].as_i64().unwrap_or(0).to_string(),
            text: message_text(value),
            date: value["date"].as_i64().unwrap_or(0),
            outgoing: value["is_outgoing"].as_bool().unwrap_or(false),
            sender_name,
        }
    }

    fn clear_account_summary(&self) {
        let mut status = self.status.lock().unwrap();
        status.account_name = None;
        status.unread_count = None;
        status.unread_chat_count = None;
        status.notifications.clear();
    }

    fn close(&mut self) {
        self.reset_notifications();
        self.status.lock().unwrap().state = "closing".into();
        self.parameters = None;
        if !self.client.is_null() {
            let _ = self.send_raw(json!({"@type":"close"}));
            let deadline = Instant::now() + Duration::from_secs(5);
            while !self.client.is_null() && Instant::now() < deadline {
                if let Some(update) = self.receive(0.1) {
                    self.update(&update);
                }
            }
            if !self.client.is_null() {
                // Destroy waits for TDLib's internal shutdown and database flush.
                unsafe {
                    (self.library.as_ref().unwrap().destroy)(self.client);
                }
                self.client = std::ptr::null_mut();
            }
        }
        self.chats.clear();
        self.users.clear();
        self.send_results.clear();
        self.clear_account_summary();
        let mut status = self.status.lock().unwrap();
        status.state = "disconnected".into();
        status.message = None;
        self.interrupt.store(false, Ordering::Relaxed);
    }
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn credentials() -> Result<(i32, String), String> {
    let stored = secrets::get("telegram-api-credentials")
        .and_then(|value| serde_json::from_str::<Value>(&value).ok())
        .ok_or("Save your Telegram API ID and API hash first. If already saved, unlock your system keyring.")?;
    let api_id = stored["apiId"]
        .as_i64()
        .and_then(|id| i32::try_from(id).ok())
        .unwrap_or(0);
    let api_hash = stored["apiHash"].as_str().unwrap_or("").to_string();
    validate_credentials(api_id, &api_hash)?;
    Ok((api_id, api_hash))
}

fn has_session(directory: &Path) -> bool {
    directory.join("db.sqlite").is_file() || directory.join("td.binlog").is_file()
}

#[derive(Debug, PartialEq)]
enum SavedConnection {
    None,
    Now,
    AfterPause,
}

fn saved_connection(
    enabled: bool,
    configured: bool,
    has_session: bool,
    paused: bool,
) -> SavedConnection {
    if !enabled || !configured || !has_session {
        SavedConnection::None
    } else if paused {
        SavedConnection::AfterPause
    } else {
        SavedConnection::Now
    }
}

fn parameters() -> Result<Value, String> {
    let (api_id, api_hash) = credentials()?;
    let directory = settings::local_dir().join("telegram");
    let existing_database = has_session(&directory);
    let encryption_key = match secrets::get("telegram-database-key") {
        Some(value) => value,
        None if existing_database => return Err("Telegram's database key is unavailable. Unlock your system keyring; the existing session will be preserved.".into()),
        None => {
            let mut bytes = [0u8; 32];
            std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes))
                .map_err(|_| "Could not create a secure Telegram database key.")?;
            let value = base64::engine::general_purpose::STANDARD.encode(bytes);
            secrets::set("telegram-database-key", &value).map_err(|e| format!("Could not save Telegram database key in Secret Service: {e}"))?;
            value
        }
    };
    let files = directory.join("files");
    for path in [&directory, &files] {
        std::fs::create_dir_all(path)
            .map_err(|e| format!("Could not create Telegram data directory: {e}"))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("Could not protect Telegram data directory: {e}"))?;
    }
    Ok(json!({
        "@type":"setTdlibParameters", "use_test_dc":false,
        "database_directory":directory, "files_directory":files,
        "database_encryption_key":encryption_key,
        "use_file_database":true,"use_chat_info_database":true,"use_message_database":true,
        "use_secret_chats":false,"api_id":api_id,"api_hash":api_hash,
        "system_language_code":"en","device_model":"Coucou Linux","system_version":"Linux",
        "application_version":env!("CARGO_PKG_VERSION")
    }))
}

fn authentication_request(kind: &str, value: &str) -> Result<Value, String> {
    match kind {
        "phone" => Ok(json!({"@type":"setAuthenticationPhoneNumber","phone_number":value})),
        "code" => Ok(json!({"@type":"checkAuthenticationCode","code":value})),
        "password" => Ok(json!({"@type":"checkAuthenticationPassword","password":value})),
        "email" => Ok(json!({"@type":"setAuthenticationEmailAddress","email_address":value})),
        "emailCode" => Ok(
            json!({"@type":"checkAuthenticationEmailCode","code":{"@type":"emailAddressAuthenticationCode","code":value}}),
        ),
        _ => Err("Unsupported Telegram sign-in step.".into()),
    }
}

fn authorization_status(auth: &Value) -> (&'static str, Option<String>) {
    let state = auth["@type"].as_str().unwrap_or("");
    match state {
        "authorizationStateWaitTdlibParameters" => ("parameters", None),
        "authorizationStateWaitPhoneNumber" => ("phone", None),
        "authorizationStateWaitCode" => ("code", Some("Enter the code Telegram sent to your Telegram app or phone.".into())),
        "authorizationStateWaitPassword" => ("password", Some("Enter your Telegram two-step verification password.".into())),
        "authorizationStateWaitEmailAddress" => ("email", None),
        "authorizationStateWaitEmailCode" => ("emailCode", Some("Enter the verification code Telegram sent to your email.".into())),
        "authorizationStateWaitRegistration" => ("registration", Some("This phone number has no existing Telegram account. Create one in the official Telegram app, then reconnect here.".into())),
        "authorizationStateReady" => ("ready", None),
        "authorizationStateLoggingOut" => ("loggingOut", None),
        "authorizationStateClosing" => ("closing", None),
        "authorizationStateClosed" => ("closed", None),
        _ => ("unsupported", Some("Telegram requested a sign-in step this client does not yet support. Use the official Telegram app to check your account, then reconnect.".into())),
    }
}

fn td_error(value: &Value) -> String {
    let message = value["message"]
        .as_str()
        .unwrap_or("Telegram could not complete the request.");
    format!(
        "Telegram: {}",
        message.chars().take(300).collect::<String>()
    )
}

fn user_name(value: &Value) -> String {
    format!(
        "{} {}",
        value["first_name"].as_str().unwrap_or(""),
        value["last_name"].as_str().unwrap_or("")
    )
    .trim()
    .into()
}

fn message_text(value: &Value) -> String {
    let content = &value["content"];
    if let Some(text) = content["text"]["text"].as_str() {
        return text.into();
    }
    let label = match content["@type"].as_str().unwrap_or("") {
        "messagePhoto" => "Photo",
        "messageVideo" => "Video",
        "messageDocument" => "Document",
        "messageAudio" => "Audio",
        "messageVoiceNote" => "Voice message",
        "messageSticker" => "Sticker",
        "messageAnimation" => "Animation",
        "messageVideoNote" => "Video message",
        "messagePoll" => "Poll",
        "messageLocation" | "messageVenue" => "Location",
        "messageContact" => "Contact",
        "" => return String::new(),
        _ => "Service or unsupported message",
    };
    let caption = content["caption"]["text"].as_str().unwrap_or("");
    if caption.is_empty() {
        format!("[{label}]")
    } else {
        format!("[{label}] {caption}")
    }
}

fn trim_cache<K: std::hash::Hash + Eq + Clone, V>(cache: &mut HashMap<K, V>, limit: usize) {
    if cache.len() >= limit {
        if let Some(key) = cache.keys().next().cloned() {
            cache.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_credentials_and_identifiers() {
        assert!(validate_credentials(123, "0123456789abcdef0123456789abcdef").is_ok());
        assert!(validate_credentials(0, "0123456789abcdef0123456789abcdef").is_err());
        assert!(validate_credentials(123, "not a secret hash").is_err());
        assert_eq!(parse_id("-1001234567890", false).unwrap(), -1001234567890);
        assert!(parse_id("9007199254740992", false).is_err());
        assert!(parse_id("0", false).is_err());
        assert!(parse_id("-1", true).is_err());
    }

    #[test]
    fn rejects_empty_and_oversize_messages() {
        assert!(validate_message(" \n ").is_err());
        assert!(validate_message(&"ü".repeat(4096)).is_ok());
        assert!(validate_message(&"ü".repeat(4097)).is_err());
    }

    #[test]
    fn auth_flow_maps_steps_and_encodes_email_code() {
        for (source, expected) in [
            ("authorizationStateWaitPhoneNumber", "phone"),
            ("authorizationStateWaitCode", "code"),
            ("authorizationStateWaitPassword", "password"),
            ("authorizationStateReady", "ready"),
        ] {
            assert_eq!(authorization_status(&json!({"@type":source})).0, expected);
        }
        assert_eq!(
            authentication_request("emailCode", "123456").unwrap()["code"]["@type"],
            "emailAddressAuthenticationCode"
        );
        assert!(authentication_request("arbitrary_method", "x").is_err());
    }

    #[test]
    fn message_summaries_preserve_text_and_identify_media() {
        assert_eq!(
            message_text(
                &json!({"content":{"@type":"messageText","text":{"text":"<script>untrusted</script>"}}})
            ),
            "<script>untrusted</script>"
        );
        assert_eq!(
            message_text(&json!({"content":{"@type":"messagePhoto","caption":{"text":"hello"}}})),
            "[Photo] hello"
        );
        assert_eq!(
            message_text(&json!({"content":{"@type":"messageSticker"}})),
            "[Sticker]"
        );
    }

    fn worker() -> Worker {
        Worker::new(
            Arc::new(Mutex::new(Status::default())),
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn notification_worker() -> Worker {
        let mut worker = worker();
        worker.home_enabled = true;
        worker.status.lock().unwrap().state = "ready".into();
        worker.update(&json!({"@type":"updateActiveNotifications","groups":[]}));
        worker.update(&json!({"@type":"updateHavePendingNotifications","have_unreceived_notifications":false,"have_delayed_notifications":false}));
        worker.notifications.armed_at = Some(unix_time() - 10);
        worker.update(
            &json!({"@type":"updateNewChat","chat":{"id":-42,"title":"Friends","unread_count":3}}),
        );
        worker
    }

    fn notification_update(id: i64, date: i64) -> Value {
        json!({
            "@type":"updateNotificationGroup", "notification_group_id":1,
            "type":{"@type":"notificationGroupTypeMessages"}, "chat_id":-42,
            "notification_sound_id":0, "removed_notification_ids":[],
            "added_notifications":[{"id":id,"date":date,"is_silent":false,
                "type":{"@type":"notificationTypeNewMessage","show_preview":true,
                    "message":{"id":id * 100,"chat_id":-42,"date":date,"is_outgoing":false,
                        "content":{"@type":"messageText","text":{"text":"hello"}}}}}]
        })
    }

    #[test]
    fn notifications_only_use_fresh_incoming_tdlib_notifications() {
        let mut worker = notification_worker();
        let now = unix_time();
        worker.notification_group(&notification_update(11, now), now);
        let notifications = worker.status.lock().unwrap().notifications.clone();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].chat.title, "Friends");
        assert_eq!(notifications[0].message.text, "hello");
        // Muting is resolved by TDLib's Notification API; a disabled sound
        // does not suppress a visual notification delivered by that API.
        worker
            .update(&json!({"@type":"updateNewMessage","message":{"id":999,"is_outgoing":false}}));
        worker.update(&json!({"@type":"updateUnreadMessageCount","chat_list":{"@type":"chatListMain"},"unread_count":9}));
        let mut outgoing = notification_update(12, now);
        outgoing["added_notifications"][0]["type"]["message"]["is_outgoing"] = json!(true);
        worker.notification_group(&outgoing, now);
        worker.notification_group(&notification_update(13, now - 61), now);
        assert_eq!(worker.status.lock().unwrap().notifications.len(), 1);
    }

    #[test]
    fn notifications_ignore_startup_and_sync_backlogs() {
        let mut worker = worker();
        worker.home_enabled = true;
        worker.status.lock().unwrap().state = "ready".into();
        let now = unix_time();
        worker.update(
            &json!({"@type":"updateActiveNotifications","groups":[{"notifications":[{"id":10}]}]}),
        );
        worker.notification_group(&notification_update(11, now), now);
        assert!(worker.status.lock().unwrap().notifications.is_empty());
        worker.update(&json!({"@type":"updateHavePendingNotifications","have_unreceived_notifications":false,"have_delayed_notifications":false}));
        worker.notification_group(&notification_update(10, now), now);
        worker.notification_group(&notification_update(11, now), now);
        // Backfilled items can have a fresh notification id but an old message.
        worker.notification_group(&notification_update(12, now - 1), now);
        assert!(worker.status.lock().unwrap().notifications.is_empty());
        worker.notifications.armed_at = Some(now - 1);
        worker.notification_group(&notification_update(13, now), now);
        assert_eq!(worker.status.lock().unwrap().notifications.len(), 1);
        worker.update(&json!({"@type":"updateHavePendingNotifications","have_unreceived_notifications":true,"have_delayed_notifications":true}));
        assert_eq!(worker.status.lock().unwrap().notifications.len(), 1);
        worker.notification_group(&notification_update(14, now), now);
        worker.update(&json!({"@type":"updateHavePendingNotifications","have_unreceived_notifications":false,"have_delayed_notifications":true}));
        assert_eq!(worker.notifications.armed_at, Some(now - 1));
        worker.update(&json!({"@type":"updateHavePendingNotifications","have_unreceived_notifications":false,"have_delayed_notifications":false}));
        worker.notification_group(&notification_update(14, now), now);
        assert_eq!(worker.status.lock().unwrap().notifications.len(), 2);
    }

    #[test]
    fn notification_in_the_activation_second_is_not_lost() {
        let mut worker = notification_worker();
        let now = unix_time();
        worker.notifications.armed_at = Some(now);
        worker.notification_group(&notification_update(11, now), now);
        assert_eq!(worker.status.lock().unwrap().notifications.len(), 1);
    }

    #[test]
    fn notification_updates_replace_preview_without_replaying_old_alerts() {
        let mut worker = notification_worker();
        let now = unix_time();
        worker.notification_group(&notification_update(11, now), now);
        let original = worker.status.lock().unwrap().notifications[0].clone();
        let mut update = json!({"@type":"updateNotification", "notification_group_id":1,
            "notification":notification_update(11, now)["added_notifications"][0].clone()});
        update["notification"]["type"]["message"]["content"]["text"]["text"] = json!("Corrected message");
        worker.update(&update);
        let current = worker.status.lock().unwrap().notifications[0].clone();
        assert_eq!(current.message.text, "Corrected message");
        assert_eq!(current.chat.last_message, "Corrected message");
        assert_eq!(current.id, original.id);
        assert_eq!(current.received_at, original.received_at);
        update["notification"]["type"]["show_preview"] = json!(false);
        worker.update(&update);
        let current = worker.status.lock().unwrap().notifications[0].clone();
        assert_eq!(current.message.text, "New message");
        assert!(current.message.sender_name.is_empty());
        worker.status.lock().unwrap().notifications.clear();
        worker.update(&update);
        assert!(worker.status.lock().unwrap().notifications.is_empty());
    }

    #[test]
    fn notifications_allow_delays_but_deduplicate_and_remove_alerts() {
        let mut worker = notification_worker();
        let now = unix_time();
        worker.update(&json!({"@type":"updateHavePendingNotifications","have_unreceived_notifications":false,"have_delayed_notifications":true}));
        worker.notification_group(&notification_update(12, now), now);
        worker.notification_group(&notification_update(11, now), now);
        worker.notification_group(&notification_update(12, now), now);
        assert_eq!(worker.status.lock().unwrap().notifications.len(), 2);
        worker.update(&json!({"@type":"updateNotificationGroup","removed_notification_ids":[11],"added_notifications":[]}));
        assert_eq!(worker.status.lock().unwrap().notifications.len(), 1);
        worker.notification_group(&notification_update(11, now), now);
        assert_eq!(worker.status.lock().unwrap().notifications.len(), 1);
    }

    #[test]
    fn notifications_respect_preview_privacy_and_cache_bounds() {
        let mut worker = notification_worker();
        let now = unix_time();
        for id in 1..=20 {
            worker.notification_group(&notification_update(id, now), now);
        }
        let mut private = notification_update(21, now);
        private["added_notifications"][0]["type"]["show_preview"] = json!(false);
        worker.notification_group(&private, now);
        let status = worker.status.lock().unwrap();
        assert_eq!(status.notifications.len(), MAX_NOTIFICATIONS);
        let last = status.notifications.last().unwrap();
        assert_eq!(last.message.text, "New message");
        assert_eq!(last.chat.last_message, "New message");
        assert!(last.message.sender_name.is_empty());
        let exported = serde_json::to_value(last).unwrap();
        assert!(exported.get("received_at").is_none());
        assert!(exported.get("notification_id").is_none());
    }

    #[test]
    fn notification_summary_reads_expire_without_consuming_live_alerts() {
        let mut worker = notification_worker();
        let now = unix_time();
        worker.notification_group(&notification_update(11, now), now);
        let (sender, _) = mpsc::sync_channel(16);
        let telegram = Telegram {
            sender,
            status: worker.status.clone(),
            interrupt: worker.interrupt.clone(),
            shutdown: Arc::new(AtomicBool::new(false)),
            stopped: Arc::new((Mutex::new(false), Condvar::new())),
        };
        assert_eq!(cached_status(&telegram).notifications.len(), 1);
        assert_eq!(cached_status(&telegram).notifications.len(), 1);
        worker.interrupt.store(true, Ordering::Relaxed);
        assert!(cached_status(&telegram).notifications.is_empty());
        worker.interrupt.store(false, Ordering::Relaxed);
        worker.status.lock().unwrap().notifications[0].received_at =
            Instant::now() - NOTIFICATION_TTL;
        assert!(cached_status(&telegram).notifications.is_empty());
    }

    #[test]
    fn notification_session_resets_clear_alerts_and_change_identifiers() {
        let mut worker = notification_worker();
        let now = unix_time();
        worker.notification_group(&notification_update(11, now), now);
        let old_id = worker.status.lock().unwrap().notifications[0].id.clone();
        worker.close();
        assert!(worker.status.lock().unwrap().notifications.is_empty());
        assert!(!worker.notifications.baseline_received);
        worker.status.lock().unwrap().state = "ready".into();
        worker.notifications.baseline_received = true;
        worker.notifications.armed_at = Some(now - 1);
        worker.notification_group(&notification_update(11, now), now);
        assert_ne!(worker.status.lock().unwrap().notifications[0].id, old_id);
        worker.handle(Action::SetHomeEnabled(false)).unwrap();
        worker.notification_group(&notification_update(12, now), now);
        assert!(worker.status.lock().unwrap().notifications.is_empty());
    }

    #[test]
    fn home_changes_are_enqueued_synchronously_in_preference_order() {
        let (sender, receiver) = mpsc::sync_channel(16);
        let telegram = Telegram {
            sender,
            status: Arc::new(Mutex::new(Status::default())),
            interrupt: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
            stopped: Arc::new((Mutex::new(false), Condvar::new())),
        };
        for enabled in [true, false, true, false] {
            telegram.set_home_enabled(enabled).unwrap();
        }
        assert!(telegram.interrupt.load(Ordering::Relaxed));
        for expected in [true, false, true, false] {
            let work = receiver.try_recv().expect("queued without awaiting a task");
            assert!(
                work.reply.is_none(),
                "control messages must not be cancelled"
            );
            match work.action {
                Action::SetHomeEnabled(enabled) => assert_eq!(enabled, expected),
                _ => panic!("unexpected action"),
            }
        }
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn home_unread_totals_use_main_list_updates_not_the_chat_cache() {
        let mut worker = worker();
        assert_eq!(worker.status.lock().unwrap().unread_count, None);
        worker.update(&json!({"@type":"updateNewChat","chat":{"id":1,"unread_count":99}}));
        worker.update(&json!({"@type":"updateUnreadMessageCount","chat_list":{"@type":"chatListArchive"},"unread_count":80}));
        assert_eq!(worker.status.lock().unwrap().unread_count, None);

        worker.update(&json!({"@type":"updateUnreadMessageCount","chat_list":{"@type":"chatListMain"},"unread_count":5000}));
        worker.update(&json!({"@type":"updateUnreadChatCount","chat_list":{"@type":"chatListMain"},"unread_count":700}));
        worker.update(&json!({"@type":"updateUnreadChatCount","chat_list":{"@type":"chatListFolder","chat_folder_id":1},"unread_count":2}));
        worker.update(&json!({"@type":"updateUnreadMessageCount","chat_list":{"@type":"chatListMain"},"unread_count":-1}));
        assert_eq!(worker.status.lock().unwrap().unread_count, Some(5000));
        assert_eq!(worker.status.lock().unwrap().unread_chat_count, Some(700));

        worker.update(&json!({"@type":"updateUnreadMessageCount","chat_list":{"@type":"chatListMain"},"unread_count":0}));
        assert_eq!(worker.status.lock().unwrap().unread_count, Some(0));
    }

    #[test]
    fn disconnect_and_logout_clear_home_counts_and_cancel_resume() {
        for action in [Action::Disconnect, Action::SetHomeEnabled(false)] {
            let mut worker = worker();
            worker.resume_after_pause = true;
            {
                let mut status = worker.status.lock().unwrap();
                status.account_name = Some("Example account".into());
                status.unread_count = Some(3);
                status.unread_chat_count = Some(2);
            }
            worker.handle(action).unwrap();
            let status = worker.status.lock().unwrap();
            assert_eq!(status.state, "disconnected");
            assert_eq!(status.account_name, None);
            assert_eq!(status.unread_count, None);
            assert_eq!(status.unread_chat_count, None);
            assert!(!worker.resume_after_pause);
        }
        let mut worker = worker();
        worker.status.lock().unwrap().unread_count = Some(3);
        worker.authorization(&json!({"@type":"authorizationStateLoggingOut"}));
        assert_eq!(worker.status.lock().unwrap().unread_count, None);
        worker.status.lock().unwrap().unread_chat_count = Some(2);
        worker.authorization(&json!({"@type":"authorizationStateClosed"}));
        assert_eq!(worker.status.lock().unwrap().unread_chat_count, None);
    }

    #[test]
    fn background_connection_requires_opt_in_credentials_and_existing_session() {
        for enabled in [false, true] {
            for configured in [false, true] {
                for existing in [false, true] {
                    if enabled && configured && existing {
                        assert_eq!(
                            saved_connection(enabled, configured, existing, false),
                            SavedConnection::Now
                        );
                        assert_eq!(
                            saved_connection(enabled, configured, existing, true),
                            SavedConnection::AfterPause
                        );
                    } else {
                        assert_eq!(
                            saved_connection(enabled, configured, existing, false),
                            SavedConnection::None
                        );
                        assert_eq!(
                            saved_connection(enabled, configured, existing, true),
                            SavedConnection::None
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn applies_updates_and_distinguishes_delivery_success_and_failure() {
        let mut worker = worker();
        worker.update(
            &json!({"@type":"updateNewChat","chat":{"id":-42,"title":"Friends","unread_count":3}}),
        );
        worker.update(&json!({"@type":"updateChatReadInbox","chat_id":-42,"unread_count":0}));
        assert_eq!(worker.chats[&-42].unread_count, 0);
        worker.update(&json!({"@type":"updateUser","user":{"id":10,"first_name":"Ada","last_name":"Lovelace"}}));
        let message = worker.message(&json!({"id":901,"sender_id":{"@type":"messageSenderUser","user_id":10},"content":{"@type":"messageText","text":{"text":"hello"}}}));
        assert_eq!(message.sender_name, "Ada Lovelace");
        worker.update(&json!({"@type":"updateMessageSendSucceeded","message":{"id":102,"chat_id":-42},"old_message_id":-1}));
        worker.update(&json!({"@type":"updateMessageSendFailed","message":{"id":103,"chat_id":-42},"old_message_id":-2,"error":{"message":"CHAT_WRITE_FORBIDDEN"}}));
        assert_eq!(worker.send_results[&(-42, -1)].as_ref().unwrap()["id"], 102);
        assert!(worker.send_results[&(-42, -2)]
            .as_ref()
            .unwrap_err()
            .contains("CHAT_WRITE_FORBIDDEN"));
    }

    #[test]
    fn cancelled_and_closed_requests_stop_before_transmitting() {
        let mut worker = worker();
        // No TDLib library is installed on this test worker. If rpc_raw tried
        // transmitting either request it would panic instead of returning.
        assert!(worker
            .rpc_raw(json!({"@type":"getMe"}))
            .unwrap_err()
            .contains("disconnected"));
        worker.client = std::ptr::NonNull::<u8>::dangling().as_ptr().cast();
        worker.interrupt.store(true, Ordering::Relaxed);
        assert!(worker
            .rpc_raw(json!({"@type":"getMe"}))
            .unwrap_err()
            .contains("paused"));
        worker.client = std::ptr::null_mut();
    }

    #[test]
    #[ignore = "requires the installed TDLib runtime, no account or network connection"]
    fn installed_runtime_creates_and_closes_client() {
        let mut worker = worker();
        worker.library = Some(TdLib::load().expect("installed TDLib"));
        worker.client = unsafe { (worker.library.as_ref().unwrap().create)() };
        worker
            .send_raw(json!({"@type":"getAuthorizationState"}))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while worker.status.lock().unwrap().state != "parameters" && Instant::now() < deadline {
            if let Some(value) = worker.receive(0.1) {
                worker.update(&value);
            }
        }
        assert_eq!(worker.status.lock().unwrap().state, "parameters");
        worker.close();
        assert!(worker.client.is_null());
    }
}
