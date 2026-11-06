//! 远程会话管理：浏览/读取/删除远端各 app 的会话。
//!
//! 通过 `FileOps` 接口支持宿主机（SFTP）与容器内（docker exec）两种数据源。
//! 单个会话文件（metadata 除外）与 SQLite 会话（经 sqlite-helper）都在这层分发。

use std::path::Path;

use crate::fsops::FileOps;
use crate::session_manager::SessionMeta;

// ============================================================================
// 远端会话「列表」扫描（FileOps 泛型）：把本机各 provider 的 meta 解析复用到远端
// ============================================================================

/// 递归收集远端目录下满足谓词的文件路径。
///
/// 走 `FileOps::list_files_recursive`：本机/SFTP 逐目录 `read_dir`，容器一次 `find`
/// ——容器逐目录遍历时每次都是一趟 docker exec，多层目录会慢到不可用。
pub async fn collect_files_fs<F: FileOps>(
    fs: &F,
    root: &str,
    pred: impl Fn(&str) -> bool,
) -> Vec<String> {
    fs.list_files_recursive(root)
        .await
        .into_iter()
        .filter(|p| pred(p))
        .collect()
}

/// claude：`<root>/**/*.jsonl`，跳过 agent-*/journal。
pub async fn scan_claude_fs<F: FileOps>(fs: &F, root: &str) -> Vec<SessionMeta> {
    use crate::session_manager::providers::claude;
    let files = collect_files_fs(fs, root, |p| p.ends_with(".jsonl")).await;
    let mut out = Vec::new();
    for f in files {
        if claude::is_agent_session(Path::new(&f)) {
            continue;
        }
        if let Ok((head, tail)) = fs.read_head_tail_lines(&f, 10, 30).await {
            if let Some(m) = claude::parse_session_lines(Path::new(&f), head, tail) {
                out.push(m);
            }
        }
    }
    out
}

/// codex：`<root>/**/*.jsonl`。`titles` 来自 `session_index.jsonl` / `state_5.sqlite`，
/// 与本机 `scan_sessions_in_roots_with_titles` 一致（有自定义线程名时覆盖首条用户消息）。
pub async fn scan_codex_fs<F: FileOps>(
    fs: &F,
    root: &str,
    titles: &std::collections::HashMap<String, String>,
) -> Vec<SessionMeta> {
    use crate::session_manager::providers::codex;
    let files = collect_files_fs(fs, root, |p| p.ends_with(".jsonl")).await;
    let mut out = Vec::new();
    for f in files {
        if let Ok((head, tail)) = fs.read_head_tail_lines(&f, 10, 30).await {
            if let Some(m) = codex::parse_session_lines(
                Path::new(&f),
                head,
                tail,
                titles,
            ) {
                out.push(m);
            }
        }
    }
    out
}

/// openclaw：`<root>/**/*.jsonl`（同名目录下若有 `sessions.json` 索引，读显示名）。
pub async fn scan_openclaw_fs<F: FileOps>(fs: &F, root: &str) -> Vec<SessionMeta> {
    use std::collections::HashMap;
    use crate::session_manager::providers::openclaw;
    let files = collect_files_fs(fs, root, |p| p.ends_with(".jsonl")).await;
    let mut names: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut out = Vec::new();
    for f in files {
        let dir = Path::new(&f)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if !names.contains_key(&dir) {
            let index_path = format!("{dir}/sessions.json");
            let map = match fs.read_text_optional(&index_path).await {
                Ok(Some(text)) => openclaw::parse_display_names(&text),
                _ => HashMap::new(),
            };
            names.insert(dir.clone(), map);
        }
        let display_names = names.get(&dir);
        if let Ok((head, tail)) = fs.read_head_tail_lines(&f, 10, 30).await {
            if let Some(m) =
                openclaw::parse_session_lines(Path::new(&f), head, tail, display_names)
            {
                out.push(m);
            }
        }
    }
    out
}

/// grokbuild：`<root>/**/summary.json`（读文本构建）。
pub async fn scan_grokbuild_fs<F: FileOps>(fs: &F, root: &str) -> Vec<SessionMeta> {
    use crate::session_manager::providers::grokbuild;
    let files = collect_files_fs(fs, root, |p| p.ends_with("summary.json")).await;
    let mut out = Vec::new();
    for f in files {
        if let Ok(Some(text)) = fs.read_text_optional(&f).await {
            if let Some(m) = grokbuild::parse_summary_text(&text, Path::new(&f)) {
                out.push(m);
            }
        }
    }
    out
}

