//! Tauri 命令层：前端通过 `invoke` 调用远程主机管理功能。
//!
//! 注意：命令需要在 `lib.rs` 的 `invoke_handler` 中注册。

use serde_json::{json, Value};
use std::str::FromStr;
use tauri::{Emitter, State};

use crate::fsops::FileOps as _;
use crate::live::patch::LivePatch as _;
#[cfg(target_os = "windows")]
use crate::remote::credentials;
use crate::remote::effect::EffectReport;
use crate::remote::settings;
use crate::remote::{connection, AuthMethod, RemoteHost};
use base64::Engine as _;
use sha2::{Digest, Sha256};

/// 取该主机「per-app 走本机路由」意图（宿主机目标）：
/// - route_proxy_apps（JSON）有该 app → 用它的值；
/// - 空但旧布尔 route_through_local_proxy=true（迁移前旧库兜底）→ 视为全开。
fn host_route_proxy_for_app(host: &RemoteHost, app: &str) -> bool {
    if let Some(&v) = host.route_proxy_apps.get(app) {
        return v;
    }
    if host.route_proxy_apps.is_empty() && host.route_through_local_proxy {
        // 旧库迁移兜底：迁移 SQL 会把 =1 的主机展开成全开，这里防御性兜底
        return true;
    }
    false
}

/// 取某目标（宿主机 / 容器）「per-app 走本机路由」意图：
/// - 容器目标：读 route_proxy_container_apps[container][app]（各自独立，缺省=关）；
/// - 宿主机目标：走 host_route_proxy_for_app（host 级字段）。
pub fn route_proxy_for_target(host: &RemoteHost, container: Option<&str>, app: &str) -> bool {
    match container {
        Some(c) => host
            .route_proxy_container_apps
            .get(c)
            .and_then(|m| m.get(app))
            .copied()
            .unwrap_or(false),
        None => host_route_proxy_for_app(host, app),
    }
}

/// 计算实际生效的 route_proxy：
/// - DB 意图（route_through_local_proxy）为 false → false；
/// - 意图为 true 但本机路由未运行 → 降级为直连（避免把 base_url 写成
///   指向无接收方隧道的地址导致远端不可用），并记 warning；
/// - 意图为 true 且本机路由运行中 → true。
async fn effective_route_proxy(
    proxy_service: &crate::services::proxy::ProxyService,
    desired: bool,
) -> bool {
    if !desired {
        return false;
    }
    if proxy_service.is_running().await {
        true
    } else {
        log::warn!("[remote] 本机路由未运行，「走本机路由」不生效，本次切换按直连写入");
        false
    }
}

/// 本机代理实际监听端口（运行中取 ProxyStatus.port；未运行读配置 listen_port，
/// 配置为 0 则报错——与本机 build_proxy_urls 同源，不硬编码默认端口）。
/// 同时同步给连接层（反向隧道端口），保证隧道/base_url/DNAT 用同一端口。
async fn current_proxy_port(
    db: &crate::database::Database,
    proxy_service: &crate::services::proxy::ProxyService,
) -> Result<u16, String> {
    let port = match proxy_service.get_status().await {
        Ok(s) if s.running && s.port != 0 => {
            // 同步反向隧道回连地址：监听地址一改（如自定义局域网 IP），
            // 隧道回连仍对准本机代理实际监听处（0.0.0.0/:: 在 setter 内归一化）。
            connection::set_tunnel_host(&s.address);
            s.port
        }
        _ => {
            let config = db
                .get_proxy_config()
                .await
                .map_err(|e| format!("获取代理配置失败: {e}"))?;
            config.listen_port
        }
    };
    if port == 0 {
        return Err("代理监听端口为 0，但代理服务器尚未运行，无法生成接管地址".to_string());
    }
    connection::set_tunnel_port(port);
    Ok(port)
}
use crate::store::AppState;

/// 内嵌的远端 SQLite helper（musl 静态，x86_64 / aarch64）。
const SQLITE_HELPER_X86_64: &[u8] = include_bytes!("../../resources/sqlite-helper-x86_64-linux");
const SQLITE_HELPER_AARCH64: &[u8] = include_bytes!("../../resources/sqlite-helper-aarch64-linux");

/// 在宿主机（`sh -c`）或容器内（`docker exec ... sh -c`）执行一段 POSIX sh 脚本。
async fn exec_in_target(
    session: &connection::RemoteSession,
    container: Option<&str>,
    script: &str,
) -> Result<String, String> {
    match container {
        None => {
            connection::exec_command(
                &session.channel,
                &format!("sh -c {}", connection::shell_quote(script)),
            )
            .await
        }
        Some(c) => {
            let ops = crate::remote::docker::DockerExecFileOps::new(&session.channel, c)?;
            connection::exec_command(
                &session.channel,
                &format!(
                    "docker exec {} sh -c {}",
                    ops.container,
                    connection::shell_quote(script)
                ),
            )
            .await
        }
    }
}

/// 确保远端 `~/.cc-switch/sqlite-helper` 已部署（按远端架构从内嵌资源上传），
/// 返回 helper 的绝对路径。仅首次上传（`test -x` 命中即跳过）。
async fn ensure_remote_sqlite_helper(
    session: &connection::RemoteSession,
    container: Option<&str>,
    root: &str,
) -> Result<String, String> {
    let path = format!("{root}/.cc-switch/sqlite-helper");
    let parent = format!("{root}/.cc-switch");
    // 注意：`test -x` 无论成功失败都没有 stdout，而 exec 通道不看退出码——
    // 必须显式 echo yes/no，否则永远误判「已存在」而从不真正上传。
    let probe = format!("if test -x {path}; then echo yes; else echo no; fi");
    let check_cmd = match container {
        None => format!("sh -c {}", connection::shell_quote(&probe)),
        Some(c) => {
            let ops = crate::remote::docker::DockerExecFileOps::new(&session.channel, c)?;
            format!(
                "docker exec {} sh -c {}",
                ops.container,
                connection::shell_quote(&probe)
            )
        }
    };
    if let Ok(out) = connection::exec_command(&session.channel, &check_cmd).await {
        if out.trim() == "yes" {
            return Ok(path);
        }
    }

    // 确保目录存在（首次使用时 ~/.cc-switch 可能还没建）
    let mkdir = format!("sh -c {}", connection::shell_quote(&format!("mkdir -p {parent}")));
    match container {
        None => {
            connection::exec_command(&session.channel, &mkdir).await?;
        }
        Some(c) => {
            let ops = crate::remote::docker::DockerExecFileOps::new(&session.channel, c)?;
            connection::exec_command(
                &session.channel,
                &format!("docker exec {} {}", ops.container, mkdir),
            )
            .await?;
        }
    }

    // 探测远端架构，选择内嵌二进制
    let uname = connection::exec_command(&session.channel, "uname -m").await?;
    let arch = uname.trim();
    let bytes: &'static [u8] = if arch == "x86_64" || arch == "amd64" {
        SQLITE_HELPER_X86_64
    } else if arch == "aarch64" || arch == "arm64" {
        SQLITE_HELPER_AARCH64
    } else {
        return Err(format!("不支持的远端架构（sqlite-helper 未内置）: {arch}"));
    };

    // 上传：宿主机走 SFTP；容器走 stdin 管道（base64 解码落盘）
    let tmp = format!("{path}.tmp");
    match container {
        None => {
            use tokio::io::AsyncWriteExt;
            let mut remote_file = session
                .sftp
                .create(&tmp)
                .await
                .map_err(|e| format!("创建远端 helper 临时文件失败: {e}"))?;
            remote_file
                .write_all(bytes)
                .await
                .map_err(|e| format!("写入远端 helper 失败: {e}"))?;
            remote_file
                .flush()
                .await
                .map_err(|e| format!("刷新远端 helper 失败: {e}"))?;
            drop(remote_file);
            let mv = format!(
                "sh -c {}",
                connection::shell_quote(&format!("mv {tmp} {path} && chmod +x {path}"))
            );
            connection::exec_command(&session.channel, &mv).await?;
        }
        Some(c) => {
            let ops = crate::remote::docker::DockerExecFileOps::new(&session.channel, c)?;
            let cmd = format!(
                "docker exec -i {} sh -c {}",
                ops.container,
                connection::shell_quote(&format!(
                    "base64 -d > {tmp} && mv {tmp} {path} && chmod +x {path}"
                ))
            );
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            connection::exec_command_with_stdin(&session.channel, &cmd, b64.as_bytes()).await?;
        }
    }
    Ok(path)
}

/// 执行远端 sqlite-helper 并解析 stdout JSON。
/// `params` 依次绑定到 SQL 的 `?1/?2/...`。
async fn run_sqlite_helper(
    session: &connection::RemoteSession,
    container: Option<&str>,
    helper_path: &str,
    subcmd: &str,
    db: &str,
    sql: &str,
    params: &[&str],
) -> Result<Value, String> {
    let mut inner = format!(
        "{} {} {} {}",
        connection::shell_quote(helper_path),
        subcmd,
        connection::shell_quote(db),
        connection::shell_quote(sql)
    );
    for p in params {
        inner.push(' ');
        inner.push_str(&connection::shell_quote(p));
    }
    let cmd = match container {
        None => format!("sh -c {}", connection::shell_quote(&inner)),
        Some(c) => {
            let ops = crate::remote::docker::DockerExecFileOps::new(&session.channel, c)?;
            format!(
                "docker exec -i {} sh -c {}",
                ops.container,
                connection::shell_quote(&inner)
            )
        }
    };
    let out = connection::exec_command(&session.channel, &cmd).await?;
    let trimmed = out.trim();
    // 输出可能是多行（最后一行才是 JSON）；取最后一行
    let json_line = trimmed.lines().next_back().unwrap_or(trimmed);
    let value: Value = serde_json::from_str(json_line)
        .map_err(|e| format!("解析 helper 输出失败: {e} → {json_line}"))?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("helper 执行失败")
            .to_string());
    }
    Ok(value)
}

/// 远程 Provider 连通性测试结果。
#[derive(serde::Serialize)]
pub struct RemoteProviderTestResult {
    /// 探测的 base_url（与本机测试提取逻辑一致）
    pub base_url: String,
    /// 远端 curl 返回的 HTTP 状态码（空 = 无响应/网络错误）
    pub http_code: String,
    /// 是否可达（2xx/3xx）
    pub reachable: bool,
}

/// 远程目标下测试 Provider 连通性：复用本机 `StreamCheckService::resolve_base_url`
/// 提取探测地址（官方 provider 报错，前端已隐藏其测试按钮），经 SSH 在远端执行
/// `curl -s -o /dev/null -w '%{http_code}' -m 10 <url>`——真实反映远端到 API 的网络。
#[tauri::command]
pub async fn test_remote_provider_connection(
    state: State<'_, AppState>,
    host_id: String,
    provider_id: String,
    app: String,
    container: Option<String>,
) -> Result<RemoteProviderTestResult, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    // provider 来自该远端目标自己的 SSOT（本机 DB 不参与）——与供应商面板
    // 数据源一致；否则远端场景下 provider 只存在于 SSOT，本机 DB 查不到。
    let ssot = crate::remote::providers::read_remote_providers_ssot(&target, &home, &app).await?;
    let provider = ssot
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| format!("远端供应商 {provider_id} 不存在"))?;

    let app_type =
        crate::app_config::AppType::from_str(&app).map_err(|_| format!("未知应用类型: {app}"))?;
    // 与本机连通性测试同一地址提取（官方 provider 会在此报错）
    let base_url =
        crate::services::stream_check::StreamCheckService::resolve_base_url(&app_type, provider)
            .map_err(|e| e.to_string())?;

    // 在远端执行 curl（curl 缺失/无网络时报错由 exec 输出带出）。
    // 容器名空串/空白（前端宿主机目标可能传 ""）归一化走宿主机路径，与
    // RemoteTarget::new 的空容器处理一致，避免 DockerExecFileOps 的容器名校验报错。
    let curl = format!(
        "curl -s -o /dev/null -w '%{{http_code}}' -m 10 {}",
        connection::shell_quote(&base_url)
    );
    let cmd = match container.as_deref().map(str::trim) {
        None | Some("") => format!("sh -c {}", connection::shell_quote(&curl)),
        Some(c) => {
            let ops = crate::remote::docker::DockerExecFileOps::new(&session.channel, c)?;
            format!(
                "docker exec {} sh -c {}",
                ops.container,
                connection::shell_quote(&curl)
            )
        }
    };
    let out = connection::exec_command(&session.channel, &cmd).await?;
    let http_code = out.trim().to_string();
    let reachable = http_code.starts_with('2') || http_code.starts_with('3');
    log::info!(
        "[remote] test_remote_provider_connection host={host_id} app={app} provider={provider_id} base_url={base_url} http_code={http_code:?} reachable={reachable}"
    );

    Ok(RemoteProviderTestResult {
        base_url,
        http_code,
        reachable,
    })
}

/// 主机信息（给前端的列表项；不含密码）。
#[tauri::command]
pub async fn list_remote_hosts(state: State<'_, AppState>) -> Result<Vec<RemoteHost>, String> {
    state.db.list_remote_hosts().map_err(|e| e.to_string())
}

/// 探测本机可用的 SSH 私钥（供「私钥」认证自动填充，用户无需手填路径）。
///
/// 顺序：ssh config 的 IdentityFile（按别名） → `~/.ssh/id_ed25519` → `id_rsa` → `id_ecdsa`。
#[tauri::command]
pub fn detect_local_ssh_keys(alias: Option<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |path: Option<String>| {
        if let Some(path) = path {
            let p = std::path::PathBuf::from(&path);
            if p.is_file() && !out.iter().any(|existing| existing == &path) {
                out.push(path);
            }
        }
    };

    if let Some(alias) = alias.as_deref().filter(|a| !a.trim().is_empty()) {
        push(connection::ssh_config_identity_file(alias));
    }
    if let Some(home) = dirs::home_dir() {
        for name in ["id_ed25519", "id_rsa", "id_ecdsa"] {
            push(Some(
                home.join(".ssh").join(name).to_string_lossy().to_string(),
            ));
        }
    }
    out
}

