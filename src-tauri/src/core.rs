use crate::{
    codex,
    model::{
        AccountProfile, AccountStatus, AccountStatusKind, UsageSnapshot, UsageWindow, VaultDocument,
    },
    storage,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{watch, Mutex as AsyncMutex, RwLock};

const OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OAUTH_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";

pub struct AppCore {
    pub root: PathBuf,
    pub codex_home: PathBuf,
    pub cli: PathBuf,
    pub proxy_port: u16,
    pub proxy_access_key: String,
    pub vault: RwLock<VaultDocument>,
    pub http: reqwest::Client,
    auth_http: reqwest::Client,
    auth_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    save_lock: AsyncMutex<()>,
    pub(crate) refresh_lock: AsyncMutex<()>,
    proxy_started: AtomicBool,
    refresh_interval_tx: watch::Sender<u32>,
    dashboard_revision_tx: watch::Sender<u64>,
    change_notifier: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    shutdown_tx: watch::Sender<bool>,
    response_owners: Mutex<storage::ResponseOwnerIndex>,
    response_owner_save_lock: Mutex<()>,
    pub proxy_error: Mutex<Option<String>>,
    admin_protected: AtomicBool,
    admin_unlocked: AtomicBool,
    add_account_running: AtomicBool,
    add_account_cancel: Arc<AtomicBool>,
}

impl AppCore {
    pub fn new(
        root: PathBuf,
        codex_home: PathBuf,
        cli: PathBuf,
        proxy_port: u16,
    ) -> Result<Arc<Self>, String> {
        storage::ensure_vault(&root)?;
        let mut vault = storage::load(&root)?;
        let refresh_interval = vault.settings.refresh_interval_seconds.clamp(1, 86_400);
        let mut changed = false;
        if refresh_interval != vault.settings.refresh_interval_seconds {
            vault.settings.refresh_interval_seconds = refresh_interval;
            changed = true;
        }
        if vault.proxy_access_key.is_none() {
            vault.proxy_access_key = Some(uuid::Uuid::new_v4().simple().to_string());
            changed = true;
        }
        if changed {
            storage::save(&root, &vault)?;
        }
        let proxy_access_key = vault
            .proxy_access_key
            .clone()
            .ok_or_else(|| "Не удалось создать локальный ключ прокси Codex.".to_string())?;
        let response_owners = storage::load_response_owners(&root);
        let (refresh_interval_tx, _) = watch::channel(refresh_interval);
        let (dashboard_revision_tx, _) = watch::channel(0);
        let (shutdown_tx, _) = watch::channel(false);
        let admin_protected = vault.admin_password_hash.is_some();
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|_| "Не удалось подготовить сетевое подключение Codex.".to_string())?;
        // Refresh tokens are sent only to the fixed OAuth endpoint. Do not
        // follow redirects with a credential-bearing form body, and bound this
        // request independently from streamed Codex responses.
        let auth_http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| {
                "Не удалось подготовить безопасное подключение для обновления входа.".to_string()
            })?;
        Ok(Arc::new(Self {
            root,
            codex_home,
            cli,
            proxy_port,
            proxy_access_key,
            vault: RwLock::new(vault),
            http,
            auth_http,
            auth_locks: Mutex::new(HashMap::new()),
            save_lock: AsyncMutex::new(()),
            refresh_lock: AsyncMutex::new(()),
            proxy_started: AtomicBool::new(false),
            refresh_interval_tx,
            dashboard_revision_tx,
            change_notifier: Mutex::new(None),
            shutdown_tx,
            response_owners: Mutex::new(response_owners),
            response_owner_save_lock: Mutex::new(()),
            proxy_error: Mutex::new(None),
            admin_protected: AtomicBool::new(admin_protected),
            admin_unlocked: AtomicBool::new(!admin_protected),
            add_account_running: AtomicBool::new(false),
            add_account_cancel: Arc::new(AtomicBool::new(false)),
        }))
    }

    pub fn admin_protected(&self) -> bool {
        self.admin_protected.load(Ordering::Acquire)
    }

    pub fn admin_unlocked(&self) -> bool {
        self.admin_unlocked.load(Ordering::Acquire)
    }

    pub fn require_admin(&self) -> Result<(), String> {
        if self.admin_unlocked() {
            Ok(())
        } else {
            Err("ADMIN_LOCKED: введите пароль администратора для доступа к данным.".to_string())
        }
    }

    /// Reserve the single interactive login slot.  A browser login must not
    /// be started twice because both flows would race over temporary Codex
    /// state and make the result ambiguous.
    pub fn begin_add_account(&self) -> bool {
        if self.add_account_running.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.add_account_cancel.store(false, Ordering::Release);
        true
    }

    pub fn finish_add_account(&self) {
        self.add_account_cancel.store(false, Ordering::Release);
        self.add_account_running.store(false, Ordering::Release);
    }

    pub fn add_account_is_running(&self) -> bool {
        self.add_account_running.load(Ordering::Acquire)
    }

    pub fn cancel_add_account(&self) {
        self.add_account_cancel.store(true, Ordering::Release);
    }

    pub fn add_account_cancel_token(&self) -> Arc<AtomicBool> {
        self.add_account_cancel.clone()
    }

    pub fn lock_admin(&self) {
        if self.admin_protected() && self.admin_unlocked.swap(false, Ordering::AcqRel) {
            self.emit_changed();
        }
    }

    pub fn update_admin_state(&self, protected: bool, unlocked: bool) {
        self.admin_protected.store(protected, Ordering::Release);
        self.admin_unlocked.store(unlocked, Ordering::Release);
    }

    pub fn set_refresh_interval(&self, seconds: u32) {
        self.refresh_interval_tx
            .send_replace(seconds.clamp(1, 86_400));
    }

    pub fn subscribe_refresh_interval(&self) -> watch::Receiver<u32> {
        self.refresh_interval_tx.subscribe()
    }

    pub fn emit_changed(&self) {
        let revision = self.dashboard_revision_tx.borrow().saturating_add(1);
        self.dashboard_revision_tx.send_replace(revision);
        let notifier = self
            .change_notifier
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if let Some(notifier) = notifier {
            notifier();
        }
    }

    pub fn set_change_notifier(&self, notifier: Arc<dyn Fn() + Send + Sync>) {
        *self
            .change_notifier
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(notifier);
    }

    pub fn subscribe_dashboard_changes(&self) -> watch::Receiver<u64> {
        self.dashboard_revision_tx.subscribe()
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    pub fn request_shutdown(&self) {
        self.shutdown_tx.send_replace(true);
    }

    pub fn response_owner(&self, response_id: &str) -> Option<String> {
        self.response_owners
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .owners
            .get(response_id)
            .cloned()
    }

    pub fn clear_response_owners(&self) {
        {
            let mut cache = self
                .response_owners
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            cache.owners.clear();
            cache.insertion_order.clear();
        }
        self.persist_response_owners();
    }

    pub fn remember_response_owner(&self, response_id: &str, account_id: &str) {
        {
            let mut owners = self
                .response_owners
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if owners
                .owners
                .get(response_id)
                .is_some_and(|owner| owner == account_id)
            {
                return;
            }
            if !owners.owners.contains_key(response_id) {
                owners.insertion_order.push_back(response_id.to_string());
            }
            owners
                .owners
                .insert(response_id.to_string(), account_id.to_string());
            while owners.owners.len() > storage::RESPONSE_OWNER_LIMIT {
                let Some(oldest) = owners.insertion_order.pop_front() else {
                    break;
                };
                owners.owners.remove(&oldest);
            }
        }
        self.persist_response_owners();
    }

    fn persist_response_owners(&self) {
        // Serialize the disk writes separately from the in-memory index. Read
        // the latest snapshot only after acquiring this lock so concurrent
        // responses cannot finish their atomic writes out of order and replace
        // a newer cache with a stale copy.
        let _save_guard = self
            .response_owner_save_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let snapshot = self
            .response_owners
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        // A cache write failure must not interrupt a live response. In that
        // case, unknown owners are still rejected rather than misrouted.
        let _ = storage::save_response_owners(&self.root, &snapshot);
    }

    pub fn local_proxy_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/{}/backend-api/codex",
            self.proxy_port, self.proxy_access_key
        )
    }

    pub fn proxy_error_message(&self) -> Option<String> {
        self.proxy_error
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn proxy_is_running(&self) -> bool {
        self.proxy_started.load(Ordering::Acquire)
    }

    pub fn set_proxy_running(&self, running: bool) {
        self.proxy_started.store(running, Ordering::Release);
    }

    pub async fn save_vault(&self) -> Result<(), String> {
        let _guard = self.save_lock.lock().await;
        let document = self.vault.read().await.clone();
        storage::save(&self.root, &document)
    }

    /// Serialize an administrative operation that must replace the vault and
    /// keep its in-memory state consistent with the persisted document.
    pub async fn lock_vault_for_transaction(
        &self,
    ) -> (
        tokio::sync::MutexGuard<'_, ()>,
        tokio::sync::RwLockWriteGuard<'_, VaultDocument>,
    ) {
        let save_guard = self.save_lock.lock().await;
        let vault_guard = self.vault.write().await;
        (save_guard, vault_guard)
    }

    pub fn account_auth_lock(&self, account_id: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self.auth_locks.lock().unwrap_or_else(|e| e.into_inner());
        locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    pub async fn set_active(&self, id: &str) -> Result<AccountProfile, String> {
        let (_save_guard, mut vault) = self.lock_vault_for_transaction().await;
        let previous = vault.clone();
        let account = vault
            .accounts
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| {
                "Этот аккаунт больше не находится в списке. Обновите экран.".to_string()
            })?;
        let now = now_timestamp();
        validate_manual_switch_target(account, now)?;
        account.last_used_at = Some(now);
        let selected = account.clone();
        vault.active_account_id = Some(id.to_string());
        if let Err(error) = storage::save(&self.root, &vault) {
            *vault = previous;
            return Err(error);
        }
        drop(vault);
        drop(_save_guard);
        self.emit_changed();
        Ok(selected)
    }

    pub async fn select_active(&self) -> Result<AccountProfile, String> {
        let (active_id, accounts, automatic_profile_switching) = {
            let vault = self.vault.read().await;
            (
                vault.active_account_id.clone(),
                vault.accounts.clone(),
                vault.settings.automatic_profile_switching,
            )
        };
        let selected = select_request_account_with_mode(
            &accounts,
            active_id.as_deref(),
            automatic_profile_switching,
        )?;
        if active_id.as_deref() != Some(selected.id.as_str()) {
            return self.set_active(&selected.id).await;
        }
        Ok(selected)
    }

    pub async fn mark_usage_limit(&self, id: &str, resets_at: Option<i64>) -> Result<(), String> {
        let now = now_timestamp();
        {
            let (_save_guard, mut vault) = self.lock_vault_for_transaction().await;
            let previous = vault.clone();
            let Some(account_index) = vault.accounts.iter().position(|account| account.id == id)
            else {
                return Ok(());
            };
            {
                let account = &mut vault.accounts[account_index];
                let usage = account.usage.get_or_insert(UsageSnapshot {
                    primary: None,
                    secondary: None,
                    updated_at: now,
                    error: None,
                });
                mark_exhausted_usage_window(usage, resets_at, now);
                usage.updated_at = now;
                usage.error = None;
                account.status = AccountStatus::new(
                    AccountStatusKind::UsageLimit,
                    "Лимит этого окна исчерпан; профиль вернётся после сброса.",
                );
            }

            // Only change the selected profile if the exhausted one is still
            // active. A late 429 from an older chat must not override a
            // profile the user has selected in the meantime.
            let next = if vault.settings.automatic_profile_switching {
                auto_switch_candidate(&vault.accounts, vault.active_account_id.as_deref(), id, now)
            } else {
                None
            };
            if let Some(next) = next {
                vault.active_account_id = Some(next.id.clone());
                if let Some(account) = vault
                    .accounts
                    .iter_mut()
                    .find(|account| account.id == next.id)
                {
                    account.last_used_at = Some(now);
                }
            }
            if let Err(error) = storage::save(&self.root, &vault) {
                *vault = previous;
                return Err(error);
            }
        }
        self.emit_changed();
        Ok(())
    }

    /// Persist a routing-relevant authentication state discovered while a
    /// live Codex request is being served. The proxy treats this as best
    /// effort so a vault write failure cannot hide the upstream error.
    pub async fn mark_account_status(
        &self,
        id: &str,
        kind: AccountStatusKind,
        detail: impl Into<String>,
    ) -> Result<(), String> {
        {
            let (_save_guard, mut vault) = self.lock_vault_for_transaction().await;
            let previous = vault.clone();
            let Some(account) = vault.accounts.iter_mut().find(|account| account.id == id) else {
                return Ok(());
            };
            account.status = AccountStatus::new(kind, detail);
            if let Some(usage) = account.usage.as_mut() {
                usage.error = account.status.detail.clone();
            }
            if let Err(error) = storage::save(&self.root, &vault) {
                *vault = previous;
                return Err(error);
            }
        }
        self.emit_changed();
        Ok(())
    }

    pub async fn access_token(&self, account: &AccountProfile) -> Result<(String, String), String> {
        let lock = self.account_auth_lock(&account.account_id);
        let _guard = lock.lock().await;

        let mut auth = {
            let vault = self.vault.read().await;
            let stored = vault
                .accounts
                .iter()
                .find(|entry| entry.id == account.id)
                .ok_or_else(|| {
                    "Аккаунт удалён во время подключения. Обновите экран.".to_string()
                })?;
            serde_json::from_slice::<Value>(&storage::open_auth(&stored.sealed_auth)?)
                .map_err(|_| "Авторизация аккаунта повреждена. Добавьте его заново.".to_string())?
        };
        let identity = codex::identity_from_auth(&serde_json::to_vec(&auth).unwrap_or_default())?;

        // Codex rotates the refresh token in its live profile. Read a newer
        // same-account login before refreshing the saved pool copy.
        if let Some(live) = self.newer_live_auth(&identity.account_id, &auth) {
            auth = live;
            self.store_auth_for_account(&account.id, &auth).await?;
        }
        let mut access = auth
            .pointer("/tokens/access_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| "В аккаунте нет токена доступа. Добавьте его заново.".to_string())?
            .to_string();
        let expires_at = jwt_expiry(&access).unwrap_or_default();
        if expires_at <= now_timestamp() + 180 {
            auth = self.refresh_login(&account.id, &auth).await?;
            access = auth
                .pointer("/tokens/access_token")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .ok_or_else(|| {
                    "Обновление входа не вернуло токен доступа. Войдите в аккаунт заново."
                        .to_string()
                })?
                .to_string();
        }
        let identity = codex::identity_from_auth(&serde_json::to_vec(&auth).unwrap_or_default())?;
        Ok((access, identity.account_id))
    }

    fn newer_live_auth(&self, account_id: &str, stored: &Value) -> Option<Value> {
        let live_bytes = fs::read(self.codex_home.join("auth.json")).ok()?;
        let live = serde_json::from_slice::<Value>(&live_bytes).ok()?;
        let identity = codex::identity_from_auth(&live_bytes).ok()?;
        if identity.account_id != account_id {
            return None;
        }
        auth_is_newer(&live, stored).then_some(live)
    }

    async fn refresh_login(&self, id: &str, auth: &Value) -> Result<Value, String> {
        let expected_account_id =
            codex::identity_from_auth(&serde_json::to_vec(auth).unwrap_or_default())?.account_id;
        let refresh_token = auth
            .pointer("/tokens/refresh_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "Срок входа истёк. Нажмите «Добавить аккаунт» и войдите снова.".to_string()
            })?;
        let response = self
            .auth_http
            .post(OAUTH_TOKEN_ENDPOINT)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", OAUTH_CLIENT_ID),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await
            .map_err(|_| "Не удалось обновить вход. Проверьте интернет и повторите.".to_string())?;
        if !response.status().is_success() {
            // Another Codex process may have rotated this token a moment ago.
            let live = fs::read(self.codex_home.join("auth.json")).ok();
            if let Some(live) = live {
                if codex::identity_from_auth(&live)
                    .ok()
                    .is_some_and(|identity| identity.account_id == expected_account_id)
                {
                    if let Ok(updated) = serde_json::from_slice::<Value>(&live) {
                        if updated
                            .pointer("/tokens/access_token")
                            .and_then(Value::as_str)
                            .and_then(jwt_expiry)
                            .unwrap_or_default()
                            > now_timestamp() + 180
                        {
                            self.store_auth_for_account(id, &updated).await?;
                            return Ok(updated);
                        }
                    }
                }
            }
            return Err("OpenAI отклонил обновление входа. Проверьте подключение или добавьте аккаунт заново.".to_string());
        }
        let response: Value = response.json().await.map_err(|_| {
            "Сервис входа вернул нечитаемый ответ. Попробуйте добавить аккаунт заново.".to_string()
        })?;
        let mut refreshed = auth.clone();
        let tokens = refreshed
            .get_mut("tokens")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "В данных аккаунта не найдена секция tokens.".to_string())?;
        for key in ["access_token", "refresh_token", "id_token"] {
            if let Some(value) = response.get(key).and_then(Value::as_str) {
                tokens.insert(key.to_string(), json!(value));
            }
        }
        refreshed["last_refresh"] = json!(chrono::Utc::now().to_rfc3339());

        // Codex owns its live auth.json. If it rotated the same account while
        // this request was in flight, keep that copy and update only our vault.
        if let Ok(live_bytes) = fs::read(self.codex_home.join("auth.json")) {
            if codex::identity_from_auth(&live_bytes)
                .ok()
                .is_some_and(|identity| identity.account_id == expected_account_id)
            {
                if let Ok(live) = serde_json::from_slice::<Value>(&live_bytes) {
                    if auth_is_newer(&live, &refreshed) {
                        self.store_auth_for_account(id, &live).await?;
                        return Ok(live);
                    }
                }
            }
        }
        self.store_auth_for_account(id, &refreshed).await?;
        Ok(refreshed)
    }

    async fn store_auth_for_account(&self, id: &str, auth: &Value) -> Result<(), String> {
        let bytes = serde_json::to_vec(auth)
            .map_err(|_| "Не удалось сохранить обновлённый вход.".to_string())?;
        let sealed = storage::seal_auth(&bytes)?;
        let (_save_guard, mut vault) = self.lock_vault_for_transaction().await;
        let previous = {
            let account = vault
                .accounts
                .iter_mut()
                .find(|account| account.id == id)
                .ok_or_else(|| "Аккаунт удалён во время обновления входа.".to_string())?;
            std::mem::replace(&mut account.sealed_auth, sealed)
        };
        if let Err(error) = storage::save(&self.root, &vault) {
            if let Some(account) = vault.accounts.iter_mut().find(|account| account.id == id) {
                account.sealed_auth = previous;
            }
            return Err(error);
        }
        drop(vault);
        self.emit_changed();
        Ok(())
    }
}

