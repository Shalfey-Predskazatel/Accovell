use crate::{
    model::{CodexPreferences, PromptView, VaultDocument},
    storage,
};
use std::{fs, path::Path};
use toml_edit::{value, DocumentMut};

const CONFIG_FILE: &str = "config.toml";
const PROMPTS_DIR: &str = "prompts";

pub fn read_config(home: &Path) -> Result<String, String> {
    let path = home.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(path).map_err(|_| {
        "Не удалось прочитать ~/.codex/config.toml. Проверьте кодировку и доступ к файлу."
            .to_string()
    })
}

pub fn preferences(home: &Path) -> Result<CodexPreferences, String> {
    let source = read_config(home)?;
    if source.trim().is_empty() {
        return Ok(CodexPreferences::default());
    }
    let doc = source.parse::<DocumentMut>().map_err(|_| {
        "config.toml содержит синтаксическую ошибку. Исправьте файл в Codex и повторите."
            .to_string()
    })?;
    Ok(CodexPreferences {
        model: string_value(&doc, "model"),
        model_reasoning_effort: string_value(&doc, "model_reasoning_effort"),
        approval_policy: string_value(&doc, "approval_policy"),
        sandbox_mode: string_value(&doc, "sandbox_mode"),
        developer_instructions: string_value(&doc, "developer_instructions"),
    })
}

fn string_value(doc: &DocumentMut, key: &str) -> String {
    doc.get(key)
        .and_then(toml_edit::Item::as_str)
        .unwrap_or_default()
        .to_string()
}

pub fn update_preferences(home: &Path, prefs: &CodexPreferences) -> Result<(), String> {
    if prefs.model.len() > 160 || prefs.developer_instructions.len() > 60_000 {
        return Err("Модель или общая инструкция слишком длинная.".to_string());
    }
    if !matches!(
        prefs.model_reasoning_effort.as_str(),
        "" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
    ) {
        return Err("Выберите поддерживаемый уровень рассуждений: минимальный, низкий, средний, высокий или максимальный.".to_string());
    }
    if !matches!(prefs.approval_policy.as_str(), "" | "on-request" | "never") {
        return Err(
            "Неизвестный режим подтверждения действий. Выберите один из предложенных вариантов."
                .to_string(),
        );
    }
    if !matches!(
        prefs.sandbox_mode.as_str(),
        "" | "read-only" | "workspace-write" | "danger-full-access"
    ) {
        return Err(
            "Неизвестный режим песочницы. Выберите один из предложенных вариантов.".to_string(),
        );
    }
    let source = read_config(home)?;
    let mut doc = if source.trim().is_empty() {
        DocumentMut::new()
    } else {
        source.parse::<DocumentMut>().map_err(|_| {
            "config.toml содержит синтаксическую ошибку. Сначала исправьте его в Codex.".to_string()
        })?
    };
    set_string(&mut doc, "model", &prefs.model);
    set_string(
        &mut doc,
        "model_reasoning_effort",
        &prefs.model_reasoning_effort,
    );
    set_string(&mut doc, "approval_policy", &prefs.approval_policy);
    set_string(&mut doc, "sandbox_mode", &prefs.sandbox_mode);
    set_string(
        &mut doc,
        "developer_instructions",
        &prefs.developer_instructions,
    );
    storage::atomic_write(&home.join(CONFIG_FILE), doc.to_string().as_bytes())
}

/// Remove the preferences managed by Codex Accounts while preserving every
/// unrelated Codex setting and provider section.
pub fn reset_managed_preferences(home: &Path) -> Result<(), String> {
    let source = read_config(home)?;
    if source.trim().is_empty() {
        return Ok(());
    }
    let mut doc = source.parse::<DocumentMut>().map_err(|_| {
        "config.toml содержит синтаксическую ошибку. Исправьте его перед полным сбросом."
            .to_string()
    })?;
    let mut changed = false;
    for key in [
        "model",
        "model_reasoning_effort",
        "approval_policy",
        "sandbox_mode",
        "developer_instructions",
    ] {
        changed |= doc.as_table_mut().remove(key).is_some();
    }
    if changed {
        storage::atomic_write(&home.join(CONFIG_FILE), doc.to_string().as_bytes())?;
    }
    Ok(())
}

fn set_string(doc: &mut DocumentMut, key: &str, value_text: &str) {
    if value_text.trim().is_empty() {
        doc.as_table_mut().remove(key);
    } else {
        doc[key] = value(value_text);
    }
}