/// gemini：`<root>/<project>/chats/*.json` 或新版 `*.jsonl`（同名 .jsonl 存在时旧 .json 跳过）。
pub async fn scan_gemini_fs<F: FileOps>(fs: &F, root: &str) -> Vec<SessionMeta> {
    use crate::session_manager::providers::gemini;
    let files = collect_files_fs(fs, root, |p| p.ends_with(".jsonl") || p.ends_with(".json")).await;
    let mut out = Vec::new();
    for f in files {
        // 与本机 is_session_file 一致：resume 迁移后同名 .json 残留，只认 .jsonl
        if f.ends_with(".json") {
            let jsonl = format!("{}.jsonl", &f[..f.len() - ".json".len()]);
            if fs.exists(&jsonl).await {
                continue;
            }
        }
        if let Ok(Some(text)) = fs.read_text_optional(&f).await {
            if let Some(mut meta) = gemini::parse_session_from_text(Path::new(&f), &text) {
                // 与本机一致：项目目录取 `<project>/.project_root` 的内容
                if let Some(chats_dir) = Path::new(&f).parent() {
                    if let Some(project_dir) = chats_dir.parent() {
                        let project_root = format!(
                            "{}/.project_root",
                            project_dir.to_string_lossy().trim_end_matches('/')
                        );
                        if let Ok(Some(text)) = fs.read_text_optional(&project_root).await {
                            meta.project_dir = Some(text.trim().to_string());
                        }
                    }
                }
                out.push(meta);
            }
        }
    }
    out
}

/// hermes：`<root>/*.jsonl|*.json`（旧版 JSONL 会话，与 SQLite 主存储并存，按 id 去重）。
pub async fn scan_hermes_fs<F: FileOps>(fs: &F, root: &str) -> Vec<SessionMeta> {
    use crate::session_manager::providers::hermes;
    let files = collect_files_fs(fs, root, |p| p.ends_with(".jsonl") || p.ends_with(".json")).await;
    let mut out = Vec::new();
    for f in files {
        if let Ok((head, tail)) = fs.read_head_tail_lines(&f, 30, 10).await {
            if let Some(m) = hermes::parse_jsonl_session_lines(Path::new(&f), head, tail) {
                out.push(m);
            }
        }
    }
    out
}

/// pi：`<root>/**/*.jsonl`（读文本构建会话树）。
pub async fn scan_pi_fs<F: FileOps>(fs: &F, root: &str) -> Vec<SessionMeta> {
    use crate::session_manager::providers::pi;
    let files = collect_files_fs(fs, root, |p| p.ends_with(".jsonl")).await;
    let mut out = Vec::new();
    for f in files {
        if let Ok(Some(text)) = fs.read_text_optional(&f).await {
            if let Ok(m) = pi::parse_session_from_content(Path::new(&f), &text) {
                out.push(m);
            }
        }
    }
    out
}

// ============================================================================
// 远端会话「按需取全文 / 图片」（FileOps 泛型）：对齐本机 session_manager::content
// ============================================================================

/// 单次读取的全文上限（与本机 content::MAX_TEXT_BYTES 一致）
pub const MAX_TEXT_BYTES: u64 = 32 * 1024 * 1024;
/// 图片（解码后）上限（与本机 content::MAX_IMAGE_BYTES 一致）
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

const STALE: &str = "会话已更新，请重新读取";
const OUTSIDE: &str = "引用的路径不在会话目录内";
const NOT_TEXT: &str = "引用的内容不是文本";
const TOO_LARGE_TEXT: &str = "内容超过 32MB，请直接打开源文件查看";

