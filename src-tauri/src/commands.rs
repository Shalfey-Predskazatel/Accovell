use crate::{
    codex, codex_config,
    core::{now_timestamp, sorted_candidates, AppCore},
    model::{
        AccountProfile, AccountStatus, AccountStatusKind, AccountView, AppSettings, BackupAccount,
        BackupPayload, CodexPreferences, DashboardData, UsageSnapshot, VaultDocument,
    },
    storage,
};
use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

const MAX_CONCURRENT_USAGE_REFRESHES: usize = 3;

pub async fn load_dashboard(core: &Arc<AppCore>) -> Result<DashboardData, String> {
    dashboard(core).await
}

pub async fn dashboard(core: &Arc<AppCore>) -> Result<DashboardData, String> {
    let vault = core.vault.read().await.clone();
    if core.admin_protected() && !core.admin_unlocked() {
        let safe_settings = AppSettings {
            sound_enabled: vault.settings.sound_enabled,
            ..AppSettings::default()
        };
        return Ok(DashboardData {
            accounts: Vec::new(),
            settings: safe_settings,
            current_account_id: None,
            refreshed_at: None,
            codex_preferences: CodexPreferences::default(),
            prompts: Vec::new(),
            codex_connected: false,
            codex_running: false,
            proxy_address: String::new(),
            proxy_error: None,
            admin_protected: true,
            admin_unlocked: false,
        });
    }
    let mut accounts = vault
        .accounts
        .iter()
        .map(|account| AccountView {
            id: account.id.clone(),
            account_id: account.account_id.clone(),
            email: account.email.clone(),
            plan: account.plan.clone(),
            created_at: account.created_at,
            last_used_at: account.last_used_at,
            usage: account.usage.clone(),
            status: account.status.clone(),
            is_active: vault.active_account_id.as_deref() == Some(account.id.as_str()),
        })
        .collect::<Vec<_>>();
    sort_accounts_by_usage(&mut accounts);
    let refreshed_at = accounts
        .iter()
        .filter_map(|account| account.usage.as_ref().map(|usage| usage.updated_at))
        .max();
    let codex_preferences = codex_config::preferences(&core.codex_home)?;
    let prompts = codex_config::list_prompts(&core.codex_home)?;
    let stored_proxy_error = core
        .proxy_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let (proxy_route_active, route_error) = if vault.proxy_configured {
        match codex_config::proxy_route_is_active(&core.codex_home, &core.local_proxy_url()) {
            Ok(true) => (true, None),
            Ok(false) => (
                false,
                Some("config.toml больше не направляет Codex через Codex Accounts. Переключение профиля остановлено, чтобы не менять аккаунт для другого провайдера. Восстановите локальный маршрут в настройках Codex.".to_string()),
            ),
            Err(error) => (false, Some(error)),
        }
    } else {
        (false, None)
    };
    let proxy_error = stored_proxy_error.or(route_error);
    Ok(DashboardData {
        accounts,
        settings: vault.settings,
        current_account_id: vault.active_account_id,
        refreshed_at,
        codex_preferences,
        prompts,
        codex_connected: vault.proxy_configured
            && proxy_route_active
            && core.proxy_is_running()
            && proxy_error.is_none(),
        codex_running: codex::desktop_is_running(),
        proxy_address: core.local_proxy_url(),
        proxy_error,
        admin_protected: core.admin_protected(),
        admin_unlocked: core.admin_unlocked(),
    })
}

#[allow(dead_code)] // Kept for the legacy native UI path; the Tauri desktop UI uses progress events.
pub async fn add_account(core: &Arc<AppCore>) -> Result<DashboardData, String> {
    add_account_with_login_mode(core, codex::LoginMode::Browser, |_| {}).await
}

pub async fn add_account_with_login_mode<F>(
    core: &Arc<AppCore>,
    login_mode: codex::LoginMode,
    on_auth_info: F,
) -> Result<DashboardData, String>
where
    F: Fn(codex::LoginAuthInfo) + Send + Sync + 'static,
{
    let core = core.clone();
    core.require_admin()?;
    if !core.cli.is_file() {
        return Err("Не найден официальный Codex CLI. Установите Codex или задайте путь через CODEX_CLI_PATH, затем перезапустите приложение.".to_string());
    }
    let on_auth_info = Arc::new(on_auth_info);
    let mut account = tokio::task::spawn_blocking({
        let root = core.root.clone();
        let cli = core.cli.clone();
        let cancel = core.add_account_cancel_token();
        move || {
            codex::add_account_with_mode(&root, &cli, cancel, login_mode, move |info| {
                on_auth_info(info)
            })
        }
    })
    .await
    .map_err(|_| {
        "Окно входа Codex завершилось с ошибкой. Запустите добавление аккаунта ещё раз.".to_string()
    })??;
    let auth_lock = core.account_auth_lock(&account.account_id);
    let _auth_guard = auth_lock.lock().await;
    let account_id = account.account_id.clone();
    {
        let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
        let previous = vault.clone();
        if let Some(existing) = vault
            .accounts
            .iter()
            .find(|existing| existing.account_id == account_id)
            .cloned()
        {
            account.id = existing.id;
            account.created_at = existing.created_at;
            account.usage = existing.usage;
        }
        account.last_used_at = Some(now_timestamp());
        if let Some(existing) = vault
            .accounts
            .iter_mut()
            .find(|existing| existing.account_id == account_id)
        {
            *existing = account.clone();
        } else {
            vault.accounts.push(account.clone());
        }
        vault.active_account_id = Some(account.id.clone());
        if let Err(error) = storage::save(&core.root, &vault) {
            *vault = previous;
            return Err(error);
        }
    }
    core.emit_changed();
    drop(_auth_guard);
    try_connect_after_account_added(&core).await;
    dashboard(&core).await
}

