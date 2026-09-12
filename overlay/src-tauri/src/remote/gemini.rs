//! 远端 Gemini 供应商切换（复用本机 `gemini_config` 变换 + 本机
//! `write_gemini_live` 的语义，远端只做 I/O）。
//!
//! 与本机一致的行为：
//! ① `~/.gemini/.env`：provider settings → `json_to_env` → `serialize_env_file` 原子写；
//!    非 Google 官方（API Key 模式）先 `validate_gemini_settings_strict`；
//! ② `~/.gemini/settings.json`：读远端现有 → merge provider `config` 对象
//!    （保留 mcpServers 等字段；config 为 null/缺失则保留现有）→
//!    设置 `security.auth.selectedType`（Google 官方=oauth-personal，其余=gemini-api-key）；
//! 不做 hash 校验（与本机一致），无校验整文件覆盖。

use serde_json::{json, Value};

use super::connection::RemoteSession;
use super::effect::EffectReport;
use crate::provider::Provider;
use crate::services::provider::gemini_auth::{detect_gemini_auth_type, GeminiAuthType};

/// 对远端执行 Gemini 供应商切换。
///
/// - `settings`：DB 中该 provider 的 `settings_config`（含 `env` 与 `config` 字段）
/// - `provider`：用于与本机一致的 auth 类型检测（Google 官方 / Packycode / 通用）
#[allow(clippy::too_many_arguments)]
pub async fn apply_gemini_provider_settings(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
    target_name: &str,
    provider_name: &str,
    settings: &Value,
    provider: &Provider,
    route_proxy: bool,
    route_base: Option<&str>,
    port: u16,
    host_id: &str,
) -> Result<EffectReport, String> {
    let env_path = format!("{root}/.gemini/.env");
    let settings_path = format!("{root}/.gemini/settings.json");

    // 与本机 write_gemini_live 同源的一次性 auth 类型检测
    let auth_type = detect_gemini_auth_type(provider);

    // ① .env：provider settings → env 键值 → .env 文本（复用本机纯函数）
    let mut env_map = crate::gemini_config::json_to_env(settings).map_err(|e| e.to_string())?;
    if route_proxy {
        // 走本机路由：与本机 gemini 接管逐字段一致（services::proxy 的
        // takeover_live_config_strict gemini 分支）——base_url 指向远端隧道
        // （宿主机=localhost；容器=网关 IP），token 用 PROXY_MANAGED:<host_id>
        // 占位（本机代理识别 host_id 按该远端自己的当前供应商路由，并注入真实密钥）。
        let base = route_base.unwrap_or("localhost");
        env_map.insert(
            "GOOGLE_GEMINI_BASE_URL".to_string(),
            format!("http://{base}:{port}"),
        );
        env_map.insert(
            "GEMINI_API_KEY".to_string(),
            crate::proxy::remote_route::remote_token_for(host_id),
        );
    }
    let env_text = crate::gemini_config::serialize_env_file(&env_map);
    match auth_type {
        GeminiAuthType::GoogleOfficial => {}
        GeminiAuthType::Packycode | GeminiAuthType::Generic => {
            // API Key 模式必须带 GEMINI_API_KEY（与本机 validate 一致）
            crate::gemini_config::validate_gemini_settings_strict(settings)
                .map_err(|e| e.to_string())?;
        }
    }
    session
        .write_settings_with_backup(&env_path, &env_text, container, None)
        .await?;

    // ② settings.json：读远端现有 → merge provider config → 设置 selectedType → 原子写回
    let existing: Value = session
        .read_remote_text(&settings_path, container)
        .await?
        .map(|t| serde_json::from_str(&t).unwrap_or_else(|_| json!({})))
        .unwrap_or_else(|| json!({}));
    let mut merged = existing;

    if let Some(config_value) = settings.get("config") {
        if config_value.is_object() {
            if let (Some(merged_obj), Some(config_obj)) =
                (merged.as_object_mut(), config_value.as_object())
            {
                for (k, v) in config_obj {
                    merged_obj.insert(k.clone(), v.clone());
                }
            }
        } else if !config_value.is_null() {
            return Err("Gemini 配置格式错误: config 必须是对象或 null".to_string());
        }
        // config 为 null：保留现有 settings.json（与本机语义一致）
    }

    // security.auth.selectedType：只改该字段，保留现有 security 其他内容
    let selected = if auth_type == GeminiAuthType::GoogleOfficial {
        "oauth-personal"
    } else {
        "gemini-api-key"
    };
    let mut security = merged.get("security").cloned().unwrap_or_else(|| json!({}));
    let mut auth = security.get("auth").cloned().unwrap_or_else(|| json!({}));
    if let Some(auth_obj) = auth.as_object_mut() {
        auth_obj.insert(
            "selectedType".to_string(),
            Value::String(selected.to_string()),
        );
    }
    security["auth"] = auth;
    merged["security"] = security;

    let settings_text = serde_json::to_string_pretty(&merged)
        .map_err(|e| format!("序列化 settings.json 失败: {e}"))?;
    session
        .write_settings_with_backup(&settings_path, &settings_text, container, None)
        .await?;

    Ok(EffectReport {
        target: target_name.to_string(),
        provider_name: provider_name.to_string(),
        current_provider_id: None,
        conflicts_cleaned: 0,
        notes: vec![
            format!("已整文件覆盖远端 {env_path}"),
            format!("已整文件覆盖远端 {settings_path}"),
            "新建的 Gemini 会话立即生效".to_string(),
        ],
        warnings: Vec::new(),
    })
}
