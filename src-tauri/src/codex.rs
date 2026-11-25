use crate::{
    model::{AccountProfile, AccountStatus, AccountStatusKind, UsageSnapshot, UsageWindow},
    storage,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(25);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEVICE_LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const STALE_AUTH_HOME_AGE: Duration = Duration::from_secs(24 * 60 * 60);

// This is the OAuth client used by the official Codex CLI.  The copy-link
// action uses the same PKCE parameters as `codex login`, but deliberately
// does not launch a browser.  Each click gets a fresh state/challenge pair,
// so the copied address is a real one-time login URL instead of the generic
// device-code landing page.
const CODEX_OAUTH_AUTHORIZE_ENDPOINT: &str = "https://auth.openai.com/oauth/authorize";
const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_OAUTH_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const CODEX_OAUTH_REDIRECT_URI: &str = "http://127.0.0.1:1455/auth/callback";
const CODEX_OAUTH_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const MANUAL_OAUTH_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone)]
struct ManualOAuthSession {
    url: String,
    state: String,
    verifier: String,
    expires_at: Instant,
}

static MANUAL_OAUTH_SESSION: std::sync::LazyLock<Mutex<Option<ManualOAuthSession>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

fn build_manual_auth_session() -> ManualOAuthSession {
    use sha2::{Digest, Sha256};

    let random_token = || {
        let bytes = (0..48).map(|_| rand::random::<u8>()).collect::<Vec<_>>();
        URL_SAFE_NO_PAD.encode(bytes)
    };
    let state = random_token();
    let verifier = random_token();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = url::Url::parse(CODEX_OAUTH_AUTHORIZE_ENDPOINT)
        .expect("the fixed Codex OAuth endpoint must be a valid URL");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CODEX_OAUTH_CLIENT_ID)
        .append_pair("redirect_uri", CODEX_OAUTH_REDIRECT_URI)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("scope", CODEX_OAUTH_SCOPE)
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", "codex_cli_rs");
    ManualOAuthSession {
        url: url.into(),
        state,
        verifier,
        expires_at: Instant::now() + MANUAL_OAUTH_TIMEOUT,
    }
}

/// Start (or resume) the one manual OAuth session used by the copy-link
/// button.  Keeping the verifier in memory is what makes the callback URL
/// usable while ensuring no credential material is written before the token
/// exchange succeeds.
pub fn begin_manual_auth_session() -> Result<String, String> {
    let mut session = MANUAL_OAUTH_SESSION
        .lock()
        .map_err(|_| "Не удалось открыть защищённую сессию ручного входа.".to_string())?;
    if let Some(current) = session.as_ref() {
        if current.expires_at > Instant::now() {
            return Ok(current.url.clone());
        }
    }
    let next = build_manual_auth_session();
    let url = next.url.clone();
    *session = Some(next);
    Ok(url)
}

/// Returns whether a manual session still owns the interactive login slot,
/// including a session whose URL expired but has not yet been cancelled.
pub fn manual_auth_session_present() -> bool {
    MANUAL_OAUTH_SESSION
        .lock()
        .map(|session| session.is_some())
        .unwrap_or(false)
}

pub fn cancel_manual_auth_session() {
    if let Ok(mut session) = MANUAL_OAUTH_SESSION.lock() {
        *session = None;
    }
}

