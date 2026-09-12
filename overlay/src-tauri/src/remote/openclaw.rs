//! 远端 OpenClaw 供应商切换（复用本机 `write_live_snapshot` 的 OpenClaw 分支语义，
//! 远端只做 I/O）。
//!
//! 与本机一致：additive 模式 —— settings_config 能解析为 `OpenClawProviderConfig`
//! 或含 `baseUrl`/`api`/`models` 才写；对远端 `~/.openclaw/openclaw.json` 的
//! `models.providers` 段 upsert `providers[id]`（保留 `models.mode` 与其他顶层段）。
//! 保形回写：复用本机 `openclaw_config::upsert_provider_preserve_format` 的
//! json-five round-trip，仅重写 `models` 段，远端文件顶层注释/排版原样保留；
//! 写前以读到内容的 sha256 做脏写防护（文件被外部改动则拒绝，对齐本机 save()）。

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::connection::RemoteSession;
use super::effect::EffectReport;

/// 对远端执行 OpenClaw 供应商切换。
///
/// - `settings`：DB 中该 provider 的 `settings_config`
/// - `provider_id`：写入 `models.providers.{provider_id}`
pub async fn apply_openclaw_provider_settings(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
    target_name: &str,
    provider_name: &str,
    settings: &Value,
    provider_id: &str,
) -> Result<EffectReport, String> {
    let config_path = format!("{root}/.openclaw/openclaw.json");

    // 校验（与本机 live.rs OpenClaw 分支一致）
    let parsed =
        serde_json::from_value::<crate::openclaw_config::OpenClawProviderConfig>(settings.clone());
    let is_valid = parsed.is_ok()
        || settings.get("baseUrl").is_some()
        || settings.get("api").is_some()
        || settings.get("models").is_some();
    if !is_valid {
        return Err(format!(
            "OpenClaw provider '{provider_id}' has invalid config structure for live config (must contain 'baseUrl', 'api', or 'models')"
        ));
    }

    // 读远端 openclaw.json（JSON5；不存在/分隔空文件 → 默认骨架起步）
    let read = session.read_remote_text(&config_path, container).await?;
    let source = read.as_deref();

    // 保形 round-trip upsert（复用本机同一 merge + json-five 回写）：仅重写
    // models 段，远端文件顶层注释/排版原样保留；源非合法 JSON5 时报错。
    let new_text = crate::openclaw_config::upsert_provider_preserve_format(
        source,
        provider_id,
        settings,
    )
    .map_err(|e| format!("处理远端 {config_path} 失败: {e}"))?;

    // 脏写防护（对齐本机 save() 的「磁盘被外部改动则拒绝」）：以读到内容 sha256
    // 作 expected_hash，同一脚本内先校验远端文件未被外部修改，冲突则中止不覆盖。
    let expected_hash = source.map(|t| format!("{:x}", Sha256::digest(t.as_bytes())));
    session
        .write_settings_with_backup(&config_path, &new_text, container, expected_hash.as_deref())
        .await?;

    Ok(EffectReport {
        target: target_name.to_string(),
        provider_name: provider_name.to_string(),
        current_provider_id: None,
        conflicts_cleaned: 0,
        notes: vec![
            format!("已更新远端 {config_path} 的 models.providers.{provider_id}"),
            "新建的 OpenClaw 会话立即生效".to_string(),
        ],
        warnings: Vec::new(),
    })
}