/// 一键「密码 → 密钥」：用密码登录一次，把本机公钥装到远端，再改用私钥登录。
///
/// 之后连接不再需要密码。失败时保持原记录不变（远端公钥可能已追加，但不影响密码登录）。
#[tauri::command]
pub async fn bootstrap_remote_key(
    state: State<'_, AppState>,
    host_id: String,
    password: String,
) -> Result<RemoteHost, String> {
    let mut host = load_host(&state, &host_id)?;

    // 没传密码时回退用已保存的密码
    let password = if password.trim().is_empty() {
        #[cfg(target_os = "windows")]
        {
            credentials::get_password(&host.id)?.unwrap_or_default()
        }
        #[cfg(not(target_os = "windows"))]
        {
            String::new()
        }
    } else {
        password
    };
    if password.is_empty() {
        return Err("需要先输入一次登录密码才能自动配置密钥".to_string());
    }

    // 1) 用密码建连（fresh，避免复用池里的旧连接）
    let mut probe = host.clone();
    probe.auth_method = AuthMethod::Password;
    probe.private_key_path = None;
    let session = connection::connect_fresh(&probe, Some(&password)).await?;

    // 2) 确保本机应用密钥存在（放在 CC Switch 自己的目录，不碰 ~/.ssh）
    let key_dir = crate::config::get_app_config_dir().join("ssh");
    std::fs::create_dir_all(&key_dir)
        .map_err(|e| format!("创建密钥目录失败 {}: {e}", key_dir.display()))?;
    let key_path = key_dir.join("id_ed25519");
    if !key_path.exists() {
        let output = std::process::Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-C", "cc-switch", "-q", "-f"])
            .arg(&key_path)
            .output()
            .map_err(|e| {
                format!("调用 ssh-keygen 失败（请确认已安装 OpenSSH 客户端）: {e}")
            })?;
        if !output.status.success() {
            return Err(format!(
                "生成密钥失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    let pub_path = key_dir.join("id_ed25519.pub");
    let pub_key = std::fs::read_to_string(&pub_path)
        .map_err(|e| format!("读取公钥失败 {}: {e}", pub_path.display()))?
        .trim()
        .to_string();
    if pub_key.is_empty() {
        return Err("公钥为空，无法配置".to_string());
    }

    // 3) 追加到远端 authorized_keys：去重 + 权限 700/600
    let home = host.default_home();
    let ssh_dir = format!("{home}/.ssh");
    let authorized_keys = format!("{ssh_dir}/authorized_keys");
    let script = format!(
        "mkdir -p {dir} && chmod 700 {dir} && touch {ak} && chmod 600 {ak} && \
         (grep -qxF {pubq} {ak} || printf '%s\\n' {pubq} >> {ak}) && echo CC_KEY_OK",
        dir = connection::shell_quote(&ssh_dir),
        ak = connection::shell_quote(&authorized_keys),
        pubq = connection::shell_quote(&pub_key),
    );
    let out = exec_in_target(&session, None, &script).await?;
    if !out.contains("CC_KEY_OK") {
        return Err(format!("写入远端 authorized_keys 失败: {}", out.trim()));
    }

    // 4) 用新私钥验证一次，成功才切换认证方式
    let key_path_text = key_path.to_string_lossy().to_string();
    let mut key_probe = host.clone();
    key_probe.auth_method = AuthMethod::Key;
    key_probe.private_key_path = Some(key_path_text.clone());
    connection::connect_fresh(&key_probe, None)
        .await
        .map_err(|e| format!("公钥已写入，但私钥登录验证失败: {e}"))?;

    // 5) 更新记录：改用私钥，删掉保存的密码
    host.auth_method = AuthMethod::Key;
    host.private_key_path = Some(key_path_text);
    host.save_password = false;
    host.updated_at = chrono::Utc::now().timestamp_millis();
    state
        .db
        .upsert_remote_host(&host)
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    let _ = credentials::delete_password(&host.id);
    crate::remote::forget_probed_home(&host.id);

    log::info!("[remote] 已为 {} 自动配置密钥登录", host.name);
    Ok(host)
}

/// 保存（新增或更新）远程主机；可选携带密码并写入系统钥匙串。
#[tauri::command]
pub async fn save_remote_host(
    state: State<'_, AppState>,
    host: RemoteHost,
    password: Option<String>,
) -> Result<RemoteHost, String> {
    let mut host = host;
    if host.id.trim().is_empty() {
        host.id = uuid::Uuid::new_v4().to_string();
    }
    let now = chrono::Utc::now().timestamp_millis();
    if host.created_at <= 0 {
        host.created_at = now;
    }
    host.updated_at = now;

    state
        .db
        .upsert_remote_host(&host)
        .map_err(|e| e.to_string())?;

    // 主机信息（username / host 等）可能已变 → 清掉旧 `$HOME` 探测缓存，
    // 下次连接重新探测（复用旧连接时 connection::ensure_probed_home 会补探测）。
    crate::remote::forget_probed_home(&host.id);

    // 只要提供了密码，就无条件写入系统钥匙串（DPAPI 加密），保证连接/切换可用。
    // 没有「不保存密码」选项：连接时没有地方临时输入密码，不保存只会让远端操作失败。
    // 删除密码只发生在「改用密钥认证」成功后（bootstrap_remote_key）。
    if let Some(pw) = password.as_deref().filter(|p| !p.is_empty()) {
        log::info!("[remote] 保存密码到钥匙串 id={}", host.id);
        #[cfg(target_os = "windows")]
        if let Err(e) = credentials::save_password(&host.id, pw) {
            log::error!("[remote] 钥匙串保存失败: {e}");
            return Err(e);
        }
        log::info!("[remote] 钥匙串保存成功 id={}", host.id);
    }
    Ok(host)
}

/// per-app 远端接管开关（对齐本机接管开关语义，按目标分流）：
/// - 宿主机目标（container=None）：写 `route_proxy_apps[app]`；
/// - 容器目标（container=Some）：写 `route_proxy_container_apps[container][app]`，各自独立；
/// - 开：写 DB 并确保本机代理进程运行（自动拉起）；
/// - 关：写 DB，若全无需要（本机接管 + 所有远端/容器路由全空）则自动停止本机代理进程；
/// - live 改写（路由态/直连态）由前端在开关后调用 reapply 完成。
#[tauri::command]
pub async fn set_remote_route_proxy_app(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    enabled: bool,
    container: Option<String>,
) -> Result<RemoteHost, String> {
    set_route_switch_inner(&state, &host_id, &app, enabled, container.as_deref()).await
}

/// [`set_remote_route_proxy_app`] 的核心（无 `State` 包装）：命令本体与
/// 「全部回到直连」的远端半场复用同一份写库 / 起停代理 / 对账隧道逻辑。
pub(crate) async fn set_route_switch_inner(
    state: &AppState,
    host_id: &str,
    app: &str,
    enabled: bool,
    container: Option<&str>,
) -> Result<RemoteHost, String> {
    let mut host = load_host(state, host_id)?;
    host.updated_at = chrono::Utc::now().timestamp_millis();
    if let Some(c) = container {
        let entry = host
            .route_proxy_container_apps
            .entry(c.to_string())
            .or_default();
        entry.insert(app.to_string(), enabled);
    } else if enabled {
        host.route_proxy_apps.insert(app.to_string(), true);
    } else {
        host.route_proxy_apps.insert(app.to_string(), false);
    }
    state
        .db
        .upsert_remote_host(&host)
        .map_err(|e| e.to_string())?;

    if enabled {
        if !state.proxy_service.is_running().await {
            log::info!("[remote] 开启 {app} 远端接管，自动启动本机代理进程");
            state
                .proxy_service
                .start()
                .await
                .map_err(|e| format!("开启远端接管时启动本机代理失败: {e}"))?;
        }
    } else if !any_route_consumer(state).await && state.proxy_service.is_running().await {
        log::info!("[remote] 关闭 {app} 远端接管后无任何接管/路由需要，自动停止本机代理进程");
        let _ = state.proxy_service.stop().await;
    }

    // 立即对账反向隧道（不重建连接）：开→补隧道，关→撤隧道，即时生效。
    // 先同步隧道端口（与代理实际监听端口一致），再对账。
    let _ = current_proxy_port(state.db.as_ref(), &state.proxy_service).await;
    connection::sync_tunnel_now(&host).await;

    Ok(host)
}

/// 「全部回到直连」的远端半场：把所有宿主机 / 容器上仍开着的 per-app 路由开关
/// 逐个关掉（写库 + 撤隧道），并把受影响目标器的 live 重投影回直连。
///
/// 本机半场由 `mode::controller::exit_all` 负责；本函数补齐远端 —— 否则远端
/// 目标器的路由开关仍是 true、隧道仍在、live 仍是隧道占位配置，
/// 「全部回到直连」对远端形同虚设（用户实测确认）。
pub(crate) async fn exit_all_remote_route(state: &AppState) -> Vec<String> {
    let hosts = match state.db.list_remote_hosts() {
        Ok(h) => h,
        Err(e) => return vec![format!("读取远端主机列表失败: {e}")],
    };
    let mut errors = Vec::new();
    for host in hosts {
        // 禁用的主机不操作（其接管意图已不参与判定，live 也不该在未排除故障的情况下被改写）
        if host.disabled {
            continue;
        }
        // 收集 (app, container) 待关闭项：宿主机开关 + 每个容器的开关。
        let mut pending: Vec<(String, Option<String>)> = Vec::new();
        for (app, on) in &host.route_proxy_apps {
            if *on {
                pending.push((app.clone(), None));
            }
        }
        for (container, apps) in &host.route_proxy_container_apps {
            for (app, on) in apps {
                if *on {
                    pending.push((app.clone(), Some(container.clone())));
                }
            }
        }
        if pending.is_empty() {
            continue;
        }

        // 逐个关开关（每次写库都会重算 any_route_consumer + 对账隧道）；
        // 全部关完时最后一次写库会因「全局再无路由消费者」停掉本机代理并撤该主机隧道。
        for (app, container) in &pending {
            if let Err(e) =
                set_route_switch_inner(state, &host.id, app, false, container.as_deref()).await
            {
                errors.push(format!(
                    "{} {app}{}: {e}",
                    host.name,
                    container
                        .as_deref()
                        .map(|c| format!(" 容器 {c}"))
                        .unwrap_or_default()
                ));
            }
        }

        // 把每个受影响目标器的 live 重投影回直连（开关已关，reapply 按直连写）。
        for (app, container) in &pending {
            if let Err(e) =
                reapply_remote_provider_target(state, &host.id, app, container.as_deref()).await
            {
                // reapply 失败不阻塞其余目标器：开关已关，live 缺的只是回写。
                log::warn!(
                    "[remote] 全部回到直连：{} {app} 重投影 live 失败: {e}",
                    host.name
                );
            }
        }
    }
    errors
}

/// 是否有任何「需要本机代理进程」的使用者：
/// 本机任一 app 接管 || 任一 host 任一 app 远端接管（含容器目标的 per-container 开关）。
/// pub(crate)：供 lib.rs 启动恢复时判断「远端是否有接管意图」复用。
pub(crate) async fn any_route_consumer(state: &AppState) -> bool {
    match state.db.is_live_takeover_active().await {
        Ok(true) => return true,
        Ok(false) => {}
        Err(e) => log::warn!("[remote] 检查本机接管状态失败: {e}"),
    }
    match state.db.list_remote_hosts() {
        Ok(hosts) => hosts
            .iter()
            // 禁用的主机不参与「是否有路由接管意图」判定：残留的接管意图不再自动拉起本机代理
            .filter(|h| !h.disabled)
            .any(|h| {
                h.route_proxy_apps.values().any(|&v| v)
                    || h.route_proxy_container_apps
                        .values()
                        .any(|m| m.values().any(|&v| v))
            }),
        Err(e) => {
            log::warn!("[remote] 检查远端路由意图失败: {e}");
            false
        }
    }
}

/// 删除远程主机（同时清除系统钥匙串里的密码）。
#[tauri::command]
pub async fn delete_remote_host(
    state: State<'_, AppState>,
    host_id: String,
) -> Result<bool, String> {
    let deleted = state
        .db
        .delete_remote_host(&host_id)
        .map_err(|e| e.to_string())?;
    if deleted {
        #[cfg(target_os = "windows")]
        let _ = credentials::delete_password(&host_id);
        let _ = crate::remote::current::delete_current_provider(state.db.as_ref(), &host_id);
        // 清掉该主机的 `$HOME` 探测缓存，防止残留记忆
        crate::remote::forget_probed_home(&host_id);
    }
    Ok(deleted)
}

/// 测试与远程主机的连接（认证 + SFTP 初始化），并探测远端配置是否存在。
#[tauri::command]
pub async fn test_remote_connection(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    app: String,
) -> Result<serde_json::Value, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    probe_remote(&host, &password, container.as_deref(), &app).await
}

/// 用「未保存的连接信息」直接测试 SSH 连接（新增主机场景，不需要先保存）。
#[tauri::command]
pub async fn test_remote_connection_info(
    host: String,
    port: u16,
    username: String,
    password: String,
    app: String,
    auth_method: Option<String>,
    private_key_path: Option<String>,
) -> Result<serde_json::Value, String> {
    let host_info = RemoteHost {
        id: "temp".to_string(),
        name: username.clone(),
        host,
        port,
        username,
        auth_method: if auth_method.as_deref() == Some("key") {
            AuthMethod::Key
        } else {
            AuthMethod::Password
        },
        private_key_path,
        save_password: false,
        route_through_local_proxy: false,
        route_proxy_apps: std::collections::HashMap::new(),
        route_proxy_container_apps: std::collections::HashMap::new(),
        disabled: false,
        created_at: 0,
        updated_at: 0,
    };
    probe_remote(&host_info, &password, None, &app).await
}

/// 共享探测逻辑：建连 + 探测「当前 app」主配置文件是否存在 + 检测该 app 的 CLI 安装。
///
/// 全部 per-app：不同 app 的远端主配置路径与 CLI 二进制不同
/// （claude→settings.json、codex→config.toml、opencode→opencode.json…），
/// 不再硬编码 Claude Code。
async fn probe_remote(
    host: &RemoteHost,
    password: &str,
    container: Option<&str>,
    app: &str,
) -> Result<serde_json::Value, String> {
    let session = connection::connect(host, Some(password)).await?;
    let home = host.default_home();
    let target =
        crate::remote::docker::RemoteTarget::new(&session.sftp, &session.channel, container)?;

    // 当前 app 的主配置文件是否存在（测试连接/环境检查用）
    let settings_exists = match remote_app_config_path(app, &home) {
        Some(path) => target.exists(&path).await,
        None => false,
    };

    // 通过 exec 通道检测当前 app 的 CLI 是否安装（命中哨兵 = 已安装）
    let cli_cmd = cli_installed_probe(app, container)?;
    let cli_installed = match connection::exec_command(&session.channel, &cli_cmd).await {
        Ok(out) => {
            log::info!(
                "[remote] {app} 探测 cmd={cli_cmd:?} out={out:?} found={}",
                out.contains(CLAUDE_INSTALLED_MARKER)
            );
            Some(out.contains(CLAUDE_INSTALLED_MARKER))
        }
        Err(e) => {
            log::warn!("[remote] 检测远端 {app} 安装状态失败: {e}");
            None
        }
    };

    Ok(json!({
        "connected": true,
        "home": home,
        "settingsExists": settings_exists,
        "cliInstalled": cli_installed,
    }))
}

/// 各 app 在远端的主配置文件路径（测试连接的环境检查用），
/// 与 SSOT 首次导入 / 远端切换写入的 live 路径保持一致。
fn remote_app_config_path(app: &str, home: &str) -> Option<String> {
    let path = match app {
        "claude" => format!("{home}/.claude/settings.json"),
        "codex" => format!("{home}/.codex/config.toml"),
        "gemini" => format!("{home}/.gemini/.env"),
        "grokbuild" => format!("{home}/.grok/config.toml"),
        "opencode" => format!("{home}/.config/opencode/opencode.json"),
        "openclaw" => format!("{home}/.openclaw/openclaw.json"),
        "hermes" => format!("{home}/.hermes/config.yaml"),
        "pi" => format!("{home}/.pi/agent/models.json"),
        "mcode" => format!("{home}/.minimax/config.yaml"),
        _ => return None,
    };
    Some(path)
}

/// 读取远端 `~/.claude/settings.json`（原始 JSON，供前端展示/编辑）。
/// `container` 为 Some 时读取容器内路径。
#[tauri::command]
pub async fn read_remote_settings(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<serde_json::Value, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;

    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    settings::read_remote_settings(&target, &host.default_home()).await
}

/// 从远端 live 配置移除某供应商（对齐本机 `remove_from_live_config` 语义：
/// 仅 additive 模式 app 支持——opencode 的 `provider.{id}`、openclaw 的
/// `models.providers.{id}`、hermes 的 `custom_providers` 按 name 过滤；
/// claude/codex/gemini/grok 与本机一致不支持，直接报错）。不清本机 DB。
#[tauri::command]
pub async fn remove_remote_provider_from_live(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    provider_id: String,
    container: Option<String>,
) -> Result<RemoteProvidersView, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let root = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    // live 移除成功后同步 SSOT：该供应商 live_config_managed → false（保留在候选池，
    // 对齐本机 remove_from_live_config 语义——本机也是移除 live 后把 DB 标记置 false）
    let mut ssot =
        crate::remote::providers::read_remote_providers_ssot(&target, &root, &app).await?;

    // 按 app 分支移除 live 中的供应商；Result 显式丢弃（错误已在分支内处理/返回）
    let _: Result<(), String> = match app.as_str() {
        "opencode" => {
            let config_path = format!("{root}/.config/opencode/opencode.json");
            let mut merged: Value = session
                .read_remote_text(&config_path, container.as_deref())
                .await?
                .map(|t| serde_json::from_str(&t).unwrap_or_else(|_| json!({})))
                .unwrap_or_else(|| json!({}));
            let removed = merged
                .get_mut("provider")
                .and_then(|v| v.as_object_mut())
                .is_some_and(|p| p.remove(&provider_id).is_some());
            // 对齐本机 remove_provider 语义：供应商本就不在配置里 → 静默成功（不写文件）
            if !removed {
                log::debug!("远端 opencode.json 中没有供应商 {provider_id}，跳过 live 移除");
            } else {
                let text = serde_json::to_string_pretty(&merged)
                    .map_err(|e| format!("序列化 opencode.json 失败: {e}"))?;
                session
                    .write_settings_with_backup(&config_path, &text, container.as_deref(), None)
                    .await?;
            }
            Ok(())
        }
        "openclaw" => {
            let config_path = format!("{root}/.openclaw/openclaw.json");
            let mut merged: Value = session
                .read_remote_text(&config_path, container.as_deref())
                .await?
                .map(|t| json5::from_str(&t).unwrap_or_else(|_| json!({})))
                .unwrap_or_else(|| json!({}));
            let removed = merged
                .get_mut("models")
                .and_then(|m| m.get_mut("providers"))
                .and_then(|v| v.as_object_mut())
                .is_some_and(|p| p.remove(&provider_id).is_some());
            if !removed {
                return Err(format!(
                    "远端 openclaw.json 中没有 models.providers.{provider_id}"
                ));
            }
            let text = serde_json::to_string_pretty(&merged)
                .map_err(|e| format!("序列化 openclaw.json 失败: {e}"))?;
            session
                .write_settings_with_backup(&config_path, &text, container.as_deref(), None)
                .await?;
            Ok(())
        }
        "hermes" => {
            let config_path = format!("{root}/.hermes/config.yaml");
            let mut root_yaml: serde_yaml::Value = match session
                .read_remote_text(&config_path, container.as_deref())
                .await?
            {
                Some(t) => serde_yaml::from_str(&t).unwrap_or_else(|_| serde_yaml::Value::Null),
                None => serde_yaml::Value::Null,
            };
            if !root_yaml.is_mapping() {
                return Err(format!("远端 {config_path} 不存在或不是 YAML 映射"));
            }
            let providers: Vec<serde_yaml::Value> = root_yaml
                .get("custom_providers")
                .and_then(|v| v.as_sequence())
                .cloned()
                .unwrap_or_default();
            let original_len = providers.len();
            let filtered: Vec<serde_yaml::Value> = providers
                .into_iter()
                .filter(|p| p.get("name").and_then(|n| n.as_str()) != Some(provider_id.as_str()))
                .collect();
            if filtered.len() == original_len {
                return Err(format!(
                    "远端 config.yaml 中没有 custom_providers 名为 {provider_id} 的条目"
                ));
            }
            if let Some(root_map) = root_yaml.as_mapping_mut() {
                root_map.insert(
                    serde_yaml::Value::String("custom_providers".to_string()),
                    serde_yaml::Value::Sequence(filtered),
                );
            }
            let text = serde_yaml::to_string(&root_yaml)
                .map_err(|e| format!("序列化 config.yaml 失败: {e}"))?;
            session
                .write_settings_with_backup(&config_path, &text, container.as_deref(), None)
                .await?;
            Ok(())
        }
        "pi" => {
            let config_path = format!("{root}/.pi/agent/models.json");
            let mut merged: Value = session
                .read_remote_text(&config_path, container.as_deref())
                .await?
                .map(|t| serde_json::from_str(&t).unwrap_or_else(|_| json!({})))
                .unwrap_or_else(|| json!({}));
            let removed = merged
                .get_mut("providers")
                .and_then(|v| v.as_object_mut())
                .is_some_and(|p| p.remove(&provider_id).is_some());
            if !removed {
                log::debug!("远端 models.json 中没有供应商 {provider_id}，跳过 live 移除");
            } else {
                let text = serde_json::to_string_pretty(&merged)
                    .map_err(|e| format!("序列化 models.json 失败: {e}"))?;
                session
                    .write_settings_with_backup(&config_path, &text, container.as_deref(), None)
                    .await?;
            }
            Ok(())
        }
        other => {
            return Err(format!(
                "应用 {other} 不支持从 live 配置移除（与本机语义一致）"
            ));
        }
    };

    // 同步 SSOT：live_config_managed → false（无论 live 是否真的移除了，
    // 对齐本机 remove_from_live_config 总是把 DB 标记置 false）
    if let Some(p) = ssot.providers.iter_mut().find(|p| p.id == provider_id) {
        if let Some(meta) = p.meta.as_mut() {
            meta.live_config_managed = Some(false);
        }
    }
    crate::remote::providers::write_remote_providers_ssot(&target, &root, &app, &ssot).await?;

    // 带回最新视图（复用内存 SSOT；live_ids 操作后读一次——additive 移除 live 后
    // 按钮态需要最新集合）
    let live_ids = if crate::remote::providers::is_additive_app(&app) {
        crate::remote::providers::read_remote_live_provider_ids(&target, &root, &app).await?
    } else {
        Vec::new()
    };
    build_remote_providers_view(
        &state,
        &host_id,
        &target,
        &root,
        &app,
        container.as_deref(),
        &session,
        ssot,
        live_ids,
    )
    .await
}

/// 远端供应商面板数据源：读该目标机器自己的 SSOT（首次从 live 导入），
/// 返回完整供应商列表 + 当前供应商 + additive live ID 集合（按钮态）。
#[derive(serde::Serialize)]
pub struct RemoteProvidersView {
    pub providers: Vec<crate::provider::Provider>,
    /// 非 additive：当前生效供应商（SSOT current / 切换记录 / live 兜底）；additive 为 None
    pub current_provider_id: Option<String>,
    /// additive：远端 live 中的供应商 ID 集合（isInConfig 按钮态）；其他 app 为空
    pub live_ids: Vec<String>,
    /// 该目标（宿主机/容器）per-app 接管开关是否开启（对齐本机 isProxyTakeover 语义，
    /// 供前端供应商卡片绿色高亮）
    pub route_proxy_enabled: bool,
}

/// 远端供应商面板数据源（per-target 独立）。
#[tauri::command]
pub async fn get_remote_providers(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    container: Option<String>,
    #[allow(non_snake_case)] autoImportDefault: Option<bool>,
) -> Result<RemoteProvidersView, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    // 首次（SSOT 空）从远端 live 导入（对齐本机启动导入语义）；
    // 非 additive 的 default 刷新由设置开关控制（默认开：随时可见当前机器配置）
    let (_, live_ids) = crate::remote::providers::sync_remote_live_into_ssot(
        &target,
        &home,
        &app,
        autoImportDefault.unwrap_or(true),
    )
    .await?;
    let ssot = crate::remote::providers::read_remote_providers_ssot(&target, &home, &app).await?;
    build_remote_providers_view(
        &state,
        &host_id,
        &target,
        &home,
        &app,
        container.as_deref(),
        &session,
        ssot,
        live_ids,
    )
    .await
}

/// 在已连接的会话上构建当前远端供应商视图（列表 + current + live_ids）。
///
/// 复用调用方已读到的内存 SSOT（操作命令改完直接传，不再重读文件）与
/// live_ids（get 场景来自 sync 返回值，操作场景来自写 live 后的一次读取）。
/// 前端 `setQueryData` 写入缓存，免「操作 + invalidate-refetch」的第二次
/// SSH 建连（远端正向优化）。
#[allow(clippy::too_many_arguments)]
async fn build_remote_providers_view(
    state: &AppState,
    host_id: &str,
    target: &crate::remote::docker::RemoteTarget<'_>,
    home: &str,
    app: &str,
    container: Option<&str>,
    session: &connection::RemoteSession,
    ssot: crate::remote::providers::RemoteProvidersSsot,
    live_ids: Vec<String>,
) -> Result<RemoteProvidersView, String> {
    let current_provider_id = resolve_remote_current_provider_id(
        state, host_id, target, home, app, container, session, &ssot,
    )
    .await?;
    // 把该目标器的当前供应商**完整配置**同步进本机表（路由基座）：
    // 面板显示什么、路由就能用什么——切目标/查看面板即预热路由数据，
    // 不必等用户「切换一次」才写入（否则开路由时查不到 provider_config）。
    // 仅非 additive（无「当前供应商」概念的 additive 走 live，不适用）。
    if let Some(cid) = current_provider_id.as_deref() {
        if let Some(p) = ssot.providers.iter().find(|p| p.id == cid) {
            let _ = crate::remote::current::save_current_provider(
                state.db.as_ref(),
                host_id,
                container,
                app,
                cid,
                Some(p),
            );
        }
    }
    let route_proxy_enabled = route_proxy_for_target(&load_host(state, host_id)?, container, app);
    Ok(RemoteProvidersView {
        providers: ssot.providers,
        current_provider_id,
        live_ids,
        route_proxy_enabled,
    })
}

/// 操作命令入口：读该目标 SSOT；若为空则先按本机语义从该远端 live 导入
/// （避免覆盖远端已有配置）。非空库不读 live（方案 A：容器场景省一次 docker exec）。
async fn load_remote_ssot_for_mutation<F: crate::fsops::FileOps>(
    target: &F,
    home: &str,
    app: &str,
) -> Result<crate::remote::providers::RemoteProvidersSsot, String> {
    let mut ssot = crate::remote::providers::read_remote_providers_ssot(target, home, app).await?;
    if ssot.providers.is_empty() {
        // 操作命令不刷新 default 卡（开关只管读面板），仅空库时确保有数据
        let (changed, _) =
            crate::remote::providers::sync_remote_live_into_ssot(target, home, app, false).await?;
        if changed > 0 {
            ssot = crate::remote::providers::read_remote_providers_ssot(target, home, app).await?;
        }
    }
    Ok(ssot)
}

/// 在远端目标添加供应商：写入该目标自己的 SSOT；addToLive=true（或非 additive
/// 且当前为空）时同时写入 live。对齐本机 `add_provider` 语义。
#[tauri::command]
pub async fn add_remote_provider(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    provider: crate::provider::Provider,
    add_to_live: Option<bool>,
    container: Option<String>,
) -> Result<RemoteProvidersView, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let _ = current_proxy_port(state.db.as_ref(), &state.proxy_service).await;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    // 操作命令入口：SSOT 为空才从该远端 live 导入（避免覆盖远端已有配置）；
    // 非空库不读 live（方案 A：容器场景省一次 docker exec）
    let mut ssot = load_remote_ssot_for_mutation(&target, &home, &app).await?;
    let mut provider = provider;
    let provider_id = provider.id.clone();
    let additive = crate::remote::providers::is_additive_app(&app);
    let mut write_live = false;

    if additive {
        // 对齐本机 add：additive 下 addToLive 决定是否写入 live（meta.live_config_managed）
        let add_to_live = add_to_live.unwrap_or(true);
        // 对齐本机 add：OMO/OMO-slim 添加不自动写入 live（用户显式切换才激活）
        let is_omo = app == "opencode"
            && matches!(provider.category.as_deref(), Some("omo") | Some("omo-slim"));
        if is_omo {
            write_live = false;
            if let Some(meta) = provider.meta.as_mut() {
                meta.live_config_managed = Some(false);
            }
        } else {
            if let Some(meta) = provider.meta.as_mut() {
                meta.live_config_managed = Some(add_to_live);
            }
            write_live = add_to_live;
        }
    } else {
        // 非 additive：仅当该目标尚无当前供应商才设为 current 并写 live
        // （对齐本机「DB 无 current → set_current + 写 live」）
        let has_current = ssot
            .current_provider_id
            .as_deref()
            .is_some_and(|c| ssot.providers.iter().any(|p| p.id == c));
        if !has_current {
            ssot.current_provider_id = Some(provider_id.clone());
            write_live = true;
        }
    }

    crate::remote::providers::upsert_provider(&mut ssot.providers, provider);
    crate::remote::providers::write_remote_providers_ssot(&target, &home, &app, &ssot).await?;

    if write_live {
        let p = ssot
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| "供应商写入 SSOT 后未找到".to_string())?;
        crate::remote::providers::apply_remote_provider_to_live(
            state.db.as_ref(),
            &session,
            container.as_deref(),
            &home,
            &host.name,
            &app,
            p,
            effective_route_proxy(
                &state.proxy_service,
                route_proxy_for_target(&host, container.as_deref(), &app),
            )
            .await,
            current_proxy_port(state.db.as_ref(), &state.proxy_service).await?,
        )
        .await?;
        if !additive {
            let _ = crate::remote::current::save_current_provider(
                state.db.as_ref(),
                &host_id,
                container.as_deref(),
                &app,
                &provider_id,
                Some(p),
            );
        }
    }

    // 带回最新视图（复用内存 SSOT；live_ids 操作后读一次——additive 写 live 后
    // 按钮态需要最新集合）
    let live_ids = if crate::remote::providers::is_additive_app(&app) {
        crate::remote::providers::read_remote_live_provider_ids(&target, &home, &app).await?
    } else {
        Vec::new()
    };
    build_remote_providers_view(
        &state,
        &host_id,
        &target,
        &home,
        &app,
        container.as_deref(),
        &session,
        ssot,
        live_ids,
    )
    .await
}

