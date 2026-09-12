//! 远端 MCP 服务器管理（SSOT + 多 app live 同步，对齐本机）。
//!
//! 与本机架构对称：
//! - SSOT 元数据文件：`~/.cc-switch/mcp.json`（完整 McpServer，含 apps 多开关 + 元数据）
//! - live 同步：apps 启用的 app，把 server spec 写入该 CLI 的远端 live 配置
//! - 通过 `FileOps` 接口同时支持宿主机（SFTP）与容器内（docker exec）
//!
//! 各 CLI live 配置（格式转换复用本机 `mcp/*.rs` 的纯函数，文件读写走 FileOps）：
//! - Claude:    `~/.claude.json` 的 `mcpServers`（JSON）
//! - Codex:     `~/.codex/config.toml` 的 `[mcp_servers]`（TOML）
//! - Gemini:    `~/.gemini/settings.json` 的 `mcpServers`（JSON）
//! - GrokBuild: `~/.grok/config.toml` 的 `[mcp_servers]`（TOML，复用 Codex 转换）
//! - OpenCode:  `~/.config/opencode/opencode.json` 的 `mcp`（JSON + 格式转换）
//! - Hermes:    `~/.hermes/config.yaml` 的 `mcp_servers`（YAML + 格式转换）

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::fsops::FileOps;

// ========================================================================
// 数据结构（镜像本机 McpServer / skills RemoteSkillRecord）
// ========================================================================

/// 远端 MCP 应用开关。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMcpApps {
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
}

impl RemoteMcpApps {
    /// 所有启用的 app 标识。
    pub fn enabled_apps(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.claude {
            out.push("claude");
        }
        if self.codex {
            out.push("codex");
        }
        if self.gemini {
            out.push("gemini");
        }
        if self.grokbuild {
            out.push("grokbuild");
        }
        if self.opencode {
            out.push("opencode");
        }
        if self.openclaw {
            out.push("openclaw");
        }
        if self.hermes {
            out.push("hermes");
        }
        out
    }

    pub fn set_enabled(&mut self, app: &str, enabled: bool) {
        match app {
            "claude" => self.claude = enabled,
            "codex" => self.codex = enabled,
            "gemini" => self.gemini = enabled,
            "grokbuild" => self.grokbuild = enabled,
            "opencode" => self.opencode = enabled,
            "openclaw" => self.openclaw = enabled,
            "hermes" => self.hermes = enabled,
            _ => {}
        }
    }
}

/// 远端 MCP 服务器条目（SSOT 记录，镜像本机 McpServer）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMcpServer {
    pub id: String,
    pub name: String,
    pub server: Value,
    #[serde(default)]
    pub apps: RemoteMcpApps,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

// ========================================================================
// 路径
// ========================================================================

/// 远端 SSOT 元数据文件（与 skills.json 同级，位于 ~/.cc-switch/ 下）。
pub fn remote_mcp_ssot_path(root: &str) -> String {
    format!("{root}/.cc-switch/mcp.json")
}

/// 远端 `~/.claude.json` 路径。
pub fn remote_claude_json_path(root: &str) -> String {
    format!("{root}/.claude.json")
}

fn codex_config_path(root: &str) -> String {
    format!("{root}/.codex/config.toml")
}
fn gemini_settings_path(root: &str) -> String {
    format!("{root}/.gemini/settings.json")
}
fn grok_config_path(root: &str) -> String {
    format!("{root}/.grok/config.toml")
}
fn opencode_config_path(root: &str) -> String {
    format!("{root}/.config/opencode/opencode.json")
}
fn hermes_config_path(root: &str) -> String {
    format!("{root}/.hermes/config.yaml")
}
fn openclaw_config_path(root: &str) -> String {
    format!("{root}/.openclaw/openclaw.json")
}
fn pi_config_path(root: &str) -> String {
    format!("{root}/.pi/agent/settings.json")
}

/// 该 app 的配置目录（判断 CLI 是否安装，与 live 配置文件不一定同名）。
fn app_dir(root: &str, app: &str) -> Option<String> {
    match app {
        "claude" => Some(format!("{root}/.claude")),
        "codex" => Some(format!("{root}/.codex")),
        "gemini" => Some(format!("{root}/.gemini")),
        "grokbuild" => Some(format!("{root}/.grok")),
        "opencode" => Some(format!("{root}/.config/opencode")),
        "hermes" => Some(format!("{root}/.hermes")),
        "openclaw" => Some(format!("{root}/.openclaw")),
        "pi" => Some(format!("{root}/.pi")),
        _ => None,
    }
}

