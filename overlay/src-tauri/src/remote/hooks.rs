//! 增强层在官方后端流程里的**挂钩点**（P2 收敛）。
//!
//! 为什么独立成模块：`services/proxy.rs` 是官方**高频改动文件**
//! （近 400 个官方提交里被改了 23 次）。把「远端接管意图」的判据与清理逻辑
//! 收进本模块，官方文件里只留**单行调用**，升级时的冲突面随之缩小。
//!
//! 调用点：`services/proxy.rs` 的停代理判据 / 全部恢复流程。

use crate::database::Database;

/// 是否还有任何远端主机 / 容器处于「远端接管」意图
/// （`route_proxy_apps` 或 `route_proxy_container_apps` 任一为 true）。
///
/// 用途：本机停代理前的判据。远端 live 指向的是本机代理进程，
/// 若还有远端在用却把进程停了，远端会指向死隧道。
pub fn has_route_intent(db: &Database) -> bool {
    db.list_remote_hosts()
        .map(|hosts| {
            hosts.iter().any(|h| {
                h.route_proxy_apps.values().any(|&v| v)
                    || h.route_proxy_container_apps
                        .values()
                        .any(|m| m.values().any(|&v| v))
            })
        })
        .unwrap_or(false)
}

/// 清除所有远端主机的「远端接管」意图（两个字段一起清空）。
///
/// 远端 live 的实际重写由命令层异步 fire-and-forget 完成；
/// 即使失败，下次切换时 `effective_route_proxy` 也会按「本机路由未运行」
/// 降级写直连自愈。
pub fn clear_route_intents(db: &Database) {
    match db.list_remote_hosts() {
        Ok(hosts) => {
            for mut h in hosts {
                if !h.route_proxy_apps.is_empty() || !h.route_proxy_container_apps.is_empty() {
                    h.route_proxy_apps.clear();
                    h.route_proxy_container_apps.clear();
                    h.updated_at = chrono::Utc::now().timestamp_millis();
                    if let Err(e) = db.upsert_remote_host(&h) {
                        log::warn!("[remote] 清除主机 {} 远端接管意图失败: {e}", h.name);
                    }
                }
            }
        }
        Err(e) => log::warn!("[remote] 读取远端主机列表失败（跳过意图清理）: {e}"),
    }
}
