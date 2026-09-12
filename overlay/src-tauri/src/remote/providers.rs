//! 远端供应商 SSOT：`~/.cc-switch/providers/{app}.json`
//!
//! 每个目标机器（宿主机 / 容器）维护自己的一份完整供应商候选池，与本机
//! SQLite providers 表同构（`Provider` 记录 + `current_provider_id`）。
//! live 文件（settings.json / opencode.json / ...）保持不变。
//!
//! 与本机语义对齐：
//! - additive（opencode/openclaw/hermes）：live 即完整集合，每次读 SSOT 时
//!   幂等同步 live → SSOT（对齐本机启动自动导入 lib.rs）；
//! - 非 additive（claude/codex/gemini/grokbuild）：仅当 SSOT 为空才从 live
//!   导入一条 `default`（对齐 `should_import_default_config_on_startup`）。
//!
//! 通过 `FileOps` 支持宿主机（SFTP）与容器（docker exec）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::fsops::FileOps;
use crate::provider::{Provider, ProviderMeta};

/// 是否 additive 模式 app（live 即完整供应商集合）。
pub fn is_additive_app(app: &str) -> bool {
    matches!(app, "opencode" | "openclaw" | "hermes" | "pi")
}

/// SSOT 文件路径。
pub fn remote_providers_ssot_path(root: &str, app: &str) -> String {
    format!("{root}/.cc-switch/providers/{app}.json")
}

/// SSOT 文件内容（版本化，未来可扩展）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteProvidersSsot {
    #[serde(default)]
    pub version: u32,
    /// 非 additive 的当前生效供应商（对齐 DB `is_current`）；additive 不维护
    /// （由 live 决定 current）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_id: Option<String>,
    #[serde(default)]
    pub providers: Vec<Provider>,
}

impl Default for RemoteProvidersSsot {
    fn default() -> Self {
        Self {
            version: 1,
            current_provider_id: None,
            providers: Vec::new(),
        }
    }
}

/// 读远端 SSOT；文件缺失视为空。
pub async fn read_remote_providers_ssot<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
) -> Result<RemoteProvidersSsot, String> {
    let path = remote_providers_ssot_path(root, app);
    match fs.read_text_optional(&path).await? {
        Some(text) if !text.trim().is_empty() => {
            serde_json::from_str(&text).map_err(|e| format!("解析 {path} 失败: {e}"))
        }
        _ => Ok(RemoteProvidersSsot::default()),
    }
}

/// 原子写回远端 SSOT（键排序保证确定性输出）。
pub async fn write_remote_providers_ssot<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
    ssot: &RemoteProvidersSsot,
) -> Result<(), String> {
    let value =
        serde_json::to_value(ssot).map_err(|e| format!("序列化 providers SSOT 失败: {e}"))?;
    let sorted = crate::config::sort_json_keys(&value);
    let json = serde_json::to_string_pretty(&sorted)
        .map_err(|e| format!("序列化 providers SSOT 失败: {e}"))?;
    fs.write_text_atomic(&remote_providers_ssot_path(root, app), &json)
        .await
}

/// 更新远端 SSOT 中供应商的排序索引（对齐本机 update_providers_sort_order）。
pub async fn update_remote_provider_sort_order<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
    updates: Vec<crate::services::ProviderSortUpdate>,
) -> Result<(), String> {
    let mut ssot = read_remote_providers_ssot(fs, root, app).await?;
    for update in &updates {
        if let Some(provider) = ssot.providers.iter_mut().find(|p| p.id == update.id) {
            provider.sort_index = Some(update.sort_index);
        }
    }
    write_remote_providers_ssot(fs, root, app, &ssot).await
}

/// 更新远端 SSOT 中供应商的元数据（用量查询配置、备注等）。
pub async fn update_remote_provider_meta<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
    provider_id: &str,
    meta: crate::provider::ProviderMeta,
) -> Result<(), String> {
    let mut ssot = read_remote_providers_ssot(fs, root, app).await?;
    let provider = ssot
        .providers
        .iter_mut()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| format!("远端供应商 {provider_id} 不存在"))?;
    provider.meta = Some(meta);
    write_remote_providers_ssot(fs, root, app, &ssot).await
}

/// upsert：按 id 更新或追加，返回是否新增。
pub fn upsert_provider(providers: &mut Vec<Provider>, provider: Provider) -> bool {
    match providers.iter_mut().find(|p| p.id == provider.id) {
        Some(existing) => {
            *existing = provider;
            false
        }
        None => {
            providers.push(provider);
            true
        }
    }
}

