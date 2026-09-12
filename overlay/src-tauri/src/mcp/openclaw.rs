//! OpenClaw MCP sync and import module
//!
//! Handles conversion between CC Switch unified MCP format and OpenClaw's
//! `openclaw.json` top-level `mcp.servers` (JSON5).
//!
//! ## Format mapping
//!
//! | CC Switch unified (JSON)              | OpenClaw `mcp.servers` (JSON5)     |
//! |---------------------------------------|------------------------------------|
//! | `{"type":"stdio","command":"npx","args":[...],"env":{}}` | `command: "npx", args: [...], env: {}` |
//! | `{"type":"sse"/"http","url":"...","headers":{}}` | `url: "...", transport: "sse"\|"streamable-http", headers: {}` |
//!
//! Key differences from Claude format:
//! - OpenClaw infers stdio (has `command`) vs HTTP (has `url`); `transport`
//!   selects the streamable-http / sse variant when a URL is present.
//! - OpenClaw allows `${ENV_VAR}` interpolation in headers/env — preserved
//!   verbatim on merge-on-write.

use serde_json::{Map, Value};
use std::collections::HashMap;

use crate::app_config::{McpApps, McpServer, MultiAppConfig};
use crate::error::AppError;
use crate::openclaw_config;

use super::validation::validate_server_spec;

/// Check if OpenClaw MCP sync should proceed (openclaw.json dir exists).
fn should_sync_openclaw_mcp() -> bool {
    openclaw_config::is_openclaw_mcp_available()
}

// ============================================================================
// Format Conversion: CC Switch -> OpenClaw
// ============================================================================

/// Convert CC Switch unified format to OpenClaw `mcp.servers` entry format.
///
/// Conversion rules:
/// - `stdio`: keep `command`/`args`/`env`; strip `type`.
/// - `sse`: output `url`/`headers` + `transport: "sse"`.
/// - `http`: output `url`/`headers` + `transport: "streamable-http"`.
pub fn convert_to_openclaw_format(spec: &Value) -> Result<Value, AppError> {
    let obj = spec
        .as_object()
        .ok_or_else(|| AppError::McpValidation("MCP spec must be a JSON object".into()))?;

    let typ = obj.get("type").and_then(|v| v.as_str()).unwrap_or("stdio");

    let mut result = Map::new();

    match typ {
        "stdio" => {
            if let Some(command) = obj.get("command") {
                result.insert("command".into(), command.clone());
            }
            if let Some(args) = obj.get("args") {
                if args.is_array() && !args.as_array().map(|a| a.is_empty()).unwrap_or(true) {
                    result.insert("args".into(), args.clone());
                }
            }
            if let Some(env) = obj.get("env") {
                if env.is_object() && !env.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    result.insert("env".into(), env.clone());
                }
            }
        }
        "sse" | "http" => {
            if let Some(url) = obj.get("url") {
                result.insert("url".into(), url.clone());
            }
            if let Some(headers) = obj.get("headers") {
                if headers.is_object() && !headers.as_object().map(|o| o.is_empty()).unwrap_or(true)
                {
                    result.insert("headers".into(), headers.clone());
                }
            }
            result.insert(
                "transport".into(),
                serde_json::json!(if typ == "sse" {
                    "sse"
                } else {
                    "streamable-http"
                }),
            );
        }
        _ => {
            return Err(AppError::McpValidation(format!("Unknown MCP type: {typ}")));
        }
    }

    Ok(Value::Object(result))
}

// ============================================================================
// Format Conversion: OpenClaw -> CC Switch
// ============================================================================

