use crate::{
    commands as domain,
    core::AppCore,
    model::{AppSettings, CodexPreferences, DashboardData},
};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tokio::runtime::Builder;

const APP_TITLE: &str = "Codex Accounts";
const TRAY_MENU_WIDTH: f64 = 306.0;
const TRAY_MENU_HEIGHT: f64 = 248.0;
const TRAY_MENU_MIN_VISIBLE: Duration = Duration::from_millis(420);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrayMenuAnchor {
    side: String,
    offset_percent: f64,
    request_id: u64,
    generation: u64,
}

/// Describes why the main window became visible.  Keeping this as a small
/// event payload lets the WebView choose the matching visual origin without
/// guessing whether the restore came from the tray or from a second launch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppRestoredEvent {
    source: String,
}

impl Default for TrayMenuAnchor {
    fn default() -> Self {
        Self {
            side: "bottom".to_string(),
            offset_percent: 88.0,
            request_id: 0,
            generation: 0,
        }
    }
}

#[derive(Default)]
struct TrayMenuState {
    anchor: Mutex<TrayMenuAnchor>,
    request_sequence: AtomicU64,
    open_generation: AtomicU64,
    presented_at: Mutex<Option<Instant>>,
}

impl TrayMenuState {
    fn begin_open(&self, mut anchor: TrayMenuAnchor) -> TrayMenuAnchor {
        let mut current = self
            .anchor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        anchor.request_id = self.request_sequence.fetch_add(1, Ordering::AcqRel) + 1;
        anchor.generation = self.open_generation.fetch_add(1, Ordering::AcqRel) + 1;
        *current = anchor.clone();
        *self
            .presented_at
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        anchor
    }

    fn current(&self) -> TrayMenuAnchor {
        self.anchor
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn present_if_current(
        &self,
        request_id: u64,
        present: impl FnOnce() -> Result<(), String>,
    ) -> Result<Option<TrayMenuAnchor>, String> {
        let mut anchor = self
            .anchor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if request_id != 0 && anchor.request_id != request_id {
            return Ok(None);
        }
        present()?;
        anchor.generation = self.open_generation.fetch_add(1, Ordering::AcqRel) + 1;
        *self
            .presented_at
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Instant::now());
        Ok(Some(anchor.clone()))
    }

    fn hide_if_current(
        &self,
        generation: u64,
        hide: impl FnOnce() -> Result<(), String>,
    ) -> Result<bool, String> {
        let anchor = self
            .anchor
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if generation != 0 && anchor.generation != generation {
            return Ok(false);
        }
        hide()?;
        Ok(true)
    }

    fn generation(&self) -> u64 {
        self.open_generation.load(Ordering::Acquire)
    }

    fn blur_grace_remaining(&self, generation: u64) -> Duration {
        if self.generation() != generation {
            return Duration::ZERO;
        }
        self.presented_at
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .map(|presented_at| TRAY_MENU_MIN_VISIBLE.saturating_sub(presented_at.elapsed()))
            .unwrap_or_default()
    }
}