pub fn install_proxy(
    home: &Path,
    local_url: &str,
    vault: &mut VaultDocument,
) -> Result<bool, String> {
    let source = read_config(home)?;
    let mut doc = if source.trim().is_empty() {
        DocumentMut::new()
    } else {
        source.parse::<DocumentMut>()
            .map_err(|_| "config.toml содержит синтаксическую ошибку. Исправьте его, прежде чем подключать прокси.".to_string())?
    };
    let old_url = doc
        .get("openai_base_url")
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string);
    let old_provider = doc
        .get("model_provider")
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string);

    if vault.proxy_configured {
        if old_url.as_deref() == Some(local_url) && uses_builtin_provider(old_provider.as_deref()) {
            return Ok(false);
        }
        return Err("Настройки маршрута в config.toml изменились после подключения. Сначала отключите маршрут и проверьте файл, чтобы не перезаписать ручные изменения.".to_string());
    }
    if old_url.as_deref().is_some_and(is_other_local_proxy) {
        return Err("В config.toml уже указан локальный адрес API. Отключите прежний маршрут или восстановите исходные настройки перед подключением Codex Accounts.".to_string());
    }

    doc["model_provider"] = value("openai");
    doc["openai_base_url"] = value(local_url);
    storage::atomic_write(&home.join(CONFIG_FILE), doc.to_string().as_bytes())?;

    vault.original_openai_base_url_set = old_url.is_some();
    vault.original_openai_base_url = old_url;
    vault.original_model_provider_set = old_provider.is_some();
    vault.original_model_provider = old_provider;
    vault.proxy_configured = true;
    Ok(true)
}

pub fn migrate_proxy_url(
    home: &Path,
    old_url: &str,
    new_url: &str,
    vault: &mut VaultDocument,
) -> Result<bool, String> {
    let source = read_config(home)?;
    if source.trim().is_empty() {
        return Ok(false);
    }
    let mut doc = source.parse::<DocumentMut>().map_err(|_| {
        "config.toml содержит синтаксическую ошибку. Исправьте файл перед переносом локального маршрута.".to_string()
    })?;
    let current_url = doc
        .get("openai_base_url")
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string);
    if current_url.as_deref() != Some(old_url) && current_url.as_deref() != Some(new_url) {
        return Ok(false);
    }

    let current_provider = doc
        .get("model_provider")
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string);
    if !uses_builtin_provider(current_provider.as_deref()) {
        if !vault.original_model_provider_set && vault.original_model_provider.is_none() {
            vault.original_model_provider_set = true;
            vault.original_model_provider = current_provider.clone();
        }
        doc["model_provider"] = value("openai");
    }
    if current_url.as_deref() != Some(new_url) {
        doc["openai_base_url"] = value(new_url);
    }
    let changed = current_url.as_deref() != Some(new_url)
        || !uses_builtin_provider(current_provider.as_deref());
    if changed {
        storage::atomic_write(&home.join(CONFIG_FILE), doc.to_string().as_bytes())?;
    }
    vault.proxy_configured = true;
    Ok(changed)
}

pub fn proxy_route_is_active(home: &Path, local_url: &str) -> Result<bool, String> {
    let source = read_config(home)?;
    if source.trim().is_empty() {
        return Ok(false);
    }
    let doc = source.parse::<DocumentMut>().map_err(|_| {
        "config.toml содержит синтаксическую ошибку. Исправьте файл перед проверкой маршрута."
            .to_string()
    })?;
    let url = doc.get("openai_base_url").and_then(toml_edit::Item::as_str);
    let provider = doc.get("model_provider").and_then(toml_edit::Item::as_str);
    Ok(url == Some(local_url) && uses_builtin_provider(provider))
}

fn uses_builtin_provider(provider: Option<&str>) -> bool {
    provider.is_none_or(|provider| provider == "openai")
}

fn restore_original_provider(doc: &mut DocumentMut, vault: &VaultDocument) -> Result<(), String> {
    if vault.original_model_provider_set {
        let provider = vault.original_model_provider.as_deref().ok_or_else(|| {
            "В хранилище не сохранено имя исходного провайдера Codex. Не меняйте config.toml вручную до восстановления резервной копии.".to_string()
        })?;
        doc["model_provider"] = value(provider);
    } else {
        doc.as_table_mut().remove("model_provider");
    }
    Ok(())
}

fn restore_original_openai_url(doc: &mut DocumentMut, vault: &VaultDocument) -> Result<(), String> {
    if vault.original_openai_base_url_set {
        let url = vault.original_openai_base_url.as_deref().ok_or_else(|| {
            "В хранилище не сохранён исходный URL Codex. Не меняйте config.toml вручную до восстановления резервной копии.".to_string()
        })?;
        doc["openai_base_url"] = value(url);
    } else {
        doc.as_table_mut().remove("openai_base_url");
    }
    Ok(())
}