/// 编辑远端目标的供应商：更新 SSOT 记录；若该供应商在生效位置
/// （非 additive current / additive live_config_managed=true）则重写远端 live
/// （对齐本机 `update_provider` 语义）。additive 改名且原名已在 live 中时报错。
#[tauri::command]
pub async fn update_remote_provider(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    provider: crate::provider::Provider,
    original_id: Option<String>,
    container: Option<String>,
    editor_save: Option<crate::services::provider::EditorSave>,
) -> Result<RemoteProvidersView, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let _ = current_proxy_port(state.db.as_ref(), &state.proxy_service).await;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    // 操作命令入口：SSOT 为空才从该远端 live 导入（避免覆盖远端已有配置）；
    // 非空库不读 live（方案 A：容器场景省一次 docker exec）
    let mut ssot = load_remote_ssot_for_mutation(&target, &home, &app).await?;
    let original_id = original_id.unwrap_or_else(|| provider.id.clone());
    let existing = ssot.providers.iter().find(|p| p.id == original_id).cloned();
    let mut provider = provider;
    let additive = crate::remote::providers::is_additive_app(&app);

    // 编辑器保存（claude/codex/gemini/grokbuild）：与本地同一套 `plan_save` 拆分行内容和
    // 「相对 live 的全局改动」。行里只留关键字段/独有字段，其余键（permissions、hooks、
    // approval_policy、.env 的全局变量等用户设置）的增删改要写回远端 live——本地编辑器
    // 就是这么做的，远端不能少这一半。
    enum RemoteEditorPlan {
        Claude(crate::services::provider::claude_editor::EditorPlan),
        Codex(crate::services::provider::codex_editor::CodexEditorPlan),
        Gemini(crate::services::provider::gemini_editor::GeminiEditorPlan),
        Grok(crate::services::provider::grok_editor::GrokEditorPlan),
    }
    let stored_settings = existing.as_ref().map(|p| &p.settings_config);
    let editor_plan = match (app.as_str(), editor_save.as_ref()) {
        ("claude", Some(editor)) => {
            let plan = crate::services::provider::claude_editor::plan_save(
                stored_settings,
                &provider.settings_config,
                &editor.base,
            )
            .map_err(|e| e.to_string())?;
            provider.settings_config = plan.row_settings.clone();
            Some(RemoteEditorPlan::Claude(plan))
        }
        ("codex", Some(editor)) => {
            let origin = crate::services::provider::codex_editor::Origin::row(
                stored_settings.unwrap_or(&provider.settings_config),
            )
            .map_err(|e| e.to_string())?;
            let plan = crate::services::provider::codex_editor::plan_save(
                stored_settings,
                &provider.settings_config,
                &editor.base,
                &origin,
                crate::services::provider::codex_direct::is_official(&provider),
                provider.uses_proxy_injected_oauth(),
                editor.on_conflict,
            )
            .map_err(|e| e.to_string())?;
            provider.settings_config = plan.row_settings.clone();
            Some(RemoteEditorPlan::Codex(plan))
        }
        ("gemini", Some(editor)) => {
            let plan = crate::services::provider::gemini_editor::plan_save(
                stored_settings,
                &provider.settings_config,
                &editor.base,
                editor.on_conflict,
            )
            .map_err(|e| e.to_string())?;
            provider.settings_config = plan.row_settings.clone();
            Some(RemoteEditorPlan::Gemini(plan))
        }
        ("grokbuild", Some(editor)) => {
            let plan = crate::services::provider::grok_editor::plan_save(
                stored_settings,
                &provider.settings_config,
                &editor.base,
                crate::services::provider::grok_direct::is_official(&provider),
                editor.on_conflict,
            )
            .map_err(|e| e.to_string())?;
            provider.settings_config = plan.row_settings.clone();
            Some(RemoteEditorPlan::Grok(plan))
        }
        _ => None,
    };

    // 对齐本机 update：非 additive 不允许修改供应商 id（provider/mod.rs:2620）
    if !additive && original_id != provider.id {
        return Err(format!(
            "应用 {app} 不支持修改供应商 id（{original_id} → {}）",
            provider.id
        ));
    }

    if additive && original_id != provider.id {
        // 对齐本机 update 改名（provider/mod.rs:2649-2677）：原名已在 live 中则禁止改名
        // （改 id 会破坏 live 引用）；新名已存在于 live 中也禁止。
        let live_ids =
            crate::remote::providers::read_remote_live_provider_ids(&target, &home, &app).await?;
        if live_ids.iter().any(|id| id == &original_id) {
            return Err(format!(
                "供应商 '{original_id}' 已写入远端 live 配置，不支持改名（请先移除再添加）"
            ));
        }
        if live_ids.iter().any(|id| id == &provider.id) {
            return Err(format!("供应商 '{}' 已存在于远端 live 配置中", provider.id));
        }
    }

    // 当前供应商判定必须与面板（build_remote_providers_view）同源，否则会出现
    // 「面板上高亮的供应商编辑后保存，远端 live 却没变」：老配置（从 live 导入、
    // 从未经本应用切换）的 SSOT current 是空的，当前项只存在于持久化记录里。
    // 这里只读同步数据（持久化记录 → SSOT current），不 await 远端解析——查看
    // 面板时 build_remote_providers_view 已把解析结果（含 live base_url 兜底）
    // 落库，persisted 即为最新结论。
    let persisted_current = current_provider_id_from_persisted(
        &state,
        &host_id,
        container.as_deref(),
        &app,
        &ssot,
    )?;
    let is_current = persisted_current.as_deref() == Some(original_id.as_str());
    log::info!(
        "[remote] update {app} original_id={original_id} resolved_current={:?} ssot_current={:?} is_current={is_current}",
        persisted_current.as_deref(),
        ssot.current_provider_id.as_deref()
    );
    let need_live_rewrite = if additive {
        // 对齐本机 check_live_config_exists（provider/mod.rs:2434）：以 live 文件为唯一
        // 事实来源——读远端 live 确认该供应商是否在 live 中，而非只信 SSOT 标记。
        // 这样从 live 导入的（无标记）/ 标记丢失的供应商编辑也能正确同步；
        // 纯 SSOT 候选（未启用）编辑不自动写入 live。
        let live_ids =
            crate::remote::providers::read_remote_live_provider_ids(&target, &home, &app).await?;
        let found = live_ids.iter().any(|id| id == original_id.as_str());
        log::info!(
            "[remote] update {app} additive original_id={original_id} live_ids={live_ids:?} found={found}"
        );
        found
    } else {
        is_current
    };
    // additive：标记写回判定结果（SSOT 标记 = live 的镜像）
    if additive {
        match provider.meta.as_mut() {
            Some(meta) => meta.live_config_managed = Some(need_live_rewrite),
            None => {
                if need_live_rewrite {
                    provider.meta = Some(crate::provider::ProviderMeta {
                        live_config_managed: Some(true),
                        ..Default::default()
                    });
                }
            }
        }
    } else if is_current {
        // 非 additive：编辑当前供应商后 current 指向新 id
        ssot.current_provider_id = Some(provider.id.clone());
    }

    let provider_id_after = provider.id.clone();

    crate::remote::providers::upsert_provider(&mut ssot.providers, provider);
    // additive 改名：移除旧 id 条目（对齐本机 update 改名后删除原 id）
    if original_id != provider_id_after {
        ssot.providers.retain(|p| p.id != original_id);
    }
    crate::remote::providers::write_remote_providers_ssot(&target, &home, &app, &ssot).await?;

    if need_live_rewrite {
        let p = ssot
            .providers
            .iter()
            .find(|p| p.id == provider_id_after)
            .ok_or_else(|| "编辑后的供应商未找到".to_string())?;
        crate::remote::providers::apply_remote_provider_to_live(
            state.db.as_ref(),
            &session,
            container.as_deref(),
            &home,
            &host.name,
            &app,
            p,
            effective_route_proxy(
                &state.proxy_service,
                route_proxy_for_target(&host, container.as_deref(), &app),
            )
            .await,
            current_proxy_port(state.db.as_ref(), &state.proxy_service).await?,
        )
        .await?;
        let _ = crate::remote::current::save_current_provider(
            state.db.as_ref(),
            &host_id,
            container.as_deref(),
            &app,
            &provider_id_after,
            Some(p),
        );
    }

    // 编辑器保存的全局改动：关键字段已按供应商语义写过，这里只把「不归供应商管」的
    // 键（permissions、hooks、approval_policy、.env 的全局变量……）的增删改应用到
    // 远端 live，和本地编辑器保存逐键一致。
    if let (Some(plan), Some(editor)) = (editor_plan.as_ref(), editor_save.as_ref()) {
        use crate::services::provider::gemini_editor;
        match plan {
            RemoteEditorPlan::Claude(plan) => {
                if !plan.changes().is_empty() {
                    let settings_path = crate::remote::settings::remote_settings_path(&home);
                    let path = std::path::Path::new(&settings_path);
                    let pre = session
                        .read_remote_text(&settings_path, container.as_deref())
                        .await?;
                    let bytes = crate::services::provider::claude_editor::apply_editor_plan(
                        path,
                        pre.as_deref().map(str::as_bytes),
                        plan,
                        editor.on_conflict,
                        None,
                    )
                    .map_err(|e| format!("远端 settings.json 改写失败: {e}"))?;
                    let text =
                        String::from_utf8(bytes).map_err(|_| "改写结果不是 UTF-8".to_string())?;
                    session
                        .write_settings_with_backup(
                            &settings_path,
                            &text,
                            container.as_deref(),
                            None,
                        )
                        .await?;
                }
            }
            RemoteEditorPlan::Codex(plan) => {
                if !plan.edits.is_empty() {
                    let path_str = format!("{home}/.codex/config.toml");
                    let path = std::path::Path::new(&path_str);
                    let pre = session.read_remote_text(&path_str, container.as_deref()).await?;
                    let mut doc =
                        crate::live::patch::toml::parse(path, pre.as_deref().map(str::as_bytes))
                            .map_err(|e| format!("远端 config.toml 解析失败: {e}"))?;
                    plan.edits
                        .apply_to(path, &mut doc)
                        .map_err(|e| format!("远端 config.toml 改写失败: {e}"))?;
                    session
                        .write_settings_with_backup(
                            &path_str,
                            &doc.to_string(),
                            container.as_deref(),
                            None,
                        )
                        .await?;
                }
            }
            RemoteEditorPlan::Gemini(plan) => {
                if plan.edits.touches_env() {
                    let path_str = format!("{home}/.gemini/.env");
                    let path = std::path::Path::new(&path_str);
                    let pre = session.read_remote_text(&path_str, container.as_deref()).await?;
                    let bytes = gemini_editor::EnvWrite {
                        edits: Some(&plan.edits),
                        key_fields: None,
                    }
                    .apply(path, pre.as_deref().map(str::as_bytes))
                    .map_err(|e| format!("远端 .env 改写失败: {e}"))?;
                    let text =
                        String::from_utf8(bytes).map_err(|_| "改写结果不是 UTF-8".to_string())?;
                    session
                        .write_settings_with_backup(&path_str, &text, container.as_deref(), None)
                        .await?;
                }
                if plan.edits.touches_settings() {
                    let path_str = format!("{home}/.gemini/settings.json");
                    let path = std::path::Path::new(&path_str);
                    let pre = session.read_remote_text(&path_str, container.as_deref()).await?;
                    let bytes = gemini_editor::SettingsWrite {
                        edits: Some(&plan.edits),
                        key_fields: None,
                    }
                    .apply(path, pre.as_deref().map(str::as_bytes))
                    .map_err(|e| format!("远端 settings.json 改写失败: {e}"))?;
                    let text =
                        String::from_utf8(bytes).map_err(|_| "改写结果不是 UTF-8".to_string())?;
                    session
                        .write_settings_with_backup(&path_str, &text, container.as_deref(), None)
                        .await?;
                }
            }
            RemoteEditorPlan::Grok(plan) => {
                if !plan.edits.is_empty() {
                    let path_str = format!("{home}/.grok/config.toml");
                    let path = std::path::Path::new(&path_str);
                    let pre = session.read_remote_text(&path_str, container.as_deref()).await?;
                    let mut doc =
                        crate::live::patch::toml::parse(path, pre.as_deref().map(str::as_bytes))
                            .map_err(|e| format!("远端 config.toml 解析失败: {e}"))?;
                    plan.edits
                        .apply_to(path, &mut doc)
                        .map_err(|e| format!("远端 config.toml 改写失败: {e}"))?;
                    session
                        .write_settings_with_backup(
                            &path_str,
                            &doc.to_string(),
                            container.as_deref(),
                            None,
                        )
                        .await?;
                }
            }
        }
    }

    // 带回最新视图（复用内存 SSOT；live_ids 操作后读一次——additive 写 live 后
    // 按钮态需要最新集合）
    let live_ids = if crate::remote::providers::is_additive_app(&app) {
        crate::remote::providers::read_remote_live_provider_ids(&target, &home, &app).await?
    } else {
        Vec::new()
    };
    build_remote_providers_view(
        &state,
        &host_id,
        &target,
        &home,
        &app,
        container.as_deref(),
        &session,
        ssot,
        live_ids,
    )
    .await
}

/// 删除远端目标的供应商：从 SSOT 移除；非 additive 的当前供应商拒绝删除
/// （对齐本机 delete 语义）；additive 且该供应商在 live 中时先移除 live。
#[tauri::command]
pub async fn delete_remote_provider(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    provider_id: String,
    container: Option<String>,
) -> Result<RemoteProvidersView, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    // 操作命令入口：SSOT 为空才从该远端 live 导入（避免覆盖远端已有配置）；
    // 非空库不读 live（方案 A：容器场景省一次 docker exec）
    let mut ssot = load_remote_ssot_for_mutation(&target, &home, &app).await?;
    let additive = crate::remote::providers::is_additive_app(&app);

    // 与面板同源判定当前供应商（持久化记录 → SSOT current），避免删掉
    // 面板上高亮、但 SSOT current 为空（从 live 导入）的当前供应商。
    let current_id = current_provider_id_from_persisted(
        &state,
        &host_id,
        container.as_deref(),
        &app,
        &ssot,
    )?;
    if !additive && current_id.as_deref() == Some(provider_id.as_str()) {
        return Err(format!(
            "当前生效的供应商 '{provider_id}' 不能删除，请先切换其他供应商"
        ));
    }

    // additive：若该供应商在 live 中，先移除（对齐本机 delete：additive 删除会同时移除 live）
    if additive {
        let live_ids =
            crate::remote::providers::read_remote_live_provider_ids(&target, &home, &app).await?;
        if live_ids.iter().any(|id| id == &provider_id) {
            remove_remote_provider_from_live(
                state.clone(),
                host_id.clone(),
                app.clone(),
                provider_id.clone(),
                container.clone(),
            )
            .await?;
        }
    }

    ssot.providers.retain(|p| p.id != provider_id);
    crate::remote::providers::write_remote_providers_ssot(&target, &home, &app, &ssot).await?;

    // 清理该远端「当前供应商」持久化记录（对齐本机 delete 后前端清 current）
    let _ = crate::remote::current::delete_current_provider_for_app(
        state.db.as_ref(),
        &host_id,
        container.as_deref(),
        &app,
    );

    // 带回最新视图（复用内存 SSOT；live_ids 操作后读一次——additive 写 live 后
    // 按钮态需要最新集合）
    let live_ids = if crate::remote::providers::is_additive_app(&app) {
        crate::remote::providers::read_remote_live_provider_ids(&target, &home, &app).await?
    } else {
        Vec::new()
    };
    build_remote_providers_view(
        &state,
        &host_id,
        &target,
        &home,
        &app,
        container.as_deref(),
        &session,
        ssot,
        live_ids,
    )
    .await
}

/// 删除供应商后清理远端「当前供应商」持久化记录（remote_current_providers.json）。
/// 远端 live 文件不动（对齐本机 delete 语义：本机 delete 也只删 DB 不回写 live）。
#[tauri::command]
pub async fn clear_remote_provider_record(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    container: Option<String>,
) -> Result<(), String> {
    let _ = state;
    crate::remote::current::delete_current_provider_for_app(
        state.db.as_ref(),
        &host_id,
        container.as_deref(),
        &app,
    )
}

/// 对远程主机执行供应商切换：将该远端目标（宿主机/容器）SSOT 中保存的供应商定义
/// 原子写回远端对应 app 的 live 文件，返回「生效方式」报告。
///
/// per-target 独立：供应商定义取自该目标自己的 `~/.cc-switch/providers/{app}.json`
/// （SSOT），本机 DB 不参与。SSOT 为空时先按本机语义从远端 live 导入
/// （additive 幂等同步 / 非 additive 空库导入 default）。
#[tauri::command]
pub async fn switch_remote_provider(
    state: State<'_, AppState>,
    host_id: String,
    provider_id: String,
    app: String,
    container: Option<String>,
) -> Result<EffectReport, String> {
    switch_remote_provider_target(
        &state,
        &host_id,
        &provider_id,
        &app,
        container.as_deref(),
        false, // 单机切换：不自动从本机库兜底，保持原「远端需已配置」严格语义
        None,  // 单机切换不涉及「Provider 池来源」
    )
    .await
}

/// 从「Provider 池来源」取供应商完整定义。
/// - `source_host_id = None`：本机 DB；
/// - `Some(h)`：那台宿主机（`source_container` 为其容器，None 为宿主机本体）自己的 SSOT。
async fn provider_from_pool(
    state: &AppState,
    source_host_id: Option<&str>,
    source_container: Option<&str>,
    app: &str,
    provider_id: &str,
) -> Option<crate::provider::Provider> {
    let Some(source_host_id) = source_host_id else {
        return state
            .db
            .as_ref()
            .get_provider_by_id(provider_id, app)
            .ok()
            .flatten();
    };
    let host = load_host(state, source_host_id).ok()?;
    let password = resolve_password(&host).ok()?;
    let session = connection::connect(&host, Some(&password)).await.ok()?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        source_container,
    )
    .ok()?;
    let ssot = load_remote_ssot_for_mutation(&target, &home, app)
        .await
        .ok()?;
    ssot.providers.into_iter().find(|p| p.id == provider_id)
}