pub fn run() -> Result<(), String> {
    // Keep all long-lived work behind Tauri's single-instance gate. Creating
    // the runtime and starting the local proxy before the plugin has rejected
    // a duplicate launch lets every extra process briefly bind ports and poll
    // account usage.
    let lifecycle = Arc::new(std::sync::Mutex::new(
        None::<(Arc<AppCore>, tokio::runtime::Runtime)>,
    ));
    let setup_lifecycle = lifecycle.clone();

    let result = tauri::Builder::default()
        // Register this before setup; setup is where the runtime, proxy, and
        // refresh loop are created.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            reveal_window(app, "duplicate");
            let _ = app.emit("duplicate-launch", ());
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let runtime = Builder::new_multi_thread()
                .enable_all()
                .thread_name("codex-accounts")
                .build()
                .map_err(std::io::Error::other)?;
            let state = super::initialize(&runtime).map_err(std::io::Error::other)?;
            app.manage(state.clone());
            app.manage(TrayMenuState::default());
            let emitter = app.handle().clone();
            state.set_change_notifier(Arc::new(move || {
                let _ = emitter.emit("dashboard-changed", ());
            }));

            let (start_hidden, sound_enabled) = runtime.block_on(async {
                let vault = state.vault.read().await;
                (
                    vault.settings.start_with_windows && vault.settings.start_in_tray,
                    vault.settings.sound_enabled,
                )
            });
            crate::sound::play("startup", sound_enabled);
            let start_hidden =
                start_hidden && std::env::args().any(|argument| argument == "--background");

            let tray_ready = match install_tray(app) {
                Ok(()) => true,
                Err(error) => {
                    let _ = app.emit("app-message", error);
                    false
                }
            };
            if let Some(window) = app.get_webview_window("main") {
                disable_browser_context_menu(&window);
                if start_hidden && tray_ready {
                    let _ = window.hide();
                } else if let Err(error) = window.show() {
                    let _ = app.emit(
                        "app-message",
                        format!("Не удалось показать окно {APP_TITLE}: {error}"),
                    );
                }
            }

            *setup_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some((state, runtime));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_dashboard,
            prepare_auth_link,
            start_add_account,
            start_manual_add_account,
            start_private_add_account,
            cancel_add_account,
            complete_add_account_callback,
            open_auth_link_private,
            switch_account,
            remove_account,
            refresh_all,
            refresh_account,
            save_settings,
            unlock_admin,
            configure_admin_password,
            reset_settings_to_default,
            reset_application,
            connect_codex,
            disconnect_codex,
            save_codex_preferences,
            save_prompt,
            delete_prompt,
            export_backup,
            import_backup,
            hide_to_tray,
            hide_tray_menu,
            present_tray_menu,
            get_tray_menu_anchor,
            show_main_window,
            exit_application,
            play_lifecycle_sound,
            tray_exit_application
        ])
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.app_handle().emit("request-exit", ());
                }
            } else if window.label() == "tray-menu" && matches!(event, WindowEvent::Focused(false))
            {
                // Ignore delayed blur events from the previous opening when a
                // user reopens the popup before Windows finishes the focus cycle.
                let tray_window = window.clone();
                let app = window.app_handle().clone();
                let tray_state = app.state::<TrayMenuState>();
                let generation = tray_state.generation();
                let grace = tray_state.blur_grace_remaining(generation);
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(180).max(grace)).await;
                    let state = app.state::<TrayMenuState>();
                    let _ = state.hide_if_current(generation, || {
                        if tray_window.is_focused().unwrap_or(false) {
                            return Ok(());
                        }
                        tray_window
                            .hide()
                            .map_err(|error| format!("Не удалось скрыть меню трея: {error}"))
                    });
                });
            }
        })
        .run(tauri::generate_context!())
        .map_err(|error| format!("Не удалось открыть окно приложения: {error}"));

    if let Some((core, runtime)) = lifecycle
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    {
        core.request_shutdown();
        runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    }
    result
}

#[cfg(windows)]
fn disable_browser_context_menu(window: &tauri::WebviewWindow) {
    // WebView2 owns a native browser context menu unless this setting is
    // disabled on the underlying settings object.  The DOM guard in the UI
    // handles synthetic events; this native setting closes the race before
    // the browser menu can be painted.
    let _ = window.with_webview(|webview| unsafe {
        if let Ok(core_webview) = webview.controller().CoreWebView2() {
            if let Ok(settings) = core_webview.Settings() {
                let _ = settings.SetAreDefaultContextMenusEnabled(false);
            }
        }
    });
}

#[cfg(not(windows))]
fn disable_browser_context_menu(_window: &tauri::WebviewWindow) {}