// ========================================================================
// SSOT 读写
// ========================================================================

/// 读取远端 `~/.claude.json` 的完整内容（供前端展示/编辑原始 JSON）。
pub async fn read_remote_mcp_json<F: FileOps>(fs: &F, root: &str) -> Result<Value, String> {
    let path = remote_claude_json_path(root);
    match fs.read_text_optional(&path).await? {
        Some(text) => serde_json::from_str(&text)
            .map_err(|e| format!("远端 ~/.claude.json 解析失败 {path}: {e}")),
        None => Ok(json!({})),
    }
}

/// 读取远端 SSOT mcp.json，返回 id -> McpServer 映射。文件缺失时返回空映射。
pub async fn read_remote_mcp_ssot<F: FileOps>(
    fs: &F,
    root: &str,
) -> Result<IndexMap<String, RemoteMcpServer>, String> {
    let path = remote_mcp_ssot_path(root);
    match fs.read_text_optional(&path).await? {
        Some(text) if !text.trim().is_empty() => {
            serde_json::from_str(&text).map_err(|e| format!("解析远端 mcp.json 失败 {path}: {e}"))
        }
        _ => Ok(IndexMap::new()),
    }
}

/// 原子写入远端 SSOT mcp.json。
pub async fn write_remote_mcp_ssot<F: FileOps>(
    fs: &F,
    root: &str,
    map: &IndexMap<String, RemoteMcpServer>,
) -> Result<(), String> {
    let text =
        serde_json::to_string_pretty(map).map_err(|e| format!("序列化远端 mcp.json 失败: {e}"))?;
    fs.write_text_atomic(&remote_mcp_ssot_path(root), &text)
        .await
}

// ========================================================================
// 核心操作：list / upsert / delete / toggle
// ========================================================================

/// 列出远端所有 MCP 服务器（完整 McpServer，含 apps 与元数据）。
pub async fn list_remote_mcp_servers<F: FileOps>(
    fs: &F,
    root: &str,
) -> Result<Vec<RemoteMcpServer>, String> {
    let map = read_remote_mcp_ssot(fs, root).await?;
    Ok(map.into_values().collect())
}

/// 新增/更新一个 MCP 服务器：写 SSOT + 同步 apps 启用的 live 配置。
pub async fn upsert_remote_mcp_server<F: FileOps>(
    fs: &F,
    root: &str,
    server: &RemoteMcpServer,
) -> Result<(), String> {
    let mut map = read_remote_mcp_ssot(fs, root).await?;
    // 旧 apps：编辑时若取消了某个 app，需要从对应 live 移除
    let prev_apps = map
        .get(&server.id)
        .map(|s| s.apps.enabled_apps())
        .unwrap_or_default();

    map.insert(server.id.clone(), server.clone());
    write_remote_mcp_ssot(fs, root, &map).await?;

    for app in &prev_apps {
        if !server.apps.enabled_apps().contains(app) {
            remove_mcp_from_app(fs, root, app, &server.id).await?;
        }
    }
    for app in server.apps.enabled_apps() {
        sync_mcp_to_app(fs, root, app, &server.id, &server.server).await?;
    }
    Ok(())
}

/// 删除一个 MCP 服务器：删 SSOT + 从所有启用的 live 配置移除。
pub async fn delete_remote_mcp_server<F: FileOps>(
    fs: &F,
    root: &str,
    id: &str,
) -> Result<bool, String> {
    let mut map = read_remote_mcp_ssot(fs, root).await?;
    let Some(server) = map.shift_remove(id) else {
        return Ok(false);
    };
    write_remote_mcp_ssot(fs, root, &map).await?;
    for app in server.apps.enabled_apps() {
        remove_mcp_from_app(fs, root, app, id).await?;
    }
    Ok(true)
}

/// 切换一个 MCP 服务器在指定 app 的启用状态：改 SSOT + 同步/移除该 app 的 live。
pub async fn toggle_remote_mcp_app<F: FileOps>(
    fs: &F,
    root: &str,
    id: &str,
    app: &str,
    enabled: bool,
) -> Result<(), String> {
    let ids = [id.to_string()];
    let result = bulk_toggle_remote_mcp_app(fs, root, &ids, app, enabled).await?;
    match result.failed.into_iter().next() {
        Some(failure) => Err(failure.error),
        None => Ok(()),
    }
}

