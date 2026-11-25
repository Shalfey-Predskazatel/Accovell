//! Tauri hosted installer flow.
//!
//! The NSIS executable is used only as a small transport layer. It extracts
//! this application and the payload to a temporary directory, then starts the
//! same Tauri UI with `--installer`. The visible flow and the actual file
//! installation are handled here in Rust.

use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
use winreg::{enums::HKEY_CURRENT_USER, enums::HKEY_LOCAL_MACHINE, RegKey};

const WEBVIEW2_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
const DEFAULT_INSTALL_SUBDIR: &str = "Programs\\Codex Accounts";
const PRODUCT_NAME: &str = "Codex Accounts";

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub payload_dir: Option<PathBuf>,
    pub result_file: Option<PathBuf>,
    pub finish: bool,
    pub uninstall: bool,
    pub install_dir: Option<PathBuf>,
    pub webview2_bootstrapper: Option<PathBuf>,
}

impl LaunchOptions {
    pub fn from_args<I>(args: I) -> Option<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let mut args = args.into_iter();
        let mut installer = false;
        let mut options = Self {
            payload_dir: None,
            result_file: None,
            finish: false,
            uninstall: false,
            install_dir: None,
            webview2_bootstrapper: None,
        };

        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--installer" => installer = true,
                "--finish" => options.finish = true,
                "--uninstall" => options.uninstall = true,
                "--payload-dir" => options.payload_dir = args.next().map(PathBuf::from),
                "--result-file" => options.result_file = args.next().map(PathBuf::from),
                "--install-dir" => options.install_dir = args.next().map(PathBuf::from),
                "--webview2-bootstrapper" => {
                    options.webview2_bootstrapper = args.next().map(PathBuf::from)
                }
                _ => {}
            }
        }

        installer.then_some(options)
    }
}

#[derive(Debug, Clone)]
pub struct InstallerState {
    pub options: Arc<LaunchOptions>,
    operation: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallerSnapshot {
    pub default_path: String,
    pub selected_path: String,
    pub finish: bool,
    pub uninstall: bool,
    pub app_version: String,
}

#[cfg(windows)]
fn disable_browser_context_menu(window: &tauri::WebviewWindow) {
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

pub fn run(options: LaunchOptions) -> Result<(), String> {
    // A Tauri window cannot render a consent dialog when WebView2 is missing.
    // The bootstrapper is still launched without a console, and the same
    // process opens the branded Tauri screen once Runtime is available.
    if !ensure_webview2(options.webview2_bootstrapper.as_deref())? {
        return Ok(());
    }

    let options = Arc::new(options);
    let setup_options = options.clone();
    let operation = Arc::new(tokio::sync::Mutex::new(()));
    let close_operation = operation.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(InstallerState { options, operation })
        .setup(move |app| {
            // The normal config creates a hidden `main` window. Replace it
            // with an installer route so the dashboard never touches the
            // account vault while setup is running.
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.close();
            }
            let query = if setup_options.uninstall {
                "index.html?installer=1&uninstall=1"
            } else if setup_options.finish {
                "index.html?installer=1&finish=1"
            } else {
                "index.html?installer=1"
            };
            let icon = app
                .default_window_icon()
                .cloned()
                .ok_or_else(|| std::io::Error::other("Иконка Codex Accounts не найдена"))?;
            let window = WebviewWindowBuilder::new(app, "installer", WebviewUrl::App(query.into()))
                .title("Codex Accounts Setup")
                // Keep the branded setup window inside a 125% scaled 1366px
                // display.  CSS reflows the two columns below this width.
                .inner_size(980.0, 660.0)
                .min_inner_size(820.0, 580.0)
                .center()
                .decorations(false)
                .transparent(true)
                .resizable(true)
                .shadow(true)
                .icon(icon)
                .map_err(std::io::Error::other)?
                .build()
                .map_err(std::io::Error::other)?;
            disable_browser_context_menu(&window);
            let close_options = setup_options.clone();
            let close_app = app.handle().clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let Ok(_guard) = close_operation.try_lock() else { return };
                    match record_installer_close(&close_options) {
                        Ok(()) => close_app.exit(0),
                        Err(error) => crate::show_error(&error),
                    }
                }
            });
            window.show().map_err(std::io::Error::other)?;
            window.set_focus().map_err(std::io::Error::other)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            installer_snapshot,
            installer_prepare,
            installer_close,
            installer_uninstall,
            installer_launch,
            installer_open_folder,
        ])
        .run(tauri::generate_context!())
        .map_err(|error| format!("Не удалось открыть установщик: {error}"))
}

