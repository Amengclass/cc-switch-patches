//! 远端 OpenCode 供应商切换（复用本机 `write_live_snapshot` 的 OpenCode 分支语义，
//! 远端只做 I/O）。
//!
//! 与本机一致：additive 模式 —— settings_config 可能是完整 config 结构（含
//! `$schema`/`provider`），先提取 `provider.{id}` 片段；能解析为
//! `OpenCodeProviderConfig` 或含 `npm`/`options` 才写；然后对远端
//! `~/.config/opencode/opencode.json` 的 `provider` 段 upsert `provider[id]`。

use serde_json::{json, Value};

use super::connection::RemoteSession;
use super::effect::EffectReport;

/// 对远端执行 OpenCode 供应商切换。
///
/// - `settings`：DB 中该 provider 的 `settings_config`
/// - `provider_id`：用于「完整 config 结构」时的片段提取
pub async fn apply_opencode_provider_settings(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
    target_name: &str,
    provider_name: &str,
    settings: &Value,
    provider_id: &str,
) -> Result<EffectReport, String> {
    let config_path = format!("{root}/.config/opencode/opencode.json");

    // 1) 复刻本机 live.rs OpenCode 分支的片段提取
    let config_to_write = if let Some(obj) = settings.as_object() {
        if obj.contains_key("$schema") || obj.contains_key("provider") {
            obj.get("provider")
                .and_then(|p| p.get(provider_id))
                .cloned()
                .unwrap_or_else(|| settings.clone())
        } else {
            settings.clone()
        }
    } else {
        settings.clone()
    };

    // 2) 校验：能解析为 OpenCodeProviderConfig，或含 npm/options（与本机一致）
    let parsed =
        serde_json::from_value::<crate::provider::OpenCodeProviderConfig>(config_to_write.clone());
    let is_valid = parsed.is_ok()
        || config_to_write.get("npm").is_some()
        || config_to_write.get("options").is_some();
    if !is_valid {
        return Err(format!(
            "OpenCode provider '{provider_id}' has invalid config structure for live config (must contain 'npm' or 'options')"
        ));
    }

    // 3) 读远端 opencode.json → provider 段 upsert → 原子写回
    let existing: Value = session
        .read_remote_text(&config_path, container)
        .await?
        .map(|t| serde_json::from_str(&t).unwrap_or_else(|_| json!({})))
        .unwrap_or_else(|| json!({}));
    let mut merged = existing;

    if !merged.get("provider").is_some_and(Value::is_object) {
        if merged.get("provider").is_some() {
            log::warn!("远端 opencode.json 的 provider 不是对象，已重置为空对象");
        }
        merged["provider"] = json!({});
    }
    if let Some(providers) = merged.get_mut("provider").and_then(|v| v.as_object_mut()) {
        providers.insert(provider_id.to_string(), config_to_write);
    }

    let sorted = crate::config::sort_json_keys(&merged);
    let text = serde_json::to_string_pretty(&sorted)
        .map_err(|e| format!("序列化 opencode.json 失败: {e}"))?;
    session
        .write_settings_with_backup(&config_path, &text, container, None)
        .await?;

    Ok(EffectReport {
        target: target_name.to_string(),
        provider_name: provider_name.to_string(),
        current_provider_id: None,
        conflicts_cleaned: 0,
        notes: vec![
            format!("已更新远端 {config_path} 的 provider.{provider_id}"),
            "新建的 OpenCode 会话立即生效".to_string(),
        ],
        warnings: Vec::new(),
    })
}