pub async fn complete_add_account_callback(
    core: &Arc<AppCore>,
    callback_url: String,
) -> Result<bool, String> {
    core.require_admin()?;
    // The copy-link action owns a PKCE session in memory.  Complete that
    // exchange here instead of forwarding the callback to a CLI process that
    // never opened this particular URL.  Keeping this branch ahead of the
    // legacy path also means a manual link can be copied, opened on another
    // browser, and pasted back without launching a browser from the app.
    if codex::manual_auth_session_present() {
        if !core.add_account_is_running() {
            return Err(
                "Сессия ручного входа уже закрыта. Скопируйте новую ссылку и повторите вход."
                    .to_string(),
            );
        }
        let auth = match codex::complete_manual_auth_callback(&callback_url).await {
            Ok(Some(auth)) => auth,
            Ok(None) => {
                core.finish_add_account();
                return Err(
                    "Сессия ручного входа уже закрыта. Скопируйте новую ссылку и повторите вход."
                        .to_string(),
                );
            }
            Err(error) => {
                // Validation and transient network errors leave the session
                // active for a retry. Expiration clears it and must also free
                // the interactive slot.
                if !codex::manual_auth_session_present() {
                    core.finish_add_account();
                }
                return Err(error);
            }
        };
        // The token request can take several seconds. If the user cancels
        // while it is in flight, do not persist the credentials when the
        // response eventually arrives.
        if !core.add_account_is_running() {
            codex::cancel_manual_auth_session();
            return Err("Вход отменён. Аккаунт не был сохранён.".to_string());
        }
        // Keep the PKCE session cancellable until the account commit starts.
        // Always release it after persistence, even if saving or the optional
        // proxy reconnect fails, so the UI cannot be stranded as busy.
        let store_result = store_manual_account(core, &auth).await;
        codex::cancel_manual_auth_session();
        core.finish_add_account();
        store_result?;
        core.emit_changed();
        return Ok(true);
    }
    if !core.add_account_is_running() {
        return Err(
            "Сначала нажмите «Добавить аккаунт» или скопируйте новую ссылку для входа.".to_string(),
        );
    }
    codex::forward_auth_callback(&callback_url).await?;
    Ok(false)
}

/// Persist tokens received by the direct PKCE flow using the same account
/// replacement rules as the official CLI login path.  This deliberately
/// keeps one profile per ChatGPT account and preserves its existing usage
/// history when a user signs in again.
async fn store_manual_account(core: &Arc<AppCore>, auth: &[u8]) -> Result<(), String> {
    let identity = codex::identity_from_auth(auth)?;
    let sealed_auth = storage::seal_auth(auth)?;
    let now = now_timestamp();
    let mut account = AccountProfile {
        id: uuid::Uuid::new_v4().to_string(),
        account_id: identity.account_id,
        email: identity.email,
        plan: identity.plan,
        created_at: now,
        last_used_at: Some(now),
        usage: None,
        status: AccountStatus::healthy(),
        sealed_auth,
    };
    let auth_lock = core.account_auth_lock(&account.account_id);
    let _auth_guard = auth_lock.lock().await;
    {
        let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
        if !core.add_account_is_running() {
            return Err("Вход отменён. Аккаунт не был сохранён.".to_string());
        }
        let previous = vault.clone();
        if let Some(existing) = vault
            .accounts
            .iter()
            .find(|existing| existing.account_id == account.account_id)
            .cloned()
        {
            account.id = existing.id;
            account.created_at = existing.created_at;
            account.usage = existing.usage;
        }
        if let Some(existing) = vault
            .accounts
            .iter_mut()
            .find(|existing| existing.account_id == account.account_id)
        {
            *existing = account.clone();
        } else {
            vault.accounts.push(account.clone());
        }
        vault.active_account_id = Some(account.id.clone());
        if let Err(error) = storage::save(&core.root, &vault) {
            *vault = previous;
            return Err(error);
        }
    }
    try_connect_after_account_added(core).await;
    Ok(())
}

async fn try_connect_after_account_added(core: &Arc<AppCore>) {
    let should_connect = {
        let vault = core.vault.read().await;
        vault.settings.proxy_enabled && !vault.proxy_configured
    };
    if !should_connect {
        return;
    }
    if let Err(error) = connect_codex_inner(core).await {
        *core
            .proxy_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
        core.emit_changed();
    }
}

pub fn open_auth_link_private(core: &AppCore, url: &str) -> Result<(), String> {
    core.require_admin()?;
    if !core.add_account_is_running() {
        return Err(
            "Сначала начните добавление аккаунта, чтобы получить актуальную ссылку входа."
                .to_string(),
        );
    }
    codex::open_private_auth_url(url)
}

pub async fn switch_account(
    core: &Arc<AppCore>,
    account_id: String,
) -> Result<DashboardData, String> {
    core.require_admin()?;
    let connected = core.vault.read().await.proxy_configured;
    if !connected {
        return Err("Сначала нажмите «Подключить Codex» и перезапустите Codex один раз. После этого переключения будут проходить в открытой сессии.".to_string());
    }
    if !core.proxy_is_running() || core.proxy_error_message().is_some() {
        return Err("Локальный маршрут Codex сейчас не активен. Проверьте config.toml, подключите маршрут снова и повторите переключение.".to_string());
    }
    match codex_config::proxy_route_is_active(&core.codex_home, &core.local_proxy_url()) {
        Ok(true) => {}
        Ok(false) => {
            return Err("config.toml больше не направляет Codex через Codex Accounts. Аккаунт не переключён; восстановите локальный маршрут в настройках Codex.".to_string());
        }
        Err(error) => {
            return Err(format!("Аккаунт не переключён: {error}"));
        }
    }
    core.set_active(&account_id).await?;
    dashboard(core).await
}

pub async fn remove_account(
    core: &Arc<AppCore>,
    account_id: String,
) -> Result<DashboardData, String> {
    core.require_admin()?;
    {
        let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
        let previous = vault.clone();
        let old_len = vault.accounts.len();
        vault.accounts.retain(|account| account.id != account_id);
        if vault.accounts.len() == old_len {
            return Err("Этот аккаунт уже удалён. Обновите список профилей.".to_string());
        }
        if vault.active_account_id.as_deref() == Some(account_id.as_str()) {
            let candidates = sorted_candidates(vault.accounts.clone(), None, 0);
            vault.active_account_id = candidates.first().map(|account| account.id.clone());
        }
        if let Err(error) = storage::save(&core.root, &vault) {
            *vault = previous;
            return Err(error);
        }
    }
    core.emit_changed();
    dashboard(core).await
}

pub async fn refresh_all(core: &Arc<AppCore>) -> Result<DashboardData, String> {
    let core = core.clone();
    core.require_admin()?;
    refresh_accounts_inner(&core, None).await?;
    dashboard(&core).await
}

pub async fn refresh_account(
    core: &Arc<AppCore>,
    profile_id: String,
) -> Result<DashboardData, String> {
    let core = core.clone();
    core.require_admin()?;
    refresh_accounts_inner(&core, Some(&profile_id)).await?;
    dashboard(&core).await
}

