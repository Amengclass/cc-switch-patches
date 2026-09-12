//! CC Switch 远端会话读取 helper。
//!
//! 目的：远端 hermes / opencode 的会话主存储在 SQLite（`~/.hermes/state.db`、
//! `~/.local/share/opencode/opencode.db`），但远端不一定有 `sqlite3` CLI。
//! 本 helper 静态编译（musl + bundled SQLite，单文件零系统依赖），经 SSH 部署到
//! 远端后执行**白名单 SQL**（SQL 由 cc-switch 主程序硬编码，非用户输入），
//! 结果以 JSON 输出到 stdout，回传给本机。全程不下载 db 文件。
//!
//! 用法：
//! ```text
//! sqlite-helper query <db> "<sql>" [param...]
//!     # 只读执行（SQLITE_OPEN_READ_ONLY），输出 {"ok":true,"rows":[{col:value},...]}
//! sqlite-helper write <db> "<sql1>\n<sql2>\n..." [param...]
//!     # 读写 + 单事务执行多条 SQL（按行拆分，每行独立 prepare，参数应用到每条）
//!     # 用于删除会话（对齐本机 provider 的 DELETE 序列）
//! ```
//! 成功输出 `{"ok":true,...}` 到 stdout、exit 0；失败输出 `{"error":"..."}`、exit 1。

use rusqlite::{params_from_iter, Connection, OpenFlags};
use serde_json::{json, Value};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("{}", json!({ "error": e }));
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<String, String> {
    if args.len() < 4 {
        return Err(
            "usage: sqlite-helper <query|write> <db> \"<sql-lines>\" [param...]".to_string(),
        );
    }
    let cmd = args[1].as_str();
    let db = &args[2];
    let sql_lines = &args[3];
    let params: Vec<&str> = args[4..].iter().map(String::as_str).collect();

    let mut flags = OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if cmd == "query" || cmd == "query-all" {
        flags |= OpenFlags::SQLITE_OPEN_READ_ONLY;
    } else if cmd == "write" {
        flags |= OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    } else {
        return Err(format!("unknown command: {cmd}"));
    }

    let mut conn = Connection::open_with_flags(db, flags)
        .map_err(|e| format!("open db {db} failed: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3)).ok();

    match cmd {
        "query" => {
            let rows = run_query(&mut conn, sql_lines, &params)?;
            Ok(json!({ "ok": true, "rows": rows }).to_string())
        }
        "query-all" => {
            // 多条 SQL（按 `;` 拆分），逐条执行，返回 rowsets（opencode 消息需 join 两表）
            let mut rowsets = Vec::new();
            for raw in sql_lines.split(';') {
                let stmt_sql = raw.trim();
                if stmt_sql.is_empty() {
                    continue;
                }
                rowsets.push(run_query(&mut conn, stmt_sql, &params)?);
            }
            Ok(json!({ "ok": true, "rowsets": rowsets }).to_string())
        }
        "write" => {
            let tx = conn.transaction().map_err(|e| format!("tx failed: {e}"))?;
            // 按 `;` 拆分多语句（rusqlite prepare 只编译单条；本 helper 的写 SQL
            // 均为简单 DELETE，不含字面分号）
            for raw in sql_lines.split(';') {
                let stmt_sql = raw.trim();
                if stmt_sql.is_empty() {
                    continue;
                }
                tx.execute(stmt_sql, params_from_iter(params.iter()))
                    .map_err(|e| format!("exec {stmt_sql} failed: {e}"))?;
            }
            tx.commit().map_err(|e| format!("commit failed: {e}"))?;
            Ok(json!({ "ok": true }).to_string())
        }
        _ => unreachable!(),
    }
}

fn run_query(
    conn: &mut Connection,
    sql: &str,
    params: &[&str],
) -> Result<Vec<Value>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("prepare failed: {e}"))?;
    let col_count = stmt.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| stmt.column_name(i).unwrap_or("").to_string())
        .collect();
    let rows = stmt
        .query_map(params_from_iter(params.iter()), |row| {
            let mut obj = serde_json::Map::new();
            for i in 0..col_count {
                let v = row.get_ref(i).map_err(|e| e)?;
                obj.insert(col_names[i].clone(), value_to_json(v)?);
            }
            Ok(json!(obj))
        })
        .map_err(|e| format!("query failed: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("row failed: {e}"))?;
    Ok(rows)
}

fn value_to_json(v: rusqlite::types::ValueRef) -> Result<Value, rusqlite::Error> {    Ok(match v {
        rusqlite::types::ValueRef::Null => Value::Null,
        rusqlite::types::ValueRef::Integer(i) => json!(i),
        rusqlite::types::ValueRef::Real(f) => json!(f),
        rusqlite::types::ValueRef::Text(t) => {
            Value::String(String::from_utf8_lossy(t).to_string())
        }
        rusqlite::types::ValueRef::Blob(b) => {
            Value::String(format!("<blob {} bytes>", b.len()))
        }
    })
}