/// Exchange the callback from the manually opened browser URL.  `None` means
/// no manual session is active, allowing the legacy CLI callback path to keep
/// working unchanged.  The caller clears the session after it commits the
/// returned credentials, so cancellation can still win while the account is
/// being persisted.  The bytes use the same `auth.json` shape written by the
/// official Codex CLI.
pub async fn complete_manual_auth_callback(raw_url: &str) -> Result<Option<Vec<u8>>, String> {
    let session = {
        let guard = MANUAL_OAUTH_SESSION
            .lock()
            .map_err(|_| "Не удалось прочитать сессию ручного входа.".to_string())?;
        let Some(current) = guard.as_ref() else {
            return Ok(None);
        };
        if current.expires_at <= Instant::now() {
            drop(guard);
            cancel_manual_auth_session();
            return Err(
                "Ссылка ручного входа истекла. Скопируйте новую ссылку и повторите вход."
                    .to_string(),
            );
        }
        current.clone()
    };
    let url = reqwest::Url::parse(raw_url).map_err(|_| {
        "Вставьте полный адрес возврата из браузера, начинающийся с http://127.0.0.1 и содержащий /auth/callback.".to_string()
    })?;
    let matches_redirect =
        url.as_str().split_once('?').map(|(base, _)| base) == Some(CODEX_OAUTH_REDIRECT_URI);
    let mut callback_state = None;
    let mut code = None;
    let mut callback_error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "state" => callback_state = Some(value.into_owned()),
            "code" => code = Some(value.into_owned()),
            "error" => callback_error = Some(value.into_owned()),
            _ => {}
        }
    }
    if let Some(error) = callback_error {
        return Err(format!("Вход отменён в браузере: {error}."));
    }
    let Some(code) = code.filter(|value| !value.is_empty()) else {
        return Err(
            "В адресе возврата нет кода авторизации. Скопируйте полный адрес /auth/callback."
                .to_string(),
        );
    };
    if !matches_redirect || callback_state.as_deref() != Some(session.state.as_str()) {
        return Err("Адрес возврата не соответствует текущей ссылке входа. Скопируйте новую ссылку и повторите.".to_string());
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| "Не удалось подготовить защищённое завершение входа.".to_string())?;
    let response = client
        .post(CODEX_OAUTH_TOKEN_ENDPOINT)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CODEX_OAUTH_CLIENT_ID),
            ("code", code.as_str()),
            ("redirect_uri", CODEX_OAUTH_REDIRECT_URI),
            ("code_verifier", session.verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|_| {
            "Не удалось завершить вход. Проверьте интернет и повторите вставку ссылки.".to_string()
        })?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|_| "Сервис входа вернул нечитаемый ответ.".to_string())?;
    if !status.is_success() {
        return Err(format!(
            "Сервис входа отклонил код ({}). Скопируйте новую ссылку и повторите.",
            status.as_u16()
        ));
    }
    let reply: Value = serde_json::from_str(&body)
        .map_err(|_| "Сервис входа вернул нечитаемые токены.".to_string())?;
    let id_token = reply
        .get("id_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "В ответе входа отсутствует id_token.".to_string())?;
    let access_token = reply
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "В ответе входа отсутствует access_token.".to_string())?;
    let refresh_token = reply
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "В ответе входа отсутствует refresh_token.".to_string())?;
    let auth = json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": id_token,
            "access_token": access_token,
            "refresh_token": refresh_token,
        },
        "last_refresh": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
    });
    let bytes = serde_json::to_vec(&auth)
        .map_err(|_| "Не удалось подготовить защищённые данные входа.".to_string())?;
    Ok(Some(bytes))
}

struct TemporaryCodexHome {
    path: PathBuf,
}

impl TemporaryCodexHome {
    fn create(root: &Path, area: &str, error: &str) -> Result<Self, String> {
        let parent = root.join(area);
        fs::create_dir_all(&parent).map_err(|_| error.to_string())?;
        let path = parent.join(Uuid::new_v4().to_string());
        fs::create_dir(&path).map_err(|_| error.to_string())?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryCodexHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn cleanup_stale_auth_homes(root: &Path) {
    for area in ["staging", "usage"] {
        let parent = root.join(area);
        let Ok(entries) = fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                continue;
            }
            let path = entry.path();
            let stale = latest_modified(&path).is_some_and(|modified| {
                SystemTime::now()
                    .duration_since(modified)
                    .is_ok_and(|age| age >= STALE_AUTH_HOME_AGE)
            });
            if stale {
                let _ = fs::remove_dir_all(path);
            }
        }
    }
}

fn latest_modified(path: &Path) -> Option<SystemTime> {
    let metadata = fs::symlink_metadata(path).ok()?;
    let mut latest = metadata.modified().ok();
    if !metadata.file_type().is_dir() {
        return latest;
    }
    for entry in fs::read_dir(path).ok()? {
        let entry = entry.ok()?;
        if entry.file_type().ok()?.is_symlink() {
            continue;
        }
        if let Some(modified) = latest_modified(&entry.path()) {
            latest = Some(latest.map_or(modified, |previous| previous.max(modified)));
        }
    }
    latest
}

#[derive(Debug, Clone)]
pub struct AccountIdentity {
    pub account_id: String,
    pub email: String,
    pub plan: String,
}

pub fn current_codex_home() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let profile = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| "Windows не сообщил путь профиля пользователя.".to_string())?;
    Ok(profile.join(".codex"))
}

