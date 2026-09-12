//! 远程主机「当前生效供应商」的持久化（per-app）。
//!
//! 原生 cc switch 判断「当前供应商」用的是持久化的 settings（`get_effective_current_provider`
//! 读 DB / 配置），而不是靠比对 base_url。远程侧为保持同一语义，在每次
//! `switch_remote_provider` 成功时记录 `host_id -> { app -> provider_id }` 到
//! SQLite 表 `remote_current_providers`（原为 `~/.cc-switch/remote_current_providers.json`，
//! 已迁入 DB）。
//!
//! 除 provider_id 外还存完整 provider 配置 JSON（`provider_config`），供本机代理
//! 按远端路由（解耦基座）使用。旧 json 数据首次调用时自动迁入 DB（一次性），
//! json 文件改名 `.bak` 保留兜底。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::database::Database;
use crate::provider::Provider;

static MIGRATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static MIGRATED: AtomicBool = AtomicBool::new(false);

/// 确保旧 json 已迁入 DB（进程内只执行一次；失败可重试）。
pub fn ensure_migrated(db: &Database) -> Result<(), String> {
    if MIGRATED.load(Ordering::Relaxed) {
        return Ok(());
    }
    let _guard = MIGRATE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if MIGRATED.load(Ordering::Relaxed) {
        return Ok(());
    }
    migrate_from_json(db)?;
    MIGRATED.store(true, Ordering::Relaxed);
    Ok(())
}

fn store_path() -> Result<PathBuf, String> {
    Ok(crate::config::get_app_config_dir().join("remote_current_providers.json"))
}

/// 记录某主机的当前生效供应商（per-app）。
/// `provider` 为完整 Provider（可空）：序列化后存入 `provider_config` 供代理路由。
pub fn save_current_provider(
    db: &Database,
    host_id: &str,
    app: &str,
    provider_id: &str,
    provider: Option<&Provider>,
) -> Result<(), String> {
    ensure_migrated(db)?;
    let config_json = provider
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| format!("序列化供应商配置失败: {e}"))?;
    db.save_remote_current_provider(host_id, app, provider_id, config_json.as_deref())
        .map_err(|e| e.to_string())
}

/// 读取某主机、某 app 的当前生效供应商；未记录时返回 None。
pub fn get_current_provider(
    db: &Database,
    host_id: &str,
    app: &str,
) -> Result<Option<String>, String> {
    ensure_migrated(db)?;
    db.get_remote_current_provider(host_id, app)
        .map_err(|e| e.to_string())
}

/// 读取某主机、某 app 的完整 provider 配置 JSON（解耦路由用；未记录/未存返回 None）。
pub fn get_current_provider_config(
    db: &Database,
    host_id: &str,
    app: &str,
) -> Result<Option<String>, String> {
    ensure_migrated(db)?;
    db.get_remote_current_provider_config(host_id, app)
        .map_err(|e| e.to_string())
}

/// 删除某主机的全部记录（删除主机时清理）。
pub fn delete_current_provider(db: &Database, host_id: &str) -> Result<(), String> {
    ensure_migrated(db)?;
    db.delete_remote_current_provider(host_id)
        .map_err(|e| e.to_string())
}

/// 删除某主机、某 app 的当前供应商记录（删除/移除供应商时清理）。
pub fn delete_current_provider_for_app(
    db: &Database,
    host_id: &str,
    app: &str,
) -> Result<(), String> {
    ensure_migrated(db)?;
    db.delete_remote_current_provider_for_app(host_id, app)
        .map_err(|e| e.to_string())
}

/// 把旧 json 文件迁进 DB（一次性）。迁移成功后把 json 改名为 `.bak` 保留兜底。
fn migrate_from_json(db: &Database) -> Result<(), String> {
    let path = store_path()?;
    if !path.exists() {
        return Ok(());
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("读取旧当前供应商文件失败: {e}"))?;
    let raw: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析旧当前供应商文件失败: {e}"))?;

    // 兼容旧格式：host -> "provider_id"（历史版本只有 claude 远程）
    if let Some(obj) = raw.as_object() {
        if obj.values().all(|v| v.is_string()) {
            for (host, v) in obj {
                if let Some(id) = v.as_str() {
                    db.save_remote_current_provider(host, "claude", id, None)
                        .map_err(|e| e.to_string())?;
                }
            }
            rename_to_bak(&path);
            return Ok(());
        }
    }

    let map: HashMap<String, HashMap<String, String>> =
        serde_json::from_value(raw).map_err(|e| format!("解析旧当前供应商文件失败: {e}"))?;
    for (host, apps) in map {
        for (app, id) in apps {
            db.save_remote_current_provider(&host, &app, &id, None)
                .map_err(|e| e.to_string())?;
        }
    }
    rename_to_bak(&path);
    Ok(())
}

fn rename_to_bak(path: &std::path::Path) {
    let bak = path.with_extension("json.bak");
    let _ = std::fs::rename(path, bak);
}