fn install_tray(app: &tauri::App) -> Result<(), String> {
    // Embed the same generated 32px PNG that Windows uses for the taskbar.
    // Reading it directly avoids a stale RC-resource/icon-cache layer making
    // the tray choose an older, differently padded artwork after an update.
    // `build.rs` watches this file, and it is regenerated from the SVG master.
    #[cfg(windows)]
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))
        .ok()
        .or_else(|| tauri::image::Image::from_app_icon_resource(32).ok())
        .or_else(|| app.default_window_icon().cloned())
        .unwrap_or_else(|| tauri::image::Image::new_owned(tray_icon_rgba(), 48, 48));
    #[cfg(not(windows))]
    let icon = app
        .default_window_icon()
        .cloned()
        .unwrap_or_else(|| tauri::image::Image::new_owned(tray_icon_rgba(), 48, 48));

    TrayIconBuilder::new()
        .tooltip("Codex Accounts · локальный менеджер")
        .icon(icon)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button,
                button_state: MouseButtonState::Up,
                position,
                rect,
                ..
            } = event
            {
                // `position` is the mouse cursor location and can differ by
                // several pixels when the user clicks an edge of the tray
                // glyph.  Anchor the bubble to the actual icon rectangle so
                // repeated opens stay in the same place and the arrow always
                // points at this icon.  Some Linux backends do not provide a
                // rectangle, so keep the event position as a fallback.
                let rect_position = rect.position.to_physical::<f64>(1.0);
                let rect_size = rect.size.to_physical::<f64>(1.0);
                let icon_position = if rect_size.width > 0.0 && rect_size.height > 0.0 {
                    tauri::PhysicalPosition::new(
                        rect_position.x + rect_size.width / 2.0,
                        rect_position.y + rect_size.height / 2.0,
                    )
                } else {
                    position
                };
                if button == MouseButton::Left {
                    reveal_window(tray.app_handle(), "tray");
                } else if button == MouseButton::Right {
                    show_tray_menu(tray.app_handle(), icon_position.x, icon_position.y);
                }
            }
        })
        .build(app)
        .map_err(|error| format!("Не удалось создать значок системного трея: {error}"))?;
    Ok(())
}

fn show_tray_menu(app: &AppHandle, x: f64, y: f64) {
    if let Some(window) = app.get_webview_window("tray-menu") {
        disable_browser_context_menu(&window);
        suppress_popup_chrome(&window);
        // Never expose the previous WebView frame while React loads the
        // current theme and settings.  The popup is shown again by
        // `present_tray_menu` after the anchor event has been applied.
        let _ = window.hide();
        let state = app.state::<TrayMenuState>();
        let anchor = state.begin_open(position_tray_menu(&window, x, y));
        // React owns presentation: it updates the anchor, clears any close
        // state, then asks the native window to show after the view is ready.
        queue_tray_menu_anchor(app, anchor);
        return;
    }
    let builder = WebviewWindowBuilder::new(app, "tray-menu", WebviewUrl::App("index.html".into()))
        .title("Codex Accounts")
        .inner_size(TRAY_MENU_WIDTH, TRAY_MENU_HEIGHT)
        .min_inner_size(TRAY_MENU_WIDTH, TRAY_MENU_HEIGHT)
        .position(0.0, 0.0)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .always_on_top(true)
        .shadow(false)
        .skip_taskbar(true)
        .visible(false)
        // Use the webview label as an explicit route marker so URL normalization
        // cannot accidentally mount the full dashboard in this compact window.
        .initialization_script("window.__CODEX_TRAY_MENU__ = true;");
    match builder.build() {
        Ok(window) => {
            disable_browser_context_menu(&window);
            suppress_popup_chrome(&window);
            let state = app.state::<TrayMenuState>();
            // Do not emit the first anchor here: the newly-created WebView has
            // not mounted React's listener yet, so that event can be lost and
            // leave the first tray popup blank. Keep the request in shared
            // state; TrayMenu's ready effect calls `present_tray_menu` after
            // its listener is attached and receives this exact anchor.
            state.begin_open(position_tray_menu(&window, x, y));
        }
        Err(error) => {
            let _ = app.emit(
                "app-message",
                format!("Не удалось открыть меню трея: {error}"),
            );
        }
    }
}

fn queue_tray_menu_anchor(app: &AppHandle, anchor: TrayMenuAnchor) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // A hidden WebView2 can take a few frames to wake. Re-send the same
        // request briefly, but stop as soon as a newer tray click supersedes
        // it. The React listener de-duplicates request ids.
        for delay in [0_u64, 45, 120, 240] {
            if delay != 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            if app.state::<TrayMenuState>().current().request_id != anchor.request_id {
                return;
            }
            let _ = app.emit_to("tray-menu", "tray-menu-anchor", anchor.clone());
        }
    });
}

#[cfg(windows)]
fn suppress_popup_chrome(window: &tauri::WebviewWindow) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };
    if let Ok(handle) = window.hwnd() {
        let hwnd = handle.0;
        let border = DWMWA_COLOR_NONE;
        let corners = DWMWCP_DONOTROUND;
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR as u32,
                &border as *const _ as *const std::ffi::c_void,
                std::mem::size_of_val(&border) as u32,
            );
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &corners as *const _ as *const std::ffi::c_void,
                std::mem::size_of_val(&corners) as u32,
            );
        }
    }
}

