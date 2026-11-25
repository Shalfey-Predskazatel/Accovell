mod codex;
mod codex_config;
mod commands;
mod core;
mod desktop;
pub mod installer;
mod instance;
mod model;
mod proxy;
mod sound;
mod storage;

use core::AppCore;
use std::{fs, path::PathBuf, sync::Arc};
use tokio::runtime::Runtime;

const PROXY_PORT: u16 = 15_789;

pub fn run() -> Result<(), String> {
    desktop::run()
}

pub fn run_installer(options: installer::LaunchOptions) -> Result<(), String> {
    installer::run(options)
}

pub fn show_error(message: &str) {
    instance::show_error(message);
}

pub fn prepare_for_uninstall() -> Result<(), String> {
    let root = storage::app_data_root()?;
    let vault_path = root.join("vault.json");
    if !vault_path.exists() {
        let codex_home = codex::current_codex_home()?;
        if let Ok(config) = fs::read_to_string(codex_home.join("config.toml")) {
            if config.contains("127.0.0.1:15789") || config.contains("localhost:15789") {
                return Err("config.toml указывает на локальный маршрут Codex, но хранилище приложения отсутствует. Удаление остановлено, чтобы Codex не потерял подключение.".to_string());
            }
        }
        return storage::set_startup(false, false);
    }
    let mut vault = storage::load(&root)?;
    let previous_settings = vault.settings.clone();
    let config_snapshot = if vault.proxy_configured {
        if codex::desktop_is_running() {
            return Err("Закройте Codex перед удалением: его открытая сессия ещё использует локальный маршрут.".to_string());
        }
        let access_key = vault.proxy_access_key.as_deref().ok_or_else(|| {
            "Не найден ключ локального маршрута. Удаление остановлено, чтобы не повредить config.toml.".to_string()
        })?;
        let codex_home = codex::current_codex_home()?;
        let config_path = codex_home.join("config.toml");
        let original_config = fs::read(&config_path).map_err(|_| {
            "Не удалось сохранить config.toml перед удалением. Проверьте папку CODEX_HOME."
                .to_string()
        })?;
        let local_url = format!("http://127.0.0.1:{PROXY_PORT}/{access_key}/backend-api/codex");
        codex_config::restore_proxy(&codex_home, &local_url, &mut vault)?;
        vault.settings.proxy_enabled = false;
        Some((config_path, original_config))
    } else {
        None
    };
    vault.settings.start_with_windows = false;
    vault.settings.start_in_tray = false;

    if let Err(error) = storage::set_startup(false, false) {
        return Err(rollback_uninstall_state(
            config_snapshot.as_ref(),
            &previous_settings,
            error,
        ));
    }
    if let Err(error) = storage::save(&root, &vault) {
        return Err(rollback_uninstall_state(
            config_snapshot.as_ref(),
            &previous_settings,
            error,
        ));
    }
    Ok(())
}

fn rollback_uninstall_state(
    config_snapshot: Option<&(PathBuf, Vec<u8>)>,
    previous_settings: &model::AppSettings,
    original_error: String,
) -> String {
    let config_restored =
        config_snapshot.is_none_or(|(path, bytes)| storage::atomic_write(path, bytes).is_ok());
    let startup_restored = storage::set_startup(
        previous_settings.start_with_windows,
        previous_settings.start_in_tray,
    )
    .is_ok();
    if config_restored && startup_restored {
        original_error
    } else {
        format!(
            "{original_error} Не удалось полностью вернуть параметры маршрута или автозапуска. Проверьте config.toml и автозапуск Windows."
        )
    }
}