/// 单个落点（宿主机 或 宿主机下的容器）的一次远程切换。被单机命令
/// `switch_remote_provider` 与批量命令 `broadcast_switch_provider` 共用，
/// 保证「单机切换」与「广播切换」产出逐字节一致。
async fn switch_remote_provider_target(
    state: &AppState,
    host_id: &str,
    provider_id: &str,
    app: &str,
    container: Option<&str>,
    // 单机切换传 false（远端未配置该 Provider 即严格报错）；
    // 批量广播传 true（远端没有时从「Provider 池来源」取定义写入远端再切换）。
    allow_local_fallback: bool,
    // 「Provider 池来源」：None = 本机 DB；Some((host_id, container)) = 那台机器自己的 SSOT。
    pool_source: Option<(&str, Option<&str>)>,
) -> Result<EffectReport, String> {
    let host = load_host(state, host_id)?;
    let password = resolve_password(&host)?;
    let _ = current_proxy_port(state.db.as_ref(), &state.proxy_service).await;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();

    let target =
        crate::remote::docker::RemoteTarget::new(&session.sftp, &session.channel, container)?;

    // 首次访问（SSOT 空）时从远端 live 导入（对齐本机启动导入语义）
    let mut ssot = load_remote_ssot_for_mutation(&target, &home, app).await?;
    // 单机切换（switch_remote_provider）：要求远端 SSOT 已有该 Provider，不存在即报错；
    // 批量广播（broadcast_switch_provider）：远端没有时，从「Provider 池来源」取完整定义
    // 写入远端 SSOT（真正「一处配置、多处生效」），再切换。池来源由前端选择：本机 DB（默认）
    // 或某台宿主机/容器自己的 SSOT（`provider_from_pool`）。
    let mut provider = ssot.providers.iter().find(|p| p.id == provider_id).cloned();
    if provider.is_none() && allow_local_fallback {
        // 批量广播：远端没有该 Provider 时，从「Provider 池来源」取定义写入远端再切换。
        // 单机切换（allow_local_fallback=false）不走这里 → 保持原「严格报错」行为不变。
        //
        // 「Provider 池来源」：默认本机 DB；批量面板选了某台机器/容器时，从它的 SSOT 取。
        if let Some(p) =
            provider_from_pool(state, pool_source.map(|(h, _)| h), pool_source.and_then(|(_, c)| c), app, provider_id)
                .await
        {
            // upsert 进远端 SSOT：无则追加，有则覆盖（以「池来源」的定义为标准）
            if let Some(existing) = ssot.providers.iter_mut().find(|p| p.id == provider_id) {
                *existing = p.clone();
            } else {
                ssot.providers.push(p.clone());
            }
            if let Err(e) =
                crate::remote::providers::write_remote_providers_ssot(&target, &home, app, &ssot)
                    .await
            {
                log::warn!("[remote] 广播时写入远端 SSOT 失败 host_id={host_id}: {e}");
            }
            provider = Some(p);
        }
    }
    let provider = provider.ok_or_else(|| {
        format!(
            "供应商 '{provider_id}' 不在远端「{}」的配置中，请先在远端面板添加后再切换",
            host.name
        )
    })?;

    let report = crate::remote::providers::apply_remote_provider_to_live(
        state.db.as_ref(),
        &session,
        container,
        &home,
        &host.name,
        app,
        &provider,
        effective_route_proxy(
            &state.proxy_service,
            route_proxy_for_target(&host, container, app),
        )
        .await,
        current_proxy_port(state.db.as_ref(), &state.proxy_service).await?,
    )
    .await?;

    // 切换成功即持久化「该远端当前生效供应商」：SSOT current（非 additive）+
    // 本地记录（remote_current_providers.json）。与原生 cc switch 的「当前供应商」
    // 语义一致（判断当前不靠 base_url 匹配）。
    if !crate::remote::providers::is_additive_app(app) {
        let mut ssot = ssot;
        ssot.current_provider_id = Some(provider_id.to_string());
        if let Err(e) =
            crate::remote::providers::write_remote_providers_ssot(&target, &home, app, &ssot).await
        {
            log::warn!("[remote] 写回远端 SSOT current 失败 host_id={host_id}: {e}");
        }
    } else {
        // additive：切换（启用）已把供应商写入 live，SSOT 标记同步为 true
        // （live 是唯一事实来源，标记只作镜像——对齐本机 set_provider_live_config_managed）
        let mut ssot = ssot;
        if let Some(p) = ssot.providers.iter_mut().find(|p| p.id == provider_id) {
            match p.meta.as_mut() {
                Some(meta) => meta.live_config_managed = Some(true),
                None => {
                    p.meta = Some(crate::provider::ProviderMeta {
                        live_config_managed: Some(true),
                        ..Default::default()
                    });
                }
            }
        }
        if let Err(e) =
            crate::remote::providers::write_remote_providers_ssot(&target, &home, app, &ssot).await
        {
            log::warn!("[remote] 切换后同步 SSOT 标记失败 host_id={host_id}: {e}");
        }
    }
    if let Err(e) = crate::remote::current::save_current_provider(
        state.db.as_ref(),
        host_id,
        container.as_deref(),
        app,
        provider_id,
        Some(&provider),
    ) {
        log::warn!("[remote] 持久化当前供应商失败 host_id={host_id}: {e}");
    }

    // 切换后重投影远端 MCP：codex/gemini/grok 等的 MCP 与 live 同文件，
    // 整文件覆盖 live 后要把远端 SSOT 里已启用的 MCP 投影回去，否则切换后 MCP 失效。
    // claude 的 MCP 在独立文件（~/.claude.json），该函数对它自然 no-op。
    if let Err(e) = crate::remote::mcp::reproject_remote_mcp_for_app(&target, &home, app).await {
        // 失败不阻断切换（下次切换 / 任一 MCP 启停会重投影自愈）
        log::warn!("[remote] 切换后 MCP 重投影失败 host_id={host_id} app={app}: {e}");
    }

    // 隧道未建立（warnings 非空）：接管实际未生效，回退开关状态，避免 UI 显示已开
    revert_route_switch_on_tunnel_failure(state, &report, host_id, container, app).await;

    // 直接带上前端需要的当前供应商 id，避免前端再调一次 get_remote_current_provider
    let mut report = report;
    report.current_provider_id = Some(provider_id.to_string());

    Ok(report)
}

/// 一个广播落点：宿主机 或 宿主机下的某个容器
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSwitchTarget {
    pub host_id: String,
    /// None = 宿主机账号本体；Some(c) = 该宿主机下的容器
    pub container: Option<String>,
}

/// 批量切换：把同一个 Provider 应用到多个落点（宿主机/容器）。
/// 与单机切换语义完全一致，只是循环多落点并聚合每个落点的成功/失败。
/// 失败不阻断其它落点（某台连不上/切失败，其余照常）。
///
/// 逐台进度通过事件 `broadcast-progress` 推送给前端（主窗口监听可靠），
/// payload = `RemoteSwitchResult`；全部结束后再 emit `broadcast-progress-done`
/// （带完整结果数组），让前端能做最终态收尾。
#[tauri::command]
pub async fn broadcast_switch_provider(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    targets: Vec<RemoteSwitchTarget>,
    provider_id: String,
    app_type: String,
    // 「Provider 池来源」宿主机 id；None = 本机 DB
    source_host_id: Option<String>,
    // 池来源为该宿主机下的容器时（None = 宿主机本体）
    source_container: Option<String>,
) -> Result<Vec<RemoteSwitchResult>, String> {
    let mut out = Vec::with_capacity(targets.len());
    for t in &targets {
        // 主机名 + 主机 id 一起返回（区分重名容器）：不同服务器的容器名可能重复，
        // 只靠名字无法区分，必须带唯一 id。名字用于展示，id 用于绝对唯一标识。
        let host_meta = state.db.get_remote_host(&t.host_id).ok().flatten();
        let host_name = host_meta
            .as_ref()
            .map(|h| h.name.clone())
            .unwrap_or_else(|| t.host_id.clone());
        let label = match t.container.as_deref() {
            Some(c) => format!("{} / {}", host_name, c),
            None => host_name.clone(),
        };
        let result = switch_remote_provider_target(
            &state,
            &t.host_id,
            &provider_id,
            &app_type,
            t.container.as_deref(),
            true, // 广播：远端没有该 Provider 时从「池来源」写入并切换（真正「一处配置、多处生效」）
            source_host_id
                .as_deref()
                .map(|h| (h, source_container.as_deref())),
        )
        .await;
        let item = match result {
            Ok(report) => RemoteSwitchResult {
                host_id: t.host_id.clone(),
                host_name,
                container: t.container.clone(),
                label,
                ok: true,
                provider_name: report.provider_name,
                error: None,
            },
            Err(e) => RemoteSwitchResult {
                host_id: t.host_id.clone(),
                host_name,
                container: t.container.clone(),
                label,
                ok: false,
                provider_name: String::new(),
                error: Some(e),
            },
        };
        out.push(item.clone());
        // 逐台进度：每切完一台立即发给前端，实时更新对应落点状态
        let _ = app.emit("broadcast-progress", &item);
    }
    // 全部完成：把完整结果一次性收尾（前端据此标记结束）
    let _ = app.emit("broadcast-progress-done", &out);
    Ok(out)
}

/// 批量切换单个落点的结果
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSwitchResult {
    pub host_id: String,
    /// 主机名（展示用，查不到回退 host_id）
    pub host_name: String,
    pub container: Option<String>,
    /// 展示名（宿主机 或 宿主机/容器）
    pub label: String,
    pub ok: bool,
    pub provider_name: String,
    pub error: Option<String>,
}

/// 重新应用该远端目标「当前生效供应商」到 live（对齐本机「开关即生效」语义）。
/// 用于「走本机路由」开关开启/关闭时，把当前供应商按新意图立即重写 live，
/// 无需用户再手动切一次供应商（含容器网络探测/DNAT 下发）。
#[tauri::command]
pub async fn reapply_remote_provider(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    container: Option<String>,
) -> Result<EffectReport, String> {
    reapply_remote_provider_target(&state, &host_id, &app, container.as_deref()).await
}

/// [`reapply_remote_provider`] 的核心（无 `State` 包装）：命令本体与
/// 「全部回到直连」的远端半场复用。
pub(crate) async fn reapply_remote_provider_target(
    state: &AppState,
    host_id: &str,
    app: &str,
    container: Option<&str>,
) -> Result<EffectReport, String> {
    // additive 无「当前供应商」概念且不走路由改写，直接返回空报告
    if crate::remote::providers::is_additive_app(app) {
        return Ok(EffectReport {
            target: String::new(),
            provider_name: String::new(),
            current_provider_id: None,
            conflicts_cleaned: 0,
            notes: Vec::new(),
            warnings: Vec::new(),
        });
    }
    let host = load_host(state, host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container,
    )?;

    let ssot = load_remote_ssot_for_mutation(&target, &home, app).await?;
    // 当前供应商：SSOT current 优先，fallback 本地持久化记录
    let provider_id = ssot
        .current_provider_id
        .clone()
        .or_else(|| {
            crate::remote::current::get_current_provider(
                state.db.as_ref(),
                host_id,
                container,
                app,
            )
            .ok()
            .flatten()
        })
        .ok_or_else(|| {
            format!(
                "远端「{}」的 {app} 当前没有生效供应商，无需重新应用",
                host.name
            )
        })?;
    let provider = ssot
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned()
        .ok_or_else(|| {
            format!(
                "当前供应商 '{provider_id}' 不在远端「{}」的配置中",
                host.name
            )
        })?;

    let report = crate::remote::providers::apply_remote_provider_to_live(
        state.db.as_ref(),
        &session,
        container,
        &home,
        &host.name,
        app,
        &provider,
        effective_route_proxy(
            &state.proxy_service,
            route_proxy_for_target(&host, container, app),
        )
        .await,
        current_proxy_port(state.db.as_ref(), &state.proxy_service).await?,
    )
    .await?;

    // 重新应用后同步持久化该远端当前供应商（含完整 provider_config）——
    // 老迁移行（仅 provider_id、无配置）或通用配置变更后，重开接管 / 重新应用即补齐，
    // 本机代理按远端路由时才能取到 base_url 与密钥。
    if let Err(e) = crate::remote::current::save_current_provider(
        state.db.as_ref(),
        host_id,
        container,
        app,
        &provider_id,
        Some(&provider),
    ) {
        log::warn!("[remote] 重新应用后持久化当前供应商失败 host_id={host_id}: {e}");
    }

    // 重新应用后重投影远端 MCP（与切换同一语义：live 被整文件覆盖后补回已启用项）。
    if let Ok(target) =
        crate::remote::docker::RemoteTarget::new(&session.sftp, &session.channel, container)
    {
        if let Err(e) =
            crate::remote::mcp::reproject_remote_mcp_for_app(&target, &home, app).await
        {
            log::warn!("[remote] 重新应用后 MCP 重投影失败 host_id={host_id} app={app}: {e}");
        }
    }

    // 隧道未建立（warnings 非空）：接管实际未生效，回退开关状态，避免 UI 显示已开
    revert_route_switch_on_tunnel_failure(state, &report, host_id, container, app).await;

    let mut report = report;
    report.current_provider_id = Some(provider_id);
    Ok(report)
}

/// 隧道未建立（report.warnings 非空）时回退该目标的远端接管开关（DB 置 false），
/// 避免 UI 显示已开但实际按直连写入。reapply 与 switch 共用，行为一致。
async fn revert_route_switch_on_tunnel_failure(
    state: &AppState,
    report: &crate::remote::effect::EffectReport,
    host_id: &str,
    container: Option<&str>,
    app: &str,
) {
    if report.warnings.is_empty() {
        return;
    }
    let Ok(Some(mut host)) = state.db.get_remote_host(host_id) else {
        return;
    };
    if let Some(c) = container {
        if let Some(m) = host.route_proxy_container_apps.get_mut(c) {
            m.insert(app.to_string(), false);
        }
    } else {
        host.route_proxy_apps.insert(app.to_string(), false);
    }
    host.updated_at = chrono::Utc::now().timestamp_millis();
    if let Err(e) = state.db.upsert_remote_host(&host) {
        log::warn!("[remote] 回退远端接管开关失败: {e}");
    }
}

/// 扫描远端 shell 配置中的冲突环境变量（ANTHROPIC_* 名单）。
#[tauri::command]
pub async fn scan_remote_env_conflicts(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Vec<crate::remote::env_clean::RemoteEnvConflict>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::env_clean::scan_remote_env_conflicts(&target, &host.default_home()).await
}

/// 读取远端「当前生效」的本地供应商 id（per-app）。
/// 读取远端「当前生效」的供应商 id（per-app，per-target 独立）。
///
/// 判定顺序（均以该远端目标自己的 SSOT 为准，本机 DB 不参与）：
/// 1. 本应用切换时持久化的记录（`~/.cc-switch/remote_current_providers.json`）；
/// 2. 非 additive：SSOT 的 `current_provider_id`（对齐本机 DB `is_current`）；
/// 3. 兜底：读远端 live 按 base_url 匹配 SSOT 供应商（从未经本应用切换的老配置）。
///
/// 用于目标选择器：选中服务器后，主界面供应商列表的当前高亮取自远端。

/// 更新远端 SSOT 中供应商的排序索引。
#[tauri::command]
pub async fn update_remote_provider_sort_order(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    app: String,
    updates: Vec<crate::services::ProviderSortUpdate>,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::providers::update_remote_provider_sort_order(
        &target,
        &host.default_home(),
        &app,
        updates,
    )
    .await?;
    Ok(true)
}

/// 更新远端 SSOT 中供应商的元数据（用量查询配置、备注等）。
#[tauri::command]
pub async fn update_remote_provider_meta(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    app: String,
    provider_id: String,
    meta: crate::provider::ProviderMeta,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::providers::update_remote_provider_meta(
        &target,
        &host.default_home(),
        &app,
        &provider_id,
        meta,
    )
    .await?;
    Ok(true)
}

/// 远端供应商编辑器底部 JSON 的显示内容：切到这个供应商之后**远端** live 会是什么样。
///
/// 与本地 `get_provider_editor_view` 同源，但只依赖远端数据：
/// - claude / gemini：完全复用本地的纯投影器（读远端 live / `.env` / `settings.json`）；
/// - codex / grokbuild：本地投影依赖本机 DB（官方账号、模型目录、设备签名）。远端在编辑
///   「当前供应商」（或新增）时直接显示远端 live 本身（此时 live 就是切换后的样子）；
///   编辑非当前供应商时回退到行本身（与未接编辑器时一致）。判定依据是前端传入的
///   `providerId` 是否等于远端当前供应商，而不是「当前供应商是否存在」。
#[tauri::command]
pub async fn get_remote_provider_editor_view(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    container: Option<String>,
    settings_config: Value,
    category: Option<String>,
    provider_id: Option<String>,
) -> Result<crate::services::provider::EditorView, String> {
    use crate::services::provider::EditorView;
    use crate::services::provider::{codex_editor, editor_toml, gemini_editor, grok_editor};

    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    let ssot = load_remote_ssot_for_mutation(&target, &home, &app).await?;
    let current_id = current_provider_id_from_persisted(
        &state,
        &host_id,
        container.as_deref(),
        &app,
        &ssot,
    )?;
    // 只有「正在编辑当前供应商」或「新增供应商（未传 id）」时，才把当前 live
    // 当作投影基底；编辑非当前供应商时必须回退到行本身 —— 否则会把当前供应商的
    // live 配置拼到另一行上，预览的不是切换后的真实效果。
    let editing_current = match provider_id.as_deref() {
        Some(id) => current_id.as_deref() == Some(id),
        None => true,
    };
    let current_row = if editing_current {
        current_id
            .as_deref()
            .and_then(|id| ssot.providers.iter().find(|p| p.id == id))
    } else {
        None
    };

    match app.as_str() {
        "claude" => {
            use crate::live::project::claude::{project_onto, ClaudeProjection};
            let prev = current_row.map(|p| ClaudeProjection::of(&p.settings_config));
            let settings_path = crate::remote::settings::remote_settings_path(&home);
            let path = std::path::Path::new(&settings_path);
            let pre = session
                .read_remote_text(&settings_path, container.as_deref())
                .await?;
            let (live, _) = crate::live::patch::json::parse(path, pre.as_deref().map(str::as_bytes))
                .map_err(|e| format!("读取远端 settings.json 失败: {e}"))?;
            let settings = project_onto(
                path,
                &live,
                prev.as_ref(),
                &ClaudeProjection::of(&settings_config),
            )
            .map_err(|e| format!("远端设置投影失败: {e}"))?;
            let inactive = crate::services::provider::claude_editor::inactive_fields(
                &settings_config,
                &settings,
            );
            Ok(EditorView { settings, inactive })
        }
        "gemini" => {
            use crate::live::project::gemini::GeminiProjection;
            let projection = GeminiProjection::of(
                &settings_config,
                category.as_deref() == Some("official"),
            );

            let env_path = format!("{home}/.gemini/.env");
            let env_pre = session
                .read_remote_text(&env_path, container.as_deref())
                .await?;
            let env_bytes = projection
                .env_patch()
                .apply(
                    std::path::Path::new(&env_path),
                    env_pre.as_deref().map(str::as_bytes),
                )
                .map_err(|e| format!("读取远端 .env 失败: {e}"))?;
            let env: serde_json::Map<String, Value> =
                crate::live::patch::dotenv::entries(&String::from_utf8_lossy(&env_bytes))
                    .into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect();

            let settings_path = format!("{home}/.gemini/settings.json");
            let path = std::path::Path::new(&settings_path);
            let settings_pre = session
                .read_remote_text(&settings_path, container.as_deref())
                .await?;
            let (mut config, _) =
                crate::live::patch::json::parse(path, settings_pre.as_deref().map(str::as_bytes))
                    .map_err(|e| format!("读取远端 settings.json 失败: {e}"))?;
            projection
                .settings_patch()
                .apply_to(path, &mut config)
                .map_err(|e| format!("远端 settings.json 投影失败: {e}"))?;

            let mut shown = settings_config.as_object().cloned().unwrap_or_default();
            shown.insert("env".to_string(), Value::Object(env.clone()));
            shown.insert("config".to_string(), config.clone());
            let inactive = gemini_editor::inactive_fields(&settings_config, &env, &config);
            Ok(EditorView {
                settings: Value::Object(shown),
                inactive,
            })
        }
        "codex" | "grokbuild" => {
            // 非当前供应商：本地投影依赖本机 DB，远端回退到行本身。
            let Some(_) = current_row else {
                return Ok(EditorView {
                    settings: settings_config,
                    inactive: Vec::new(),
                });
            };
            let path_str = if app == "codex" {
                format!("{home}/.codex/config.toml")
            } else {
                format!("{home}/.grok/config.toml")
            };
            let path = std::path::Path::new(&path_str);
            let pre = session.read_remote_text(&path_str, container.as_deref()).await?;
            let doc = crate::live::patch::toml::parse(path, pre.as_deref().map(str::as_bytes))
                .map_err(|e| format!("读取远端 {path_str} 失败: {e}"))?;
            let mut settings = settings_config.as_object().cloned().unwrap_or_default();
            settings.insert("config".to_string(), Value::String(doc.to_string()));
            if app == "codex" {
                settings
                    .entry("auth".to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
            }
            let inactive = if app == "codex" {
                codex_editor::inactive_fields(editor_toml::config_text(&settings_config), &doc)
            } else {
                grok_editor::inactive_fields_for(&settings_config, &doc)
            };
            Ok(EditorView {
                settings: Value::Object(settings),
                inactive,
            })
        }
        other => Err(format!("远端编辑器预览不支持应用 {other}")),
    }
}

#[tauri::command]
pub async fn get_remote_current_provider(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    container: Option<String>,
) -> Result<Option<String>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let _ = current_proxy_port(state.db.as_ref(), &state.proxy_service).await;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let ssot = load_remote_ssot_for_mutation(&target, &home, &app).await?;
    resolve_remote_current_provider_id(
        &state,
        &host_id,
        &target,
        &home,
        &app,
        container.as_deref(),
        &session,
        &ssot,
    )
    .await
}

