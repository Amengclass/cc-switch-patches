//! 文件操作抽象：本机走 `std::fs`，远端走 SFTP。
//!
//! 让 session / prompt / mcp / skill 等逻辑只依赖 `FileOps` 接口，
//! 通过传入 `LocalFileOps`（本机）或 `RemoteSftpFileOps`（远端）实现「同一套逻辑、两套数据源」。

use std::io::SeekFrom;
use std::path::Path;

use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// 目录项。
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// 文件操作接口（异步，远端 SFTP / docker exec 需要 await）。
pub trait FileOps {
    async fn exists(&self, path: &str) -> bool;
    async fn is_dir(&self, path: &str) -> bool;
    async fn read_head_tail_lines(
        &self,
        path: &str,
        head_n: usize,
        tail_n: usize,
    ) -> Result<(Vec<String>, Vec<String>), String>;
    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, String>;
    async fn remove_file(&self, path: &str) -> Result<(), String>;
    async fn remove_dir_all(&self, path: &str) -> Result<(), String>;
    /// 读取整个文本文件；文件不存在时返回 None。
    async fn read_text_optional(&self, path: &str) -> Result<Option<String>, String>;
    /// 原子写回整个文本文件（临时文件 + rename），自动确保父目录存在。
    async fn write_text_atomic(&self, path: &str, content: &str) -> Result<(), String>;
}

/// 本机实现：直接走 std::fs。
#[allow(dead_code)]
pub struct LocalFileOps;

impl FileOps for LocalFileOps {
    async fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    async fn is_dir(&self, path: &str) -> bool {
        Path::new(path).is_dir()
    }

    async fn read_head_tail_lines(
        &self,
        path: &str,
        head_n: usize,
        tail_n: usize,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        crate::session_manager::providers::utils::read_head_tail_lines(
            Path::new(path),
            head_n,
            tail_n,
        )
        .map_err(|e| e.to_string())
    }

    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, String> {
        let entries = std::fs::read_dir(path).map_err(|e| format!("读取目录失败 {path}: {e}"))?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let p = entry.path();
            out.push(DirEntry {
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                path: p.to_string_lossy().to_string(),
                is_dir: p.is_dir(),
            });
        }
        Ok(out)
    }

    async fn remove_file(&self, path: &str) -> Result<(), String> {
        std::fs::remove_file(path).map_err(|e| format!("删除文件失败 {path}: {e}"))
    }

    async fn remove_dir_all(&self, path: &str) -> Result<(), String> {
        std::fs::remove_dir_all(path).map_err(|e| format!("删除目录失败 {path}: {e}"))
    }

    async fn read_text_optional(&self, path: &str) -> Result<Option<String>, String> {
        let p = Path::new(path);
        if !p.exists() {
            return Ok(None);
        }
        std::fs::read_to_string(p)
            .map(Some)
            .map_err(|e| format!("读取文件失败 {path}: {e}"))
    }

    async fn write_text_atomic(&self, path: &str, content: &str) -> Result<(), String> {
        let p = Path::new(path);
        if let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录失败 {}: {e}", parent.display()))?;
        }
        crate::config::write_text_file(p, content).map_err(|e| format!("写入文件失败 {path}: {e}"))
    }
}

/// 远端实现：走 SFTP。
pub struct RemoteSftpFileOps<'a> {
    pub sftp: &'a SftpSession,
}

/// 从文件内容里取头/尾行（纯函数，供本机与远端共用）。
pub fn split_head_tail(content: &str, head_n: usize, tail_n: usize) -> (Vec<String>, Vec<String>) {
    let all: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let head = all.iter().take(head_n).cloned().collect();
    let skip = all.len().saturating_sub(tail_n);
    let tail = all.into_iter().skip(skip).collect();
    (head, tail)
}

