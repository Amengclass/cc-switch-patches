//! SSH 远程主机管理模块。
//!
//! 让用户在本机 GUI 中连接 Linux 服务器,直接读写远端 `~/.claude/settings.json`,
//! 执行供应商切换(原子写回)、冲突环境变量清理,并明确提示切换生效方式。
//!
//! 里程碑:M1 骨架(类型 + 模块声明)→ M2 连接/读配置 → M3 切换写回 → M4 env 清理 → M5 远程会话。

pub mod codex;
pub mod commands;
pub mod connection;
#[cfg(target_os = "windows")]
pub mod credentials;
pub mod current;
pub mod docker;
pub mod effect;
pub mod env_clean;
pub mod gemini;
pub mod grok;
pub mod hermes;
pub mod mcp;
pub mod openclaw;
pub mod opencode;
pub mod pi;
pub mod prompt;
pub mod providers;
pub mod sessions;
pub mod settings;
pub mod sftp_io;
pub mod skill;

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 批量 toggle 的逐条结果,形状对齐前端 `SequentialBulkActionResult`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteBulkToggleResult {
    pub succeeded: Vec<String>,
    pub failed: Vec<RemoteBulkToggleFailure>,
}

/// 批量 toggle 中单条失败项。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteBulkToggleFailure {
    pub item: String,
    pub error: String,
}

/// 认证方式(M1 仅支持密码,密钥在二期实现)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    Password,
    Key,
}

/// 一台可管理的远程主机。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHost {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: AuthMethod,
    /// 是否把密码保存进系统钥匙串(用于下次一键连接)。
    pub save_password: bool,
    /// 走本机路由：切换时把远端 base_url 指向本机代理（经 SSH 反向隧道）。
    pub route_through_local_proxy: bool,
    /// per-app 远端接管开关（JSON：{"claude":true,"codex":false,...}）。
    /// 兼容字段：旧库只有 route_through_local_proxy 布尔，迁移时展开为全 app。
    #[serde(default = "default_route_proxy_apps")]
    pub route_proxy_apps: std::collections::HashMap<String, bool>,
    /// per-container × app 远端接管开关（JSON：{"<容器名>":{"claude":true,...}}）。
    /// 容器目标各自独立：容器 A 开了路由不影响容器 B / 宿主机目标。
    #[serde(default = "default_route_proxy_container_apps")]
    pub route_proxy_container_apps:
        std::collections::HashMap<String, std::collections::HashMap<String, bool>>,
    /// 是否被禁用（软禁用，不删除）：禁用的主机不可被目标选择/操作，远程管理页仍可见可恢复。
    #[serde(default)]
    pub disabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

fn default_route_proxy_apps() -> std::collections::HashMap<String, bool> {
    std::collections::HashMap::new()
}

fn default_route_proxy_container_apps(
) -> std::collections::HashMap<String, std::collections::HashMap<String, bool>> {
    std::collections::HashMap::new()
}

/// 已探测到的远端 `$HOME` 缓存（host_id → 真实路径）。
/// 由 `connection::connect_fresh` 每次新建连接时经 SSH `printf "$HOME"` 探测并写入；
/// `default_home()` 优先生效，未探测/失败时回退推导值。host 编辑/删除时失效
/// （`save_remote_host` / `delete_remote_host`），防旧账号的 `$HOME` 残留。
static REMOTE_HOME_CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// 记录某远端主机探测到的 `$HOME`。空 host_id / `temp` 一次性探测 / 空值忽略。
pub(crate) fn remember_probed_home(host_id: &str, home: &str) {
    if host_id.is_empty() || host_id == "temp" {
        return;
    }
    let home = home.trim();
    if home.is_empty() {
        return;
    }
    match REMOTE_HOME_CACHE.lock() {
        Ok(mut g) => {
            g.get_or_insert_with(HashMap::new)
                .insert(host_id.to_string(), home.to_string());
        }
        Err(p) => {
            p.into_inner()
                .get_or_insert_with(HashMap::new)
                .insert(host_id.to_string(), home.to_string());
        }
    }
}

