//! 增强版自有的数据库结构（建表 / 补列）——**P2 收敛点**。
//!
//! 为什么独立成模块：官方的 `database/schema.rs` 是**高频改动文件**
//! （近 400 个官方提交里被改了 30+ 次，几乎每次发版都动）。
//! 我们把「增强版需要的表 / 列」全部收敛到这里，官方 `create_tables_on_conn`
//! 里只留**一行调用**，从而把升级官方版本时的冲突面从「~90 行」压到「1 行」。
//!
//! 调用点：`Database::create_tables_on_conn` 末尾（database/schema.rs）。
//!
//! 幂等保证：`CREATE TABLE IF NOT EXISTS` + 列存在则跳过，可重复调用。

use rusqlite::Connection;

use crate::error::AppError;

/// 幂等：确保增强版所需的表与列存在。
pub fn ensure(conn: &Connection) -> Result<(), AppError> {
    // ---- 1) 官方表上的增强列：OpenClaw 支持 ----
    add_column_if_missing(conn, "skills", "enabled_openclaw", "BOOLEAN NOT NULL DEFAULT 0")?;
    add_column_if_missing(
        conn,
        "mcp_servers",
        "enabled_openclaw",
        "BOOLEAN NOT NULL DEFAULT 0",
    )?;

    // ---- 2) 增强版自有表 ----
    // Remote Hosts 表（SSH 远程主机管理）。密码不落库，存系统凭据库（见 remote::credentials）。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS remote_hosts (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            host TEXT NOT NULL,
            port INTEGER NOT NULL DEFAULT 22,
            username TEXT NOT NULL,
            auth_method TEXT NOT NULL DEFAULT 'password',
            save_password BOOLEAN NOT NULL DEFAULT 1,
            route_through_local_proxy BOOLEAN NOT NULL DEFAULT 0,
            route_proxy_apps TEXT NOT NULL DEFAULT '{}',
            route_proxy_container_apps TEXT NOT NULL DEFAULT '{}',
            disabled BOOLEAN NOT NULL DEFAULT 0,
            created_at INTEGER,
            updated_at INTEGER
        )",
        [],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;

    // 远程主机「当前生效供应商」记录（per-app），含完整 provider 配置 JSON（解耦路由基座）。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS remote_current_providers (
            host_id TEXT NOT NULL,
            app TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            provider_config TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (host_id, app)
        )",
        [],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;

    // ---- 3) 旧库升级：补列 + 数据迁移（幂等）----
    // 列已存在时 ALTER 会报错，用 `let _` 吞掉；新库建表已含全部列，对它是空操作。
    let _ = conn.execute(
        "ALTER TABLE remote_hosts ADD COLUMN route_through_local_proxy BOOLEAN NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE remote_hosts ADD COLUMN route_proxy_apps TEXT NOT NULL DEFAULT '{}'",
        [],
    );
    // 旧数据迁移：route_through_local_proxy=1 的主机 → 4 个接管 app 全开（保留原意图）
    let _ = conn.execute(
        "UPDATE remote_hosts SET route_proxy_apps = '{\"claude\":true,\"codex\":true,\"gemini\":true,\"grokbuild\":true}' WHERE route_through_local_proxy = 1 AND route_proxy_apps = '{}'",
        [],
    );
    // 展开后清零旧布尔：route_through_local_proxy 已废弃（per-app 全覆盖），
    // 不清会残留 1，导致 host_wants_tunnel 误判「需要隧道」（旧库升级场景）。
    let _ = conn.execute(
        "UPDATE remote_hosts SET route_through_local_proxy = 0 WHERE route_through_local_proxy = 1",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE remote_hosts ADD COLUMN route_proxy_container_apps TEXT NOT NULL DEFAULT '{}'",
        [],
    );
    // disabled 列（软禁用）：旧库升级补列，新库建表已含。
    let _ = conn.execute(
        "ALTER TABLE remote_hosts ADD COLUMN disabled BOOLEAN NOT NULL DEFAULT 0",
        [],
    );

    Ok(())
}

/// 幂等补列：**表不存在或列已存在时静默跳过**。
///
/// 比官方版本更宽容 —— 增强版可能在官方建表之前的时机被调用，
/// 不希望因为表尚未创建而报错（官方版本此处会 Err）。
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), AppError> {
    if !table_exists(conn, table)? || has_column(conn, table, column)? {
        return Ok(());
    }
    let sql = format!("ALTER TABLE \"{table}\" ADD COLUMN \"{column}\" {definition};");
    conn.execute(&sql, [])
        .map_err(|e| AppError::Database(format!("为表 {table} 添加列 {column} 失败: {e}")))?;
    log::info!("[magic] 已为表 {table} 添加缺失列 {column}");
    Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, AppError> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |r| r.get(0),
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(n > 0)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, AppError> {
    let sql = format!("PRAGMA table_info(\"{table}\")");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::Database(e.to_string()))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| AppError::Database(e.to_string()))?;
    while let Some(row) = rows.next().map_err(|e| AppError::Database(e.to_string()))? {
        let name: String = row.get(1).map_err(|e| AppError::Database(e.to_string()))?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}