/// 从远端 live 同步到 SSOT（对齐本机启动导入语义，幂等）。
///
/// 返回 `(变更条数, additive live 供应商 ID 集合)`。
///
/// `auto_import_default`：仅非 additive 生效——true = 每次确保 SSOT 存在一条
/// `default`（内容 = live 当前配置，幂等更新，用户可随时看到当前机器配置）；
/// false = 仅空库才从 live 导入 default（旧行为，更快，方案 A 保留）。
/// live_ids 仅在 additive 时返回本次读到的 live 内容（get 场景直接复用，
/// 省一次对 live 文件的重复读取）；非 additive 恒为空 Vec。
pub async fn sync_remote_live_into_ssot<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
    auto_import_default: bool,
) -> Result<(usize, Vec<String>), String> {
    if is_additive_app(app) {
        // additive：live 即完整集合，幂等 upsert（live_config_managed = true）
        let mut ssot = read_remote_providers_ssot(fs, root, app).await?;
        let live = parse_remote_live_providers(fs, root, app).await?;
        let mut changed = 0usize;
        for p in &live {
            if upsert_provider(&mut ssot.providers, p.clone()) {
                changed += 1;
            }
        }
        if changed > 0 {
            write_remote_providers_ssot(fs, root, app, &ssot).await?;
        }
        let live_ids = live.iter().map(|p| p.id.clone()).collect();
        Ok((changed, live_ids))
    } else {
        // 非 additive：live 有内容时——
        // - auto_import_default=true：确保 SSOT 中存在一条 `default`
        //   （内容 = live 当前生效配置，幂等更新）——用户要求：远端要能随时看到
        //   「当前机器实际在用什么配置」。已有候选池时不动 current 标记。
        // - auto_import_default=false：仅空库才从 live 导入 default（旧行为）。
        let Some(default) = parse_remote_live_default(fs, root, app).await? else {
            return Ok((0, Vec::new())); // live 无内容 → 不导入
        };
        // live 是远端接管/路由写的占位配置（token 含 PROXY_MANAGED），不是真实
        // 供应商配置——不导入 SSOT，避免把 `http://127.0.0.1:15721` 之类路由残留
        // 当成「当前机器实际在用什么配置」展示在供应商面板。
        if crate::proxy::remote_route::is_route_placeholder_settings(&default.settings_config) {
            return Ok((0, Vec::new()));
        }
        let mut ssot = read_remote_providers_ssot(fs, root, app).await?;
        // 清理历史导入的占位 provider（路由残留，非真实供应商）——防止关路由后
        // 面板仍显示隧道 URL。
        let had_placeholder = ssot
            .providers
            .iter()
            .any(|p| crate::proxy::remote_route::is_route_placeholder_settings(&p.settings_config));
        if had_placeholder {
            ssot.providers.retain(|p| {
                !crate::proxy::remote_route::is_route_placeholder_settings(&p.settings_config)
            });
            write_remote_providers_ssot(fs, root, app, &ssot).await?;
        }
        if !auto_import_default && !ssot.providers.is_empty() {
            return Ok((0, Vec::new()));
        }
        // 已有候选池（含用户手动添加的供应商）→ 不再从 live 导入 default，
        // 避免切换供应商后 live 配置变化触发重复导入。
        // 只有空库首次导入时才从 live 拉取 default。
        if !ssot.providers.is_empty() {
            return Ok((0, Vec::new()));
        }
        // 空库首导时把 default 设为 current；已有候选池时保留用户切换的记录
        if ssot.providers.is_empty() {
            ssot.current_provider_id = Some("default".to_string());
        }
        upsert_provider(&mut ssot.providers, default);
        write_remote_providers_ssot(fs, root, app, &ssot).await?;
        Ok((1, Vec::new()))
    }
}

/// additive：解析远端 live 文件为 Provider 列表
/// （对齐本机 `import_*_providers_from_live` 的字段语义）。
async fn parse_remote_live_providers<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
) -> Result<Vec<Provider>, String> {
    let mut out = Vec::new();
    match app {
        "opencode" => {
            let path = format!("{root}/.config/opencode/opencode.json");
            let Some(text) = fs.read_text_optional(&path).await? else {
                return Ok(out);
            };
            let value = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({}));
            let Some(providers_obj) = value.get("provider").and_then(Value::as_object) else {
                return Ok(out);
            };
            for (id, config_value) in providers_obj {
                match serde_json::from_value::<crate::provider::OpenCodeProviderConfig>(
                    config_value.clone(),
                ) {
                    Ok(config) => {
                        let settings_config =
                            serde_json::to_value(&config).unwrap_or_else(|_| config_value.clone());
                        let display_name = config.name.clone().unwrap_or_else(|| id.clone());
                        let mut p =
                            Provider::with_id(id.clone(), display_name, settings_config, None);
                        p.meta = Some(ProviderMeta {
                            live_config_managed: Some(true),
                            ..Default::default()
                        });
                        out.push(p);
                    }
                    Err(e) => log::warn!("解析远端 opencode 供应商 '{id}' 失败: {e}"),
                }
            }
        }
        "openclaw" => {
            let path = format!("{root}/.openclaw/openclaw.json");
            let Some(text) = fs.read_text_optional(&path).await? else {
                return Ok(out);
            };
            let value = json5::from_str::<Value>(&text).unwrap_or_else(|_| json!({}));
            let Some(providers_obj) = value
                .get("models")
                .and_then(|m| m.get("providers"))
                .and_then(Value::as_object)
            else {
                return Ok(out);
            };
            for (id, config_value) in providers_obj {
                if id.trim().is_empty() {
                    continue;
                }
                match serde_json::from_value::<crate::openclaw_config::OpenClawProviderConfig>(
                    config_value.clone(),
                ) {
                    Ok(config) => {
                        if config.models.is_empty() {
                            continue;
                        }
                        let settings_config =
                            serde_json::to_value(&config).unwrap_or_else(|_| config_value.clone());
                        let display_name = config
                            .models
                            .first()
                            .and_then(|m| m.name.clone())
                            .unwrap_or_else(|| id.clone());
                        let mut p =
                            Provider::with_id(id.clone(), display_name, settings_config, None);
                        p.meta = Some(ProviderMeta {
                            live_config_managed: Some(true),
                            ..Default::default()
                        });
                        out.push(p);
                    }
                    Err(e) => log::warn!("解析远端 openclaw 供应商 '{id}' 失败: {e}"),
                }
            }
        }
        "hermes" => {
            let path = format!("{root}/.hermes/config.yaml");
            let Some(text) = fs.read_text_optional(&path).await? else {
                return Ok(out);
            };
            let yaml = serde_yaml::from_str::<serde_yaml::Value>(&text)
                .unwrap_or_else(|_| serde_yaml::Value::Null);
            let Ok(value) = crate::hermes_config::yaml_to_json(&yaml) else {
                return Ok(out);
            };
            if let Some(seq) = value.get("custom_providers").and_then(Value::as_array) {
                for item in seq {
                    let Some(name) = item.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    if name.trim().is_empty() {
                        continue;
                    }
                    let mut p =
                        Provider::with_id(name.to_string(), name.to_string(), item.clone(), None);
                    p.meta = Some(ProviderMeta {
                        live_config_managed: Some(true),
                        ..Default::default()
                    });
                    out.push(p);
                }
            }
        }
        "pi" => {
            let path = format!("{root}/.pi/agent/models.json");
            let Some(text) = fs.read_text_optional(&path).await? else {
                return Ok(out);
            };
            let value = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({}));
            let Some(providers_obj) = value.get("providers").and_then(Value::as_object) else {
                return Ok(out);
            };
            for (id, config_value) in providers_obj {
                if id.trim().is_empty() {
                    continue;
                }
                // Pi providers are plain JSON objects with fields like name, baseUrl, etc.
                // Validate they are objects (consistent with pi_config::validate_provider_node).
                if !config_value.is_object() {
                    log::warn!("远端 Pi provider '{id}' 不是对象，跳过");
                    continue;
                }
                let display_name = config_value
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or(id)
                    .to_string();
                let mut p = Provider::with_id(id.clone(), display_name, config_value.clone(), None);
                p.meta = Some(ProviderMeta {
                    live_config_managed: Some(true),
                    ..Default::default()
                });
                out.push(p);
            }
        }
        _ => {}
    }
    Ok(out)
}