fn clear_proxy_snapshot(vault: &mut VaultDocument) {
    vault.proxy_configured = false;
    vault.original_openai_base_url = None;
    vault.original_openai_base_url_set = false;
    vault.original_model_provider = None;
    vault.original_model_provider_set = false;
}

/// Drop ownership of a stale route snapshot without touching config.toml.
/// This is used after a manual edit: the next explicit connection records
/// the user's current provider and endpoint as the new restore point.
pub fn forget_proxy_state(vault: &mut VaultDocument) {
    clear_proxy_snapshot(vault);
}

pub fn restore_proxy(
    home: &Path,
    local_url: &str,
    vault: &mut VaultDocument,
) -> Result<(), String> {
    if !vault.proxy_configured {
        return Ok(());
    }
    let source = read_config(home)?;
    let mut doc = if source.trim().is_empty() {
        DocumentMut::new()
    } else {
        source.parse::<DocumentMut>()
            .map_err(|_| "config.toml сейчас содержит синтаксическую ошибку. Исправьте его и повторите отключение.".to_string())?
    };
    let current_url = doc.get("openai_base_url").and_then(toml_edit::Item::as_str);
    let current_provider = doc.get("model_provider").and_then(toml_edit::Item::as_str);
    if current_url != Some(local_url) || !uses_builtin_provider(current_provider) {
        return Err("Маршрут или провайдер в config.toml уже изменён вручную. Codex Accounts не перезаписал файл; проверьте config.toml и восстановите исходный URL и провайдер.".to_string());
    }
    restore_original_openai_url(&mut doc, vault)?;
    restore_original_provider(&mut doc, vault)?;
    storage::atomic_write(&home.join(CONFIG_FILE), doc.to_string().as_bytes())?;
    clear_proxy_snapshot(vault);
    Ok(())
}

pub fn restore_proxy_text(
    source: &str,
    local_url: &str,
    vault: &VaultDocument,
) -> Result<String, String> {
    let mut doc = if source.trim().is_empty() {
        DocumentMut::new()
    } else {
        source.parse::<DocumentMut>()
            .map_err(|_| "config.toml содержит синтаксическую ошибку и не может быть добавлен в резервную копию.".to_string())?
    };
    let current_url = doc.get("openai_base_url").and_then(toml_edit::Item::as_str);
    let current_provider = doc.get("model_provider").and_then(toml_edit::Item::as_str);
    if current_url != Some(local_url) || !uses_builtin_provider(current_provider) {
        return Ok(source.to_string());
    }
    restore_original_openai_url(&mut doc, vault)?;
    restore_original_provider(&mut doc, vault)?;
    Ok(doc.to_string())
}

/*
 * Keep provider selection and endpoint restoration paired: openai_base_url is
 * only honored by Codex when the selected provider is the built-in `openai`.
 */

fn is_other_local_proxy(url: &str) -> bool {
    let lowered = url.to_ascii_lowercase();
    lowered.starts_with("http://127.0.0.1:")
        || lowered.starts_with("http://localhost:")
        || lowered.starts_with("https://127.0.0.1:")
        || lowered.starts_with("https://localhost:")
}

pub fn list_prompts(home: &Path) -> Result<Vec<PromptView>, String> {
    let dir = home.join(PROMPTS_DIR);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(dir)
        .map_err(|_| "Не удалось открыть папку prompts в CODEX_HOME.".to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Не удалось прочитать список файлов в папке prompts.".to_string())?;
    let mut prompts = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "В prompts найден файл с неподдерживаемым именем.".to_string())?
            .to_string();
        let content = fs::read_to_string(&path).map_err(|_| {
            format!("Не удалось прочитать prompt «{name}». Проверьте файл перед полным сбросом.")
        })?;
        prompts.push(PromptView { name, content });
    }
    prompts.sort_by_key(|prompt| prompt.name.to_lowercase());
    Ok(prompts)
}

pub fn save_prompt(home: &Path, name: &str, content: &str) -> Result<(), String> {
    if !valid_prompt_name(name) {
        return Err(
            "Для имени prompt используйте латинские буквы, цифры, дефис или подчёркивание."
                .to_string(),
        );
    }
    if content.len() > 100_000 {
        return Err("Prompt превышает лимит 100 КБ.".to_string());
    }
    let dir = home.join(PROMPTS_DIR);
    fs::create_dir_all(&dir)
        .map_err(|_| "Не удалось создать папку CODEX_HOME/prompts.".to_string())?;
    storage::atomic_write(&dir.join(format!("{name}.md")), content.as_bytes())
}