fn mark_exhausted_usage_window(usage: &mut UsageSnapshot, resets_at: Option<i64>, now: i64) {
    let primary_reset_distance = usage
        .primary
        .as_ref()
        .and_then(|window| window.resets_at)
        .zip(resets_at)
        .map(|(existing, reported)| existing.abs_diff(reported));
    let secondary_reset_distance = usage
        .secondary
        .as_ref()
        .and_then(|window| window.resets_at)
        .zip(resets_at)
        .map(|(existing, reported)| existing.abs_diff(reported));
    let mark_primary = match (primary_reset_distance, secondary_reset_distance) {
        (Some(primary), Some(secondary)) => primary <= secondary,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => match (
            usage
                .primary
                .as_ref()
                .map(|window| window.remaining_percent_at(now)),
            usage
                .secondary
                .as_ref()
                .map(|window| window.remaining_percent_at(now)),
        ) {
            (Some(primary), Some(secondary)) => primary <= secondary,
            (Some(_), None) | (None, None) => true,
            (None, Some(_)) => false,
        },
    };
    let window = if mark_primary {
        &mut usage.primary
    } else {
        &mut usage.secondary
    };
    let exhausted = window.get_or_insert(UsageWindow {
        used_percent: 100,
        window_minutes: None,
        resets_at,
    });
    exhausted.used_percent = 100;
    if let Some(reset) = resets_at {
        exhausted.resets_at = Some(reset);
    } else if exhausted.resets_at.is_some_and(|reset| reset <= now) {
        // A fresh 429 supersedes an elapsed cached reset. Keeping that stale
        // timestamp would make this confirmed exhausted window look restored.
        exhausted.resets_at = None;
    }
}