pub async fn refresh_all_inner(core: &Arc<AppCore>) -> Result<(), String> {
    refresh_accounts_inner(core, None).await
}

async fn refresh_accounts_inner(
    core: &Arc<AppCore>,
    profile_id: Option<&str>,
) -> Result<(), String> {
    let _refresh_guard = core.refresh_lock.lock().await;
    let accounts = {
        let vault = core.vault.read().await;
        vault
            .accounts
            .iter()
            .filter(|account| profile_id.is_none_or(|id| account.id == id))
            .cloned()
            .collect::<Vec<_>>()
    };
    if profile_id.is_some() && accounts.is_empty() {
        return Err("Не удалось найти выбранный аккаунт. Обновите список профилей.".to_string());
    }
    if accounts.is_empty() {
        return Ok(());
    }
    if !core.cli.is_file() {
        set_refresh_error(
            core,
            "Не найден Codex CLI. Установите Codex и перезапустите приложение.",
        )
        .await?;
        return Err(
            "Не найден официальный Codex CLI. Установите Codex или задайте CODEX_CLI_PATH."
                .to_string(),
        );
    }
    let mut jobs = Vec::new();
    let refresh_limit = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_USAGE_REFRESHES));
    for account in accounts {
        let account_profile_id = account.id.clone();
        let account_id = account.account_id.clone();
        let root = core.root.clone();
        let cli = core.cli.clone();
        let core = core.clone();
        let refresh_limit = refresh_limit.clone();
        jobs.push(tokio::spawn(async move {
            let Ok(_refresh_permit) = refresh_limit.acquire_owned().await else {
                return None;
            };
            let auth_lock = core.account_auth_lock(&account_id);
            let _auth_guard = auth_lock.lock().await;
            let account = {
                let vault = core.vault.read().await;
                vault
                    .accounts
                    .iter()
                    .find(|account| account.id == account_profile_id)
                    .cloned()
            };
            let mut account = account?;
            let fallback_account = account.clone();
            let refresh_result = tokio::task::spawn_blocking(move || {
                codex::refresh_one(&root, &cli, &mut account);
                account
            })
            .await;
            let mut refreshed = match refresh_result {
                Ok(account) => account,
                Err(_) => {
                    let mut account = fallback_account;
                    let now = now_timestamp();
                    let detail =
                        "Проверка лимитов Codex завершилась с ошибкой. Повторите обновление.";
                    if let Some(usage) = account.usage.as_mut() {
                        usage.error = Some(detail.to_string());
                    } else {
                        account.usage = Some(UsageSnapshot {
                            primary: None,
                            secondary: None,
                            updated_at: now,
                            error: Some(detail.to_string()),
                        });
                    }
                    account.status = AccountStatus::new(
                        AccountStatusKind::TemporaryError,
                        "Проверка лимитов завершилась с ошибкой. Повторите обновление.",
                    );
                    account
                }
            };
            refreshed.id = account_profile_id;
            Some(refreshed)
        }));
    }
    let mut task_failed = false;
    let mut refreshed_accounts = Vec::with_capacity(jobs.len());
    for job in jobs {
        match job.await {
            Ok(Some(account)) => refreshed_accounts.push(account),
            Ok(None) => {}
            Err(_) => task_failed = true,
        }
    }
    if task_failed {
        return Err(
            "Проверка лимитов одного из аккаунтов неожиданно завершилась. Повторите обновление."
                .to_string(),
        );
    }
    sync_live_auth(core).await?;
    {
        let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
        let previous = vault.clone();
        for refreshed in refreshed_accounts {
            if let Some(target) = vault
                .accounts
                .iter_mut()
                .find(|target| target.id == refreshed.id)
            {
                target.usage = refreshed.usage;
                target.status = refreshed.status;
                if sealed_auth_is_newer(&refreshed.sealed_auth, &target.sealed_auth) {
                    target.sealed_auth = refreshed.sealed_auth;
                }
            }
        }
        if let Err(error) = storage::save(&core.root, &vault) {
            *vault = previous;
            return Err(error);
        }
    }
    core.emit_changed();
    if core.vault.read().await.proxy_configured
        && core.proxy_is_running()
        && core.proxy_error_message().is_none()
    {
        // A quota refresh is successful even when every saved login now
        // needs reauthentication. Keep the account error visible without
        // turning the refresh action itself into a misleading failure.
        let _ = core.select_active().await;
    }
    Ok(())
}

async fn set_refresh_error(core: &Arc<AppCore>, error: &str) -> Result<(), String> {
    let now = now_timestamp();
    {
        let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
        let previous = vault.clone();
        for account in &mut vault.accounts {
            if account.status.kind.is_permanently_unavailable()
                || account.status.kind == AccountStatusKind::UsageLimit
            {
                continue;
            }
            account.status = AccountStatus::new(AccountStatusKind::TemporaryError, error);
            if let Some(usage) = account.usage.as_mut() {
                usage.error = Some(error.to_string());
            } else {
                account.usage = Some(UsageSnapshot {
                    primary: None,
                    secondary: None,
                    updated_at: now,
                    error: Some(error.to_string()),
                });
            }
        }
        if let Err(save_error) = storage::save(&core.root, &vault) {
            *vault = previous;
            return Err(save_error);
        }
    }
    core.emit_changed();
    Ok(())
}

fn sealed_auth_is_newer(candidate: &str, current: &str) -> bool {
    let candidate = storage::open_auth(candidate)
        .ok()
        .and_then(|auth| serde_json::from_slice::<serde_json::Value>(&auth).ok());
    let current = storage::open_auth(current)
        .ok()
        .and_then(|auth| serde_json::from_slice::<serde_json::Value>(&auth).ok());
    match (candidate, current) {
        (Some(candidate), Some(current)) => crate::core::auth_is_newer(&candidate, &current),
        (Some(_), None) => true,
        _ => false,
    }
}