pub fn delete_prompt(home: &Path, name: &str) -> Result<(), String> {
    if !valid_prompt_name(name) {
        return Err("Имя prompt некорректно.".to_string());
    }
    match fs::remove_file(home.join(PROMPTS_DIR).join(format!("{name}.md"))) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(
            "Не удалось удалить prompt. Проверьте доступ к папке CODEX_HOME/prompts.".to_string(),
        ),
    }
}

pub fn validate_prompts(prompts: &[PromptView]) -> Result<(), String> {
    let mut names = std::collections::HashSet::with_capacity(prompts.len());
    for prompt in prompts {
        if !valid_prompt_name(&prompt.name) {
            return Err("В резервной копии есть prompt с неподдерживаемым именем.".to_string());
        }
        if prompt.content.len() > 100_000 {
            return Err(format!(
                "Prompt «{}» в копии превышает лимит 100 КБ.",
                prompt.name
            ));
        }
        if !names.insert(prompt.name.to_lowercase()) {
            return Err("В резервной копии повторяется имя prompt.".to_string());
        }
    }
    Ok(())
}

pub fn replace_prompts(home: &Path, prompts: &[PromptView]) -> Result<(), String> {
    validate_prompts(prompts)?;
    let dir = home.join(PROMPTS_DIR);
    if dir.exists() {
        for entry in fs::read_dir(&dir)
            .map_err(|_| "Не удалось прочитать папку prompts перед восстановлением.".to_string())?
        {
            let entry = entry.map_err(|_| {
                "Не удалось прочитать файл в папке prompts перед восстановлением.".to_string()
            })?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("md") {
                fs::remove_file(&path).map_err(|_| {
                    "Не удалось заменить текущие prompts. Проверьте доступ к папке CODEX_HOME/prompts.".to_string()
                })?;
            }
        }
    }
    for prompt in prompts {
        save_prompt(home, &prompt.name, &prompt.content)?;
    }
    Ok(())
}