pub fn identity_from_auth(bytes: &[u8]) -> Result<AccountIdentity, String> {
    let auth: Value = serde_json::from_slice(bytes)
        .map_err(|_| "Codex не вернул читаемые данные входа. Повторите авторизацию.".to_string())?;
    let tokens = auth
        .get("tokens")
        .ok_or_else(|| "В профиле Codex не найден ChatGPT OAuth-вход.".to_string())?;
    let claims = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(decode_jwt_payload)
        .unwrap_or(Value::Null);
    let auth_claims = claims
        .get("https://api.openai.com/auth")
        .or_else(|| claims.get("https://api.openai.com/authentication"));
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .or_else(|| {
            auth_claims
                .and_then(|v| v.get("chatgpt_account_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| claims.get("chatgpt_account_id").and_then(Value::as_str))
        .or_else(|| claims.get("account_id").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "Не удалось определить аккаунт из ответа Codex. Проверьте вход и повторите.".to_string()
        })?
        .to_string();
    let email = claims
        .get("email")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Аккаунт ChatGPT")
        .to_string();
    let plan = auth_claims
        .and_then(|value| {
            value
                .get("chatgpt_plan_type")
                .or_else(|| value.get("plan_type"))
        })
        .and_then(Value::as_str)
        .or_else(|| claims.get("plan_type").and_then(Value::as_str))
        .unwrap_or("План не указан")
        .to_string();
    Ok(AccountIdentity {
        account_id,
        email,
        plan,
    })
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let mut padded = payload.to_string();
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(padded))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

pub fn find_codex_cli() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CODEX_CLI_PATH").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err("CODEX_CLI_PATH указывает на несуществующий файл. Исправьте настройку и перезапустите приложение.".to_string());
    }

    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        let bin_root = local_app_data.join("OpenAI").join("Codex").join("bin");
        if let Ok(entries) = fs::read_dir(&bin_root) {
            let mut candidates = entries
                .flatten()
                .map(|entry| entry.path().join("codex.exe"))
                .filter(|path| path.is_file())
                .collect::<Vec<_>>();
            candidates.sort_by_key(|path| {
                fs::metadata(path)
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH)
            });
            if let Some(path) = candidates.pop() {
                return Ok(path);
            }
        }
    }

    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let output = Command::new("where.exe")
        .arg("codex.exe")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|_| {
            "Не найден Codex CLI. Установите приложение Codex или настройте CODEX_CLI_PATH."
                .to_string()
        })?;
    if output.status.success() {
        if let Some(path) = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .map(PathBuf::from)
            .find(|path| path.is_file() && !path.to_string_lossy().contains("WindowsApps"))
        {
            return Ok(path);
        }
    }
    Err("Не найден Codex CLI. Установите официальное приложение Codex или добавьте путь к codex.exe в CODEX_CLI_PATH.".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMode {
    Browser,
    DeviceCode,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginAuthInfo {
    pub url: Option<String>,
    pub user_code: Option<String>,
    pub device_code: bool,
    pub open_error: Option<String>,
}

#[allow(dead_code)] // Kept for the legacy native UI path; the Tauri desktop UI uses progress events.
pub fn add_account(
    root: &Path,
    cli: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<AccountProfile, String> {
    add_account_with_mode(root, cli, cancel, LoginMode::Browser, |_| {})
}

pub fn add_account_with_mode<F>(
    root: &Path,
    cli: &Path,
    cancel: Arc<AtomicBool>,
    login_mode: LoginMode,
    on_auth_info: F,
) -> Result<AccountProfile, String>
where
    F: Fn(LoginAuthInfo) + Send + 'static,
{
    let staging = TemporaryCodexHome::create(
        root,
        "staging",
        "Не удалось подготовить временный профиль для входа.",
    )?;
    fs::write(
        staging.path().join("config.toml"),
        "cli_auth_credentials_store = \"file\"\n",
    )
    .map_err(|_| "Не удалось настроить временный профиль Codex.".to_string())?;

    run_login(cli, staging.path(), &cancel, login_mode, on_auth_info).and_then(|_| {
        let auth_path = staging.path().join("auth.json");
        let auth = fs::read(&auth_path).map_err(|_| {
            "Вход не завершился: файл авторизации отсутствует. Повторите добавление аккаунта."
                .to_string()
        })?;
        let identity = identity_from_auth(&auth)?;
        let sealed_auth = storage::seal_auth(&auth)?;
        let now = now_timestamp();
        let account = AccountProfile {
            id: Uuid::new_v4().to_string(),
            account_id: identity.account_id,
            email: identity.email,
            plan: identity.plan,
            created_at: now,
            last_used_at: None,
            usage: None,
            status: AccountStatus::healthy(),
            sealed_auth,
        };
        Ok(account)
    })
}

fn run_login<F>(
    cli: &Path,
    staging: &Path,
    cancel: &AtomicBool,
    login_mode: LoginMode,
    on_auth_info: F,
) -> Result<(), String>
where
    F: Fn(LoginAuthInfo),
{
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut command = Command::new(cli);
    command
        .arg("login")
        .env("CODEX_HOME", staging)
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .current_dir(staging)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let device_code_login = matches!(login_mode, LoginMode::DeviceCode);
    if device_code_login {
        command.arg("--device-auth");
    }
    let mut child = command
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| "Не удалось запустить официальный Codex login. Проверьте установку Codex и CODEX_CLI_PATH.".to_string())?;

    let (line_tx, line_rx) = mpsc::channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        spawn_login_output_reader(stdout, line_tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_login_output_reader(stderr, line_tx.clone());
    }
    drop(line_tx);

    let started = Instant::now();
    let timeout = if device_code_login {
        DEVICE_LOGIN_TIMEOUT
    } else {
        LOGIN_TIMEOUT
    };
    let mut current_user_code = None;
    let mut expecting_device_code = false;
    let mut device_auth_unavailable = false;
    loop {
        while let Ok(raw_line) = line_rx.try_recv() {
            let line = strip_ansi_sequences(&raw_line);
            let trimmed = line.trim();
            if device_code_login
                && trimmed
                    .to_ascii_lowercase()
                    .contains("device code login is not enabled")
            {
                device_auth_unavailable = true;
            }
            if device_code_login
                && trimmed
                    .to_ascii_lowercase()
                    .contains("enter this one-time code")
            {
                expecting_device_code = true;
                continue;
            }

            let url_prefix = match login_mode {
                LoginMode::Browser => "https://auth.openai.com/oauth/authorize",
                LoginMode::DeviceCode => "https://auth.openai.com/codex/device",
            };
            if let Some(url) = trimmed
                .split_whitespace()
                .find(|part| part.starts_with(url_prefix))
                .map(|part| part.trim_end_matches([')', ']', '>', '"', '\'', ',', '.']))
            {
                on_auth_info(LoginAuthInfo {
                    url: Some(url.to_string()),
                    user_code: current_user_code.clone(),
                    device_code: device_code_login,
                    open_error: None,
                });
                continue;
            }

            if expecting_device_code {
                expecting_device_code = false;
                if let Some(code) = parse_device_code(trimmed) {
                    current_user_code = Some(code.clone());
                    on_auth_info(LoginAuthInfo {
                        url: None,
                        user_code: Some(code),
                        device_code: true,
                        open_error: None,
                    });
                }
            }
        }

        if cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(
                "Вход отменён. Окно браузера можно закрыть и повторить добавление позже."
                    .to_string(),
            );
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Не удалось проверить результат входа. Закройте окно Codex login и попробуйте снова.".to_string());
            }
        };
        if let Some(status) = status {
            return if status.success() {
                Ok(())
            } else if device_auth_unavailable {
                Err("Сервер не разрешил вход по одноразовому коду. Используйте обычную кнопку добавления аккаунта или повторите вход в стандартном браузере.".to_string())
            } else {
                Err("Codex login завершился без успешной авторизации. Проверьте окно входа и повторите.".to_string())
            };
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if device_code_login {
                "Вход по одноразовому коду занял больше 15 минут. Начните вход заново.".to_string()
            } else {
                "Вход занял больше 10 минут и был отменён. Добавьте аккаунт ещё раз.".to_string()
            });
        }
        thread::sleep(Duration::from_millis(120));
    }
}