#[cfg(not(windows))]
fn suppress_popup_chrome(_window: &tauri::WebviewWindow) {}

fn position_tray_menu(window: &tauri::WebviewWindow, x: f64, y: f64) -> TrayMenuAnchor {
    let Ok(monitors) = window.available_monitors() else {
        return TrayMenuAnchor::default();
    };
    let monitor = monitors
        .iter()
        .find(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            x >= position.x as f64
                && x < (position.x + size.width as i32) as f64
                && y >= position.y as f64
                && y < (position.y + size.height as i32) as f64
        })
        .or_else(|| monitors.first());
    let Some(monitor) = monitor else {
        return TrayMenuAnchor::default();
    };

    let scale_factor = monitor.scale_factor();
    let popup_width = (TRAY_MENU_WIDTH * scale_factor).round() as i32;
    let popup_height = (TRAY_MENU_HEIGHT * scale_factor).round() as i32;
    let work_area = monitor.work_area();
    let placement = calculate_tray_menu_placement(
        x,
        y,
        TrayMenuBounds {
            left: work_area.position.x,
            top: work_area.position.y,
            width: work_area.size.width as i32,
            height: work_area.size.height as i32,
        },
        popup_width,
        popup_height,
        (8.0 * scale_factor).round() as i32,
    );
    let _ = window.set_position(tauri::PhysicalPosition::new(placement.left, placement.top));
    placement.anchor
}

#[derive(Debug, Clone, Copy)]
struct TrayMenuBounds {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

struct TrayMenuPlacement {
    left: i32,
    top: i32,
    anchor: TrayMenuAnchor,
}

fn calculate_tray_menu_placement(
    icon_x: f64,
    icon_y: f64,
    work_area: TrayMenuBounds,
    popup_width: i32,
    popup_height: i32,
    gap: i32,
) -> TrayMenuPlacement {
    let min_x = work_area.left;
    let min_y = work_area.top;
    let max_x = (min_x + work_area.width - popup_width).max(min_x);
    let max_y = (min_y + work_area.height - popup_height).max(min_y);
    let icon_x = icon_x.round() as i32;
    let icon_y = icon_y.round() as i32;
    let work_right = min_x + work_area.width;
    let work_bottom = min_y + work_area.height;

    let horizontal_position = |side: &str, top: i32| {
        let left = (icon_x - popup_width / 2).clamp(min_x, max_x);
        let offset_percent = (icon_x - left) as f64 / popup_width.max(1) as f64 * 100.0;
        (
            left,
            top.clamp(min_y, max_y),
            TrayMenuAnchor {
                side: side.to_string(),
                offset_percent: offset_percent.clamp(8.0, 92.0),
                ..TrayMenuAnchor::default()
            },
        )
    };
    let vertical_position = |side: &str, left: i32| {
        let top = (icon_y - popup_height / 2).clamp(min_y, max_y);
        let offset_percent = (icon_y - top) as f64 / popup_height.max(1) as f64 * 100.0;
        (
            left.clamp(min_x, max_x),
            top,
            TrayMenuAnchor {
                side: side.to_string(),
                offset_percent: offset_percent.clamp(8.0, 92.0),
                ..TrayMenuAnchor::default()
            },
        )
    };

    // Taskbar icons sit just outside the monitor work area. Use that edge to
    // keep the whole popup above/below the taskbar instead of under it.
    let (left, top, anchor) = if icon_y < min_y {
        horizontal_position("top", icon_y + gap)
    } else if icon_y >= work_bottom {
        horizontal_position("bottom", icon_y + gap - popup_height)
    } else if icon_x < min_x {
        vertical_position("left", icon_x + gap)
    } else if icon_x >= work_right {
        vertical_position("right", icon_x - gap - popup_width)
    } else {
        let space_above = icon_y - min_y;
        let space_below = work_bottom - icon_y;
        if space_above >= space_below {
            horizontal_position("bottom", icon_y + gap - popup_height)
        } else {
            horizontal_position("top", icon_y + gap)
        }
    };

    TrayMenuPlacement { left, top, anchor }
}

fn reveal_window(app: &AppHandle, source: &str) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        let show_result = window.show();
        let focus_result = window.set_focus();

