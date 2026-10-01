// Coucou for Windows — app wiring and the commands the island calls.

#[cfg(windows)]
mod claude;
#[cfg(target_os = "linux")]
mod codex;
#[cfg(target_os = "linux")]
mod codex_models;
#[cfg(target_os = "linux")]
mod codex_monitor;
#[cfg(target_os = "linux")]
mod codex_window;
#[cfg(target_os = "linux")]
mod autostart_linux;
mod encoding;
mod files;
#[cfg(windows)]
mod hooks;
mod integrations;
#[cfg(windows)]
mod island;
#[cfg(target_os = "linux")]
#[path = "island_linux.rs"]
mod island;
mod log;
#[cfg(windows)]
mod pipe;
mod secrets;
mod settings;
#[cfg(target_os = "linux")]
mod telegram;
mod tray;
#[cfg(windows)]
mod win_user;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
#[cfg(windows)]
use tauri_plugin_autostart::{ManagerExt, MacosLauncher};

#[cfg(windows)]
use claude::{Chat, ChatContext, ChatReply};
#[cfg(target_os = "linux")]
use codex::{Chat, ChatContext, ChatReply};
use files::DroppedFile;
#[cfg(windows)]
use hooks::{HookPreview, HookStatus};
use island::{PollGate, ScreenInfo};
#[cfg(windows)]
use pipe::Pending;
use settings::Settings;

/// Keeps spawned helpers from flashing a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
    hook_path: String,
    agent_provider: &'static str,
}

#[tauri::command]
fn boot(app: AppHandle, shared: State<Shared>) -> BootInfo {
    let mut settings = shared.settings.lock().unwrap().clone();
    // The real state of ~/.claude/settings.json wins over whatever we stored.
    settings.hooks_installed = hooks_status().installed;
    #[cfg(target_os = "linux")]
    { settings.autostart = autostart_linux::enabled(); }
    let screen = island::screen_info(&app, &settings.screen);
    BootInfo {
        settings,
        screen,
        version: env!("CARGO_PKG_VERSION").to_string(),
        hook_path: hooks_status().hook_path,
        agent_provider: if cfg!(target_os = "linux") { "codex" } else { "claude" },
    }
}

