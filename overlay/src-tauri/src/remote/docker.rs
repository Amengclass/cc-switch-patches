//! Docker 容器内的文件操作（`DockerExecFileOps`）。
//!
//! 容器内的配置文件（如 `~/.claude/settings.json`）不在宿主机文件系统里，
//! SFTP 够不到，必须通过 `docker exec` 在容器内跑 shell 命令读写。
//! 本模块实现 `FileOps` trait 的第三种数据源（Local = 本机 / SFTP = 宿主机 /
//! Docker exec = 容器内），让 settings/MCP/Prompts/Skills 业务逻辑在容器内直接复用。

use russh::client::Handle;
use russh_sftp::client::SftpSession;

use crate::fsops::{split_head_tail, DirEntry, FileOps};

use super::connection::{exec_command, shell_quote, RemoteHandler};

/// 统一的数据源包装：宿主机（SFTP）或容器内（docker exec）。
/// 命令层构造一个 `RemoteTarget`，业务函数按 `FileOps` 泛型使用，
/// 不需要关心背后是哪种实现。
pub enum RemoteTarget<'a> {
    Sftp(crate::fsops::RemoteSftpFileOps<'a>),
    Docker(DockerExecFileOps<'a>),
}

impl<'a> RemoteTarget<'a> {
    /// 构造目标数据源。`container` 为 Some（非空）时走容器内，否则走宿主机 SFTP。
    /// 空串/空白容器名按宿主机处理（前端可能以 `""` 表示宿主机）。
    pub fn new(
        sftp: &'a SftpSession,
        channel: &'a Handle<RemoteHandler>,
        container: Option<&str>,
    ) -> Result<Self, String> {
        match container {
            Some(c) if c.trim().is_empty() => {
                Ok(RemoteTarget::Sftp(crate::fsops::RemoteSftpFileOps { sftp }))
            }
            Some(c) => Ok(RemoteTarget::Docker(DockerExecFileOps::new(channel, c)?)),
            None => Ok(RemoteTarget::Sftp(crate::fsops::RemoteSftpFileOps { sftp })),
        }
    }

    /// 获取底层 SFTP 句柄（仅宿主机模式可用，供 zip 安装等二进制写场景）。
    #[allow(dead_code)]
    pub fn sftp(&self) -> Option<&'a SftpSession> {
        match self {
            RemoteTarget::Sftp(f) => Some(f.sftp),
            RemoteTarget::Docker(_) => None,
        }
    }
}

impl FileOps for RemoteTarget<'_> {
    async fn exists(&self, path: &str) -> bool {
        match self {
            RemoteTarget::Sftp(f) => f.exists(path).await,
            RemoteTarget::Docker(f) => f.exists(path).await,
        }
    }

    async fn is_dir(&self, path: &str) -> bool {
        match self {
            RemoteTarget::Sftp(f) => f.is_dir(path).await,
            RemoteTarget::Docker(f) => f.is_dir(path).await,
        }
    }

    async fn read_head_tail_lines(
        &self,
        path: &str,
        head_n: usize,
        tail_n: usize,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        match self {
            RemoteTarget::Sftp(f) => f.read_head_tail_lines(path, head_n, tail_n).await,
            RemoteTarget::Docker(f) => f.read_head_tail_lines(path, head_n, tail_n).await,
        }
    }

    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, String> {
        match self {
            RemoteTarget::Sftp(f) => f.read_dir(path).await,
            RemoteTarget::Docker(f) => f.read_dir(path).await,
        }
    }

    async fn remove_file(&self, path: &str) -> Result<(), String> {
        match self {
            RemoteTarget::Sftp(f) => f.remove_file(path).await,
            RemoteTarget::Docker(f) => f.remove_file(path).await,
        }
    }

    async fn remove_dir_all(&self, path: &str) -> Result<(), String> {
        match self {
            RemoteTarget::Sftp(f) => f.remove_dir_all(path).await,
            RemoteTarget::Docker(f) => f.remove_dir_all(path).await,
        }
    }

    async fn read_text_optional(&self, path: &str) -> Result<Option<String>, String> {
        match self {
            RemoteTarget::Sftp(f) => f.read_text_optional(path).await,
            RemoteTarget::Docker(f) => f.read_text_optional(path).await,
        }
    }

    async fn write_text_atomic(&self, path: &str, content: &str) -> Result<(), String> {
        match self {
            RemoteTarget::Sftp(f) => f.write_text_atomic(path, content).await,
            RemoteTarget::Docker(f) => f.write_text_atomic(path, content).await,
        }
    }
}