/// 同步版当前供应商判定（不读远端 live）：持久化记录 → SSOT current。
///
/// 与 `resolve_remote_current_provider_id` 的前两级完全一致。写操作（编辑/删除）
/// 用它而不是 full resolver：查看面板时 `build_remote_providers_view` 已把
/// 含 live 兜底的结论落库，同步判定即可命中；同时避免在巨型 async fn 里再
/// await 远端解析（rustc 在 Windows 上把两层状态机内联会爆栈）。
fn current_provider_id_from_persisted(
    state: &AppState,
    host_id: &str,
    container: Option<&str>,
    app: &str,
    ssot: &crate::remote::providers::RemoteProvidersSsot,
) -> Result<Option<String>, String> {
    if let Some(persisted) = crate::remote::current::get_current_provider(
        state.db.as_ref(),
        host_id,
        container,
        app,
    )? {
        if ssot.providers.iter().any(|p| p.id == persisted) {
            return Ok(Some(persisted));
        }
    }

    if !crate::remote::providers::is_additive_app(app) {
        if let Some(current) = ssot.current_provider_id.as_deref() {
            if ssot.providers.iter().any(|p| p.id == current) {
                return Ok(Some(current.to_string()));
            }
        }
    }

    Ok(None)
}

/// 远端当前供应商判定（`get_remote_providers` / `get_remote_current_provider` 共用）。
#[allow(clippy::too_many_arguments)]
async fn resolve_remote_current_provider_id(
    state: &AppState,
    host_id: &str,
    target: &crate::remote::docker::RemoteTarget<'_>,
    home: &str,
    app: &str,
    container: Option<&str>,
    session: &connection::RemoteSession,
    ssot: &crate::remote::providers::RemoteProvidersSsot,
) -> Result<Option<String>, String> {
    // 1) 持久化记录优先：本应用上次「切换」写入的真实当前供应商，不受用户后续
    //    编辑 base_url / 通用配置片段影响（那正是匹配法失效的场景）。校验其仍
    //    存在于该远端 SSOT（per-target 独立：本机 DB 里有没有不再相关）。
    if let Some(persisted) = crate::remote::current::get_current_provider(
        state.db.as_ref(),
        host_id,
        container,
        app,
    )?
    {
        if ssot.providers.iter().any(|p| p.id == persisted) {
            return Ok(Some(persisted));
        }
    }

    // 2) 非 additive：SSOT current_provider_id（对齐本机 DB is_current）。
    //    additive（opencode/openclaw/hermes）无「当前供应商」概念，跳过。
    if !crate::remote::providers::is_additive_app(app) {
        if let Some(current) = ssot.current_provider_id.as_deref() {
            if ssot.providers.iter().any(|p| p.id == current) {
                return Ok(Some(current.to_string()));
            }
        }
    }

    // 3) 兜底：读远端 live 匹配 base_url（对从未经本应用切换的老配置）。
    //    仅「整文件覆盖式」app 有明确的 base_url 判定字段。
    if !matches!(app, "claude" | "codex" | "gemini" | "grokbuild") {
        return Ok(None);
    }

    // 读远端 live 文件 → 提取当前 base_url（与本机各 app 判定字段一致）
    let remote_base = match app {
        "codex" => {
            let path = format!("{home}/.codex/config.toml");
            session
                .read_remote_text(&path, container)
                .await?
                .and_then(|t| crate::codex_config::extract_codex_base_url(&t))
                .unwrap_or_default()
        }
        "gemini" => {
            let path = format!("{home}/.gemini/.env");
            session
                .read_remote_text(&path, container)
                .await?
                .map(|t| crate::gemini_config::parse_env_file(&t))
                .and_then(|m| m.get("GOOGLE_GEMINI_BASE_URL").cloned())
                .unwrap_or_default()
        }
        "grokbuild" => {
            let path = format!("{home}/.grok/config.toml");
            session
                .read_remote_text(&path, container)
                .await?
                .and_then(|t| crate::grok_config::extract_base_url(&t))
                .unwrap_or_default()
        }
        _ => {
            // claude：settings.json（FileOps，容器兼容）
            settings::read_remote_settings(target, home)
                .await?
                .pointer("/env/ANTHROPIC_BASE_URL")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        }
    };
    if remote_base.is_empty() {
        return Ok(None);
    }

    let app_type =
        crate::app_config::AppType::from_str(app).map_err(|_| format!("未知应用类型: {app}"))?;
    for p in &ssot.providers {
        // 远端 live 里存的就是供应商自己的 settings_config —— 官方 v4.0.4 已废弃
        // 「回填 / 合并通用配置片段」（见 official services/provider/claude_direct.rs
        // 的模块文档：「用户的设置本来就留在 live 里」），所以这里直接拿它比对即可，
        // 不再另算一份「生效配置」。
        let effective = p.settings_config.clone();
        let local_base = match app {
            "codex" => effective
                .get("config")
                .and_then(Value::as_str)
                .and_then(crate::codex_config::extract_codex_base_url)
                .unwrap_or_default(),
            "gemini" => effective
                .pointer("/env/GOOGLE_GEMINI_BASE_URL")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            "grokbuild" => effective
                .get("config")
                .and_then(Value::as_str)
                .and_then(crate::grok_config::extract_base_url)
                .unwrap_or_default(),
            _ => effective
                .pointer("/env/ANTHROPIC_BASE_URL")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        };
        if !local_base.is_empty() && local_base == remote_base {
            return Ok(Some(p.id.clone()));
        }
    }

    Ok(None)
}

/// 检测本机是否安装指定 app 的 CLI（`where <bin>` / `command -v <bin>`）。
#[tauri::command]
pub fn check_local_cli_installed(app: String) -> Result<bool, String> {
    let bin = cli_binary_for_app(&app).ok_or_else(|| format!("未知应用: {app}"))?;

    #[cfg(target_os = "windows")]
    let found = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new("where")
            .arg(bin)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    #[cfg(not(target_os = "windows"))]
    let found = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    Ok(found)
}

/// 检测远端是否安装指定 app 的 CLI（`command -v <bin>`），带超时。
/// 返回 true=已安装 / false=未安装 / None=检测失败或超时。
#[tauri::command]
pub async fn check_remote_cli_installed(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    container: Option<String>,
) -> Result<Option<bool>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;

    let probe = cli_installed_probe(&app, container.as_deref())?;
    let app_name = app.clone();
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), async move {
        let session = connection::connect(&host, Some(&password)).await?;
        match connection::exec_command(&session.channel, &probe).await {
            Ok(out) => {
                // Info 级：默认日志等级可记录，用于确认探测命令的真实返回
                log::info!(
                    "[remote] {app_name} 探测 cmd={probe:?} out={out:?} found={}",
                    out.contains(CLAUDE_INSTALLED_MARKER)
                );
                Ok(Some(out.contains(CLAUDE_INSTALLED_MARKER)))
            }
            Err(e) => {
                log::warn!("[remote] 检测远端 {app_name} 安装状态失败: {e}");
                Ok(None)
            }
        }
    })
    .await;

    match result {
        Ok(r) => r,
        Err(_) => {
            log::warn!("[remote] 检测远端 {app} 安装状态超时 host_id={host_id}");
            Ok(None)
        }
    }
}