/// Convert an OpenClaw `mcp.servers` entry to CC Switch unified format.
///
/// Conversion rules:
/// - If `command` exists: `type: "stdio"`, extract `command`/`args`/`env`.
/// - If `url` exists: `type` from `transport` (`sse` vs `streamable-http`),
///   extract `url`/`headers`. No `transport` → `sse`.
pub fn convert_from_openclaw_format(id: &str, spec: &Value) -> Result<Value, AppError> {
    let obj = spec
        .as_object()
        .ok_or_else(|| AppError::McpValidation("OpenClaw MCP spec must be a JSON object".into()))?;

    let mut result = Map::new();

    if obj.contains_key("command") {
        // stdio type
        result.insert("type".into(), serde_json::json!("stdio"));

        if let Some(command) = obj.get("command") {
            result.insert("command".into(), command.clone());
        }
        if let Some(args) = obj.get("args") {
            if args.is_array() && !args.as_array().map(|a| a.is_empty()).unwrap_or(true) {
                result.insert("args".into(), args.clone());
            }
        }
        if let Some(env) = obj.get("env") {
            if env.is_object() && !env.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                result.insert("env".into(), env.clone());
            }
        }
    } else if obj.contains_key("url") {
        // HTTP/SSE type
        let transport = obj
            .get("transport")
            .and_then(|v| v.as_str())
            .unwrap_or("sse");
        let typ = if transport == "streamable-http" {
            "http"
        } else {
            "sse"
        };
        result.insert("type".into(), serde_json::json!(typ));

        if let Some(url) = obj.get("url") {
            result.insert("url".into(), url.clone());
        }
        if let Some(headers) = obj.get("headers") {
            if headers.is_object() && !headers.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                result.insert("headers".into(), headers.clone());
            }
        }
    } else {
        return Err(AppError::McpValidation(format!(
            "OpenClaw MCP server '{id}' has neither 'command' nor 'url' field"
        )));
    }

    Ok(Value::Object(result))
}

// ============================================================================
// Public API: Sync Functions
// ============================================================================

/// Sync a single MCP server to OpenClaw live config (merge-on-write).
pub fn sync_single_server_to_openclaw(
    _config: &MultiAppConfig,
    id: &str,
    server_spec: &Value,
) -> Result<(), AppError> {
    if !should_sync_openclaw_mcp() {
        return Ok(());
    }

    let openclaw_spec = convert_to_openclaw_format(server_spec)?;
    openclaw_config::upsert_mcp_server(id, &openclaw_spec)?;
    Ok(())
}

/// Remove a single MCP server from OpenClaw live config.
pub fn remove_server_from_openclaw(id: &str) -> Result<(), AppError> {
    if !should_sync_openclaw_mcp() {
        return Ok(());
    }

    openclaw_config::remove_mcp_server(id)?;
    Ok(())
}