/// 批量切换多个 MCP 服务器在指定 app 的启用状态。
///
/// 单次连接内完成:SSOT 读一次、改全部、写一次;同一 app 的 live 文件也
/// 只读一次、写一次(由 `sync_mcp_to_app_many` / `remove_mcp_from_app_many`
/// 保证)。逐条失败(如 id 不存在)聚合进 `failed`,不中断其余条目。
pub async fn bulk_toggle_remote_mcp_app<F: FileOps>(
    fs: &F,
    root: &str,
    ids: &[String],
    app: &str,
    enabled: bool,
) -> Result<crate::remote::RemoteBulkToggleResult, String> {
    use crate::remote::{RemoteBulkToggleFailure, RemoteBulkToggleResult};

    let mut map = read_remote_mcp_ssot(fs, root).await?;
    let mut succeeded: Vec<String> = Vec::new();
    let mut failed: Vec<RemoteBulkToggleFailure> = Vec::new();
    let mut enabled_specs: Vec<(String, Value)> = Vec::new();

    for id in ids {
        match map.get_mut(id) {
            Some(server) => {
                server.apps.set_enabled(app, enabled);
                succeeded.push(id.clone());
                if enabled {
                    enabled_specs.push((id.clone(), server.server.clone()));
                }
            }
            None => failed.push(RemoteBulkToggleFailure {
                item: id.clone(),
                error: format!("MCP 服务器 {id} 不存在"),
            }),
        }
    }

    if !succeeded.is_empty() {
        write_remote_mcp_ssot(fs, root, &map).await?;
        if enabled {
            sync_mcp_to_app_many(fs, root, app, &enabled_specs).await?;
        } else {
            remove_mcp_from_app_many(fs, root, app, &succeeded).await?;
        }
    }

    Ok(RemoteBulkToggleResult { succeeded, failed })
}

/// 切换供应商后重投影该 app 已启用的远端 MCP（对齐本机
/// `McpService::sync_enabled_for_app`：切换整文件覆盖了 live，必须把 SSOT 里
/// 已启用的 MCP 补回，否则 codex/gemini/grok 等「MCP 与 live 同文件」的
/// app 切换后 MCP 会失效）。
///
/// 读远端 SSOT `~/.cc-switch/mcp.json` → 筛出 `app` 启用的服务器 →
/// `sync_mcp_to_app_many` 投影回 live（一次 exec）。无启用项或 SSOT 缺失时
/// no-op；失败由调用方降级为警告（投影自愈：下次切换 / 任一 MCP 启停会重投影）。
pub async fn reproject_remote_mcp_for_app<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
) -> Result<(), String> {
    let map = read_remote_mcp_ssot(fs, root).await?;
    let enabled_specs: Vec<(String, Value)> = map
        .iter()
        .filter(|(_, server)| server.apps.enabled_apps().contains(&app))
        .map(|(id, server)| (id.clone(), server.server.clone()))
        .collect();
    sync_mcp_to_app_many(fs, root, app, &enabled_specs).await
}

// ========================================================================
// 从远端各 CLI live 配置导入到 SSOT（对齐本机 importFromApps）
// ========================================================================

pub async fn import_remote_mcp_from_apps<F: FileOps>(fs: &F, root: &str) -> Result<usize, String> {
    let mut map = read_remote_mcp_ssot(fs, root).await?;
    let mut new_count = 0;

    for app in [
        "claude",
        "codex",
        "gemini",
        "grokbuild",
        "opencode",
        "openclaw",
        "hermes",
        "pi",
    ] {
        let servers = read_live_servers(fs, root, app).await?;
        for (id, spec) in servers {
            if id.trim().is_empty() || !spec.is_object() {
                continue;
            }
            let to_save = if let Some(existing) = map.get(&id) {
                // 已存在：仅启用该 app，不覆盖其他字段
                let mut merged = existing.clone();
                merged.apps.set_enabled(app, true);
                merged
            } else {
                // 真正的新服务器
                new_count += 1;
                let mut apps = RemoteMcpApps::default();
                apps.set_enabled(app, true);
                RemoteMcpServer {
                    id: id.clone(),
                    name: id.clone(),
                    server: spec,
                    apps,
                    description: None,
                    homepage: None,
                    docs: None,
                    tags: Vec::new(),
                }
            };
            map.insert(id, to_save);
        }
    }

    if !map.is_empty() {
        write_remote_mcp_ssot(fs, root, &map).await?;
    }
    Ok(new_count)
}

// ========================================================================
// live 同步（薄层：读远端文本 -> 调本机纯转换函数 -> 写回）
// ========================================================================

/// 同步一个 MCP 服务器到指定 app 的远端 live 配置。
async fn sync_mcp_to_app<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
    id: &str,
    spec: &Value,
) -> Result<(), String> {
    let items = [(id.to_string(), spec.clone())];
    sync_mcp_to_app_many(fs, root, app, &items).await
}