/// 列出远端所有 app 的会话（对齐本机 sessionsApi.list()，全量返回）。
#[tauri::command]
pub async fn list_remote_sessions_all(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Vec<crate::session_manager::SessionMeta>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let home = host.default_home();

    // 3 个 SQLite 源（hermes / opencode / mcode）+ codex 线程标题库
    let hermes_db = format!("{home}/.hermes/state.db");
    let opencode_db = format!("{home}/.local/share/opencode/opencode.db");
    let mcode_db = format!("{home}/.minimax/v2/sqlite/runtime-state.sqlite");
    let codex_state_db = format!("{home}/.codex/state_5.sqlite");
    let hermes_sql = "SELECT * FROM sessions ORDER BY rowid DESC LIMIT 500";

    // 先一次性探测哪些 SQLite 库真的存在：一个都没有就完全跳过 helper
    // （首次上传 helper 约 2.6MB，容器上很贵，不能为不存在的库付这个成本）。
    let db_paths = [&hermes_db, &opencode_db, &mcode_db, &codex_state_db];
    let probe = format!(
        "for f in {}; do [ -f \"$f\" ] && echo \"$f\"; done",
        db_paths
            .iter()
            .map(|p| connection::shell_quote(p))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let existing_dbs = exec_in_target(&session, container.as_deref(), &probe)
        .await
        .unwrap_or_default();

    // hermes 可能出现 SQLite 与 JSONL 两套会话并存（本机 scan_sessions 也是合并 + SQLite 优先）
    let mut hermes_sqlite_ids: std::collections::HashSet<String> = Default::default();

    // codex 线程标题：本机会用 session_index.jsonl + state_5.sqlite 覆盖标题，
    // 远端同样读取，否则列表标题与本地不一致。
    let mut codex_titles: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    if let Ok(Some(text)) = target
        .read_text_optional(&format!("{home}/.codex/session_index.jsonl"))
        .await
    {
        codex_titles.extend(
            crate::session_manager::providers::codex::parse_session_index_titles(&text),
        );
    }
    if existing_dbs
        .lines()
        .any(|line| line.trim() == codex_state_db)
    {
        if let Ok(helper) =
            ensure_remote_sqlite_helper(&session, container.as_deref(), &home).await
        {
            if let Ok(v) = run_sqlite_helper(
                &session,
                container.as_deref(),
                &helper,
                "query",
                &codex_state_db,
                crate::session_manager::providers::codex::THREAD_TITLES_SQL,
                &[],
            )
            .await
            {
                if let Some(rows) = v.get("rows").and_then(Value::as_array) {
                    for row in rows {
                        if let (Some(id), Some(title)) = (
                            row.get("id").and_then(Value::as_str),
                            row.get("title").and_then(Value::as_str),
                        ) {
                            if !id.trim().is_empty() && !title.trim().is_empty() {
                                codex_titles.insert(id.to_string(), title.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // 6 个文件源 app：FileOps 泛型扫描，复用本机各 provider 的纯解析。
    // 根目录与本机 `session_roots()` 对齐——尤其 codex 不能扫整个 `~/.codex`
    // （那里还有 .tmp/plugins、skills 等几百个无关文件，容器下每个目录都是一趟 exec）。
    let file_apps: Vec<(&str, Vec<String>)> = vec![
        ("claude", vec![format!("{home}/.claude/projects")]),
        (
            "grokbuild",
            vec![
                format!("{home}/.grok/sessions"),
                format!("{home}/.grok/archived_sessions"),
            ],
        ),
        (
            "codex",
            vec![
                format!("{home}/.codex/sessions"),
                format!("{home}/.codex/archived_sessions"),
            ],
        ),
        ("gemini", vec![format!("{home}/.gemini/tmp")]),
        ("openclaw", vec![format!("{home}/.openclaw/agents")]),
        ("pi", vec![format!("{home}/.pi/agent/sessions")]),
    ];

    let mut all_sessions: Vec<crate::session_manager::SessionMeta> = Vec::new();
    for (app, roots) in &file_apps {
        for root in roots {
            let sessions = match *app {
                "claude" => crate::remote::sessions::scan_claude_fs(&target, root).await,
                "grokbuild" => crate::remote::sessions::scan_grokbuild_fs(&target, root).await,
                "codex" => {
                    crate::remote::sessions::scan_codex_fs(&target, root, &codex_titles).await
                }
                "gemini" => crate::remote::sessions::scan_gemini_fs(&target, root).await,
                "openclaw" => crate::remote::sessions::scan_openclaw_fs(&target, root).await,
                "pi" => crate::remote::sessions::scan_pi_fs(&target, root).await,
                _ => unreachable!(),
            };
            all_sessions.extend(sessions);
        }
    }

    if !existing_dbs.trim().is_empty() {
      if let Ok(helper) = ensure_remote_sqlite_helper(&session, container.as_deref(), &home).await {
        // hermes / mcode：单查询
        let simple_apps: Vec<(&str, &str, &str)> = vec![
            ("hermes", &hermes_db, hermes_sql),
            (
                "mcode",
                &mcode_db,
                crate::session_manager::providers::mcode::SCAN_SQL,
            ),
        ];
        for (app, db, sql) in &simple_apps {
            if !existing_dbs.lines().any(|line| line.trim() == *db) {
                continue;
            }
            if let Ok(v) = run_sqlite_helper(
                &session,
                container.as_deref(),
                &helper,
                "query",
                db,
                sql,
                &[],
            )
            .await
            {
                let rows = v
                    .get("rows")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for row in rows {
                    let meta = if *app == "hermes" {
                        let m =
                            crate::session_manager::providers::hermes::sqlite_row_to_session_meta(
                                &row,
                                &format!("sqlite:{db}"),
                            );
                        if let Some(meta) = &m {
                            hermes_sqlite_ids.insert(meta.session_id.clone());
                        }
                        m
                    } else {
                        crate::session_manager::providers::mcode::row_to_session_meta(&row)
                    };
                    if let Some(m) = meta {
                        all_sessions.push(m);
                    }
                }
            }
            // db 不存在/查询失败 → 跳过该 app，不影响文件源结果
        }

        // opencode：v1（session）与 v2（session_v2 + session_message）两套 schema，
        // 远端必须探测后选 SQL，否则 2.x 机器上一条会话都列不出来。
        if existing_dbs.lines().any(|line| line.trim() == opencode_db) {
            use crate::session_manager::providers::opencode as oc;
            let tables = run_sqlite_helper(
                &session,
                container.as_deref(),
                &helper,
                "query",
                &opencode_db,
                "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('session','session_v2','session_message')",
                &[],
            )
            .await
            .ok()
            .map(|v| {
                v.get("rows")
                    .and_then(Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .filter_map(|r| r.get("name").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
            let has_v1 = tables.iter().any(|t| t == "session");
            let has_v2 = tables.iter().any(|t| t == "session_v2") && tables.iter().any(|t| t == "session_message");

            let mut queries: Vec<&str> = Vec::new();
            if has_v2 {
                queries.push(oc::LIST_SQL_V2);
                if has_v1 {
                    queries.push(oc::LIST_SQL_V1_EXTRA);
                }
            } else if has_v1 {
                queries.push(oc::LIST_SQL_V1);
            }
            for sql in queries {
                if let Ok(v) = run_sqlite_helper(
                    &session,
                    container.as_deref(),
                    &helper,
                    "query",
                    &opencode_db,
                    sql,
                    &[],
                )
                .await
                {
                    let rows = v
                        .get("rows")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for row in rows {
                        if let Some(m) = oc::opencode_row_to_session_meta(&row, &opencode_db) {
                            all_sessions.push(m);
                        }
                    }
                }
            }
        }
      }
    }

    // hermes 旧版 JSONL 会话：与 SQLite 结果合并，同 id 以 SQLite 为准（对齐本机）。
    for meta in
        crate::remote::sessions::scan_hermes_fs(&target, &format!("{home}/.hermes/sessions"))
            .await
    {
        if !hermes_sqlite_ids.contains(&meta.session_id) {
            all_sessions.push(meta);
        }
    }

    // 时间倒序，与本机 scan_sessions 一致。
    all_sessions.sort_by(|a, b| {
        let a_ts = a.last_active_at.or(a.created_at).unwrap_or(0);
        let b_ts = b.last_active_at.or(b.created_at).unwrap_or(0);
        b_ts.cmp(&a_ts)
    });

    Ok(all_sessions)
}

#[tauri::command]
pub async fn get_remote_session_messages(
    state: State<'_, AppState>,
    host_id: String,
    source_path: String,
    session_id: String,
    container: Option<String>,
    app: String,
) -> Result<Vec<crate::session_manager::SessionMessage>, String> {
    let _ = session_id;
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    // hermes（sqlite: 源）/ opencode / mcode：SQLite 主存储，经远端 sqlite-helper 查询
    let is_sqlite_source = source_path.starts_with("sqlite:") || app == "mcode";
    if is_sqlite_source && (app == "hermes" || app == "opencode" || app == "mcode") {
        let home = host.default_home();
        let helper = ensure_remote_sqlite_helper(&session, container.as_deref(), &home).await?;
        if app == "mcode" {
            let db = format!("{home}/.minimax/v2/sqlite/runtime-state.sqlite");
            let v = run_sqlite_helper(
                &session,
                container.as_deref(),
                &helper,
                "query",
                &db,
                crate::session_manager::providers::mcode::MESSAGES_SQL,
                &[session_id.as_str()],
            )
            .await?;
            let rows = v
                .get("rows")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            return Ok(rows
                .iter()
                .filter_map(crate::session_manager::providers::mcode::row_to_message)
                .collect());
        }
        if app == "hermes" {
            let db = format!("{home}/.hermes/state.db");
            // 与本机 `load_messages_from_conn` 对齐：列名是 `timestamp`（REAL 秒），
            // 顺序按 `id ASC`（插入顺序；时间戳可能回退，不能按时间排）。
            let sql = "SELECT id, role, content, timestamp FROM messages WHERE session_id = ?1 ORDER BY id ASC";
            let v = run_sqlite_helper(
                &session,
                container.as_deref(),
                &helper,
                "query",
                &db,
                sql,
                &[session_id.as_str()],
            )
            .await?;
            let rows = v
                .get("rows")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            return Ok(rows
                .iter()
                .filter_map(crate::session_manager::providers::hermes::sqlite_row_to_message)
                .collect());
        }
        // opencode：v2（session_v2/session_message）优先，按会话所在表选择
        let db = format!("{home}/.local/share/opencode/opencode.db");
        let in_v2 = run_sqlite_helper(
            &session,
            container.as_deref(),
            &helper,
            "query",
            &db,
            "SELECT id FROM session_v2 WHERE id = ?1",
            &[session_id.as_str()],
        )
        .await
        .ok()
        .and_then(|v| {
            v.get("rows")
                .and_then(Value::as_array)
                .map(|rows| !rows.is_empty())
        })
        .unwrap_or(false);
        if in_v2 {
            let v = run_sqlite_helper(
                &session,
                container.as_deref(),
                &helper,
                "query",
                &db,
                crate::session_manager::providers::opencode::MESSAGES_SQL_V2,
                &[session_id.as_str()],
            )
            .await?;
            let rows = v
                .get("rows")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            return Ok(
                crate::session_manager::providers::opencode::assemble_opencode_v2_messages(&rows),
            );
        }

        // v1：message + part 两表，query-all 一次取两个结果集
        let sql = "SELECT id, time_created, data FROM message WHERE session_id = ?1 ORDER BY time_created ASC; SELECT message_id, data FROM part WHERE session_id = ?1 ORDER BY time_created ASC";
        let v = run_sqlite_helper(
            &session,
            container.as_deref(),
            &helper,
            "query-all",
            &db,
            sql,
            &[session_id.as_str()],
        )
        .await?;
        let rowsets = v
            .get("rowsets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let message_rows = rowsets
            .first()
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let part_rows = rowsets
            .get(1)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        return Ok(
            crate::session_manager::providers::opencode::assemble_opencode_messages(
                &message_rows,
                &part_rows,
            ),
        );
    }

    match app.as_str() {
        "claude" => {
            let content = target
                .read_text_optional(&source_path)
                .await?
                .unwrap_or_default();
            Ok(crate::session_manager::providers::claude::parse_messages_from_content(
                content.as_bytes(),
                std::path::PathBuf::from(&source_path).with_extension(""),
            ))
        }
        "codex" => {
            let content = target
                .read_text_optional(&source_path)
                .await?
                .unwrap_or_default();
            Ok(crate::session_manager::providers::codex::parse_messages_from_content(
                content.as_bytes(),
            ))
        }
        // openclaw / grokbuild：JSONL 内容直接解析
        "openclaw" => {
            let content = target
                .read_text_optional(&source_path)
                .await?
                .unwrap_or_default();
            Ok(
                crate::session_manager::providers::openclaw::parse_messages_from_content(
                    content.as_bytes(),
                ),
            )
        }
        "grokbuild" => {
            // sourcePath 是 summary.json，正文在同目录 chat_history.jsonl
            let chat_path = std::path::Path::new(&source_path)
                .parent()
                .map(|dir| dir.join("chat_history.jsonl"))
                .and_then(|p| p.to_str().map(str::to_string))
                .ok_or_else(|| format!("无效的 Grok Build 会话路径: {source_path}"))?;
            let content = target.read_text_optional(&chat_path).await?.unwrap_or_default();
            Ok(
                crate::session_manager::providers::grokbuild::parse_messages_from_content(
                    content.as_bytes(),
                ),
            )
        }
        // gemini：单个 JSON 文档（非 JSONL）
        "gemini" => {
            let content = target
                .read_text_optional(&source_path)
                .await?
                .unwrap_or_default();
            crate::session_manager::providers::gemini::parse_messages_from_content(
                &content, &source_path,
            )
        }
        // pi：树 + 正文都从同一份内容得出（无本机 root 解析）
        "pi" => {
            let content = target
                .read_text_optional(&source_path)
                .await?
                .unwrap_or_default();
            crate::session_manager::providers::pi::parse_messages_from_content(&content)
        }
        // hermes 旧版 JSONL 会话（非 sqlite: 源）
        "hermes" => {
            let content = target
                .read_text_optional(&source_path)
                .await?
                .unwrap_or_default();
            Ok(crate::session_manager::providers::hermes::parse_messages_from_content(
                content.as_bytes(),
            ))
        }
        other => Err(format!("远端会话管理暂不支持应用 {other}（尚未移植）")),
    }
}

#[tauri::command]
pub async fn delete_remote_session(
    state: State<'_, AppState>,
    host_id: String,
    source_path: String,
    session_id: String,
    container: Option<String>,
    app: String,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    // mcode：与本机一致——会话只能在 MCode 内删（runtime state 与历史要一起移除）
    if app == "mcode" {
        return Err(
            "请在 MiniMax Code 中删除该会话（其运行时状态与历史需由 MCode 一起清理）".to_string(),
        );
    }

    // hermes：SQLite 主存储（会话 + 消息两张表）；JSONL 会话走文件删除
    if app == "hermes" && source_path.starts_with("sqlite:") {
        let home = host.default_home();
        let helper = ensure_remote_sqlite_helper(&session, container.as_deref(), &home).await?;
        let db = format!("{home}/.hermes/state.db");
        let v = run_sqlite_helper(
            &session,
            container.as_deref(),
            &helper,
            "query",
            &db,
            "SELECT id FROM sessions WHERE id = ?1",
            &[session_id.as_str()],
        )
        .await?;
        if !v
            .get("rows")
            .and_then(Value::as_array)
            .map(|r| !r.is_empty())
            .unwrap_or(false)
        {
            return Err(format!("远端会话不存在: {session_id}"));
        }
        run_sqlite_helper(
            &session,
            container.as_deref(),
            &helper,
            "write",
            &db,
            "DELETE FROM messages WHERE session_id = ?1; DELETE FROM sessions WHERE id = ?1;",
            &[session_id.as_str()],
        )
        .await?;
        return Ok(true);
    }

    // opencode：v1（session/message/part）与 v2（session_v2/session_message）两套 schema，
    // 远端按实际表结构删除；2.x 机器上原来的 v1 SQL 会直接报「会话不存在」。
    if app == "opencode" {
        let home = host.default_home();
        let helper = ensure_remote_sqlite_helper(&session, container.as_deref(), &home).await?;
        let db = format!("{home}/.local/share/opencode/opencode.db");
        let tables: Vec<String> = run_sqlite_helper(
            &session,
            container.as_deref(),
            &helper,
            "query",
            &db,
            "SELECT name FROM sqlite_master WHERE type='table'",
            &[],
        )
        .await
        .ok()
        .and_then(|v| {
            v.get("rows").and_then(Value::as_array).map(|rows| {
                rows.iter()
                    .filter_map(|r| r.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
        })
        .unwrap_or_default();
        let has = |t: &str| tables.iter().any(|x| x == t);
        let has_v1 = has("session");
        let has_v2 = has("session_v2") && has("session_message");
        if !has_v1 && !has_v2 {
            return Err(format!("远端会话不存在: {session_id}"));
        }

        let mut checks = Vec::new();
        if has_v2 {
            checks.push("SELECT id FROM session_v2 WHERE id = ?1");
        }
        if has_v1 {
            checks.push("SELECT id FROM session WHERE id = ?1");
        }
        let v = run_sqlite_helper(
            &session,
            container.as_deref(),
            &helper,
            "query",
            &db,
            &checks.join(" UNION ALL "),
            &[session_id.as_str()],
        )
        .await?;
        if !v
            .get("rows")
            .and_then(Value::as_array)
            .map(|r| !r.is_empty())
            .unwrap_or(false)
        {
            return Err(format!("远端会话不存在: {session_id}"));
        }

        let mut stmts: Vec<String> = Vec::new();
        if has_v2 {
            stmts.push("DELETE FROM session_message WHERE session_id = ?1;".into());
            for child in [
                "session_pending",
                "session_inbox",
                "instruction_entry",
                "instruction_state",
            ] {
                if has(child) {
                    stmts.push(format!("DELETE FROM {child} WHERE session_id = ?1;"));
                }
            }
            if has("event_sequence") {
                stmts.push("DELETE FROM event_sequence WHERE aggregate_id = ?1;".into());
            }
            stmts.push("DELETE FROM session_v2 WHERE id = ?1;".into());
        }
        if has_v1 {
            if has("part") {
                stmts.push("DELETE FROM part WHERE session_id = ?1;".into());
            }
            if has("message") {
                stmts.push("DELETE FROM message WHERE session_id = ?1;".into());
            }
            stmts.push("DELETE FROM session WHERE id = ?1;".into());
        }
        run_sqlite_helper(
            &session,
            container.as_deref(),
            &helper,
            "write",
            &db,
            &stmts.join(" "),
            &[session_id.as_str()],
        )
        .await?;
        return Ok(true);
    }

    crate::remote::sessions::delete_session_fs(&target, &app, &source_path).await
}

/// 远端会话根（File 引用的相对路径以它为基，与本机 provider_roots 一致）。
fn remote_session_root(app: &str, home: &str) -> String {
    match app {
        "claude" => format!("{home}/.claude/projects"),
        "codex" => format!("{home}/.codex"),
        "gemini" => format!("{home}/.gemini/tmp"),
        "grokbuild" => format!("{home}/.grok"),
        "openclaw" => format!("{home}/.openclaw/agents"),
        "pi" => format!("{home}/.pi/agent/sessions"),
        "opencode" => format!("{home}/.local/share/opencode/storage"),
        "hermes" => format!("{home}/.hermes"),
        "mcode" => format!("{home}/.minimax"),
        _ => home.to_string(),
    }
}

/// 远端 SQLite 会话的数据库路径。
fn remote_sqlite_db(app: &str, home: &str) -> Option<String> {
    match app {
        "hermes" => Some(format!("{home}/.hermes/state.db")),
        "opencode" => Some(format!("{home}/.local/share/opencode/opencode.db")),
        "mcode" => Some(format!("{home}/.minimax/v2/sqlite/runtime-state.sqlite")),
        _ => None,
    }
}

/// 从 sourcePath 取出 SQLite 会话 id（hermes/opencode/mcode 三种源格式）。
fn remote_sqlite_session_id(app: &str, source_path: &str) -> Option<String> {
    match app {
        "hermes" => crate::session_manager::providers::hermes::parse_sqlite_source(source_path)
            .map(|(_, id)| id),
        "opencode" => crate::session_manager::providers::opencode::parse_sqlite_source(source_path)
            .map(|(_, id)| id),
        "mcode" => source_path.strip_prefix("mcode:").map(str::to_string),
        _ => None,
    }
}

/// 远端 SQLite 单元格 → 文本（对齐本机 `content::read_sqlite_cell`：
/// 表/列白名单 + id 与 session_id 双绑定，只能读本会话的行）。
#[allow(clippy::too_many_arguments)]
async fn resolve_remote_sqlite_cell(
    session: &connection::RemoteSession,
    container: Option<&str>,
    home: &str,
    app: &str,
    source_path: &str,
    table: &str,
    id: &str,
    column: &str,
    pointer: &str,
) -> Result<String, String> {
    if !crate::session_manager::content::sqlite_allowed(app, table, column) {
        return Err("不支持的内容引用".to_string());
    }
    let session_id = remote_sqlite_session_id(app, source_path)
        .ok_or_else(|| format!("Invalid SQLite source reference: {source_path}"))?;
    let db = remote_sqlite_db(app, home).ok_or_else(|| format!("应用 {app} 没有 SQLite 会话"))?;
    let helper = ensure_remote_sqlite_helper(session, container, home).await?;
    // 表名/列名已过白名单，可安全拼接；id/session_id 用参数绑定
    let sql = format!(
        "SELECT length(CAST({column} AS BLOB)) AS __len, {column} AS __cell FROM {table} WHERE id = ?1 AND session_id = ?2"
    );
    let v = run_sqlite_helper(
        session,
        container,
        &helper,
        "query",
        &db,
        &sql,
        &[id, session_id.as_str()],
    )
    .await?;
    let rows = v
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let row = rows
        .first()
        .ok_or_else(|| "会话已更新，请重新读取".to_string())?;
    let size = row.get("__len").and_then(Value::as_i64).unwrap_or(0);
    if size as u64 > crate::remote::sessions::MAX_TEXT_BYTES {
        return Err("内容超过 32MB，请直接打开源文件查看".to_string());
    }
    let raw = row
        .get("__cell")
        .and_then(Value::as_str)
        .ok_or_else(|| "引用的内容不是文本".to_string())?
        .to_string();
    if pointer.is_empty() {
        return Ok(raw);
    }
    let value: Value =
        serde_json::from_str(&raw).map_err(|_| "会话已更新，请重新读取".to_string())?;
    crate::session_manager::content::extract_text(&value, pointer)
}

/// 远端会话按需取块全文（阅读页「加载更多」），使用与本机一致的字符分页。
#[tauri::command]
pub async fn get_remote_session_block_content(
    state: State<'_, AppState>,
    host_id: String,
    source_path: String,
    container: Option<String>,
    app: String,
    content_ref: crate::session_manager::model::ContentRef,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<crate::session_manager::content::BlockContent, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let home = host.default_home();
    let full = match &content_ref {
        // SQLite 引用（opencode 的 part / hermes 的 content 等）走 sqlite-helper
        crate::session_manager::model::ContentRef::Sqlite {
            table,
            id,
            column,
            pointer,
        } => {
            resolve_remote_sqlite_cell(
                &session,
                container.as_deref(),
                &home,
                &app,
                &source_path,
                table,
                id,
                column,
                pointer,
            )
            .await?
        }
        _ => {
            let root = remote_session_root(&app, &home);
            crate::remote::sessions::resolve_content_ref_fs(
                &target,
                &app,
                &source_path,
                &root,
                &content_ref,
            )
            .await?
        }
    };
    Ok(crate::session_manager::content::paginate(&full, offset, limit))
}

/// 远端会话图片原始字节（阅读页图片预览）。返回 `tauri::ipc::Response`（与命令层一致）。
#[tauri::command]
pub async fn get_remote_session_image(
    state: State<'_, AppState>,
    host_id: String,
    source_path: String,
    container: Option<String>,
    app: String,
    image: crate::session_manager::model::ImageRef,
) -> Result<tauri::ipc::Response, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let home = host.default_home();
    let bytes = match &image.source {
        // 内联图片存在 SQLite 列里（opencode/hermes）→ helper 取回后解码
        crate::session_manager::model::ImageSource::Inline {
            content:
                crate::session_manager::model::ContentRef::Sqlite {
                    table,
                    id,
                    column,
                    pointer,
                },
        } => {
            let encoded = resolve_remote_sqlite_cell(
                &session,
                container.as_deref(),
                &home,
                &app,
                &source_path,
                table,
                id,
                column,
                pointer,
            )
            .await?;
            crate::session_manager::content::decode_inline_image(
                &encoded,
                crate::remote::sessions::MAX_IMAGE_BYTES,
            )?
        }
        _ => {
            let root = remote_session_root(&app, &home);
            crate::remote::sessions::load_image_fs(&target, &app, &source_path, &root, &image)
                .await?
        }
    };
    Ok(tauri::ipc::Response::new(bytes))
}

/// 列出远端 MCP 服务器（完整 McpServer，读 SSOT ~/.cc-switch/mcp.json）。
#[tauri::command]
pub async fn read_remote_mcp_servers(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Vec<crate::remote::mcp::RemoteMcpServer>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::mcp::list_remote_mcp_servers(&target, &host.default_home()).await
}

/// 读取远端 `~/.claude.json` 的**完整内容**（供编辑/展示）。
#[tauri::command]
pub async fn read_remote_mcp_json(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<serde_json::Value, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::mcp::read_remote_mcp_json(&target, &host.default_home()).await
}

/// 新增/更新远端 MCP 服务器：写 SSOT + 同步 apps 启用的各 live 配置。
#[tauri::command]
pub async fn upsert_remote_mcp_server(
    state: State<'_, AppState>,
    host_id: String,
    server: crate::remote::mcp::RemoteMcpServer,
    container: Option<String>,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::mcp::upsert_remote_mcp_server(&target, &host.default_home(), &server).await?;
    Ok(true)
}

/// 从远端删除一个 MCP 服务器：删 SSOT + 从所有启用的 live 配置移除。
#[tauri::command]
pub async fn delete_remote_mcp_server(
    state: State<'_, AppState>,
    host_id: String,
    id: String,
    container: Option<String>,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::mcp::delete_remote_mcp_server(&target, &host.default_home(), &id).await
}

/// 重投影指定 app 的远端 live 配置（对齐本机 resyncToApps 的单 app 语义）。
#[tauri::command]
pub async fn reproject_remote_mcp_for_app(
    state: State<'_, AppState>,
    host_id: String,
    app: String,
    container: Option<String>,
) -> Result<(), String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::mcp::reproject_remote_mcp_for_app(&target, &host.default_home(), &app).await
}

/// 切换远端 MCP 服务器在指定 app 的启用状态（改 SSOT + 同步/移除该 app live）。
#[tauri::command]
pub async fn toggle_remote_mcp_app(
    state: State<'_, AppState>,
    host_id: String,
    id: String,
    app: String,
    enabled: bool,
    container: Option<String>,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::mcp::toggle_remote_mcp_app(&target, &host.default_home(), &id, &app, enabled)
        .await?;
    Ok(true)
}

/// 批量切换多个 MCP 服务器在某应用：一次连接内改完 SSOT 与 live。
#[tauri::command]
pub async fn bulk_toggle_remote_mcp_app(
    state: State<'_, AppState>,
    host_id: String,
    ids: Vec<String>,
    app: String,
    enabled: bool,
    container: Option<String>,
) -> Result<crate::remote::RemoteBulkToggleResult, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::mcp::bulk_toggle_remote_mcp_app(
        &target,
        &host.default_home(),
        &ids,
        &app,
        enabled,
    )
    .await
}

/// 从远端各 CLI live 配置导入 MCP 到 SSOT，返回新导入数量。
#[tauri::command]
pub async fn import_remote_mcp_from_apps(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<usize, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::mcp::import_remote_mcp_from_apps(&target, &host.default_home()).await
}

/// 读取远端 live 提示词文件内容（文件缺失返回空字符串）。
#[tauri::command]
pub async fn read_remote_prompt(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    app: String,
) -> Result<String, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::prompt::read_remote_prompt(&target, &host.default_home(), &app).await
}

/// 将内容整文件原子写回远端 live 提示词文件。
#[tauri::command]
pub async fn write_remote_prompt(
    state: State<'_, AppState>,
    host_id: String,
    content: String,
    container: Option<String>,
    app: String,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::prompt::write_remote_prompt(&target, &host.default_home(), &app, &content)
        .await?;
    Ok(true)
}

/// 列出远端 prompts.json 中的提示词列表。
#[tauri::command]
pub async fn list_remote_prompts(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    app: String,
) -> Result<Vec<crate::prompt::Prompt>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::prompt::read_remote_prompts(&target, &host.default_home(), &app).await
}

/// 保存（新增/更新）远端提示词列表，并同步启用项到 live 提示词文件。
#[tauri::command]
pub async fn save_remote_prompts(
    state: State<'_, AppState>,
    host_id: String,
    prompts: Vec<crate::prompt::Prompt>,
    container: Option<String>,
    app: String,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::prompt::write_remote_prompts(&target, &host.default_home(), &app, &prompts)
        .await?;
    Ok(true)
}

// ========================================================================
// Pi 原生指令文件 + 模板（远端）
// ========================================================================

/// 读远端 Pi 系统指令文件（SYSTEM.md / APPEND_SYSTEM.md）。
#[tauri::command]
pub async fn get_remote_pi_prompt_file(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    kind: crate::services::pi_prompt_files::PiPromptFileKind,
) -> Result<crate::services::pi_prompt_files::PiPromptFileSnapshot, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::prompt::read_remote_pi_file(&target, &host.default_home(), kind).await
}

/// 写远端 Pi 系统指令文件（带 revision 冲突检测）。
#[tauri::command]
pub async fn replace_remote_pi_prompt_file(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    kind: crate::services::pi_prompt_files::PiPromptFileKind,
    #[allow(non_snake_case)] expectedRevision: String,
    content: String,
) -> Result<crate::services::pi_prompt_files::PiPromptFileSnapshot, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::prompt::write_remote_pi_file(
        &target,
        &host.default_home(),
        kind,
        &expectedRevision,
        &content,
    )
    .await
}

/// 删除远端 Pi 系统指令文件。
#[tauri::command]
pub async fn delete_remote_pi_prompt_file(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    kind: crate::services::pi_prompt_files::PiPromptFileKind,
    #[allow(non_snake_case)] expectedRevision: String,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::prompt::delete_remote_pi_file(
        &target,
        &host.default_home(),
        kind,
        &expectedRevision,
    )
    .await
}

/// 列出远端 Pi 模板。
#[tauri::command]
pub async fn list_remote_pi_prompt_templates(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Vec<crate::services::pi_prompt_files::PiPromptTemplate>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::prompt::list_remote_pi_templates(&target, &host.default_home()).await
}

/// 创建/更新远端 Pi 模板。
#[tauri::command]
pub async fn upsert_remote_pi_prompt_template(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    slug: String,
    #[allow(non_snake_case)] originalSlug: Option<String>,
    #[allow(non_snake_case)] expectedRevision: String,
    content: String,
) -> Result<crate::services::pi_prompt_files::PiPromptTemplate, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::prompt::upsert_remote_pi_template(
        &target,
        &host.default_home(),
        &slug,
        originalSlug.as_deref(),
        &expectedRevision,
        &content,
    )
    .await
}

/// 删除远端 Pi 模板。
#[tauri::command]
pub async fn delete_remote_pi_prompt_template(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    slug: String,
    #[allow(non_snake_case)] expectedRevision: String,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::prompt::delete_remote_pi_template(
        &target,
        &host.default_home(),
        &slug,
        &expectedRevision,
    )
    .await
}

/// 列出远端 `~/.claude/skills/` 下的已安装技能目录。
#[tauri::command]
pub async fn list_remote_skills(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Vec<crate::remote::skill::RemoteSkillEntry>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::skill::list_remote_skills(&target, &host.default_home()).await
}

/// 重同步：把远端 SSOT 里所有技能按各自 apps 开关重新投影到各应用目录。
#[tauri::command]
pub async fn resync_remote_skills(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<(), String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    crate::remote::skill::resync_remote_skills(
        &session.channel,
        container.as_deref(),
        &host.default_home(),
    )
    .await
}

/// 删除远端 SSOT 中的技能（目录 + symlink + skills.json）。
#[tauri::command]
pub async fn delete_remote_skill(
    state: State<'_, AppState>,
    host_id: String,
    name: String,
    container: Option<String>,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let home = host.default_home();

    // 卸载前备份（对齐本机：先备份再删；备份失败则中止，避免丢技能）
    let records = crate::remote::skill::read_remote_skills_json(&target, &home).await?;
    if let Some(record) = records.iter().find(|r| r.id == name) {
        // 备份失败必须中止：继续删除会永久丢失这份技能（与本机
        // `SkillService::uninstall_skill` 的语义一致）。
        backup_remote_skill_before_delete(&session, container.as_deref(), &home, record)
            .await
            .map_err(|e| format!("卸载前备份失败，已中止删除以保留技能: {e}"))?;
    }

    crate::remote::skill::delete_remote_skill(
        &target,
        Some(&session.channel),
        container.as_deref(),
        &home,
        &name,
    )
    .await
}

/// 卸载前把远端 SSOT 技能目录备份到 `~/.cc-switch/skill-backups/<ts>_<dir>/`。
async fn backup_remote_skill_before_delete(
    session: &connection::RemoteSession,
    container: Option<&str>,
    root: &str,
    record: &crate::remote::skill::RemoteSkillRecord,
) -> Result<(), String> {
    let ssot = crate::remote::skill::remote_ssot_path(root);
    let source = format!("{ssot}/{}", record.directory);
    let probe = format!(
        "if [ -d {p} ]; then echo yes; else echo no; fi",
        p = connection::shell_quote(&source)
    );
    let exists = exec_in_target(session, container, &probe)
        .await
        .unwrap_or_default();
    if exists.trim() != "yes" {
        log::warn!("[remote-skill] 备份源不存在，跳过备份: {source}");
        return Ok(());
    }

    let backup_root = crate::remote::skill::remote_skill_backups_root(root);
    let stamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let mut backup_id = format!("{stamp}_{}", record.directory);
    // 同一秒内重复卸载同一技能时加后缀
    for counter in 1..100 {
        let probe = format!(
            "if [ -e {p} ]; then echo yes; else echo no; fi",
            p = connection::shell_quote(&format!("{backup_root}/{backup_id}"))
        );
        let taken = exec_in_target(session, container, &probe)
            .await
            .unwrap_or_default();
        if taken.trim() != "yes" {
            break;
        }
        backup_id = format!("{stamp}_{}_{counter}", record.directory);
    }

    let backup_dir = format!("{backup_root}/{backup_id}");
    let script = format!(
        "mkdir -p {dest} && cp -r {src} {dest}/skill",
        dest = connection::shell_quote(&backup_dir),
        src = connection::shell_quote(&source),
    );
    exec_in_target(session, container, &script).await?;

    let meta = crate::remote::skill::RemoteSkillBackupMeta {
        skill: installed_skill_from_record(record),
        backup_created_at: chrono::Utc::now().timestamp(),
        source_path: source,
    };
    let text = serde_json::to_string_pretty(&meta)
        .map_err(|e| format!("序列化备份 meta.json 失败: {e}"))?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container,
    )?;
    target
        .write_text_atomic(&format!("{backup_dir}/meta.json"), &text)
        .await
}

/// `RemoteSkillRecord` → `InstalledSkill`（备份 meta 用，前端复用本机类型）。
pub(crate) fn installed_skill_from_record(
    record: &crate::remote::skill::RemoteSkillRecord,
) -> crate::app_config::InstalledSkill {
    crate::app_config::InstalledSkill {
        id: record.id.clone(),
        name: record.name.clone(),
        description: record.description.clone(),
        directory: record.directory.clone(),
        repo_owner: record.repo_owner.clone(),
        repo_name: record.repo_name.clone(),
        repo_branch: record.repo_branch.clone(),
        readme_url: record.readme_url.clone(),
        apps: crate::app_config::SkillApps {
            mcode: record.apps.mcode,
            claude: record.apps.claude,
            codex: record.apps.codex,
            gemini: record.apps.gemini,
            grokbuild: record.apps.grokbuild,
            opencode: record.apps.opencode,
            hermes: record.apps.hermes,
            pi: record.apps.pi,
        },
        installed_at: record.installed_at,
        content_hash: record.content_hash.clone(),
        updated_at: record.updated_at,
    }
}

/// 列出远端技能备份。
#[tauri::command]
pub async fn list_remote_skill_backups(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Vec<crate::services::skill::SkillBackupEntry>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::skill::list_remote_skill_backups(&target, &host.default_home()).await
}

/// 删除远端技能备份。
#[tauri::command]
pub async fn delete_remote_skill_backup(
    state: State<'_, AppState>,
    host_id: String,
    backup_id: String,
    container: Option<String>,
) -> Result<(), String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::skill::delete_remote_skill_backup(
        &target,
        &host.default_home(),
        &backup_id,
    )
    .await
}

/// 从远端备份恢复技能。
#[tauri::command]
pub async fn restore_remote_skill_backup(
    state: State<'_, AppState>,
    host_id: String,
    backup_id: String,
    current_app: String,
    container: Option<String>,
) -> Result<crate::app_config::InstalledSkill, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::skill::restore_remote_skill_backup(
        &target,
        &session.channel,
        container.as_deref(),
        &host.default_home(),
        &backup_id,
        &current_app,
    )
    .await
}

/// 切换远端技能在某应用的启用状态（更新 skills.json + 增删 symlink）。
#[tauri::command]
pub async fn toggle_remote_skill_app(
    state: State<'_, AppState>,
    host_id: String,
    name: String,
    app: String,
    enabled: bool,
    container: Option<String>,
) -> Result<bool, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let use_copy =
        crate::settings::get_skill_sync_method() == crate::services::skill::SyncMethod::Copy;
    let result = crate::remote::skill::bulk_toggle_remote_skill_app(
        &target,
        &session.channel,
        container.as_deref(),
        &host.default_home(),
        std::slice::from_ref(&name),
        &app,
        enabled,
        use_copy,
    )
    .await?;
    match result.failed.into_iter().next() {
        Some(failure) => Err(failure.error),
        None => Ok(true),
    }
}

/// 批量切换多个远端技能在某应用的启用状态（一次连接 + 一次 exec）。
#[tauri::command]
pub async fn bulk_toggle_remote_skill_app(
    state: State<'_, AppState>,
    host_id: String,
    ids: Vec<String>,
    app: String,
    enabled: bool,
    container: Option<String>,
) -> Result<crate::remote::RemoteBulkToggleResult, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let use_copy =
        crate::settings::get_skill_sync_method() == crate::services::skill::SyncMethod::Copy;
    crate::remote::skill::bulk_toggle_remote_skill_app(
        &target,
        &session.channel,
        container.as_deref(),
        &host.default_home(),
        &ids,
        &app,
        enabled,
        use_copy,
    )
    .await
}

fn shell_q_rs(s: &str) -> String {
    if s.contains('\'') {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{s}'")
    }
}

/// 从本地 ZIP 安装技能到远端 SSOT → 写 skills.json → 创建 symlink。
#[tauri::command]
pub async fn install_remote_skills_from_zip(
    state: State<'_, AppState>,
    host_id: String,
    zip_path: String,
    container: Option<String>,
    app: String,
) -> Result<Vec<crate::remote::skill::RemoteSkillRecord>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    // 解压 + 上传到 SSOT
    let installed = crate::remote::skill::install_remote_skills_from_zip_generic(
        &session.sftp,
        &session.channel,
        container.as_deref(),
        &host.default_home(),
        &zip_path,
    )
    .await?;

    // 对每个新安装的技能：写 skills.json + 创建 symlink，同时收集完整记录
    let ssot_dir = crate::remote::skill::remote_ssot_path(&host.default_home());
    let mut records =
        crate::remote::skill::read_remote_skills_json(&target, &host.default_home()).await?;
    let mut apps = crate::remote::skill::RemoteSkillApps::default();
    apps.set_enabled(&app, true);
    let use_copy =
        crate::settings::get_skill_sync_method() == crate::services::skill::SyncMethod::Copy;
    let mut result: Vec<crate::remote::skill::RemoteSkillRecord> = Vec::new();

    for name in &installed {
        let skill_dir = format!("{ssot_dir}/{name}");
        let (display_name, description) =
            crate::remote::skill::read_skill_md_meta_static(&target, &skill_dir).await;
        let record = crate::remote::skill::build_remote_skill_record(
            name,
            display_name,
            description,
            None,
            None,
            None,
            None,
            apps.clone(),
        );
        records.push(record.clone());
        result.push(record);
        crate::remote::skill::sync_remote_skill_links(
            &session.channel,
            container.as_deref(),
            &host.default_home(),
            name,
            &apps,
            use_copy,
        )
        .await?;
    }

    crate::remote::skill::write_remote_skills_json(&target, &host.default_home(), &records).await?;
    Ok(result)
}

/// 从「发现技能」列表把一个技能安装到远端。
///
/// 对齐本机 `install_skill_unified` 语义：网络动作（从 GitHub 仓库下载 zip）走本机，
/// 下载后上传到远端 SSOT → 写 skills.json → 建链接。同仓库同名复用，不同仓库同名报错。
#[tauri::command]
pub async fn install_remote_skill_from_discoverable(
    state: State<'_, AppState>,
    host_id: String,
    skill: crate::services::skill::DiscoverableSkill,
    container: Option<String>,
    app: String,
) -> Result<crate::remote::skill::RemoteSkillRecord, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let root = host.default_home();
    let channel = &session.channel;
    let container_ref = container.as_deref();

    // 1. 安装目录名 = directory 最后一段（对齐本机 sanitize_install_name）。
    let install_name = crate::remote::skill::sanitize_name(
        skill
            .directory
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&skill.directory),
    );
    if install_name.is_empty() {
        return Err("无效的技能目录名".to_string());
    }

    // 2. 冲突检测（对齐本机 install 的 DB 检查：同仓库更新启用，不同仓库报错）。
    let mut records = crate::remote::skill::read_remote_skills_json(&target, &root).await?;
    let mut apps = crate::remote::skill::RemoteSkillApps::default();
    apps.set_enabled(&app, true);
    if let Some(existing) = records
        .iter()
        .find(|r| r.directory.eq_ignore_ascii_case(&install_name))
    {
        let same_repo = existing.repo_owner.as_deref() == Some(skill.repo_owner.as_str())
            && existing.repo_name.as_deref() == Some(skill.repo_name.as_str());
        if !same_repo {
            return Err(format!(
                "远端已存在同名技能目录 {install_name}（来自 {}/{}），请先卸载或选择其他仓库",
                existing.repo_owner.as_deref().unwrap_or("unknown"),
                existing.repo_name.as_deref().unwrap_or("unknown")
            ));
        }
        // 复制记录再改，避免可变借用横跨 sync/write 导致借用冲突。
        let mut updated = existing.clone();
        updated.apps = apps.clone();
        updated.updated_at = chrono::Utc::now().timestamp_millis();
        let use_copy =
            crate::settings::get_skill_sync_method() == crate::services::skill::SyncMethod::Copy;
        crate::remote::skill::sync_remote_skill_links(
            channel,
            container_ref,
            &root,
            &install_name,
            &updated.apps,
            use_copy,
        )
        .await?;
        crate::remote::skill::write_remote_skills_json(&target, &root, &records).await?;
        return Ok(updated);
    }

    // 3. 本机下载仓库 + 解析技能源目录（与本地 install 同一实现，含路径安全校验）。
    let service = crate::services::skill::SkillService::new();
    // _temp_guard 保持存活直到上传完成（作用域末尾才释放），防止下载目录被提前回收。
    let (_temp_guard, _canonical_temp, source_dir, _used_branch) = service
        .download_and_resolve_skill_source(&skill)
        .await
        .map_err(|e| e.to_string())?;

    // 4. 上传远端 SSOT。
    let ssot_dir = crate::remote::skill::remote_ssot_path(&root);
    let remote_dir = format!("{ssot_dir}/{install_name}");
    crate::remote::skill::upload_dir_via_tar(channel, container_ref, &source_dir, &remote_dir)
        .await?;

    // 5. 写 skills.json 记录 + 建链接。
    let description = if skill.description.is_empty() {
        None
    } else {
        Some(skill.description.clone())
    };
    let record = crate::remote::skill::build_remote_skill_record(
        &install_name,
        Some(skill.name.clone()),
        description,
        Some(skill.repo_owner.clone()),
        Some(skill.repo_name.clone()),
        Some(skill.repo_branch.clone()),
        skill.readme_url.clone(),
        apps.clone(),
    );
    records.push(record.clone());
    let use_copy =
        crate::settings::get_skill_sync_method() == crate::services::skill::SyncMethod::Copy;
    crate::remote::skill::sync_remote_skill_links(
        channel,
        container_ref,
        &root,
        &install_name,
        &apps,
        use_copy,
    )
    .await?;
    crate::remote::skill::write_remote_skills_json(&target, &root, &records).await?;

    Ok(record)
}

