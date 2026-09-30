// Linux island window. Keep the native bounds as small as the visible island:
// some Wayland compositors hit-test XWayland windows without their X input mask.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use gtk::cairo::{RectangleInt, Region};
use gtk::gdk::WindowTypeHint;
use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Monitor, PhysicalPosition, WebviewWindow};

pub const PANEL_W: f64 = 720.0;
pub const PANEL_H: f64 = 320.0;
pub const STRIP_W: f64 = 240.0;
pub const STRIP_H: f64 = 6.0;
pub const WINDOW_LABEL: &str = "island";

#[derive(Serialize, Clone)]
pub struct CursorPayload {
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Clone)]
pub struct ScreenInfo {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

#[derive(Clone, Copy, Default)]
pub struct IslandRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

pub struct PollGate {
    active: Mutex<bool>,
    cv: Condvar,
    pub collapsed: AtomicBool,
    pub rect: Mutex<IslandRect>,
    input_dirty: AtomicBool,
    tick_pending: AtomicBool,
    anchor: Mutex<Option<WindowAnchor>>,
}

impl PollGate {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(false),
            cv: Condvar::new(),
            collapsed: AtomicBool::new(true),
            rect: Mutex::new(IslandRect::default()),
            input_dirty: AtomicBool::new(true),
            tick_pending: AtomicBool::new(false),
            anchor: Mutex::new(None),
        }
    }

    pub fn set_rect(&self, rect: IslandRect) {
        *self.rect.lock().unwrap() = rect;
    }

    pub fn forget_ignore_state(&self) {
        self.input_dirty.store(true, Ordering::Relaxed);
    }

    pub fn set_active(&self, on: bool) {
        *self.active.lock().unwrap() = on;
        self.input_dirty.store(true, Ordering::Relaxed);
        self.cv.notify_all();
    }

    fn wait_until_active(&self) {
        let mut guard = self.active.lock().unwrap();
        while !*guard {
            guard = self.cv.wait(guard).unwrap();
        }
    }

    fn is_active(&self) -> bool {
        *self.active.lock().unwrap()
    }
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(WINDOW_LABEL)
}

fn target_monitor(app: &AppHandle, pref: &str) -> Option<Monitor> {
    let monitors = app.available_monitors().ok()?;
    if pref == "cursor" {
        if let Ok(cursor) = app.cursor_position() {
            if let Some(monitor) = monitors.iter().find(|monitor| {
                let p = monitor.position();
                let s = monitor.size();
                cursor.x >= p.x as f64
                    && cursor.x < p.x as f64 + s.width as f64
                    && cursor.y >= p.y as f64
                    && cursor.y < p.y as f64 + s.height as f64
            }) {
                return Some(monitor.clone());
            }
        }
    }
    app.primary_monitor()
        .ok()
        .flatten()
        .or_else(|| monitors.into_iter().next())
}

pub fn screen_info(app: &AppHandle, pref: &str) -> ScreenInfo {
    match target_monitor(app, pref) {
        Some(monitor) => {
            let scale = monitor.scale_factor();
            let p = monitor.position();
            let s = monitor.size();
            ScreenInfo {
                x: p.x as f64 / scale,
                y: p.y as f64 / scale,
                width: s.width as f64 / scale,
                height: s.height as f64 / scale,
                scale,
            }
        }
        None => ScreenInfo {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
            scale: 1.0,
        },
    }
}

pub fn apply_geometry(app: &AppHandle, pref: &str, collapsed: bool) {
    let Some(win) = window(app) else { return };
    let Some(monitor) = target_monitor(app, pref) else {
        return;
    };
    let anchor = WindowAnchor {
        x: monitor.position().x,
        y: hyprland_top_edge(&monitor)
            .unwrap_or_else(|| monitor.work_area().position.y.max(monitor.position().y)),
        width: monitor.size().width,
        scale: monitor.scale_factor(),
    };
    let shared = app.state::<crate::Shared>();
    *shared.gate.anchor.lock().unwrap() = Some(anchor);
    let size = native_size(*shared.gate.rect.lock().unwrap(), collapsed);
    apply_native_geometry(&win, anchor, size);
}