/// 同步多个 MCP 服务器到指定 app 的远端 live 配置。
///
/// 同一 app 只碰一个 live 文件:整文件读一次、内存改全部、写一次。
async fn sync_mcp_to_app_many<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
    items: &[(String, Value)],
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    if !app_installed(fs, root, app).await {
        // CLI 未安装：跳过，不创建配置文件（与本机 should_sync_* 语义一致）
        return Ok(());
    }
    match app {
        "claude" => json_upsert_many(fs, &remote_claude_json_path(root), "mcpServers", items).await,
        "codex" => toml_upsert_many(fs, &codex_config_path(root), items).await,
        "gemini" => json_upsert_many(fs, &gemini_settings_path(root), "mcpServers", items).await,
        "grokbuild" => toml_upsert_many(fs, &grok_config_path(root), items).await,
        "opencode" => {
            let mut converted = Vec::with_capacity(items.len());
            for (id, spec) in items {
                let opencode_spec =
                    crate::mcp::convert_to_opencode_format(spec).map_err(|e| e.to_string())?;
                converted.push((id.clone(), opencode_spec));
            }
            json_upsert_many(fs, &opencode_config_path(root), "mcp", &converted).await
        }
        "hermes" => hermes_upsert_many(fs, &hermes_config_path(root), items).await,
        "openclaw" => {
            let mut converted = Vec::with_capacity(items.len());
            for (id, spec) in items {
                let openclaw_spec =
                    crate::mcp::convert_to_openclaw_format(spec).map_err(|e| e.to_string())?;
                converted.push((id.clone(), openclaw_spec));
            }
            openclaw_json_upsert_many(fs, &openclaw_config_path(root), &converted).await
        }
        "pi" => json_upsert_many(fs, &pi_config_path(root), "mcpServers", items).await,
        _ => Ok(()),
    }
}

/// 从指定 app 的远端 live 配置移除一个 MCP 服务器。
async fn remove_mcp_from_app<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
    id: &str,
) -> Result<(), String> {
    let ids = [id.to_string()];
    remove_mcp_from_app_many(fs, root, app, &ids).await
}

/// 从指定 app 的远端 live 配置移除多个 MCP 服务器。
async fn remove_mcp_from_app_many<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
    ids: &[String],
) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    if !app_installed(fs, root, app).await {
        return Ok(());
    }
    match app {
        "claude" => json_remove_many(fs, &remote_claude_json_path(root), "mcpServers", ids).await,
        "codex" => toml_remove_many(fs, &codex_config_path(root), ids).await,
        "gemini" => json_remove_many(fs, &gemini_settings_path(root), "mcpServers", ids).await,
        "grokbuild" => toml_remove_many(fs, &grok_config_path(root), ids).await,
        "opencode" => json_remove_many(fs, &opencode_config_path(root), "mcp", ids).await,
        "hermes" => hermes_remove_many(fs, &hermes_config_path(root), ids).await,
        "openclaw" => openclaw_json_remove_many(fs, &openclaw_config_path(root), ids).await,
        "pi" => json_remove_many(fs, &pi_config_path(root), "mcpServers", ids).await,
        _ => Ok(()),
    }
}

/// 该 app 是否已安装（配置目录存在）。
async fn app_installed<F: FileOps>(fs: &F, root: &str, app: &str) -> bool {
    let Some(dir) = app_dir(root, app) else {
        return false;
    };
    // Claude 特殊：~/.claude.json 存在也算初始化
    if app == "claude" {
        return fs.exists(&remote_claude_json_path(root)).await || fs.is_dir(&dir).await;
    }
    fs.is_dir(&dir).await
}

// ------------------------------------------------------------------------
// JSON 类 live 配置（Claude / Gemini / OpenCode）
// ------------------------------------------------------------------------

async fn json_upsert_many<F: FileOps>(
    fs: &F,
    path: &str,
    field: &str,
    items: &[(String, Value)],
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    // app_installed 已确保 app 配置目录存在；live 文件不存在时直接创建（对齐本机语义）
    let mut root: Value = match fs.read_text_optional(path).await? {
        Some(t) if !t.trim().is_empty() => {
            serde_json::from_str(&t).map_err(|e| format!("解析远端配置失败 {path}: {e}"))?
        }
        _ => json!({}),
    };
    {
        let obj = root
            .as_object_mut()
            .ok_or_else(|| format!("{path} 根必须是 JSON 对象"))?;
        if !obj.contains_key(field) {
            obj.insert(field.to_string(), json!({}));
        }
        let servers = obj
            .get_mut(field)
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| format!("{path} 的 {field} 必须是 JSON 对象"))?;
        for (id, spec) in items {
            servers.insert(id.clone(), spec.clone());
        }
    }
    let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    fs.write_text_atomic(path, &text).await
}

