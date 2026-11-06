//! 远端接管路由标记（proxy 侧解析 + remote 侧写入共用）。
//!
//! 解耦目标：远端 app 经反向隧道发到本机代理的请求，必须能区分「来自哪个远端目标器」，
//! 才能按**该目标器自己**的当前供应商路由，而不是依赖本机当前供应商 / 别的容器。
//!
//! **方案乙（2026-10-12）** 路由身份升级 = 目标器 `(host_id, container)`：
//! - 宿主机：`PROXY_MANAGED:<host_id>`
//! - 容器：`PROXY_MANAGED:<host_id>:<容器名>`
//!
//! 各目标器有自己的供应商 SSOT / 面板，`remote_current_providers` 按目标器一行，
//! 路由据此区分宿主机与它的每个容器（不再共享 provider_config，不再串台）。
//!
//! 实现：远端接管写入 live 时，把占位 token 写成上述格式。远端 app 原样把该 token
//! 放进 auth 头发出（auth 头对 HTTP 客户端是透明字节，绝不改写），本机代理从 auth 头
//! 识别 (host_id, container)，按 `remote_current_providers.provider_config` 路由到该
//! 目标器自己的当前供应商；找不到/解析失败**不回退本机路由**（远端请求路由到本机
//! 供应商是错的），明确告警走错误路径。
//!
//! 为什么不用 base_url 路径标记：远端 app（尤其 Claude Code 用的 Anthropic SDK）对
//! base_url 的路径前缀处理不保证保留——按 origin 重建 URL 的客户端会丢掉 `/ccr-<id>`，
//! 标记直接失效。token 标记经 auth 头传递，客户端从不解析/改写 auth 值，100% 可靠。
//!
//! 局限：codex 官方接管不写 bearer token（走原生 ChatGPT 登录），token 标记对它不适用；
//! 该场景本就不经密钥路由，维持旧行为（本机路由）。
use http::HeaderMap;

/// 占位 token 前缀：`PROXY_MANAGED:<host_id>`。沿用既有 `PROXY_MANAGED` 占位符
/// （本机代理识别并替换为真实密钥），加 `:<host_id>` 后缀识别来源主机，
/// `:<host_id>:<容器名>` 进一步识别来源容器。宿主机不带容器段（兼容旧格式）。
/// 本地接管（非远端）仍写裸 `PROXY_MANAGED`（无冒号），不会被误判为远端。
pub const REMOTE_TOKEN_MARKER: &str = "PROXY_MANAGED:";
const BEARER_PREFIXES: [&str; 2] = ["Bearer ", "bearer "];

/// 远端目标器来源（host_id + container，容器空串 = 宿主机）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSource {
    pub host_id: String,
    pub container: String,
}

/// 生成带目标器标记的占位 token。
pub fn remote_token_for(host_id: &str, container: Option<&str>) -> String {
    match container.filter(|c| !c.is_empty()) {
        Some(c) => format!("{REMOTE_TOKEN_MARKER}{host_id}:{c}"),
        None => format!("{REMOTE_TOKEN_MARKER}{host_id}"),
    }
}

/// 从请求 auth 头中识别来源目标器（无标记返回 None）。
/// 扫描 `authorization` / `x-api-key` / `x-goog-api-key` / `api-key`，值形如
/// `Bearer PROXY_MANAGED:<host_id>` 或 `PROXY_MANAGED:<host_id>:<container>`。
/// 历史格式（只有 host_id）解析为宿主机（container=''）——兼容。
pub fn detect_remote_source_from_headers(headers: &HeaderMap) -> Option<RemoteSource> {
    let raw_value = detect_token_value(headers)?;
    let mut parts = raw_value.splitn(2, ':');
    let host_id = parts.next()?.to_string();
    if host_id.is_empty() {
        return None;
    }
    let container = parts.next().unwrap_or("").to_string();
    Some(RemoteSource { host_id, container })
}

