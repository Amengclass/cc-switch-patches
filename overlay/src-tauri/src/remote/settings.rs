//! 远端 Claude Code `~/.claude/settings.json` 的读写与供应商切换。
//!
//! 所有函数通过 `FileOps` 接口访问数据源（本机 std::fs / 宿主机 SFTP / 容器内 docker exec），
//! 同一套业务逻辑在三种目标上直接复用。

use serde_json::{Map, Value};

use crate::fsops::FileOps;

use super::effect::EffectReport;

/// 远端 settings.json 路径（`root` 为家目录，如 `/root` 或 `/home/xxx`）。
pub fn remote_settings_path(root: &str) -> String {
    format!("{root}/.claude/settings.json")
}

/// 读取远端 settings.json;文件缺失时返回空对象。
pub async fn read_remote_settings<F: FileOps>(fs: &F, root: &str) -> Result<Value, String> {
    match fs.read_text_optional(&remote_settings_path(root)).await? {
        Some(text) => {
            serde_json::from_str(&text).map_err(|e| format!("远端 settings.json 解析失败: {e}"))
        }
        None => Ok(Value::Object(Map::new())),
    }
}

/// 将完整 settings 对象应用到远端 settings.json（**整文件覆盖**，与 cc switch
/// 本机切换行为一致），并保留 .bak 备份 + 原子写回。
///
/// settings 应通过 `build_effective_settings_with_common_config` 构建，
/// 保证与本地切换产出完全一致（provider env + 通用配置片段）。
///
/// **落盘方式**：一次 exec + stdin 管道（宿主机 `sh -c` / 容器 `docker exec sh -c`，
/// 共用同一 POSIX 脚本），把「存在才备份 .bak + mkdir + base64 原子写 + 失败清理」
/// 合并为单次往返——宿主机由原来 15~20 个 SFTP RTT 降到 ~1 RTT。
///
/// 返回「生效方式」报告，前端据此明确提示用户如何生效。
pub async fn apply_provider_settings(
    session: &crate::remote::connection::RemoteSession,
    container: Option<&str>,
    root: &str,
    target_name: &str,
    provider_name: &str,
    settings: &Value,
) -> Result<EffectReport, String> {
    let path = remote_settings_path(root);
    let text = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("序列化 settings.json 失败: {e}"))?;

    // 一次 exec：目标存在才备份（[ -f ] && cp），原子替换失败自动清理 tmp。
    // claude 保持历史行为：不校验 hash（expected_hash=None）
    session
        .write_settings_with_backup(&path, &text, container, None)
        .await?;

    Ok(EffectReport {
        target: target_name.to_string(),
        provider_name: provider_name.to_string(),
        current_provider_id: None,
        conflicts_cleaned: 0,
        notes: vec![
            format!("已整文件覆盖远端 {path}"),
            "新建的 Claude Code 会话立即生效".to_string(),
            "已在运行的会话按热重载生效；若远端 shell 存在冲突环境变量，建议清理后重新登录终端"
                .to_string(),
        ],
        warnings: Vec::new(),
    })
}
