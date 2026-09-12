//! 远端接管路由标记（proxy 侧解析 + remote 侧写入共用）。
//!
//! 解耦目标：远端 app 经反向隧道发到本机代理的请求，必须能区分「来自哪台远端主机」，
//! 才能按**该远端自己**的当前供应商路由，而不是依赖本机当前供应商。
//!
//! 实现：远端接管写入 live 时，把占位 token 写成 `PROXY_MANAGED:<host_id>`
//! （如 `ANTHROPIC_AUTH_TOKEN=PROXY_MANAGED:9b2a0411-...`）。远端 app 原样把该 token
//! 放进 auth 头发出（auth 头对 HTTP 客户端是透明字节，绝不改写），本机代理从 auth 头
//! 识别 host_id，按 `remote_current_providers.provider_config` 路由到该远端自己的当前
//! 供应商；找不到/解析失败回退本机路由（升级前旧行为，优雅降级）。
//!
//! 为什么不用 base_url 路径标记：远端 app（尤其 Claude Code 用的 Anthropic SDK）对
//! base_url 的路径前缀处理不保证保留——按 origin 重建 URL 的客户端会丢掉 `/ccr-<id>`，
//! 标记直接失效。token 标记经 auth 头传递，客户端从不解析/改写 auth 值，100% 可靠。
//!
//! 局限：codex 官方接管不写 bearer token（走原生 ChatGPT 登录），token 标记对它不适用；
//! 该场景本就不经密钥路由，维持旧行为（本机路由）。
use http::HeaderMap;

/// 占位 token 前缀：`PROXY_MANAGED:<host_id>`。沿用既有 `PROXY_MANAGED` 占位符
/// （本机代理识别并替换为真实密钥），加 `:<host_id>` 后缀识别来源主机。
/// 本地接管（非远端）仍写裸 `PROXY_MANAGED`（无冒号），不会被误判为远端。
pub const REMOTE_TOKEN_MARKER: &str = "PROXY_MANAGED:";
const BEARER_PREFIXES: [&str; 2] = ["Bearer ", "bearer "];

/// 生成带主机标记的占位 token。
pub fn remote_token_for(host_id: &str) -> String {
    format!("{REMOTE_TOKEN_MARKER}{host_id}")
}

/// 从请求 auth 头中识别来源主机 id（无标记返回 None）。
/// 扫描 `authorization` / `x-api-key` / `x-goog-api-key` / `api-key`，值形如
/// `Bearer PROXY_MANAGED:<host_id>` 或 `PROXY_MANAGED:<host_id>`。
pub fn detect_host_id_from_headers(headers: &HeaderMap) -> Option<String> {
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
                return Some(rest.to_string());
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
    fn detects_bearer_token() {
        let mut h = HeaderMap::new();
        h.insert(
            "authorization",
            "Bearer PROXY_MANAGED:9b2a0411-abc".parse().unwrap(),
        );
        assert_eq!(
            detect_host_id_from_headers(&h),
            Some("9b2a0411-abc".to_string())
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
            detect_host_id_from_headers(&h),
            Some("9b2a0411-abc".to_string())
        );
    }

    #[test]
    fn ignores_plain_placeholder() {
        // 本机接管（非远端）占位 token 无冒号 → 不识别为远端
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer PROXY_MANAGED".parse().unwrap());
        assert_eq!(detect_host_id_from_headers(&h), None);
    }

    #[test]
    fn ignores_real_key() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer sk-real-123".parse().unwrap());
        assert_eq!(detect_host_id_from_headers(&h), None);
    }

    #[test]
    fn token_format() {
        assert_eq!(remote_token_for("h1"), "PROXY_MANAGED:h1");
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
