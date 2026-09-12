//! 远端 Pi 供应商切换（复用本机 `pi_config` 语义，远端只做 I/O）。
//!
//! 与本机一致：additive 模式 —— `~/.pi/agent/models.json` 的 `providers`
//! 段 upsert `providers.{id}`。Pi 的 settings_config 是完整 provider 节点
//! （含 name/baseUrl/apiKey/models 等），直接写入。

use serde_json::{json, Value};

use super::connection::RemoteSession;
use super::effect::EffectReport;

/// 对远端执行 Pi 供应商切换。
///
/// - `settings`：DB 中该 provider 的 `settings_config`（Pi 原生 provider 节点）
/// - `provider_id`：用于 `models.json` 的 `providers.{id}` 键
pub async fn apply_pi_provider_settings(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
    target_name: &str,
    provider_name: &str,
    settings: &Value,
    provider_id: &str,
) -> Result<EffectReport, String> {
    let config_path = format!("{root}/.pi/agent/models.json");

    // Pi 的 settings_config 就是完整的 provider 节点，直接写入。
    // 与本机 pi_config::insert_pi_provider / replace_pi_provider 语义一致。
    let config_to_write = settings.clone();

    // 校验：必须是对象（与 pi_config::validate_provider_node 一致）
    if !config_to_write.is_object() {
        return Err(format!("Pi provider '{provider_id}' 的配置必须是对象"));
    }

    // 读远端 models.json → providers 段 upsert → 原子写回
    let existing: Value = session
        .read_remote_text(&config_path, container)
        .await?
        .map(|t| serde_json::from_str(&t).unwrap_or_else(|_| json!({})))
        .unwrap_or_else(|| json!({}));
    let mut merged = existing;

    if !merged.get("providers").is_some_and(Value::is_object) {
        if merged.get("providers").is_some() {
            log::warn!("远端 models.json 的 providers 不是对象，已重置为空对象");
        }
        merged["providers"] = json!({});
    }
    if let Some(providers) = merged.get_mut("providers").and_then(|v| v.as_object_mut()) {
        providers.insert(provider_id.to_string(), config_to_write);
    }

    let sorted = crate::config::sort_json_keys(&merged);
    let text = serde_json::to_string_pretty(&sorted)
        .map_err(|e| format!("序列化 models.json 失败: {e}"))?;
    session
        .write_settings_with_backup(&config_path, &text, container, None)
        .await?;

    Ok(EffectReport {
        target: target_name.to_string(),
        provider_name: provider_name.to_string(),
        current_provider_id: None,
        conflicts_cleaned: 0,
        notes: vec![
            format!("已更新远端 {config_path} 的 providers.{provider_id}"),
            "新建的 Pi 会话立即生效".to_string(),
        ],
        warnings: Vec::new(),
    })
}