/// 从本地单个技能目录直接上传到远端 `~/.claude/skills/`（递归）。
/// 宿主机走 SFTP，容器内走 docker exec base64 编码写入。
#[tauri::command]
pub async fn install_remote_skill_from_dir(
    state: State<'_, AppState>,
    host_id: String,
    local_path: String,
    container: Option<String>,
) -> Result<String, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;

    let local_path = std::path::Path::new(&local_path);
    let dir_name = local_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| "无法确定技能目录名".to_string())?;
    let install_name = crate::remote::skill::sanitize_name(&dir_name);
    if install_name.is_empty() {
        return Err("技能目录名无效".to_string());
    }

    let skills_root = crate::remote::skill::remote_ssot_path(&host.default_home());
    let remote_dir = format!("{skills_root}/{install_name}");

    // 检查是否已存在（宿主机 SFTP）
    let fs = crate::fsops::RemoteSftpFileOps {
        sftp: &session.sftp,
    };
    let existing = crate::remote::skill::list_remote_skills(&fs, &host.default_home()).await?;
    if existing.iter().any(|e| e.name == install_name) {
        return Err(format!("远端已存在技能 {install_name}，请先删除再导入"));
    }

    crate::remote::skill::upload_dir_via_tar(
        &session.channel,
        container.as_deref(),
        local_path,
        &remote_dir,
    )
    .await?;
    Ok(install_name)
}

/// 更新远端某个 Skill：从该 Skill 的 GitHub 仓库重新下载最新版，
/// 替换远端 SSOT 目录并同步链接（本机下载 + 本机算 hash 的方案）。
#[tauri::command]
pub async fn update_remote_skill(
    state: State<'_, AppState>,
    host_id: String,
    skill_id: String,
    container: Option<String>,
) -> Result<crate::remote::skill::RemoteSkillRecord, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::skill::update_remote_skill_impl(
        &target,
        &session.channel,
        container.as_deref(),
        &host.default_home(),
        &skill_id,
    )
    .await
}

/// 检查远端某个目标上各 Skill 是否有更新（对齐本机 `check_updates`）。
/// 返回与本机一致的结果：可更新列表 + 没读到的仓库（不再静默吞掉失败）。
#[tauri::command]
pub async fn check_remote_skill_updates(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<crate::services::skill::SkillUpdateCheckResult, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    crate::remote::skill::check_remote_skill_updates_impl(&target, &host.default_home()).await
}

/// 在远端文件系统上扫描未管理的技能目录（远端「导入已有」的扫描阶段）。
///
/// 容器模式：用合并 shell 脚本一次 exec 收集所有源目录的技能，避免逐文件 exec 调用过多导致延迟。
#[tauri::command]
pub async fn scan_remote_unmanaged_skills(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Vec<crate::remote::skill::RemoteUnmanagedSkill>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let root = host.default_home();

    if let Some(ref c) = container {
        return scan_container_unmanaged_skills(&session.channel, c, &root).await;
    }

    let target = crate::remote::docker::RemoteTarget::new(&session.sftp, &session.channel, None)?;
    crate::remote::skill::scan_remote_unmanaged_skills(&target, &root).await
}

/// 容器内批量扫描：一个 shell 脚本一趟 exec 收完所有源目录。
async fn scan_container_unmanaged_skills(
    channel: &russh::client::Handle<crate::remote::connection::RemoteHandler>,
    container: &str,
    root: &str,
) -> Result<Vec<crate::remote::skill::RemoteUnmanagedSkill>, String> {
    // 已管理技能：读 skills.json
    let json_path = crate::remote::skill::remote_skills_json_path(root);
    let managed_json = crate::remote::connection::exec_command(
        channel,
        &format!(
            "docker exec {} sh -c {}",
            container,
            shell_q_rs(&format!("cat {} 2>/dev/null", shell_q_rs(&json_path)))
        ),
    )
    .await
    .unwrap_or_default();
    let managed: std::collections::HashSet<String> = if managed_json.trim().is_empty() {
        Default::default()
    } else {
        serde_json::from_str::<Vec<crate::remote::skill::RemoteSkillRecord>>(&managed_json)
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.directory)
            .collect()
    };

    // 源目录列表
    let sources: [(&str, &str); 8] = [
        ("cc-switch", ".cc-switch/skills"),
        ("claude", ".claude/skills"),
        ("codex", ".codex/skills"),
        ("gemini", ".gemini/skills"),
        ("grokbuild", ".grok/skills"),
        ("opencode", ".config/opencode/skills"),
        ("openclaw", ".openclaw/workspace/skills"),
        ("hermes", ".hermes/skills"),
    ];

    let mut unmanaged: std::collections::HashMap<
        String,
        crate::remote::skill::RemoteUnmanagedSkill,
    > = Default::default();

    for (label, app_rel) in &sources {
        let src = format!("{root}/{app_rel}");
        let script = format!(
            "for d in {src}/*/; do name=$(basename \"$d\"); case \"$name\" in .*) continue ;; esac; if [ -f \"$d/SKILL.md\" ]; then echo CCSW_NAME:$name; echo CCSW_PATH:$d; cat \"$d/SKILL.md\"; echo CCSW_END; fi; done",
        );
        let out = crate::remote::connection::exec_command(
            channel,
            &format!("docker exec {} sh -c {}", container, shell_q_rs(&script)),
        )
        .await
        .unwrap_or_default();

        // 解析输出
        for block in out.split("CCSW_END") {
            let block = block.trim();
            if block.is_empty() {
                continue;
            }
            let dir = block
                .lines()
                .find(|l| l.starts_with("CCSW_NAME:"))
                .map(|l| l.trim_start_matches("CCSW_NAME:").to_string());
            let dir = match dir {
                Some(d) => d,
                None => continue,
            };
            if managed.contains(&dir) {
                continue;
            }
            let dir_path = block
                .lines()
                .find(|l| l.starts_with("CCSW_PATH:"))
                .map(|l| l.trim_start_matches("CCSW_PATH:").trim().to_string())
                .unwrap_or_default();

            // 解析 SKILL.md 内容中的 YAML frontmatter
            let filtered: Vec<&str> = block
                .lines()
                .filter(|l| !l.starts_with("CCSW_NAME:") && !l.starts_with("CCSW_PATH:"))
                .collect();
            let content = filtered.join("\n");
            let parts: Vec<&str> = content.trim().splitn(3, "---").collect();
            let (display_name, description) = if parts.len() >= 3 {
                match serde_yaml::from_str::<serde_json::Value>(parts[1].trim()) {
                    Ok(meta) => (
                        meta.get("name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        meta.get("description")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    ),
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };
            let name = display_name.unwrap_or_else(|| dir.clone());
            let label_str = label.to_string();
            unmanaged
                .entry(dir.clone())
                .and_modify(|s| s.found_in.push(label_str.clone()))
                .or_insert(crate::remote::skill::RemoteUnmanagedSkill {
                    directory: dir,
                    name,
                    description,
                    found_in: vec![label_str.clone()],
                    path: dir_path,
                });
        }
    }

    let mut out: Vec<crate::remote::skill::RemoteUnmanagedSkill> =
        unmanaged.into_values().collect();
    out.sort_by(|a, b| a.directory.cmp(&b.directory));
    Ok(out)
}

/// 在远端将技能目录复制到 SSOT → 更新 skills.json → 创建链接（symlink 或 copy，取决于本机设置）。
#[tauri::command]
pub async fn import_remote_skill(
    state: State<'_, AppState>,
    host_id: String,
    source_path: String,
    name: String,
    container: Option<String>,
    app: String,
) -> Result<crate::remote::skill::RemoteSkillRecord, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let mut apps = crate::remote::skill::RemoteSkillApps::default();
    apps.set_enabled(&app, true);
    let use_copy =
        crate::settings::get_skill_sync_method() == crate::services::skill::SyncMethod::Copy;
    crate::remote::skill::import_remote_skill_local(
        &target,
        &session.channel,
        container.as_deref(),
        &host.default_home(),
        &source_path,
        &name,
        &apps,
        use_copy,
    )
    .await
}

/// 清理远端 shell 配置中的冲突环境变量（注释 + .bak 备份）。
#[tauri::command]
pub async fn clean_remote_env_conflicts(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<serde_json::Value, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let home = host.default_home();
    let conflicts = crate::remote::env_clean::scan_remote_env_conflicts(&target, &home).await?;
    let cleaned = crate::remote::env_clean::clean_remote_env_conflicts(&target, &conflicts).await?;
    Ok(json!({ "cleaned": cleaned, "total": conflicts.len() }))
}

/// 列出远端主机上的 Docker 容器（`docker ps` 解析），供「目标 = 容器」选择。
#[tauri::command]
pub async fn list_docker_containers(
    state: State<'_, AppState>,
    host_id: String,
) -> Result<Vec<String>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    // 结构化错误前缀：前端据此区分「SSH 层失败」与「docker 命令失败」，
    // 不依赖匹配错误文案（文案可读作人话，类型靠前缀判定）。
    let session = connection::connect(&host, Some(&password))
        .await
        .map_err(|e| format!("SSH_ERR:{e}"))?;
    crate::remote::docker::list_docker_containers(&session.channel)
        .await
        .map_err(|e| format!("DOCKER_ERR:{e}"))
}

/// 检测 CLI 是否安装的**标记**（命令命中时输出；调用方按 `contains` 判断）。
const CLAUDE_INSTALLED_MARKER: &str = "CC_SWITCH_FOUND";

/// app → CLI 二进制名（安装检测用；与前端 APP_ICON_MAP 的 key 一致）。
fn cli_binary_for_app(app: &str) -> Option<&'static str> {
    match app {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "gemini" => Some("gemini"),
        "grokbuild" => Some("grok"),
        "opencode" => Some("opencode"),
        "openclaw" => Some("openclaw"),
        "hermes" => Some("hermes"),
        "pi" => Some("pi"),
        // MiniMax Code 的终端版 CLI 就叫 mcode（官方 release notes 原文），
        // 与桌面版共用 ~/.minimax 数据目录。
        "mcode" => Some("mcode"),
        _ => None,
    }
}