/// 取 auth 头里裸标记值（去 Bearer 前缀、去 REMOTE_TOKEN_MARKER 前缀）。
fn detect_token_value(headers: &HeaderMap) -> Option<&str> {
    for (name, value) in headers.iter() {
        let name = name.as_str();
        if !matches!(
            name,
            "authorization" | "x-api-key" | "x-goog-api-key" | "api-key"
        ) {
            continue;
        }
        let Ok(raw) = value.to_str() else {
            continue;
        };
        let value = raw.trim();
        let value = BEARER_PREFIXES
            .iter()
            .find_map(|p| value.strip_prefix(p))
            .unwrap_or(value)
            .trim();
        if let Some(rest) = value.strip_prefix(REMOTE_TOKEN_MARKER) {
            if !rest.is_empty() {
                return Some(rest);
            }
        }
    }
    None
}

/// 判断某 provider 的 `settings_config` 是否为远端接管/路由写的**占位配置**
/// （token 含 `PROXY_MANAGED` 占位符，base_url 指向隧道）。这类配置不是真实
/// 供应商配置，不应导入 SSOT / 展示为「当前机器配置」。
/// 真实供应商的 token 不会出现该占位串，子串匹配对 claude/codex/gemini/grok 均有效。
pub fn is_route_placeholder_settings(settings: &serde_json::Value) -> bool {
    settings.to_string().contains("PROXY_MANAGED")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bearer_token_host() {
        let mut h = HeaderMap::new();
        h.insert(
            "authorization",
            "Bearer PROXY_MANAGED:9b2a0411-abc".parse().unwrap(),
        );
        assert_eq!(
            detect_remote_source_from_headers(&h),
            Some(RemoteSource {
                host_id: "9b2a0411-abc".to_string(),
                container: String::new(),
            })
        );
    }

    #[test]
    fn detects_container_token() {
        let mut h = HeaderMap::new();
        h.insert(
            "authorization",
            "Bearer PROXY_MANAGED:9b2a0411-abc:my_cont".parse().unwrap(),
        );
        assert_eq!(
            detect_remote_source_from_headers(&h),
            Some(RemoteSource {
                host_id: "9b2a0411-abc".to_string(),
                container: "my_cont".to_string(),
            })
        );
    }

    #[test]
    fn detects_bare_token() {
        let mut h = HeaderMap::new();
        h.insert(
            "x-goog-api-key",
            "PROXY_MANAGED:9b2a0411-abc".parse().unwrap(),
        );
        assert_eq!(
            detect_remote_source_from_headers(&h),
            Some(RemoteSource {
                host_id: "9b2a0411-abc".to_string(),
                container: String::new(),
            })
        );
    }

    #[test]
    fn ignores_plain_placeholder() {
        // 本机接管（非远端）占位 token 无冒号 → 不识别为远端
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer PROXY_MANAGED".parse().unwrap());
        assert_eq!(detect_remote_source_from_headers(&h), None);
    }

    #[test]
    fn ignores_real_key() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer sk-real-123".parse().unwrap());
        assert_eq!(detect_remote_source_from_headers(&h), None);
    }

    #[test]
    fn token_format() {
        assert_eq!(remote_token_for("h1", None), "PROXY_MANAGED:h1");
        assert_eq!(
            remote_token_for("h1", Some("c1")),
            "PROXY_MANAGED:h1:c1"
        );
    }

    #[test]
    fn detects_route_placeholder_settings() {
        // 路由/接管写的占位配置（隧道 base_url + PROXY_MANAGED token）→ true
        let routing = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:15721",
                "ANTHROPIC_AUTH_TOKEN": "PROXY_MANAGED:9b2a0411-abc",
            }
        });
        assert!(is_route_placeholder_settings(&routing));

        // 本地接管占位（裸 PROXY_MANAGED）同样识别
        let local = serde_json::json!({ "env": { "ANTHROPIC_AUTH_TOKEN": "PROXY_MANAGED" } });
        assert!(is_route_placeholder_settings(&local));

        // 真实供应商配置 → false
        let real = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
                "ANTHROPIC_AUTH_TOKEN": "sk-abc123",
            }
        });
        assert!(!is_route_placeholder_settings(&real));
    }
}