/// 非 additive：解析远端 live 为一条 `default` 记录
/// （对齐本机 `import_default_config` 的 settings_config 结构）。
async fn parse_remote_live_default<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
) -> Result<Option<Provider>, String> {
    let settings_config = match app {
        "claude" => {
            let path = format!("{root}/.claude/settings.json");
            let Some(text) = fs.read_text_optional(&path).await? else {
                return Ok(None);
            };
            serde_json::from_str::<Value>(&text)
                .map_err(|e| format!("解析远端 settings.json 失败: {e}"))?
        }
        "codex" => {
            let cfg_path = format!("{root}/.codex/config.toml");
            let Some(cfg_text) = fs.read_text_optional(&cfg_path).await? else {
                return Ok(None);
            };
            let auth_path = format!("{root}/.codex/auth.json");
            let auth: Value = match fs.read_text_optional(&auth_path).await? {
                Some(t) => serde_json::from_str(&t).unwrap_or_else(|_| json!({})),
                None => json!({}),
            };
            json!({ "auth": auth, "config": cfg_text })
        }
        "gemini" => {
            let env_path = format!("{root}/.gemini/.env");
            let Some(env_text) = fs.read_text_optional(&env_path).await? else {
                return Ok(None);
            };
            let env_map = crate::gemini_config::parse_env_file(&env_text);
            let env_json = crate::gemini_config::env_to_json(&env_map)
                .get("env")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let settings_path = format!("{root}/.gemini/settings.json");
            let config_obj: Value = match fs.read_text_optional(&settings_path).await? {
                Some(t) => serde_json::from_str(&t).unwrap_or_else(|_| json!({})),
                None => json!({}),
            };
            json!({ "env": env_json, "config": config_obj })
        }
        "grokbuild" => {
            let path = format!("{root}/.grok/config.toml");
            let Some(config) = fs.read_text_optional(&path).await? else {
                return Ok(None);
            };
            json!({ "config": config })
        }
        _ => return Ok(None),
    };

    let mut provider = Provider::with_id(
        "default".to_string(),
        "default".to_string(),
        settings_config,
        None,
    );
    // 对齐本机 import_default_config 的 category 判定（codex 官方登录态 → official）
    if app == "codex" {
        let config_text = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str);
        let has_provider_key = crate::codex_config::extract_codex_api_key(
            provider.settings_config.get("auth"),
            config_text,
        )
        .is_some();
        let has_login_material = provider
            .settings_config
            .get("auth")
            .is_some_and(crate::codex_config::codex_auth_has_login_material);
        provider.category = Some(
            if has_login_material && !has_provider_key {
                "official"
            } else {
                "custom"
            }
            .to_string(),
        );
    } else {
        provider.category = Some("custom".to_string());
    }
    Ok(Some(provider))
}

/// 探测容器走本机路由所需的 base_url host：
/// - host 网络 → `127.0.0.1`（容器共享宿主机回环，隧道直接可达）；
/// - bridge / 自定义网络 → 返回网关 IP（容器访问宿主机的地址），并自动下发
///   **per-container DNAT**（`-s <容器IP>` 限定源，只影响该容器）让容器经网关
///   打通隧道；
/// - 探测失败 / 容器不存在 → `Ok(None)`（上层降级为直连）。
///
/// 容器网络模式 → `docker network inspect` 用的网络名。
/// `default` 是 Docker 对「默认 bridge 网络」的 NetworkMode 表示（容器未显式指定
/// 网络时返回 default），与 `bridge` 语义相同；其余模式（自定义网络名 / none /
/// container:<id>）原样返回，交由上层探测。
fn resolve_container_network(mode: &str) -> &str {
    if mode == "bridge" || mode == "default" {
        "bridge"
    } else {
        mode
    }
}

