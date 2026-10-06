// Coucou for Linux — app wiring and the commands the island calls.

mod codex;
mod codex_models;
mod codex_monitor;
mod codex_navigation;
mod codex_window;
mod autostart_linux;
mod encoding;
mod files;
mod hyprland;
mod integrations;
#[path = "island_linux.rs"]
mod island;
mod log;
mod secrets;
mod settings;
mod telegram;
mod tray;

use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use codex::{Chat, ChatContext, ChatReply};
use files::DroppedFile;
use island::{PollGate, ScreenInfo};
use settings::Settings;

pub struct Shared {
    pub settings: Mutex<Settings>,
    pub gate: Arc<PollGate>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootInfo {
    settings: Settings,
    screen: ScreenInfo,
    version: String,
}

#[tauri::command]
fn boot(app: AppHandle, shared: State<Shared>) -> BootInfo {
    let mut settings = shared.settings.lock().unwrap().clone();
    settings.autostart = autostart_linux::enabled();
    let screen = island::screen_info(&app, &settings.screen);
    BootInfo {
        settings,
        screen,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

#[tauri::command]
fn save_settings(app: AppHandle, shared: State<Shared>, settings: Settings) {
    let (screen_changed, autostart_changed) = {
        let mut current = shared.settings.lock().unwrap();
        let screen_changed = current.screen != settings.screen;
        let autostart_changed = current.autostart != settings.autostart;
        let enabled = settings.active_integrations.iter().any(|id| id == "integration_telegram");
        if current.active_integrations.iter().any(|id| id == "integration_telegram") != enabled {
            if let Err(error) = app.state::<telegram::Telegram>().set_home_enabled(enabled) {
                eprintln!("[coucou] Telegram Home integration: {error}");
            }
        }
        *current = settings.clone();
        (screen_changed, autostart_changed)
    };
    if let Err(err) = settings::save(&settings) {
        eprintln!("[coucou] could not save settings: {err}");
    }
    if autostart_changed {
        let result = autostart_linux::set(settings.autostart);
        if let Err(err) = result {
            eprintln!("[coucou] autostart: {err}");
        }
    }
    if screen_changed {
        let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
        island::apply_geometry(&app, &settings.screen, collapsed);
    }
    // Keep the other window in step (island ⇄ settings window).
    let _ = app.emit("settings-changed", settings);
}

/// Hidden island → shrink the window to the invisible wake strip and park the
/// cursor poll; visible states use 60 Hz polling and fit native bounds.
#[tauri::command]
fn set_collapsed(app: AppHandle, shared: State<Shared>, collapsed: bool) {
    let pref = shared.settings.lock().unwrap().screen.clone();
    shared.gate.collapsed.store(collapsed, Ordering::Relaxed);
    island::apply_geometry(&app, &pref, collapsed);
    // The wake strip must always take the mouse, and a resize invalidates the flag.
    island::set_ignore_cursor(&app, false);
    shared.gate.forget_ignore_state();
    shared.gate.set_active(!collapsed);
}

/// The front end pushes the island shape; Rust decides click-through from it.
#[tauri::command]
fn set_island_rect(shared: State<Shared>, x: f64, y: f64, width: f64, height: f64) {
    shared.gate.set_rect(island::IslandRect { x, y, w: width, h: height });
}

#[tauri::command]
fn focus_window(app: AppHandle, focused: bool) {
    let Some(win) = island::window(&app) else { return };
    island::set_activating(&win, focused);
}

#[tauri::command]
fn reposition(app: AppHandle, shared: State<Shared>) {
    let pref = shared.settings.lock().unwrap().screen.clone();
    let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
    island::apply_geometry(&app, &pref, collapsed);
}

#[tauri::command]
fn open_url(url: String) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let _ = Command::new("xdg-open").arg(&url).spawn();
}

#[tauri::command]
async fn open_codex_session(session_id: String) -> Result<(), String> {
    codex_navigation::open_session(&session_id).await
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Tray → Pause. Paused means paused: the pollers stop talking to the network,
/// not just the island stopping showing things.
#[tauri::command]
fn set_paused(paused: bool) {
    integrations::set_paused(paused);
}

// ── Codex session monitor ────────────────────────────────────────────────────

#[tauri::command]
fn monitor_status() -> codex_monitor::Status {
    codex_monitor::status()
}

#[tauri::command]
fn codex_monitor_ready(app: AppHandle) {
    codex_monitor::start(app);
}

#[tauri::command]
fn codex_activity() -> codex_monitor::ActivitySnapshot {
    codex_monitor::snapshot()
}

#[tauri::command]
async fn focus_codex_window(cwd: Option<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || codex_window::focus(cwd.as_deref()))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn codex_status() -> codex::AccountStatus {
    codex::account_status().await
}

#[tauri::command]
async fn codex_models(chat: State<'_, Chat>, refresh: bool) -> Result<Vec<codex_models::Model>, String> {
    chat.models.list(refresh).await
}

#[tauri::command]
async fn codex_set_preferences(
    app: AppHandle,
    shared: State<'_, Shared>,
    chat: State<'_, Chat>,
    model: String,
    reasoning_effort: String,
) -> Result<Settings, String> {
    chat.models.resolve(&model, &reasoning_effort).await?;
    let updated = {
        let mut current = shared.settings.lock().unwrap();
        let mut updated = current.clone();
        updated.model = model;
        updated.reasoning_effort = reasoning_effort;
        settings::save(&updated).map_err(|e| format!("Cannot save Codex preferences: {e}"))?;
        *current = updated.clone();
        updated
    };
    let _ = app.emit("settings-changed", &updated);
    Ok(updated)
}

// ── Chat, files and secrets ───────────────────────────────────────────────────

#[tauri::command]
async fn chat_warmup(shared: State<'_, Shared>, chat: State<'_, Chat>) -> Result<(), String> {
    let preferences = shared.settings.lock().unwrap().clone();
    codex::warmup(&chat, &preferences.model, &preferences.reasoning_effort).await
}

#[tauri::command]
async fn chat_send(
    shared: State<'_, Shared>,
    chat: State<'_, Chat>,
    query: String,
    context: Option<ChatContext>,
    request_id: String,
    progress: tauri::ipc::Channel<codex::ChatEvent>,
) -> Result<ChatReply, String> {
    let preferences = shared.settings.lock().unwrap().clone();
    codex::send(
        &chat,
        &preferences.model,
        &preferences.reasoning_effort,
        query,
        context,
        request_id,
        move |event| { let _ = progress.send(event); },
    )
    .await
}

#[tauri::command]
fn chat_approve(
    chat: State<'_, Chat>,
    request_id: String,
    approval_id: String,
    allow: bool,
) -> Result<(), String> {
    chat.approve(&request_id, &approval_id, allow)
}

#[tauri::command]
fn chat_cancel(chat: State<'_, Chat>, request_id: String) -> Result<(), String> {
    chat.cancel(&request_id)
}

#[tauri::command]
fn chat_reset(chat: State<Chat>) {
    chat.reset();
}

/// Copies a dropped file into the inbox and reports its name back.
#[tauri::command]
fn ingest_file(path: String) -> Result<DroppedFile, String> {
    files::ingest(&path)
}

/// The island may only ask whether a key exists — never read it.
#[tauri::command]
fn secret_present(key: String) -> bool {
    secrets::present(&key)
}

#[tauri::command]
fn secret_set(key: String, value: String) -> Result<(), String> {
    secrets::set(&key, &value)
}

#[tauri::command]
fn secret_clear(key: String) -> Result<(), String> {
    secrets::clear(&key)
}

/// Opens the configured n8n instance — the URL lives in the system keyring.
#[tauri::command]
fn open_n8n() {
    if let Some(url) = secrets::get("n8n-url") {
        open_url(url);
    }
}

/// Refresh buttons in the integration cards.
#[tauri::command]
async fn refresh_integration(app: AppHandle, id: String) {
    integrations::poll_once(app, &id).await;
}

/// Lets the island write to the same log as the Rust side.
#[tauri::command]
fn log_line(message: String) {
    log::line(format!("ui  {message}"));
}

// ── Settings window ───────────────────────────────────────────────────────────

/// In a dev build the pages are served by Vite, so the second window needs the
/// absolute dev URL; a bundled build resolves it inside the app bundle.
fn settings_page_url(app: &AppHandle) -> WebviewUrl {
    #[cfg(dev)]
    if let Some(mut base) = app.config().build.dev_url.clone() {
        base.set_path("/settings.html");
        return WebviewUrl::External(base);
    }
    let _ = app;
    WebviewUrl::App("settings.html".into())
}

/// The settings window is created hidden at launch and only ever shown and
/// hidden afterwards so its state survives closing and reopening it.
fn create_settings_window(app: &AppHandle) {
    let url = settings_page_url(app);
    let builder = WebviewWindowBuilder::new(app, "settings", url);
    match builder
        .title("Settings — Coucou")
        .inner_size(560.0, 680.0)
        .min_inner_size(460.0, 480.0)
        .resizable(true)
        .visible(false)
        .center()
        .build()
    {
        Ok(win) => {
            // Closing it must only hide it, or it could never be reopened.
            let hidden = win.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = hidden.hide();
                }
            });
        }
        Err(err) => log::line(format!("settings window failed: {err}")),
    }
}