/// 按 [`ContentRef`] 校验并读取远端全文（对齐本机 `content::resolve_content_ref`）。
/// `source_path` 是列表里那一条的 sourcePath；`app` 决定 Jsonl / File 的落点规则；
/// `root` 是该 app 的会话根（File 引用的相对路径以它为基，如 gemini 的 `~/.gemini/tmp`）。
pub async fn resolve_content_ref_fs<F: FileOps>(
    fs: &F,
    app: &str,
    source_path: &str,
    root: &str,
    content: &crate::session_manager::model::ContentRef,
) -> Result<String, String> {
    use crate::session_manager::model::ContentRef;
    match content {
        ContentRef::Jsonl {
            offset,
            len,
            pointer,
        } => {
            // Grok Build：正文在同目录固定名的 chat_history.jsonl；其余 app 就是源本身
            let jsonl = if app == "grokbuild" {
                let dir = std::path::Path::new(source_path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .ok_or_else(|| OUTSIDE.to_string())?;
                format!("{dir}/{}", crate::session_manager::content::GROK_CHAT_HISTORY)
            } else {
                source_path.to_string()
            };
            let line = read_jsonl_line_fs(fs, &jsonl, *offset, u64::from(*len)).await?;
            let value: serde_json::Value =
                serde_json::from_slice(&line).map_err(|_| STALE.to_string())?;
            crate::session_manager::content::extract_text(&value, pointer)
        }
        ContentRef::File { rel_path, pointer } => {
            // 相对路径以该 app 的会话根为基（与本机 `safe_join(root, rel_path)` 一致）
            let file = safe_join_fs(root.to_string(), rel_path)?;
            let bytes = fs
                .read_bytes_optional(&file)
                .await?
                .ok_or_else(|| "引用的文件不存在".to_string())?;
            if bytes.len() as u64 > MAX_TEXT_BYTES {
                return Err(TOO_LARGE_TEXT.to_string());
            }
            let value: serde_json::Value = if app == "gemini" {
                std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(crate::session_manager::providers::gemini::parse_session_document)
                    .ok_or_else(|| STALE.to_string())?
            } else {
                serde_json::from_slice(&bytes).map_err(|_| STALE.to_string())?
            };
            if app == "gemini" && pointer.ends_with("/thoughts") {
                if let Some(thoughts) = value
                    .pointer(pointer)
                    .and_then(crate::session_manager::providers::gemini::format_thoughts)
                {
                    return Ok(thoughts.text);
                }
            }
            crate::session_manager::content::extract_text(&value, pointer)
        }
        ContentRef::Sidecar { rel_path } => {
            // 附属目录：源文件去扩展名（Claude <sessionId>/tool-results/…）
            let base = source_path
                .strip_suffix(".jsonl")
                .unwrap_or(source_path)
                .to_string();
            let file = safe_join_fs(base, rel_path)?;
            let bytes = fs
                .read_bytes_optional(&file)
                .await?
                .ok_or_else(|| "附属输出目录不存在".to_string())?;
            if bytes.len() as u64 > MAX_TEXT_BYTES {
                return Err(TOO_LARGE_TEXT.to_string());
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        }
        // SQLite 引用：远端不取列内全文（helper 查询成本高、收益低），给出明确提示
        ContentRef::Sqlite { .. } => Err("远端会话暂不支持按引用取 SQLite 全文".to_string()),
    }
}

/// 读取远端文件 `[offset, offset+len)` 并校验是完整一行（对齐本机 `read_jsonl_line` 的边界校验）。
async fn read_jsonl_line_fs<F: FileOps>(
    fs: &F,
    path: &str,
    offset: u64,
    len: u64,
) -> Result<Vec<u8>, String> {
    if len == 0 || len > MAX_TEXT_BYTES {
        return Err(STALE.to_string());
    }
    let file_len = fs.file_size(path).await.ok_or_else(|| "无法读取会话文件".to_string())?;
    let end = offset
        .checked_add(len)
        .filter(|end| *end <= file_len)
        .ok_or_else(|| STALE.to_string())?;
    // 多读前后各一字节判断行边界
    let start = offset.saturating_sub(1);
    let read_end = (end + 1).min(file_len);
    let buf = fs.read_range(path, start, read_end - start).await?;
    let line_start = (offset - start) as usize;
    if offset > 0 && buf.first() != Some(&b'\n') {
        return Err(STALE.to_string());
    }
    let mut line = &buf[line_start..line_start + len as usize];
    if line.last() == Some(&b'\n') {
        line = &line[..line.len() - 1];
    } else if end < file_len {
        let next = *buf.last().unwrap_or(&0);
        if next != b'\n' && next != b'\r' {
            return Err(STALE.to_string());
        }
    }
    if line.last() == Some(&b'\r') {
        line = &line[..line.len() - 1];
    }
    if line.is_empty() || line.contains(&b'\n') {
        return Err(STALE.to_string());
    }
    Ok(line.to_vec())
}

/// 拼接相对路径并挡 `..` 穿越（远端不做 canonicalize，按组件判断）。
fn safe_join_fs(base: String, rel_path: &str) -> Result<String, String> {
    use std::path::Component;
    let rel = std::path::Path::new(rel_path);
    if rel_path.is_empty()
        || !rel
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(OUTSIDE.to_string());
    }
    Ok(format!("{}/{}", base.trim_end_matches('/'), rel_path))
}

/// 读取远端图片原始字节（对齐本机 `content::load_image`）。
/// 远端只支持内联图片（LocalFile 的本地路径在远端机上也可能是本机路径，无法判断归属，
/// 因此统一按内联处理）；SVG 由解码前的扩展名判断不进这条路。
pub async fn load_image_fs<F: FileOps>(
    fs: &F,
    app: &str,
    source_path: &str,
    root: &str,
    image: &crate::session_manager::model::ImageRef,
) -> Result<Vec<u8>, String> {
    use crate::session_manager::model::ImageSource;
    match &image.source {
        ImageSource::Inline { content } => {
            let encoded = resolve_content_ref_fs(fs, app, source_path, root, content).await?;
            crate::session_manager::content::decode_inline_image(&encoded, MAX_IMAGE_BYTES)
        }
        ImageSource::LocalFile { path } => {
            // 远端目标上的本地图片：按绝对路径读取（限定图片扩展名 + 大小上限）
            let ext = std::path::Path::new(path)
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if !["png", "jpg", "jpeg", "gif", "webp", "bmp"].contains(&ext.as_str()) {
                return Err("不支持的图片格式".to_string());
            }
            let bytes = fs
                .read_bytes_optional(path)
                .await?
                .ok_or_else(|| "图片文件不存在".to_string())?;
            if bytes.len() as u64 > MAX_IMAGE_BYTES {
                return Err("图片超过 20MB，无法预览".to_string());
            }
            Ok(bytes)
        }
    }
}

// ============================================================================
// 远端会话「删除」（FileOps 泛型）：各 app 的删除语义与本机 delete_session 对齐
// ============================================================================

/// 删除远端会话文件。各 app 的附属清理（sidecar 目录 / 旧版残留 / 索引）与本机一致。
pub async fn delete_session_fs<F: FileOps>(
    fs: &F,
    app: &str,
    source_path: &str,
) -> Result<bool, String> {
    if !fs.exists(source_path).await {
        return Err(format!("session source not found: {source_path}"));
    }

    match app {
        // claude：同 stem 的 sidecar 目录一并删
        "claude" => {
            if let Some(stem) = Path::new(source_path).file_stem() {
                if let Some(parent) = Path::new(source_path).parent() {
                    let sibling = parent.join(stem);
                    let sibling = sibling.to_string_lossy().to_string();
                    if fs.exists(&sibling).await {
                        if fs.is_dir(&sibling).await {
                            fs.remove_dir_all(&sibling).await?;
                        } else {
                            fs.remove_file(&sibling).await?;
                        }
                    }
                }
            }
            fs.remove_file(source_path).await?;
        }
        // grokbuild：sourcePath 是 summary.json，删整个会话目录
        "grokbuild" => {
            let dir = Path::new(source_path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .ok_or_else(|| format!("无效的会话路径: {source_path}"))?;
            fs.remove_dir_all(&dir).await?;
        }
        // gemini：resume 迁移后残留的旧版 `.json` 先删，否则 `.jsonl` 没了它会重新出现
        "gemini" => {
            if source_path.ends_with(".jsonl") {
                let legacy = format!("{}.json", &source_path[..source_path.len() - 6]);
                if fs.exists(&legacy).await {
                    fs.remove_file(&legacy).await?;
                }
            }
            fs.remove_file(source_path).await?;
        }
        // codex / openclaw / pi / hermes(JSONL)：单文件即会话
        "codex" | "openclaw" | "pi" | "hermes" => {
            fs.remove_file(source_path).await?;
        }
        other => {
            return Err(format!(
                "远端会话删除暂不支持应用 {other}（尚未移植）"
            ))
        }
    }
    Ok(true)
}