fn valid_prompt_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_home() -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!(
            "codex-accounts-config-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&home).expect("create temporary Codex home");
        home
    }

    fn config(home: &Path) -> String {
        fs::read_to_string(home.join(CONFIG_FILE)).expect("read temporary config")
    }

    #[test]
    fn reset_managed_preferences_preserves_other_codex_configuration() {
        let home = temp_home();
        fs::write(
            home.join(CONFIG_FILE),
            r#"model = "gpt-6-luna"
model_reasoning_effort = "max"
approval_policy = "never"
sandbox_mode = "danger-full-access"
developer_instructions = "keep this only until full reset"
model_provider = "codex-lb"
other_setting = "preserve"

[model_providers.codex-lb]
base_url = "http://127.0.0.1:2455/backend-api/codex"
"#,
        )
        .expect("write initial config");

        reset_managed_preferences(&home).expect("reset application-managed preferences");

        let reset = config(&home)
            .parse::<DocumentMut>()
            .expect("parse reset config");
        for key in [
            "model",
            "model_reasoning_effort",
            "approval_policy",
            "sandbox_mode",
            "developer_instructions",
        ] {
            assert!(reset.get(key).is_none(), "{key} should be removed");
        }
        assert_eq!(string_value(&reset, "model_provider"), "codex-lb");
        assert_eq!(string_value(&reset, "other_setting"), "preserve");
        assert_eq!(
            reset["model_providers"]["codex-lb"]["base_url"].as_str(),
            Some("http://127.0.0.1:2455/backend-api/codex")
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn custom_provider_is_temporarily_replaced_and_restored() {
        let home = temp_home();
        let original = r#"model = "gpt-6-luna"
model_provider = "codex-lb"
model_reasoning_effort = "max"

[model_providers.codex-lb]
name = "openai"
base_url = "http://127.0.0.1:2455/backend-api/codex"
wire_api = "responses"
supports_websockets = true
requires_openai_auth = true
"#;
        fs::write(home.join(CONFIG_FILE), original).expect("write initial config");
        let local_url = "http://127.0.0.1:15789/test-key/backend-api/codex";
        let mut vault = VaultDocument::default();

        assert!(install_proxy(&home, local_url, &mut vault).expect("install local route"));
        let installed = config(&home)
            .parse::<DocumentMut>()
            .expect("parse installed config");
        assert_eq!(string_value(&installed, "model_provider"), "openai");
        assert_eq!(string_value(&installed, "openai_base_url"), local_url);
        assert_eq!(
            installed["model_providers"]["codex-lb"]["base_url"].as_str(),
            Some("http://127.0.0.1:2455/backend-api/codex")
        );
        assert_eq!(vault.original_model_provider.as_deref(), Some("codex-lb"));
        assert!(vault.original_model_provider_set);

        restore_proxy(&home, local_url, &mut vault).expect("restore previous route");
        let restored = config(&home)
            .parse::<DocumentMut>()
            .expect("parse restored config");
        assert_eq!(string_value(&restored, "model_provider"), "codex-lb");
        assert!(restored.get("openai_base_url").is_none());
        assert_eq!(
            restored["model_providers"]["codex-lb"]["base_url"].as_str(),
            Some("http://127.0.0.1:2455/backend-api/codex")
        );
        assert!(!vault.proxy_configured);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn active_proxy_requires_openai_provider_and_its_exact_local_endpoint() {
        let home = temp_home();
        let local_url = "http://127.0.0.1:15789/test-key/backend-api/codex";
        fs::write(
            home.join(CONFIG_FILE),
            format!("model_provider = \"openai\"\nopenai_base_url = \"{local_url}\"\n"),
        )
        .expect("write active local route");
        assert!(proxy_route_is_active(&home, local_url).expect("read active route"));

        fs::write(
            home.join(CONFIG_FILE),
            format!("model_provider = \"codex-lb\"\nopenai_base_url = \"{local_url}\"\n"),
        )
        .expect("write changed provider");
        assert!(!proxy_route_is_active(&home, local_url).expect("read changed provider"));

        fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"openai\"\nopenai_base_url = \"https://api.openai.com/v1\"\n",
        )
        .expect("write changed endpoint");
        assert!(!proxy_route_is_active(&home, local_url).expect("read changed endpoint"));
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn restore_does_not_overwrite_a_manual_provider_change() {
        let home = temp_home();
        fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"codex-lb\"\n[model_providers.codex-lb]\nbase_url = \"https://example.test\"\n",
        )
        .expect("write initial config");
        let local_url = "http://127.0.0.1:15789/test-key/backend-api/codex";
        let mut vault = VaultDocument::default();
        install_proxy(&home, local_url, &mut vault).expect("install local route");

        let mut edited = config(&home)
            .parse::<DocumentMut>()
            .expect("parse installed config");
        edited["model_provider"] = value("codex-lb");
        fs::write(home.join(CONFIG_FILE), edited.to_string()).expect("write manual edit");

        assert!(restore_proxy(&home, local_url, &mut vault).is_err());
        let after = config(&home)
            .parse::<DocumentMut>()
            .expect("parse unchanged config");
        assert_eq!(string_value(&after, "model_provider"), "codex-lb");
        assert_eq!(string_value(&after, "openai_base_url"), local_url);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn legacy_route_migration_preserves_custom_provider_for_later_restore() {
        let home = temp_home();
        let original = r#"model_provider = "codex-lb"
openai_base_url = "http://127.0.0.1:15789/backend-api/codex"

[model_providers.codex-lb]
name = "openai"
base_url = "http://127.0.0.1:2455/backend-api/codex"
"#;
        fs::write(home.join(CONFIG_FILE), original).expect("write legacy config");
        let new_url = "http://127.0.0.1:15789/test-key/backend-api/codex";
        let mut vault = VaultDocument {
            proxy_configured: true,
            original_openai_base_url: Some("https://api.openai.com/v1".to_string()),
            original_openai_base_url_set: true,
            ..VaultDocument::default()
        };

        assert!(migrate_proxy_url(
            &home,
            "http://127.0.0.1:15789/backend-api/codex",
            new_url,
            &mut vault,
        )
        .expect("migrate legacy route"));
        let migrated = config(&home)
            .parse::<DocumentMut>()
            .expect("parse migrated config");
        assert_eq!(string_value(&migrated, "model_provider"), "openai");
        assert_eq!(string_value(&migrated, "openai_base_url"), new_url);
        assert_eq!(vault.original_model_provider.as_deref(), Some("codex-lb"));

        restore_proxy(&home, new_url, &mut vault).expect("restore legacy route");
        let restored = config(&home)
            .parse::<DocumentMut>()
            .expect("parse restored config");
        assert_eq!(string_value(&restored, "model_provider"), "codex-lb");
        assert_eq!(
            string_value(&restored, "openai_base_url"),
            "https://api.openai.com/v1"
        );
        let _ = fs::remove_dir_all(home);
    }
}