pub fn show_settings_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("settings") else {
        log::line("settings window missing");
        return;
    };
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
}

#[tauri::command]
fn open_settings_window(app: AppHandle) {
    show_settings_window(&app);
}

#[tauri::command]
fn open_telegram_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("telegram") {
        win.unminimize().map_err(|error| error.to_string())?;
        win.show().map_err(|error| error.to_string())?;
        return win.set_focus().map_err(|error| error.to_string());
    }
    #[allow(unused_mut)]
    let mut url = WebviewUrl::App("telegram.html".into());
    #[cfg(dev)]
    if let Some(mut base) = app.config().build.dev_url.clone() {
        base.set_path("/telegram.html");
        url = WebviewUrl::External(base);
    }
    WebviewWindowBuilder::new(&app, "telegram", url)
        .title("Telegram — Coucou")
        .inner_size(960.0, 700.0)
        .min_inner_size(740.0, 540.0)
        .resizable(true)
        .center()
        .build()
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn run() {
    let loaded = settings::load();
    let gate = Arc::new(PollGate::new());

    // GTK's native Wayland backend cannot position a top-edge utility window.
    // XWayland supports the positioning and input regions needed by the island.
    if std::env::var_os("DISPLAY").is_some() && std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }

    tauri::Builder::default()
        .manage(telegram::Telegram::default())
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if argv.iter().any(|arg| arg == "--settings") {
                show_settings_window(app);
            } else if argv.iter().any(|arg| arg == "--telegram") {
                if let Err(error) = open_telegram_window(app.clone()) {
                    log::line(format!("telegram window failed: {error}"));
                }
            } else {
                let _ = app.emit_to(island::WINDOW_LABEL, "tray", "open".to_string());
            }
        }))
        .manage(Shared {
            settings: Mutex::new(loaded.clone()),
            gate: gate.clone(),
        })
        .manage(Chat::default())
        .invoke_handler(tauri::generate_handler![
            boot,
            save_settings,
            set_collapsed,
            set_island_rect,
            focus_window,
            reposition,
            open_url,
            open_codex_session,
            quit_app,
            monitor_status,
            log_line,
            codex_status,
            codex_models,
            codex_set_preferences,
            codex_monitor_ready,
            codex_activity,
            focus_codex_window,
            chat_send,
            chat_warmup,
            chat_approve,
            chat_cancel,
            chat_reset,
            ingest_file,
            secret_present,
            secret_set,
            secret_clear,
            refresh_integration,
            open_n8n,
            open_settings_window,
            open_telegram_window,
            telegram::telegram_status,
            telegram::telegram_summary,
            telegram::telegram_configure,
            telegram::telegram_connect,
            telegram::telegram_authenticate,
            telegram::telegram_chats,
            telegram::telegram_history,
            telegram::telegram_send,
            telegram::telegram_disconnect,
            telegram::telegram_logout,
            set_paused,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            // Only the primary instance may reopen the saved Telegram database.
            if loaded.active_integrations.iter().any(|id| id == "integration_telegram") {
                if let Err(error) = app.state::<telegram::Telegram>().set_home_enabled(true) {
                    eprintln!("[coucou] Telegram Home integration: {error}");
                }
            }
            tray::build(&handle)?;
            // Before the island: see create_settings_window.
            create_settings_window(&handle);
            if std::env::args().any(|arg| arg == "--settings") {
                show_settings_window(&handle);
            }
            if std::env::args().any(|arg| arg == "--telegram") {
                open_telegram_window(handle.clone())?;
            }

            if let Some(win) = island::window(&handle) {
                island::make_non_activating(&win);
                island::apply_geometry(&handle, &loaded.screen, false);
                let _ = win.show();
            }
            gate.collapsed.store(false, Ordering::Relaxed);
            gate.set_active(true);
            island::spawn_cursor_poll(handle.clone(), gate.clone());

            log::line(format!("--- Coucou {} started ---", env!("CARGO_PKG_VERSION")));
            integrations::start(handle.clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Coucou")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                if !app.state::<telegram::Telegram>().shutdown() {
                    log::line("Telegram shutdown exceeded its deadline");
                }
            }
        });
}