fn sort_accounts_by_usage(accounts: &mut [AccountView]) {
    let now = now_timestamp();
    accounts.sort_by(|left, right| {
        let left_unavailable = left.status.kind.is_permanently_unavailable();
        let right_unavailable = right.status.kind.is_permanently_unavailable();
        if left_unavailable != right_unavailable {
            return left_unavailable.cmp(&right_unavailable);
        }
        let left_remaining = left
            .usage
            .as_ref()
            .and_then(|usage| usage.highest_remaining_at(now));
        let right_remaining = right
            .usage
            .as_ref()
            .and_then(|usage| usage.highest_remaining_at(now));

        match (left_remaining, right_remaining) {
            (Some(left_remaining), Some(right_remaining)) => right_remaining
                .cmp(&left_remaining)
                .then_with(|| {
                    let left_reset = left
                        .usage
                        .as_ref()
                        .and_then(UsageSnapshot::next_reset_at)
                        .unwrap_or(i64::MAX);
                    let right_reset = right
                        .usage
                        .as_ref()
                        .and_then(UsageSnapshot::next_reset_at)
                        .unwrap_or(i64::MAX);
                    left_reset.cmp(&right_reset)
                })
                .then_with(|| left.created_at.cmp(&right.created_at)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => left.created_at.cmp(&right.created_at),
        }
    });
}

async fn sync_live_auth(core: &Arc<AppCore>) -> Result<(), String> {
    let path = core.codex_home.join("auth.json");
    let Ok(initial_bytes) = fs::read(&path) else {
        return Ok(());
    };
    let Ok(initial_identity) = codex::identity_from_auth(&initial_bytes) else {
        return Ok(());
    };
    let auth_lock = core.account_auth_lock(&initial_identity.account_id);
    let _auth_guard = auth_lock.lock().await;
    let Ok(live_bytes) = fs::read(&path) else {
        return Ok(());
    };
    let Ok(live_identity) = codex::identity_from_auth(&live_bytes) else {
        return Ok(());
    };
    if live_identity.account_id != initial_identity.account_id {
        return Ok(());
    }
    let profile = {
        core.vault
            .read()
            .await
            .accounts
            .iter()
            .find(|account| account.account_id == live_identity.account_id)
            .cloned()
    };
    let Some(profile) = profile else {
        return Ok(());
    };
    let saved = storage::open_auth(&profile.sealed_auth)?;
    let saved_value = serde_json::from_slice::<serde_json::Value>(&saved)
        .map_err(|_| "Сохранённый вход Codex повреждён.".to_string())?;
    let live_value = serde_json::from_slice::<serde_json::Value>(&live_bytes)
        .map_err(|_| "Текущий вход Codex повреждён.".to_string())?;
    if crate::core::auth_is_newer(&live_value, &saved_value) {
        let sealed = storage::seal_auth(&live_bytes)?;
        let mut vault = core.vault.write().await;
        if let Some(account) = vault.accounts.iter_mut().find(|account| {
            account.id == profile.id && account.account_id == live_identity.account_id
        }) {
            let current = storage::open_auth(&account.sealed_auth)?;
            let current_value = serde_json::from_slice::<serde_json::Value>(&current)
                .map_err(|_| "Сохранённый вход Codex повреждён.".to_string())?;
            if crate::core::auth_is_newer(&live_value, &current_value) {
                account.sealed_auth = sealed;
            }
        }
    }
    Ok(())
}

pub async fn save_settings(
    core: &Arc<AppCore>,
    settings: AppSettings,
) -> Result<DashboardData, String> {
    core.require_admin()?;
    if settings.switch_threshold > 100 {
        return Err("Порог предупреждения должен быть в диапазоне от 0 до 100%.".to_string());
    }
    if !(1..=86_400).contains(&settings.refresh_interval_seconds) {
        return Err("Интервал проверки должен быть от 1 секунды до 24 часов.".to_string());
    }
    if !matches!(settings.theme.as_str(), "system" | "dark" | "light") {
        return Err("Тема должна быть одной из: системная, тёмная или светлая.".to_string());
    }
    let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
    let previous_settings = vault.settings.clone();
    let refresh_interval = settings.refresh_interval_seconds;
    storage::set_startup(settings.start_with_windows, settings.start_in_tray)?;
    vault.settings = settings;
    vault.settings.proxy_enabled = previous_settings.proxy_enabled;
    if let Err(error) = storage::save(&core.root, &vault) {
        vault.settings = previous_settings.clone();
        drop(vault);
        drop(_save_guard);
        return match storage::set_startup(
            previous_settings.start_with_windows,
            previous_settings.start_in_tray,
        ) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(format!(
                "{error} Не удалось вернуть прежний параметр автозапуска: {rollback_error}"
            )),
        };
    }
    drop(vault);
    drop(_save_guard);
    core.emit_changed();
    core.set_refresh_interval(refresh_interval);
    dashboard(core).await
}

pub async fn unlock_admin(
    core: &Arc<AppCore>,
    mut password: String,
) -> Result<DashboardData, String> {
    let hash = core.vault.read().await.admin_password_hash.clone();
    let Some(hash) = hash else {
        password.zeroize();
        return Err("Защищённый режим отключён. Обновите экран приложения.".to_string());
    };
    let valid = storage::verify_admin_password(&hash, &password);
    password.zeroize();
    if !valid {
        return Err(
            "Пароль администратора не подходит. Проверьте раскладку и повторите ввод.".to_string(),
        );
    }
    core.update_admin_state(true, true);
    dashboard(core).await
}

pub async fn configure_admin_password(
    core: &Arc<AppCore>,
    mut current_password: Option<String>,
    mut new_password: Option<String>,
) -> Result<DashboardData, String> {
    core.require_admin()?;
    let current_hash = core.vault.read().await.admin_password_hash.clone();
    let current_valid = current_hash.as_deref().is_none_or(|hash| {
        storage::verify_admin_password(hash, current_password.as_deref().unwrap_or_default())
    });
    if let Some(password) = current_password.as_mut() {
        password.zeroize();
    }
    if !current_valid {
        if let Some(password) = new_password.as_mut() {
            password.zeroize();
        }
        return Err("Введите текущий пароль администратора, чтобы изменить защиту.".to_string());
    }
    let mut new_password = new_password.take().filter(|password| !password.is_empty());
    let new_hash = match new_password.as_mut() {
        Some(password) if password.chars().count() < 8 => {
            password.zeroize();
            return Err("Пароль администратора должен содержать не менее 8 символов.".to_string());
        }
        Some(password) => {
            let hash = storage::hash_admin_password(password);
            password.zeroize();
            Some(hash?)
        }
        None => None,
    };
    let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
    if vault.admin_password_hash != current_hash {
        return Err(
            "Защита изменилась во время операции. Обновите окно и повторите действие.".to_string(),
        );
    }
    vault.admin_password_hash = new_hash.clone();
    if let Err(error) = storage::save(&core.root, &vault) {
        vault.admin_password_hash = current_hash;
        return Err(error);
    }
    drop(vault);
    drop(_save_guard);
    core.update_admin_state(new_hash.is_some(), true);
    dashboard(core).await
}

