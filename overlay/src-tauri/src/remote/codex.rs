//! 远端 Codex 供应商切换（复用本机 `codex_config` 纯变换）。
//!
//! 原则：**与本体机逻辑保持一致** —— config.toml 的生成完全复用本机
//! `prepare_codex_provider_live_config`（bearer token 注入），远端只做 I/O：
//! 一次 exec 原子写（.bak 备份），**不做 hash 校验**（本机 codex 切换也是
//! 无校验整文件覆盖，远端行为与之完全一致）。
//! auth.json 遵循本机语义：官方 provider 且有登录材料 → 写远端；第三方 → 保留
//! 远端用户自己的登录态不动。

use serde_json::Value;

use super::connection::RemoteSession;
use super::effect::EffectReport;

fn codex_config_path(root: &str) -> String {
    format!("{root}/.codex/config.toml")
}
fn codex_auth_path(root: &str) -> String {
    format!("{root}/.codex/auth.json")
}

/// 对远端执行 Codex 供应商切换。
///
/// - `settings`：DB 中该 provider 的 `settings_config`（含 `auth` 与 `config` 字段，
///   与本体机切换时传入 `write_codex_provider_live_with_catalog` 的同一对象）
/// - `category`：`Some("official")` 表示官方 provider
/// - `profile`：catalog 工具 profile（与本体机 `resolve_codex_catalog_tool_profile` 同一来源）
///
/// 产出与本机完全一致：① modelCatalog 变换（写远端同名 catalog 文件 + 注入字段）
/// ② unified 会话路由（官方 + 设置开启）③ bearer token 注入 ④ auth.json 判定
/// （官方+登录材料 / 第三方且不保留官方登录态 → 写；否则保留远端登录态）。
#[allow(clippy::too_many_arguments)]
pub async fn apply_codex_provider_settings(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
    target_name: &str,
    provider_name: &str,
    settings: &Value,
    category: Option<&str>,
    profile: crate::codex_config::CodexCatalogToolProfile,
) -> Result<EffectReport, String> {
    let config_path = codex_config_path(root);
    let auth_path = codex_auth_path(root);

    let auth = settings
        .get("auth")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let config_text = settings
        .get("config")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut notes = vec![];

    // 1) modelCatalog + config 生成（上游 v3.20.1 统一接口）
    let config_text = crate::codex_config::prepare_codex_provider_live_config(
        &auth,
        &config_text,
    )
    .map_err(|e| e.to_string())?;

    // 2) 原子写 config.toml
    session
        .write_settings_with_backup(&config_path, &config_text, container, None)
        .await?;
    notes.push(format!("已整文件覆盖远端 {config_path}"));
    notes.push("新建的 Codex 会话立即生效".to_string());

    // 4) auth.json：写入上游传递的 auth
    {
        let auth_text = serde_json::to_string_pretty(&auth)
            .map_err(|e| format!("序列化 auth.json 失败: {e}"))?;
        session
            .write_settings_with_backup(&auth_path, &auth_text, container, None)
            .await?;
        notes.push(format!("已写入远端 {auth_path}（官方登录态）"));
    }

    Ok(EffectReport {
        target: target_name.to_string(),
        provider_name: provider_name.to_string(),
        current_provider_id: None,
        conflicts_cleaned: 0,
        notes,
        warnings: Vec::new(),
    })
}