#[tauri::command]
fn installer_snapshot(state: State<'_, InstallerState>) -> Result<InstallerSnapshot, String> {
    let default_path = default_install_path();
    let selected_path = state
        .options
        .install_dir
        .clone()
        .unwrap_or_else(|| default_path.clone());
    Ok(InstallerSnapshot {
        default_path: default_path.to_string_lossy().into_owned(),
        selected_path: selected_path.to_string_lossy().into_owned(),
        finish: state.options.finish,
        uninstall: state.options.uninstall,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[tauri::command]
async fn installer_prepare(
    app: AppHandle,
    state: State<'_, InstallerState>,
    path: String,
) -> Result<(), String> {
    let target = normalize_install_path(&path)?;
    let options = state.options.clone();
    let _operation = state.operation.clone().try_lock_owned().map_err(|_| "Дождитесь завершения текущей операции.".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let payload_dir = options
            .payload_dir
            .as_deref()
            .ok_or_else(|| "Не найдены временные файлы установщика.".to_string())?;
        install_payload(payload_dir, &target)?;
        create_shortcuts(&target)?;
        write_install_registry(&target)?;
        write_result(&options, "installed", Some(&target))
    })
    .await
    .map_err(|error| format!("Не удалось завершить установку: {error}"))??;
    let _ = app.emit("installer-prepared", ());
    Ok(())
}

#[tauri::command]
fn installer_close(app: AppHandle, state: State<'_, InstallerState>) -> Result<(), String> {
    let _operation = state.operation.try_lock().map_err(|_| "Дождитесь завершения текущей операции.".to_string())?;
    record_installer_close(&state.options)?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
async fn installer_uninstall(app: AppHandle, state: State<'_, InstallerState>) -> Result<(), String> {
    if !state.options.uninstall {
        return Err("Этот экран не запущен в режиме удаления.".to_string());
    }
    let options = state.options.clone();
    let _operation = state.operation.clone().try_lock_owned().map_err(|_| "Дождитесь завершения текущей операции.".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::prepare_for_uninstall()?;
        write_result(&options, "uninstalled", None)
    })
    .await
    .map_err(|error| format!("Не удалось завершить подготовку удаления: {error}"))??;
    let _ = app.emit("installer-uninstall-prepared", ());
    Ok(())
}

#[tauri::command]
fn installer_launch(
    app: AppHandle,
    state: State<'_, InstallerState>,
    path: Option<String>,
) -> Result<(), String> {
    let install_dir = path
        .as_deref()
        .map(normalize_install_path)
        .transpose()?
        .or_else(|| state.options.install_dir.clone())
        .ok_or_else(|| "Не указана папка установленного приложения.".to_string())?;
    let executable = install_dir.join("codex-accounts.exe");
    if !executable.is_file() {
        return Err("Файл Codex Accounts ещё не найден в выбранной папке.".to_string());
    }
    let mut command = Command::new(executable);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command
        .spawn()
        .map_err(|error| format!("Не удалось запустить Codex Accounts: {error}"))?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn installer_open_folder(
    state: State<'_, InstallerState>,
    path: Option<String>,
) -> Result<(), String> {
    let install_dir = path
        .as_deref()
        .map(normalize_install_path)
        .transpose()?
        .or_else(|| state.options.install_dir.clone())
        .ok_or_else(|| "Не указана папка установленного приложения.".to_string())?;
    if !install_dir.is_dir() {
        return Err("Папка установленного приложения ещё не найдена.".to_string());
    }
    let mut command = Command::new("explorer.exe");
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command
        .arg(&install_dir)
        .spawn()
        .map_err(|error| format!("Не удалось открыть папку приложения: {error}"))?;
    Ok(())
}

fn default_install_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DEFAULT_INSTALL_SUBDIR)
}

fn normalize_install_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Выберите папку для установки приложения.".to_string());
    }
    let mut path = PathBuf::from(trimmed);
    if path.is_relative() {
        path = std::env::current_dir()
            .map_err(|error| format!("Не удалось определить текущую папку: {error}"))?
            .join(path);
    }
    if path.file_name().is_none() || path.parent().is_none() {
        return Err("Выберите папку для установки приложения.".to_string());
    }
    Ok(path)
}