pub async fn reset_settings_to_default(core: &Arc<AppCore>) -> Result<DashboardData, String> {
    core.require_admin()?;
    let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
    let previous = vault.clone();
    let config_snapshot = snapshot_config_if_needed(core, previous.proxy_configured)?;
    let mut replacement = vault.clone();
    codex_config::restore_proxy(&core.codex_home, &core.local_proxy_url(), &mut replacement)?;
    if let Err(error) = storage::set_startup(false, false) {
        return Err(rollback_reset(
            core,
            config_snapshot.as_ref(),
            &previous.settings,
            error,
        ));
    }
    replacement.settings = AppSettings::default();
    *vault = replacement;
    if let Err(error) = storage::save(&core.root, &vault) {
        *vault = previous.clone();
        drop(vault);
        drop(_save_guard);
        return Err(rollback_reset(
            core,
            config_snapshot.as_ref(),
            &previous.settings,
            error,
        ));
    }
    drop(vault);
    drop(_save_guard);
    core.set_refresh_interval(AppSettings::default().refresh_interval_seconds);
    dashboard(core).await
}

pub async fn reset_application(core: &Arc<AppCore>) -> Result<DashboardData, String> {
    core.require_admin()?;
    if !core.begin_add_account() {
        return Err(
            "Сначала завершите или отмените открытый вход в браузере, затем повторите полный сброс."
                .to_string(),
        );
    }
    let _interactive_slot = InteractiveSlotReservation(core);
    let _refresh_guard = core.refresh_lock.lock().await;
    let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
    let previous = vault.clone();
    let config_snapshot = snapshot_config(core)?;
    let old_prompts = codex_config::list_prompts(&core.codex_home)?;
    let mut restored = previous.clone();
    codex_config::restore_proxy(&core.codex_home, &core.local_proxy_url(), &mut restored)?;
    if let Err(error) = codex_config::reset_managed_preferences(&core.codex_home) {
        return Err(rollback_full_reset(
            core,
            &config_snapshot,
            &old_prompts,
            &previous,
            error,
        ));
    }
    if let Err(error) = codex_config::replace_prompts(&core.codex_home, &[]) {
        return Err(rollback_full_reset(
            core,
            &config_snapshot,
            &old_prompts,
            &previous,
            error,
        ));
    }
    if let Err(error) = storage::set_startup(false, false) {
        return Err(rollback_full_reset(
            core,
            &config_snapshot,
            &old_prompts,
            &previous,
            error,
        ));
    }

    let replacement = VaultDocument {
        proxy_access_key: Some(core.proxy_access_key.clone()),
        ..VaultDocument::default()
    };
    if let Err(error) = storage::save(&core.root, &replacement) {
        return Err(rollback_full_reset(
            core,
            &config_snapshot,
            &old_prompts,
            &previous,
            error,
        ));
    }

    let runtime_snapshot = match storage::stage_runtime_data_reset(&core.root) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Err(rollback_full_reset(
                core,
                &config_snapshot,
                &old_prompts,
                &previous,
                error,
            ));
        }
    };
    *vault = replacement;
    drop(vault);
    core.clear_response_owners();
    core.update_admin_state(false, true);
    core.set_refresh_interval(AppSettings::default().refresh_interval_seconds);
    let cleanup_warning = runtime_snapshot.commit().err();
    *core
        .proxy_error
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = cleanup_warning;
    core.emit_changed();
    dashboard(core).await
}

struct InteractiveSlotReservation<'a>(&'a AppCore);

impl Drop for InteractiveSlotReservation<'_> {
    fn drop(&mut self) {
        self.0.finish_add_account();
    }
}

struct ConfigSnapshot {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
}

fn snapshot_config_if_needed(
    core: &AppCore,
    proxy_configured: bool,
) -> Result<Option<ConfigSnapshot>, String> {
    if !proxy_configured {
        return Ok(None);
    }
    snapshot_config(core).map(Some)
}

fn snapshot_config(core: &AppCore) -> Result<ConfigSnapshot, String> {
    let path = core.codex_home.join("config.toml");
    let bytes = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            return Err(
                "Не удалось сохранить исходную копию config.toml перед сбросом.".to_string(),
            )
        }
    };
    Ok(ConfigSnapshot { path, bytes })
}

fn rollback_full_reset(
    core: &AppCore,
    config_snapshot: &ConfigSnapshot,
    old_prompts: &[crate::model::PromptView],
    previous: &VaultDocument,
    original_error: String,
) -> String {
    let prompt_restore = codex_config::replace_prompts(&core.codex_home, old_prompts);
    let vault_restore = storage::save(&core.root, previous);
    let external_restore = rollback_reset(
        core,
        Some(config_snapshot),
        &previous.settings,
        original_error,
    );
    let mut failed = Vec::new();
    if prompt_restore.is_err() {
        failed.push("prompts Codex");
    }
    if vault_restore.is_err() {
        failed.push("локальное хранилище");
    }
    if failed.is_empty() {
        external_restore
    } else {
        format!(
            "{external_restore} Не удалось полностью восстановить {}. Проверьте исходные файлы перед повторным сбросом.",
            failed.join(" и ")
        )
    }
}

fn rollback_reset(
    core: &AppCore,
    snapshot: Option<&ConfigSnapshot>,
    previous_settings: &AppSettings,
    original_error: String,
) -> String {
    let config_rollback = match snapshot {
        Some(snapshot) => match &snapshot.bytes {
            Some(bytes) => storage::atomic_write(&snapshot.path, bytes),
            None => match fs::remove_file(&snapshot.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err("не удалось восстановить предыдущий config.toml".to_string()),
            },
        },
        None => Ok(()),
    };
    let startup_rollback = storage::set_startup(
        previous_settings.start_with_windows,
        previous_settings.start_in_tray,
    );
    if config_rollback.is_err() || startup_rollback.is_err() {
        *core
            .proxy_error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some("После неудачного сброса проверьте config.toml и автозапуск Windows.".to_string());
        format!("{original_error} Также не удалось полностью восстановить прежние настройки Codex или автозапуска. Проверьте config.toml и реестр запуска Windows.")
    } else {
        original_error
    }
}

pub async fn connect_codex(core: &Arc<AppCore>) -> Result<DashboardData, String> {
    core.require_admin()?;
    connect_codex_inner(core).await?;
    dashboard(core).await
}