fn spawn_login_output_reader<R>(reader: R, line_tx: mpsc::Sender<String>)
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(line) => {
                    if line_tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn strip_ansi_sequences(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut escape = false;
    let mut csi = false;
    for character in input.chars() {
        if escape {
            if character == '[' {
                csi = true;
            } else {
                escape = false;
            }
            continue;
        }
        if csi {
            if ('@'..='~').contains(&character) {
                csi = false;
                escape = false;
            }
            continue;
        }
        if character == '\u{1b}' {
            escape = true;
        } else {
            output.push(character);
        }
    }
    output
}

fn parse_device_code(line: &str) -> Option<String> {
    let code = line.split_whitespace().next()?;
    if (4..=24).contains(&code.len())
        && code
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        Some(code.to_string())
    } else {
        None
    }
}

pub fn open_private_auth_url(raw_url: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw_url)
        .map_err(|_| "Не удалось прочитать ссылку официального входа.".to_string())?;
    if url.scheme() != "https"
        || url.host_str() != Some("auth.openai.com")
        || !matches!(url.path(), "/oauth/authorize" | "/codex/device")
    {
        return Err("Можно открыть только ссылку официального входа auth.openai.com.".to_string());
    }
    launch_private_browser(raw_url)
}

pub async fn forward_auth_callback(raw_url: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw_url).map_err(|_| {
        "Вставьте полный адрес возврата из браузера, начинающийся с http://localhost и содержащий /auth/callback.".to_string()
    })?;
    let local_host = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    let has_code = url
        .query_pairs()
        .any(|(key, value)| key == "code" && !value.is_empty());
    let has_state = url
        .query_pairs()
        .any(|(key, value)| key == "state" && !value.is_empty());
    if url.scheme() != "http"
        || !local_host
        || url.port().is_none()
        || url.path() != "/auth/callback"
        || !has_code
        || !has_state
    {
        return Err("Это не адрес возврата Codex. Скопируйте из браузера полный адрес localhost с путем /auth/callback и параметрами code и state.".to_string());
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "Не удалось подготовить локальное завершение входа.".to_string())?;
    let response = client.get(url).send().await.map_err(|_| {
        "Локальный вход не получил ссылку возврата. Проверьте, что окно добавления аккаунта всё ещё открыто, и повторите вставку.".to_string()
    })?;
    if response.status().is_success() || response.status().is_redirection() {
        Ok(())
    } else {
        Err("Codex отклонил ссылку возврата. Проверьте, что вы завершили тот же вход, который сейчас открыт в приложении.".to_string())
    }
}