async fn json_remove_many<F: FileOps>(
    fs: &F,
    path: &str,
    field: &str,
    ids: &[String],
) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let Some(t) = fs.read_text_optional(path).await? else {
        return Ok(());
    };
    if t.trim().is_empty() {
        return Ok(());
    }
    let mut root: Value =
        serde_json::from_str(&t).map_err(|e| format!("解析远端配置失败 {path}: {e}"))?;
    let mut removed_any = false;
    if let Some(servers) = root.get_mut(field).and_then(|v| v.as_object_mut()) {
        for id in ids {
            removed_any |= servers.remove(id).is_some();
        }
    }
    if removed_any {
        let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
        fs.write_text_atomic(path, &text).await?;
    }
    Ok(())
}

// ------------------------------------------------------------------------
// OpenClaw live 配置（JSON5, openclaw.json 顶层 mcp.servers）
// ------------------------------------------------------------------------

/// 读取远端 openclaw.json 为 Value。用 JSON5 解析以兼容注释/尾逗号，
/// 缺失或为空时返回空对象。
async fn read_openclaw_json<F: FileOps>(fs: &F, path: &str) -> Result<Value, String> {
    let Some(t) = fs.read_text_optional(path).await? else {
        return Ok(json!({}));
    };
    if t.trim().is_empty() {
        return Ok(json!({}));
    }
    json5::from_str(&t).map_err(|e| format!("解析远端 OpenClaw 配置失败 {path}: {e}"))
}

/// 写回远端 openclaw.json。OpenClaw 官方接受纯 JSON（JSON 是 JSON5 子集），
/// 因此用 serde_json 序列化即可被 OpenClaw 读取。
async fn write_openclaw_json<F: FileOps>(fs: &F, path: &str, root: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(root).map_err(|e| e.to_string())?;
    fs.write_text_atomic(path, &text).await
}

/// 在远端 openclaw.json 的 mcp.servers 中 upsert 多个服务器（一次读写）。
async fn openclaw_json_upsert_many<F: FileOps>(
    fs: &F,
    path: &str,
    items: &[(String, Value)],
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let mut root = read_openclaw_json(fs, path).await?;
    {
        let obj = root
            .as_object_mut()
            .ok_or_else(|| format!("{path} 根必须是对象"))?;
        if !obj.contains_key("mcp") {
            obj.insert("mcp".to_string(), json!({}));
        }
        let mcp = obj
            .get_mut("mcp")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| format!("{path} 的 mcp 必须是对象"))?;
        if !mcp.contains_key("servers") {
            mcp.insert("servers".to_string(), json!({}));
        }
        let servers = mcp
            .get_mut("servers")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| format!("{path} 的 mcp.servers 必须是对象"))?;
        for (id, spec) in items {
            servers.insert(id.clone(), spec.clone());
        }
    }
    write_openclaw_json(fs, path, &root).await
}

/// 从远端 openclaw.json 的 mcp.servers 移除多个服务器（一次读写）。
async fn openclaw_json_remove_many<F: FileOps>(
    fs: &F,
    path: &str,
    ids: &[String],
) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut root = read_openclaw_json(fs, path).await?;
    let mut removed_any = false;
    if let Some(servers) = root
        .get_mut("mcp")
        .and_then(|v| v.as_object_mut())
        .and_then(|m| m.get_mut("servers"))
        .and_then(|v| v.as_object_mut())
    {
        for id in ids {
            removed_any |= servers.remove(id).is_some();
        }
    }
    if removed_any {
        write_openclaw_json(fs, path, &root).await?;
    }
    Ok(())
}

// ------------------------------------------------------------------------
// TOML 类 live 配置（Codex / GrokBuild）
// ------------------------------------------------------------------------

async fn toml_upsert_many<F: FileOps>(
    fs: &F,
    path: &str,
    items: &[(String, Value)],
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let text = fs.read_text_optional(path).await?.unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("解析远端 TOML 失败 {path}: {e}"))?;
    for (id, spec) in items {
        let table = crate::mcp::json_server_to_toml_table(spec).map_err(|e| e.to_string())?;
        upsert_toml_table(&mut doc, id, table)?;
    }
    fs.write_text_atomic(path, &doc.to_string()).await
}