#[derive(Clone, Copy)]
struct WindowAnchor {
    x: i32,
    y: i32,
    width: u32,
    scale: f64,
}

fn native_size(rect: IslandRect, collapsed: bool) -> (i32, i32) {
    if collapsed || !rect.w.is_finite() || !rect.h.is_finite() || rect.w <= 0.0 || rect.h < 0.0 {
        return (STRIP_W as i32, STRIP_H as i32);
    }
    (
        rect.w.ceil().clamp(1.0, PANEL_W) as i32,
        rect.h.ceil().clamp(1.0, PANEL_H) as i32,
    )
}

fn apply_native_geometry(win: &WebviewWindow, anchor: WindowAnchor, size: (i32, i32)) {
    let (width, height) = size;
    let physical_width = (width as f64 * anchor.scale).round().max(1.0) as u32;
    let x = anchor.x + (anchor.width as i32 - physical_width as i32) / 2;
    let handle = win.clone();
    let _ = win.run_on_main_thread(move || {
        if let Ok(gtk) = handle.gtk_window() {
            // A non-resizable GTK window follows its natural size. Explicit
            // logical dimensions also prevent the empty-child 200px fallback.
            gtk.set_size_request(width, height);
            gtk.resize(width, height);
        }
        let _ = handle.set_position(PhysicalPosition::new(x, anchor.y));
    });
}

// Wayland panels are above XWayland windows, and Hyprland does not expose their
// reserved space through X11's _NET_WORKAREA. Query its local IPC only when
// applying geometry, so the invisible wake strip stays just below the panel.
fn hyprland_top_edge(monitor: &Monitor) -> Option<i32> {
    let bytes = hyprland_request("j/monitors")?;
    let monitors = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    let scale = monitor.scale_factor();
    let offset = hyprland_top_offset(
        &monitors,
        monitor.name().map(String::as_str),
        monitor.position().x as f64 / scale,
        monitor.position().y as f64 / scale,
    )?;
    (offset < monitor.size().height as f64 / scale)
        .then(|| monitor.position().y + (offset * scale).round() as i32)
}

fn hyprland_request(command: &str) -> Option<Vec<u8>> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let instance = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let socket = PathBuf::from(runtime)
        .join("hypr")
        .join(instance)
        .join(".socket.sock");
    let mut stream = UnixStream::connect(socket).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(100)))
        .ok()?;
    stream.write_all(command.as_bytes()).ok()?;
    let mut bytes = Vec::new();
    stream.take(128 * 1024).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn hyprland_top_offset(
    monitors: &serde_json::Value,
    name: Option<&str>,
    x: f64,
    y: f64,
) -> Option<f64> {
    let monitors = monitors.as_array()?;
    let monitor = monitors
        .iter()
        .find(|monitor| name.is_some() && monitor["name"].as_str() == name)
        .or_else(|| {
            monitors.iter().find(|monitor| {
                monitor["x"].as_f64() == Some(x) && monitor["y"].as_f64() == Some(y)
            })
        })?;
    let top = monitor["reserved"][1].as_f64()?;
    (top.is_finite() && top >= 0.0).then_some(top)
}

pub fn make_non_activating(win: &WebviewWindow) {
    let handle = win.clone();
    let _ = win.run_on_main_thread(move || {
        if let Ok(gtk) = handle.gtk_window() {
            gtk.set_type_hint(WindowTypeHint::Dock);
            gtk.set_skip_taskbar_hint(true);
            gtk.set_skip_pager_hint(true);
            gtk.set_accept_focus(false);
            gtk.set_focus_on_map(false);
            gtk.set_keep_above(true);
            gtk.stick();
            let app = handle.app_handle().clone();
            gtk.connect_size_allocate(move |gtk, allocation| {
                let Some(native) = gtk.window() else { return };
                let Some(shared) = app.try_state::<crate::Shared>() else {
                    return;
                };
                // Resizing is asynchronous. Reapply after allocation even when
                // the hidden cursor poll has already gone to sleep.
                let shape = input_shape(
                    *shared.gate.rect.lock().unwrap(),
                    allocation.width(),
                    allocation.height(),
                    shared.gate.collapsed.load(Ordering::Relaxed),
                );
                apply_input_shape(&native, shape);
                shared.gate.forget_ignore_state();
            });
            if let Some(native) = gtk.window() {
                apply_input_shape(&native, InputShape(0, 0, 0, 0));
            }
        }
    });
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        let title = win.title().unwrap_or_else(|_| "Coucou".into());
        let app = win.app_handle().clone();
        std::thread::spawn(move || {
            configure_hyprland_window(&title);
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                let Some(shared) = handle.try_state::<crate::Shared>() else {
                    return;
                };
                let pref = shared.settings.lock().unwrap().screen.clone();
                let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
                // Pinning can move a new window onto Hyprland's focused output.
                // Restore the user's chosen monitor after that operation finishes.
                apply_geometry(&handle, &pref, collapsed);
                shared.gate.forget_ignore_state();
            });
        });
    }
}

