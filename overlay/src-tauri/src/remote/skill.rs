//! 远端 Skills SSOT 管理。
//!
//! 与本机架构完全对称：
//! - SSOT 物理目录：`~/.cc-switch/skills/{name}/`
//! - 元数据文件：`~/.cc-switch/skills.json`（JSON 数组，SFTP 读 / 原子写回）
//! - 同步：各应用 skills 目录下创建 symlink 指向 SSOT
//!
//! 通过 `FileOps` 接口支持三种数据源：本机（std::fs）/ 宿主机（SFTP）/ 容器内（docker exec）。

use std::collections::HashMap;
use std::path::Path;

use russh_sftp::client::SftpSession;
use serde::{Deserialize, Serialize};

use crate::fsops::FileOps;

// ========================================================================
// 路径
// ========================================================================

/// 远端 SSOT 技能目录（根据本机「技能存储位置」设置）。
pub fn remote_ssot_path(root: &str) -> String {
    match crate::settings::get_skill_storage_location() {
        crate::services::skill::SkillStorageLocation::Unified => {
            format!("{root}/.agents/skills")
        }
        _ => format!("{root}/.cc-switch/skills"),
    }
}

/// 远端 `~/.claude/skills` 目录路径。
pub fn remote_claude_skills_path(root: &str) -> String {
    format!("{root}/.claude/skills")
}

/// 远端 skills.json 路径（跟 SSOT 目录同级）。
pub(crate) fn remote_skills_json_path(root: &str) -> String {
    let ssot = remote_ssot_path(root);
    // ~/.cc-switch/skills.json 或 ~/.agents/skills.json
    if let Some(parent) = ssot.rsplit_once('/').map(|(p, _)| p) {
        format!("{parent}/skills.json")
    } else {
        format!("{root}/skills.json")
    }
}

// ========================================================================
// 数据结构
// ========================================================================

/// 远端技能应用开关。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSkillApps {
    #[serde(default)]
    pub claude: bool,
    #[serde(default)]
    pub codex: bool,
    #[serde(default)]
    pub gemini: bool,
    #[serde(default)]
    pub grokbuild: bool,
    #[serde(default)]
    pub opencode: bool,
    #[serde(default)]
    pub openclaw: bool,
    #[serde(default)]
    pub hermes: bool,
    #[serde(default)]
    pub pi: bool,
}

impl RemoteSkillApps {
    pub fn set_enabled(&mut self, app: &str, enabled: bool) {
        match app {
            "claude" => self.claude = enabled,
            "codex" => self.codex = enabled,
            "gemini" => self.gemini = enabled,
            "grokbuild" => self.grokbuild = enabled,
            "opencode" => self.opencode = enabled,
            "openclaw" => self.openclaw = enabled,
            "hermes" => self.hermes = enabled,
            "pi" => self.pi = enabled,
            _ => {}
        }
    }
}

/// skills.json 中的一条记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSkillRecord {
    /// 唯一标识（UUID，与 SQLite id 对齐）。
    #[serde(default)]
    pub id: String,
    /// 显示名（从 SKILL.md 解析，无则回退目录名）。
    pub name: String,
    pub description: Option<String>,
    /// 目录名。
    pub directory: String,
    /// 各应用启用状态。
    #[serde(default)]
    pub apps: RemoteSkillApps,
    #[serde(default)]
    pub installed_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub repo_owner: Option<String>,
    #[serde(default)]
    pub repo_name: Option<String>,
    #[serde(default)]
    pub repo_branch: Option<String>,
    /// README 或文档链接。
    #[serde(default)]
    pub readme_url: Option<String>,
    /// 目录内容哈希（用于更新检查）。
    #[serde(default)]
    pub content_hash: Option<String>,
}

/// 远端技能目录项（前端用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSkillEntry {
    pub id: String,
    /// 显示名称（来自 SKILL.md，无则回退到目录名）
    pub name: String,
    /// 技能目录名（文件系统用）
    pub directory: String,
    pub path: String,
    pub description: Option<String>,
    /// 各应用启用状态。
    pub apps: RemoteSkillApps,
    pub installed_at: i64,
    pub updated_at: i64,
    pub repo_owner: Option<String>,
    pub repo_name: Option<String>,
    pub repo_branch: Option<String>,
    pub readme_url: Option<String>,
    pub content_hash: Option<String>,
}

/// 远端未管理技能条目（扫描用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteUnmanagedSkill {
    pub directory: String,
    pub name: String,
    pub description: Option<String>,
    pub found_in: Vec<String>,
    pub path: String,
}

// ========================================================================
// SSOT 初始化
// ========================================================================

/// 确保远端 SSOT 目录和 skills.json 存在。
/// 首次进入远端 Skills 面板时调用，幂等。
pub async fn init_remote_skills_ssot<F: FileOps>(fs: &F, root: &str) -> Result<(), String> {
    let _ssot_dir = remote_ssot_path(root);
    // 通过 SFTP ensure_remote_dir 需要 SftpSession，这里用 FileOps 的 write_text_atomic
    // 触发父目录创建。先确保 SSOT 目录存在：创建 .cc-switch/ + skills/ 目录结构。
    init_ssot_dirs(fs, root).await?;

    let json_path = remote_skills_json_path(root);
    if !fs.exists(&json_path).await {
        fs.write_text_atomic(&json_path, "[]").await?;
    }
    Ok(())
}