fn launch_private_browser(url: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let candidates = [
        (
            program_files_x86
                .as_ref()
                .map(|root| root.join("Microsoft/Edge/Application/msedge.exe")),
            "--inprivate",
        ),
        (
            program_files
                .as_ref()
                .map(|root| root.join("Microsoft/Edge/Application/msedge.exe")),
            "--inprivate",
        ),
        (
            local_app_data
                .as_ref()
                .map(|root| root.join("Microsoft/Edge/Application/msedge.exe")),
            "--inprivate",
        ),
        (
            program_files_x86
                .as_ref()
                .map(|root| root.join("Google/Chrome/Application/chrome.exe")),
            "--incognito",
        ),
        (
            program_files
                .as_ref()
                .map(|root| root.join("Google/Chrome/Application/chrome.exe")),
            "--incognito",
        ),
        (
            local_app_data
                .as_ref()
                .map(|root| root.join("Google/Chrome/Application/chrome.exe")),
            "--incognito",
        ),
    ];
    for (candidate, private_flag) in candidates {
        let Some(executable) = candidate.filter(|path| path.is_file()) else {
            continue;
        };
        return Command::new(executable)
            .arg(private_flag)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|_| "Не удалось открыть приватное окно браузера. Скопируйте ссылку и откройте её вручную.".to_string());
    }
    Err("Не найден Microsoft Edge или Google Chrome. Скопируйте ссылку и откройте её в приватном окне вручную.".to_string())
}

