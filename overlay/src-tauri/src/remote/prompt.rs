//! 远端 Prompts 管理。
//!
//! 与本地完全对称：`~/.cc-switch/prompts.json`（claude）或
//! `~/.cc-switch/prompts-{app}.json`（其余 app）存多提示词列表，
//! 启用那条写入各 app 的 live 提示词文件。通过 `FileOps` 支持宿主机（SFTP）与容器（docker exec）。
//!
//! live 文件路径与本机 `prompt_files::prompt_file_path` 矩阵一致：
//! claude `~/.claude/CLAUDE.md`、codex `~/.codex/AGENTS.md`、
//! gemini `~/.gemini/GEMINI.md`、grok `~/.grok/AGENTS.md`、
//! opencode `~/.config/opencode/AGENTS.md`、openclaw `~/.openclaw/AGENTS.md`、
//! hermes `~/.hermes/SOUL.md`（远端路径忽略本机 override 设置，按 home 直拼）。

use crate::fsops::FileOps;
use crate::prompt::Prompt;
use crate::services::pi_prompt_files::{PiPromptFileKind, PiPromptFileSnapshot, PiPromptTemplate};
use sha2::{Digest, Sha256};

/// 各 app 的远端 live 提示词文件路径。
pub fn remote_prompt_path(root: &str, app: &str) -> String {
    let (dir, file) = match app {
        "codex" => (".codex", "AGENTS.md"),
        "gemini" => (".gemini", "GEMINI.md"),
        "grokbuild" => (".grok", "AGENTS.md"),
        "opencode" => (".config/opencode", "AGENTS.md"),
        "openclaw" => (".openclaw", "AGENTS.md"),
        "hermes" => (".hermes", "SOUL.md"),
        "pi" => (".pi/agent", "AGENTS.md"),
        _ => (".claude", "CLAUDE.md"),
    };
    format!("{root}/{dir}/{file}")
}

/// 远端 prompts.json 路径（claude 保持 `prompts.json` 兼容老数据，其余 per-app）。
pub fn remote_prompts_json_path(root: &str, app: &str) -> String {
    if app == "claude" {
        format!("{root}/.cc-switch/prompts.json")
    } else {
        format!("{root}/.cc-switch/prompts-{app}.json")
    }
}

/// 读取远端 live 提示词文件内容；文件缺失时返回空字符串。
pub async fn read_remote_prompt<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
) -> Result<String, String> {
    let path = remote_prompt_path(root, app);
    Ok(fs.read_text_optional(&path).await?.unwrap_or_default())
}

/// 将内容原子写回远端 live 提示词文件。
pub async fn write_remote_prompt<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
    content: &str,
) -> Result<(), String> {
    fs.write_text_atomic(&remote_prompt_path(root, app), content)
        .await
}

/// 读远端 prompts.json 列表。
pub async fn read_remote_prompts<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
) -> Result<Vec<Prompt>, String> {
    let path = remote_prompts_json_path(root, app);
    match fs.read_text_optional(&path).await? {
        Some(text) if !text.trim().is_empty() => {
            serde_json::from_str(&text).map_err(|e| format!("解析 {path} 失败: {e}"))
        }
        _ => Ok(Vec::new()),
    }
}

/// 原子写远端 prompts.json + 同步启用提示词到 live 提示词文件。
pub async fn write_remote_prompts<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
    prompts: &[Prompt],
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(prompts)
        .map_err(|e| format!("序列化 prompts.json 失败: {e}"))?;
    let path = remote_prompts_json_path(root, app);
    fs.write_text_atomic(&path, &json).await?;

    // 启用的提示词写入 live 提示词文件
    let live_content = prompts
        .iter()
        .find(|p| p.enabled)
        .map(|p| p.content.as_str())
        .unwrap_or("");
    write_remote_prompt(fs, root, app, live_content).await
}

// ========================================================================
// Pi 原生指令文件 + 模板（system / templates tab）
// ========================================================================

fn pi_agent_dir(root: &str) -> String {
    format!("{root}/.pi/agent")
}

