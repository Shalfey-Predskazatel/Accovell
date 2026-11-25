use crate::model::{BackupPayload, EncryptedBackup, VaultDocument, VAULT_VERSION};
use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rand::{rngs::OsRng, RngCore};
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zeroize::Zeroizing;

const VAULT_FILE: &str = "vault.json";
const RESPONSE_OWNERS_FILE: &str = "response-owners.dat";
pub const RESPONSE_OWNER_LIMIT: usize = 4096;
const ENTROPY: &[u8] = b"codex-accounts-vault-v1";

#[derive(Debug, Clone, Default)]
pub struct ResponseOwnerIndex {
    pub owners: HashMap<String, String>,
    pub insertion_order: VecDeque<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedResponseOwnerIndex {
    version: u8,
    entries: Vec<(String, String)>,
}

pub fn app_data_root() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| {
            "Windows не сообщил путь LocalAppData. Проверьте профиль Windows.".to_string()
        })?;
    Ok(local.join("CodexAccounts"))
}

pub fn load(root: &Path) -> Result<VaultDocument, String> {
    let path = root.join(VAULT_FILE);
    if !path.exists() {
        return Ok(VaultDocument::default());
    }
    let bytes = fs::read(&path).map_err(|_| {
        "Не удалось прочитать хранилище аккаунтов. Проверьте доступ к папке приложения.".to_string()
    })?;
    let document: VaultDocument = serde_json::from_slice(&bytes)
        .map_err(|_| "Файл хранилища повреждён или создан другой версией. Сохраните его копию перед восстановлением.".to_string())?;
    if document.version > VAULT_VERSION {
        return Err(
            "Хранилище создано более новой версией приложения. Обновите Codex Accounts."
                .to_string(),
        );
    }
    Ok(document)
}

pub fn save(root: &Path, document: &VaultDocument) -> Result<(), String> {
    fs::create_dir_all(root)
        .map_err(|_| "Не удалось создать папку приложения в LocalAppData.".to_string())?;
    let path = root.join(VAULT_FILE);
    let bytes = serde_json::to_vec_pretty(document)
        .map_err(|_| "Не удалось подготовить данные аккаунтов для сохранения.".to_string())?;
    atomic_write(&path, &bytes)
}

/// Response IDs are account-scoped. Persist this small routing index so a
/// restarted manager never guesses a different account for an existing chat.
/// The index contains no OAuth tokens and is protected with the same Windows
/// DPAPI boundary as the local auth records.
pub fn load_response_owners(root: &Path) -> ResponseOwnerIndex {
    let Ok(encoded) = fs::read_to_string(root.join(RESPONSE_OWNERS_FILE)) else {
        return ResponseOwnerIndex::default();
    };
    let Ok(plaintext) = open_auth(encoded.trim()) else {
        return ResponseOwnerIndex::default();
    };
    // The legacy format stored only a map, so it had no reliable eviction order.
    // Continue reading it for upgrades and retain a deterministic migration
    // order; all new writes preserve the true insertion order.
    let entries = match serde_json::from_slice::<PersistedResponseOwnerIndex>(&plaintext) {
        Ok(index) if index.version == 2 => index.entries,
        _ => {
            let Ok(legacy) = serde_json::from_slice::<HashMap<String, String>>(&plaintext) else {
                return ResponseOwnerIndex::default();
            };
            let mut entries = legacy.into_iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            entries
        }
    };

    let mut index = ResponseOwnerIndex::default();
    for (response_id, account_id) in entries {
        if response_id.trim().is_empty() || account_id.trim().is_empty() {
            continue;
        }
        if index
            .owners
            .insert(response_id.clone(), account_id)
            .is_none()
        {
            index.insertion_order.push_back(response_id);
        }
    }
    while index.owners.len() > RESPONSE_OWNER_LIMIT {
        let Some(oldest) = index.insertion_order.pop_front() else {
            break;
        };
        index.owners.remove(&oldest);
    }
    index
}