fn upsert_toml_table(
    doc: &mut toml_edit::DocumentMut,
    id: &str,
    table: toml_edit::Table,
) -> Result<(), String> {
    if doc
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
        .is_none()
    {
        if doc.get("mcp_servers").is_some_and(|i| !i.is_none()) {
            log::warn!("远端 TOML 的 mcp_servers 不是表，已重置为空表");
        }
        doc["mcp_servers"] = toml_edit::table();
    }
    let servers = doc
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| "远端 TOML 的 mcp_servers 不是表".to_string())?;
    servers.insert(id, toml_edit::Item::Table(table));
    Ok(())
}

async fn toml_remove_many<F: FileOps>(fs: &F, path: &str, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let Some(t) = fs.read_text_optional(path).await? else {
        return Ok(());
    };
    if t.trim().is_empty() {
        return Ok(());
    }
    let mut doc: toml_edit::DocumentMut = match t.parse() {
        Ok(d) => d,
        Err(_) => return Ok(()), // 解析失败，无法删除不存在内容
    };
    let mut removed_any = false;
    if let Some(servers) = doc
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
    {
        for id in ids {
            removed_any |= servers.remove(id).is_some();
        }
    }
    if removed_any {
        fs.write_text_atomic(path, &doc.to_string()).await?;
    }
    Ok(())
}

// ------------------------------------------------------------------------
// Hermes live 配置（YAML）
// ------------------------------------------------------------------------

async fn hermes_upsert_many<F: FileOps>(
    fs: &F,
    path: &str,
    items: &[(String, Value)],
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let text = fs.read_text_optional(path).await?.unwrap_or_default();
    let mut y: serde_yaml::Value = if text.trim().is_empty() {
        serde_yaml::Value::Mapping(Default::default())
    } else {
        serde_yaml::from_str(&text).map_err(|e| format!("解析 Hermes config.yaml 失败: {e}"))?
    };

    let map = y
        .as_mapping_mut()
        .ok_or_else(|| "Hermes config.yaml 根必须是映射".to_string())?;
    let servers_key = serde_yaml::Value::String("mcp_servers".to_string());
    if !map.contains_key(&servers_key) {
        map.insert(
            servers_key.clone(),
            serde_yaml::Value::Mapping(Default::default()),
        );
    }
    let servers = map
        .get_mut(&servers_key)
        .and_then(|v| v.as_mapping_mut())
        .ok_or_else(|| "Hermes config.yaml 的 mcp_servers 不是映射".to_string())?;

    for (id, spec) in items {
        let hermes_spec = crate::mcp::convert_to_hermes_format(spec).map_err(|e| e.to_string())?;
        let id_key = serde_yaml::Value::String(id.clone());
        let merged = if let Some(existing) = servers.get(&id_key) {
            let existing_json = yaml_to_json(existing)?;
            crate::mcp::merge_hermes_spec(&existing_json, &hermes_spec)
        } else {
            hermes_spec
        };
        servers.insert(id_key, json_to_yaml(&merged)?);
    }

    let new_text =
        serde_yaml::to_string(&y).map_err(|e| format!("序列化 Hermes config.yaml 失败: {e}"))?;
    fs.write_text_atomic(path, &new_text).await
}

async fn hermes_remove_many<F: FileOps>(fs: &F, path: &str, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let Some(t) = fs.read_text_optional(path).await? else {
        return Ok(());
    };
    if t.trim().is_empty() {
        return Ok(());
    }
    let mut y: serde_yaml::Value =
        serde_yaml::from_str(&t).map_err(|e| format!("解析 Hermes config.yaml 失败: {e}"))?;
    let Some(map) = y.as_mapping_mut() else {
        return Ok(());
    };
    let mut removed_any = false;
    if let Some(servers) = map
        .get_mut(serde_yaml::Value::String("mcp_servers".to_string()))
        .and_then(|v| v.as_mapping_mut())
    {
        for id in ids {
            removed_any |= servers
                .remove(serde_yaml::Value::String(id.clone()))
                .is_some();
        }
    }
    if removed_any {
        let new_text = serde_yaml::to_string(&y).map_err(|e| e.to_string())?;
        fs.write_text_atomic(path, &new_text).await?;
    }
    Ok(())
}

/// serde_yaml Value -> serde_json Value。
fn yaml_to_json(v: &serde_yaml::Value) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| format!("YAML 转 JSON 失败: {e}"))
}

/// serde_json Value -> serde_yaml Value。
fn json_to_yaml(v: &Value) -> Result<serde_yaml::Value, String> {
    serde_yaml::to_value(v).map_err(|e| format!("JSON 转 YAML 失败: {e}"))
}