/// 通过 SFTP 创建 SSOT 目录结构。
async fn init_ssot_dirs<F: FileOps>(fs: &F, root: &str) -> Result<(), String> {
    let ssot_dir = remote_ssot_path(root);
    let parent_dir = ssot_dir.rsplit_once('/').map(|(p, _)| p).unwrap_or(root);
    for dir in &[&parent_dir.to_string(), &ssot_dir] {
        if !fs.exists(dir).await {
            // 用 write_text_atomic 的副作用（mkdir -p）创建目录
            let marker = format!("{dir}/.ccswitch_placeholder");
            if !fs.exists(&marker).await {
                fs.write_text_atomic(&marker, "").await?;
                let _ = fs.remove_file(&marker).await;
            }
        }
    }
    Ok(())
}

// ========================================================================
// skills.json 读写
// ========================================================================

/// 读取远端 skills.json，返回记录列表。文件缺失或为空时返回空数组。
pub async fn read_remote_skills_json<F: FileOps>(
    fs: &F,
    root: &str,
) -> Result<Vec<RemoteSkillRecord>, String> {
    let path = remote_skills_json_path(root);
    let text = match fs.read_text_optional(&path).await? {
        Some(t) => t,
        None => return Ok(Vec::new()),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text).map_err(|e| format!("解析 skills.json 失败: {e}"))
}

/// 原子写入远端 skills.json。
pub async fn write_remote_skills_json<F: FileOps>(
    fs: &F,
    root: &str,
    records: &[RemoteSkillRecord],
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(records)
        .map_err(|e| format!("序列化 skills.json 失败: {e}"))?;
    let path = remote_skills_json_path(root);
    fs.write_text_atomic(&path, &json).await
}

// ========================================================================
// 同步管理（symlink 或 copy）
// ========================================================================

/// 已知应用的 skills 目录列表（相对于 home 的路径）。
const APP_SKILLS_DIRS: &[&str] = &[
    ".claude/skills",
    ".codex/skills",
    ".gemini/skills",
    ".grok/skills",
    ".config/opencode/skills",
    ".openclaw/workspace/skills",
    ".hermes/skills",
    ".pi/agent/skills",
];

/// 按 apps 开关过滤出要同步的应用目录。
fn enabled_app_dirs(apps: &RemoteSkillApps) -> Vec<&'static str> {
    let map: &[(&str, bool)] = &[
        (".claude/skills", apps.claude),
        (".codex/skills", apps.codex),
        (".gemini/skills", apps.gemini),
        (".grok/skills", apps.grokbuild),
        (".config/opencode/skills", apps.opencode),
        (".openclaw/workspace/skills", apps.openclaw),
        (".hermes/skills", apps.hermes),
        (".pi/agent/skills", apps.pi),
    ];
    map.iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(dir, _)| *dir)
        .collect()
}

/// 为技能在各应用目录创建链接（symlink 或 copy，取决于 `use_copy`）。
/// 通过 exec 通道在远端执行。
pub async fn sync_remote_skill_links(
    channel: &russh::client::Handle<crate::remote::connection::RemoteHandler>,
    container: Option<&str>,
    root: &str,
    name: &str,
    apps: &RemoteSkillApps,
    use_copy: bool,
) -> Result<(), String> {
    let ssot_dir = remote_ssot_path(root);
    let target = format!("{ssot_dir}/{name}");

    for app_rel in enabled_app_dirs(apps) {
        let link_path = format!("{root}/{app_rel}/{name}");
        let parent = format!("{root}/{app_rel}");

        // 确保父目录存在
        exec_cmd(
            channel,
            container,
            &format!("mkdir -p {}", shell_q(&parent)),
        )
        .await?;
        // 删除旧链接/目录
        exec_cmd(
            channel,
            container,
            &format!("rm -rf {}", shell_q(&link_path)),
        )
        .await?;

        let sync_cmd = if use_copy {
            format!("cp -r {} {}", shell_q(&target), shell_q(&link_path))
        } else {
            format!("ln -s {} {}", shell_q(&target), shell_q(&link_path))
        };
        exec_cmd(channel, container, &sync_cmd).await?;
    }
    Ok(())
}

/// 删除技能在所有应用目录下的链接/副本。
pub async fn remove_remote_skill_links(
    channel: &russh::client::Handle<crate::remote::connection::RemoteHandler>,
    container: Option<&str>,
    root: &str,
    name: &str,
) -> Result<(), String> {
    for app_rel in APP_SKILLS_DIRS {
        let link_path = format!("{root}/{app_rel}/{name}");
        exec_cmd(
            channel,
            container,
            &format!("rm -rf {}", shell_q(&link_path)),
        )
        .await?;
    }
    Ok(())
}

// ========================================================================
// 批量 toggle（纯函数 + 一次 exec）
// ========================================================================