fn validate_manual_switch_target(account: &AccountProfile, now: i64) -> Result<(), String> {
    if account.status.kind.is_permanently_unavailable() {
        return Err(format!(
            "Аккаунт недоступен: {}",
            account
                .status
                .detail
                .as_deref()
                .unwrap_or("потребуется повторный вход")
        ));
    }

    let remaining = account
        .usage
        .as_ref()
        .and_then(|usage| usage.lowest_remaining_at(now));
    if remaining == Some(0) {
        return Err(
            "У выбранного профиля исчерпано одно из окон лимита. Обновите данные или дождитесь сброса."
                .to_string(),
        );
    }
    if account.status.kind == AccountStatusKind::UsageLimit && remaining.is_none() {
        return Err(
            "Остаток лимита этого профиля неизвестен. Обновите данные перед переключением."
                .to_string(),
        );
    }

    Ok(())
}

#[cfg(test)]
fn select_request_account(
    accounts: &[AccountProfile],
    active_id: Option<&str>,
) -> Result<AccountProfile, String> {
    select_request_account_with_mode(accounts, active_id, true)
}

fn select_request_account_with_mode(
    accounts: &[AccountProfile],
    active_id: Option<&str>,
    automatic_profile_switching: bool,
) -> Result<AccountProfile, String> {
    if accounts.is_empty() {
        return Err("Сначала добавьте хотя бы один аккаунт ChatGPT.".to_string());
    }

    if let Some(active) = active_id.and_then(|id| accounts.iter().find(|account| account.id == id))
    {
        if active.status.kind.is_permanently_unavailable() {
            return Err(format!(
                "Текущий аккаунт недоступен: {}. Исправьте вход или выберите другой профиль вручную.",
                active.status.detail.as_deref().unwrap_or("проверьте статус аккаунта")
            ));
        }

        // A confirmed exhausted profile can remain active when no backup was
        // known to be usable at the time of the 429. Do not keep sending new
        // requests into that profile; switch only to another profile whose
        // cached remaining quota is known and positive.
        if active.status.kind == AccountStatusKind::UsageLimit
            && !active
                .usage
                .as_ref()
                .and_then(|usage| usage.lowest_remaining_at(now_timestamp()))
                .is_some_and(|remaining| remaining > 0)
        {
            if !automatic_profile_switching {
                return Err(
                    "Лимит выбранного профиля исчерпан. Автоматическое переключение отключено; выберите другой доступный профиль вручную."
                        .to_string(),
                );
            }
            if let Some(next) =
                known_available_candidate(accounts, Some(&active.id), now_timestamp())
            {
                return Ok(next);
            }
            return Err(
                "Лимит текущего профиля исчерпан, а у других профилей нет проверенного остатка. Обновите лимиты или выберите профиль вручную после проверки."
                    .to_string(),
            );
        }

        return Ok(active.clone());
    }

    let next = sorted_candidates(accounts.to_vec(), active_id, 0)
        .into_iter()
        .next();
    next.ok_or_else(|| {
        "Нет аккаунта с доступным лимитом. Обновите данные, повторите вход или дождитесь сброса."
            .to_string()
    })
}