fn initialize(runtime: &Runtime) -> Result<Arc<AppCore>, String> {
    let root = storage::app_data_root()?;
    // OAuth and usage helpers use isolated homes that contain short-lived
    // plaintext auth files. Clean up abandoned workspaces after a prior crash.
    codex::cleanup_stale_auth_homes(&root);
    let codex_home = codex::current_codex_home()?;
    let cli = codex::find_codex_cli().unwrap_or_else(|_| PathBuf::new());
    let core = AppCore::new(root, codex_home, cli, PROXY_PORT)?;

    if runtime.block_on(async { core.vault.read().await.proxy_configured }) {
        let old_proxy_url = format!("http://127.0.0.1:{PROXY_PORT}/backend-api/codex");
        let migration = runtime.block_on(async {
            let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
            let previous = vault.clone();
            let config_path = core.codex_home.join("config.toml");
            let config_snapshot = snapshot_optional_file(&config_path)?;
            let changed = match codex_config::migrate_proxy_url(
                &core.codex_home,
                &old_proxy_url,
                &core.local_proxy_url(),
                &mut vault,
            ) {
                Ok(changed) => changed,
                Err(error) => {
                    *vault = previous;
                    return Err(error);
                }
            };
            if changed {
                if let Err(error) = storage::save(&core.root, &vault) {
                    *vault = previous;
                    let restore_result = restore_optional_file(
                        &config_path,
                        config_snapshot.as_deref(),
                    );
                    return match restore_result {
                        Ok(()) => Err(error),
                        Err(restore_error) => Err(format!(
                            "{error} Не удалось восстановить config.toml после переноса маршрута: {restore_error}"
                        )),
                    };
                }
            }
            drop(vault);
            drop(_save_guard);
            codex_config::proxy_route_is_active(&core.codex_home, &core.local_proxy_url())
        });
        match migration {
            Ok(true) => {}
            Ok(false) => {
                let reset = runtime.block_on(async {
                    let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
                    let previous = vault.clone();
                    codex_config::forget_proxy_state(&mut vault);
                    vault.settings.proxy_enabled = false;
                    if let Err(error) = storage::save(&core.root, &vault) {
                        *vault = previous;
                        return Err(error);
                    }
                    Ok(())
                });
                if let Err(error) = reset {
                    *core.proxy_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                        format!("Локальный маршрут отключён, но состояние приложения не удалось сохранить: {error}"),
                    );
                } else {
                    *core.proxy_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                        "config.toml был изменён вручную, поэтому старый локальный маршрут отключён без перезаписи файла. Подключите маршрут снова, чтобы продолжить бесшовное переключение.".to_string(),
                    );
                }
            }
            Err(error) => {
                *core.proxy_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
            }
        }
    }
    match runtime.block_on(proxy::start(core.clone())) {
        Ok(()) => core.set_proxy_running(true),
        Err(error) => {
            core.set_proxy_running(false);
            *core.proxy_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
        }
    }
    commands::start_refresh_loop(core.clone(), runtime.handle());
    Ok(core)
}

pub async fn restore_proxy_before_exit(core: &Arc<AppCore>) -> Result<(), String> {
    let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
    let previous = vault.clone();
    let config_path = core.codex_home.join("config.toml");
    let config_snapshot = snapshot_optional_file(&config_path)?;
    if let Err(error) =
        codex_config::restore_proxy(&core.codex_home, &core.local_proxy_url(), &mut vault)
    {
        *vault = previous;
        let restore_result = restore_optional_file(&config_path, config_snapshot.as_deref());
        return match restore_result {
            Ok(()) => Err(error),
            Err(restore_error) => Err(format!(
                "{error} Не удалось вернуть исходный config.toml: {restore_error}"
            )),
        };
    }
    vault.settings.proxy_enabled = false;
    if let Err(error) = storage::save(&core.root, &vault) {
        *vault = previous;
        let restore_result = restore_optional_file(&config_path, config_snapshot.as_deref());
        return match restore_result {
            Ok(()) => Err(error),
            Err(restore_error) => Err(format!(
                "{error} Не удалось вернуть исходный config.toml: {restore_error}"
            )),
        };
    }
    drop(vault);
    drop(_save_guard);
    *core
        .proxy_error
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    Ok(())
}

fn snapshot_optional_file(path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("Не удалось сохранить config.toml перед изменением маршрута.".to_string()),
    }
}

fn restore_optional_file(path: &std::path::Path, bytes: Option<&[u8]>) -> Result<(), String> {
    if let Some(bytes) = bytes {
        return storage::atomic_write(path, bytes)
            .map_err(|_| "не удалось восстановить исходный файл".to_string());
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("не удалось удалить изменённый файл".to_string()),
    }
}
