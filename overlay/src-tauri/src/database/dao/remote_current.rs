//! 远程主机「当前生效供应商」数据访问对象。
//!
//! 原为 `~/.cc-switch/remote_current_providers.json`，迁入 SQLite 表
//! `remote_current_providers`（host_id, app → provider_id + 完整 provider 配置 JSON）。
//! `provider_config` 是完整 Provider 的序列化，供本机代理按远端路由时使用（解耦基座）。

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::params;

impl Database {
    /// 保存某主机某 app 的当前生效供应商（含完整 provider 配置 JSON，可为空）。
    pub fn save_remote_current_provider(
        &self,
        host_id: &str,
        app: &str,
        provider_id: &str,
        provider_config: Option<&str>,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO remote_current_providers (host_id, app, provider_id, provider_config, updated_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%s','now'))
             ON CONFLICT(host_id, app) DO UPDATE SET
               provider_id = excluded.provider_id,
               provider_config = excluded.provider_config,
               updated_at = excluded.updated_at",
            params![host_id, app, provider_id, provider_config],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 读取某主机某 app 的当前供应商 id；未记录返回 None。
    pub fn get_remote_current_provider(
        &self,
        host_id: &str,
        app: &str,
    ) -> Result<Option<String>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT provider_id FROM remote_current_providers WHERE host_id = ?1 AND app = ?2",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut rows = stmt
            .query_map(params![host_id, app], |row| row.get::<_, String>(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        if let Some(row) = rows.next() {
            Ok(Some(row.map_err(|e| AppError::Database(e.to_string()))?))
        } else {
            Ok(None)
        }
    }

    /// 读取某主机某 app 的完整 provider 配置 JSON（解耦路由用；未记录/未存返回 None）。
    pub fn get_remote_current_provider_config(
        &self,
        host_id: &str,
        app: &str,
    ) -> Result<Option<String>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT provider_config FROM remote_current_providers WHERE host_id = ?1 AND app = ?2",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut rows = stmt
            .query_map(params![host_id, app], |row| row.get::<_, Option<String>>(0))
            .map_err(|e| AppError::Database(e.to_string()))?;
        if let Some(row) = rows.next() {
            Ok(row.map_err(|e| AppError::Database(e.to_string()))?)
        } else {
            Ok(None)
        }
    }

    /// 删除某主机的全部记录（删除主机时清理）。
    pub fn delete_remote_current_provider(&self, host_id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM remote_current_providers WHERE host_id = ?1",
            params![host_id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除某主机某 app 的记录（删除/移除供应商时清理）。
    pub fn delete_remote_current_provider_for_app(
        &self,
        host_id: &str,
        app: &str,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM remote_current_providers WHERE host_id = ?1 AND app = ?2",
            params![host_id, app],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}