/// 返回 `(base_host, 容器IP)`；容器 IP 供后续按容器精确删除 DNAT。
async fn detect_container_route_base(
    session: &crate::remote::connection::RemoteSession,
    container: &str,
    port: u16,
) -> Result<Option<(String, String)>, String> {
    let q = crate::remote::connection::shell_quote;
    // 1. 网络模式（host / bridge / 自定义网络名）
    let mode = crate::remote::connection::exec_command(
        &session.channel,
        &format!(
            "docker inspect {} --format '{{{{.HostConfig.NetworkMode}}}}' 2>/dev/null || true",
            q(container)
        ),
    )
    .await?;
    let mode = mode.trim();
    log::debug!("[remote] 容器 {container} 网络模式: {mode:?}");
    if mode.is_empty() {
        log::warn!(
            "[remote] 容器 {container} 网络模式探测为空（容器不存在或 docker inspect 失败），按直连写入"
        );
        return Ok(None); // 容器不存在或 inspect 失败
    }
    if mode == "host" {
        return Ok(Some((
            crate::remote::connection::REMOTE_TUNNEL_LISTEN_ADDR.to_string(),
            String::new(),
        )));
    }
    // 2. 非 host：从网络定义拿网关（bridge 网桥 / 自定义网络）。
    //    `default` 与 `bridge` 同义（见 resolve_container_network），否则会被当成
    //    网络名去 `docker network inspect default` 而查空 → 误判探测失败。
    let net = resolve_container_network(mode);
    let gw = crate::remote::connection::exec_command(
        &session.channel,
        &format!(
            "docker network inspect {} --format '{{{{(index .IPAM.Config 0).Gateway}}}}' 2>/dev/null || true",
            q(net)
        ),
    )
    .await?;
    let gw = gw.trim();
    log::debug!("[remote] 容器 {container} 网络 {net} 网关: {gw:?}");
    if gw.is_empty() || !gw.contains('.') {
        log::warn!("[remote] 容器 {container} 网络 {net} 网关探测为空/非法（{gw:?}），按直连写入");
        return Ok(None);
    }
    // 3. 拿容器自身 IP（per-container DNAT 的源限定）
    let ip = crate::remote::connection::exec_command(
        &session.channel,
        &format!(
            "docker inspect {} --format '{{{{range .NetworkSettings.Networks}}}}{{{{.IPAddress}}}} {{{{end}}}}' 2>/dev/null || true",
            q(container)
        ),
    )
    .await?;
    // 取第一个 IP（多网络时 range 输出多个 IP，空格分隔后取首个；无分隔会把多个
    // IP 串成一串，`.contains('.')` 误通过、DNAT 源 IP 变乱值）
    let ip = ip.split_whitespace().next().unwrap_or("").to_string();
    log::debug!("[remote] 容器 {container} IP: {ip:?}");
    if ip.is_empty() || !ip.contains('.') {
        log::warn!("[remote] 容器 {container} IP 探测为空/非法（{ip:?}），按直连写入");
        return Ok(None);
    }
    // 4. 下发 per-container DNAT（幂等），让该容器经网关访问隧道
    ensure_container_dnat(session, &ip, port).await?;
    log::debug!("[remote] 容器 {container} 走本机路由就绪：网关 {gw} 容器IP {ip}（DNAT 已下发）");
    Ok(Some((gw.to_string(), ip.to_string())))
}

/// 取容器的 IP（用于 per-container DNAT 精确删除）。失败/无 IP 返回 None。
async fn container_ip(
    session: &crate::remote::connection::RemoteSession,
    container: &str,
) -> Option<String> {
    let q = crate::remote::connection::shell_quote;
    let out = crate::remote::connection::exec_command(
        &session.channel,
        &format!(
            // IP 后跟空格分隔：容器可能挂在多个网络上，无分隔会把多个 IP 串成一串
            // （如 172.17.0.8fd00::1），`.contains('.')` 检查会误通过，DNAT 源 IP 变乱值。
            "docker inspect {} --format '{{{{range .NetworkSettings.Networks}}}}{{{{.IPAddress}}}} {{{{end}}}}' 2>/dev/null || true",
            q(container)
        ),
    )
    .await
    .ok()?;
    // 取第一个 IP（单网络即唯一 IP；多网络取首个，best-effort）
    let ip = out.split_whitespace().next().unwrap_or("").to_string();
    if ip.is_empty() || !ip.contains('.') {
        None
    } else {
        Some(ip)
    }
}

