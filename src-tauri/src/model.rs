use serde::{Deserialize, Serialize};

pub const VAULT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(alias = "start_with_windows")]
    pub start_with_windows: bool,
    #[serde(alias = "start_in_tray")]
    pub start_in_tray: bool,
    #[serde(alias = "sound_enabled")]
    pub sound_enabled: bool,
    /// Remaining-quota warning level. It never changes the selected account.
    #[serde(alias = "switch_threshold")]
    pub switch_threshold: u8,
    /// Automatically select an available profile after the active one is confirmed exhausted.
    #[serde(alias = "automatic_profile_switching")]
    pub automatic_profile_switching: bool,
    #[serde(alias = "refresh_interval_seconds")]
    pub refresh_interval_seconds: u32,
    #[serde(alias = "proxy_enabled")]
    pub proxy_enabled: bool,
    /// Colour appearance for the desktop surface. `system` follows Windows,
    /// while `dark` and `light` are explicit overrides.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Enables the low-cost glass, glow and motion layer in the window.
    #[serde(default = "default_visual_effects")]
    pub visual_effects_enabled: bool,
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_visual_effects() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            start_with_windows: false,
            start_in_tray: false,
            sound_enabled: true,
            switch_threshold: 1,
            automatic_profile_switching: true,
            refresh_interval_seconds: 60,
            proxy_enabled: false,
            theme: default_theme(),
            visual_effects_enabled: default_visual_effects(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CodexPreferences {
    pub model: String,
    pub model_reasoning_effort: String,
    pub approval_policy: String,
    pub sandbox_mode: String,
    pub developer_instructions: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    #[serde(alias = "used_percent")]
    pub used_percent: i32,
    #[serde(alias = "window_minutes")]
    pub window_minutes: Option<i64>,
    #[serde(alias = "resets_at")]
    pub resets_at: Option<i64>,
}

impl UsageWindow {
    pub fn remaining_percent(&self) -> i32 {
        (100 - self.used_percent).clamp(0, 100)
    }

    pub fn remaining_percent_at(&self, now: i64) -> i32 {
        if self.resets_at.is_some_and(|resets_at| resets_at <= now) {
            100
        } else {
            self.remaining_percent()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub primary: Option<UsageWindow>,
    pub secondary: Option<UsageWindow>,
    #[serde(alias = "updated_at")]
    pub updated_at: i64,
    pub error: Option<String>,
}

impl UsageSnapshot {
    pub fn lowest_remaining(&self) -> Option<i32> {
        [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .flatten()
            .map(UsageWindow::remaining_percent)
            .min()
    }

    pub fn highest_remaining(&self) -> Option<i32> {
        [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .flatten()
            .map(UsageWindow::remaining_percent)
            .max()
    }

    pub fn lowest_remaining_at(&self, now: i64) -> Option<i32> {
        [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .flatten()
            .map(|window| window.remaining_percent_at(now))
            .min()
    }

    pub fn highest_remaining_at(&self, now: i64) -> Option<i32> {
        [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .flatten()
            .map(|window| window.remaining_percent_at(now))
            .max()
    }

    pub fn next_reset_at(&self) -> Option<i64> {
        let now = chrono::Utc::now().timestamp();
        [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .flatten()
            .filter(|window| window.remaining_percent() < 100)
            .filter_map(|window| window.resets_at)
            .filter(|resets_at| *resets_at > now)
            .min()
    }

    /// Predict when the account's currently limiting quota will recover.
    /// The effective remaining quota is the minimum of the active windows, so
    /// every window tied at that minimum must reset before the minimum rises.
    /// If any tied window has no known reset time, its recovery cannot be
    /// ranked reliably.
    pub fn next_quota_recovery_at(&self, now: i64) -> Option<i64> {
        let lowest = self.lowest_remaining_at(now)?;
        if lowest >= 100 {
            return Some(now);
        }

        let limiting_windows: Vec<&UsageWindow> = [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .flatten()
            .filter(|window| window.remaining_percent_at(now) == lowest)
            .collect();
        let reset_times: Option<Vec<i64>> = limiting_windows
            .into_iter()
            .map(|window| window.resets_at.filter(|resets_at| *resets_at > now))
            .collect();
        reset_times?.into_iter().max()
    }
}

/// Health of a saved login.  The value is deliberately separate from the
/// quota snapshot: a zero quota can recover by itself, while a revoked login
/// or denied account must never be selected automatically.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountStatusKind {
    #[default]
    Healthy,
    UsageLimit,
    ReauthRequired,
    AccessDenied,
    Banned,
    CorruptAuth,
    TemporaryError,
}

impl AccountStatusKind {
    pub fn is_permanently_unavailable(&self) -> bool {
        matches!(
            self,
            Self::ReauthRequired | Self::AccessDenied | Self::Banned | Self::CorruptAuth
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountStatus {
    pub kind: AccountStatusKind,
    pub detail: Option<String>,
}

impl AccountStatus {
    pub fn healthy() -> Self {
        Self::default()
    }

    pub fn new(kind: AccountStatusKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: Some(detail.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProfile {
    pub id: String,
    #[serde(alias = "account_id")]
    pub account_id: String,
    pub email: String,
    pub plan: String,
    #[serde(alias = "created_at")]
    pub created_at: i64,
    #[serde(alias = "last_used_at")]
    pub last_used_at: Option<i64>,
    pub usage: Option<UsageSnapshot>,
    #[serde(default)]
    pub status: AccountStatus,
    #[serde(alias = "sealed_auth")]
    pub sealed_auth: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VaultDocument {
    pub version: u32,
    pub settings: AppSettings,
    #[serde(alias = "active_account_id")]
    pub active_account_id: Option<String>,
    pub accounts: Vec<AccountProfile>,
    #[serde(alias = "proxy_configured")]
    pub proxy_configured: bool,
    #[serde(alias = "original_openai_base_url")]
    pub original_openai_base_url: Option<String>,
    #[serde(alias = "original_openai_base_url_set")]
    pub original_openai_base_url_set: bool,
    #[serde(default)]
    pub original_model_provider: Option<String>,
    #[serde(default)]
    pub original_model_provider_set: bool,
    pub proxy_access_key: Option<String>,
    #[serde(default)]
    pub admin_password_hash: Option<String>,
}

impl Default for VaultDocument {
    fn default() -> Self {
        Self {
            version: VAULT_VERSION,
            settings: AppSettings::default(),
            active_account_id: None,
            accounts: Vec::new(),
            proxy_configured: false,
            original_openai_base_url: None,
            original_openai_base_url_set: false,
            original_model_provider: None,
            original_model_provider_set: false,
            proxy_access_key: None,
            admin_password_hash: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    pub account_id: String,
    pub email: String,
    pub plan: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub usage: Option<UsageSnapshot>,
    pub status: AccountStatus,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptView {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardData {
    pub accounts: Vec<AccountView>,
    pub settings: AppSettings,
    pub current_account_id: Option<String>,
    pub refreshed_at: Option<i64>,
    pub codex_preferences: CodexPreferences,
    pub prompts: Vec<PromptView>,
    pub codex_connected: bool,
    pub codex_running: bool,
    pub proxy_address: String,
    pub proxy_error: Option<String>,
    pub admin_protected: bool,
    pub admin_unlocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupAccount {
    pub account_id: String,
    pub email: String,
    pub plan: String,
    pub created_at: i64,
    #[serde(default)]
    pub last_used_at: Option<i64>,
    #[serde(default)]
    pub usage: Option<UsageSnapshot>,
    #[serde(default)]
    pub status: AccountStatus,
    pub auth_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPayload {
    pub format: String,
    pub version: u32,
    pub exported_at: i64,
    pub accounts: Vec<BackupAccount>,
    pub settings: AppSettings,
    pub codex_config_toml: String,
    pub prompts: Vec<PromptView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedBackup {
    pub format: String,
    pub version: u32,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}