fn install_payload(payload_dir: &Path, target: &Path) -> Result<(), String> {
    if !payload_dir.is_dir() {
        return Err("Временные файлы приложения не найдены. Перезапустите установщик.".to_string());
    }

    let payload_absolute = absolute_path(payload_dir)?;
    let target_absolute = absolute_path(target)?;
    if paths_overlap(&payload_absolute, &target_absolute) {
        return Err(
            "Папка установки не может совпадать с временной папкой установщика.".to_string(),
        );
    }

    fs::create_dir_all(target)
        .map_err(|error| format!("Не удалось создать папку установки: {error}"))?;
    copy_tree(payload_dir, target)
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source)
        .map_err(|error| format!("Не удалось прочитать файлы установщика: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("Не удалось прочитать файл установщика: {error}"))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)
            .map_err(|error| format!("Не удалось проверить {}: {error}", source_path.display()))?;
        if metadata.is_dir() {
            fs::create_dir_all(&target_path).map_err(|error| {
                format!(
                    "Не удалось создать папку {}: {error}",
                    target_path.display()
                )
            })?;
            copy_tree(&source_path, &target_path)?;
        } else if metadata.is_file() {
            copy_file_atomic(&source_path, &target_path)?;
        } else {
            return Err(format!(
                "Неподдерживаемый файл в payload: {}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

fn copy_file_atomic(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("Не удалось подготовить папку {}: {error}", parent.display())
        })?;
    }
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temporary = target.with_file_name(format!(".{file_name}.codex-part"));
    let _ = fs::remove_file(&temporary);
    if let Err(error) = fs::copy(source, &temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "Не удалось скопировать {}: {error}",
            source.display()
        ));
    }
    let backup = target.with_file_name(format!(".{file_name}.codex-old"));
    let had_existing = target.exists();
    if had_existing {
        let _ = fs::remove_file(&backup);
        if let Err(error) = fs::rename(target, &backup) {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "Не удалось заменить {}. Закройте приложение и повторите установку: {error}",
                target.display()
            ));
        }
    }
    if let Err(error) = fs::rename(&temporary, target) {
        let _ = fs::remove_file(&temporary);
        if had_existing {
            let _ = fs::rename(&backup, target);
        }
        return Err(format!(
            "Не удалось завершить запись {}: {error}",
            target.display()
        ));
    }
    if had_existing {
        let _ = fs::remove_file(&backup);
    }
    Ok(())
}

fn create_shortcuts(target: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let executable = target.join("codex-accounts.exe");
        let icon = target.join("icon.ico");
        let start_menu = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| "Не удалось определить папку меню «Пуск».".to_string())?
            .join("Microsoft\\Windows\\Start Menu\\Programs\\Codex Accounts");
        let desktop = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .ok_or_else(|| "Не удалось определить рабочий стол пользователя.".to_string())?
            .join("Desktop");
        fs::create_dir_all(&start_menu)
            .map_err(|error| format!("Не удалось создать папку меню «Пуск»: {error}"))?;
        fs::create_dir_all(&desktop)
            .map_err(|error| format!("Не удалось подготовить рабочий стол: {error}"))?;

        create_shortcut(&start_menu.join("Codex Accounts.lnk"), &executable, &icon)?;
        create_shortcut(&desktop.join("Codex Accounts.lnk"), &executable, &icon)?;
    }
    let _ = target;
    Ok(())
}