pub fn save_response_owners(root: &Path, index: &ResponseOwnerIndex) -> Result<(), String> {
    let entries = index
        .insertion_order
        .iter()
        .filter_map(|response_id| {
            index
                .owners
                .get(response_id)
                .map(|account_id| (response_id.clone(), account_id.clone()))
        })
        .collect();
    let plaintext = serde_json::to_vec(&PersistedResponseOwnerIndex {
        version: 2,
        entries,
    })
    .map_err(|_| "Не удалось подготовить привязки чатов к аккаунтам.".to_string())?;
    let encoded = seal_auth(&plaintext)?;
    atomic_write(&root.join(RESPONSE_OWNERS_FILE), encoded.as_bytes())
}

pub fn seal_auth(auth_json: &[u8]) -> Result<String, String> {
    protect(auth_json).map(|bytes| B64.encode(bytes))
}

pub fn open_auth(encoded: &str) -> Result<Vec<u8>, String> {
    let protected = B64.decode(encoded).map_err(|_| {
        "Зашифрованные данные аккаунта повреждены. Восстановите резервную копию.".to_string()
    })?;
    unprotect(&protected)
}

pub fn hash_admin_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| "Не удалось надёжно сохранить пароль администратора.".to_string())
}

pub fn verify_admin_password(encoded: &str, password: &str) -> bool {
    PasswordHash::new(encoded).ok().is_some_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Не удалось определить папку файла.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|_| "Не удалось создать папку для сохранения.".to_string())?;
    let temp = parent.join(format!(".codex-accounts-{}.tmp", Uuid::new_v4()));
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| {
                "Не удалось создать временный файл для безопасного переключения.".to_string()
            })?;
        file.write_all(bytes)
            .map_err(|_| "Не удалось полностью записать временный файл.".to_string())?;
        file.sync_all()
            .map_err(|_| "Не удалось зафиксировать временный файл на диске.".to_string())?;
        drop(file);
        replace_file(&temp, path)?;
        Ok::<(), String>(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;

    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
        fn GetLastError() -> u32;
    }

    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let success = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            0x1 | 0x8, // REPLACE_EXISTING | WRITE_THROUGH
        )
    };
    if success == 0 {
        let code = unsafe { GetLastError() };
        Err(format!(
            "Windows не смог безопасно заменить файл (код {code}). Закройте Codex и повторите."
        ))
    } else {
        Ok(())
    }
}

fn protect(input: &[u8]) -> Result<Vec<u8>, String> {
    use std::{ffi::c_void, ptr};

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "Crypt32")]
    extern "system" {
        fn CryptProtectData(
            data_in: *const DataBlob,
            description: *const u16,
            optional_entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
    }
    #[link(name = "Kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
        fn GetLastError() -> u32;
    }

    let input_blob = DataBlob {
        cb_data: input.len() as u32,
        pb_data: input.as_ptr() as *mut u8,
    };
    let entropy_blob = DataBlob {
        cb_data: ENTROPY.len() as u32,
        pb_data: ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &input_blob,
            ptr::null(),
            &entropy_blob,
            ptr::null_mut(),
            ptr::null_mut(),
            0x1,
            &mut output,
        )
    };
    if ok == 0 || output.pb_data.is_null() {
        return Err(format!(
            "Windows DPAPI не смог зашифровать данные (код {}).",
            unsafe { GetLastError() }
        ));
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() };
    unsafe {
        std::ptr::write_bytes(output.pb_data, 0, output.cb_data as usize);
        LocalFree(output.pb_data as *mut c_void);
    }
    Ok(result)
}

fn unprotect(input: &[u8]) -> Result<Vec<u8>, String> {
    use std::{ffi::c_void, ptr};

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "Crypt32")]
    extern "system" {
        fn CryptUnprotectData(
            data_in: *const DataBlob,
            description: *mut *mut u16,
            optional_entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
    }
    #[link(name = "Kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
        fn GetLastError() -> u32;
    }

    let input_blob = DataBlob {
        cb_data: input.len() as u32,
        pb_data: input.as_ptr() as *mut u8,
    };
    let entropy_blob = DataBlob {
        cb_data: ENTROPY.len() as u32,
        pb_data: ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };
    let mut description: *mut u16 = ptr::null_mut();
    let ok = unsafe {
        CryptUnprotectData(
            &input_blob,
            &mut description,
            &entropy_blob,
            ptr::null_mut(),
            ptr::null_mut(),
            0x1,
            &mut output,
        )
    };
    if !description.is_null() {
        unsafe {
            LocalFree(description as *mut c_void);
        }
    }
    if ok == 0 || output.pb_data.is_null() {
        return Err(format!("Windows DPAPI не смог расшифровать данные (код {}). Проверьте пользователя Windows или восстановите копию с паролем.", unsafe { GetLastError() }));
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec() };
    unsafe {
        std::ptr::write_bytes(output.pb_data, 0, output.cb_data as usize);
        LocalFree(output.pb_data as *mut c_void);
    }
    Ok(result)
}