/// Import MCP servers from OpenClaw live config to unified structure.
///
/// Existing servers will have OpenClaw app enabled without overwriting other fields.
pub fn import_from_openclaw(config: &mut MultiAppConfig) -> Result<usize, AppError> {
    let servers_map = openclaw_config::get_mcp_servers()?;
    if servers_map.is_empty() {
        return Ok(0);
    }

    let servers = config.mcp.servers.get_or_insert_with(HashMap::new);

    let mut changed = 0;
    let mut errors = Vec::new();

    for (id, spec) in &servers_map {
        // Convert from OpenClaw format to unified format
        let unified_spec = match convert_from_openclaw_format(id, spec) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Skip invalid OpenClaw MCP server '{id}': {e}");
                errors.push(format!("{id}: {e}"));
                continue;
            }
        };

        // Validate the converted spec
        if let Err(e) = validate_server_spec(&unified_spec) {
            log::warn!("Skip invalid MCP server '{id}' after conversion: {e}");
            errors.push(format!("{id}: {e}"));
            continue;
        }

        if let Some(existing) = servers.get_mut(id) {
            // Existing server: just enable OpenClaw app
            if !existing.apps.openclaw {
                existing.apps.openclaw = true;
                changed += 1;
                log::info!("MCP server '{id}' enabled for OpenClaw");
            }
        } else {
            // New server: default to only OpenClaw enabled
            servers.insert(
                id.clone(),
                McpServer {
                    id: id.clone(),
                    name: id.clone(),
                    server: unified_spec,
                    apps: McpApps {
                        claude: false,
                        codex: false,
                        gemini: false,
                        grokbuild: false,
                        opencode: false,
                        openclaw: true,
                        hermes: false,
                    },
                    description: None,
                    homepage: None,
                    docs: None,
                    tags: Vec::new(),
                },
            );
            changed += 1;
            log::info!("Imported new MCP server '{id}' from OpenClaw");
        }
    }

    if !errors.is_empty() {
        log::warn!(
            "Import completed with {} failures: {:?}",
            errors.len(),
            errors
        );
    }

    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // convert_to_openclaw_format tests
    // ========================================================================

    #[test]
    fn test_convert_stdio_to_openclaw() {
        let spec = serde_json::json!({
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem"],
            "env": { "HOME": "/Users/test" }
        });

        let result = convert_to_openclaw_format(&spec).unwrap();
        // No type field in OpenClaw format
        assert!(result.get("type").is_none());
        assert_eq!(result["command"], "npx");
        assert_eq!(result["args"][0], "-y");
        assert_eq!(result["args"][1], "@modelcontextprotocol/server-filesystem");
        assert_eq!(result["env"]["HOME"], "/Users/test");
    }

    #[test]
    fn test_convert_http_to_openclaw_streamable() {
        let spec = serde_json::json!({
            "type": "http",
            "url": "https://example.com/mcp",
            "headers": { "Authorization": "Bearer xxx" }
        });

        let result = convert_to_openclaw_format(&spec).unwrap();
        assert!(result.get("type").is_none());
        assert_eq!(result["url"], "https://example.com/mcp");
        assert_eq!(result["transport"], "streamable-http");
        assert_eq!(result["headers"]["Authorization"], "Bearer xxx");
    }

    #[test]
    fn test_convert_sse_to_openclaw() {
        let spec = serde_json::json!({
            "type": "sse",
            "url": "https://example.com/mcp/sse"
        });

        let result = convert_to_openclaw_format(&spec).unwrap();
        assert_eq!(result["url"], "https://example.com/mcp/sse");
        assert_eq!(result["transport"], "sse");
    }

    #[test]
    fn test_convert_stdio_empty_env_omitted() {
        let spec = serde_json::json!({
            "type": "stdio",
            "command": "node",
            "args": [],
            "env": {}
        });

        let result = convert_to_openclaw_format(&spec).unwrap();
        assert_eq!(result["command"], "node");
        // Empty args and env should be omitted
        assert!(result.get("args").is_none());
        assert!(result.get("env").is_none());
    }

    #[test]
    fn test_convert_unknown_type_fails() {
        let spec = serde_json::json!({ "type": "grpc", "command": "foo" });
        assert!(convert_to_openclaw_format(&spec).is_err());
    }

    // ========================================================================
    // convert_from_openclaw_format tests
    // ========================================================================

    #[test]
    fn test_convert_openclaw_stdio_to_unified() {
        let spec = serde_json::json!({
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem"],
            "env": { "HOME": "/Users/test" }
        });

        let result = convert_from_openclaw_format("filesystem", &spec).unwrap();
        assert_eq!(result["type"], "stdio");
        assert_eq!(result["command"], "npx");
        assert_eq!(result["args"][0], "-y");
        assert_eq!(result["env"]["HOME"], "/Users/test");
    }

    #[test]
    fn test_convert_openclaw_http_streamable_to_unified() {
        let spec = serde_json::json!({
            "url": "https://example.com/mcp",
            "transport": "streamable-http",
            "headers": { "Authorization": "Bearer xxx" }
        });

        let result = convert_from_openclaw_format("remote-server", &spec).unwrap();
        assert_eq!(result["type"], "http");
        assert_eq!(result["url"], "https://example.com/mcp");
        assert_eq!(result["headers"]["Authorization"], "Bearer xxx");
    }

    #[test]
    fn test_convert_openclaw_sse_default_to_unified() {
        let spec = serde_json::json!({
            "url": "https://example.com/mcp/sse",
            "transport": "sse"
        });

        let result = convert_from_openclaw_format("remote-sse", &spec).unwrap();
        assert_eq!(result["type"], "sse");
        assert_eq!(result["url"], "https://example.com/mcp/sse");
    }

    #[test]
    fn test_convert_openclaw_no_url_no_command_fails() {
        let spec = serde_json::json!({ "description": "no transport" });
        assert!(convert_from_openclaw_format("bad-server", &spec).is_err());
    }
}