pub fn sorted_candidates(
    mut accounts: Vec<AccountProfile>,
    exclude: Option<&str>,
    threshold: i32,
) -> Vec<AccountProfile> {
    let now = now_timestamp();
    accounts = sorted_candidates_at(accounts, exclude, threshold, now);
    accounts
}

fn sorted_candidates_at(
    mut accounts: Vec<AccountProfile>,
    exclude: Option<&str>,
    threshold: i32,
    now: i64,
) -> Vec<AccountProfile> {
    accounts.retain(|account| {
        if exclude == Some(account.id.as_str()) {
            return false;
        }
        if account.status.kind.is_permanently_unavailable() {
            return false;
        }
        account
            .usage
            .as_ref()
            .and_then(|usage| usage.lowest_remaining_at(now))
            .is_none_or(|remaining| remaining > threshold)
    });
    accounts.sort_by_key(|account| {
        let reset = account
            .usage
            .as_ref()
            .and_then(|usage| usage.next_quota_recovery_at(now))
            .unwrap_or(i64::MAX);
        let remaining = account
            .usage
            .as_ref()
            .and_then(|usage| usage.highest_remaining_at(now))
            .unwrap_or(100);
        (reset, std::cmp::Reverse(remaining), account.created_at)
    });
    accounts
}