#[cfg(windows)]
fn create_shortcut(shortcut: &Path, executable: &Path, icon: &Path) -> Result<(), String> {
    let shortcut = powershell_literal(shortcut);
    let executable_path = executable.to_path_buf();
    let executable = powershell_literal(executable);
    let working_directory = executable_path
        .parent()
        .map(powershell_literal)
        .unwrap_or_default();
    let icon = powershell_literal(icon);
    let script = format!(
        "$ws=New-Object -ComObject WScript.Shell;$s=$ws.CreateShortcut('{shortcut}');$s.TargetPath='{executable}';$s.WorkingDirectory='{working_directory}';$s.IconLocation='{icon},0';$s.Description='{PRODUCT_NAME}';$s.Save();"
    );
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ]);
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let status = command
        .status()
        .map_err(|error| format!("Не удалось создать ярлык {}: {error}", shortcut))?;
    if !status.success() {
        return Err(format!("Не удалось создать ярлык {}.", shortcut));
    }
    Ok(())
}

#[cfg(windows)]
fn powershell_literal(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

#[cfg(windows)]
fn write_install_registry(target: &Path) -> Result<(), String> {
    let root = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = root
        .create_subkey("Software\\CodexAccounts")
        .map_err(|error| format!("Не удалось сохранить путь установки: {error}"))?;
    key.set_value("InstallDir", &target.to_string_lossy().to_string())
        .map_err(|error| format!("Не удалось сохранить путь установки: {error}"))?;
    Ok(())
}

#[cfg(not(windows))]
fn write_install_registry(_target: &Path) -> Result<(), String> {
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .map_err(|error| format!("Не удалось определить текущую папку: {error}"))?
            .join(path))
    }
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    let normalize = |path: &Path| path.to_string_lossy().replace('/', "\\").to_lowercase();
    let a = normalize(a);
    let b = normalize(b);
    a == b || a.starts_with(&(b.clone() + "\\")) || b.starts_with(&(a + "\\"))
}

fn write_result(options: &LaunchOptions, status: &str, path: Option<&Path>) -> Result<(), String> {
    let Some(result_file) = options.result_file.as_deref() else {
        return Ok(());
    };
    if let Some(parent) = result_file.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("Не удалось подготовить временную папку установщика: {error}")
        })?;
    }
    let mut contents = format!("[installer]\nstatus={status}\n");
    if let Some(path) = path {
        contents.push_str(&format!("path={}\n", sanitize_ini_value(path)));
    }
    fs::write(result_file, contents.as_bytes())
        .map_err(|error| format!("Не удалось сохранить состояние установщика: {error}"))
}

fn record_installer_close(options: &LaunchOptions) -> Result<(), String> {
    let completed = if options.uninstall { "uninstalled" } else { "installed" };
    if result_has_status(options, completed) {
        return Ok(());
    }
    write_result(options, "cancelled", None)
}

fn result_has_status(options: &LaunchOptions, expected: &str) -> bool {
    let Some(result_file) = options.result_file.as_deref() else {
        return false;
    };
    let Ok(contents) = fs::read_to_string(result_file) else {
        return false;
    };
    contents
        .lines()
        .any(|line| line.trim() == format!("status={expected}"))
}

fn sanitize_ini_value(path: &Path) -> String {
    path.to_string_lossy().replace(['\n', '\r'], "")
}

#[cfg(test)]
mod result_tests {
    use super::*;

