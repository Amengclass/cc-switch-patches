//! 远端 MCode（MiniMax Code）供应商读写。
//!
//! 与本机 `mcode_config` 语义对齐，远端只做 I/O：
//! - 配置文件是 YAML：`~/.minimax/config.yaml`
//! - 供应商都在顶层 `custom_provider:` 映射下，每项形如
//!   `{ kind: custom, enabled, name, api, options: { baseURL, apiKey }, models: {...} }`
//! - `kind` 缺失视为 `custom`（官方 `get_providers` 的判据），非 custom 的项不动
//!
//! 远端路径固定 `{root}/.minimax`，不读本机 `MINIMAX_DATA_DIR` / `MAVIS_DATA_DIR`
//! 之类的环境变量（与 hermes 等其它远端 app 的处理一致：远端按 home 直拼）。

use indexmap::IndexMap;
use serde_json::Value;

use super::connection::RemoteSession;
use super::effect::EffectReport;

/// 远端 MCode 配置目录（相对 home）。
pub const MCODE_REMOTE_DIR: &str = ".minimax";

/// 远端 config.yaml 绝对路径。
pub fn remote_mcode_config_path(root: &str) -> String {
    format!("{root}/{MCODE_REMOTE_DIR}/config.yaml")
}

/// 读远端 config.yaml 并解析成 YAML 根；文件不存在 / 空文件视为空映射。
async fn read_root_yaml(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
) -> Result<serde_yaml::Value, String> {
    let path = remote_mcode_config_path(root);
    let Some(text) = session.read_remote_text(&path, container).await? else {
        return Ok(serde_yaml::Value::Mapping(Default::default()));
    };
    if text.trim().is_empty() {
        return Ok(serde_yaml::Value::Mapping(Default::default()));
    }
    let value: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("解析远端 MCode config.yaml 失败: {e}"))?;
    if value.is_null() {
        return Ok(serde_yaml::Value::Mapping(Default::default()));
    }
    if !value.is_mapping() {
        return Err("远端 MCode config.yaml 根不是映射".to_string());
    }
    Ok(value)
}

/// 读远端 MCode 的 `custom_provider` 映射（对齐本机 `mcode_config::get_providers`）。
///
/// 只保留 `kind` 缺失或 `kind == "custom"` 的项 —— 官方认为非 custom 的是
/// 内置/托管供应商，不归 CC Switch 管。
pub async fn read_remote_mcode_providers(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
) -> Result<IndexMap<String, Value>, String> {
    let document = read_root_yaml(session, container, root).await?;
    let Some(custom) = document.get("custom_provider") else {
        return Ok(IndexMap::new());
    };
    if custom.is_null() {
        return Ok(IndexMap::new());
    }
    let json = crate::hermes_config::yaml_to_json(custom)
        .map_err(|e| format!("转换远端 custom_provider 失败: {e}"))?;
    let mut providers: IndexMap<String, Value> = serde_json::from_value(json)
        .map_err(|_| "远端 MCode custom_provider 结构不合法".to_string())?;
    providers.retain(|_, provider| {
        provider
            .get("kind")
            .and_then(Value::as_str)
            .is_none_or(|kind| kind == "custom")
    });
    Ok(providers)
}

/// 对远端执行 MCode 供应商切换（累加模式：不覆盖别的供应商，只 upsert 目标项）。
///
/// MCode 是 additive 模式且**没有「当前供应商」概念**（官方 `is_additive_mode`
/// 含 Mcode），所以这里只把供应商写进 `custom_provider`，不动别的键。
pub async fn apply_mcode_provider_settings(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
    target_name: &str,
    provider_name: &str,
    settings: &Value,
    provider_id: &str,
) -> Result<EffectReport, String> {
    let config_path = remote_mcode_config_path(root);
    let mut root_yaml = read_root_yaml(session, container, root).await?;

    // settings 归一化：确保 kind / enabled / name 齐备，且只保留字符串键
    let mut normalized = settings.clone();
    if let Some(obj) = normalized.as_object_mut() {
        obj.insert("kind".to_string(), Value::String("custom".to_string()));
        obj.entry("enabled".to_string())
            .or_insert(Value::Bool(true));
        obj.insert("name".to_string(), Value::String(provider_name.to_string()));
    }

    let yaml_val = crate::hermes_config::json_to_yaml(&normalized)
        .map_err(|e| format!("序列化 MCode provider 失败: {e}"))?;
    let serde_yaml::Value::Mapping(mut new_map) = yaml_val else {
        return Err("MCode provider 配置必须是映射".to_string());
    };
    // 官方写入时把 id 也放进配置（前端据此显示归属）
    new_map.insert(
        serde_yaml::Value::String("name".to_string()),
        serde_yaml::Value::String(provider_name.to_string()),
    );

    let root_map = root_yaml
        .as_mapping_mut()
        .ok_or_else(|| "远端 MCode 配置根不是映射".to_string())?;

    // 取现有 custom_provider 映射（缺失则新建）
    let mut custom_map: serde_yaml::Mapping = match root_map
        .get(&serde_yaml::Value::String("custom_provider".to_string()))
    {
        Some(serde_yaml::Value::Mapping(m)) => m.clone(),
        _ => serde_yaml::Mapping::new(),
    };

    // upsert：命中已有条目时合并远端额外字段（forward-compat），否则新增
    let key = serde_yaml::Value::String(provider_id.to_string());
    match custom_map.get(&key) {
        Some(serde_yaml::Value::Mapping(existing)) => {
            let existing = existing.clone();
            for (k, v) in existing {
                new_map.entry(k).or_insert(v);
            }
        }
        _ => {}
    }
    custom_map.insert(key, serde_yaml::Value::Mapping(new_map));

    root_map.insert(
        serde_yaml::Value::String("custom_provider".to_string()),
        serde_yaml::Value::Mapping(custom_map),
    );

    let text =
        serde_yaml::to_string(&root_yaml).map_err(|e| format!("序列化 config.yaml 失败: {e}"))?;
    session
        .write_settings_with_backup(&config_path, &text, container, None)
        .await?;

    Ok(EffectReport {
        target: target_name.to_string(),
        provider_name: provider_name.to_string(),
        current_provider_id: None,
        conflicts_cleaned: 0,
        notes: vec![
            format!("已更新远端 {config_path} 的 custom_provider.{provider_id}"),
            "MCode 为累加模式：新供应商已加入，可在 MCode 内选择使用".to_string(),
        ],
        warnings: Vec::new(),
    })
}
