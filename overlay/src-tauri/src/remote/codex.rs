//! 远端 Codex 供应商切换。
//!
//! 原则：**与本体机逻辑保持一致** —— 用官方 v4.0.4 的纯投影 + 保序补丁
//! （`live::project::codex::CodexProjection` + `CodexConfigPatch`）算出远端
//! `config.toml` 的新内容，远端只做 I/O。
//!
//! 为什么不用官方本机那套 `codex_direct` 的 prepare/plan/run：那套是给「本机 live
//! 文件 + 钥匙串 + 托管账号 + 登录暂存 + 契约」设计的，远端没有这些概念（远端
//! 的 auth.json 由用户自己管）。这里只取其中**纯**的部分：
//!   行（`{auth, config}`）→ `CodexProjection::of` → `CodexConfigPatch` →
//!   打到远端现有 config.toml 的字节上 → 写回。
//! 这样远端用户的其它键（MCP、projects、插件）原样保留，与官方本机行为一致。
//!
//! auth.json 遵循本机语义：官方 provider 且有登录材料 → 写远端；第三方 → 保留
//! 远端用户自己的登录态不动。

use serde_json::Value;
use sha2::{Digest, Sha256};
use toml_edit::Value as TomlValue;

use crate::live::patch::LivePatch;
use crate::live::project::codex::{
    CodexConfigPatch, CodexProjection, Route, RouteWrite, RowInput, WEB_SEARCH_DISABLED,
};

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
/// - `settings`：DB 中该 provider 的 `settings_config`（含 `auth` 与 `config` 字段）
/// - `category`：`Some("official")` 表示官方 provider
/// - `profile`：catalog 工具 profile（与本体机 `resolve_codex_catalog_tool_profile` 同一来源）
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
    let official = category == Some("official");

    // 1) 纯投影：行（{auth, config}）→ 该供应商在 config.toml 里拥有的键与路由。
    let projection = CodexProjection::of(&RowInput {
        settings,
        official,
        proxy_injected_oauth: false,
    })
    .map_err(|e| format!("Codex 供应商配置无法投影: {e}"))?;

    // 2) 组装补丁：关键字段 + 独有字段 + 路由。远端没有「上一家」的概念
    //    （我们每次都是对远端现有文件做增量补丁），所以 outgoing/retired 留空。
    let route = match &projection.route {
        // 官方：不写选路（远端走 Codex 内置 openai）。休眠表地址用不到，给空串。
        Route::Official => RouteWrite::Official {
            dormant_base_url: String::new(),
        },
        Route::Custom { table, .. } => RouteWrite::Custom(table.clone()),
        Route::BuiltIn { id, table } => RouteWrite::BuiltIn {
            id: id.clone(),
            table: table.clone(),
        },
        Route::Default => RouteWrite::Default,
    };
    // 独有字段：投影带的 + 需要时把 web_search 改成 "disabled"（照官方本机
    // `exclusive_of` 的做法，判定用归一化后的配置文本）。
    let mut exclusive = projection.exclusive.clone();
    if crate::codex_config::codex_disables_web_search(
        settings,
        &projection.catalog_input_text(),
        profile,
    ) {
        exclusive.retain(|(key, _)| key != "web_search");
        exclusive.push((
            "web_search".to_string(),
            TomlValue::from(WEB_SEARCH_DISABLED),
        ));
    }
    let patch = CodexConfigPatch {
        top: projection.top.clone(),
        nested: projection.nested.clone(),
        exclusive,
        outgoing: Vec::new(),
        route,
        // 远端不生成模型目录（那是本机 live 的事）；行里自己的指针照投影走。
        catalog: false,
        retired: Vec::new(),
    };

    // 3) 读远端现有 config.toml，把补丁打上去（其余字节不动），原子写回。
    let pre = session.read_remote_text(&config_path, container).await?;
    let updated = patch
        .apply(
            std::path::Path::new(&config_path),
            pre.as_deref().map(str::as_bytes),
        )
        .map_err(|e| format!("远端 {config_path} 改写失败: {e}"))?;
    let config_text = String::from_utf8(updated)
        .map_err(|e| format!("改写结果不是 UTF-8: {e}"))?;

    let expected_hash = pre
        .as_deref()
        .map(|t| format!("{:x}", Sha256::digest(t.as_bytes())));
    session
        .write_settings_with_backup(&config_path, &config_text, container, expected_hash.as_deref())
        .await?;

    let mut notes = vec![format!("已更新远端 {config_path} 的关键字段")];
    notes.push("新建的 Codex 会话立即生效".to_string());

    // 4) auth.json：官方 provider 且有登录材料才写远端；第三方保留远端登录态。
    if official && crate::codex_config::codex_auth_has_credential_login_material(&auth) {
        let auth_text = serde_json::to_string_pretty(&auth)
            .map_err(|e| format!("序列化 auth.json 失败: {e}"))?;
        session
            .write_settings_with_backup(&auth_path, &auth_text, container, None)
            .await?;
        notes.push(format!("已写入远端 {auth_path}（官方登录态）"));
    } else {
        notes.push(format!("保留远端 {auth_path} 的登录态（第三方供应商）"));
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
