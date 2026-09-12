//! 远端 shell 配置中与供应商切换冲突的 `ANTHROPIC_*` 环境变量扫描与清理。
//!
//! 扫描逻辑对齐本地 `services/env_checker.rs`;清理采用「注释 + .bak 备份」，
//! 避免破坏用户手写配置。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::fsops::FileOps;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEnvConflict {
    pub var_name: String,
    pub var_value: String,
    pub source_type: String, // 恒为 "file"
    pub source_path: String, // 形如 "/home/user/.bashrc:12"
}

/// 与供应商切换直接冲突、常被误设进 shell profile 的变量。
const CONFLICT_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_CUSTOM_HEADERS",
    "ANTHROPIC_PROXY_URL",
];

fn is_conflict_var(name: &str) -> bool {
    let upper = name.to_uppercase();
    CONFLICT_VARS.iter().any(|k| upper == *k)
}

/// 需要扫描的远端 shell 配置文件列表。
pub fn remote_shell_configs(home: &str) -> Vec<String> {
    vec![
        format!("{home}/.bashrc"),
        format!("{home}/.bash_profile"),
        format!("{home}/.zshrc"),
        format!("{home}/.zprofile"),
        format!("{home}/.profile"),
        "/etc/profile".to_string(),
        "/etc/bashrc".to_string(),
        "/etc/environment".to_string(),
    ]
}

/// 扫描远端 shell 配置,返回与目标供应商冲突的环境变量。
pub async fn scan_remote_env_conflicts<F: FileOps>(
    fs: &F,
    home: &str,
) -> Result<Vec<RemoteEnvConflict>, String> {
    let mut conflicts = Vec::new();
    for path in remote_shell_configs(home) {
        let Some(content) = fs.read_text_optional(&path).await? else {
            continue;
        };
        for (idx, raw_line) in content.lines().enumerate() {
            let trimmed = raw_line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let export_line = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            if let Some(eq) = export_line.find('=') {
                let name = export_line[..eq].trim();
                if is_conflict_var(name) {
                    conflicts.push(RemoteEnvConflict {
                        var_name: name.to_string(),
                        var_value: export_line[eq + 1..]
                            .trim()
                            .trim_matches('"')
                            .trim_matches('\'')
                            .to_string(),
                        source_type: "file".to_string(),
                        source_path: format!("{path}:{}", idx + 1),
                    });
                }
            }
        }
    }
    Ok(conflicts)
}

/// 清理远端 shell 配置中的冲突变量行：注释掉（保留原值可追溯），带 .bak 备份。
/// 返回实际清理的行数。
pub async fn clean_remote_env_conflicts<F: FileOps>(
    fs: &F,
    conflicts: &[RemoteEnvConflict],
) -> Result<usize, String> {
    // 按文件分组，行号从 1 开始
    let mut by_file: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for c in conflicts {
        if c.source_type != "file" {
            continue;
        }
        if let Some((file, line)) = c.source_path.rsplit_once(':') {
            if let Ok(ln) = line.parse::<usize>() {
                by_file.entry(file.to_string()).or_default().push(ln);
            }
        }
    }

    let mut cleaned = 0usize;
    for (file, mut lines) in by_file {
        // 备份原文件
        if let Some(original) = fs.read_text_optional(&file).await? {
            fs.write_text_atomic(&format!("{file}.bak"), &original)
                .await?;
        }

        let content = match fs.read_text_optional(&file).await? {
            Some(c) => c,
            None => continue,
        };
        lines.sort_unstable();
        let mut out = String::new();
        for (idx, line) in content.lines().enumerate() {
            if lines.contains(&(idx + 1)) {
                out.push_str("# ccswitch-removed: ");
                out.push_str(line);
                out.push('\n');
                cleaned += 1;
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        fs.write_text_atomic(&file, &out).await?;
    }

    Ok(cleaned)
}