fn known_available_candidate(
    accounts: &[AccountProfile],
    exclude: Option<&str>,
    now: i64,
) -> Option<AccountProfile> {
    sorted_candidates_at(accounts.to_vec(), exclude, 0, now)
        .into_iter()
        .find(|account| {
            account
                .usage
                .as_ref()
                .and_then(|usage| usage.lowest_remaining_at(now))
                .is_some_and(|remaining| remaining > 0)
        })
}

fn auto_switch_candidate(
    accounts: &[AccountProfile],
    active_id: Option<&str>,
    exhausted_id: &str,
    now: i64,
) -> Option<AccountProfile> {
    if active_id != Some(exhausted_id) {
        return None;
    }
    known_available_candidate(accounts, Some(exhausted_id), now)
}

pub fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn jwt_expiry(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let mut normalized = payload.replace('-', "+").replace('_', "/");
    while normalized.len() % 4 != 0 {
        normalized.push('=');
    }
    URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(normalized))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|claims| claims.get("exp").and_then(Value::as_i64))
}

pub(crate) fn auth_is_newer(candidate: &Value, current: &Value) -> bool {
    let candidate_expiry = candidate
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .and_then(jwt_expiry)
        .unwrap_or_default();
    let current_expiry = current
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .and_then(jwt_expiry)
        .unwrap_or_default();
    if candidate_expiry != current_expiry {
        return candidate_expiry > current_expiry;
    }
    auth_refresh_timestamp(candidate) > auth_refresh_timestamp(current)
}