/// 清除某主机的探测缓存（host 编辑/删除后调用）。
pub(crate) fn forget_probed_home(host_id: &str) {
    if let Ok(mut g) = REMOTE_HOME_CACHE.lock() {
        if let Some(m) = g.as_mut() {
            m.remove(host_id);
        }
    }
}

/// 该主机的探测缓存是否已存在（供连接复用路径判断是否需补一次探测）。
pub(crate) fn home_is_cached(host_id: &str) -> bool {
    match REMOTE_HOME_CACHE.lock() {
        Ok(g) => g.as_ref().and_then(|m| m.get(host_id)).is_some(),
        Err(p) => p
            .into_inner()
            .as_ref()
            .and_then(|m| m.get(host_id))
            .is_some(),
    }
}

/// 读取某远端主机探测到的 `$HOME`（None = 未探测 / 探测结果不可信）。
fn probed_home(host_id: &str) -> Option<String> {
    match REMOTE_HOME_CACHE.lock() {
        Ok(g) => g.as_ref().and_then(|m| m.get(host_id).cloned()),
        Err(p) => p
            .into_inner()
            .as_ref()
            .and_then(|m| m.get(host_id).cloned()),
    }
}

impl RemoteHost {
    /// 远端用户主目录：
    /// - 优先使用连接时经 SSH `$HOME` 探测的真实路径（connection.rs 每次新建连接
    ///   写入缓存；已落地原 TODO(M3)——非 root 账号的自定义 home 不再推导错）；
    /// - 未探测 / 探测失败时回退按用户名的推导（root→/root，其余→/home/<username>）。
    ///
    /// 语义说明：claude/codex/openclaw 等远端 CLI 都按运行时 `$HOME` 展开 `~/` 落盘
    /// （本模块写回的 `{home}/...` 就是镜像这一行为），所以探测 `$HOME` 与它们
    /// 实际使用的路径一致，比 `/home/<username>` 的经验假设更准。
    pub fn default_home(&self) -> String {
        if let Some(home) = probed_home(&self.id) {
            return home;
        }
        if self.username == "root" {
            "/root".to_string()
        } else {
            format!("/home/{}", self.username)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(id: &str, username: &str) -> RemoteHost {
        RemoteHost {
            id: id.to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: username.to_string(),
            auth_method: AuthMethod::Password,
            save_password: false,
            route_through_local_proxy: false,
            route_proxy_apps: HashMap::new(),
            route_proxy_container_apps: HashMap::new(),
            disabled: false,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn default_home_derives_when_not_probed() {
        // 未探测：按用户名推导（旧行为，仍是兜底）
        assert_eq!(host("dh-1", "ubuntu").default_home(), "/home/ubuntu");
        assert_eq!(host("dh-2", "root").default_home(), "/root");
        assert_eq!(host("dh-3", "deploy").default_home(), "/home/deploy");
    }

    #[test]
    fn default_home_prefers_probed_value_over_derivation() {
        // 探测到自定义 home → 优先生效，不再推导
        remember_probed_home("dh-4", "/data/users/ubuntu");
        assert_eq!(host("dh-4", "ubuntu").default_home(), "/data/users/ubuntu");

        // root 也一样
        remember_probed_home("dh-5", "/home/srv");
        assert_eq!(host("dh-5", "root").default_home(), "/home/srv");
    }

    #[test]
    fn forget_probed_home_restores_derivation() {
        remember_probed_home("dh-6", "/opt/user/x");
        assert_eq!(host("dh-6", "ubuntu").default_home(), "/opt/user/x");
        forget_probed_home("dh-6");
        // 失效后再查 → 回退推导（不会打到旧路径）
        assert_eq!(host("dh-6", "ubuntu").default_home(), "/home/ubuntu");
        assert!(!home_is_cached("dh-6"));
    }

    #[test]
    fn remember_ignores_empty_value() {
        // 空/纯空白探测值不应污染缓存
        remember_probed_home("dh-7", "   ");
        assert!(!home_is_cached("dh-7"));
        assert_eq!(host("dh-7", "ubuntu").default_home(), "/home/ubuntu");
    }
}
