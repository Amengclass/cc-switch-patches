//! 远程会话管理：浏览远端 `~/.claude/projects/*.jsonl`。
//!
//! 通过 `FileOps` 接口支持宿主机（SFTP）与容器内（docker exec）两种数据源。
//! 注意：容器 exec 模式下拿不到文件 metadata（size/mtime），仅提供名称与路径。

use serde::{Deserialize, Serialize};

use crate::fsops::FileOps;

/// 远端会话文件信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionInfo {
    pub path: String,
    pub name: String,
    pub size: Option<u64>,
    pub modified: Option<i64>,
}

/// 列出远端 `~/.claude/projects/` 下全部会话 jsonl 文件。
/// 目录不存在（远端尚未用过 Claude Code）时返回空列表。
pub async fn list_remote_sessions<F: FileOps>(
    fs: &F,
    home: &str,
) -> Result<Vec<RemoteSessionInfo>, String> {
    let projects = format!("{home}/.claude/projects");
    let mut sessions = Vec::new();

    let Ok(dir) = fs.read_dir(&projects).await else {
        return Ok(Vec::new());
    };

    for project in dir {
        if !project.is_dir {
            continue;
        }
        let Ok(sub) = fs.read_dir(&project.path).await else {
            continue;
        };
        for file in sub {
            if !file.name.ends_with(".jsonl") {
                continue;
            }
            sessions.push(RemoteSessionInfo {
                path: file.path,
                name: file.name,
                size: None,
                modified: None,
            });
        }
    }

    Ok(sessions)
}