// ========================================================================
// 导入辅助：读各 live 配置的 {id: spec}
// ========================================================================

async fn read_live_servers<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
) -> Result<IndexMap<String, Value>, String> {
    match app {
        "claude" => read_json_field_map(fs, &remote_claude_json_path(root), "mcpServers").await,
        "codex" => read_toml_mcp_map(fs, &codex_config_path(root)).await,
        "gemini" => read_json_field_map(fs, &gemini_settings_path(root), "mcpServers").await,
        "grokbuild" => read_toml_mcp_map(fs, &grok_config_path(root)).await,
        "opencode" => read_json_field_map(fs, &opencode_config_path(root), "mcp").await,
        "hermes" => read_yaml_mcp_map(fs, &hermes_config_path(root)).await,
        "openclaw" => read_openclaw_mcp_map(fs, &openclaw_config_path(root)).await,
        _ => Ok(IndexMap::new()),
    }
}

async fn read_json_field_map<F: FileOps>(
    fs: &F,
    path: &str,
    field: &str,
) -> Result<IndexMap<String, Value>, String> {
    if !fs.exists(path).await {
        return Ok(IndexMap::new());
    }
    let Some(t) = fs.read_text_optional(path).await? else {
        return Ok(IndexMap::new());
    };
    if t.trim().is_empty() {
        return Ok(IndexMap::new());
    }
    let root: Value =
        serde_json::from_str(&t).map_err(|e| format!("解析远端配置失败 {path}: {e}"))?;
    let mut out = IndexMap::new();
    if let Some(map) = root.get(field).and_then(|v| v.as_object()) {
        for (k, v) in map {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(out)
}

async fn read_toml_mcp_map<F: FileOps>(
    fs: &F,
    path: &str,
) -> Result<IndexMap<String, Value>, String> {
    if !fs.exists(path).await {
        return Ok(IndexMap::new());
    }
    let Some(t) = fs.read_text_optional(path).await? else {
        return Ok(IndexMap::new());
    };
    if t.trim().is_empty() {
        return Ok(IndexMap::new());
    }
    let doc: toml_edit::DocumentMut = t
        .parse()
        .map_err(|e| format!("解析远端 TOML 失败 {path}: {e}"))?;
    let mut out = IndexMap::new();
    let Some(table) = doc
        .get("mcp_servers")
        .and_then(toml_edit::Item::as_table_like)
    else {
        return Ok(out);
    };
    for (k, item) in table.iter() {
        if let Ok(spec) = toml_item_to_json(item) {
            out.insert(k.to_string(), spec);
        }
    }
    Ok(out)
}

fn toml_item_to_json(item: &toml_edit::Item) -> Result<Value, String> {
    let toml_str = item.to_string();
    // 用 toml crate 反序列化该子表为 JSON 兼容结构
    let val: toml::Value =
        toml::from_str(&toml_str).map_err(|e| format!("解析 TOML 条目失败: {e}"))?;
    serde_json::to_value(val).map_err(|e| format!("TOML 转 JSON 失败: {e}"))
}

async fn read_yaml_mcp_map<F: FileOps>(
    fs: &F,
    path: &str,
) -> Result<IndexMap<String, Value>, String> {
    if !fs.exists(path).await {
        return Ok(IndexMap::new());
    }
    let Some(t) = fs.read_text_optional(path).await? else {
        return Ok(IndexMap::new());
    };
    if t.trim().is_empty() {
        return Ok(IndexMap::new());
    }
    let y: serde_yaml::Value =
        serde_yaml::from_str(&t).map_err(|e| format!("解析 Hermes config.yaml 失败: {e}"))?;
    let mut out = IndexMap::new();
    if let Some(map) = y.get("mcp_servers").and_then(|v| v.as_mapping()) {
        for (k, v) in map {
            if let Some(key) = k.as_str() {
                if let Ok(spec) = yaml_to_json(v) {
                    out.insert(key.to_string(), spec);
                }
            }
        }
    }
    Ok(out)
}

async fn read_openclaw_mcp_map<F: FileOps>(
    fs: &F,
    path: &str,
) -> Result<IndexMap<String, Value>, String> {
    let root = read_openclaw_json(fs, path).await?;
    let mut out = IndexMap::new();
    if let Some(servers) = root
        .get("mcp")
        .and_then(|v| v.as_object())
        .and_then(|m| m.get("servers"))
        .and_then(|v| v.as_object())
    {
        for (id, spec) in servers {
            // OpenClaw 条目格式 → 统一格式（与远端同步时相反）
            if let Ok(unified) = crate::mcp::convert_from_openclaw_format(id, spec) {
                out.insert(id.clone(), unified);
            } else {
                log::warn!("跳过无效的远端 OpenClaw MCP 服务器 '{id}'");
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsops::LocalFileOps;

    fn seed_server(id: &str) -> RemoteMcpServer {
        RemoteMcpServer {
            id: id.to_string(),
            name: id.to_string(),
            server: json!({ "type": "stdio", "command": "npx", "args": ["-y", id] }),
            apps: RemoteMcpApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        }
    }

    /// 预置 SSOT(a/b/c) + claude live 文件,返回 root 字符串。
    async fn seed(tmp: &tempfile::TempDir) -> String {
        let root = tmp.path().to_string_lossy().to_string();
        let fs = LocalFileOps;

        let mut map = IndexMap::new();
        for id in ["a", "b", "c"] {
            map.insert(id.to_string(), seed_server(id));
        }
        write_remote_mcp_ssot(&fs, &root, &map)
            .await
            .expect("seed ssot");
        // claude 的 app_installed 依赖 ~/.claude.json 或 ~/.claude 目录存在
        fs.write_text_atomic(&remote_claude_json_path(&root), "{}")
            .await
            .expect("seed claude.json");
        root
    }

    async fn live_claude_servers(root: &str) -> serde_json::Map<String, Value> {
        let fs = LocalFileOps;
        let t = fs
            .read_text_optional(&remote_claude_json_path(root))
            .await
            .expect("read claude.json")
            .expect("claude.json exists");
        serde_json::from_str::<Value>(&t)
            .expect("parse claude.json")
            .get("mcpServers")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn bulk_enable_writes_ssot_and_live_once_with_per_item_failures() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = seed(&tmp).await;
        let fs = LocalFileOps;

        let result = bulk_toggle_remote_mcp_app(
            &fs,
            &root,
            &["a".to_string(), "b".to_string(), "missing".to_string()],
            "claude",
            true,
        )
        .await
        .expect("bulk toggle");

        assert_eq!(result.succeeded, vec!["a", "b"]);
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].item, "missing");
        assert!(result.failed[0].error.contains("不存在"));

        // SSOT: a/b claude 已启用,c 未动
        let map = read_remote_mcp_ssot(&fs, &root).await.expect("read ssot");
        assert!(map["a"].apps.claude);
        assert!(map["b"].apps.claude);
        assert!(!map["c"].apps.claude);

        // claude live: 含 a/b,不含 missing
        let live = live_claude_servers(&root).await;
        assert!(live.contains_key("a"));
        assert!(live.contains_key("b"));
        assert!(!live.contains_key("missing"));
    }

    #[tokio::test]
    async fn bulk_disable_removes_from_live() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = seed(&tmp).await;
        let fs = LocalFileOps;

        bulk_toggle_remote_mcp_app(
            &fs,
            &root,
            &["a".to_string(), "b".to_string()],
            "claude",
            true,
        )
        .await
        .expect("enable");

        bulk_toggle_remote_mcp_app(
            &fs,
            &root,
            &["a".to_string(), "b".to_string()],
            "claude",
            false,
        )
        .await
        .expect("disable");

        let map = read_remote_mcp_ssot(&fs, &root).await.expect("read ssot");
        assert!(!map["a"].apps.claude);
        assert!(!map["b"].apps.claude);

        let live = live_claude_servers(&root).await;
        assert!(!live.contains_key("a"));
        assert!(!live.contains_key("b"));
    }

    #[tokio::test]
    async fn single_toggle_matches_bulk_of_one() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = seed(&tmp).await;
        let fs = LocalFileOps;

        toggle_remote_mcp_app(&fs, &root, "a", "claude", true)
            .await
            .expect("single toggle");

        let map = read_remote_mcp_ssot(&fs, &root).await.expect("read ssot");
        assert!(map["a"].apps.claude);
        let live = live_claude_servers(&root).await;
        assert!(live.contains_key("a"));
    }

    #[tokio::test]
    async fn empty_ids_is_noop() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = seed(&tmp).await;
        let fs = LocalFileOps;

        let result = bulk_toggle_remote_mcp_app(&fs, &root, &[], "claude", true)
            .await
            .expect("bulk empty");
        assert!(result.succeeded.is_empty());
        assert!(result.failed.is_empty());

        let map = read_remote_mcp_ssot(&fs, &root).await.expect("read ssot");
        assert!(!map["a"].apps.claude);
    }
}
