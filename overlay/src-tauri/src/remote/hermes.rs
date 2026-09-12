//! 远端 Hermes 供应商切换（复用本机 `hermes_config::set_provider` 语义，
//! 远端只做 I/O）。
//!
//! 与本机一致：`custom_providers` 序列按 name upsert ——
//! ① `sanitize_hermes_provider_keys`（camelCase → snake）
//! ② `normalize_provider_models_for_write`（models 数组 → dict）
//! ③ 注入 `name` 字段 + 第一个 model id 作为 `model` 字段
//! ④ 命中已有条目时合并远端额外字段（forward-compat），否则 push。
//! 远端文件为 YAML（`~/.hermes/config.yaml`），整体读-改-写。

use serde_json::Value;

use super::connection::RemoteSession;
use super::effect::EffectReport;

/// 读远端 Hermes 的 `model` 段（对齐本机 `hermes_config::get_model_config`）。
/// 远端「设为默认 / 切换」写的是远端 config.yaml 的 `model.provider`，前端按钮态
/// 必须读同一文件才能正确高亮「当前激活」；本机命令读本机文件，远端目标下会读到
/// 错误数据。
pub async fn read_remote_hermes_model_config(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
) -> Result<Option<crate::hermes_config::HermesModelConfig>, String> {
    let config_path = format!("{root}/.hermes/config.yaml");
    let Some(text) = session.read_remote_text(&config_path, container).await? else {
        return Ok(None);
    };
    let yaml = serde_yaml::from_str::<serde_yaml::Value>(&text)
        .map_err(|e| format!("解析远端 config.yaml 失败: {e}"))?;
    let Some(model_value) = yaml.get("model") else {
        return Ok(None);
    };
    let json_val = crate::hermes_config::yaml_to_json(model_value)
        .map_err(|e| format!("转换远端 model 段失败: {e}"))?;
    serde_json::from_value(json_val)
        .map(Some)
        .map_err(|e| format!("解析远端 model 配置失败: {e}"))
}

/// 远端版 `hermes_config::apply_switch_defaults`：在已读出的远端 config.yaml 的
/// root 上更新顶层 `model:` 段（对齐本机合并规则，见 hermes_config.rs:914-947）：
/// - `model.provider` 恒更新为新 provider id；
/// - `model.default` 仅当新 provider 声明了模型才覆盖（否则保留旧的，保证可运行）；
/// - `context_length` / `max_tokens` / `base_url` 等现有字段保留。
///
/// 直接改 `root_yaml`（调用方随后整体序列化写回），不额外落盘。
fn apply_remote_switch_defaults(
    root_yaml: &mut serde_yaml::Value,
    provider_id: &str,
    normalized: &serde_json::Value,
) -> Result<(), String> {
    // 新 provider 声明的第一个模型 id（对齐本机：settings 里 models 数组首项）
    let first_model_id = normalized
        .get("models")
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.keys().next())
        .cloned()
        .filter(|s| !s.trim().is_empty());

    let root_map = root_yaml
        .as_mapping_mut()
        .ok_or_else(|| "远端 Hermes 配置根不是映射".to_string())?;

    // 现有 model 段（保留 context_length / max_tokens / base_url 等字段）
    let mut model_map: serde_yaml::Mapping = match root_map
        .get(&serde_yaml::Value::String("model".to_string()))
    {
        Some(serde_yaml::Value::Mapping(m)) => m.clone(),
        _ => serde_yaml::Mapping::new(),
    };

    model_map.insert(
        serde_yaml::Value::String("provider".to_string()),
        serde_yaml::Value::String(provider_id.to_string()),
    );
    if let Some(id) = first_model_id {
        model_map.insert(
            serde_yaml::Value::String("default".to_string()),
            serde_yaml::Value::String(id),
        );
    }

    root_map.insert(
        serde_yaml::Value::String("model".to_string()),
        serde_yaml::Value::Mapping(model_map),
    );
    Ok(())
}

/// 对远端执行 Hermes 供应商切换。
///
/// - `settings`：DB 中该 provider 的 `settings_config`
/// - `provider_id`：作为 `custom_providers[].name`
pub async fn apply_hermes_provider_settings(
    session: &RemoteSession,
    container: Option<&str>,
    root: &str,
    target_name: &str,
    provider_name: &str,
    settings: &Value,
    provider_id: &str,
) -> Result<EffectReport, String> {
    let config_path = format!("{root}/.hermes/config.yaml");

    // 读远端 config.yaml（不存在 / 解析失败视为空配置）
    let mut root_yaml: serde_yaml::Value =
        match session.read_remote_text(&config_path, container).await? {
            Some(t) => serde_yaml::from_str(&t).unwrap_or_else(|_| serde_yaml::Value::Null),
            None => serde_yaml::Value::Null,
        };
    if !root_yaml.is_mapping() {
        root_yaml = serde_yaml::Value::Mapping(Default::default());
    }

    // 复刻本机 set_provider 的变换链
    let mut normalized = settings.clone();
    crate::hermes_config::sanitize_hermes_provider_keys(&mut normalized);
    crate::hermes_config::normalize_provider_models_for_write(&mut normalized);

    let first_model_id = normalized
        .get("models")
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.keys().next())
        .cloned();

    let mut yaml_val = crate::hermes_config::json_to_yaml(&normalized)
        .map_err(|e| format!("序列化 Hermes provider 失败: {e}"))?;
    if let serde_yaml::Value::Mapping(ref mut m) = yaml_val {
        m.insert(
            serde_yaml::Value::String("name".to_string()),
            serde_yaml::Value::String(provider_id.to_string()),
        );
        if let Some(model_id) = first_model_id {
            m.insert(
                serde_yaml::Value::String("model".to_string()),
                serde_yaml::Value::String(model_id),
            );
        } else {
            m.remove(serde_yaml::Value::String("model".to_string()));
        }
    }

    // custom_providers 序列 upsert（含 forward-compat merge，照本机 838-857）
    let mut providers: Vec<serde_yaml::Value> = root_yaml
        .get("custom_providers")
        .and_then(|v| v.as_sequence())
        .cloned()
        .unwrap_or_default();
    if let Some(existing) = providers
        .iter_mut()
        .find(|p| p.get("name").and_then(|n| n.as_str()) == Some(provider_id))
    {
        if let (Some(existing_map), serde_yaml::Value::Mapping(new_map)) =
            (existing.as_mapping(), &mut yaml_val)
        {
            for (k, v) in existing_map {
                new_map.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        *existing = yaml_val;
    } else {
        providers.push(yaml_val);
    }

    if let Some(root_map) = root_yaml.as_mapping_mut() {
        root_map.insert(
            serde_yaml::Value::String("custom_providers".to_string()),
            serde_yaml::Value::Sequence(providers),
        );
    }

    // 对齐本机 apply_switch_defaults（provider/mod.rs:5230-5242）：hermes 是 additive，
    // 切换不覆盖 live 文件，而是更新顶层 `model:` 段指向该 provider 的第一个模型。
    // 否则只是 shuffle custom_providers[]，hermes 运行时仍用旧的 model.provider——
    // 这就是远端「启用/设为默认后不生效」的根因。
    apply_remote_switch_defaults(&mut root_yaml, provider_id, &normalized)?;

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
            format!("已更新远端 {config_path} 的 custom_providers.{provider_id}"),
            "新建的 Hermes 会话立即生效".to_string(),
        ],
        warnings: Vec::new(),
    })
}
