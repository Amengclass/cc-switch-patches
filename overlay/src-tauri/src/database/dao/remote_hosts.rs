//! 远程主机(SSH)数据访问对象
//!
//! remote_hosts 表存放连接所需的非敏感信息;密码存系统钥匙串
//! (见 `remote::credentials`),不落库。

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::remote::{AuthMethod, RemoteHost};
use rusqlite::params;

impl Database {
    /// 获取全部远程主机(按创建时间排序)
    pub fn list_remote_hosts(&self) -> Result<Vec<RemoteHost>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, host, port, username, auth_method, save_password, route_through_local_proxy, route_proxy_apps, route_proxy_container_apps, disabled, created_at, updated_at
                 FROM remote_hosts
                 ORDER BY created_at IS NULL, created_at, id",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], Self::map_remote_host_row)
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut hosts = Vec::new();
        for row in rows {
            hosts.push(row.map_err(|e| AppError::Database(e.to_string()))?);
        }
        Ok(hosts)
    }

    /// 获取单个远程主机
    pub fn get_remote_host(&self, id: &str) -> Result<Option<RemoteHost>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, host, port, username, auth_method, save_password, route_through_local_proxy, route_proxy_apps, route_proxy_container_apps, disabled, created_at, updated_at
                 FROM remote_hosts WHERE id = ?1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        match stmt.query_row(params![id], Self::map_remote_host_row) {
            Ok(host) => Ok(Some(host)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// 保存远程主机(插入或整行替换)
    pub fn upsert_remote_host(&self, host: &RemoteHost) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let auth = match host.auth_method {
            AuthMethod::Password => "password",
            AuthMethod::Key => "key",
        };
        conn.execute(
            "INSERT OR REPLACE INTO remote_hosts
             (id, name, host, port, username, auth_method, save_password, route_through_local_proxy, route_proxy_apps, route_proxy_container_apps, disabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                host.id,
                host.name,
                host.host,
                host.port,
                host.username,
                auth,
                host.save_password,
                host.route_through_local_proxy,
                serde_json::to_string(&host.route_proxy_apps)
                    .unwrap_or_else(|_| "{}".to_string()),
                serde_json::to_string(&host.route_proxy_container_apps)
                    .unwrap_or_else(|_| "{}".to_string()),
                host.disabled,
                host.created_at,
                host.updated_at,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 删除远程主机，返回是否实际删除
    pub fn delete_remote_host(&self, id: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute("DELETE FROM remote_hosts WHERE id = ?1", params![id])
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(affected > 0)
    }

    /// 行 → RemoteHost 映射
    fn map_remote_host_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RemoteHost> {
        let auth_str: String = row.get(5)?;
        Ok(RemoteHost {
            id: row.get(0)?,
            name: row.get(1)?,
            host: row.get(2)?,
            port: row.get(3)?,
            username: row.get(4)?,
            auth_method: if auth_str == "key" {
                AuthMethod::Key
            } else {
                AuthMethod::Password
            },
            save_password: row.get(6)?,
            route_through_local_proxy: row.get(7)?,
            route_proxy_apps: {
                let s: String = row.get(8)?;
                serde_json::from_str(&s).unwrap_or_default()
            },
            route_proxy_container_apps: {
                let s: String = row.get(9)?;
                serde_json::from_str(&s).unwrap_or_default()
            },
            disabled: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, name: &str, host: &str) -> RemoteHost {
        RemoteHost {
            id: id.to_string(),
            name: name.to_string(),
            host: host.to_string(),
            port: 22,
            username: "root".to_string(),
            auth_method: AuthMethod::Password,
            save_password: true,
            route_through_local_proxy: false,
            route_proxy_apps: std::collections::HashMap::new(),
            route_proxy_container_apps: std::collections::HashMap::new(),
            disabled: false,
            created_at: 1_000,
            updated_at: 1_000,
        }
    }

    #[test]
    fn test_remote_host_crud_roundtrip() -> Result<(), AppError> {
        let db = Database::memory()?;

        db.upsert_remote_host(&sample("a", "GPU 服务器", "10.0.0.1"))?;
        db.upsert_remote_host(&sample("b", "云主机", "example.com"))?;

        let all = db.list_remote_hosts()?;
        assert_eq!(all.len(), 2);

        let got = db.get_remote_host("a")?.expect("host a exists");
        assert_eq!(got.name, "GPU 服务器");
        assert_eq!(got.host, "10.0.0.1");
        assert_eq!(got.port, 22);
        assert_eq!(got.auth_method, AuthMethod::Password);

        // 更新
        let mut updated = sample("a", "GPU 服务器(新)", "10.0.0.2");
        updated.port = 2222;
        db.upsert_remote_host(&updated)?;
        let got = db.get_remote_host("a")?.expect("host a exists");
        assert_eq!(got.host, "10.0.0.2");
        assert_eq!(got.port, 2222);

        assert!(db.delete_remote_host("a")?);
        assert!(!db.delete_remote_host("a")?);
        assert!(db.get_remote_host("a")?.is_none());
        Ok(())
    }
}