#[tauri::command]
fn save_settings(app: AppHandle, shared: State<Shared>, settings: Settings) {
    let (screen_changed, autostart_changed) = {
        let mut current = shared.settings.lock().unwrap();
        let screen_changed = current.screen != settings.screen;
        let autostart_changed = current.autostart != settings.autostart;
        #[cfg(target_os = "linux")]
        {
            let enabled = settings.active_integrations.iter().any(|id| id == "integration_telegram");
            if current.active_integrations.iter().any(|id| id == "integration_telegram") != enabled {
                if let Err(error) = app.state::<telegram::Telegram>().set_home_enabled(enabled) {
                    eprintln!("[coucou] Telegram Home integration: {error}");
                }
            }
        }
        *current = settings.clone();
        (screen_changed, autostart_changed)
    };
    if let Err(err) = settings::save(&settings) {
        eprintln!("[coucou] could not save settings: {err}");
    }
    if autostart_changed {
        #[cfg(windows)]
        let result = if settings.autostart { app.autolaunch().enable() } else { app.autolaunch().disable() };
        #[cfg(target_os = "linux")]
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
/// cursor poll; visible states use 60 Hz polling (Linux also fits native bounds).
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
    #[cfg(windows)]
    if focused {
        let _ = win.set_focus();
    }
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
    #[cfg(windows)]
    let _ = Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", &url])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    #[cfg(target_os = "linux")]
    let _ = Command::new("xdg-open").arg(&url).spawn();
}

/// "Open terminal" opens the working folder in VS Code when `code` is on PATH,
/// and falls back to Explorer otherwise.
#[cfg(windows)]
#[tauri::command]
fn open_in_vscode(path: Option<String>) -> bool {
    // No `cmd /C` anywhere near this. The path is a project folder chosen by
    // whoever is using Claude Code, and cmd would happily read `&`, `^` and `%`
    // in a folder name as syntax. Finding the launcher ourselves and handing the
    // path over as a separate argument keeps it a path.
    if let Some(code) = find_on_path("code") {
        let mut cmd = Command::new(code);
        if let Some(p) = path.as_deref().filter(|p| !p.is_empty()) {
            cmd.arg(p);
        }
        cmd.creation_flags(CREATE_NO_WINDOW);
        if cmd.spawn().is_ok() {
            return true;
        }
    }
    if let Some(p) = path.as_deref().filter(|p| !p.is_empty()) {
        let opener = "explorer";
        return Command::new(opener).arg(p).spawn().is_ok();
    }
    false
}

#[cfg(target_os = "linux")]
#[tauri::command]
fn open_in_vscode(path: Option<String>) -> bool {
    let Some(path) = path.filter(|p| std::path::Path::new(p).is_absolute() && std::path::Path::new(p).is_dir()) else {
        return false;
    };
    Command::new("xdg-open").arg(path).spawn().is_ok()
}

/// Our own `where`: walks %PATH% against %PATHEXT%, no shell involved.
/// Rust quotes arguments correctly for `.cmd`/`.bat` targets since 1.77, so
/// spawning `code.cmd` directly is safe.
#[cfg(windows)]
fn find_on_path(stem: &str) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let dirs = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&dirs) {
        #[cfg(windows)]
        for ext in exts.split(';').filter(|e| !e.is_empty()) {
            let candidate = dir.join(format!("{stem}{}", ext.to_lowercase()));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
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

// ── Claude Code hooks ─────────────────────────────────────────────────────────

#[tauri::command]
fn hooks_status() -> HookStatus {
    #[cfg(windows)]
    { hooks::status() }
    #[cfg(target_os = "linux")]
    {
        let status = codex_monitor::status();
        HookStatus {
            installed: status.available,
            settings_path: status.sessions_path,
            hook_path: "Codex session monitor".into(),
            hook_ready: status.available,
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HookStatus {
    installed: bool,
    settings_path: String,
    hook_path: String,
    hook_ready: bool,
}

#[cfg(target_os = "linux")]
#[tauri::command]
fn codex_monitor_ready(app: AppHandle) {
    codex_monitor::start(app);
}

#[cfg(target_os = "linux")]
#[tauri::command]
fn codex_activity() -> codex_monitor::ActivitySnapshot {
    codex_monitor::snapshot()
}

#[cfg(target_os = "linux")]
#[tauri::command]
async fn focus_codex_window(cwd: Option<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || codex_window::focus(cwd.as_deref()))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(target_os = "linux")]
#[tauri::command]
async fn codex_status() -> codex::AccountStatus {
    codex::account_status().await
}

#[cfg(target_os = "linux")]
#[tauri::command]
async fn codex_models(chat: State<'_, Chat>, refresh: bool) -> Result<Vec<codex_models::Model>, String> {
    chat.models.list(refresh).await
}

#[cfg(target_os = "linux")]
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

/// Returns the diff the user has to look at before anything is written.
#[cfg(windows)]
#[tauri::command]
fn hooks_preview(install: bool) -> Result<HookPreview, String> {
    hooks::preview(install)
}

/// Only ever called from an explicit click in the settings window.
#[cfg(windows)]
#[tauri::command]
fn hooks_apply(
    app: AppHandle,
    shared: State<Shared>,
    install: bool,
    fingerprint: String,
) -> Result<String, String> {
    // The fingerprint comes from the preview the user actually looked at, so a
    // settings.json that changed in between is refused rather than overwritten.
    let backup = hooks::write(install, &fingerprint)?;
    let updated = {
        let mut current = shared.settings.lock().unwrap();
        current.hooks_installed = install;
        let _ = settings::save(&current);
        current.clone()
    };
    let _ = app.emit("settings-changed", updated);
    Ok(backup)
}

#[cfg(windows)]
#[tauri::command]
fn approval_decision(app: AppHandle, request_id: String, decision: String) {
    pipe::answer(&app, &request_id, &decision);
}

/// The island has the card on screen, so the long wait for a human may begin.
/// Until this arrives the relay only waits a few hundred milliseconds, which is
/// what stops a paused or unresponsive island from freezing Claude Code.
#[cfg(windows)]
#[tauri::command]
fn approval_ack(app: AppHandle, request_id: String) {
    pipe::acknowledge(&app, &request_id);
}

/// Nobody can act on this request — the island is paused, or another card is
/// already up. Claude Code falls back to asking in the terminal immediately.
#[cfg(windows)]
#[tauri::command]
fn approval_decline(app: AppHandle, request_id: String) {
    pipe::decline(&app, &request_id);
}

// ── Chat, files and secrets ───────────────────────────────────────────────────

/// One chat turn. The API key and any file bytes stay on the Rust side.
#[tauri::command]
async fn chat_send(
    shared: State<'_, Shared>,
    chat: State<'_, Chat>,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let preferences = shared.settings.lock().unwrap().clone();
    #[cfg(windows)]
    { claude::send(&chat, &preferences.model, query, context).await }
    #[cfg(target_os = "linux")]
    { codex::send(&chat, &preferences.model, &preferences.reasoning_effort, query, context).await }
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

/// Opens the configured n8n instance — the URL lives in the Credential Manager.
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

/// WebView2 allows exactly one browser environment per app, and its options are
/// fixed by whichever webview is created first. Every window must therefore ask
/// for the *same* arguments as the island (see `additionalBrowserArgs` in
/// tauri.conf.json) — a mismatch makes the second window come up blank, with no
/// error anywhere.
#[cfg(windows)]
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required";

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
/// hidden afterwards. A WebView2 window created later — on the main thread or
/// not — silently comes up blank in this app, so the window that works is the
/// one that exists before the island's webview does.
fn create_settings_window(app: &AppHandle) {
    let url = settings_page_url(app);
    let builder = WebviewWindowBuilder::new(app, "settings", url);
    #[cfg(windows)]
    let builder = builder.additional_browser_args(BROWSER_ARGS);
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

#[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() && std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }

    let builder = tauri::Builder::default();
    #[cfg(windows)]
    let builder = builder.manage(Pending::default())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None));
    #[cfg(target_os = "linux")]
    let builder = builder.manage(telegram::Telegram::default());
    builder
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if argv.iter().any(|arg| arg == "--settings") {
                show_settings_window(app);
            } else if argv.iter().any(|arg| arg == "--telegram") {
                #[cfg(target_os = "linux")]
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
            open_in_vscode,
            quit_app,
            hooks_status,
            #[cfg(windows)]
            hooks_preview,
            #[cfg(windows)]
            hooks_apply,
            #[cfg(windows)]
            approval_decision,
            #[cfg(windows)]
            approval_ack,
            #[cfg(windows)]
            approval_decline,
            log_line,
            #[cfg(target_os = "linux")]
            codex_status,
            #[cfg(target_os = "linux")]
            codex_models,
            #[cfg(target_os = "linux")]
            codex_set_preferences,
            #[cfg(target_os = "linux")]
            codex_monitor_ready,
            #[cfg(target_os = "linux")]
            codex_activity,
            #[cfg(target_os = "linux")]
            focus_codex_window,
            chat_send,
            chat_reset,
            ingest_file,
            secret_present,
            secret_set,
            secret_clear,
            refresh_integration,
            open_n8n,
            open_settings_window,
            #[cfg(target_os = "linux")]
            open_telegram_window,
            #[cfg(target_os = "linux")]
            telegram::telegram_status,
            #[cfg(target_os = "linux")]
            telegram::telegram_summary,
            #[cfg(target_os = "linux")]
            telegram::telegram_configure,
            #[cfg(target_os = "linux")]
            telegram::telegram_connect,
            #[cfg(target_os = "linux")]
            telegram::telegram_authenticate,
            #[cfg(target_os = "linux")]
            telegram::telegram_chats,
            #[cfg(target_os = "linux")]
            telegram::telegram_history,
            #[cfg(target_os = "linux")]
            telegram::telegram_send,
            #[cfg(target_os = "linux")]
            telegram::telegram_disconnect,
            #[cfg(target_os = "linux")]
            telegram::telegram_logout,
            set_paused,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            // Only the primary instance may reopen the saved Telegram database.
            #[cfg(target_os = "linux")]
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
            #[cfg(target_os = "linux")]
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
            #[cfg(windows)]
            {
                hooks::ensure_hook_exe(&handle);
                pipe::start(handle.clone());
            }
            integrations::start(handle.clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Coucou")
        .run(|app, event| {
            #[cfg(target_os = "linux")]
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                if !app.state::<telegram::Telegram>().shutdown() {
                    log::line("Telegram shutdown exceeded its deadline");
                }
            }
            #[cfg(windows)]
            let _ = (app, event);
        });
}