impl FileOps for RemoteSftpFileOps<'_> {
    async fn exists(&self, path: &str) -> bool {
        self.sftp.try_exists(path).await.unwrap_or(false)
    }

    async fn is_dir(&self, path: &str) -> bool {
        self.sftp
            .metadata(path)
            .await
            .map(|m| m.file_type().is_dir())
            .unwrap_or(false)
    }

    async fn read_head_tail_lines(
        &self,
        path: &str,
        head_n: usize,
        tail_n: usize,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        // 与本机 utils::read_head_tail_lines 完全一致：
        // 小文件(<16KB)全读 split；大文件只读头部 head_n 行 + seek 到末尾 ~16KB 取尾 tail_n 行。
        // 会话文件可能很大（几千行），全量 SFTP 传输正是会话列表卡顿的根因。
        const HEAD_TAIL_BUFFER: u64 = 16 * 1024;

        let Some(size) = self
            .sftp
            .metadata(path)
            .await
            .map_err(|e| format!("远端读取失败 {path}: {e}"))?
            .size
        else {
            // 远端未返回文件大小：退回整文件读取
            let data = self
                .sftp
                .read(path)
                .await
                .map_err(|e| format!("远端读取失败 {path}: {e}"))?;
            let content = String::from_utf8_lossy(&data);
            return Ok(split_head_tail(&content, head_n, tail_n));
        };

        // 小文件：全读一次 split（与本机 < 16KB 分支一致，省一次 open/seek 往返）
        if size <= HEAD_TAIL_BUFFER {
            let data = self
                .sftp
                .read(path)
                .await
                .map_err(|e| format!("远端读取失败 {path}: {e}"))?;
            let content = String::from_utf8_lossy(&data);
            return Ok(split_head_tail(&content, head_n, tail_n));
        }

        let mut file = self
            .sftp
            .open(path)
            .await
            .map_err(|e| format!("远端打开失败 {path}: {e}"))?;

        // 头部：按行读，恰好取 head_n 行（读完 head_n 行即停）
        let head = read_head_lines(&mut file, head_n, path).await?;

        // 尾部：seek 到「文件大小 - 16KB」，读最后 ~16KB，跳过截断的半行后取尾 tail_n 行
        let seek_pos = size.saturating_sub(HEAD_TAIL_BUFFER);
        file.seek(SeekFrom::Start(seek_pos))
            .await
            .map_err(|e| format!("远端读取失败 {path}: {e}"))?;
        let tail_buf = read_up_to(&mut file, HEAD_TAIL_BUFFER, path).await?;
        let tail_text = String::from_utf8_lossy(&tail_buf);
        // 与本机一致：seek 进入行中间时第一个元素是半行，跳过
        let skip_first = if seek_pos > 0 { 1 } else { 0 };
        let usable: Vec<&str> = tail_text.lines().skip(skip_first).collect();
        let skip = usable.len().saturating_sub(tail_n);
        let tail: Vec<String> = usable
            .into_iter()
            .skip(skip)
            .map(|s| s.to_string())
            .collect();

        Ok((head, tail))
    }

    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, String> {
        let dir = self
            .sftp
            .read_dir(path)
            .await
            .map_err(|e| format!("远端读取目录失败 {path}: {e}"))?;
        let mut out = Vec::new();
        for entry in dir {
            out.push(DirEntry {
                name: entry.file_name(),
                path: entry.path(),
                is_dir: entry.file_type().is_dir(),
            });
        }
        Ok(out)
    }

    async fn remove_file(&self, path: &str) -> Result<(), String> {
        self.sftp
            .remove_file(path)
            .await
            .map_err(|e| format!("远端删除文件失败 {path}: {e}"))
    }

    async fn remove_dir_all(&self, path: &str) -> Result<(), String> {
        // 远端递归删除：先删内容再删目录本身
        let dir = self.read_dir(path).await?;
        for entry in dir {
            let p = entry.path;
            if entry.is_dir {
                Box::pin(self.remove_dir_all(&p)).await?;
            } else {
                self.remove_file(&p).await?;
            }
        }
        self.sftp
            .remove_dir(path)
            .await
            .map_err(|e| format!("远端删除目录失败 {path}: {e}"))
    }

    async fn read_text_optional(&self, path: &str) -> Result<Option<String>, String> {
        crate::remote::sftp_io::read_remote_text_optional(self.sftp, path).await
    }

    async fn write_text_atomic(&self, path: &str, content: &str) -> Result<(), String> {
        crate::remote::sftp_io::write_remote_text_atomic(self.sftp, path, content).await
    }
}

/// 从 SFTP 文件头部按行读取，恰好取 `head_n` 行（读完 head_n 行即停）。
/// 与本机 `BufReader::lines().take(head_n)` 语义一致。
async fn read_head_lines(
    file: &mut (impl tokio::io::AsyncRead + Unpin),
    head_n: usize,
    path: &str,
) -> Result<Vec<String>, String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = file
            .read(&mut chunk)
            .await
            .map_err(|e| format!("远端读取失败 {path}: {e}"))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        // 收集到 head_n 个换行即停（head_n 行完整行已到齐）
        if buf.iter().filter(|&&b| b == b'\n').count() >= head_n {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut head: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    // 可能在行中间截断（最后一段无换行），truncate 保证恰好 head_n 行完整行
    head.truncate(head_n);
    Ok(head)
}

/// 从 SFTP 文件最多读取 `limit` 字节（分块循环读，兼容服务器按包返回）。
async fn read_up_to(
    file: &mut (impl tokio::io::AsyncRead + Unpin),
    limit: u64,
    path: &str,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    while (out.len() as u64) < limit {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| format!("远端读取失败 {path}: {e}"))?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}