async fn connect_codex_inner(core: &Arc<AppCore>) -> Result<(), String> {
    if !core.proxy_is_running() {
        return Err("Локальный маршрут не запустился на этом порту. Перезапустите Codex Accounts и повторите подключение.".to_string());
    }
    if !core.cli.is_file() {
        return Err("Для подключения нужен официальный Codex CLI. Установите Codex и перезапустите приложение.".to_string());
    }
    if core.vault.read().await.accounts.is_empty() {
        return Err("Сначала добавьте аккаунт ChatGPT.".to_string());
    }
    let (_save_guard, mut vault) = core.lock_vault_for_transaction().await;
    let previous = vault.clone();
    let config_path = core.codex_home.join("config.toml");
    let config_snapshot = snapshot_optional_file(&config_path)?;
    if let Err(error) =
        codex_config::install_proxy(&core.codex_home, &core.local_proxy_url(), &mut vault)
    {
        *vault = previous;
        return Err(error);
    }
    vault.settings.proxy_enabled = true;
    if vault.active_account_id.is_none() {
        vault.active_account_id = sorted_candidates(vault.accounts.clone(), None, 0)
            .first()
            .map(|account| account.id.clone());
    }
    if let Err(error) = storage::save(&core.root, &vault) {
        *vault = previous;
        let restore_result = restore_optional_file(&config_path, config_snapshot.as_deref());
        return match restore_result {
            Ok(()) => Err(error),
            Err(restore_error) => Err(format!(
                "{error} Не удалось восстановить config.toml после сбоя: {restore_error}"
            )),
        };
    }
    drop(vault);
    drop(_save_guard);
    *core
        .proxy_error
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    core.emit_changed();
    Ok(())
}

pub async fn disconnect_codex(core: &Arc<AppCore>) -> Result<DashboardData, String> {
    core.require_admin()?;
    {
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
                    "{error} Не удалось восстановить config.toml после сбоя: {restore_error}"
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
                    "{error} Не удалось восстановить config.toml после сбоя: {restore_error}"
                )),
            };
        }
    }
    *core
        .proxy_error
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    core.emit_changed();
    dashboard(core).await
}

pub async fn save_codex_preferences(
    core: &Arc<AppCore>,
    preferences: CodexPreferences,
) -> Result<DashboardData, String> {
    core.require_admin()?;
    codex_config::update_preferences(&core.codex_home, &preferences)?;
    dashboard(core).await
}

pub async fn save_prompt(
    core: &Arc<AppCore>,
    name: String,
    content: String,
) -> Result<DashboardData, String> {
    core.require_admin()?;
    codex_config::save_prompt(&core.codex_home, &name, &content)?;
    dashboard(core).await
}

pub async fn delete_prompt(core: &Arc<AppCore>, name: String) -> Result<DashboardData, String> {
    core.require_admin()?;
    codex_config::delete_prompt(&core.codex_home, &name)?;
    dashboard(core).await
}

pub async fn export_backup(
    core: &Arc<AppCore>,
    path: String,
    password: Option<String>,
) -> Result<(), String> {
    core.require_admin()?;
    let _refresh_guard = core.refresh_lock.lock().await;
    sync_live_auth(core).await?;
    let mut auth_account_ids = core
        .vault
        .read()
        .await
        .accounts
        .iter()
        .map(|account| account.account_id.clone())
        .collect::<Vec<_>>();
    auth_account_ids.sort();
    auth_account_ids.dedup();
    let mut _auth_guards = Vec::with_capacity(auth_account_ids.len());
    for account_id in auth_account_ids {
        _auth_guards.push(core.account_auth_lock(&account_id).lock_owned().await);
    }
    let vault = core.vault.read().await.clone();
    let mut accounts = Vec::new();
    for account in &vault.accounts {
        let auth = storage::open_auth(&account.sealed_auth)?;
        accounts.push(BackupAccount {
            account_id: account.account_id.clone(),
            email: account.email.clone(),
            plan: account.plan.clone(),
            created_at: account.created_at,
            last_used_at: account.last_used_at,
            usage: account.usage.clone(),
            status: account.status.clone(),
            auth_json: String::from_utf8(auth)
                .map_err(|_| "Данные входа нельзя представить в резервной копии.".to_string())?,
        });
    }
    let mut config = codex_config::read_config(&core.codex_home)?;
    if vault.proxy_configured {
        config = codex_config::restore_proxy_text(&config, &core.local_proxy_url(), &vault)?;
    }
    let payload = BackupPayload {
        format: "codex-accounts-backup".to_string(),
        version: 1,
        exported_at: now_timestamp(),
        accounts,
        settings: vault.settings,
        codex_config_toml: config,
        prompts: codex_config::list_prompts(&core.codex_home)?,
    };
    let password = password.map(Zeroizing::new);
    let encoded = storage::encode_backup(
        &payload,
        password.as_deref().map(|password| password.as_str()),
    )?;
    storage::atomic_write(&PathBuf::from(path), &encoded)
}

