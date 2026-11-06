//! 通过 SFTP 对远端文件进行读写;写回采用「临时文件 + rename」保证原子性。
//!
//! 基于 russh-sftp 2.3 的高层 API(read/write/try_exists/rename/remove_file)。

use russh_sftp::client::SftpSession;
use tokio::io::AsyncWriteExt;

/// 读取远端文本文件(UTF-8)。文件不存在或非 UTF-8 时返回 Err。
pub async fn read_remote_text(sftp: &SftpSession, path: &str) -> Result<String, String> {
    let data = sftp
        .read(path)
        .await
        .map_err(|e| format!("远端读取失败 {path}: {e}"))?;
    String::from_utf8(data).map_err(|e| format!("远端文件不是 UTF-8: {path}: {e}"))
}

/// 读取远端文件;文件不存在时返回 None(settings.json 可能缺失的场景)。
pub async fn read_remote_text_optional(
    sftp: &SftpSession,
    path: &str,
) -> Result<Option<String>, String> {
    match sftp.try_exists(path).await {
        Ok(true) => read_remote_text(sftp, path).await.map(Some),
        Ok(false) => Ok(None),
        Err(e) => Err(format!("远端检查文件失败 {path}: {e}")),
    }
}

/// 确保远端目录存在（逐级创建）。用于 settings.json 的父目录可能尚未生成
/// （服务器还没用过 Claude Code，~/.claude 不存在）的场景。
pub async fn ensure_remote_dir(sftp: &SftpSession, dir: &str) -> Result<(), String> {
    let mut current = String::new();
    for part in dir.split('/') {
        if part.is_empty() {
            continue;
        }
        if current.is_empty() {
            current = format!("/{part}");
        } else {
            current = format!("{current}/{part}");
        }
        match sftp.try_exists(&current).await {
            Ok(true) => {}
            Ok(false) => sftp
                .create_dir(&current)
                .await
                .map_err(|e| format!("创建远端目录失败 {current}: {e}"))?,
            Err(e) => return Err(format!("检查远端目录失败 {current}: {e}")),
        }
    }
    Ok(())
}

/// 读取远端文件的原始字节;文件不存在返回 None。
///
/// 给官方 v4.0.4 的纯改写器用:`LivePatch::apply_file(path, pre: Option<&[u8]>)`
/// 吃「原字节」吐「新字节」,不碰 I/O —— 远端只要把字节喂进去、把结果写回来,
/// 「写什么」的逻辑就与本机逐字节同源(同一个投影器 + 同一个补丁器)。
pub async fn read_remote_bytes_optional(
    sftp: &SftpSession,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    match sftp.read(path).await {
        Ok(data) => Ok(Some(data)),
        Err(read_err) => match sftp.try_exists(path).await {
            // 能 stat 到却读不出来 = 真读失败,不能误报「文件不存在」
            Ok(true) => Err(format!("远端读取失败 {path}: {read_err}")),
            Ok(false) => Ok(None),
            Err(e) => Err(format!("远端检查文件失败 {path}: {e}")),
        },
    }
}

/// 删除远端文件;文件本就不存在时视为成功。
///
/// 官方纯改写器返回 `None` 就表示「删掉这个文件」(如 Codex 切到第三方时删
/// `auth.json`),所以要有一条删除路径。
pub async fn remove_remote_file(sftp: &SftpSession, path: &str) -> Result<(), String> {
    match sftp.remove_file(path).await {
        Ok(()) => Ok(()),
        Err(remove_err) => match sftp.try_exists(path).await {
            Ok(false) => Ok(()),
            _ => Err(format!("远端删除失败 {path}: {remove_err}")),
        },
    }
}

/// 原子写回字节(`write_remote_text_atomic` 的字节版:官方改写器产出的是 `Vec<u8>`,
/// 不做 UTF-8 往返,避免边缘字节被改)。写前自动确保父目录存在。
pub async fn write_remote_bytes_atomic(
    sftp: &SftpSession,
    path: &str,
    content: &[u8],
) -> Result<(), String> {
    let tmp = format!("{path}.ccswitch.tmp");
    if let Some(parent) = path
        .rsplit_once('/')
        .map(|(d, _)| d)
        .filter(|d| !d.is_empty())
    {
        ensure_remote_dir(sftp, parent).await?;
    }
    let mut file = sftp
        .create(&tmp)
        .await
        .map_err(|e| format!("远端创建临时文件失败 {tmp}: {e}"))?;
    file.write_all(content)
        .await
        .map_err(|e| format!("远端写入临时文件失败 {tmp}: {e}"))?;
    file.flush()
        .await
        .map_err(|e| format!("远端刷新临时文件失败 {tmp}: {e}"))?;
    if sftp.try_exists(path).await.unwrap_or(false) {
        sftp.remove_file(path)
            .await
            .map_err(|e| format!("远端删除旧文件失败 {path}: {e}"))?;
    }
    if let Err(e) = sftp.rename(&tmp, path).await {
        let _ = sftp.remove_file(&tmp).await;
        return Err(format!("远端原子替换失败 {path}: {e}"));
    }
    Ok(())
}

/// 原子写回:先写临时文件,再 rename 覆盖目标。中断不留半文件。
/// 写前自动确保父目录存在。
pub async fn write_remote_text_atomic(
    sftp: &SftpSession,
    path: &str,
    content: &str,
) -> Result<(), String> {
    let tmp = format!("{path}.ccswitch.tmp");
    if let Some(parent) = path
        .rsplit_once('/')
        .map(|(d, _)| d)
        .filter(|d| !d.is_empty())
    {
        ensure_remote_dir(sftp, parent).await?;
    }
    // 注意：不能用 `sftp.write()`（内部只有 WRITE 标志，文件不存在会报 NoSuchFile），
    // 必须用 `create()`（CREATE|TRUNCATE|WRITE）创建并写入。
    let mut file = sftp
        .create(&tmp)
        .await
        .map_err(|e| format!("远端创建临时文件失败 {tmp}: {e}"))?;
    file.write_all(content.as_bytes())
        .await
        .map_err(|e| format!("远端写入临时文件失败 {tmp}: {e}"))?;
    file.flush()
        .await
        .map_err(|e| format!("远端刷新临时文件失败 {tmp}: {e}"))?;
    // OpenSSH 的 SFTP rename 不带覆盖标志时不允许覆盖已存在文件，
    // 且 russh-sftp 的 Rename 包无法携带 RENAME_OVERWRITE 标志。
    // 因此目标存在时先删除再 rename（内容仍是原子替换，仅目标短暂缺失；有 .bak 兜底）。
    if sftp.try_exists(path).await.unwrap_or(false) {
        sftp.remove_file(path)
            .await
            .map_err(|e| format!("远端删除旧文件失败 {path}: {e}"))?;
    }
    if let Err(e) = sftp.rename(&tmp, path).await {
        let _ = sftp.remove_file(&tmp).await;
        return Err(format!("远端原子替换失败 {path}: {e}"));
    }
    Ok(())
}