        // A window hidden with `Window::hide` can remain absent from the taskbar
        // until the next native activation.  The single-instance callback runs
        // on the Win32 message thread, so use the native handle as a fallback
        // for tray restores and duplicate launches.  Tauri remains the primary
        // path; this only repairs a failed/ignored WebView activation.
        #[cfg(windows)]
        if let Ok(hwnd) = window.hwnd() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                BringWindowToTop, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
            };
            let hwnd = hwnd.0;
            unsafe {
                ShowWindow(
                    hwnd,
                    if IsIconic(hwnd) != 0 {
                        SW_RESTORE
                    } else {
                        SW_SHOW
                    },
                );
                BringWindowToTop(hwnd);
                SetForegroundWindow(hwnd);
            }
        }
        if show_result.is_err() || focus_result.is_err() {
            let _ = app.emit(
                "app-message",
                "Окно было скрыто, но Windows не подтвердил восстановление. Откройте его ещё раз из трея.",
            );
        }
        let _ = app.emit(
            "app-restored",
            AppRestoredEvent {
                source: source.to_string(),
            },
        );
    }
}

fn tray_icon_rgba() -> Vec<u8> {
    let mut rgba = Vec::with_capacity(48 * 48 * 4);
    for y in 0..48_i32 {
        for x in 0..48_i32 {
            let dx = (x - 24).abs() as f32;
            let dy = (y - 24).abs() as f32;
            let qx = dx - 15.0;
            let qy = dy - 15.0;
            let rounded_rect_distance = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - 8.0;
            let inside = rounded_rect_distance <= 0.0;
            let progress = (y as f32 / 47.0).clamp(0.0, 1.0);
            let top = [218.0, 132.0, 221.0];
            let bottom = [105.0, 58.0, 148.0];
            let mut pixel = if inside {
                [
                    (top[0] + (bottom[0] - top[0]) * progress) as u8,
                    (top[1] + (bottom[1] - top[1]) * progress) as u8,
                    (top[2] + (bottom[2] - top[2]) * progress) as u8,
                    255,
                ]
            } else {
                [0, 0, 0, 0]
            };
            if inside && rounded_rect_distance >= -1.2 {
                pixel = [255, 218, 249, 255];
            }
            let mark_radius = (dx * dx + dy * dy).sqrt();
            let horizontal = dy <= 1.05 && dx <= 13.0;
            let vertical = dx <= 1.05 && dy <= 13.0;
            let diagonal_a = (dx - dy).abs() <= 1.05 && mark_radius <= 13.0;
            let diagonal_b = (dx + dy - 48.0).abs() <= 1.05 && mark_radius <= 13.0;
            if inside && (horizontal || vertical || diagonal_a || diagonal_b) {
                pixel = [255, 245, 255, 255];
            }
            if inside && mark_radius <= 2.2 {
                pixel = [255, 249, 255, 255];
            }
            let bead_x = x - 35;
            let bead_y = y - 35;
            if inside && bead_x * bead_x + bead_y * bead_y <= 11 {
                pixel = [255, 239, 255, 255];
            }
            rgba.extend_from_slice(&pixel);
        }
    }
    rgba
}

#[tauri::command]
async fn load_dashboard(core: State<'_, Arc<AppCore>>) -> Result<DashboardData, String> {
    domain::load_dashboard(core.inner()).await
}

/// Start (or reuse) a direct PKCE OAuth session and return its unique link.
/// Copying the link never launches a browser; the callback is completed after
/// the user pastes the browser's localhost return address into the app.
#[tauri::command]
fn prepare_auth_link(core: State<'_, Arc<AppCore>>) -> Result<String, String> {
    core.require_admin()?;
    if !core.begin_add_account() {
        return Err(
            "Вход уже открыт. Завершите текущую авторизацию или отмените её, затем повторите."
                .to_string(),
        );
    }
    match crate::codex::begin_manual_auth_session() {
        Ok(url) => Ok(url),
        Err(error) => {
            core.finish_add_account();
            Err(error)
        }
    }
}

#[tauri::command]
async fn start_add_account(app: AppHandle, core: State<'_, Arc<AppCore>>) -> Result<(), String> {
    spawn_account_add_flow(
        app,
        core.inner().clone(),
        crate::codex::LoginMode::Browser,
        false,
    )
}