pub async fn import_backup(
    core: &Arc<AppCore>,
    path: String,
    password: Option<String>,
) -> Result<DashboardData, String> {
    core.require_admin()?;
    let bytes = fs::read(&path)
        .map_err(|_| "Не удалось прочитать выбранную резервную копию.".to_string())?;
    let password = password.map(Zeroizing::new);
    let payload = storage::decode_backup(
        &bytes,
        password.as_deref().map(|password| password.as_str()),
    )?;
    if payload.format != "codex-accounts-backup" || payload.version != 1 {
        return Err("Эта резервная копия создана в несовместимом формате.".to_string());
    }
    let backup_settings = payload.settings;
    validate_import_settings(&backup_settings)?;
    let backup_config = payload.codex_config_toml;
    let backup_prompts = payload.prompts;
    let mut accounts = Vec::new();
    for item in payload.accounts {
        let auth_bytes = item.auth_json.as_bytes();
        let identity = codex::identity_from_auth(auth_bytes)?;
        if identity.account_id != item.account_id {
            return Err("В резервной копии id аккаунта не совпадает с токеном входа.".to_string());
        }
        accounts.push(AccountProfile {
            id: uuid::Uuid::new_v4().to_string(),
            account_id: item.account_id,
            email: item.email,
            plan: item.plan,
            created_at: item.created_at,
            last_used_at: item.last_used_at,
            usage: item.usage,
            status: item.status,
            sealed_auth: storage::seal_auth(auth_bytes)?,
        });
    }
    if !backup_config.trim().is_empty() {
        backup_config.parse::<toml_edit::DocumentMut>()
            .map_err(|_| "config.toml в копии содержит синтаксическую ошибку. Импорт остановлен без изменений.".to_string())?;
    }
    codex_config::validate_prompts(&backup_prompts)?;
    if backup_settings.proxy_enabled && accounts.is_empty() {
        return Err("В резервной копии включён локальный маршрут, но нет аккаунтов. Импорт остановлен без изменений.".to_string());
    }

    let _refresh_guard = core.refresh_lock.lock().await;
    let old_vault = core.vault.read().await.clone();
    let mut auth_account_ids = core
        .vault
        .read()
        .await
        .accounts
        .iter()
        .map(|account| account.account_id.clone())
        .collect::<Vec<_>>();
    auth_account_ids.extend(accounts.iter().map(|account| account.account_id.clone()));
    auth_account_ids.sort();
    auth_account_ids.dedup();
    let mut auth_guards = Vec::with_capacity(auth_account_ids.len());
    for account_id in auth_account_ids {
        auth_guards.push(core.account_auth_lock(&account_id).lock_owned().await);
    }

    let config_path = core.codex_home.join("config.toml");
    let old_config = snapshot_optional_file(&config_path)?;
    let old_prompts = codex_config::list_prompts(&core.codex_home)?;
    let mut replacement = old_vault.clone();
    replacement.accounts = merge_accounts(old_vault.accounts.clone(), accounts);
    replacement.settings = backup_settings;
    let refresh_interval = replacement.settings.refresh_interval_seconds;
    replacement.proxy_configured = false;
    replacement.original_openai_base_url = None;
    replacement.original_openai_base_url_set = false;
    replacement.original_model_provider = None;
    replacement.original_model_provider_set = false;
    replacement.active_account_id = replacement
        .accounts
        .first()
        .map(|account| account.id.clone());

    let new_config = if backup_config.trim().is_empty() {
        None
    } else {
        Some(backup_config.into_bytes())
    };
    write_optional_file(&config_path, new_config.as_deref())?;
    if let Err(error) = codex_config::replace_prompts(&core.codex_home, &backup_prompts) {
        return Err(rollback_import(
            core,
            &config_path,
            old_config.as_deref(),
            &old_prompts,
            &old_vault.settings,
            error,
        ));
    }
    if let Err(error) = storage::set_startup(
        replacement.settings.start_with_windows,
        replacement.settings.start_in_tray,
    ) {
        return Err(rollback_import(
            core,
            &config_path,
            old_config.as_deref(),
            &old_prompts,
            &old_vault.settings,
            error,
        ));
    }
    if replacement.settings.proxy_enabled {
        if let Err(error) =
            codex_config::install_proxy(&core.codex_home, &core.local_proxy_url(), &mut replacement)
        {
            return Err(rollback_import(
                core,
                &config_path,
                old_config.as_deref(),
                &old_prompts,
                &old_vault.settings,
                error,
            ));
        }
        if replacement.active_account_id.is_none() {
            replacement.active_account_id =
                sorted_candidates(replacement.accounts.clone(), None, 0)
                    .first()
                    .map(|account| account.id.clone());
        }
    }
    {
        let mut vault = core.vault.write().await;
        *vault = replacement;
    }
    if let Err(error) = core.save_vault().await {
        {
            let mut vault = core.vault.write().await;
            *vault = old_vault.clone();
        }
        return Err(rollback_import(
            core,
            &config_path,
            old_config.as_deref(),
            &old_prompts,
            &old_vault.settings,
            error,
        ));
    }
    core.set_refresh_interval(refresh_interval);
    core.emit_changed();
    drop(auth_guards);
    dashboard(core).await
}

fn validate_import_settings(settings: &AppSettings) -> Result<(), String> {
    if settings.switch_threshold > 100 {
        return Err("Порог предупреждения в резервной копии должен быть от 0 до 100%. Импорт остановлен без изменений.".to_string());
    }
    if !(1..=86_400).contains(&settings.refresh_interval_seconds) {
        return Err("Интервал проверки в резервной копии должен быть от 1 секунды до 24 часов. Импорт остановлен без изменений.".to_string());
    }
    if !matches!(settings.theme.as_str(), "system" | "dark" | "light") {
        return Err(
            "Тема в резервной копии не распознана. Импорт остановлен без изменений.".to_string(),
        );
    }
    Ok(())
}

fn snapshot_optional_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => {
            Err("Не удалось сохранить текущий файл перед импортом резервной копии.".to_string())
        }
    }
}

fn write_optional_file(path: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
    if let Some(bytes) = bytes {
        return storage::atomic_write(path, bytes);
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            Err("Не удалось удалить config.toml согласно выбранной резервной копии.".to_string())
        }
    }
}

fn restore_optional_file(path: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
    if let Some(bytes) = bytes {
        return storage::atomic_write(path, bytes)
            .map_err(|_| "Не удалось вернуть исходный файл.".to_string());
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("не удалось удалить изменённый файл".to_string()),
    }
}

fn rollback_import(
    core: &AppCore,
    config_path: &Path,
    old_config: Option<&[u8]>,
    old_prompts: &[crate::model::PromptView],
    old_settings: &AppSettings,
    original_error: String,
) -> String {
    let mut restore_errors = Vec::new();
    if write_optional_file(config_path, old_config).is_err() {
        restore_errors.push("config.toml");
    }
    if codex_config::replace_prompts(&core.codex_home, old_prompts).is_err() {
        restore_errors.push("prompts");
    }
    if storage::set_startup(old_settings.start_with_windows, old_settings.start_in_tray).is_err() {
        restore_errors.push("автозапуск Windows");
    }
    if restore_errors.is_empty() {
        original_error
    } else {
        format!(
            "{original_error} Не удалось полностью восстановить: {}. Проверьте исходные файлы и автозапуск Windows.",
            restore_errors.join(", ")
        )
    }
}

pub async fn shutdown(core: &Arc<AppCore>) -> Result<(), String> {
    super::restore_proxy_before_exit(core).await?;
    core.lock_admin();
    core.request_shutdown();
    Ok(())
}

fn merge_accounts(
    mut current: Vec<AccountProfile>,
    imported: Vec<AccountProfile>,
) -> Vec<AccountProfile> {
    for account in imported {
        if let Some(existing) = current
            .iter_mut()
            .find(|existing| existing.account_id == account.account_id)
        {
            existing.email = account.email;
            existing.plan = account.plan;
            existing.sealed_auth = account.sealed_auth;
            existing.last_used_at = account.last_used_at;
            existing.usage = account.usage;
            existing.status = account.status;
        } else {
            current.push(account);
        }
    }
    current
}