/// 在宿主机下发 **per-container** DNAT：仅把该容器（源 IP 限定）发往
/// 网关端口的流量转到本机路由隧道（127.0.0.1:{port}）。
/// - `sysctl route_localnet=1`：允许 DNAT 到回环地址；
/// - iptables PREROUTING（`-s <ip>` 入口 {port} → 127.0.0.1:{port}），`-C` 检查幂等。
///   **不限定 `-i docker0`**：默认 bridge 的容器经 docker0 进入，自定义 bridge 网络
///   的容器经 `br-<hash>` 进入；`-s <容器IP>` 已唯一限定该容器，按入口接口过滤反而会
///   漏掉自定义网络容器（与 netmode=default 同一类硬编码假设）。
async fn ensure_container_dnat(
    session: &crate::remote::connection::RemoteSession,
    container_ip: &str,
    port: u16,
) -> Result<(), String> {
    crate::remote::connection::exec_command(
        &session.channel,
        "sysctl -w net.ipv4.conf.all.route_localnet=1 >/dev/null 2>&1 || true",
    )
    .await?;
    let rule = format!(
        "iptables -t nat -C PREROUTING -s {ip} -p tcp --dport {port} -j DNAT --to-destination {}:{port} 2>/dev/null || iptables -t nat -A PREROUTING -s {ip} -p tcp --dport {port} -j DNAT --to-destination {}:{port} >/dev/null 2>&1 || true",
        crate::remote::connection::REMOTE_TUNNEL_LISTEN_ADDR,
        crate::remote::connection::REMOTE_TUNNEL_LISTEN_ADDR,
        ip = container_ip,
        port = port
    );
    crate::remote::connection::exec_command(&session.channel, &rule).await?;
    Ok(())
}

/// 撤销某容器的 per-container DNAT（与 ensure_container_dnat 对称，幂等）：
/// 按 `-s <ip>` 精确删除，只影响该容器；规则不存在时 `-D` 失败被 `|| true` 吞掉。
pub async fn remove_container_dnat(
    session: &crate::remote::connection::RemoteSession,
    container_ip: &str,
    port: u16,
) -> Result<(), String> {
    let rule = format!(
        "iptables -t nat -D PREROUTING -s {ip} -p tcp --dport {port} -j DNAT --to-destination {}:{port} 2>/dev/null || true",
        crate::remote::connection::REMOTE_TUNNEL_LISTEN_ADDR,
        ip = container_ip,
        port = port
    );
    crate::remote::connection::exec_command(&session.channel, &rule).await?;
    Ok(())
}