    #[test]
    fn closing_preserves_completed_results_and_records_cancellation_before_commit() {
        let root = std::env::temp_dir().join(format!("codex-install-result-{}", uuid::Uuid::new_v4()));
        for (mode, completed) in [(false, "installed"), (true, "uninstalled")] {
            let options = LaunchOptions {
                payload_dir: None,
                result_file: Some(root.join(format!("{completed}.ini"))),
                finish: false,
                uninstall: mode,
                install_dir: None,
                webview2_bootstrapper: None,
            };
            record_installer_close(&options).expect("record cancellation");
            assert!(result_has_status(&options, "cancelled"));
            write_result(&options, completed, Some(&root)).expect("record completion");
            let path = options.result_file.as_ref().expect("result path");
            let committed = fs::read(path).expect("read committed result");
            record_installer_close(&options).expect("close after completion");
            assert_eq!(fs::read(path).expect("read closed result"), committed);
            fs::remove_file(path).expect("remove test result");
        }
        fs::remove_dir(root).expect("remove test directory");
    }
}

fn ensure_webview2(bootstrapper: Option<&Path>) -> Result<bool, String> {
    if webview2_is_installed() {
        return Ok(true);
    }
    let Some(bootstrapper) = bootstrapper.filter(|path| path.is_file()) else {
        return Err("WebView2 Runtime не найден, а встроенный установщик недоступен.".to_string());
    };

    #[cfg(windows)]
    {
        let message =
            wide("Для установщика нужен Microsoft Edge WebView2 Runtime. Установить его сейчас?");
        let title = wide("Codex Accounts · WebView2");
        let answer = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_YESNO
                    | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONQUESTION,
            )
        };
        if answer != windows_sys::Win32::UI::WindowsAndMessaging::IDYES {
            return Ok(false);
        }
        let mut command = Command::new(bootstrapper);
        command.args(["/silent", "/install"]);
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let status = command
            .status()
            .map_err(|error| format!("Не удалось запустить установщик WebView2: {error}"))?;
        if !status.success() && !webview2_is_installed() {
            return Err(format!(
                "Установщик WebView2 завершился с кодом {}.",
                status.code().unwrap_or(-1)
            ));
        }
        if webview2_is_installed() {
            Ok(true)
        } else {
            Err("Установщик WebView2 завершился, но Runtime не найден.".to_string())
        }
    }

    #[cfg(not(windows))]
    {
        let _ = bootstrapper;
        Ok(true)
    }
}

/// Verify the shared WebView2 runtime before starting the normal dashboard.
///
/// The dashboard itself is a Tauri WebView, so it cannot show a branded
/// recovery screen when the runtime has been removed after installation. Keep
/// the small official bootstrapper beside the app and perform this native
/// preflight first instead. A portable build can use the same mechanism when
/// the bootstrapper is placed beside its executable.
pub fn ensure_webview2_for_app() -> Result<bool, String> {
    let bootstrapper = std::env::current_exe().ok().and_then(|executable| {
        executable
            .parent()
            .map(|parent| parent.join("MicrosoftEdgeWebView2Setup.exe"))
    });
    ensure_webview2(bootstrapper.as_deref())
}

#[cfg(windows)]
fn webview2_is_installed() -> bool {
    let subkey = format!(
        "SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{}",
        WEBVIEW2_GUID
    );
    let wow_subkey = format!(
        "SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{}",
        WEBVIEW2_GUID
    );
    let keys = [
        (HKEY_LOCAL_MACHINE, subkey.clone()),
        (HKEY_LOCAL_MACHINE, wow_subkey.clone()),
        (HKEY_CURRENT_USER, subkey),
        (HKEY_CURRENT_USER, wow_subkey),
    ];
    keys.into_iter().any(|(hive, key)| {
        RegKey::predef(hive)
            .open_subkey(&key)
            .ok()
            .and_then(|key| key.get_value::<String, _>("pv").ok())
            .is_some_and(|version| !version.trim().is_empty())
    })
}

#[cfg(not(windows))]
fn webview2_is_installed() -> bool {
    true
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