/// 容器内文件操作：每个方法都通过 `docker exec <container> sh -c '<cmd>'` 封装。
///
/// 借用 `&Handle` 而非持有：`exec_command` 每次调用时才 `channel_open_session`，
/// 命令调用方可以同时借用 `channel`(exec)与 `sftp`(需要时)。
pub struct DockerExecFileOps<'a> {
    pub channel: &'a Handle<RemoteHandler>,
    pub container: String,
}

impl<'a> DockerExecFileOps<'a> {
    /// 构造容器名：只允许单段（容器名不包含路径分隔符）。
    pub fn new(channel: &'a Handle<RemoteHandler>, container: &str) -> Result<Self, String> {
        if container.is_empty()
            || container.contains('/')
            || container.contains('\\')
            || container.contains(' ')
            || container.contains(';')
            || container.contains('$')
            || container.contains('`')
            || container.contains('&')
            || container.contains('|')
            || container.contains('>')
            || container.contains('<')
        {
            return Err("非法容器名".to_string());
        }
        Ok(Self {
            channel,
            container: container.to_string(),
        })
    }

    async fn exec(&self, shell_cmd: &str) -> Result<String, String> {
        exec_command(
            self.channel,
            &format!(
                "docker exec {} sh -c {}",
                self.container,
                shell_quote(shell_cmd)
            ),
        )
        .await
    }

    /// 将二进制数据原子写入容器内文件（base64 编码 → stdin 管道 → 临时文件 → mv）。
    /// 数据通过 SSH channel 的 stdin 流式传入，不嵌入命令字符串，无大小限制。
    pub async fn write_bytes_atomic(&self, path: &str, data: &[u8]) -> Result<(), String> {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(data);

        let parent = match path.rsplit_once('/') {
            Some((d, _)) if !d.is_empty() => d,
            _ => "/",
        };
        let tmp = format!("{}.ccswitch.tmp", path);

        let script = format!(
            "mkdir -p {parent} && base64 -d > {tmp} && mv {tmp} {path}",
            parent = shell_quote(parent),
            tmp = shell_quote(&tmp),
            path = shell_quote(path),
        );
        let cmd = format!(
            "docker exec -i {} sh -c {}",
            self.container,
            shell_quote(&script),
        );
        let _ =
            crate::remote::connection::exec_command_with_stdin(self.channel, &cmd, b64.as_bytes())
                .await?;
        Ok(())
    }
}