pub fn refresh_one(root: &Path, cli: &Path, account: &mut AccountProfile) {
    let updated_at = now_timestamp();
    let auth = match storage::open_auth(&account.sealed_auth) {
        Ok(auth) => auth,
        Err(error) => {
            account.usage = Some(UsageSnapshot {
                primary: None,
                secondary: None,
                updated_at,
                error: Some(error),
            });
            account.status = AccountStatus::new(
                classify_account_error(
                    account
                        .usage
                        .as_ref()
                        .and_then(|usage| usage.error.as_deref())
                        .unwrap_or_default(),
                ),
                "Не удалось расшифровать сохранённые данные входа.",
            );
            return;
        }
    };
    let result = (|| {
        let staging =
            TemporaryCodexHome::create(root, "usage", "Не удалось подготовить чтение лимитов.")?;
        fs::write(
            staging.path().join("config.toml"),
            "cli_auth_credentials_store = \"file\"\n",
        )
        .map_err(|_| "Не удалось подготовить профиль Codex для чтения лимитов.".to_string())?;
        fs::write(staging.path().join("auth.json"), &auth)
            .map_err(|_| "Не удалось временно открыть профиль для чтения лимитов.".to_string())?;
        // `app-server` can rotate the refresh token before it reports a
        // usage error. Always capture that newer same-account auth snapshot,
        // including when the quota read itself fails, or the next refresh
        // would keep using a revoked token.
        let response = query_rate_limits(cli, staging.path());
        if let Ok(rotated) = fs::read(staging.path().join("auth.json")) {
            let same_account = identity_from_auth(&rotated)
                .ok()
                .is_some_and(|identity| identity.account_id == account.account_id);
            if same_account {
                if let Ok(sealed) = storage::seal_auth(&rotated) {
                    account.sealed_auth = sealed;
                }
            }
        }
        let response = response?;
        parse_usage(&response, updated_at)
    })();
    account.usage = Some(match result {
        Ok(snapshot) => {
            account.status = AccountStatus::healthy();
            snapshot
        }
        Err(error) => {
            let old = account.usage.clone();
            let kind = classify_account_error(&error);
            account.status = AccountStatus::new(kind, error.clone());
            if let Some(mut previous) = old {
                // Keep the timestamp of the last successful quota snapshot.
                // Updating only the error state must not make stale percentages
                // look as though they were refreshed just now.
                previous.error = Some(error);
                previous
            } else {
                UsageSnapshot {
                    primary: None,
                    secondary: None,
                    updated_at,
                    error: Some(error),
                }
            }
        }
    });
}

/// Turn the small set of stable Codex/auth failure messages into a UI and
/// routing state.  Network and protocol failures remain temporary so a later
/// refresh can recover them automatically.
pub fn classify_account_error(error: &str) -> AccountStatusKind {
    let lower = error.to_ascii_lowercase();
    if lower.contains("banned")
        || lower.contains("suspended")
        || lower.contains("deactivated")
        || lower.contains("заблок")
        || lower.contains("приостанов")
        || lower.contains("деактив")
    {
        return AccountStatusKind::Banned;
    }
    // Upstream 403 responses can omit the words "forbidden" or "policy".
    // They still mean this profile cannot use the requested Codex endpoint,
    // so keep it out of automatic candidate selection until a later check
    // confirms that access has recovered.
    if lower.starts_with("http 403") || lower.starts_with("status code: 403") {
        return AccountStatusKind::AccessDenied;
    }
    if lower.contains("forbidden")
        || lower.contains("доступ к codex")
        || lower.contains("доступ запрещ")
        || lower.contains("policy")
        || lower.contains("политик")
    {
        return AccountStatusKind::AccessDenied;
    }
    if lower.contains("повреж")
        || lower.contains("не найден токен")
        || lower.contains("нет токена")
        || lower.contains("не найдена секция tokens")
        || lower.contains("нечитаемые данные входа")
        || lower.contains("dpapi")
        || lower.contains("расшифр")
        || lower.contains("malformed token")
        || lower.contains("invalid token")
    {
        return AccountStatusKind::CorruptAuth;
    }
    if lower.contains("повторный вход")
        || lower.contains("войдите")
        || lower.contains("срок входа")
        || lower.contains("отклонил обновление")
        || lower.contains("недейств")
        || lower.contains("unauthorized")
        || lower.contains("401")
    {
        return AccountStatusKind::ReauthRequired;
    }
    AccountStatusKind::TemporaryError
}