/// app 字符串 → 该应用的 skills 目录相对路径。
pub(crate) fn app_skills_rel(app: &str) -> Option<&'static str> {
    match app {
        "claude" => Some(".claude/skills"),
        "codex" => Some(".codex/skills"),
        "gemini" => Some(".gemini/skills"),
        "grokbuild" => Some(".grok/skills"),
        "opencode" => Some(".config/opencode/skills"),
        "openclaw" => Some(".openclaw/workspace/skills"),
        "hermes" => Some(".hermes/skills"),
        "pi" => Some(".pi/agent/skills"),
        _ => None,
    }
}

/// 生成单个技能在某应用目录的链接/删除脚本（纯函数，便于单测）。
///
/// - 启用:`mkdir -p <app_dir> && rm -rf <link> && {ln -s|cp -r} <ssot/dir> <link>`
/// - 禁用:`rm -rf <link>`
pub(crate) fn build_skill_link_script(
    root: &str,
    ssot_path: &str,
    app_rel: &str,
    dir: &str,
    enabled: bool,
    use_copy: bool,
) -> String {
    let link_path = format!("{root}/{app_rel}/{dir}");
    if !enabled {
        return format!("rm -rf {}", shell_q(&link_path));
    }
    let target_path = format!("{ssot_path}/{dir}");
    let app_dir = format!("{root}/{app_rel}");
    let sync_op = if use_copy {
        format!("cp -r {} {}", shell_q(&target_path), shell_q(&link_path))
    } else {
        format!("ln -s {} {}", shell_q(&target_path), shell_q(&link_path))
    };
    format!(
        "mkdir -p {} && rm -rf {} && {}",
        shell_q(&app_dir),
        shell_q(&link_path),
        sync_op
    )
}

/// 对 skills.json 记录应用批量 toggle:逐 id 定位、改 apps、收集结果与链接脚本。
pub(crate) fn apply_skill_toggles(
    records: &mut [RemoteSkillRecord],
    ids: &[String],
    app: &str,
    enabled: bool,
    root: &str,
    ssot_path: &str,
    use_copy: bool,
) -> Result<(crate::remote::RemoteBulkToggleResult, Vec<String>), String> {
    use crate::remote::{RemoteBulkToggleFailure, RemoteBulkToggleResult};

    let Some(app_rel) = app_skills_rel(app) else {
        return Err(format!("未知应用: {app}"));
    };

    let mut succeeded: Vec<String> = Vec::new();
    let mut failed: Vec<RemoteBulkToggleFailure> = Vec::new();
    let mut scripts: Vec<String> = Vec::new();

    for id in ids {
        match records.iter_mut().find(|r| r.id == *id) {
            Some(rec) => {
                rec.apps.set_enabled(app, enabled);
                let dir = rec.directory.clone();
                scripts.push(build_skill_link_script(
                    root, ssot_path, app_rel, &dir, enabled, use_copy,
                ));
                succeeded.push(id.clone());
            }
            None => failed.push(RemoteBulkToggleFailure {
                item: id.clone(),
                error: format!("技能 {id} 不存在"),
            }),
        }
    }

    Ok((RemoteBulkToggleResult { succeeded, failed }, scripts))
}

/// 批量切换多个远端技能在某应用的启用状态。
///
/// 单次连接内完成:skills.json 读一次 → 内存改全部 → base64 一次 → 把每条
/// 链接脚本拼进同一个 `&&` 链,最终一次 `exec_command_with_stdin` 写盘 + 操作链接。
#[allow(clippy::too_many_arguments)]
pub async fn bulk_toggle_remote_skill_app<F: FileOps>(
    fs: &F,
    channel: &russh::client::Handle<crate::remote::connection::RemoteHandler>,
    container: Option<&str>,
    root: &str,
    ids: &[String],
    app: &str,
    enabled: bool,
    use_copy: bool,
) -> Result<crate::remote::RemoteBulkToggleResult, String> {
    let mut records = read_remote_skills_json(fs, root).await?;
    let ssot_path = remote_ssot_path(root);
    let (result, scripts) =
        apply_skill_toggles(&mut records, ids, app, enabled, root, &ssot_path, use_copy)?;

    if !result.succeeded.is_empty() {
        let json_path = remote_skills_json_path(root);
        let json_str = serde_json::to_string_pretty(&records)
            .map_err(|e| format!("序列化 skills.json 失败: {e}"))?;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(json_str.as_bytes());

        let combined = format!(
            "base64 -d > {} && {}",
            shell_q(&json_path),
            scripts.join(" && ")
        );
        let full_cmd = match container {
            Some(c) => format!("docker exec -i {} sh -c {}", c, shell_q(&combined)),
            None => combined,
        };
        crate::remote::connection::exec_command_with_stdin(channel, &full_cmd, b64.as_bytes())
            .await?;
    }

    Ok(result)
}

// ========================================================================
// 核心操作
// ========================================================================