/// 生成「检测指定 app 的 CLI 是否安装」的 shell 命令。
///
/// 不用「输出非空」判断（stderr 混流/时序抖动都会误判），改用固定哨兵：
/// 命中则输出 `CC_SWITCH_FOUND`，未命中 stderr 丢弃、无哨兵。
/// `|| true` 保证命令本身成功退出，避免非零退出码带来的读取歧义。
/// `container` 为 Some 时包一层 `docker exec <c> sh -c '...'`。
fn cli_installed_probe(app: &str, container: Option<&str>) -> Result<String, String> {
    let bin = cli_binary_for_app(app).ok_or_else(|| format!("未知应用: {app}"))?;
    let inner = format!(
        "command -v {bin} 2>/dev/null && echo {} || true",
        CLAUDE_INSTALLED_MARKER
    );
    Ok(match container {
        Some(c) => format!("docker exec {c} sh -c '{inner}'"),
        None => inner,
    })
}

/// 批量探测主机在线状态（目标选择器下拉打开时调用）。
///
/// 并行探活、每台 5 秒超时（覆盖内部 connect 的 10 秒总超时，保证下拉快速响应）。
/// 复用连接池：池中已有连接秒回，且探活成功的同时为后续操作预热连接。
#[tauri::command]
pub async fn probe_hosts_online(
    state: State<'_, AppState>,
    host_ids: Vec<String>,
) -> Result<std::collections::HashMap<String, bool>, String> {
    use futures::future::join_all;

    let mut tasks = Vec::with_capacity(host_ids.len());
    for id in &host_ids {
        let host = match load_host(&state, id) {
            Ok(h) => h,
            Err(_) => continue, // 已被删除的主机跳过
        };
        let password = match resolve_password(&host) {
            Ok(p) => p,
            Err(_) => continue, // 无密码（未保存）跳过
        };
        tasks.push(async move {
            let ok = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                connection::connect(&host, Some(&password)),
            )
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false);
            (id.clone(), ok)
        });
    }

    let mut online: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for (id, ok) in join_all(tasks).await {
        online.insert(id, ok);
    }
    Ok(online)
}

/// 设置远端 OpenClaw 的默认模型（对齐本机 `openclawApi.setDefaultModel`）。
/// `default_model` 形如 `{"primary": "prov/模型id", "fallbacks": [...]}`。
///
/// 写 **`agents.defaults.model`** —— openclaw 实际读取的键（读 `docs/tools/*.md` 的
/// `agents.defaults.model.primary` 语义）；旧实现误写 `models.defaultModel`，
/// 该键 openclaw 不读 → 切了不生效。同时做保形回写（json-five round-trip，
/// 保留顶层注释）与 expected_hash 脏写防护。
#[tauri::command]
pub async fn set_remote_openclaw_default_model(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    default_model: serde_json::Value,
) -> Result<(), String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let config_path = format!("{home}/.openclaw/openclaw.json");

    // 读远端 openclaw.json（JSON5；不存在/空 → 默认骨架起步）
    let read = session.read_remote_text(&config_path, container.as_deref()).await?;
    let source = read.as_deref();

    let new_text =
        crate::openclaw_config::set_default_model_preserve_format(source, &default_model)
            .map_err(|e| format!("处理远端 {config_path} 失败: {e}"))?;

    let expected_hash = source.map(|t| format!("{:x}", Sha256::digest(t.as_bytes())));
    session
        .write_settings_with_backup(&config_path, &new_text, container.as_deref(), expected_hash.as_deref())
        .await?;
    Ok(())
}

/// 获取远端 OpenClaw 的默认模型（对齐本机 `openclawApi.getDefaultModel`）。
/// 从 `agents.defaults.model` 读取（与 set 同一键）。
#[tauri::command]
pub async fn get_remote_openclaw_default_model(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Option<serde_json::Value>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let config_path = format!("{home}/.openclaw/openclaw.json");

    let merged: serde_json::Value = match session
        .read_remote_text(&config_path, container.as_deref())
        .await?
    {
        Some(t) => json5::from_str(&t).unwrap_or_else(|_| json!({})),
        None => json!({}),
    };
    let default_model = merged
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("model"))
        .cloned();
    Ok(default_model)
}

// ============================================================================
// 远端 OpenClaw 配置管理（env / tools / agents.defaults）
// ============================================================================

/// 获取远端 OpenClaw 的 env 配置（对齐本机 `openclawApi.getEnv`）。
#[tauri::command]
pub async fn get_remote_openclaw_env(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<crate::openclaw_config::OpenClawEnvConfig, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let config_path = format!("{home}/.openclaw/openclaw.json");

    let merged: serde_json::Value = match session
        .read_remote_text(&config_path, container.as_deref())
        .await?
    {
        Some(t) => json5::from_str(&t).unwrap_or_else(|_| json!({})),
        None => json!({}),
    };

    let env_value = merged.get("env").cloned().unwrap_or_else(|| json!({}));
    serde_json::from_value(env_value)
        .map_err(|e| format!("解析远端 OpenClaw env 配置失败: {e}"))
}

/// 设置远端 OpenClaw 的 env 配置（对齐本机 `openclawApi.setEnv`）。
#[tauri::command]
pub async fn set_remote_openclaw_env(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    env: crate::openclaw_config::OpenClawEnvConfig,
) -> Result<(), String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let config_path = format!("{home}/.openclaw/openclaw.json");

    let read = session.read_remote_text(&config_path, container.as_deref()).await?;
    let source = read.as_deref();

    let env_value = serde_json::to_value(&env)
        .map_err(|e| format!("序列化 env 配置失败: {e}"))?;
    let new_text =
        crate::openclaw_config::set_section_preserve_format(source, "env", &env_value)
            .map_err(|e| format!("处理远端 {config_path} 失败: {e}"))?;

    let expected_hash = source.map(|t| format!("{:x}", Sha256::digest(t.as_bytes())));
    session
        .write_settings_with_backup(&config_path, &new_text, container.as_deref(), expected_hash.as_deref())
        .await?;
    Ok(())
}

/// 获取远端 OpenClaw 的 tools 配置（对齐本机 `openclawApi.getTools`）。
#[tauri::command]
pub async fn get_remote_openclaw_tools(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<crate::openclaw_config::OpenClawToolsConfig, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let config_path = format!("{home}/.openclaw/openclaw.json");

    let merged: serde_json::Value = match session
        .read_remote_text(&config_path, container.as_deref())
        .await?
    {
        Some(t) => json5::from_str(&t).unwrap_or_else(|_| json!({})),
        None => json!({}),
    };

    let tools_value = merged.get("tools").cloned().unwrap_or_else(|| json!({}));
    serde_json::from_value(tools_value)
        .map_err(|e| format!("解析远端 OpenClaw tools 配置失败: {e}"))
}

/// 设置远端 OpenClaw 的 tools 配置（对齐本机 `openclawApi.setTools`）。
#[tauri::command]
pub async fn set_remote_openclaw_tools(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    tools: crate::openclaw_config::OpenClawToolsConfig,
) -> Result<(), String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let config_path = format!("{home}/.openclaw/openclaw.json");

    let read = session.read_remote_text(&config_path, container.as_deref()).await?;
    let source = read.as_deref();

    let tools_value = serde_json::to_value(&tools)
        .map_err(|e| format!("序列化 tools 配置失败: {e}"))?;
    let new_text =
        crate::openclaw_config::set_section_preserve_format(source, "tools", &tools_value)
            .map_err(|e| format!("处理远端 {config_path} 失败: {e}"))?;

    let expected_hash = source.map(|t| format!("{:x}", Sha256::digest(t.as_bytes())));
    session
        .write_settings_with_backup(&config_path, &new_text, container.as_deref(), expected_hash.as_deref())
        .await?;
    Ok(())
}

/// 获取远端 OpenClaw 的 agents.defaults 配置（对齐本机 `openclawApi.getAgentsDefaults`）。
#[tauri::command]
pub async fn get_remote_openclaw_agents_defaults(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Option<crate::openclaw_config::OpenClawAgentsDefaults>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let config_path = format!("{home}/.openclaw/openclaw.json");

    let merged: serde_json::Value = match session
        .read_remote_text(&config_path, container.as_deref())
        .await?
    {
        Some(t) => json5::from_str(&t).unwrap_or_else(|_| json!({})),
        None => json!({}),
    };

    let defaults_value = merged
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .cloned();

    match defaults_value {
        Some(v) => {
            let defaults: crate::openclaw_config::OpenClawAgentsDefaults =
                serde_json::from_value(v)
                    .map_err(|e| format!("解析远端 OpenClaw agents.defaults 失败: {e}"))?;
            Ok(Some(defaults))
        }
        None => Ok(None),
    }
}

/// 设置远端 OpenClaw 的 agents.defaults 配置（对齐本机 `openclawApi.setAgentsDefaults`）。
#[tauri::command]
pub async fn set_remote_openclaw_agents_defaults(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
    defaults: crate::openclaw_config::OpenClawAgentsDefaults,
) -> Result<(), String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let config_path = format!("{home}/.openclaw/openclaw.json");

    let read = session.read_remote_text(&config_path, container.as_deref()).await?;
    let source = read.as_deref();

    // 构建 agents.defaults JSON
    let defaults_value = serde_json::to_value(&defaults)
        .map_err(|e| format!("序列化 agents.defaults 失败: {e}"))?;

    let new_text =
        crate::openclaw_config::set_nested_section_preserve_format(source, "agents", "defaults", &defaults_value)
            .map_err(|e| format!("处理远端 {config_path} 失败: {e}"))?;

    let expected_hash = source.map(|t| format!("{:x}", Sha256::digest(t.as_bytes())));
    session
        .write_settings_with_backup(&config_path, &new_text, container.as_deref(), expected_hash.as_deref())
        .await?;
    Ok(())
}

/// 获取远端 Hermes 的 `model` 段（对齐本机 `get_hermes_model_config`）。
/// 远端「设为默认 / 切换」写远端 config.yaml 的 model.provider，前端按钮态必须读
/// 同一文件才能正确高亮「当前激活」；本机命令读本机文件，远端目标下会读到错误数据。
#[tauri::command]
pub async fn get_remote_hermes_model_config(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Option<crate::hermes_config::HermesModelConfig>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    crate::remote::hermes::read_remote_hermes_model_config(&session, container.as_deref(), &home)
        .await
}

/// 按 id 加载主机，不存在时报错。
fn load_host(state: &AppState, host_id: &str) -> Result<RemoteHost, String> {
    let host = state
        .db
        .get_remote_host(host_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "远程主机不存在，可能已被删除".to_string())?;
    // 软禁用兜底：所有远端命令都经 load_host（约 40 处），此处拦截即全量拒绝。
    // 即使前端漏过滤，禁用的主机也无法被任何操作读到。
    if host.disabled {
        return Err(format!(
            "远程主机「{}」已被禁用，请先在远程主机管理页启用后再操作",
            host.name
        ));
    }
    Ok(host)
}

/// 解析连接用密码：优先系统钥匙串；否则要求编辑主机补充密码。
fn resolve_password(host: &RemoteHost) -> Result<String, String> {
    // debug：38 处远端命令都会调它，每次远程操作都打 info 会让日志暴涨
    //（实测 17MB 日志里 resolve_password 占约 29%）。需要追踪时调高级别可见。
    log::debug!("[remote] resolve_password: id={}", host.id);
    #[cfg(target_os = "windows")]
    let pw = credentials::get_password(&host.id).map_err(|e| {
        log::error!("[remote] 钥匙串读取失败: {e}");
        e
    })?;
    #[cfg(not(target_os = "windows"))]
    let pw: Option<String> = None;
    match pw {
        Some(p) => {
            log::info!("[remote] 钥匙串命中 id={}", host.id);
            Ok(p)
        }
        None if host.auth_method == AuthMethod::Key => {
            // 密钥认证：无口令（未加密私钥）时合法，返回空串；有口令时存的就是口令
            Ok(String::new())
        }
        None => {
            log::error!("[remote] 钥匙串未命中 id={}", host.id);
            Err("未找到该主机的密码，请在编辑界面重新填写".to_string())
        }
    }
}

/// 远端供应商余量查询：读目标机器（宿主机 / 容器）自己的 SSOT 拿到该 provider
/// 的 usage_script 配置，再走与本机完全一致的分派逻辑执行查询。
///
/// 本机路径 `queryProviderUsage` 从本机 SQLite 读 provider；远端路径这里从
/// 远端 SSOT 读 provider，然后共用 `commands::provider::query_usage_for_provider`
/// 这一份分派逻辑，保证远端与本地行为一致（远端向本机看齐，见 AGENTS.md）。
#[allow(non_snake_case)]
#[tauri::command]
pub async fn query_remote_provider_usage(
    state: State<'_, AppState>,
    copilot_state: State<'_, crate::CopilotAuthState>,
    xai_state: State<'_, crate::XaiOAuthState>,
    host_id: String,
    container: Option<String>,
    app: String,
    provider_id: String,
) -> Result<crate::provider::UsageResult, String> {
    log::info!(
        "[remote] query_remote_provider_usage 调用 host={host_id} app={app} provider={provider_id}"
    );
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    let ssot = crate::remote::providers::read_remote_providers_ssot(&target, &home, &app).await?;
    let provider = ssot
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| format!("远端供应商 {provider_id} 不存在"))?;

    let app_type = crate::app_config::AppType::from_str(&app)
        .map_err(|e| format!("未知应用类型: {e}"))?;
    crate::commands::provider::query_usage_for_provider(
        Some(provider),
        &copilot_state,
        &xai_state,
        app_type,
    )
    .await
}

/// 远端目标下检测 Provider 连通性：provider 定义从该远端 SSOT 读取，但连通性
/// 检查在本机执行（测的是本机到 API 的网络，与本机场景一致）。
/// 用户关心的是本机能否连上该 API，而非远端机器的网络。
#[tauri::command]
pub async fn stream_check_remote_provider(
    state: State<'_, AppState>,
    copilot_state: State<'_, crate::CopilotAuthState>,
    host_id: String,
    container: Option<String>,
    app: String,
    provider_id: String,
) -> Result<crate::services::stream_check::StreamCheckResult, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let home = host.default_home();
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;

    let ssot = crate::remote::providers::read_remote_providers_ssot(&target, &home, &app).await?;
    let provider = ssot
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| format!("远端供应商 {provider_id} 不存在"))?;

    let app_type = crate::app_config::AppType::from_str(&app)
        .map_err(|e| format!("未知应用类型: {e}"))?;
    // v4.0.4：连通性检查统一走 StreamCheckService::check_with_retry（配置用默认值；
    // Copilot 的 base_url 动态解析在本机场景才需要，远端场景传 None）。
    crate::services::stream_check::StreamCheckService::check_with_retry(
        &app_type,
        provider,
        &crate::services::stream_check::StreamCheckConfig::default(),
        None,
    )
    .await
    .map_err(|e| e.to_string())
}

// ============================================================================
// 远端 Hermes 记忆（A2）：~/.hermes/memories/{MEMORY,USER}.md + config.yaml 开关
// ============================================================================

fn remote_hermes_memory_path(home: &str, kind: crate::hermes_config::MemoryKind) -> String {
    format!("{home}/.hermes/memories/{}", kind.filename())
}

fn remote_hermes_config_path(home: &str) -> String {
    format!("{home}/.hermes/config.yaml")
}

#[tauri::command]
pub async fn get_remote_hermes_memory(
    state: State<'_, AppState>,
    host_id: String,
    kind: crate::hermes_config::MemoryKind,
    container: Option<String>,
) -> Result<String, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let path = remote_hermes_memory_path(&host.default_home(), kind);
    Ok(target.read_text_optional(&path).await?.unwrap_or_default())
}

#[tauri::command]
pub async fn set_remote_hermes_memory(
    state: State<'_, AppState>,
    host_id: String,
    kind: crate::hermes_config::MemoryKind,
    content: String,
    container: Option<String>,
) -> Result<(), String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let path = remote_hermes_memory_path(&host.default_home(), kind);
    target.write_text_atomic(&path, &content).await
}

#[tauri::command]
pub async fn get_remote_hermes_memory_limits(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<crate::hermes_config::HermesMemoryLimits, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let raw = target
        .read_text_optional(&remote_hermes_config_path(&host.default_home()))
        .await?
        .unwrap_or_default();
    Ok(crate::hermes_config::memory_limits_from_yaml(&raw))
}

#[tauri::command]
pub async fn set_remote_hermes_memory_enabled(
    state: State<'_, AppState>,
    host_id: String,
    kind: crate::hermes_config::MemoryKind,
    enabled: bool,
    container: Option<String>,
) -> Result<(), String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let path = remote_hermes_config_path(&host.default_home());
    let raw = target.read_text_optional(&path).await?.unwrap_or_default();
    let updated = crate::hermes_config::set_memory_enabled_in_yaml(&raw, kind, enabled)?;
    target.write_text_atomic(&path, &updated).await
}

// ============================================================================
// 远端 OpenClaw 工作区（A1）：~/.openclaw/workspace/
// ============================================================================

fn remote_workspace_dir(home: &str) -> String {
    format!("{home}/.openclaw/workspace")
}

#[tauri::command]
pub async fn read_remote_workspace_file(
    state: State<'_, AppState>,
    host_id: String,
    filename: String,
    container: Option<String>,
) -> Result<Option<String>, String> {
    crate::commands::validate_filename(&filename)?;
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let path = format!("{}/{filename}", remote_workspace_dir(&host.default_home()));
    target.read_text_optional(&path).await
}

#[tauri::command]
pub async fn write_remote_workspace_file(
    state: State<'_, AppState>,
    host_id: String,
    filename: String,
    content: String,
    container: Option<String>,
) -> Result<(), String> {
    crate::commands::validate_filename(&filename)?;
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let path = format!("{}/{filename}", remote_workspace_dir(&host.default_home()));
    target.write_text_atomic(&path, &content).await
}

#[tauri::command]
pub async fn list_remote_daily_memory_files(
    state: State<'_, AppState>,
    host_id: String,
    container: Option<String>,
) -> Result<Vec<crate::commands::DailyMemoryFileInfo>, String> {
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let dir = format!("{}/memory", remote_workspace_dir(&host.default_home()));
    let mut files = Vec::new();
    let entries = match target.read_dir(&dir).await {
        Ok(entries) => entries,
        Err(_) => return Ok(Vec::new()),
    };
    for entry in entries {
        if entry.is_dir || !entry.name.ends_with(".md") {
            continue;
        }
        let content = target
            .read_text_optional(&entry.path)
            .await?
            .unwrap_or_default();
        let size = target.file_size(&entry.path).await.unwrap_or(0);
        files.push(crate::commands::daily_memory_info(
            entry.name, size, 0, &content,
        ));
    }
    files.sort_by(|a, b| b.filename.cmp(&a.filename));
    Ok(files)
}

#[tauri::command]
pub async fn read_remote_daily_memory_file(
    state: State<'_, AppState>,
    host_id: String,
    filename: String,
    container: Option<String>,
) -> Result<Option<String>, String> {
    crate::commands::validate_daily_memory_filename(&filename)?;
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let path = format!(
        "{}/memory/{filename}",
        remote_workspace_dir(&host.default_home())
    );
    target.read_text_optional(&path).await
}

#[tauri::command]
pub async fn write_remote_daily_memory_file(
    state: State<'_, AppState>,
    host_id: String,
    filename: String,
    content: String,
    container: Option<String>,
) -> Result<(), String> {
    crate::commands::validate_daily_memory_filename(&filename)?;
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let path = format!(
        "{}/memory/{filename}",
        remote_workspace_dir(&host.default_home())
    );
    target.write_text_atomic(&path, &content).await
}

#[tauri::command]
pub async fn delete_remote_daily_memory_file(
    state: State<'_, AppState>,
    host_id: String,
    filename: String,
    container: Option<String>,
) -> Result<(), String> {
    crate::commands::validate_daily_memory_filename(&filename)?;
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let path = format!(
        "{}/memory/{filename}",
        remote_workspace_dir(&host.default_home())
    );
    if target.exists(&path).await {
        target.remove_file(&path).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn search_remote_daily_memory_files(
    state: State<'_, AppState>,
    host_id: String,
    query: String,
    container: Option<String>,
) -> Result<Vec<crate::commands::DailyMemorySearchResult>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let host = load_host(&state, &host_id)?;
    let password = resolve_password(&host)?;
    let session = connection::connect(&host, Some(&password)).await?;
    let target = crate::remote::docker::RemoteTarget::new(
        &session.sftp,
        &session.channel,
        container.as_deref(),
    )?;
    let dir = format!("{}/memory", remote_workspace_dir(&host.default_home()));
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();
    let entries = match target.read_dir(&dir).await {
        Ok(entries) => entries,
        Err(_) => return Ok(Vec::new()),
    };
    for entry in entries {
        if entry.is_dir || !entry.name.ends_with(".md") {
            continue;
        }
        let content = target
            .read_text_optional(&entry.path)
            .await?
            .unwrap_or_default();
        let size = target.file_size(&entry.path).await.unwrap_or(0);
        if let Some(result) =
            crate::commands::daily_memory_search_result(entry.name, size, 0, &content, &query_lower)
        {
            results.push(result);
        }
    }
    results.sort_by(|a, b| b.filename.cmp(&a.filename));
    Ok(results)
}