fn query_rate_limits(cli: &Path, codex_home: &Path) -> Result<Value, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut child = Command::new(cli)
        .args(["app-server", "--listen", "stdio://"])
        .env("CODEX_HOME", codex_home)
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .current_dir(codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| {
            "Не удалось запустить Codex app-server. Обновите Codex или проверьте CODEX_CLI_PATH."
                .to_string()
        })?;

    let result = communicate_with_server(&mut child);
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn communicate_with_server(child: &mut Child) -> Result<Value, String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server не открыл канал ответа.".to_string())?;
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    let _ = tx.send(line);
                }
                Err(_) => break,
            }
        }
    });
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server не открыл канал команд.".to_string())?;
    write_rpc(
        &mut stdin,
        &json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": { "name": "codex-accounts", "title": "Codex Accounts", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "experimentalApi": true }
            }
        }),
    )?;
    let _ = read_rpc_response(&rx, 1)?;
    write_rpc(&mut stdin, &json!({"method": "initialized"}))?;
    write_rpc(
        &mut stdin,
        &json!({"method": "account/rateLimits/read", "id": 2, "params": {}}),
    )?;
    read_rpc_response(&rx, 2)
}

fn write_rpc(stdin: &mut impl Write, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *stdin, value)
        .map_err(|_| "Не удалось отправить запрос в Codex app-server.".to_string())?;
    stdin
        .write_all(b"\n")
        .and_then(|_| stdin.flush())
        .map_err(|_| {
            "Codex app-server закрыл канал запросов. Обновите Codex и попробуйте снова.".to_string()
        })
}

fn read_rpc_response(rx: &mpsc::Receiver<String>, id: i64) -> Result<Value, String> {
    let deadline = Instant::now() + APP_SERVER_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Codex не успел ответить о лимитах за 25 секунд. Проверьте интернет и обновите позже.".to_string());
        }
        let line = rx.recv_timeout(remaining).map_err(|_| {
            "Codex app-server не ответил. Проверьте подключение и повторите обновление.".to_string()
        })?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("id").and_then(Value::as_i64) != Some(id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            let code = error
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            if code == -32001 || code == -32000 {
                return Err("Требуется повторный вход в этот аккаунт. Удалите профиль и добавьте его заново.".to_string());
            }
            return Err(
                "Codex app-server не вернул данные лимитов. Обновите Codex и попробуйте ещё раз."
                    .to_string(),
            );
        }
        return message
            .get("result")
            .cloned()
            .ok_or_else(|| "Ответ Codex не содержит данных лимитов. Обновите Codex.".to_string());
    }
}

fn parse_usage(response: &Value, updated_at: i64) -> Result<UsageSnapshot, String> {
    let limits = response.get("rateLimits").ok_or_else(|| {
        "Ответ Codex не содержит доступных лимитов. Повторите обновление позже.".to_string()
    })?;
    let primary = parse_window(limits.get("primary"));
    let secondary = parse_window(limits.get("secondary"));
    if primary.is_none() && secondary.is_none() {
        return Err("Codex пока не сообщил доступные окна лимитов. Обновите позже.".to_string());
    }
    Ok(UsageSnapshot {
        primary,
        secondary,
        updated_at,
        error: None,
    })
}

fn parse_window(value: Option<&Value>) -> Option<UsageWindow> {
    let value = value?;
    let used = value.get("usedPercent")?.as_i64()? as i32;
    Some(UsageWindow {
        used_percent: used.clamp(0, 100),
        window_minutes: value.get("windowDurationMins").and_then(Value::as_i64),
        resets_at: value.get("resetsAt").and_then(Value::as_i64),
    })
}

pub fn desktop_is_running() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let output = Command::new("tasklist.exe")
            .args(["/FI", "IMAGENAME eq Codex.exe", "/FO", "CSV", "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        output
            .ok()
            .filter(|output| output.status.success())
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .to_ascii_lowercase()
                    .contains("codex.exe")
            })
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    false
}

pub fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod status_tests {
    use super::classify_account_error;
    use crate::model::AccountStatusKind;

    #[test]
    fn classifies_auth_and_access_failures_for_routing() {
        assert_eq!(
            classify_account_error("HTTP 401 unauthorized"),
            AccountStatusKind::ReauthRequired
        );
        assert_eq!(
            classify_account_error("HTTP 403 account suspended"),
            AccountStatusKind::Banned
        );
        assert_eq!(
            classify_account_error("HTTP 403 policy restriction"),
            AccountStatusKind::AccessDenied
        );
        assert_eq!(
            classify_account_error("HTTP 403 {\"error\":{\"message\":\"not entitled\"}}"),
            AccountStatusKind::AccessDenied
        );
        assert_eq!(
            classify_account_error("network timeout"),
            AccountStatusKind::TemporaryError
        );
    }
}