/// 列出已安装技能（读 skills.json + SSOT 目录验证）。
pub async fn list_remote_skills<F: FileOps>(
    fs: &F,
    root: &str,
) -> Result<Vec<RemoteSkillEntry>, String> {
    init_remote_skills_ssot(fs, root).await?;
    let records = read_remote_skills_json(fs, root).await?;
    let ssot_dir = remote_ssot_path(root);

    let mut out = Vec::new();
    for rec in &records {
        let skill_dir = format!("{ssot_dir}/{}", rec.directory);
        // 验证 SSOT 目录存在
        if !fs.exists(&skill_dir).await {
            log::warn!("[remote-skill] SSOT 目录不存在: {skill_dir}");
            continue;
        }
        let (display_name, description) = read_skill_md_meta_static(fs, &skill_dir).await;
        log::info!(
            "[remote-skill] name={} display_name={:?} description={:?} json_desc={:?}",
            rec.directory,
            display_name,
            description,
            rec.description,
        );
        out.push(RemoteSkillEntry {
            id: if rec.id.is_empty() {
                rec.directory.clone()
            } else {
                rec.id.clone()
            },
            name: display_name
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| rec.directory.clone()),
            directory: rec.directory.clone(),
            description: description.or_else(|| rec.description.clone()),
            path: skill_dir,
            apps: rec.apps.clone(),
            installed_at: rec.installed_at,
            updated_at: rec.updated_at,
            repo_owner: rec.repo_owner.clone(),
            repo_name: rec.repo_name.clone(),
            repo_branch: rec.repo_branch.clone(),
            readme_url: rec.readme_url.clone(),
            content_hash: rec.content_hash.clone(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 删除技能：删 SSOT → 删 symlink → 更新 skills.json。
pub async fn delete_remote_skill<F: FileOps>(
    fs: &F,
    channel: Option<&russh::client::Handle<crate::remote::connection::RemoteHandler>>,
    container: Option<&str>,
    root: &str,
    name: &str,
) -> Result<bool, String> {
    // name 实际上是 id（UUID），从 JSON 中解析出真正的 directory
    let records = read_remote_skills_json(fs, root).await?;
    let resolved = records
        .iter()
        .find(|r| r.id == name)
        .map(|r| r.directory.clone())
        .ok_or_else(|| format!("技能 {name} 不存在"))?;

    if resolved.contains('/') || resolved.contains('\\') || resolved == "." || resolved == ".." {
        return Err("非法技能名称".to_string());
    }
    let ssot_dir = remote_ssot_path(root);
    let path = format!("{ssot_dir}/{resolved}");

    // 删 SSOT 目录
    if fs.exists(&path).await && fs.is_dir(&path).await {
        fs.remove_dir_all(&path).await?;
    }

    // 删链接
    if let Some(ch) = channel {
        remove_remote_skill_links(ch, container, root, &resolved).await?;
    }

    // 更新 JSON（同时按 id 和 directory 剔除）
    let mut records = records;
    records.retain(|r| r.id != name);
    write_remote_skills_json(fs, root, &records).await?;

    Ok(true)
}

// ========================================================================
// 导入已有（远端本地复制）
// ========================================================================

/// 在远端文件系统上扫描未管理的技能目录。
///
/// 遍历远端已知应用 skills 目录，逻辑与本机 scan_unmanaged 一致：
/// 每个目录 → 列子目录 → 跳过无 SKILL.md → 跳过 . 开头 → 跳过已管理 → 去重合并。
pub async fn scan_remote_unmanaged_skills<F: FileOps>(
    fs: &F,
    root: &str,
) -> Result<Vec<RemoteUnmanagedSkill>, String> {
    // 已管理技能：从 skills.json 获取
    let records = read_remote_skills_json(fs, root).await?;
    let managed: std::collections::HashSet<String> =
        records.into_iter().map(|r| r.directory).collect();

    // 扫描来源：各应用的 skills 目录
    let ssot_dir = remote_ssot_path(root);
    let mut sources: Vec<(String, String)> = Vec::new();
    sources.push((ssot_dir.clone(), "cc-switch".to_string()));
    sources.push((remote_claude_skills_path(root), "claude".to_string()));
    sources.push((format!("{root}/.codex/skills"), "codex".to_string()));
    sources.push((format!("{root}/.gemini/skills"), "gemini".to_string()));
    sources.push((format!("{root}/.grok/skills"), "grokbuild".to_string()));
    sources.push((
        format!("{root}/.config/opencode/skills"),
        "opencode".to_string(),
    ));
    sources.push((
        format!("{root}/.openclaw/workspace/skills"),
        "openclaw".to_string(),
    ));
    sources.push((format!("{root}/.hermes/skills"), "hermes".to_string()));

    let mut unmanaged: HashMap<String, RemoteUnmanagedSkill> = HashMap::new();

    for (src_dir, label) in &sources {
        if !fs.exists(src_dir).await {
            continue;
        }
        let entries = match fs.read_dir(src_dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries {
            if !entry.is_dir || entry.name.starts_with('.') {
                continue;
            }
            if managed.contains(&entry.name) {
                continue;
            }
            let skill_md = format!("{}/SKILL.md", entry.path);
            if !fs.exists(&skill_md).await {
                continue;
            }
            let (display_name, description) = read_skill_md_meta_static(fs, &entry.path).await;
            unmanaged
                .entry(entry.name.clone())
                .and_modify(|s| s.found_in.push(label.clone()))
                .or_insert(RemoteUnmanagedSkill {
                    directory: entry.name.clone(),
                    name: display_name.unwrap_or_else(|| entry.name.clone()),
                    description,
                    found_in: vec![label.clone()],
                    path: entry.path,
                });
        }
    }

    let mut out: Vec<RemoteUnmanagedSkill> = unmanaged.into_values().collect();
    out.sort_by(|a, b| a.directory.cmp(&b.directory));
    Ok(out)
}

/// 在远端将技能目录复制到 SSOT → 更新 skills.json → 创建 symlink。
#[allow(clippy::too_many_arguments)]
pub async fn import_remote_skill_local<F: FileOps>(
    fs: &F,
    channel: &russh::client::Handle<crate::remote::connection::RemoteHandler>,
    container: Option<&str>,
    root: &str,
    source_path: &str,
    name: &str,
    apps: &RemoteSkillApps,
    use_copy: bool,
) -> Result<RemoteSkillRecord, String> {
    let ssot_dir = remote_ssot_path(root);
    let dest = format!("{ssot_dir}/{name}");

    // cp -r 源到 SSOT
    if let Some(c) = container {
        // 容器：源在容器内部，docker exec cp -r 在容器内复制
        let cp_cmd = format!(
            "docker exec {} cp -r {} {}",
            c,
            shell_q(source_path),
            shell_q(&dest)
        );
        crate::remote::connection::exec_command(channel, &cp_cmd).await?;
    } else {
        // 宿主机：exec cp -r
        let cp_cmd = format!("cp -r {} {}", shell_q(source_path), shell_q(&dest));
        crate::remote::connection::exec_command(channel, &cp_cmd).await?;
    }

    // 读取 SKILL.md 获取元数据
    let (display_name, description) = read_skill_md_meta_static(fs, &dest).await;

    let now = chrono::Utc::now().timestamp_millis();
    let record = RemoteSkillRecord {
        id: uuid::Uuid::new_v4().to_string(),
        name: display_name.unwrap_or_else(|| name.to_string()),
        description,
        directory: name.to_string(),
        apps: apps.clone(),
        installed_at: now,
        updated_at: now,
        repo_owner: None,
        repo_name: None,
        repo_branch: None,
        readme_url: None,
        content_hash: None,
    };

    // 更新 skills.json
    let mut records = read_remote_skills_json(fs, root).await?;
    records.retain(|r| r.directory != name);
    records.push(record.clone());
    write_remote_skills_json(fs, root, &records).await?;

    // 创建链接（symlink 或 copy，取决于设置）
    sync_remote_skill_links(channel, container, root, name, apps, use_copy).await?;

    Ok(record)
}

// ========================================================================
// ZIP 安装
// ========================================================================

/// 通过 tar.gz 流将本地目录一次性上传到远端（宿主机或容器）。
///
/// 本地内存 tar.gz 打包 → `exec_command_with_stdin` 传二进制流 → 远端 `tar xzf -` 解包。
/// 一次 SSH 往返完成，不落宿主机磁盘。
pub(crate) async fn upload_dir_via_tar(
    channel: &russh::client::Handle<crate::remote::connection::RemoteHandler>,
    container: Option<&str>,
    local_dir: &Path,
    remote_dir: &str,
) -> Result<(), String> {
    // 1. 内存中打包 tar.gz
    let mut buf = Vec::new();
    {
        let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
        let mut archive = tar::Builder::new(gz);
        let dir_name = local_dir
            .file_name()
            .ok_or_else(|| "local_dir 无文件名".to_string())?;
        archive
            .append_dir_all(dir_name, local_dir)
            .map_err(|e| format!("tar 打包失败: {e}"))?;
        let gz = archive
            .into_inner()
            .map_err(|e| format!("tar 完成失败: {e}"))?;
        gz.finish().map_err(|e| format!("gzip 完成失败: {e}"))?;
    }

    // 2. 解包目标父目录（remote_dir 的父目录，tar 内已包含 dir_name）
    let tar_parent = match remote_dir.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => parent,
        _ => "/",
    };

    // 3. 构建命令并执行
    let cmd = if let Some(c) = container {
        format!("docker exec -i {} tar xzf - -C {}", c, shell_q(tar_parent))
    } else {
        format!("tar xzf - -C {}", shell_q(tar_parent))
    };

    crate::remote::connection::exec_command_with_stdin(channel, &cmd, &buf).await?;
    Ok(())
}

/// 构造一条远端技能记录（纯内存，无 I/O）。
///
/// zip 安装与「发现技能」远端安装共用，保证两端记录结构一致。
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_remote_skill_record(
    name: &str,
    display_name: Option<String>,
    description: Option<String>,
    repo_owner: Option<String>,
    repo_name: Option<String>,
    repo_branch: Option<String>,
    readme_url: Option<String>,
    apps: RemoteSkillApps,
) -> RemoteSkillRecord {
    let now = chrono::Utc::now().timestamp_millis();
    RemoteSkillRecord {
        id: uuid::Uuid::new_v4().to_string(),
        name: display_name.unwrap_or_else(|| name.to_string()),
        description,
        directory: name.to_string(),
        apps,
        installed_at: now,
        updated_at: now,
        repo_owner,
        repo_name,
        repo_branch,
        readme_url,
        content_hash: None,
    }
}

pub async fn install_remote_skills_from_zip_generic(
    sftp: &SftpSession,
    channel: &russh::client::Handle<crate::remote::connection::RemoteHandler>,
    container: Option<&str>,
    root: &str,
    zip_path: &str,
) -> Result<Vec<String>, String> {
    use crate::services::skill::SkillService;

    let zip_path = Path::new(zip_path);
    let temp_guard =
        SkillService::extract_local_zip(zip_path).map_err(|e| format!("解压 ZIP 失败: {e}"))?;
    let temp_dir = temp_guard.path().to_path_buf();

    let skill_dirs = SkillService::scan_skills_in_dir(&temp_dir)
        .map_err(|e| format!("扫描 ZIP 内技能失败: {e}"))?;
    if skill_dirs.is_empty() {
        return Err("ZIP 内未找到含 SKILL.md 的技能目录".to_string());
    }

    let zip_stem = zip_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());

    let ssot_dir = remote_ssot_path(root);
    let mut installed: Vec<String> = Vec::new();
    let target = crate::remote::docker::RemoteTarget::new(sftp, channel, container)?;
    let records = read_remote_skills_json(&target, root).await?;
    let existing: std::collections::HashSet<String> =
        records.iter().map(|r| r.directory.clone()).collect();

    for skill_dir in &skill_dirs {
        let dir_name = skill_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let install_name =
            if skill_dir == &temp_dir || dir_name.is_empty() || dir_name.starts_with('.') {
                zip_stem
                    .as_deref()
                    .map(sanitize_name)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "skill".to_string())
            } else {
                sanitize_name(&dir_name)
            };
        if install_name.is_empty() {
            continue;
        }
        if existing.contains(&install_name) {
            log::warn!("[remote] 远端已存在技能目录 {install_name}，跳过");
            continue;
        }

        let remote_dir = format!("{ssot_dir}/{install_name}");
        upload_dir_via_tar(channel, container, skill_dir, &remote_dir).await?;
        installed.push(install_name);
    }

    if installed.is_empty() {
        return Err("没有可安装的新技能（可能远端已全部存在）".to_string());
    }
    Ok(installed)
}

pub(crate) fn sanitize_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed == "."
        || trimmed == ".."
    {
        return String::new();
    }
    trimmed.to_string()
}

// ========================================================================
// 内部辅助
// ========================================================================

pub(crate) async fn read_skill_md_meta_static<F: FileOps>(
    fs: &F,
    skill_dir: &str,
) -> (Option<String>, Option<String>) {
    let md_path = format!("{skill_dir}/SKILL.md");
    let content = match fs.read_text_optional(&md_path).await {
        Ok(Some(text)) => text,
        Ok(None) => {
            log::warn!("[remote-skill] SKILL.md 不存在: {md_path}");
            return (None, None);
        }
        Err(e) => {
            log::warn!("[remote-skill] 读取 SKILL.md 失败: {md_path}: {e}");
            return (None, None);
        }
    };
    let content = content.trim_start_matches('\u{feff}');
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 {
        log::warn!(
            "[remote-skill] SKILL.md 无 YAML frontmatter: {md_path} (parts={})",
            parts.len()
        );
        return (None, None);
    }
    let front_matter = parts[1].trim();
    let meta: serde_json::Value = match serde_yaml::from_str(front_matter) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[remote-skill] SKILL.md YAML 解析失败: {md_path}: {e}");
            return (None, None);
        }
    };
    let name = meta
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let description = meta
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    log::info!("[remote-skill] parsed {md_path}: name={name:?} desc={description:?}");
    (name, description)
}