impl FileOps for DockerExecFileOps<'_> {
    async fn exists(&self, path: &str) -> bool {
        // test -e 靠退出码，但 exec_command 不检查退出码。
        // 改用 stdout 输出结果：路径存在则打 "yes"。
        self.exec(&format!(
            "if test -e {}; then echo yes; else echo no; fi",
            shell_quote(path)
        ))
        .await
        .map(|s| s.trim() == "yes")
        .unwrap_or(false)
    }

    async fn is_dir(&self, path: &str) -> bool {
        self.exec(&format!(
            "if test -d {}; then echo yes; else echo no; fi",
            shell_quote(path)
        ))
        .await
        .map(|s| s.trim() == "yes")
        .unwrap_or(false)
    }

    async fn read_head_tail_lines(
        &self,
        path: &str,
        head_n: usize,
        tail_n: usize,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        // 与本机 utils::read_head_tail_lines 分档一致：
        // 小文件(<16KB)整文件读回、Rust 侧 split；大文件「头 head_n 行 + seek 末尾 ~16KB 取尾」。
        // 容器端等价实现：head -n 恰好取 head_n 行；tail -c 16384 服务端 seek 到末尾读 ~16KB 字节。
        // 一次 exec 完成分档判定 + 读取，首行标记 F/H 区分两种输出。
        const HEAD_TAIL_BUFFER: usize = 16 * 1024;
        const SEP: &str = "__CCSWITCH_HEADTAIL_SEP_7f3a2b__";
        let script = format!(
            "s=$(stat -c %s {p}); if [ \"$s\" -lt {buf} ]; then echo F; cat {p}; else echo H; {{ head -n {hn}; echo {sep}; tail -c {buf} {p}; }} < {p}; fi",
            p = shell_quote(path),
            hn = head_n,
            buf = HEAD_TAIL_BUFFER,
            sep = SEP,
        );
        let out = self
            .exec(&script)
            .await
            .map_err(|e| format!("容器内读取失败 {path}: {e}"))?;

        // 首行是 F/H 标记，去掉后为数据
        let (marker, rest) = match out.split_once('\n') {
            Some((m, r)) => (m.trim(), r),
            None => (out.as_str(), ""),
        };

        if marker == "F" {
            // 小文件：整文件内容直接 split 头尾（与本机 < 16KB 分支一致）
            return Ok(split_head_tail(rest, head_n, tail_n));
        }

        // 大文件：head 的 head_n 行 + SEP + 末尾 ~16KB 字节
        let mut parts = rest.splitn(2, SEP);
        let head: Vec<String> = parts
            .next()
            .unwrap_or("")
            .lines()
            .map(|l| l.to_string())
            .collect();
        // 末尾字节可能从行中间开始（seek 进入行内），前半行/空行不影响取最后 tail_n 行
        let tail_all: Vec<&str> = parts.next().unwrap_or("").lines().collect();
        let skip = tail_all.len().saturating_sub(tail_n);
        let tail: Vec<String> = tail_all
            .into_iter()
            .skip(skip)
            .map(|s| s.to_string())
            .collect();
        Ok((head, tail))
    }

    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, String> {
        // ls -pA：文件名后加 / 标记目录（如 "dir1/"、"file.jsonl"），一次 exec 区分文件/目录
        let out = self
            .exec(&format!("ls -pA {}", shell_quote(path)))
            .await
            .map_err(|e| format!("容器内读取目录失败 {path}: {e}"))?;
        let mut entries = Vec::new();
        for name in out.lines() {
            let name = name.trim().to_string();
            if name.is_empty() {
                continue;
            }
            let is_dir = name.ends_with('/');
            let clean_name = if is_dir {
                name[..name.len() - 1].to_string()
            } else {
                name.clone()
            };
            let full = format!("{}/{}", path.trim_end_matches('/'), clean_name);
            entries.push(DirEntry {
                name: clean_name,
                path: full,
                is_dir,
            });
        }
        Ok(entries)
    }

    async fn remove_file(&self, path: &str) -> Result<(), String> {
        self.exec(&format!("rm -f {}", shell_quote(path)))
            .await
            .map_err(|e| format!("容器内删除文件失败 {path}: {e}"))?;
        Ok(())
    }

    async fn remove_dir_all(&self, path: &str) -> Result<(), String> {
        self.exec(&format!("rm -rf {}", shell_quote(path)))
            .await
            .map_err(|e| format!("容器内删除目录失败 {path}: {e}"))?;
        Ok(())
    }

    async fn read_text_optional(&self, path: &str) -> Result<Option<String>, String> {
        // 直接 cat，文件不存在时返回空（省一次 test -e 的 round-trip）
        let out = self
            .exec(&format!("cat {} 2>/dev/null", shell_quote(path)))
            .await;
        match out {
            Ok(text) => Ok(if text.is_empty() { None } else { Some(text) }),
            Err(_) => Ok(None),
        }
    }

    async fn write_text_atomic(&self, path: &str, content: &str) -> Result<(), String> {
        // 复用 write_bytes_atomic（text → bytes → base64 → stdin 管道）
        self.write_bytes_atomic(path, content.as_bytes()).await
    }
}

/// 通过 `docker ps` 列出容器（id + 名称），供前端选择。
pub async fn list_docker_containers(
    channel: &Handle<RemoteHandler>,
) -> Result<Vec<String>, String> {
    // --format 输出 "id\tname"；name 取最后一个（--format '{{.Names}}' 已是逗号分隔，取第一个即可）
    let out = exec_command(channel, "docker ps --format '{{.ID}} {{.Names}}'").await?;
    let mut containers = Vec::new();
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(_id), Some(names)) = (parts.next(), parts.next()) {
            // Names 可能是逗号分隔的多个；取第一个作为容器名
            let name = names.split(',').next().unwrap_or("").to_string();
            if !name.is_empty() {
                containers.push(name);
            }
        }
    }
    Ok(containers)
}