// Native dock/sticky hints alone do not disable compositor blur or pin an
// XWayland window in Hyprland. Scope runtime properties to this process's island
// address; neither settings windows nor the user's compositor config is touched.
fn configure_hyprland_window(title: &str) {
    for _ in 0..20 {
        if let Some((address, pinned)) = hyprland_client_for_title(title) {
            let selector = format!("address:{address}");
            // Hyprland 0.55+ uses Lua dispatchers. The address contains only hex
            // digits (validated below), so it cannot insert code into this call.
            let lua = format!(
                "/eval local w = '{selector}'; \
                for p,v in pairs({{no_blur='1',no_shadow='1',no_anim='1',border_size='0'}}) do \
                hl.dispatch(hl.dsp.window.set_prop({{window=w,prop=p,value=v}})) end; \
                hl.dispatch(hl.dsp.window.pin({{window=w,action='set'}}))"
            );
            if hyprland_command_ok(&lua) {
                return;
            }
            // Support the older hyprctl protocol as well as current Lua builds.
            for (property, legacy, value) in [
                ("no_blur", "noblur", "1"),
                ("no_shadow", "noshadow", "1"),
                ("no_anim", "noanim", "1"),
                ("border_size", "bordersize", "0"),
            ] {
                if !hyprland_command_ok(&format!("/setprop {selector} {property} {value}")) {
                    let _ = hyprland_command_ok(&format!("/setprop {selector} {legacy} {value}"));
                }
            }
            // The old pin dispatcher toggles, unlike Lua's explicit action=set.
            if !pinned {
                let _ = hyprland_command_ok(&format!("/dispatch pin {selector}"));
            }
            return;
        }
        // Mapping can finish after Tauri's setup callback. Retry briefly in this
        // worker, never on the GTK thread or in the regular cursor poll.
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn hyprland_client_for_title(title: &str) -> Option<(String, bool)> {
    let bytes = hyprland_request("j/clients")?;
    let clients = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    hyprland_island_client(&clients, std::process::id(), title)
}

fn hyprland_command_ok(command: &str) -> bool {
    hyprland_request(command)
        .is_some_and(|response| String::from_utf8_lossy(&response).trim() == "ok")
}

fn hyprland_island_client(
    clients: &serde_json::Value,
    pid: u32,
    title: &str,
) -> Option<(String, bool)> {
    let client = clients.as_array()?.iter().find(|client| {
        client["pid"].as_u64() == Some(u64::from(pid)) && client["title"].as_str() == Some(title)
    })?;
    let address = client["address"].as_str()?;
    let hex = address.strip_prefix("0x")?;
    if hex.is_empty() || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some((
        address.to_string(),
        client["pinned"].as_bool().unwrap_or(false),
    ))
}

pub fn set_activating(win: &WebviewWindow, activating: bool) {
    let handle = win.clone();
    let _ = win.run_on_main_thread(move || {
        if let Ok(gtk) = handle.gtk_window() {
            gtk.set_accept_focus(activating);
            gtk.set_focus_on_map(activating);
            if activating {
                gtk.present();
                if let Some(native) = gtk.window() {
                    native.display().flush();
                }
                // Hyprland ignores a dock's normal activation request even after
                // its GTK focus hint changes. Explicitly request focus only when
                // the user opens chat or a text field, using this island's address.
                let title = gtk
                    .title()
                    .map(|title| title.to_string())
                    .unwrap_or_default();
                if let Some((address, _)) = hyprland_client_for_title(&title) {
                    let selector = format!("address:{address}");
                    if !hyprland_command_ok(&format!(
                        "/eval hl.dispatch(hl.dsp.focus({{window='{selector}'}}))"
                    )) {
                        let _ = hyprland_command_ok(&format!("/dispatch focuswindow {selector}"));
                    }
                }
                if let Some(native) = gtk.window() {
                    focus_x11_input(&native);
                }
            }
        }
    });
}

fn focus_x11_input(native: &gtk::gdk::Window) {
    let display = native.display();
    let Some(display) = display.downcast_ref::<gdkx11::X11Display>() else {
        return;
    };
    let Some(window) = native.downcast_ref::<gdkx11::X11Window>() else {
        return;
    };
    if !native.is_viewable() {
        return;
    }
    let Ok(xlib) = x11_dl::xlib::Xlib::open() else {
        return;
    };
    // Hyprland can mark a dock active while XWayland's keyboard focus is None.
    // Match XSetInputFocus's normal client activation, only after an explicit
    // request to type. These GTK-owned handles stay alive on the GTK main thread.
    let raw = unsafe { gdkx11::ffi::gdk_x11_display_get_xdisplay(display.to_glib_none().0) };
    if raw.is_null() {
        return;
    }
    display.error_trap_push();
    unsafe {
        (xlib.XSetInputFocus)(
            raw.cast(),
            window.xid(),
            x11_dl::xlib::RevertToParent,
            x11_dl::xlib::CurrentTime,
        );
        (xlib.XFlush)(raw.cast());
    }
    // A window can be unmapped while X11 handles the request. Ignore that race
    // through GDK's error trap instead of allowing the Xlib handler to abort.
    display.error_trap_pop_ignored();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InputShape(i32, i32, i32, i32);

// GDK coordinates and Cairo regions both use GTK logical pixels, including on
// scaled displays. Only the visible island takes input; hover tolerance is
// handled separately by the frontend's global cursor events.
fn input_shape(rect: IslandRect, width: i32, height: i32, collapsed: bool) -> InputShape {
    let width = width.max(0);
    let height = height.max(0);
    if collapsed {
        // GTK/compositors may allocate more than the requested six pixels.
        // Polling parks while hidden, so the wake region must remain bounded.
        let strip_width = width.min(STRIP_W as i32);
        return InputShape(
            (width - strip_width) / 2,
            0,
            strip_width,
            height.min(STRIP_H as i32),
        );
    }
    if ![rect.x, rect.y, rect.w, rect.h]
        .iter()
        .all(|value| value.is_finite())
        || rect.w <= 0.0
        || rect.h <= 0.0
    {
        return InputShape(0, 0, 0, 0);
    }
    let local_x = rect.x - (PANEL_W - width as f64) / 2.0;
    let x = local_x.floor().max(0.0).min(width as f64) as i32;
    let y = rect.y.floor().max(0.0).min(height as f64) as i32;
    let right = (local_x + rect.w).ceil().max(x as f64).min(width as f64) as i32;
    let bottom = (rect.y + rect.h).ceil().max(y as f64).min(height as f64) as i32;
    InputShape(x, y, right - x, bottom - y)
}

fn apply_input_shape(window: &gtk::gdk::Window, shape: InputShape) {
    let InputShape(x, y, width, height) = shape;
    let region = Region::create_rectangle(&RectangleInt::new(x, y, width, height));
    window.input_shape_combine_region(&region, 0, 0);
}

fn current_input_shape(gate: &PollGate, native: &gtk::gdk::Window) -> InputShape {
    input_shape(
        *gate.rect.lock().unwrap(),
        native.width(),
        native.height(),
        gate.collapsed.load(Ordering::Relaxed),
    )
}

type ScreenKey = (i32, i32, u32, u32, u64);

#[derive(Default)]
struct PollState {
    cursor: Option<(f64, f64)>,
    shape: Option<InputShape>,
    screen: Option<ScreenKey>,
    ticks: u32,
}

fn poll_tick(app: &AppHandle, gate: &PollGate, state: &Mutex<PollState>) {
    if !gate.is_active() {
        return;
    }
    let Some(win) = window(app) else { return };
    let Ok(gtk) = win.gtk_window() else { return };
    let Some(native) = gtk.window() else { return };
    let rect = *gate.rect.lock().unwrap();
    let size = native_size(rect, gate.collapsed.load(Ordering::Relaxed));
    if (native.width(), native.height()) != size {
        // Coalesce frontend animation updates in the existing 60Hz poll. The
        // monitor anchor is cached; resizing does not query compositor IPC.
        if let Some(anchor) = *gate.anchor.lock().unwrap() {
            apply_native_geometry(&win, anchor, size);
        }
    }
    let Some(pointer) = native
        .display()
        .default_seat()
        .and_then(|seat| seat.pointer())
    else {
        return;
    };
    let (_, local_x, y, _) = native.device_position_double(&pointer);
    // The frontend keeps its original 720px virtual canvas coordinates while
    // GTK's viewport now follows the visible island's width.
    let x = local_x + (PANEL_W - native.width() as f64) / 2.0;
    // A button held over another application must never enlarge our region.
    // GTK already delivers native drag/drop events on the island itself.
    let shape = current_input_shape(gate, &native);
    let mut state = state.lock().unwrap();
    if gate.input_dirty.swap(false, Ordering::Relaxed) || state.shape != Some(shape) {
        apply_input_shape(&native, shape);
        state.shape = Some(shape);
    }
    if state.cursor.map_or(true, |last| {
        (x - last.0).abs() >= 1.0 || (y - last.1).abs() >= 1.0
    }) {
        state.cursor = Some((x, y));
        let _ = win.emit("cursor", CursorPayload { x, y });
    }

    state.ticks = state.ticks.wrapping_add(1);
    if state.ticks % 30 == 0 {
        let pref = app
            .state::<crate::Shared>()
            .settings
            .lock()
            .unwrap()
            .screen
            .clone();
        if let Some(monitor) = target_monitor(app, &pref) {
            let p = monitor.position();
            let s = monitor.size();
            let key = (
                p.x,
                p.y,
                s.width,
                s.height,
                monitor.scale_factor().to_bits(),
            );
            if state.screen.is_some() && state.screen != Some(key) {
                let _ = win.emit("screen-changed", ());
            }
            state.screen = Some(key);
        }
    }
}

pub fn spawn_cursor_poll(app: AppHandle, gate: Arc<PollGate>) {
    std::thread::spawn(move || {
        let state = Arc::new(Mutex::new(PollState::default()));
        loop {
            gate.wait_until_active();
            while gate.is_active() {
                // GTK must be accessed on its main thread. Keep at most one tick
                // queued if the UI is busy, and park entirely while collapsed.
                if !gate.tick_pending.swap(true, Ordering::Relaxed) {
                    let handle = app.clone();
                    let tick_gate = gate.clone();
                    let tick_state = state.clone();
                    if app
                        .run_on_main_thread(move || {
                            poll_tick(&handle, &tick_gate, &tick_state);
                            tick_gate.tick_pending.store(false, Ordering::Relaxed);
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(16));
            }
        }
    });
}

pub fn set_ignore_cursor(app: &AppHandle, ignore: bool) {
    let Some(win) = window(app) else { return };
    let handle = win.clone();
    let _ = win.run_on_main_thread(move || {
        if let Some(native) = handle.gtk_window().ok().and_then(|gtk| gtk.window()) {
            let shared = handle.state::<crate::Shared>();
            apply_input_shape(
                &native,
                if ignore {
                    InputShape(0, 0, 0, 0)
                } else {
                    current_input_shape(&shared.gate, &native)
                },
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_bounds_follow_compact_and_expanded_content() {
        for (w, h) in [(288.0, 32.0), (640.0, 160.0), (640.0, 300.0)] {
            let rect = IslandRect {
                x: (PANEL_W - w) / 2.0,
                y: 0.0,
                w,
                h,
            };
            assert_eq!(native_size(rect, false), (w as i32, h as i32));
            assert_eq!(
                input_shape(rect, w as i32, h as i32, false),
                InputShape(0, 0, w as i32, h as i32)
            );
            assert_eq!(native_size(rect, true), (240, 6));
        }
    }

    #[test]
    fn native_bounds_round_animation_frames_without_extra_panel_space() {
        assert_eq!(
            native_size(
                IslandRect {
                    w: 302.2,
                    h: 48.1,
                    ..IslandRect::default()
                },
                false
            ),
            (303, 49)
        );
        assert_eq!(
            native_size(
                IslandRect {
                    w: 184.0,
                    h: 0.0,
                    ..IslandRect::default()
                },
                false
            ),
            (184, 1)
        );
        assert_eq!(
            native_size(
                IslandRect {
                    w: f64::NAN,
                    h: 32.0,
                    ..IslandRect::default()
                },
                false
            ),
            (240, 6)
        );
        assert_eq!(
            native_size(
                IslandRect {
                    w: 900.0,
                    h: 600.0,
                    ..IslandRect::default()
                },
                false
            ),
            (720, 320)
        );
    }

    #[test]
    fn hyprland_properties_target_only_our_island() {
        let clients = serde_json::json!([
            { "pid": 9, "title": "Coucou", "address": "0x123", "pinned": false },
            { "pid": 42, "title": "Settings — Coucou", "address": "0x456", "pinned": false },
            { "pid": 42, "title": "Coucou", "address": "0xabc", "pinned": true }
        ]);
        assert_eq!(
            hyprland_island_client(&clients, 42, "Coucou"),
            Some(("0xabc".into(), true))
        );
        assert_eq!(hyprland_island_client(&clients, 50, "Coucou"), None);
        let invalid = serde_json::json!([
            { "pid": 42, "title": "Coucou", "address": "0xabc'; command()" }
        ]);
        assert_eq!(hyprland_island_client(&invalid, 42, "Coucou"), None);
    }

    #[test]
    fn hyprland_reserved_top_is_selected_per_monitor() {
        let monitors = serde_json::json!([
            { "name": "eDP-1", "x": 0, "y": 0, "reserved": [0, 37, 0, 0] },
            { "name": "DP-1", "x": 1920, "y": 0, "reserved": [0, 24, 0, 0] }
        ]);
        assert_eq!(
            hyprland_top_offset(&monitors, Some("eDP-1"), 0.0, 0.0),
            Some(37.0)
        );
        assert_eq!(
            hyprland_top_offset(&monitors, None, 1920.0, 0.0),
            Some(24.0)
        );
        assert_eq!(hyprland_top_offset(&monitors, None, 9000.0, 0.0), None);
    }

    #[test]
    fn input_region_tracks_visible_island_and_clips_to_window() {
        let shape = input_shape(
            IslandRect {
                x: 160.0,
                y: 0.0,
                w: 400.0,
                h: 200.0,
            },
            720,
            320,
            false,
        );
        assert_eq!(shape, InputShape(160, 0, 400, 200));
        let shape = input_shape(
            IslandRect {
                x: -20.0,
                y: -10.0,
                w: 800.0,
                h: 500.0,
            },
            720,
            320,
            false,
        );
        assert_eq!(shape, InputShape(0, 0, 720, 320));
    }

    #[test]
    fn hidden_input_region_stays_six_pixels_high_in_larger_native_window() {
        assert_eq!(
            input_shape(IslandRect::default(), 240, 200, true),
            InputShape(0, 0, 240, 6)
        );
        assert_eq!(
            input_shape(
                IslandRect {
                    x: 40.0,
                    y: 0.0,
                    w: 640.0,
                    h: 300.0
                },
                720,
                320,
                true
            ),
            InputShape(240, 0, 240, 6)
        );
        assert_eq!(
            input_shape(IslandRect::default(), 120, 4, true),
            InputShape(0, 0, 120, 4)
        );
    }

    #[test]
    fn input_region_rejects_invalid_geometry() {
        assert!(matches!(
            input_shape(IslandRect::default(), 720, 320, false),
            InputShape(0, 0, 0, 0)
        ));
        assert!(matches!(
            input_shape(
                IslandRect {
                    x: f64::NAN,
                    ..IslandRect::default()
                },
                720,
                320,
                false
            ),
            InputShape(0, 0, 0, 0)
        ));
    }
}