fn shell_q(s: &str) -> String {
    if s.contains('\'') {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{s}'")
    }
}

async fn exec_cmd(
    channel: &russh::client::Handle<crate::remote::connection::RemoteHandler>,
    container: Option<&str>,
    cmd: &str,
) -> Result<String, String> {
    let full = match container {
        Some(c) => format!("docker exec {} sh -c {}", c, shell_q(cmd)),
        None => cmd.to_string(),
    };
    crate::remote::connection::exec_command(channel, &full).await
}

// ========================================================================
// 更新远端 Skill（本机下载仓库 → 替换远端 SSOT）
// ========================================================================

/// 更新一台远端目标上的某个 Skill：从它的 GitHub 仓库重新下载最新版，
/// 替换远端 SSOT 目录，并同步各已启用 app 的链接。
///
/// 对齐本机 `SkillService::update_skill` 语义。方案：**本机下载 + 本机算 hash**，
/// 传远端替换（不需要远端访问 GitHub）。删除旧 SSOT 目录用 FileOps 的
/// `remove_dir_all`（SFTP/docker exec 均已实现）。
pub async fn update_remote_skill_impl<F: FileOps>(
    fs: &F,
    channel: &russh::client::Handle<crate::remote::connection::RemoteHandler>,
    container: Option<&str>,
    root: &str,
    skill_id: &str,
) -> Result<RemoteSkillRecord, String> {
    use crate::services::skill::{DiscoverableSkill, SkillService};

    // 1. 取该 Skill 的记录（含 repo 来源）。
    let records = read_remote_skills_json(fs, root).await?;
    let record = records
        .iter()
        .find(|r| r.id == skill_id || r.directory.eq_ignore_ascii_case(skill_id))
        .cloned()
        .ok_or_else(|| format!("远端找不到该 Skill: {skill_id}"))?;

    // 2. 必须有仓库来源才能从仓库更新（对齐本机 update_skill 的判定）。
    let (owner, name, branch) = match (&record.repo_owner, &record.repo_name) {
        (Some(o), Some(n)) => (
            o.clone(),
            n.clone(),
            record
                .repo_branch
                .clone()
                .unwrap_or_else(|| "main".to_string()),
        ),
        _ => {
            return Err(format!(
                "无法更新「{}」：没有仓库来源（不是从仓库安装的）",
                record.name
            ))
        }
    };

    // 3. 从 record 构造 DiscoverableSkill，触发本机下载 + 解析源目录
    //    （复用 install_from_discoverable 同款入口，含 60s 超时 / 路径安全校验）。
    let discoverable = DiscoverableSkill {
        key: format!("{owner}/{name}:{}", record.directory),
        name: record.name.clone(),
        description: record.description.clone().unwrap_or_default(),
        directory: record.directory.clone(),
        readme_url: record.readme_url.clone(),
        repo_owner: owner.clone(),
        repo_name: name.clone(),
        repo_branch: branch.clone(),
    };
    let service = SkillService::new();
    // 保持 temp_guard 存活直到上传完成，防止下载目录被提前回收。
    let (_temp_guard, _canonical_temp, source_dir, used_branch) = service
        .download_and_resolve_skill_source(&discoverable)
        .await
        .map_err(|e| e.to_string())?;

    // 4. 本机算新内容 hash（更新后远端应有的指纹，供 check_updates 对比）。
    let new_hash = SkillService::compute_dir_hash(&source_dir).ok();

    // 5. 先读更新后 SKILL.md 的元数据（覆盖 name/description）。
    let (md_name, md_description) =
        read_skill_md_meta_static(fs, &source_dir.to_string_lossy()).await;
    let new_name = md_name.unwrap_or(record.name.clone());
    let new_description = md_description.or(record.description.clone());

    // 6. 替换远端 SSOT 目录：删旧 + 上传新。
    let ssot_dir = remote_ssot_path(root);
    let remote_dir = format!("{ssot_dir}/{}", record.directory);
    if let Err(e) = fs.remove_dir_all(&remote_dir).await {
        // 目录不存在也算成功（上传会重建）。
        log::warn!("[remote] 更新 Skill 删除旧目录失败（可能不存在）: {e}");
    }
    upload_dir_via_tar(channel, container, &source_dir, &remote_dir).await?;

    // 7. 写回更新后的记录。
    let mut updated = record.clone();
    updated.name = new_name;
    updated.description = new_description;
    updated.repo_branch = Some(used_branch);
    updated.content_hash = new_hash;
    updated.updated_at = chrono::Utc::now().timestamp_millis();

    let mut new_records = read_remote_skills_json(fs, root).await?;
    if let Some(position) = new_records.iter().position(|r| r.id == updated.id) {
        new_records[position] = updated.clone();
    } else {
        new_records.push(updated.clone());
    }
    write_remote_skills_json(fs, root, &new_records).await?;

    // 8. 同步各已启用 app 的链接（symlink 或 copy，取决于设置）。
    let use_copy =
        crate::settings::get_skill_sync_method() == crate::services::skill::SyncMethod::Copy;
    sync_remote_skill_links(
        channel,
        container,
        root,
        &updated.directory,
        &updated.apps,
        use_copy,
    )
    .await?;

    Ok(updated)
}

