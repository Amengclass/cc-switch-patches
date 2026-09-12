//! 远端 Grok Build 供应商切换（复用本机 `grok_config` 变换）。
//!
//! 原则：**与本体机逻辑保持一致** —— 本机 `write_grok_provider_live` 里
//! `settings_config["config"]` 即最终 config.toml 文本（无 token 注入/无 unified），
//! 非官方供应商必须通过 `validate_config_toml` 完整形状校验；官方按快照原样写回。
//! 远端只做 I/O：一次 exec 原子写（.bak 备份），不做 hash 校验（与本机一致）。

use serde_json::Value;

use super::connection::RemoteSession;
use super::effect::EffectReport;

/// 对远端执行 Grok Build 供应商切换。
///
/// - `settings`：DB 中该 provider 的 `settings_config`（`config` 字段即 TOML 文本）
/// - `category`：`Some("official")` 表示官方 provider
pub async fn apply_grok_provider_settings(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
    target_name: &str,
    provider_name: &str,
    settings: &Value,
    category: Option<&str>,
) -> Result<EffectReport, String> {
    let config_path = format!("{root}/.grok/config.toml");
    let config = settings
        .get("config")
        .and_then(Value::as_str)
        .ok_or_else(|| "Grok Build 配置缺少 config 字段".to_string())?;

    // 与本机 write_grok_provider_live 一致：非官方必须携带完整的自定义模型配置
    if category != Some("official") {
        crate::grok_config::validate_config_toml(config).map_err(|e| e.to_string())?;
    }

    // 一次 exec 原子写（存在才备份 .bak + 失败清理 tmp）；不做 hash 校验，
    // 与本机 grok 切换行为完全一致。
    session
        .write_settings_with_backup(&config_path, config, container, None)
        .await?;

    Ok(EffectReport {
        target: target_name.to_string(),
        provider_name: provider_name.to_string(),
        current_provider_id: None,
        conflicts_cleaned: 0,
        notes: vec![
            format!("已整文件覆盖远端 {config_path}"),
            "新建的 Grok Build 会话立即生效".to_string(),
        ],
        warnings: Vec::new(),
    })
}