fn pi_revision(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

const MISSING_REVISION: &str = "missing";

/// 读远端 Pi 系统指令文件（SYSTEM.md / APPEND_SYSTEM.md）。
pub async fn read_remote_pi_file<F: FileOps>(
    fs: &F,
    root: &str,
    kind: PiPromptFileKind,
) -> Result<PiPromptFileSnapshot, String> {
    let dir = pi_agent_dir(root);
    let filename = match kind {
        PiPromptFileKind::SystemOverride => "SYSTEM.md",
        PiPromptFileKind::SystemAppend => "APPEND_SYSTEM.md",
    };
    let path = format!("{dir}/{filename}");
    match fs.read_text_optional(&path).await? {
        Some(content) => {
            let revision = pi_revision(&content);
            Ok(PiPromptFileSnapshot {
                exists: true,
                revision,
                content,
            })
        }
        None => Ok(PiPromptFileSnapshot {
            exists: false,
            revision: MISSING_REVISION.to_string(),
            content: String::new(),
        }),
    }
}

/// 写远端 Pi 系统指令文件（带 revision 冲突检测）。
pub async fn write_remote_pi_file<F: FileOps>(
    fs: &F,
    root: &str,
    kind: PiPromptFileKind,
    expected_revision: &str,
    content: &str,
) -> Result<PiPromptFileSnapshot, String> {
    let dir = pi_agent_dir(root);
    let filename = match kind {
        PiPromptFileKind::SystemOverride => "SYSTEM.md",
        PiPromptFileKind::SystemAppend => "APPEND_SYSTEM.md",
    };
    let path = format!("{dir}/{filename}");

    // 冲突检测：读当前文件 revision
    let current = match fs.read_text_optional(&path).await? {
        Some(c) => pi_revision(&c),
        None => MISSING_REVISION.to_string(),
    };
    if current != expected_revision {
        return Err("Pi 指令文件已被外部修改，请刷新后重试".to_string());
    }

    // 确保目录存在
    if !fs.is_dir(&dir).await {
        // 创建目录
        fs.write_text_atomic(&format!("{dir}/.keep"), "").await?;
    }

    fs.write_text_atomic(&path, content).await?;
    read_remote_pi_file(fs, root, kind).await
}

/// 删除远端 Pi 系统指令文件（带 revision 冲突检测）。
pub async fn delete_remote_pi_file<F: FileOps>(
    fs: &F,
    root: &str,
    kind: PiPromptFileKind,
    expected_revision: &str,
) -> Result<bool, String> {
    let dir = pi_agent_dir(root);
    let filename = match kind {
        PiPromptFileKind::SystemOverride => "SYSTEM.md",
        PiPromptFileKind::SystemAppend => "APPEND_SYSTEM.md",
    };
    let path = format!("{dir}/{filename}");

    // 冲突检测
    let current = match fs.read_text_optional(&path).await? {
        Some(c) => pi_revision(&c),
        None => MISSING_REVISION.to_string(),
    };
    if current != expected_revision {
        return Err("Pi 指令文件已被外部修改，请刷新后重试".to_string());
    }

    if !fs.exists(&path).await {
        return Ok(false);
    }
    fs.remove_file(&path).await?;
    Ok(true)
}

/// 列出远端 Pi 模板（~/.pi/agent/prompts/*.md）。
pub async fn list_remote_pi_templates<F: FileOps>(
    fs: &F,
    root: &str,
) -> Result<Vec<PiPromptTemplate>, String> {
    let dir = format!("{}/prompts", pi_agent_dir(root));
    if !fs.is_dir(&dir).await {
        return Ok(Vec::new());
    }

    let entries = fs.read_dir(&dir).await?;
    let mut templates = Vec::new();

    for entry in entries {
        if entry.is_dir {
            continue;
        }
        let name = &entry.name;
        if !name.ends_with(".md") {
            continue;
        }
        let slug = name.trim_end_matches(".md");
        if slug.is_empty() || slug.starts_with('.') {
            continue;
        }

        let path = format!("{dir}/{name}");
        if let Some(content) = fs.read_text_optional(&path).await? {
            let revision = pi_revision(&content);
            templates.push(PiPromptTemplate {
                slug: slug.to_string(),
                content,
                revision,
            });
        }
    }
    templates.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(templates)
}

/// 创建/更新远端 Pi 模板。
pub async fn upsert_remote_pi_template<F: FileOps>(
    fs: &F,
    root: &str,
    slug: &str,
    original_slug: Option<&str>,
    expected_revision: &str,
    content: &str,
) -> Result<PiPromptTemplate, String> {
    let dir = format!("{}/prompts", pi_agent_dir(root));
    let path = format!("{dir}/{slug}.md");

    if let Some(original) = original_slug.filter(|s| *s != slug) {
        // 重命名：检查原文件 revision + 新文件不存在
        let original_path = format!("{dir}/{original}.md");
        let current = match fs.read_text_optional(&original_path).await? {
            Some(c) => pi_revision(&c),
            None => MISSING_REVISION.to_string(),
        };
        if current != expected_revision {
            return Err("模板已被外部修改，请刷新后重试".to_string());
        }
        if fs.exists(&path).await {
            return Err(format!("目标 slug /{slug} 已存在"));
        }
        // 删除原文件 + 写新文件
        fs.remove_file(&original_path).await?;
        fs.write_text_atomic(&path, content).await?;
    } else {
        // 新建/更新：检查 revision
        let current = match fs.read_text_optional(&path).await? {
            Some(c) => pi_revision(&c),
            None => MISSING_REVISION.to_string(),
        };
        if current != expected_revision {
            return Err("模板已被外部修改，请刷新后重试".to_string());
        }
        fs.write_text_atomic(&path, content).await?;
    }

    Ok(PiPromptTemplate {
        slug: slug.to_string(),
        content: content.to_string(),
        revision: pi_revision(content),
    })
}

/// 删除远端 Pi 模板。
pub async fn delete_remote_pi_template<F: FileOps>(
    fs: &F,
    root: &str,
    slug: &str,
    expected_revision: &str,
) -> Result<bool, String> {
    let dir = format!("{}/prompts", pi_agent_dir(root));
    let path = format!("{dir}/{slug}.md");

    let current = match fs.read_text_optional(&path).await? {
        Some(c) => pi_revision(&c),
        None => MISSING_REVISION.to_string(),
    };
    if current != expected_revision {
        return Err("模板已被外部修改，请刷新后重试".to_string());
    }

    if !fs.exists(&path).await {
        return Ok(false);
    }
    fs.remove_file(&path).await?;
    Ok(true)
}