fn auth_refresh_timestamp(auth: &Value) -> i64 {
    auth.get("last_refresh")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp())
        .or_else(|| auth.get("last_refresh").and_then(Value::as_i64))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppSettings, UsageWindow};

    fn account(id: &str, remaining: Option<i32>, reset_at: Option<i64>) -> AccountProfile {
        let usage = remaining.map(|remaining| UsageSnapshot {
            primary: Some(UsageWindow {
                used_percent: 100 - remaining,
                window_minutes: Some(300),
                resets_at: reset_at,
            }),
            secondary: None,
            updated_at: 0,
            error: None,
        });
        AccountProfile {
            id: id.to_owned(),
            account_id: id.to_owned(),
            email: format!("{id}@example.test"),
            plan: "Plus".to_owned(),
            created_at: 0,
            last_used_at: None,
            usage,
            status: AccountStatus::healthy(),
            sealed_auth: String::new(),
        }
    }

    #[test]
    fn active_account_stays_selected_until_quota_is_zero() {
        let now = now_timestamp();
        let accounts = vec![
            account("active", Some(1), Some(now + 200)),
            account("backup", Some(90), Some(now + 300)),
        ];

        let selected = select_request_account(&accounts, Some("active")).unwrap();

        assert_eq!(selected.id, "active");
    }

    #[test]
    fn exhausted_active_account_selects_a_known_available_backup() {
        let now = now_timestamp();
        let mut active = account("active", Some(0), Some(now + 200));
        active.status = AccountStatus::new(AccountStatusKind::UsageLimit, "Лимит исчерпан.");
        let accounts = vec![
            active,
            account("backup", Some(1), Some(now + 300)),
            account("unknown", None, None),
        ];

        let selected = select_request_account(&accounts, Some("active")).unwrap();

        assert_eq!(selected.id, "backup");
    }

    #[test]
    fn exhausted_active_account_is_blocked_when_no_known_backup_is_available() {
        let now = now_timestamp();
        let mut active = account("active", Some(0), Some(now + 200));
        active.status = AccountStatus::new(AccountStatusKind::UsageLimit, "Лимит исчерпан.");
        let accounts = vec![active, account("unknown", None, None)];

        let error = select_request_account(&accounts, Some("active")).unwrap_err();

        assert!(error.contains("Лимит текущего профиля исчерпан"));
    }

    #[test]
    fn confirmed_limit_only_auto_switches_when_exhausted_profile_is_still_active() {
        let now = now_timestamp();
        let accounts = vec![
            account("exhausted", Some(0), Some(now + 200)),
            account("manual-choice", Some(75), Some(now + 300)),
        ];

        let candidate = auto_switch_candidate(&accounts, Some("manual-choice"), "exhausted", now);

        assert!(candidate.is_none());
    }

    #[test]
    fn confirmed_limit_chooses_only_a_profile_with_known_positive_quota() {
        let now = now_timestamp();
        let accounts = vec![
            account("exhausted", Some(0), Some(now + 200)),
            account("unknown", None, None),
            account("available", Some(50), Some(now + 300)),
        ];

        let candidate = auto_switch_candidate(&accounts, Some("exhausted"), "exhausted", now);

        assert_eq!(candidate.unwrap().id, "available");
    }

    #[test]
    fn permanent_auth_failure_never_rotates_automatically() {
        let mut blocked = account("blocked", Some(0), Some(100));
        blocked.status = AccountStatus::new(AccountStatusKind::ReauthRequired, "Срок входа истёк.");
        let accounts = vec![blocked, account("backup", Some(90), Some(200))];

        let error = select_request_account(&accounts, Some("blocked")).unwrap_err();

        assert!(error.contains("Срок входа истёк"));
        assert!(error.contains("выберите другой профиль вручную"));
    }

    #[test]
    fn available_candidates_prefer_soonest_reset_and_respect_threshold() {
        let now = now_timestamp();
        let accounts = vec![
            account("later-reset", Some(90), Some(now + 7_200)),
            account("soon-reset", Some(20), Some(now + 3_600)),
            account("under-threshold", Some(1), Some(now + 1_800)),
            account("unknown-usage", None, None),
        ];

        let candidates = sorted_candidates(accounts, None, 1);

        assert_eq!(
            candidates
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            vec!["soon-reset", "later-reset", "unknown-usage"]
        );
    }

    #[test]
    fn available_candidates_rank_by_limiting_window_recovery() {
        let now = now_timestamp();
        let mut unrelated_reset_soon = account("unrelated-reset", Some(5), Some(now + 8 * 3600));
        unrelated_reset_soon.usage.as_mut().unwrap().secondary = Some(UsageWindow {
            used_percent: 20,
            window_minutes: Some(10_080),
            resets_at: Some(now + 3600),
        });
        let limiting_reset_soon = account("limiting-reset", Some(30), Some(now + 3 * 3600));

        let candidates = sorted_candidates_at(
            vec![unrelated_reset_soon, limiting_reset_soon],
            None,
            0,
            now,
        );

        assert_eq!(
            candidates
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            ["limiting-reset", "unrelated-reset"]
        );
    }

    #[test]
    fn already_restored_profile_is_prioritized_before_a_future_reset() {
        let now = now_timestamp();
        let candidates = sorted_candidates_at(
            vec![
                account("future-reset", Some(70), Some(now + 3600)),
                account("already-restored", Some(100), None),
            ],
            None,
            0,
            now,
        );

        assert_eq!(candidates[0].id, "already-restored");
    }

    #[test]
    fn quota_recovery_waits_for_every_window_tied_at_the_minimum() {
        let now = now_timestamp();
        let mut tied = account("tied", Some(0), Some(now + 3600));
        tied.usage.as_mut().unwrap().secondary = Some(UsageWindow {
            used_percent: 100,
            window_minutes: Some(10_080),
            resets_at: Some(now + 3 * 3600),
        });

        let recovery_at = tied.usage.as_ref().unwrap().next_quota_recovery_at(now);

        assert_eq!(recovery_at, Some(now + 3 * 3600));
    }

    #[test]
    fn initial_selection_excludes_permanently_unavailable_accounts() {
        let mut blocked = account("blocked", Some(90), Some(now_timestamp() + 100));
        blocked.status = AccountStatus::new(AccountStatusKind::ReauthRequired, "Срок входа истёк.");
        let candidates = sorted_candidates(
            vec![
                blocked,
                account("healthy", Some(70), Some(now_timestamp() + 200)),
            ],
            None,
            1,
        );
        assert_eq!(
            candidates
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            ["healthy"]
        );
    }

    #[test]
    fn expired_zero_quota_does_not_rotate_away_from_a_reset_account() {
        let now = now_timestamp();
        let mut reset_active = account("reset-active", Some(0), Some(now - 1));
        reset_active.status = AccountStatus::new(AccountStatusKind::UsageLimit, "Лимит исчерпан.");
        let accounts = vec![reset_active, account("backup", Some(90), Some(now + 300))];

        let selected = select_request_account(&accounts, Some("reset-active")).unwrap();

        assert_eq!(selected.id, "reset-active");
    }

    #[test]
    fn fresh_limit_without_reset_invalidates_an_elapsed_cached_reset() {
        let now = now_timestamp();
        let mut active = account("active", Some(0), Some(now - 1));
        active.status = AccountStatus::new(AccountStatusKind::UsageLimit, "Лимит исчерпан.");
        mark_exhausted_usage_window(active.usage.as_mut().unwrap(), None, now);

        assert_eq!(
            active
                .usage
                .as_ref()
                .and_then(|usage| usage.lowest_remaining_at(now)),
            Some(0)
        );
        assert!(active
            .usage
            .as_ref()
            .and_then(|usage| usage.primary.as_ref())
            .is_some_and(|window| window.resets_at.is_none()));

        let error =
            select_request_account(&[active, account("unknown", None, None)], Some("active"))
                .unwrap_err();
        assert!(error.contains("нет проверенного остатка"));
    }

    #[test]
    fn account_with_expired_zero_window_is_available_as_a_candidate() {
        let now = now_timestamp();
        let candidates = sorted_candidates(
            vec![
                account("reset", Some(0), Some(now - 1)),
                account("exhausted", Some(0), Some(now + 300)),
            ],
            None,
            0,
        );

        assert_eq!(candidates[0].id, "reset");
    }

    #[test]
    fn manual_switch_rejects_an_exhausted_profile() {
        let account = account("exhausted", Some(0), Some(now_timestamp() + 300));

        let error = validate_manual_switch_target(&account, now_timestamp()).unwrap_err();

        assert!(error.contains("исчерпано"));
    }

    #[test]
    fn manual_switch_allows_a_profile_after_its_limit_resets() {
        let now = now_timestamp();
        let mut account = account("reset", Some(0), Some(now - 1));
        account.status = AccountStatus::new(
            AccountStatusKind::UsageLimit,
            "Лимит этого окна исчерпан; профиль вернётся после сброса.",
        );

        validate_manual_switch_target(&account, now).unwrap();
    }

    #[test]
    fn manual_switch_rejects_usage_limit_status_without_quota_data() {
        let mut account = account("unknown", None, None);
        account.status = AccountStatus::new(
            AccountStatusKind::UsageLimit,
            "Лимит этого окна исчерпан; профиль вернётся после сброса.",
        );

        let error = validate_manual_switch_target(&account, now_timestamp()).unwrap_err();

        assert!(error.contains("неизвестен"));
    }

    #[test]
    fn manual_switch_still_rejects_permanent_account_errors() {
        let mut account = account("blocked", Some(90), Some(now_timestamp() + 300));
        account.status = AccountStatus::new(AccountStatusKind::Banned, "Аккаунт заблокирован.");

        let error = validate_manual_switch_target(&account, now_timestamp()).unwrap_err();

        assert!(error.contains("Аккаунт недоступен"));
    }

    #[test]
    fn default_usage_poll_interval_is_one_minute() {
        assert_eq!(AppSettings::default().refresh_interval_seconds, 60);
    }
}