pub fn ensure_vault(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root)
        .map_err(|_| "Не удалось создать локальную папку приложения.".to_string())?;
    let path = root.join(VAULT_FILE);
    if !path.exists() {
        save(root, &VaultDocument::default())?;
    }
    Ok(())
}

/// Move volatile login and routing data aside so a full reset can either
/// commit their removal or put every item back after a later write fails.
pub struct RuntimeDataReset {
    backup_dir: PathBuf,
    moved: Vec<String>,
}

impl RuntimeDataReset {
    pub fn rollback(self, root: &Path) -> Result<(), String> {
        let mut errors = Vec::new();
        for name in self.moved.iter().rev() {
            let source = self.backup_dir.join(name);
            let destination = root.join(name);
            if path_exists(&source) {
                if let Err(error) = remove_path(&destination) {
                    errors.push(format!("не удалось убрать новый файл {name}: {error}"));
                    continue;
                }
                if let Err(error) = fs::rename(&source, &destination) {
                    errors.push(format!("не удалось вернуть {name}: {error}"));
                }
            }
        }
        if let Err(error) = fs::remove_dir_all(&self.backup_dir) {
            if error.kind() != std::io::ErrorKind::NotFound {
                errors.push(format!(
                    "не удалось удалить временную папку сброса: {error}"
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub fn commit(self) -> Result<(), String> {
        fs::remove_dir_all(&self.backup_dir).map_err(|error| {
            format!(
                "Не удалось удалить временную копию данных сброса ({}): {error}",
                self.backup_dir.display()
            )
        })
    }
}

pub fn stage_runtime_data_reset(root: &Path) -> Result<RuntimeDataReset, String> {
    let backup_dir = root.join(format!(".reset-{}", Uuid::new_v4()));
    fs::create_dir(&backup_dir)
        .map_err(|_| "Не удалось подготовить безопасный полный сброс данных.".to_string())?;
    let mut snapshot = RuntimeDataReset {
        backup_dir,
        moved: Vec::new(),
    };
    for name in [RESPONSE_OWNERS_FILE, "staging", "usage"] {
        let source = root.join(name);
        if !path_exists(&source) {
            continue;
        }
        if let Err(error) = fs::rename(&source, snapshot.backup_dir.join(name)) {
            let rollback = snapshot.rollback(root);
            let detail = format!("Не удалось временно убрать {name} при полном сбросе: {error}.");
            return Err(match rollback {
                Ok(()) => detail,
                Err(restore) => {
                    format!("{detail} Не удалось вернуть исходные временные данные: {restore}.")
                }
            });
        }
        snapshot.moved.push(name.to_string());
    }
    Ok(snapshot)
}

fn path_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

pub fn encode_backup(payload: &BackupPayload, password: Option<&str>) -> Result<Vec<u8>, String> {
    let plaintext = serde_json::to_vec_pretty(payload)
        .map_err(|_| "Не удалось подготовить резервную копию.".to_string())?;
    let Some(password) = password.filter(|value| !value.is_empty()) else {
        return Ok(plaintext);
    };
    if password.chars().count() < 8 {
        return Err(
            "Для шифрования резервной копии задайте пароль длиной не менее 8 символов.".to_string(),
        );
    }
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::default()
        .hash_password_into(password.as_bytes(), &salt, &mut *key)
        .map_err(|_| "Не удалось подготовить ключ шифрования резервной копии.".to_string())?;
    let cipher = Aes256Gcm::new_from_slice(&*key)
        .map_err(|_| "Не удалось создать ключ шифрования резервной копии.".to_string())?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|_| "Не удалось зашифровать резервную копию.".to_string())?;
    let envelope = EncryptedBackup {
        format: "codex-accounts-encrypted".to_string(),
        version: 1,
        salt: B64.encode(salt),
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ciphertext),
    };
    serde_json::to_vec_pretty(&envelope)
        .map_err(|_| "Не удалось подготовить зашифрованную резервную копию.".to_string())
}

pub fn decode_backup(bytes: &[u8], password: Option<&str>) -> Result<BackupPayload, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| {
        "Файл не похож на резервную копию Codex Accounts. Выберите другой файл.".to_string()
    })?;
    if value.get("format").and_then(serde_json::Value::as_str) == Some("codex-accounts-encrypted") {
        let envelope: EncryptedBackup = serde_json::from_value(value)
            .map_err(|_| "Структура зашифрованной копии повреждена.".to_string())?;
        let password = password.filter(|value| !value.is_empty()).ok_or_else(|| {
            "Введите пароль, которым была зашифрована резервная копия.".to_string()
        })?;
        let salt = B64
            .decode(envelope.salt)
            .map_err(|_| "Соль копии повреждена.".to_string())?;
        let nonce = B64
            .decode(envelope.nonce)
            .map_err(|_| "Вектор шифрования повреждён.".to_string())?;
        let ciphertext = B64
            .decode(envelope.ciphertext)
            .map_err(|_| "Зашифрованные данные повреждены.".to_string())?;
        if salt.len() != 16 || nonce.len() != 12 {
            return Err("Параметры шифрования в резервной копии повреждены.".to_string());
        }
        let mut key = Zeroizing::new([0u8; 32]);
        Argon2::default()
            .hash_password_into(password.as_bytes(), &salt, &mut *key)
            .map_err(|_| "Не удалось подготовить ключ для расшифровки.".to_string())?;
        let cipher = Aes256Gcm::new_from_slice(&*key)
            .map_err(|_| "Не удалось открыть зашифрованную копию.".to_string())?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
            .map_err(|_| {
                "Пароль не подходит или резервная копия повреждена. Проверьте пароль и повторите."
                    .to_string()
            });
        let plaintext = plaintext?;
        return serde_json::from_slice(&plaintext)
            .map_err(|_| "После расшифровки данные резервной копии не читаются.".to_string());
    }
    serde_json::from_value(value)
        .map_err(|_| "В резервной копии отсутствуют обязательные данные.".to_string())
}

pub fn set_startup(enabled: bool, start_in_tray: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};
        let current_user = RegKey::predef(HKEY_CURRENT_USER);
        let (run_key, _) = current_user
            .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
            .map_err(|_| {
                "Не удалось открыть ключ автозапуска Windows для текущего пользователя.".to_string()
            })?;
        if enabled {
            let exe = std::env::current_exe().map_err(|_| {
                "Не удалось определить путь приложения для автозапуска.".to_string()
            })?;
            let background = if start_in_tray { " --background" } else { "" };
            let command = format!("\"{}\"{}", exe.display(), background);
            run_key
                .set_value("CodexAccounts", &command)
                .map_err(|_| "Windows не сохранил настройку автозапуска. Проверьте доступ к текущему профилю.".to_string())?;
        } else {
            match run_key.delete_value("CodexAccounts") {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err("Не удалось отключить автозапуск в настройках Windows.".to_string())
                }
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (enabled, start_in_tray);
        Err("Автозапуск реализован только для Windows.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppSettings, BackupAccount, PromptView};

    fn backup_payload() -> BackupPayload {
        BackupPayload {
            format: "codex-accounts-backup".to_string(),
            version: 1,
            exported_at: 1,
            accounts: vec![BackupAccount {
                account_id: "test-account".to_string(),
                email: "test@example.test".to_string(),
                plan: "Plus".to_string(),
                created_at: 1,
                last_used_at: Some(2),
                usage: None,
                status: crate::model::AccountStatus::healthy(),
                auth_json: "test-secret-auth".to_string(),
            }],
            settings: AppSettings::default(),
            codex_config_toml: "model = \"test-model\"\n".to_string(),
            prompts: vec![PromptView {
                name: "review".to_string(),
                content: "Review this change".to_string(),
            }],
        }
    }

    #[test]
    fn encrypted_backup_roundtrips_with_password_and_hides_plaintext() {
        let payload = backup_payload();
        let encoded =
            encode_backup(&payload, Some("correct horse battery staple")).expect("encrypt backup");
        let envelope: serde_json::Value =
            serde_json::from_slice(&encoded).expect("read encrypted envelope");

        assert_eq!(envelope["format"], "codex-accounts-encrypted");
        assert!(!String::from_utf8_lossy(&encoded).contains("test-secret-auth"));
        assert!(decode_backup(&encoded, Some("incorrect password")).is_err());

        let restored =
            decode_backup(&encoded, Some("correct horse battery staple")).expect("decrypt backup");
        assert_eq!(restored.accounts[0].auth_json, "test-secret-auth");
        assert_eq!(restored.accounts[0].last_used_at, Some(2));
        assert_eq!(restored.prompts[0].name, "review");
    }

    #[test]
    fn response_owner_cache_roundtrips_without_plaintext_response_ids() {
        let root =
            std::env::temp_dir().join(format!("codex-accounts-owner-test-{}", Uuid::new_v4()));
        let mut owners = ResponseOwnerIndex::default();
        owners.owners.insert(
            "resp_sensitive_test_id".to_string(),
            "account-123".to_string(),
        );
        owners
            .insertion_order
            .push_back("resp_sensitive_test_id".to_string());

        save_response_owners(&root, &owners).expect("save encrypted routing cache");
        let stored = fs::read_to_string(root.join(RESPONSE_OWNERS_FILE)).expect("read cache");
        assert!(!stored.contains("resp_sensitive_test_id"));
        let loaded = load_response_owners(&root);
        assert_eq!(loaded.owners, owners.owners);
        assert_eq!(loaded.insertion_order, owners.insertion_order);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn staged_runtime_reset_can_restore_all_moved_data() {
        let root =
            std::env::temp_dir().join(format!("codex-accounts-reset-test-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("staging")).expect("create staging data");
        fs::create_dir_all(root.join("usage")).expect("create usage data");
        fs::write(root.join(RESPONSE_OWNERS_FILE), "sealed routing data")
            .expect("write routing data");
        fs::write(root.join("staging").join("auth.json"), "temporary auth")
            .expect("write staging auth");
        fs::write(root.join("usage").join("auth.json"), "temporary usage auth")
            .expect("write usage auth");

        let snapshot = stage_runtime_data_reset(&root).expect("stage reset transaction");
        assert!(!path_exists(&root.join(RESPONSE_OWNERS_FILE)));
        assert!(!path_exists(&root.join("staging")));
        assert!(!path_exists(&root.join("usage")));

        snapshot.rollback(&root).expect("rollback runtime reset");
        assert_eq!(
            fs::read_to_string(root.join(RESPONSE_OWNERS_FILE)).unwrap(),
            "sealed routing data"
        );
        assert_eq!(
            fs::read_to_string(root.join("staging").join("auth.json")).unwrap(),
            "temporary auth"
        );
        assert_eq!(
            fs::read_to_string(root.join("usage").join("auth.json")).unwrap(),
            "temporary usage auth"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn committing_runtime_reset_removes_staged_data_and_backup() {
        let root =
            std::env::temp_dir().join(format!("codex-accounts-reset-test-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("staging")).expect("create staging data");
        fs::write(root.join("staging").join("auth.json"), "temporary auth")
            .expect("write staging auth");

        let snapshot = stage_runtime_data_reset(&root).expect("stage reset transaction");
        let backup_dir = snapshot.backup_dir.clone();
        snapshot.commit().expect("commit runtime reset");

        assert!(!path_exists(&root.join("staging")));
        assert!(!path_exists(&backup_dir));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unencrypted_backup_roundtrips_as_plain_json() {
        let payload = backup_payload();
        let encoded = encode_backup(&payload, None).expect("encode plain backup");
        let restored = decode_backup(&encoded, None).expect("decode plain backup");

        assert_eq!(restored.format, "codex-accounts-backup");
        assert_eq!(restored.accounts[0].auth_json, "test-secret-auth");
    }
}