/// 把 provider 定义应用到远端对应 app 的 live 文件（复用各 remote::*::apply_*
/// 纯变换，产出与本机切换逐字节一致）。claude 分支与本机一致走
/// `build_effective_settings_with_common_config`（通用配置片段来自本机 DB）。
#[allow(clippy::too_many_arguments)]
pub async fn apply_remote_provider_to_live(
    db: &crate::database::Database,
    session: &crate::remote::connection::RemoteSession,
    container: Option<&str>,
    home: &str,
    host_name: &str,
    app: &str,
    provider: &Provider,
    route_proxy: bool,
    port: u16,
) -> Result<crate::remote::effect::EffectReport, String> {
    // 走本机路由的 base_url host 解析：
    // - 宿主机目标：127.0.0.1（隧道监听在宿主机回环）；
    // - 容器目标：探测网络模式——host → 127.0.0.1（共享宿主机回环）；
    //   bridge/自定义 → 网关 IP（容器访问宿主机的地址）+ per-container DNAT
    //   打通 docker0 → 隧道；探测失败则降级直连（避免写出无效 base_url）。
    // 隧道检查：意图走路由但反向隧道未建成（远端禁了端口转发等）→ 降级直连，
    // 并追加提示 notes，让用户知道「开了接管但没走路由」。
    let route_proxy_was = route_proxy;
    let tunnel_ok = if route_proxy {
        session.tunnel_is_active()
    } else {
        true
    };
    if route_proxy && !tunnel_ok {
        log::warn!(
            "[remote] 主机 {} 反向隧道未建立，「远端接管」不生效，本次按直连写入",
            host_name
        );
    }
    let route_proxy = route_proxy && tunnel_ok;
    let route_base = if !route_proxy {
        // 直连态：撤掉该容器先前走路由时下发的 per-container DNAT（幂等）
        if let Some(c) = container {
            if let Some(ip) = container_ip(session, c).await {
                if let Err(e) = remove_container_dnat(session, &ip, port).await {
                    log::warn!("[remote] 容器 {c} 直连态清理 DNAT 失败: {e}");
                }
            }
        }
        None
    } else if let Some(c) = container {
        match detect_container_route_base(session, c, port).await {
            Ok(Some((base, _ip))) => Some(base),
            Ok(None) => {
                log::warn!("[remote] 容器 {c} 走本机路由探测失败，按直连写入");
                None
            }
            Err(e) => {
                log::warn!("[remote] 容器 {c} 走本机路由探测失败: {e}，按直连写入");
                None
            }
        }
    } else {
        Some(crate::remote::connection::REMOTE_TUNNEL_LISTEN_ADDR.to_string())
    };
    let route_proxy = route_base.is_some();
    let report = match app {
        "claude" => {
            let effective =
                crate::services::provider::live::build_effective_settings_with_common_config(
                    db,
                    &crate::app_config::AppType::Claude,
                    provider,
                )
                .map_err(|e| e.to_string())?;
            let mut sanitized =
                crate::services::provider::live::sanitize_claude_settings_for_live(&effective);
            if route_proxy {
                // 走本机路由：远端 claude 的 live 与本机路由模式逐字节一致——
                // base_url 指向远端隧道（宿主机=127.0.0.1；容器=网关 IP），
                // token 用 PROXY_MANAGED:<host_id> 占位（本机代理识别 host_id 按该远端
                // 自己的当前供应商路由，并注入真实密钥）。
                let obj = sanitized
                    .as_object_mut()
                    .ok_or_else(|| "sanitized settings 不是对象".to_string())?;
                let env = obj.entry("env").or_insert_with(|| serde_json::json!({}));
                if let Some(e) = env.as_object_mut() {
                    let base = route_base
                        .as_deref()
                        .unwrap_or(crate::remote::connection::REMOTE_TUNNEL_LISTEN_ADDR);
                    e.insert(
                        "ANTHROPIC_BASE_URL".to_string(),
                        serde_json::json!(format!("http://{base}:{port}")),
                    );
                    e.insert(
                        "ANTHROPIC_AUTH_TOKEN".to_string(),
                        serde_json::json!(crate::proxy::remote_route::remote_token_for(
                            session.host_id()
                        )),
                    );
                }
            }
            crate::remote::settings::apply_provider_settings(
                session,
                container,
                home,
                host_name,
                &provider.name,
                &sanitized,
            )
            .await
        }
        "codex" => {
            let settings = if route_proxy {
                // 走本机路由：base_url 指向远端隧道（宿主机=127.0.0.1；容器=网关 IP），
                // 复用本机同一变换 apply_codex_official_proxy_route，保持与本机一致。
                let mut s = provider.settings_config.clone();
                if let Some(config) = s.get("config").and_then(|v| v.as_str()) {
                    let base = route_base
                        .as_deref()
                        .unwrap_or(crate::remote::connection::REMOTE_TUNNEL_LISTEN_ADDR);
                    let routed = crate::codex_config::apply_codex_official_proxy_route(
                        config,
                        &format!("http://{base}:{port}/v1"),
                    )
                    .map_err(|e| e.to_string())?;
                    s["config"] = serde_json::json!(routed);
                }
                s
            } else {
                provider.settings_config.clone()
            };
            crate::remote::codex::apply_codex_provider_settings(
                session,
                container,
                home,
                host_name,
                &provider.name,
                &settings,
                provider.category.as_deref(),
                crate::proxy::providers::resolve_codex_catalog_tool_profile(provider),
            )
            .await
        }
        "grokbuild" => {
            let settings = if route_proxy {
                // 走本机路由：直接复用本机代理接管的同一变换
                // （base_url 指向远端隧道：宿主机=127.0.0.1；容器=网关 IP +
                // api_key 占位 + api_backend=responses），与本机产物逐字段一致。
                let mut s = provider.settings_config.clone();
                if let Some(config) = s.get("config").and_then(|v| v.as_str()) {
                    let base = route_base
                        .as_deref()
                        .unwrap_or(crate::remote::connection::REMOTE_TUNNEL_LISTEN_ADDR);
                    let routed = crate::grok_config::apply_proxy_takeover(
                        config,
                        &format!("http://{base}:{port}/grokbuild/v1"),
                        &crate::proxy::remote_route::remote_token_for(session.host_id()),
                    )
                    .map_err(|e| e.to_string())?;
                    s["config"] = serde_json::json!(routed);
                }
                s
            } else {
                provider.settings_config.clone()
            };
            crate::remote::grok::apply_grok_provider_settings(
                session,
                container,
                home,
                host_name,
                &provider.name,
                &settings,
                provider.category.as_deref(),
            )
            .await
        }
        "gemini" => {
            crate::remote::gemini::apply_gemini_provider_settings(
                session,
                container,
                home,
                host_name,
                &provider.name,
                &provider.settings_config,
                provider,
                route_proxy,
                route_base.as_deref(),
                port,
                session.host_id(),
            )
            .await
        }
        "opencode" => {
            crate::remote::opencode::apply_opencode_provider_settings(
                session,
                container,
                home,
                host_name,
                &provider.name,
                &provider.settings_config,
                &provider.id,
            )
            .await
        }
        "openclaw" => {
            crate::remote::openclaw::apply_openclaw_provider_settings(
                session,
                container,
                home,
                host_name,
                &provider.name,
                &provider.settings_config,
                &provider.id,
            )
            .await
        }
        "hermes" => {
            crate::remote::hermes::apply_hermes_provider_settings(
                session,
                container,
                home,
                host_name,
                &provider.name,
                &provider.settings_config,
                &provider.id,
            )
            .await
        }
        "pi" => {
            crate::remote::pi::apply_pi_provider_settings(
                session,
                container,
                home,
                host_name,
                &provider.name,
                &provider.settings_config,
                &provider.id,
            )
            .await
        }
        other => Err(format!("远程切换暂不支持应用: {other}")),
    }?;
    // 隧道未建立而降级直连时，把原因追加进 warnings 让前端以醒目样式提示用户
    let mut report = report;
    if route_proxy_was && !tunnel_ok {
        report.warnings.push(format!(
            "「{host_name}」的远端接管已开启，但反向隧道未建立（远端可能禁用了端口转发），本次按直连写入；请检查远端 sshd 的 AllowTcpForwarding"
        ));
    }
    Ok(report)
}