/// 检查远端某个目标上各 Skill 是否有更新（对齐本机 `SkillService::check_updates`）。
///
/// 读远端 skills.json 记录 → 对每个有仓库来源的 Skill，本机下载其仓库并解析源目录 →
/// 对比「仓库最新 hash」与「远端记录存的 content_hash」→ 不同记为可更新。
/// 返回与本机一致的 `SkillUpdateInfo` 列表。方案：本机下载 + 本机算 hash。
pub async fn check_remote_skill_updates_impl<F: FileOps>(
    fs: &F,
    root: &str,
) -> Result<Vec<crate::services::skill::SkillUpdateInfo>, String> {
    use crate::services::skill::{DiscoverableSkill, SkillService};

    let records = read_remote_skills_json(fs, root).await?;
    if records.is_empty() {
        return Ok(Vec::new());
    }

    let service = SkillService::new();
    let mut updates: Vec<crate::services::skill::SkillUpdateInfo> = Vec::new();

    for record in &records {
        // 必须有仓库来源；没有来源的 Skill 无法从仓库检查更新（对齐本机）。
        let (owner, name, branch) = match (&record.repo_owner, &record.repo_name) {
            (Some(o), Some(n)) => (
                o.clone(),
                n.clone(),
                record
                    .repo_branch
                    .clone()
                    .unwrap_or_else(|| "main".to_string()),
            ),
            _ => continue,
        };
        let discoverable = DiscoverableSkill {
            key: format!("{owner}/{name}:{}", record.directory),
            name: record.name.clone(),
            description: record.description.clone().unwrap_or_default(),
            directory: record.directory.clone(),
            readme_url: record.readme_url.clone(),
            repo_owner: owner,
            repo_name: name,
            repo_branch: branch,
        };
        let (_temp_guard, _canonical, source_dir, _used_branch) = match tokio::time::timeout(
            std::time::Duration::from_secs(60),
            service.download_and_resolve_skill_source(&discoverable),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                log::warn!(
                    "检查远端更新时下载 {}/{} 失败: {e}",
                    record.name,
                    record.directory
                );
                continue;
            }
            Err(_) => {
                log::warn!("检查远端更新时下载 {} 超时", record.name);
                continue;
            }
        };
        let remote_hash = match SkillService::compute_dir_hash(&source_dir) {
            Ok(h) => h,
            Err(_) => continue,
        };
        // 远端记录存的 hash 与仓库最新不同 → 有更新。
        if record.content_hash.as_deref() != Some(&remote_hash) {
            updates.push(crate::services::skill::SkillUpdateInfo {
                id: record.id.clone(),
                name: record.name.clone(),
                current_hash: record.content_hash.clone(),
                remote_hash,
            });
        }
    }

    Ok(updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/home/u";
    const SSOT: &str = "/home/u/.cc-switch/skills";

    fn record(id: &str, directory: &str) -> RemoteSkillRecord {
        RemoteSkillRecord {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            directory: directory.to_string(),
            apps: RemoteSkillApps::default(),
            installed_at: 0,
            updated_at: 0,
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: None,
            content_hash: None,
        }
    }

    #[test]
    fn set_enabled_toggles_known_apps_only() {
        let mut apps = RemoteSkillApps::default();
        apps.set_enabled("claude", true);
        apps.set_enabled("grokbuild", true);
        apps.set_enabled("unknown", true); // 未知应用 no-op
        assert!(apps.claude);
        assert!(apps.grokbuild);
        assert!(!apps.codex);
        apps.set_enabled("claude", false);
        assert!(!apps.claude);
    }

    #[test]
    fn app_skills_rel_maps_known_apps() {
        assert_eq!(app_skills_rel("claude"), Some(".claude/skills"));
        assert_eq!(
            app_skills_rel("openclaw"),
            Some(".openclaw/workspace/skills")
        );
        assert_eq!(app_skills_rel("hermes"), Some(".hermes/skills"));
        assert_eq!(app_skills_rel("pi"), Some(".pi/agent/skills"));
        assert_eq!(app_skills_rel("nope"), None);
    }

    #[test]
    fn build_link_script_enable_symlink() {
        let script = build_skill_link_script(ROOT, SSOT, ".claude/skills", "foo", true, false);
        assert_eq!(
            script,
            "mkdir -p '/home/u/.claude/skills' && rm -rf '/home/u/.claude/skills/foo' && ln -s '/home/u/.cc-switch/skills/foo' '/home/u/.claude/skills/foo'"
        );
    }

    #[test]
    fn build_link_script_enable_copy() {
        let script = build_skill_link_script(ROOT, SSOT, ".codex/skills", "foo", true, true);
        assert_eq!(
            script,
            "mkdir -p '/home/u/.codex/skills' && rm -rf '/home/u/.codex/skills/foo' && cp -r '/home/u/.cc-switch/skills/foo' '/home/u/.codex/skills/foo'"
        );
    }

    #[test]
    fn build_link_script_disable() {
        let script = build_skill_link_script(ROOT, SSOT, ".hermes/skills", "foo", false, false);
        assert_eq!(script, "rm -rf '/home/u/.hermes/skills/foo'");
    }

    #[test]
    fn apply_skill_toggles_collects_success_and_failure() {
        let mut records = vec![record("id-a", "dir-a"), record("id-b", "dir-b")];
        let ids = vec!["id-a".to_string(), "missing".to_string()];
        let (result, scripts) =
            apply_skill_toggles(&mut records, &ids, "claude", true, ROOT, SSOT, false)
                .expect("apply");

        assert_eq!(result.succeeded, vec!["id-a"]);
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].item, "missing");
        assert!(result.failed[0].error.contains("不存在"));
        assert_eq!(scripts.len(), 1);
        assert!(scripts[0].contains("dir-a"));

        assert!(records[0].apps.claude);
        assert!(!records[1].apps.claude); // id-b 未在本次 ids 中
    }

    #[test]
    fn apply_skill_toggles_unknown_app_fails() {
        let mut records = vec![record("id-a", "dir-a")];
        let ids = vec!["id-a".to_string()];
        let err = apply_skill_toggles(&mut records, &ids, "nope", true, ROOT, SSOT, false)
            .expect_err("unknown app should error");
        assert!(err.contains("未知应用"));
    }
}