#[tauri::command]
async fn start_manual_add_account(
    app: AppHandle,
    core: State<'_, Arc<AppCore>>,
) -> Result<(), String> {
    spawn_account_add_flow(
        app,
        core.inner().clone(),
        // Manual copy uses the device-code flow.  It never invokes a browser;
        // the CLI emits the official verification URL and one-time code,
        // which the UI can keep visible while copying the URL to the clipboard.
        crate::codex::LoginMode::DeviceCode,
        false,
    )
}

#[tauri::command]
async fn start_private_add_account(
    app: AppHandle,
    core: State<'_, Arc<AppCore>>,
) -> Result<(), String> {
    spawn_account_add_flow(
        app,
        core.inner().clone(),
        crate::codex::LoginMode::DeviceCode,
        true,
    )
}

fn spawn_account_add_flow(
    app: AppHandle,
    core: Arc<AppCore>,
    login_mode: crate::codex::LoginMode,
    open_device_auth: bool,
) -> Result<(), String> {
    core.require_admin()?;
    if !core.cli.is_file() {
        return Err("Не найден официальный Codex CLI. Установите Codex или задайте путь через CODEX_CLI_PATH, затем перезапустите приложение.".to_string());
    }
    if !core.begin_add_account() {
        return Err(
            "Вход уже открыт в браузере. Завершите его или отмените текущую попытку.".to_string(),
        );
    }
    let started_status = match login_mode {
        crate::codex::LoginMode::Browser => "Открываем официальный вход в браузере…",
        crate::codex::LoginMode::DeviceCode if open_device_auth => {
            "Готовим одноразовый вход для приватного окна…"
        }
        crate::codex::LoginMode::DeviceCode => "Готовим ссылку для ручного входа…",
    };
    let _ = app.emit("account-add-started", started_status);
    let event_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let auth_event_app = event_app.clone();
        let result = domain::add_account_with_login_mode(&core, login_mode, move |mut info| {
            if info.device_code
                && open_device_auth
                && login_mode == crate::codex::LoginMode::DeviceCode
            {
                if let Some(url) = info.url.as_deref() {
                    info.open_error = crate::codex::open_private_auth_url(url).err();
                }
            }
            let _ = auth_event_app.emit("account-add-auth-info", info);
        })
        .await;
        core.finish_add_account();
        match result {
            Ok(data) => {
                let _ = event_app.emit("account-add-complete", data);
                core.emit_changed();
            }
            Err(error) => {
                let _ = event_app.emit("account-add-error", error);
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn cancel_add_account(core: State<'_, Arc<AppCore>>) -> Result<(), String> {
    core.require_admin()?;
    let manual_session = crate::codex::manual_auth_session_present();
    crate::codex::cancel_manual_auth_session();
    core.cancel_add_account();
    // The legacy browser/device flow owns a live CLI child and will release
    // the slot from its background task.  A direct PKCE session has no child,
    // so release it immediately when the user cancels that path.
    if manual_session {
        core.finish_add_account();
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
async fn complete_add_account_callback(
    app: AppHandle,
    core: State<'_, Arc<AppCore>>,
    callback_url: String,
) -> Result<(), String> {
    let direct = domain::complete_add_account_callback(core.inner(), callback_url).await?;
    if direct {
        let dashboard = domain::load_dashboard(core.inner()).await?;
        let _ = app.emit("account-add-complete", dashboard);
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn open_auth_link_private(core: State<'_, Arc<AppCore>>, url: String) -> Result<(), String> {
    domain::open_auth_link_private(core.inner(), &url)
}

#[tauri::command(rename_all = "camelCase")]
async fn switch_account(
    core: State<'_, Arc<AppCore>>,
    account_id: String,
) -> Result<DashboardData, String> {
    domain::switch_account(core.inner(), account_id).await
}

#[tauri::command(rename_all = "camelCase")]
async fn remove_account(
    core: State<'_, Arc<AppCore>>,
    account_id: String,
) -> Result<DashboardData, String> {
    domain::remove_account(core.inner(), account_id).await
}

#[tauri::command]
async fn refresh_all(core: State<'_, Arc<AppCore>>) -> Result<DashboardData, String> {
    domain::refresh_all(core.inner()).await
}

#[tauri::command(rename_all = "camelCase")]
async fn refresh_account(
    core: State<'_, Arc<AppCore>>,
    account_id: String,
) -> Result<DashboardData, String> {
    domain::refresh_account(core.inner(), account_id).await
}

#[tauri::command]
async fn save_settings(
    core: State<'_, Arc<AppCore>>,
    settings: AppSettings,
) -> Result<DashboardData, String> {
    domain::save_settings(core.inner(), settings).await
}

#[tauri::command]
async fn unlock_admin(
    core: State<'_, Arc<AppCore>>,
    password: String,
) -> Result<DashboardData, String> {
    domain::unlock_admin(core.inner(), password).await
}

#[tauri::command(rename_all = "camelCase")]
async fn configure_admin_password(
    core: State<'_, Arc<AppCore>>,
    current_password: Option<String>,
    new_password: Option<String>,
) -> Result<DashboardData, String> {
    domain::configure_admin_password(core.inner(), current_password, new_password).await
}

#[tauri::command]
async fn reset_settings_to_default(core: State<'_, Arc<AppCore>>) -> Result<DashboardData, String> {
    domain::reset_settings_to_default(core.inner()).await
}

#[tauri::command]
async fn reset_application(core: State<'_, Arc<AppCore>>) -> Result<DashboardData, String> {
    domain::reset_application(core.inner()).await
}

#[tauri::command]
async fn connect_codex(core: State<'_, Arc<AppCore>>) -> Result<DashboardData, String> {
    domain::connect_codex(core.inner()).await
}

#[tauri::command]
async fn disconnect_codex(core: State<'_, Arc<AppCore>>) -> Result<DashboardData, String> {
    domain::disconnect_codex(core.inner()).await
}

#[tauri::command]
async fn save_codex_preferences(
    core: State<'_, Arc<AppCore>>,
    preferences: CodexPreferences,
) -> Result<DashboardData, String> {
    domain::save_codex_preferences(core.inner(), preferences).await
}

#[tauri::command]
async fn save_prompt(
    core: State<'_, Arc<AppCore>>,
    name: String,
    content: String,
) -> Result<DashboardData, String> {
    domain::save_prompt(core.inner(), name, content).await
}

#[tauri::command]
async fn delete_prompt(
    core: State<'_, Arc<AppCore>>,
    name: String,
) -> Result<DashboardData, String> {
    domain::delete_prompt(core.inner(), name).await
}

#[tauri::command(rename_all = "camelCase")]
async fn export_backup(
    core: State<'_, Arc<AppCore>>,
    path: String,
    password: Option<String>,
) -> Result<(), String> {
    domain::export_backup(core.inner(), path, password).await
}

#[tauri::command(rename_all = "camelCase")]
async fn import_backup(
    core: State<'_, Arc<AppCore>>,
    path: String,
    password: Option<String>,
) -> Result<DashboardData, String> {
    domain::import_backup(core.inner(), path, password).await
}

#[tauri::command]
fn hide_to_tray(app: AppHandle, core: State<'_, Arc<AppCore>>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Окно Codex Accounts уже закрыто.".to_string())?;
    window
        .hide()
        .map_err(|error| format!("Не удалось свернуть приложение в трей: {error}"))?;
    let was_protected = core.admin_protected();
    core.lock_admin();
    if was_protected {
        let _ = app.emit("admin-locked", ());
    }
    let _ = app.emit("tray-hidden", ());
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn hide_tray_menu(
    app: AppHandle,
    state: State<'_, TrayMenuState>,
    expected_generation: u64,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("tray-menu") {
        state.hide_if_current(expected_generation, || {
            window
                .hide()
                .map_err(|error| format!("Не удалось закрыть меню трея: {error}"))
        })?;
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn get_tray_menu_anchor(state: State<'_, TrayMenuState>) -> TrayMenuAnchor {
    state.current()
}

#[tauri::command(rename_all = "camelCase")]
fn present_tray_menu(
    app: AppHandle,
    state: State<'_, TrayMenuState>,
    expected_request_id: u64,
) -> Result<TrayMenuAnchor, String> {
    let window = app
        .get_webview_window("tray-menu")
        .ok_or_else(|| "Окно меню трея ещё не создано.".to_string())?;
    let presentation = state.present_if_current(expected_request_id, || {
        suppress_popup_chrome(&window);
        window
            .show()
            .map_err(|error| format!("Не удалось показать меню трея: {error}"))?;
        window
            .set_focus()
            .map_err(|error| format!("Не удалось активировать меню трея: {error}"))?;
        Ok(())
    })?;
    Ok(presentation.unwrap_or_else(|| state.current()))
}

#[tauri::command]
fn show_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("tray-menu") {
        let _ = window.hide();
    }
    reveal_window(&app, "tray");
    Ok(())
}

#[tauri::command]
async fn exit_application(app: AppHandle, core: State<'_, Arc<AppCore>>) -> Result<(), String> {
    domain::shutdown(core.inner()).await?;
    let sound_enabled = core.vault.read().await.settings.sound_enabled;
    crate::sound::play("exit", sound_enabled);
    tokio::time::sleep(std::time::Duration::from_millis(760)).await;
    app.exit(0);
    Ok(())
}

#[tauri::command]
async fn play_lifecycle_sound(core: State<'_, Arc<AppCore>>, name: String) -> Result<(), String> {
    if !matches!(name.as_str(), "restore" | "hide" | "minimize") {
        return Err("Этот системный звук недоступен.".to_string());
    }
    let sound_enabled = core.vault.read().await.settings.sound_enabled;
    crate::sound::play(&name, sound_enabled);
    Ok(())
}

#[tauri::command]
async fn tray_exit_application(
    app: AppHandle,
    core: State<'_, Arc<AppCore>>,
) -> Result<(), String> {
    let sound_enabled = core.vault.read().await.settings.sound_enabled;
    crate::sound::play("exit", sound_enabled);
    domain::shutdown(core.inner()).await?;
    tokio::time::sleep(std::time::Duration::from_millis(760)).await;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tray_menu_tests {
    use super::{calculate_tray_menu_placement, TrayMenuAnchor, TrayMenuBounds, TrayMenuState};

    #[test]
    fn delayed_close_from_an_old_open_cannot_hide_a_reopened_menu() {
        let state = TrayMenuState::default();
        let first_open = state.begin_open(TrayMenuAnchor::default());
        let first_presentation = state
            .present_if_current(first_open.request_id, || Ok(()))
            .unwrap()
            .unwrap();

        let reopened = state.begin_open(TrayMenuAnchor::default());
        let mut did_hide = false;
        let hidden = state
            .hide_if_current(first_presentation.generation, || {
                did_hide = true;
                Ok(())
            })
            .unwrap();

        assert!(!hidden);
        assert!(!did_hide);
        assert!(reopened.generation > first_presentation.generation);
    }

    #[test]
    fn stale_open_request_cannot_present_over_the_latest_request() {
        let state = TrayMenuState::default();
        let first_open = state.begin_open(TrayMenuAnchor::default());
        let latest_open = state.begin_open(TrayMenuAnchor::default());
        let mut did_present = false;

        let presentation = state
            .present_if_current(first_open.request_id, || {
                did_present = true;
                Ok(())
            })
            .unwrap();

        assert!(presentation.is_none());
        assert!(!did_present);
        assert_eq!(state.current().request_id, latest_open.request_id);
    }

    #[test]
    fn bottom_taskbar_popup_stays_in_the_monitor_work_area() {
        let placement = calculate_tray_menu_placement(
            1_910.0,
            1_060.0,
            TrayMenuBounds {
                left: 0,
                top: 0,
                width: 1_920,
                height: 1_040,
            },
            306,
            248,
            8,
        );

        assert_eq!(placement.anchor.side, "bottom");
        assert_eq!(placement.left, 1_614);
        assert_eq!(placement.top, 792);
        assert!(placement.left >= 0);
        assert!(placement.left + 306 <= 1_920);
        assert!(placement.top >= 0);
        assert!(placement.top + 248 <= 1_040);
    }

    #[test]
    fn popup_from_left_vertical_taskbar_opens_inward() {
        let placement = calculate_tray_menu_placement(
            20.0,
            500.0,
            TrayMenuBounds {
                left: 40,
                top: 0,
                width: 1_880,
                height: 1_080,
            },
            306,
            248,
            8,
        );

        assert_eq!(placement.anchor.side, "left");
        assert_eq!(placement.left, 40);
        assert_eq!(placement.top, 376);
        assert!(placement.left >= 40);
        assert!(placement.left + 306 <= 1_920);
        assert!(placement.top + 248 <= 1_080);
    }
}