/// 读远端 additive live 文件中的供应商 ID 集合（isInConfig 按钮态用）。
pub async fn read_remote_live_provider_ids<F: FileOps>(
    fs: &F,
    root: &str,
    app: &str,
) -> Result<Vec<String>, String> {
    let ids = match app {
        "opencode" => {
            let path = format!("{root}/.config/opencode/opencode.json");
            match fs.read_text_optional(&path).await? {
                Some(text) => serde_json::from_str::<Value>(&text)
                    .unwrap_or_else(|_| json!({}))
                    .get("provider")
                    .and_then(Value::as_object)
                    .map(|p| p.keys().cloned().collect())
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
        "openclaw" => {
            let path = format!("{root}/.openclaw/openclaw.json");
            match fs.read_text_optional(&path).await? {
                Some(text) => json5::from_str::<Value>(&text)
                    .unwrap_or_else(|_| json!({}))
                    .get("models")
                    .and_then(|m| m.get("providers"))
                    .and_then(Value::as_object)
                    .map(|p| p.keys().cloned().collect())
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
        "hermes" => {
            let path = format!("{root}/.hermes/config.yaml");
            match fs.read_text_optional(&path).await? {
                Some(text) => serde_yaml::from_str::<serde_yaml::Value>(&text)
                    .unwrap_or_else(|_| serde_yaml::Value::Null)
                    .get("custom_providers")
                    .and_then(|v| v.as_sequence())
                    .map(|seq| {
                        seq.iter()
                            .filter_map(|p| p.get("name").and_then(|n| n.as_str()))
                            .map(|n| n.to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
        "pi" => {
            let path = format!("{root}/.pi/agent/models.json");
            match fs.read_text_optional(&path).await? {
                Some(text) => serde_json::from_str::<Value>(&text)
                    .unwrap_or_else(|_| json!({}))
                    .get("providers")
                    .and_then(Value::as_object)
                    .map(|p| p.keys().cloned().collect())
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
        _ => Vec::new(),
    };
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsops::LocalFileOps;

    #[test]
    fn resolve_network_maps_default_to_bridge() {
        // 回归点：Docker 对未显式指定网络的容器返回 NetworkMode=default，
        // 必须映射到 bridge 网络去查网关，否则误判探测失败而降级直连。
        assert_eq!(resolve_container_network("default"), "bridge");
        assert_eq!(resolve_container_network("bridge"), "bridge");
        assert_eq!(resolve_container_network("host"), "host");
        assert_eq!(resolve_container_network("my_custom_net"), "my_custom_net");
        assert_eq!(resolve_container_network("none"), "none");
    }

    fn temp_root(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-remote-ssot-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.to_string_lossy().to_string()
    }

    fn sample_provider(id: &str, name: &str) -> Provider {
        let mut p = Provider::with_id(
            id.to_string(),
            name.to_string(),
            json!({ "env": { "ANTHROPIC_BASE_URL": "https://x.example" } }),
            None,
        );
        p.meta = Some(ProviderMeta {
            live_config_managed: Some(true),
            ..Default::default()
        });
        p
    }

    #[test]
    fn upsert_adds_and_updates() {
        let mut list = vec![sample_provider("a", "A")];
        assert!(!upsert_provider(&mut list, sample_provider("a", "A2")));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "A2");
        assert!(upsert_provider(&mut list, sample_provider("b", "B")));
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn ssot_roundtrip_missing_file_is_empty() {
        let root = temp_root("roundtrip");
        let fs = LocalFileOps;
        let empty = read_remote_providers_ssot(&fs, &root, "claude")
            .await
            .expect("read empty");
        assert!(empty.providers.is_empty());

        let mut ssot = RemoteProvidersSsot::default();
        ssot.current_provider_id = Some("a".to_string());
        ssot.providers.push(sample_provider("a", "A"));
        write_remote_providers_ssot(&fs, &root, "claude", &ssot)
            .await
            .expect("write");

        let read = read_remote_providers_ssot(&fs, &root, "claude")
            .await
            .expect("read back");
        assert_eq!(read.current_provider_id.as_deref(), Some("a"));
        assert_eq!(read.providers.len(), 1);
        assert_eq!(read.providers[0].name, "A");
        // 幂等：再写一次仍可读（确定性输出）
        write_remote_providers_ssot(&fs, &root, "claude", &read)
            .await
            .expect("rewrite");
    }

    #[tokio::test]
    async fn sync_additive_imports_live_into_ssot() {
        let root = temp_root("additive");
        let fs = LocalFileOps;
        // 构造远端 live：opencode.json 带两个 provider
        let live = json!({
            "provider": {
                "p1": { "npm": "@ai-sdk/openai-compatible", "name": "P One", "options": { "baseURL": "https://p1.example" } },
                "p2": { "npm": "@ai-sdk/anthropic", "options": { "apiKey": "k" } }
            }
        });
        let path = format!("{root}/.config/opencode/opencode.json");
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&live).unwrap()).unwrap();

        let (changed, live_ids) = sync_remote_live_into_ssot(&fs, &root, "opencode", false)
            .await
            .expect("sync");
        assert_eq!(changed, 2);
        assert_eq!(live_ids.len(), 2); // additive：顺带返回 live ID 集合
        assert!(live_ids.contains(&"p1".to_string()));
        let ssot = read_remote_providers_ssot(&fs, &root, "opencode")
            .await
            .expect("read");
        assert_eq!(ssot.providers.len(), 2);
        let p1 = ssot.providers.iter().find(|p| p.id == "p1").unwrap();
        assert_eq!(p1.name, "P One"); // display_name 取 config.name
        assert_eq!(
            p1.meta.as_ref().and_then(|m| m.live_config_managed),
            Some(true)
        );
        let p2 = ssot.providers.iter().find(|p| p.id == "p2").unwrap();
        assert_eq!(p2.name, "p2"); // 无 name 时回退 id

        // 幂等：再同步不新增
        let (changed2, _) = sync_remote_live_into_ssot(&fs, &root, "opencode", false)
            .await
            .expect("sync2");
        assert_eq!(changed2, 0);
    }

    #[tokio::test]
    async fn sync_additive_preserves_existing_ssot_entries() {
        let root = temp_root("additive-preserve");
        let fs = LocalFileOps;
        // SSOT 已有候选池条目（未在 live 中，如 db-only 供应商）
        let mut ssot = RemoteProvidersSsot::default();
        ssot.providers.push(sample_provider("db-only", "DB Only"));
        write_remote_providers_ssot(&fs, &root, "opencode", &ssot)
            .await
            .expect("seed ssot");
        // live 有一个 provider
        let live = json!({
            "provider": { "p1": { "npm": "@ai-sdk/openai-compatible", "name": "P One" } }
        });
        let path = format!("{root}/.config/opencode/opencode.json");
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&live).unwrap()).unwrap();

        let _ = sync_remote_live_into_ssot(&fs, &root, "opencode", false)
            .await
            .expect("sync");
        let read = read_remote_providers_ssot(&fs, &root, "opencode")
            .await
            .expect("read");
        let ids: Vec<&str> = read.providers.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"db-only"));
        assert!(ids.contains(&"p1"));
    }

    #[tokio::test]
    async fn sync_non_additive_imports_default_once() {
        let root = temp_root("noadditive");
        let fs = LocalFileOps;
        let settings_path = format!("{root}/.claude/settings.json");
        std::fs::create_dir_all(std::path::Path::new(&settings_path).parent().unwrap()).unwrap();
        std::fs::write(
            &settings_path,
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://remote.example","ANTHROPIC_AUTH_TOKEN":"sk-1"}}"#,
        )
        .unwrap();

        let (changed, _) = sync_remote_live_into_ssot(&fs, &root, "claude", true)
            .await
            .expect("sync");
        assert_eq!(changed, 1);
        let ssot = read_remote_providers_ssot(&fs, &root, "claude")
            .await
            .expect("read");
        assert_eq!(ssot.current_provider_id.as_deref(), Some("default"));
        assert_eq!(ssot.providers.len(), 1);
        assert_eq!(ssot.providers[0].id, "default");
        assert_eq!(ssot.providers[0].category.as_deref(), Some("custom"));
        assert_eq!(
            ssot.providers[0]
                .settings_config
                .pointer("/env/ANTHROPIC_BASE_URL")
                .and_then(Value::as_str),
            Some("https://remote.example")
        );

        // 非 additive：已有 SSOT 条目且 default 内容一致 → 不再写（幂等）
        let (changed2, _) = sync_remote_live_into_ssot(&fs, &root, "claude", true)
            .await
            .expect("sync2");
        assert_eq!(changed2, 0);
    }

    #[tokio::test]
    async fn sync_non_additive_ensures_default_with_existing_candidates() {
        let root = temp_root("noadditive-candidates");
        let fs = LocalFileOps;
        // 已有候选池（自定义供应商），但无 default
        let mut ssot = RemoteProvidersSsot::default();
        ssot.providers
            .push(sample_provider("my-vendor", "My Vendor"));
        write_remote_providers_ssot(&fs, &root, "claude", &ssot)
            .await
            .expect("seed");
        // live 有当前配置
        let settings_path = format!("{root}/.claude/settings.json");
        std::fs::create_dir_all(std::path::Path::new(&settings_path).parent().unwrap()).unwrap();
        std::fs::write(
            &settings_path,
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://remote.example"}}"#,
        )
        .unwrap();

        let (changed, _) = sync_remote_live_into_ssot(&fs, &root, "claude", true)
            .await
            .expect("sync");
        assert_eq!(changed, 1); // default 被 upsert（用户要求：随时可见当前机器配置）
        let read = read_remote_providers_ssot(&fs, &root, "claude")
            .await
            .expect("read");
        assert!(read.providers.iter().any(|p| p.id == "default"));
        assert!(read.providers.iter().any(|p| p.id == "my-vendor")); // 候选池保留
                                                                     // 已有候选池时不动 current 标记（空库首导才设 default 为 current）
        assert_eq!(read.current_provider_id.as_deref(), None);

        // 幂等：内容一致不再写
        let (changed2, _) = sync_remote_live_into_ssot(&fs, &root, "claude", true)
            .await
            .expect("sync2");
        assert_eq!(changed2, 0);
    }

    #[tokio::test]
    async fn sync_missing_live_is_noop() {
        let root = temp_root("nolive");
        let fs = LocalFileOps;
        let (changed, _) = sync_remote_live_into_ssot(&fs, &root, "claude", true)
            .await
            .expect("sync no live");
        assert_eq!(changed, 0);
        let ssot = read_remote_providers_ssot(&fs, &root, "claude")
            .await
            .expect("read");
        assert!(ssot.providers.is_empty());
    }

    #[tokio::test]
    async fn read_live_ids_openclaw_json5() {
        let root = temp_root("liveids");
        let fs = LocalFileOps;
        // JSON5 语法（无引号键、尾逗号）
        let live = r#"{
            models: {
                providers: {
                    m1: { name: "M1", models: [{ id: "m1-model" }] },
                    m2: { name: "M2", models: [{ id: "m2-model" }] },
                },
            },
        }"#;
        let path = format!("{root}/.openclaw/openclaw.json");
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, live).unwrap();

        let ids = read_remote_live_provider_ids(&fs, &root, "openclaw")
            .await
            .expect("ids");
        assert!(ids.contains(&"m1".to_string()));
        assert!(ids.contains(&"m2".to_string()));
    }
}