pub fn start_refresh_loop(core: Arc<AppCore>, runtime: &tokio::runtime::Handle) {
    runtime.spawn(async move {
        let mut interval = core.subscribe_refresh_interval();
        let mut shutdown = core.subscribe_shutdown();
        if *shutdown.borrow_and_update() {
            return;
        }
        let _ = refresh_all_inner(&core).await;
        if core.vault.read().await.active_account_id.is_none()
            && core.proxy_is_running()
            && core.proxy_error_message().is_none()
        {
            let _ = core.select_active().await;
        }
        loop {
            if *shutdown.borrow_and_update() {
                return;
            }
            let seconds = u64::from(*interval.borrow_and_update());
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(seconds)) => {}
                changed = interval.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    continue;
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
            let _ = refresh_all_inner(&core).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account_view(id: &str, remaining: Option<i32>, created_at: i64) -> AccountView {
        AccountView {
            id: id.to_string(),
            account_id: format!("account-{id}"),
            email: format!("{id}@example.test"),
            plan: "Plus".to_string(),
            created_at,
            last_used_at: None,
            usage: remaining.map(|remaining| UsageSnapshot {
                primary: Some(crate::model::UsageWindow {
                    used_percent: 100 - remaining,
                    window_minutes: Some(300),
                    resets_at: Some(2_000_000_000),
                }),
                secondary: None,
                updated_at: 1_900_000_000,
                error: None,
            }),
            status: crate::model::AccountStatus::healthy(),
            is_active: false,
        }
    }

    #[test]
    fn dashboard_account_sort_places_known_highest_usage_first_and_unknown_last() {
        let mut accounts = vec![
            account_view("unknown-old", None, 1),
            account_view("low", Some(10), 2),
            account_view("high", Some(90), 3),
            account_view("unknown-new", None, 4),
        ];

        sort_accounts_by_usage(&mut accounts);

        let ids = accounts
            .into_iter()
            .map(|account| account.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, ["high", "low", "unknown-old", "unknown-new"]);
    }

    #[test]
    fn backup_settings_accept_supported_interval_and_threshold_ranges() {
        let defaults = AppSettings::default();
        assert!(validate_import_settings(&defaults).is_ok());

        let shortest_interval = AppSettings {
            refresh_interval_seconds: 1,
            switch_threshold: 100,
            ..defaults.clone()
        };
        assert!(validate_import_settings(&shortest_interval).is_ok());

        let longest_interval = AppSettings {
            refresh_interval_seconds: 86_400,
            ..defaults
        };
        assert!(validate_import_settings(&longest_interval).is_ok());
    }

    #[test]
    fn backup_settings_reject_out_of_range_values() {
        let invalid_interval = AppSettings {
            refresh_interval_seconds: 0,
            ..AppSettings::default()
        };
        assert!(validate_import_settings(&invalid_interval).is_err());

        let too_long_interval = AppSettings {
            refresh_interval_seconds: 86_401,
            ..AppSettings::default()
        };
        assert!(validate_import_settings(&too_long_interval).is_err());

        let invalid_threshold = AppSettings {
            switch_threshold: 101,
            ..AppSettings::default()
        };
        assert!(validate_import_settings(&invalid_threshold).is_err());
    }

    #[test]
    fn legacy_backup_account_defaults_new_usage_fields() {
        let account: BackupAccount = serde_json::from_value(serde_json::json!({
            "accountId": "legacy-id",
            "email": "legacy@example.test",
            "plan": "Plus",
            "createdAt": 1,
            "authJson": "{}"
        }))
        .expect("deserialize legacy backup account");

        assert!(account.last_used_at.is_none());
        assert!(account.usage.is_none());
    }

    #[test]
    fn backup_account_roundtrip_preserves_usage_snapshot() {
        let account = BackupAccount {
            account_id: "usage-id".to_string(),
            email: "usage@example.test".to_string(),
            plan: "Plus".to_string(),
            created_at: 1,
            last_used_at: Some(2),
            usage: Some(UsageSnapshot {
                primary: Some(crate::model::UsageWindow {
                    used_percent: 68,
                    window_minutes: Some(300),
                    resets_at: Some(3),
                }),
                secondary: None,
                updated_at: 4,
                error: None,
            }),
            status: crate::model::AccountStatus::healthy(),
            auth_json: "{}".to_string(),
        };

        let encoded = serde_json::to_value(&account).expect("serialize backup account");
        let decoded: BackupAccount =
            serde_json::from_value(encoded).expect("deserialize backup account");

        assert_eq!(decoded.last_used_at, Some(2));
        let usage = decoded.usage.expect("usage snapshot");
        let primary = usage.primary.expect("primary usage window");
        assert_eq!(primary.used_percent, 68);
        assert_eq!(primary.resets_at, Some(3));
        assert_eq!(usage.updated_at, 4);
    }

    #[test]
    fn merging_backup_keeps_saved_usage_snapshot() {
        let current = AccountProfile {
            id: "existing-profile".to_string(),
            account_id: "shared-id".to_string(),
            email: "old@example.test".to_string(),
            plan: "Plus".to_string(),
            created_at: 1,
            last_used_at: None,
            usage: None,
            status: crate::model::AccountStatus::healthy(),
            sealed_auth: "old-auth".to_string(),
        };
        let imported = AccountProfile {
            id: "imported-profile".to_string(),
            account_id: "shared-id".to_string(),
            email: "new@example.test".to_string(),
            plan: "Plus".to_string(),
            created_at: 2,
            last_used_at: Some(3),
            usage: Some(UsageSnapshot {
                primary: Some(crate::model::UsageWindow {
                    used_percent: 42,
                    window_minutes: Some(300),
                    resets_at: Some(4),
                }),
                secondary: None,
                updated_at: 5,
                error: None,
            }),
            status: crate::model::AccountStatus::healthy(),
            sealed_auth: "new-auth".to_string(),
        };

        let merged = merge_accounts(vec![current], vec![imported]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "existing-profile");
        assert_eq!(merged[0].last_used_at, Some(3));
        assert_eq!(
            merged[0]
                .usage
                .as_ref()
                .and_then(UsageSnapshot::highest_remaining),
            Some(58)
        );
    }
}
