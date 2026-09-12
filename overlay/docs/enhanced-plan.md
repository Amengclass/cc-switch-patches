# CC Switch 增强版 — 远程主机统一控制面

> **定位**:在 farion1231/cc-switch 基础上 **Fork**,保持上游完整(不裁剪),新增 **SSH 远程主机管理**。
> **目标**:「**一处配置、多机生效**」的统一控制平面(定义 Provider 于一处,应用到本机 + 任意远程主机),**不是**单纯的文件同步工具。
>
> 与原版的差异:
> - 原版 Web 版(端口 17666)是「在服务器上开网页,改服务器自己的配置」;
> - 增强版是「**本机 GUI 直连任意远程主机,直接读写远端 `~/.claude/settings.json`**」。
>
> **同步策略**:不裁剪上游,远程能力作为独立新增层叠加。上游同步 = 纯 `git merge`,零冲突。
>
> **守则**:改动前必读项目根 `AGENTS.md` —— 第一要素:不破坏本机(原生 cc-switch)功能,本机代码能不改就不改;本机逻辑是完善远端功能的标准参考。

---

## 范围总览

**保留(上游完整,不做裁剪):**
- 全部既有功能:多 CLI 供应商切换、MCP、Prompts、Skills、会话管理、WebDAV/S3 云同步、深链等

**新增(本项目的核心):** SSH 远程主机管理 —— 连接 → 读远端配置 → 切换 Provider → 原子写回 → 清理冲突环境变量 → 明确提示生效方式;并延伸出远程会话管理、Docker 容器目标。新增部分只读写远端 `~/.claude/`,不碰上游既有逻辑。

---

## 功能模块总览

> 全文按 4 大功能模块组织,每个模块下有「已完成」与「待完成」。

| 模块 | 范围 | 状态速览 |
|---|---|---|
| **模块① 远程主机统一控制面(SSH)** | 远程主机管理/切换 Provider/远程面板/Docker 容器/提速/per-app 扩展/安装检测 | 已完成 11 项;待完成 12 项(见「待完成」模块①) |
| **模块② 悬浮窗组件(加速球)** | 桌面小球/悬停面板/右键菜单/定位与尺寸保护/路由状态同步 | 已完成(含样式打磨) |
| **模块③ 本地体验增强** | UI 修复/图像拦截钩子/前端刷新策略一致性/更新提示 | 已完成 4 项;待完成 5 项 |
| **模块④ 基座升级与工程** | 官方 v3.20.1 合并/构建与发布 | 已完成 3 项;待完成 1 项 |

---

## 已完成 ✅(按功能模块)

### 模块① 远程主机统一控制面(SSH)

### 远程主机连接与管理
- 远程主机 CRUD(名称、host、port、用户名、密码认证),配置存 SQLite
- 测试连接(russh,超时控制)
- 密码用 Windows DPAPI 加密保存(`~/.cc-switch/remote_passwords.json`)
- 读取远端 `~/.claude/settings.json` 并在 GUI 展示当前 Provider / 模型 / env 现状

### 远程切换 Provider
- 对远端执行供应商切换:env 块合并 → 原子写回(远端临时文件 + rename)
- 切换失败回滚(保留远端原文件 `.bak`)
- 切换后明确提示生效方式(本机/远程、claude 运行中/未运行)

### 远程切换提速与 per-app 扩展(2026-08-07)
- **切换提速**:宿主机落盘从多趟 SFTP(15~20 RTT)改为**一次 exec + stdin 管道**——
  `RemoteSession.exec_with_stdin`(强制 `sh -c`)+ `write_settings_with_backup`:
  `[ -f path ] && cp path path.bak; mkdir -p && base64 -d > tmp && mv tmp path || rm -f tmp`,
  宿主机/容器共用脚本,~1 RTT;失败自动清理 tmp
- **脏写防护(预留)**:`write_settings_with_backup` 保留 `expected_hash` 参数(写前读 sha256,
  同一脚本内校验,冲突输出 `REMOTE_CONFLICT` abort,0 额外 RTT);**当前 claude 与 codex 均传
  None —— 与各 app 本机切换行为完全一致(无校验整文件覆盖)**,机制仅作预留
- **EffectReport.current_provider_id**:`switch_remote_provider` 一次返回当前供应商 id,
  前端省一次 `get_remote_current_provider` IPC
- **前端 mutation 同构**:新增 `useSwitchRemoteProviderMutation`(remoteMutations.ts),
  与本机 `useSwitchProviderMutation` 同构(onSuccess 回写高亮+invalidateQueries+toast,
  onError 统一 toast);ProviderCard 切换按钮 `isSwitching` 禁用防连点
- **远程切换 per-app(阶段 1:codex MVP)**:`switch_remote_provider` 加 `app` 参数;
  claude 保持整文件覆盖 settings.json;codex 走 `remote/codex.rs`,**产出与本机逐字节一致**——
  ① modelCatalog:复用本机 `prepare_codex_catalog_plan`,有 catalog 则写远端同名
  `cc-switch-model-catalog.json` + 注入字段,无则清理/web_search 兜底;
  ② 配置文本:复用本机 `build_codex_live_config`(官方+统一会话开关开启 → 注入 unified 路由;
  bearer token 注入);
  ③ auth.json 判定:与本机 `should_write_auth` 一致(官方+登录材料,或第三方且
  「不保留官方登录态」设置 → 写;否则保留远端登录态);
  无 hash 校验原子写回(与本机一致),`remote/codex.rs` 只做远端 I/O
- **当前供应商记录 per-app**:`remote_current_providers.json` 结构 `host -> {app -> id}`,
  兼容老格式(字符串视为 claude)
- **目标选择器全 app 放开**:`TargetBreadcrumb` 不再限 `activeApp === "claude"`,
  所有 app 页面可选中远程服务器/容器;Sessions 面板非 claude 时忽略远程目标(避免误读远端 ~/.claude/projects)
- **安装检测全 app 化**:`check_local_cli_installed(app)` / `check_remote_cli_installed(host, app, container)`,
  `cli_binary_for_app` 映射(claude/codex/gemini/grokbuild→grok/opencode/openclaw/hermes);
  前端徽标动态显示「{App} 已安装/未安装」+ 按 app 给安装命令(`APP_INSTALL_CMDS`)

### 远端路由/接管(走本机代理,2026-08-11)

> 宿主机/容器上的 app 经 SSH 反向隧道走**本机**代理(模型映射/格式转换),与本机「接管」语义对齐。

- **反向隧道对账(不重建连接)**:`tcpip_forward`/`cancel_tcpip_forward` 在**取连接时按宿主当前意图对账**——
  开→补隧道、关→撤隧道,全程复用池内连接、不断线;开关命令立即对账 + 取连接兜底对账双保险
- **隧道端口动态化**:读 `ProxyStatus.port`(默认 15721),隧道/base_url/DNAT 三处统一同一端口
- **per-container DNAT**:容器走路由时按容器 IP(`-s <ip>`)下发独立 DNAT 规则,只影响该容器;
  容器切回直连按 IP 精确删除;host 网络容器无需 DNAT(共享宿主机回环)
- **容器×app 独立接管开关**:`remote_hosts.route_proxy_container_apps`(`{"<容器>":{"claude":true}}`),
  宿主机/容器A/容器B 三者开关互不影响;切换按目标分流读取(`route_proxy_for_target`)
- **隧道失败降级 + 回退 + 提示**:写 live 前检查 `session.tunnel_is_active()`——隧道未建立(如 sshd
  禁了 `AllowTcpForwarding`)→ 按直连写入 + `EffectReport.warnings` 醒目提示 + **DB 开关回退为 false**
  (reapply 与 switch 双路径一致)
- **远端目标隐藏 Claude Desktop**:远端无此应用,AppSwitcher 过滤 claude-desktop tab + 切远端自动跳回 claude
- **claude-desktop 下远端入口全隐藏(2026-08-14)**:除「选远端后才切回 claude」外,补成在 claude-desktop 标签下
  自身就把所有远端入口隐藏(`remoteAvailableForApp` 门控 = `remoteFeatureEnabled && activeApp !== "claude-desktop"`):
  远程主机导航按钮、目标选择器(含远端主机/容器下拉项)、批量应用按钮、远端状态栏、BatchApplyPanel 挂载全隐藏,还原本机体验。
- **进程生命周期**:任一 app 远端接管(含容器)开启 → 自动拉起本机代理;全关 → 自动停止(any_route_consumer
  含容器判断,stop_with_restore 连容器字段一起清空并异步重写远端直连)
- **宿主机仅需开放 22 端口**:隧道复用 SSH 连接,15721 由 sshd 监听在回环,无需额外开放端口
- **端口来源同源化(2026-08-11)**:`current_proxy_port` 未运行不再硬编码 15721,改读配置 `listen_port`;
  配置为 0 时明确报错(与本机 `build_proxy_urls` 同款),隧道/base_url/DNAT 端口来源单点化,改端口不再分叉
- **启动按远端接管意图自动拉起代理(2026-08-11)**:重启后若 DB 仍有远端接管意图(宿主机/容器任一 app),
  `restore_proxy_state_on_startup` 自动 `start()` 拉起本机代理(隧道由下次取连接对账自动建立),
  与本机接管启动恢复对齐——开关显示开、流心即亮,不再需要手动抖一下
- **隧道回连地址动态化(2026-08-11)**:`server_channel_open_forwarded_tcpip` 回连目标从"远端传来的
  connected_address"改为 `tunnel_host():tunnel_port()`(本机代理实际监听处,0.0.0.0/:: 归一化),
  监听地址一改(如自定义局域网 IP)隧道桥接不再失效
- **隧道地址单点化**:`REMOTE_TUNNEL_LISTEN_ADDR` 常量(connection.rs),8 处引用统一指向;
  与本机回连 `TUNNEL_HOST`(127.0.0.1 默认)语义区分,连接层模块注释完整记录两地址机制
- **前端状态刷新对齐本机(2026-08-11)**:远端接管开关操作后 invalidate `proxyKeys.status` +
  `takeoverStatus`(开/关都刷——`running=false` 时不轮询,开接管必须主动刷流心才及时出现);
  关闭时 removeQueries(`providerHealth`/`circuitBreakerStats`)对齐 stopWithRestore 清缓存
- **走马灯(流心)CSS 修复**:`route-service-live` 改用 `@property` 只旋转 conic 渐变角度,
  不再 rotate 整个方形元素——流动边框严格沿灰色圆角框轮廓,四角不再偏离
- **停代理全局判断双向对称**:本机 `set_takeover_for_app(false)`(proxy.rs:906-937)与远端
  `any_route_consumer`(commands.rs:375-393)都检查「本机接管 + 所有远端宿主机 + 所有容器」——
  容器全关但宿主机/本机仍用代理则不停;唯一停代理条件 = 全局所有 app 都不使用
- **隧道状态主机级共享(2026-08-12 修复)**:隧道是「主机」级资源——任一连接绑定远端端口即整机可用,
  并发多连接只有第一条能绑定,其余 tcpip_forward 被 EADDRINUSE 拒(日志"The request was rejected by the
  other party")不代表没隧道。新增 `HOST_TUNNEL_STATE`(host_id→bool),`tunnel_is_active()` 改查主机级;
  `RemoteSession` 加 host_id + Drop 清状态(连接关闭即释放远端端口)。修复"开接管后 reapply 恰好用
  非持连接 → 误判隧道未建 → 降级直连 + 回退 DB 开关"的间歇性 bug
- **连接池持隧道者不被替换(2026-08-12 修复)**:`put_back` 时池里已有一条持有隧道的连接则不替换——
  并发操作(开关命令 + reapply + onUpdated 触发的远端面板刷新)会创建冗余连接,若把持隧道者挤出池,
  其连接被关、隧道丢失,reapply 又误判降级。配合主机级状态,reapply 无论拿到哪条连接都能识别隧道可用
- **前端开关状态跟随后端真实回退 + 暴露隧道警告(2026-08-12)**:reapply 后重拉主机列表
  (`onHostRefreshed` 只更新 servers 不 invalidate 远端供应商,避免二次刷新抖两次),隧道失败回退 DB 时
  开关 UI 立即归位;reapply 返回的 `report.warnings` 弹 toast 明确提示(不再吞掉误报成功)
- **流心即时刷新恢复(2026-08-12)**:上游 clone 丢失了"开关后 invalidate `proxyKeys.status`/`takeoverStatus`"
  的增强(`running=false` 时 useProxyStatusQuery 不轮询,开接管后不主动刷则流心不亮)——已补回,
  与文档上文"前端状态刷新对齐本机"一致
- **供应商页加载文案统一(2026-08-12)**:首载("正在读取 {App} 配置…")与后台刷新("正在刷新远端配置…")
  两套措辞统一为前者(带 app 名,首载/刷新共用 `remoteLoadingLabel`);补 `remote.readingConfigPrefix/Suffix`
  到 zh/zh-TW/en/ja 四语(此前 `remote.*` 全无 locale key,全硬编码中文)
- **类型清理(2026-08-12)**:remote.ts 5 处 invoke 返回值类型补 `route_proxy_enabled`(后端 RemoteProvidersView
  已有该字段,前端声明漏了,tsc 报错);RemoteHostsPanel 删未用 Plus import。`pnpm typecheck` 恢复全绿

- **remote_current_providers 迁入 SQLite(2026-08-13)**:`~/.cc-switch/remote_current_providers.json` 迁入
  `remote_current_providers` 表(host_id, app, provider_id, provider_config, updated_at;主键 host_id+app);
  旧 json 首次调用自动迁移(改名 `.bak` 兜底);`save_current_provider` 扩展存完整 Provider 配置
  (`provider_config`)——「本机代理按远端路由」解耦方案的数据基座,供下方解耦路由实现消费
- **解耦路由实现:本机代理按「远端各自」的当前供应商路由(2026-08-13)**:远端接管经隧道发到本机代理的
  请求,不再依赖本机当前供应商,而是按来源远端自己存的 provider_config 路由:
  - **写入端**(`remote/providers.rs` 各 app 分支 + `remote/gemini.rs`):走路由时 base_url 保持干净
    (`http://<host>:<port>`,Claude Code 等客户端原样识别),占位 token 写成 `PROXY_MANAGED:<host_id>`
    (如 `ANTHROPIC_AUTH_TOKEN=PROXY_MANAGED:9b2a0411-...`)。gemini 顺带修了 base_url 端口写死
    15721 的问题(改随 `port` 参数)
  - **为什么用 token 标记而非 base_url 路径标记**:远端 app(尤其 Claude Code 用的 Anthropic SDK)对
    base_url 的路径前缀不保证保留——按 origin 重建 URL 的客户端会丢掉路径段,标记直接失效;而 auth 头对
    HTTP 客户端是完全透明的字节,`PROXY_MANAGED:<host_id>` 一定原样到达代理,100% 可靠
  - **代理端**:新模块 `proxy/remote_route.rs`(`detect_host_id_from_headers` 从 authorization /
    x-api-key / x-goog-api-key 扫描 `PROXY_MANAGED:` 前缀 + 单测);`RequestContext::new` 识别 host_id
    后从 `remote_current_providers.provider_config` 反序列化该 host 的完整 Provider 作为唯一 provider
    路由,并把 `current_provider_id` 对齐到它——forwarder 据此**不会误触发本机故障转移切换**(否则会改写
    本机配置)。claude 路由时补 `build_effective_settings_with_common_config` 生效配置(官方供应商原始
    配置缺 base_url/密钥)
  - **回退**:远端无已存配置/解析失败(如升级前迁移的旧行 provider_config 为 NULL)→ 回退本机路由(旧行为,
    优雅降级)。**重开接管 / 重新应用即补齐**(reapply 也写 provider_config,老行自愈)。
    裸 `PROXY_MANAGED`(本机接管占位,无冒号)不会被误判为远端
  - **已知边界**:codex 远端接管固定走官方路由(不写 bearer token,走原生登录),token 标记对它不适用,维持
    旧行为;codex `/v1/models` 模型目录探测仍返回本机 catalog(远端目录在远端机上,不在本机代理范围);
    远端请求不写本机 provider_health(不在本机 providers 表的 provider 静默跳过,无 FK 噪音);
    `current_providers`(active_targets)会显示最后实际使用该 app 的供应商
- **路由残留不导入供应商面板(2026-08-13)**:`get_remote_providers` 的 `sync_remote_live_into_ssot`
  (auto_import_default)会把 live 当前配置导入成 SSOT default——开启路由时 live 是隧道占位配置
  (`http://127.0.0.1:15721` + `PROXY_MANAGED` token),会被当真实配置导入,关路由后仍残留展示。
  修复:settings 含 `PROXY_MANAGED` 占位即视为路由残留——**跳过导入** + **清理 SSOT 已有占位 provider**
  (面板不再显示隧道 URL)
- **容器路由稳健性(2026-08-13)**:修复三类「硬编码 Docker 假设」导致容器接管静默降级直连——
  - **NetworkMode=default 识别**:Docker 对未显式指定网络的容器返回 `default`(默认 bridge 网络),
    原代码只认 host/bridge,把 default 当网络名 `docker network inspect default` 查空 → 误判探测失败
    降级直连(容器接管不生效、配置保持原始)。`resolve_container_network` 把 default 映射为 bridge
  - **DNAT 去掉写死 `-i docker0`**:自定义 bridge 网络容器流量从 `br-<hash>` 进(非 docker0),
    `-i docker0` 规则匹配不到 → 容器连网关 15721 被拒 → 路由静默失败。改为 `-s <容器IP>` 限定
    (源 IP 唯一标识该容器,与入口接口无关)
  - **容器 IP 多网络取首个**:`{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}` 多网络时会把
    多个 IP 无分隔串成一串(如 `172.17.0.8fd00::1`),`.contains('.') 误通过。IP 后加空格分隔取首个
  - **探测加 debug 日志**:detect_container_route_base 打出 mode/gw/ip + 各失败点,下次日志直接可见卡点
### 目标选择器(本机 / 服务器 / 容器)
- 头部连体胶囊式选择器,选服务器后供应商当前高亮 + 切换走远端
- 编辑当前供应商保存后原子写回远端
- 容器目标支持(Docker exec,方案 B):`docker ps` 列表 + 容器下拉
- 下拉可滚动(`max-h-[50vh]` + 细滚动条)
- 聚焦样式与全局 `*:focus-visible` 蓝框一致
- **切主机同步清空容器列表(2026-08-13)**:TargetBreadcrumb 切主机时只清 remoteContainerId 不清
  containers 列表,容器 effect 渲染后才跑且正常路径开头未清空 → 拉取完成前(SSH 往返 1~3 秒甚至超时)
  容器下拉显示上一台主机的容器。修复:切主机同步 setContainers([]) + 容器 effect 拉取前先清空

### 远程功能面板
- **远程会话**:浏览/查看消息/删除远端 `~/.claude/projects/*.jsonl`(复用本机解析,FileOps)
- **远程 MCP**:读/写/增/删远端 `~/.claude.json` 的 `mcpServers`(原子写回)
- **远程 Prompts**:`~/.cc-switch/prompts.json` 存多提示词列表 + 启用那条原子写入 `~/.claude/CLAUDE.md`,与本地 SQLite 完全对称。列表/添加/编辑/删除/启用切换全功能,乐观更新 + toast 提示
- **远程 Skills**:远端 `~/.cc-switch/skills/` SSOT + `skills.json` + symlink/copy 同步,与本地完全对称。列出/删除/导入已有/ZIP 安装/切换应用全功能,跟随 `skill_sync_method` 和 `skill_storage_location` 设置

### Docker 容器支持
- exec 通道(`connection::exec_command`) + 容器列表(`docker ps` 解析)
- `DockerExecFileOps`(容器内文件操作,原子写 = base64 + 临时文件 + mv)
  - **写入方案**：`docker exec -i <容器> sh -c 'base64 -d > <tmp> && mv <tmp> <path>'`，数据通过 SSH channel stdin 管道分块（64KB/块）流式传入，不嵌入命令字符串，无文件大小限制。`connection::exec_command_with_stdin` 封装：`channel_open_session` → `exec` → `data()` 分块写入 → `eof()` → 读输出
- `RemoteTarget` 枚举(宿主机 SFTP / 容器 exec)
- settings/mcp/prompt/skills/sessions/env_clean 全部切为 `<F: FileOps>` 泛型,容器自动获得全部功能
- 前端 `remoteContainerId` 贯穿各面板,按目标分流

### Skills ZIP 安装机制

> 本机 / 远端宿主机 / 远端容器 共用同一套 ZIP 解压 + 扫描逻辑（`extract_local_zip` + `scan_skills_in_dir`），区别仅在第 4 步「搬进 SSOT」的传输方式。

**本机流程：**
1. **本机解压**：`extract_local_zip` → 临时目录（支持 zip 炸弹预算、symlink 解析、`..` 路径防护）
2. **扫描**：`scan_skills_in_dir` → 递归找含 `SKILL.md` 的子目录
3. **去重**：查 SQLite → 取安装名
4. **搬进 SSOT**：`std::fs::copy_dir_recursive(skill_dir, ssot_dir)` — 本地磁盘递归复制，毫秒级
5. **注册**：SQLite INSERT + 计算 content_hash
6. **同步**：创建 symlink 或 copy 到应用目录

**远端宿主机流程：**
- 步骤 1~3 同上
- 步骤 4 `upload_dir_to_remote`：先 `collect_dirs_recursive` 收集子目录 → 远端 `mkdir -p`；再 `collect_files_recursive` 收集所有文件(含二进制) → 每个文件 `std::fs::read` → SFTP `create` + `write` + `flush`
- 步骤 5~6：写远端 `skills.json` + 创建 symlink

**远端容器流程：**
- 步骤 1~3 同上
- 步骤 4 `upload_dir_to_container`：先 `docker exec mkdir -p` 建目录；再每个文件 base64 编码 → `docker exec -i` + stdin 管道(`channel.data()` 64KB/块)传入 → `base64 -d > .ccswitch.tmp && mv .ccswitch.tmp path`（原子写）
- 步骤 5~6：写远端 `skills.json` + 创建 symlink（一次 exec_with_stdin 合并）

| | 本机 | 宿主机 | 容器 |
|---|---|---|---|
| 解压 | `std::fs` | ← 同左 | ← 同左 |
| 搬进 SSOT | `copy_dir_recursive` | SFTP 逐文件 | docker exec + stdin 管道 |
| 原子写 | 本地 mv 原子 | SFTP create 新文件 | tmp + mv 原子 |
| 注册 | SQLite | skills.json SFTP 写 | skills.json stdin 管道 |

### FileOps 抽象
- `FileOps` trait(存在/读头尾行/读目录/删除/软链接等)
- 三个实现:`LocalFileOps`(std::fs)、`RemoteSftpFileOps`(SFTP)、`DockerExecFileOps`(docker exec)
- 所有远端模块(settings/mcp/prompt/skills/sessions/env_clean)只写一次,通过泛型适配本机/远端/容器

### 冲突环境变量清理
- 扫描远端 shell 配置:`~/.bashrc`、`~/.zshrc`、`~/.profile`、`~/.zshenv`、`/etc/environment`
- 识别与目标 Provider 冲突的 `ANTHROPIC_*` 导出(见附录)
- 一键清理:注释或删除 + `.bak` 备份 + 展示 diff + 二次确认

### Claude Code 安装检测
- 通过 SSH `command -v claude` 检测远端安装状态
- 面板徽标 + 测试连接展示

### 远端批量开关优化(C + A)
- **背景**:远端场景批量开关(MCP/Skills)此前前端 `runSequentialBulkAction` 逐条调 `toggle_remote_*`——每条都新建 SSH 连接 + 重复读写同一份 SSOT/live 文件,且点击后要等 SSH 往返图标才有反馈
- **C(真实提速)**:新增后端 `bulk_toggle_remote_mcp_app` / `bulk_toggle_remote_skill_app` 命令,一次连接内改完 N 个开关(SSOT 读一次写一次;同一 app 的 live 文件读一次写一次);skills 把每条链接脚本拼进同一个 `&&` 链,仍是一次 `exec_command_with_stdin`。MCP per-format helper 全部改成 `*_many` 版,单条委托 many。前端批量从 N 次 invoke 变 1 次
- **A(感知提速)**:前端批量 hook 加 `onMutate`/`onError` 乐观更新(照 `failover.ts` 范式),点击瞬间翻转缓存图标,失败回滚,settle 后权威校准
- **返回值**:后端 `RemoteBulkToggleResult{succeeded, failed}` 与前端 `SequentialBulkActionResult` 形状一致,面板零改动;逐条失败(id 不存在)聚合上报
- **测试**:新增 `remote::mcp` 4 个 + `remote::skill` 7 个单元测试(`LocalFileOps` + `tempfile`);前端批量 hook 测试改断言单次 bulk 调用 + 新增部分失败用例
- 验证全过:`pnpm typecheck` / `format:check` / vitest 48 个相关测试 / `cargo test` 全绿 / `cargo clippy` 无新增 warning / 打包启动正常


### 模块② 悬浮窗组件(加速球)

### 悬浮窗(加速球)
- 桌面常驻透明小球 + 悬停展开面板:列出每个 app(Claude Code / Codex / Gemini / Grok Build / OpenCode / OpenClaw / Hermes)的当前供应商 / 模型 / 余量
- 窗口:透明 + 置顶 + 无边框 + 跳过任务栏,`floating.html` 独立 Vite 入口,与主 App 完全分离(`floating-main.tsx` 按窗口 label 分派渲染球/面板)
- 拖动:**Rust 端 `GetCursorPos` 全局光标轮询 + `GetAsyncKeyState` 左键检测**,绕开 WebView 事件;球上任意位置可拖,左键按住拖动画板收起
- 单击(无位移)打开主窗口;松手边缘吸附(阈值可调),吸附速度设置:快/中/慢/关闭(关闭 = 不自动吸附)
- 主题:跟随主应用外观(light/dark/system),`storage` 事件同步 + `set_window_theme`
- 设置:「悬浮组件」独立区块(开关 + 吸附速度)+ 托盘右键「悬浮窗」开关,两处联动
- 余量数据:**只读 `UsageCache` 缓存,绝不主动查 API**;查询全部由主窗口(useUsageQuery / 手动刷新)和托盘悬停发起,面板 3s 轮询 + `usage-cache-updated` / `floating-data-refresh` 事件同步
- **悬浮球显示目标(置顶 app / 跟随最近活跃,2026-08-13)**:修掉「球固定显示 Claude」的不合理
  - 后端 `settings.rs` 新增 `floating_pin_app`(持久化置顶 app)与 `floating_last_app`(provider-switched 事件写入的最近活跃 app)
  - `get_floating_ball_target` 解析目标:**置顶优先,否则最近活跃**,两者皆无退化为默认
  - `floating.rs` 新增 `floating_set_pin_app`(置顶/取消置顶,None=跟随)与 `floating_record_active_app`(球前端 `provider-switched` 时记录)
  - **悬浮球**:不再硬编码 `find(appType==="claude")`,改为读 `get_floating_ball_target` 找对应行;置顶时 app 名旁显示锁图标(`.ball-pin`)
  - **悬浮面板**:每行加图钉按钮(激活态蓝色锁 / 常态灰解锁)单击置顶该 app 到球、再点取消;置顶行高亮(`.row-pin-active`)
  - **设置页「置顶到悬浮球」下拉**:直接读写 `floating_set_pin_app`/`get_floating_ball_target`,与面板图钉同一数据源;选项含「跟随最近使用」
- 余量样式对齐主窗口:标签灰、数值状态色(绿/橙/红,阈值与主窗口一致)+ 等宽数字、刷新时间(时钟图标 + 「刚刚/x分钟前」);供应商/模型名已设置时用主窗口链接蓝(`--primary`)高亮,未设置保持灰
- 多套餐:主行显示第一条,其余套餐逐条补行
- 右键菜单:自定义 HTML 菜单窗口(`floating-menu`,瘦高样式、与面板同款配色);设置 / 分隔线 / 隐藏悬浮窗;与面板**互斥**(同刻只出一个);「固定当前位置」开关在设置页「悬浮组件」区块(固定后不可拖动/不吸附,单击仍开主窗)
- 弹出定位统一:面板与菜单共用 `position_for_ball`——**朝屏幕中央方向展开**且**球贴弹窗靠屏幕边缘侧的角**:球偏左 → 弹窗右展、弹窗左缘=球左缘;球偏右 → 弹窗左展、弹窗右缘=球右缘;球偏上 → 弹窗下展(顶缘=球底缘+间隙)、球偏下 → 弹窗上展(底缘=球顶缘-间隙);某方向放不下翻转该轴;从而**绝不遮挡球体**且面板/菜单位置永远一致;全用逻辑像素 + `set_position(LogicalPosition)`,同一显示器下右缘/底缘精确对齐
- **悬浮窗尺寸保护(修复窗口大小异常,2026-08-07)**:两个根因——(1) `window-state` 插件把悬浮窗恢复成历史错误尺寸(`skip_initial_state` 实测拦不住,改用 **`with_denylist`** 完全排除球/面板/菜单三个窗口);(2) **WebView2 内容加载后把悬浮窗窗口 resize 成 133 逻辑宽**(球/菜单实测被拉宽,面板/菜单未 show 故不受影响),用 **`on_page_load` 回调强制 `set_size` 回逻辑尺寸**(含 300ms 延迟二次兜底);右键菜单宽度 60、面板 300、球 64 由此保证,菜单右缘与球右缘对齐
- 右键菜单交互:弹出即抢焦点,**鼠标离开/重新悬停球都不关闭**;只有**点击菜单外部任意处**(小球/桌面/其他应用,靠失焦 `Focused(false)` 收起)才关闭;点击球只关菜单不开主窗口(`MENU_CLOSED_AT` 300ms 内抑制单击开主窗)

### 悬浮窗路由接管状态实时同步(2026-09-12)
- **症状**:开/关路由接管后,悬浮球与悬浮面板的绿框(`.route-service-live` / `.row-takeover`)更新缓慢甚至不变。
- **根因(三层,逐一修复)**:
  1. **前端快照只推远端**:`App.tsx` 的同步 effect 仅在有 `remoteTargetId` 时推 `routeProxyApps`,
     本机模式传 `null` → 后端只能读 DB `proxy_config`,须等接管流程落库,延迟明显。
  2. **无乐观更新**:点击后要等 mutation 返回(后端写 live 配置,数百 ms)→ `invalidate` →
     `refetch` → `state` 变化 → 才推快照,共 **4 次 IPC 往返**。
  3. **轮询间隔被拉长 + 跨窗口事件不可靠**:球曾为 5s、面板曾为 30s;
     而实测 emit(含定向 `emit_to`)到悬浮窗 webview 不可靠——隐藏窗口会被 WebView2 节流,
     连 IPC 回调都会推迟到窗口重新可见才执行,导致「只能干等轮询」。
- **修复**:
  - **数据源统一**:`floating.rs` 新增 `ROUTE_PROXY_MAP`(`Mutex<Option<BTreeMap<String,bool>>>`),
    存前端推送的 per-app 路由快照;`pushed_route_for()` **优先读快照、未推送时回退本机 DB**;
    **悬浮球(`get_floating_ball_target`)与面板(`build_floating_entry`)共用同一函数**,保证两者一致。
  - **本机也推快照**:`App.tsx` 改为本机取 `takeoverStatus`、远端取 `routeProxyApps`/`routeProxyContainerApps`,
    统一推送,不再传 `null`。
  - **乐观更新**:`useProxyStatus` 的 `setTakeoverForApp` 加 `onMutate` 立即写 `takeoverStatus` 缓存
    (+ `onError` 回滚),**点击瞬间后端值即正确**。
  - **事件定向推送**:新增 `emit_data_refresh()`,对 `floating-panel`/`floating-ball`/`floating-strip`
    逐窗口 `w.emit()`(全局广播不可靠);接管开关、远端目标切换、置顶、面板展开全部改用它。
  - **轮询作为可靠保底**:球与面板统一 **1s 轮询**(`get_floating_ball_detail` /
    `get_floating_window_data` 均只读本地 SQLite 缓存,**不发网络请求**);
    面板隐藏时 Chromium 自动降频至约 1 次/分钟,后台开销可忽略。事件保留作加速路径。
- **设计结论**:Tauri 多窗口内存隔离,跨窗口只能走 IPC;轮询是**唯一 100% 可靠**的通知机制,
  事件只能作为「能到达时更快」的优化,不能作为正确性依赖。

### 模块③ 本地体验增强

### 图像拦截钩子(纯文本模型保护)
- 三道闸,防止纯文本模型(deepseek/glm 经火山引擎)因上下文含有图片块触发 `400 Model only support text input`
  - **PreToolUse `Read`**:拦截图片/PDF 文件读取
  - **PreToolUse `*screenshot*`**:拦截 MCP 截图工具,根本不让截图发生
  - **PostToolUse `mcp__*`**:兜底扫描所有 MCP 工具返回结果,含 base64 图片标记则 `decision: block` 隐藏
- 残余局限:用户直接拖拽/粘贴进 prompt 的图片不经过任何工具,拦不住;遇到 400 请 `/compact` 或 `/clear`

### UI 修复与打磨
- **Sonner toast 修复**:portaling 到 document.body + `pointerEvents:auto`,解决模态抽屉打开时 toast X 点不了的问题(根因:Radix 模态 Dialog 设置 `body { pointer-events: none }`)
- **Sonner 关闭按钮位置(2026-08-12 改回默认)**:最初修正到右上角(覆盖 sonner 默认左上角定位),
  后发现本机/远端 toast 的 X 位置表现不一致,已移除 index.css 覆盖、**保持 sonner 默认(左上角)**——
  所有 toast 统一默认定位,不再因覆盖选择性生效而漂移
- **目标选择器可滚动**:主机/容器下拉框 `max-h-[50vh]` + 可见细滚动条
- **目标选择器聚焦样式**:与全局 `*:focus-visible` 保持一致,不下拉特殊处理

### 更新提示回归上游 + 可关闭开关(2026-09-12)
- **背景**:早前 fork 把上游的更新提示横幅改成了 **toast 弹窗**(提交 `41fe5d1e`/`0ae14a01`/`8771b75f`),
  且引入功能退化——用 `ccswitch:update:toastEverShown` **一次性 key** 记忆,
  **关闭过一次后所有后续新版本都永远不再提示**(而原版是按 `dismissedVersion` 按版本记忆)。
- **改动**:
  - 删除自制的 `UpdateNotification.tsx`(toast 版)
  - 启用上游 `UpdateBadge`(merge v3.20.1 时已带进来但与上游逐字节一致、未被引用):
    header 里绿色 ⬆️ 圆形图标,仅 `hasUpdate` 时渲染,点击跳「设置 > 关于」;
    **不打断用户、不会「关了就永远看不到」、无需 localStorage 记忆逻辑**
  - 新增设置项 `AppSettings.show_update_badge`(`Option<bool>`,serde camelCase;
    `None`/`true` = 显示,跟随上游行为;`false` = 隐藏图标入口)。
    **仅控制入口显隐,后台自动检查更新不受影响**,「关于」页仍可手动检查。
  - 开关位置:设置 → 通用 → **「主页面显示」区**,紧邻同类的「显示项目切换」
    (`showProfileSwitcher`)——两者都是主页面顶部入口的显隐控制,归为一类。
  - 清理:`App.tsx` 中 UpdateNotification 遗留的横幅容器空 `<div>`;
    `BatchApplyPanel.tsx` 中 `DropdownMenu` 残留 import(app 选择器改 Popover 后的死代码,曾触发 TS6192)。
- **i18n**:key 置于 `settings.appVisibility.showUpdateBadge` / `showUpdateBadgeDescription`,四语齐全。

### 模块④ 基座升级与工程基建

### 基座升级:官方 v3.19.2 合并
- 网络:git https 无代理连不上、ssh 传输断流,最终用 **socks5 代理**(`socks5h://127.0.0.1:7897`,Clash)稳定拉取官方仓库
- 流程:`upgrade-v3.19.2` 试验分支 merge 官方 v3.19.2(14 个 commit)→ 解决冲突 → 并入 main 推送
- 官方带来的主要变化:#5967 搜索列表 + 批量开关 + busy 状态、usage 性能优化、安全加固等
- 冲突解决(9 文件):`mcp.rs`/`skill.rs`(后端)、MCP/Skills/Prompts 面板——融合官方重构 + fork 远端/OpenClaw 逻辑;悬浮窗/远端控制面/OpenClaw 核心零冲突完整保留
- 顺手修复 fork 遗留的 **3 个 OpenClaw MCP 测试**(merge 前就失败):目录 gate(每个 app 的 `should_sync_xxx_mcp` 都要求目录存在,测试内创建 `~/.openclaw`)、JSON5 解析(改走 `openclaw_config::get_mcp_servers`)、import 计数(测试改用 `Database::memory()` 内存库)
- 验证全过:`pnpm typecheck` / `format:check` / `build:renderer` / `cargo check` / `cargo test` / `cargo clippy`
- 已知遗留:
  - vitest 693/695 —— 2 个 `App.test.tsx` 集成测试全量并行时超时/隔离 flaky,单独跑全过(测试环境问题,非功能 bug)
  - clippy 16 个 warning 均在官方 v3.19.2 / fork 遗留代码(不在 merge 改动里),未处理

### 构建与基础设施
- 直接运行 `cargo build`（在 src-tauri 目录下）
- 推送到 GitHub fork `https://github.com/Amengclass/cc-switch`

### PR Checklist / 合并前自检清单

> 每次向 main 分支合并代码前，必须逐项确认通过：

| 检查项 | 命令 | 说明 |
|--------|------|------|
| [ ] TypeScript 类型检查 | `pnpm typecheck` | 无类型错误 |
| [ ] 代码格式检查 | `pnpm format:check` | Prettier 格式符合规范 |
| [ ] ESLint 检查 | `pnpm lint` | 代码规范通过 |
| [ ] 单元测试 | `pnpm test:unit` | 全部测试通过 |
| [ ] 前端构建 | `pnpm build:renderer` | 前端打包成功 |
| [ ] Rust Clippy 检查 | `cargo clippy` | 仅 Rust 代码有改动时执行 |
| [ ] i18n 检查 | 手动检查 | 新增/修改的用户可见文案已添加对应语言 key（zh/en/ja/zh-TW） |

---

## 远程工作清单(SSH 远程主机统一控制面)

> **本 fork 的核心新增能力。** 一条命令把当前 Provider 应用到远端主机/容器:SSH 连接 → 读远端配置文件 → 合并 env 块 → 原子写回 → 清理冲突环境变量 → 提示生效方式;并延伸出远程会话管理、Docker 容器目标、桌面悬浮球等。
> 详细实现见上文「远程主机连接与管理」「远程切换 Provider(env 合并)」「远程功能面板」「Docker 容器支持」等小节。

### ✅ 远程·已完成

| 类别 | 内容 |
|---|---|
| 远程主机基础 | SSH 连接 / 主机 CRUD / DPAPI 密码加密 / 读远端 settings.json |
| 远程切换(本机) | env 块合并 → 原子写回 → .bak 备份 → 生效提示;冲突 env 清理;安装检测 |
| 远程切换提速(08-07) | 宿主机**一次 exec+stdin 原子写**(15~20 RTT→1,强制 sh -c 兼容);`[ -f ] && cp .bak` 在 || 分支;hash 脏写防护**机制预留**(当前 claude/codex 均传 None,与本机行为一致);远程 ~10x 提速 |
| 前端对齐(08-07) | `useSwitchRemoteProviderMutation` onSuccess/onError/toast **同构本机**;`EffectReport.current_provider_id` 少一次 IPC;ProviderCard/ProviderActions isPending 禁用防连点;切换/同步成功 toast 的 notes 以 ① 序号列表展示(单条保持纯文本) |
| per-app 扩展·codex(08-07) | `switch_remote_provider`/`get_remote_current_provider` 加 `app` 参数;`remote/codex.rs` 复用本机 `prepare_codex_catalog_plan` + `build_codex_live_config`(catalog/unified/bearer/auth 判定,产出逐字节一致)+ 无校验原子写;`remote_current_providers.json` 迁移 `host -> {app:id}`;auth.json 按本机语义 |
| 目标选择器全 app(08-07) | `TargetBreadcrumb` 去掉 claude 限制,Sessions 非 claude 忽略远程目标 |
| 安装检测全 app(08-07) | `check_remote_cli_installed(app)` / `check_local_cli_installed(app)` + `cli_binary_for_app` 映射;徽标动态「{App} 已安装/未安装」+ `APP_INSTALL_CMDS` 按 app 给安装命令 |
| **全 app 远程切换(08-07)** | gemini(.env+settings.json 读-改-写)/grokbuild(TOML)/opencode(additive upsert)/openclaw(JSON5)/hermes(YAML) 五个分支,全部复用本机纯变换产出一致;connection 加 read_remote_text(exec cat,容器兼容) |
| 功能按钮远端适配(08-07) | 卡片测试按钮经 SSH 在远端 curl base_url(复用 resolve_base_url);删除 remove 清远端 live(additive 3 app)/delete 只删 DB+清远端记录(对齐本机) |
| 批量应用面板增强(08-14) | 批量应用 Provider 面板底部固定框加「当前 app 标识」徽章(ProviderIcon 品牌图标 + `t(apps.<app>)` 多语言名,复用 AppSwitcher 的 APP_ICON_NAME 映射;claude-desktop 兜底映射 claude);Provider 下拉边框加深;面板标题「批量应用 Provider」→「批量应用」 |
| 远端路由/接管(08-11) | 反向隧道按意图对账(不重建连接)+ 端口动态化;per-container DNAT(按容器 IP);容器×app 独立接管开关(`route_proxy_container_apps`);隧道失败降级直连 + warnings 提示 + 开关回退(reapply/switch 双路径);远端隐藏 claude-desktop;宿主机仅需 22 端口 |
| 隧道回连地址动态化(08-11) | `TUNNEL_HOST` 全局同步本机代理监听地址(0.0.0.0→127.0.0.1、::→::1);`server_channel_open_forwarded_tcpip` 回连改 `tunnel_host():tunnel_port()`——本机监听地址改任意 IP 远端仍可用 |
| 远程面板 | Sessions / MCP / Prompts / Skills(全 app 读写)+ Docker 容器目标 |
| 远端 Skills「导入已有」绿点对齐目标(2026-08-16) | 「导入」按钮绿点远端下改为**反映当前所选目标(宿主机/容器)可导入技能数**,而非本机。新增 `useRemoteUnmanagedSkillsQuery(host,container)`,query key 精确到目标互不干扰;UnifiedSkillsPanel 挂载(enabled:true)自动扫当前目标,重进 Skill 页即重扫(对齐本机 `useScanUnmanagedSkills`);点开导入 refetch 取最新;导入成功 invalidate 当前目标 key → 圆点立即消失。提交 c5fcb48a |
| 远端 Skill「更新」+「检查更新」(2026-08-16) | 对齐本机:远端下「检查更新」不再禁用(原 `远端→()=>undefined`),`check_remote_skill_updates` 读远端 skills.json→逐 skill 本机下载 repo→比对 hash→返回可更新列表;更新按钮放开(有 repoOwner 且 hasUpdate 才显示),`update_remote_skill` 从该 repo 重新下载最新版→替换远端 SSOT→写回 skills.json(hash/元数据)→同步各 app 链接。方案:**本机下载 + 本机算 hash**(远端无需访问 GitHub)。后端 impl 在 remote/skill.rs + commands.rs;前端 useCheckSkillUpdates 支持远端分流 + useUpdateRemoteSkill。提交 3f93934a |
| 远端导入合并对齐本机(2026-08-16) | UnifiedSkillsPanel 远端「导入已有」成功后更新 installed 缓存，从 `[...old, ...installed]` 简单拼接改为 `mergeImportedSkills(old, installed)`（按 id 去重、新记录覆盖旧），与本机 `useImportSkillsFromApps.onSuccess` 完全一致。提交 23b49ff5 |
| **Pi 远端全面支持(2026-08-19)** | Pi 的远程主机控制面全面补齐，涉及 SKILL / Prompts / Sessions / 供应商四个面板。详见下方「Pi 远端支持(2026-08-19)」小节。提交序列:8ed34c9a → 154eb26b → b19e255a → 35d5432d → bdd4a980 → 8c589461 → 102a6fc8 → e3e4f217 → 1cfb7eae |
| **远端供应商面板修复(2026-08-19)** | 远端供应商排序/重复导入/配置持久化三个问题修复。详见下方「远端供应商面板修复(2026-08-19)」小节。提交序列:cd2b1a25 → f477ec87 → 97de9a53 |
| **OpenClaw 远端配置支持(2026-08-19)** | OpenClaw 的环境变量/工具/代理默认设置三个面板支持远程主机操作。详见下方「OpenClaw 远端配置支持(2026-08-19)」小节。 |

### Pi 远端支持(2026-08-19)

> Pi CLI 的远程主机控制面全面补齐，涉及 SKILL / Prompts / Sessions 三个面板 + 后端基础修复。

**SKILL 管理面板：**

| 功能 | 本机 | 远端 | 说明 |
|---|---|---|---|
| 列出已装技能 | ✅ | ✅ | `list_remote_skills` 读远端 skills.json |
| 启用/停用(按 app) | ✅ | ✅ | `bulk_toggle_remote_skill_app` + `app_skills_rel` 补 pi |
| 从 ZIP 安装 | ✅ | ✅ | `install_remote_skills_from_zip` 已支持 pi |
| 导入已有 | ✅ | ✅ | 导入对话框远端显示 Pi 徽章；本机导入勾选 Pi 也生效(`sync_to_app_dir`) |
| 发现技能安装 | ✅ | ✅ | `install_remote_skill_from_discoverable` 已支持 pi |
| 更新 | ✅ | ✅ | `update_remote_skill_impl` + `sync_remote_skill_links` |
| 卸载/删除 | ✅ | ✅ | `delete_remote_skill` + `remove_remote_skill_links`(补 pi) |
| 检查更新 | ✅ | ✅ | `check_remote_skill_updates` 已支持 pi |

**后端修复（`remote/skill.rs`）：**
- `app_skills_rel` 补 `"pi" => Some(".pi/agent/skills")` — 修复"未知应用: pi"错误
- `APP_SKILLS_DIRS` 补 `.pi/agent/skills` — 远端删除技能时清理 pi 目录

**本机导入 Pi 支持（`services/skill.rs`）：**
- `import_from_apps` 勾选 Pi 时调 `sync_to_app_dir(&dir_name, &AppType::Pi)` 部署到 Pi 目录
- Pi 的启用状态由 `skill_exists_in_app` 推导(exists=active)，不走 DB 列

**Prompts 管理面板：**

| 功能 | 本机 | 远端 | 说明 |
|---|---|---|---|
| Global tab(提示词列表) | ✅ | ✅ | `listRemotePrompts` / `saveRemotePrompts` + 全操作 toast |
| System tab(SYSTEM.md/APPEND_SYSTEM.md) | ✅ | ✅ | `getRemotePiPromptFile` / `replaceRemotePiPromptFile` / `deleteRemotePiPromptFile` |
| Templates tab(~/.pi/agent/prompts/*.md) | ✅ | ✅ | `listRemotePiPromptTemplates` / `upsertRemotePiPromptTemplate` / `deleteRemotePiPromptTemplate` |
| Pi 原生资源 tab(system/templates) | 仅本机 | 远端可用 | 后端新增 6 个远端命令(`remote/commands.rs`) + 6 个辅助函数(`remote/prompt.rs`) |

**会话管理面板：**

| 功能 | 本机 | 远端 | 说明 |
|---|---|---|---|
| 会话列表 | ✅ | ✅ | `scan_sessions_fs`(Flat + ProjectDirectories 布局) |
| 查看消息 | ✅ | ✅ | `parse_messages_from_content`(JSONL 树结构解析) |
| 删除会话 | ✅ | ✅ | 校验 session_id 后删除文件 |
| 刷新按钮 | ✅ | ✅ | 同时刷新列表 + 当前会话消息 |

**后端修复（`remote/commands.rs`）：**
- 会话路径修正: `~/.pi/sessions` → `~/.pi/agent/sessions`（与本地 `get_pi_agent_dir()` 一致）
- `list_remote_sessions_all`:一次连接扫描所有 8 个 app(6 文件源 + 2 SQLite)，返回全量会话列表
- `pi.rs` 重构:抽出 `read_tree_from_lines` / `read_active_messages_from_lines`，新增 `parse_messages_from_content` / `parse_session_from_content` / `scan_sessions_fs`

**前端改动：**
- `PiPromptPanel` 新增 `remoteTargetId`/`remoteContainerId` props，三个 tab 全部支持远端
- `PiNativePromptResources` 各组件根据 `remoteTargetId` 选用本地/远端 API
- `SessionManagerPage` `effectiveRemoteTargetId` 包含 pi，providerFilter 全量拉取对齐本机
- 会话消息查询/删除改用 `selectedSession.providerId`（非页面级 appId）

---

### 远端供应商面板修复(2026-08-19)

> 修复远端供应商管理的三个问题，使远端行为与本机对称。

**问题 1：远端排序不生效**
- 根因: `updateSortOrder` 只写本地 DB，远端重新拉取时 SSOT 没有新排序
- 修复:新增 `update_remote_provider_sort_order` 远端命令，写入 SSOT `sort_index`；`useDragSort` 支持远端模式
- 优化:排序成功后用 `setQueryData` 乐观更新缓存，消除 UI 闪回

**问题 2：切换 app 供应商重复导入**
- 根因: `sync_remote_live_into_ssot` 在 `autoImportDefault=true` 时，每次从 live 重新读 → 和 SSOT 里用户已有的重复
- 修复:SSOT 已有供应商时直接跳过 live 导入（空库才首次导入 default）

**问题 3：远端供应商配置（用量查询等）不持久**
- 根因:用量查询配置等元数据保存在本地 DB，不写远端 SSOT
- 修复:新增 `update_remote_provider_meta` 远端命令；`useProviderActions.saveUsageScript` 远端模式调 `updateRemoteProviderMeta` 写远端 SSOT，不写本地 DB

### OpenClaw 远端配置支持(2026-08-19)

> OpenClaw 的环境变量/工具/代理默认设置三个面板支持远程主机操作，修复远端下操作实际修改本机的静默错误。

**新增后端命令(src-tauri/src/remote/commands.rs):**
- `get_remote_openclaw_env` / `set_remote_openclaw_env` - 读写远端 `~/.openclaw/openclaw.json` 的 env 段
- `get_remote_openclaw_tools` / `set_remote_openclaw_tools` - 读写远端 `~/.openclaw/openclaw.json` 的 tools 段
- `get_remote_openclaw_agents_defaults` / `set_remote_openclaw_agents_defaults` - 读写远端 `~/.openclaw/openclaw.json` 的 agents.defaults 段

**辅助函数(src-tauri/src/openclaw_config.rs):**
- `set_section_preserve_format` - 保形设置顶层 section（供远端命令使用）
- `set_nested_section_preserve_format` - 保形设置嵌套 section（如 agents.defaults）

**前端改动:**
- `src/lib/api/remote.ts` - 新增 6 个远端 API 函数
- `src/hooks/useOpenClaw.ts` - 所有 query/mutation hooks 支持 `remoteTargetId`/`remoteContainerId` 参数
- `EnvPanel.tsx` / `ToolsPanel.tsx` / `AgentsDefaultsPanel.tsx` - 组件接受远端 props
- `App.tsx` - 传递 `remoteTargetId`/`remoteContainerId` 到 openclaw 面板

### i18n 与 UI 修复(2026-08-19)

> 修复硬编码中文文案，优化 UI 细节。

**i18n 补全:**
- 远程主机状态徽章：`"已禁用"/"启用中"` → `t("remote.statusDisabled/statusEnabled")`（zh/en/ja/zh-TW 四语）
- 设置面板套餐用量显示：修正 i18n key 路径为 `settings.advanced.quotaDisplay.*`（title/description/expanded/expandedDescription）
- 四语 locale 文件全部更新（zh.json、en.json、zh-TW.json、ja.json）

**UI 优化:**
- 远程主机筛选下拉框：添加 `className="min-w-0 w-auto"` 去除右侧空白
- 批量应用搜索框：替换 raw `<input>` 为 `ManagementListSearch` 组件，对齐远程主机风格

### 远端 Provider 使用统计显示名称(2026-08-19)

> 使用统计中，远端 Provider 显示为 UUID（如 `9547eea1-f0c8-4d3a-a422-27be1885053c`），不够友好。

**问题原因：**
- 远端 Provider ID 是 UUID 格式，存储在 `~/.cc-switch/providers-{app}.json`
- 使用统计记录时，使用远端 Provider 的 UUID 作为 `provider_id`
- 查询统计时，在本地 `providers` 表找不到这个 UUID，所以显示原始 ID

**解决方案（长期）：**
- **扩展日志表**：在 `proxy_request_logs` 表增加 `remote_host_id` 和 `remote_container` 字段
- **记录时存储**：使用统计记录时，同时存储主机信息
- **查询时拼接**：SQL 查询时直接拼接显示名称
- **显示格式**：`{主机IP}({目标类型}) → {Provider名称}`
  - 示例：`192.168.1.100(宿主机) → Claude Opus`
  - 示例：`192.168.1.100(容器:redis) → GPT-4`
- **优势**：
  - 数据自包含，查询快
  - 可按主机/容器筛选
  - 不依赖实时远端连接
- **改动点**：
  - 数据库迁移：增加字段
  - 使用统计记录逻辑：记录远端请求时存储主机信息
  - 查询逻辑：SQL 中拼接显示名称

### ⏳ 远程·待完成(优先级从高到低)

| 优先级 | 事项 |
|---|---|
| 中 | ~~远程 Sessions per-app~~ ✅ 已完成实现(2026-08-08):claude/grokbuild/codex/gemini/openclaw + hermes/opencode(SQLite hybrid:远端 sqlite3 探测直查 / 下载 db 回本机读,commands.rs:1369-1538)。**实测验证状态(2026-08-14):claude ✅ codex ✅ 已实测确认;gemini/grokbuild/openclaw/hermes/opencode 实现已完成但待实测** |
| 中 | ~~远程 Prompts per-app~~ ✅ 已完成(2026-08-07,按本机 prompt_files 矩阵映射 live 文件,claude 外 per-app SSOT) |
| 低 | ~~远端 Skills 安装默认启用 app 硬编码 claude~~ ✅ 已完成(2026-08-07,加 app 参数默认启用当前所在 app,对齐本机) |
| 中 | ~~`get_remote_current_provider` 兜底 per-app~~ ✅ 已完成(2026-08-07,claude/codex/gemini/grokbuild;additive 无当前概念跳过) |
| 低 | `APP_INSTALL_CMDS` 包名校对 ⚠️ 2026-08-08 核实:grokbuild 前端 `@grok/grok-cli`(npm 404,应为 `@xai-official/grok`)、hermes 前端 `hermes-cli`(无关包,应为 PyPI `hermes-agent`)——均需改,后端 misc.rs 已用正确包名 |
| 低 | 远端隧道监听地址可配置化(2026-08-11 记录):现 `tcpip_forward("127.0.0.1",port)` 已抽为常量 `REMOTE_TUNNEL_LISTEN_ADDR`(connection.rs,含"跨机需 GatewayPorts"注释)。**决策:当前无跨机需求,勿提前实现**——若未来要做,改常量 + 远端 sshd 配 GatewayPorts clientspecified;注意与本机回连 `TUNNEL_HOST` 语义不同 |
| 既有 | 广播模式、密钥认证、`~/.ssh/config` 兼容、远端发现/恢复 Skills 等(见下方「待完成」) |

### 📋 功能按钮远端适配核查(2026-08-08 记录)

> 顶栏各功能按钮在「远端目标(remoteTargetId 非空)」下的适配现状。结论来自逐面板代码核查(src/ 前端组件/hooks/api + src-tauri/remote/ 后端命令清单)。

**适配情况总表:**

| 功能 | 状态 | 说明 |
|---|---|---|
| Prompts(📖) | ✅ 完全适配 | 读/增/删/改/开关全走 `remote_*` 命令(App.tsx 传 remote props)；**Pi 提示词三个 tab(global/system/templates)远端全支持(2026-08-19)** |
| MCP(Ⓜ️) | ✅ 完全适配 | 读/增删改/开关/导入全走远端(hooks 均带远端分支)；**Pi 已移除 MCP 入口** |
| Sessions(🕘) | ✅ 适配(1 处残留) | 读/删走远端;**resume 仍走本机 launchTerminal**(SessionManagerPage.tsx:447-469)；**Pi 会话远端全支持(2026-08-19)** |
| Skills(🔧) | ✅ 远端全适配(2026-08-19) | 列表/toggle/卸载/导入/ZIP/发现安装/更新全走远端；**Pi 远端全支持** |
| openclaw 工作区(📁) | ❌ 未适配 | 纯本机 workspaceApi,App.tsx 未传 remote props(1502) |
| openclaw 环境变量(🔑) | ✅ 已适配(2026-08-19) | `get_remote_openclaw_env` / `set_remote_openclaw_env`，hooks 支持远端模式 |
| openclaw 工具(🛡) | ✅ 已适配(2026-08-19) | `get_remote_openclaw_tools` / `set_remote_openclaw_tools`，hooks 支持远端模式 |
| openclaw Agents(⚙️) | ✅ 已适配(2026-08-19) | `get_remote_openclaw_agents_defaults` / `set_remote_openclaw_agents_defaults`，hooks 支持远端模式 |
| hermes 记忆(🧠) | ❌ 未适配 | 纯本机 hermesApi(App.tsx:1445 无 remote props) |
| hermes WebUI(📊) | ❌ 未适配 | 纯本机 open_hermes_web_ui(App.tsx:2088) |

**后端命令覆盖核对(src-tauri/src/remote/commands.rs):**
- 已有远端命令:providers 全 CRUD/切换、sessions、MCP、prompts、skills、env 冲突扫描/清理、settings 读取、docker 容器列表、openclaw 默认模型、主机管理
- **openclaw 配置命令(2026-08-19 已完成)**:openclaw env/tools/agents.defaults 的读写命令已实现
- **未完成**:workspace 文件读写、hermes memory 的远端命令

**缺口清单(按优先级):**
1. ~~**高**:openclaw 环境变量/工具/Agents~~ ✅ **已修复(2026-08-19)**:新增 6 个远端命令，前端 hooks 支持远端模式
2. **高**:hermes 记忆 —— 远端下操作的是**本机**(静默错误:用户以为改了远端,实际改本机)
3. **中**:openclaw 工作区(文件读写,工作量较大)
4. **低**:hermes WebUI(远端无浏览器概念,建议远端下禁用/隐藏按钮)
5. **低**:Skills 备份/恢复(远端下禁用,建议提示或补远端备份)+ Sessions resume(本机终端执行)

---

## 待完成 ⏳(按功能模块)

### 模块① 远程主机统一控制面(SSH) — 待完成

| 事项 | 说明 |
|---|---|
| ~~广播模式~~ | ✅ 已完成(2026-08-14):前端「批量应用 Provider」全屏面板(宿主机/容器多选 + 搜索 + 容器数 + 已选清单 + 逐台事件推送实时进度),入口A=主界面目标栏、入口B=远程主机管理页;后端 `broadcast_switch_provider` 命令(抽单机 `switch_remote_provider_target` 复用,逐落点聚合成功/失败,失败不阻断;A「逐台进度」走 `broadcast-progress` 事件推送)。**方案B**:广播时远端无该 Provider 则从「Provider 池来源」(当前=本机 DB,app.tsx 面板处 + commands.rs 均标了改点注释)写入远端 SSOT 再切换——本机一处配置、多处生效;**单机切换仍严格报错不受影响**(allow_local_fallback=false)。**未含「本机」落点**(当前仅远程宿主/容器)。后续可优化:B=连接复用/并行切换 |
| 🟡 **Provider 池来源可选(未来做)** | 需求(2026-08-14 记录):当前批量应用面板的「Provider 池来源」写死为本机 DB。想要**底部固定栏可切换来源**——来源 = **所有可选择目标(本机 / 各宿主机 / 各容器)**;选某来源后,Provider 下拉列出该来源的 Provider,后端广播也从**所选来源**读取 Provider 完整定义(不再写死本机)写入目标落点 SSOT 再切换。改动点:①前端底部固定栏加「Provider 来源」选择器 + 按来源拉 Provider 列表;②后端 `broadcast_switch_provider` 增加来源参数(本机 DB 或远端 SSOT 读定义);③现有「池来源改点注释」(app.tsx + commands.rs)`get_provider_by_id` 那处即此来源的落点 |
| 密钥认证 | 支持私钥文件 `~/.ssh/id_*` / ssh-agent;数据模型已预留 `auth_method` 字段(connection.rs Key 分支当前直接报错) |
| `~/.ssh/config` 兼容 | 解析别名、ProxyJump/跳板机配置 |
| ~~远端「发现」安装 Skills~~ | ✅ 已完成(2026-08-08 核实):`useInstallRemoteSkillFromDiscoverable`(useSkills.ts:186) + 后端 `install_remote_skill_from_discoverable`(commands.rs:2203) |
| 远端「从备份恢复」Skills | 本地备份 → 恢复到远端 SSOT。**未完成**:前端 isRemote 时 no-op(UnifiedSkillsPanel.tsx:770-772) + 后端无 remote restore 命令 |
| ~~远程切换应用到全部模型槽位~~ | ✅ 已完成(2026-09-04):批量应用面板已支持选择 app + 多选落点 + 广播切换,实现「应用到全部」行为 |
| 团队共享与审计 | 切换记录、操作日志、只读成员视图(host_switches 表未落地) |
| ~~远程 Sessions per-app~~ | ✅ 已完成(2026-08-08 核实):hermes/opencode SQLite hybrid 已实现(commands.rs:1369-1457,直查远端 sqlite3) |
| ~~get_remote_current_provider 兜底 per-app~~ | ✅ 已完成(2026-08-08 核实):claude/codex/gemini/grokbuild 四 app 均有 base_url 兜底(commands.rs:1188-1227) |

### 远端「live → 本机 DB」导入类能力对齐(2026-08-07 记录)

> 本机有、远端无的反向同步能力。opencode 行为差异已清零,剩余三项均为「从远端 live 读回导入本机 DB」。空状态「导入」按钮已置灰兜底(`ProviderEmptyState importDisabled`)。

| 事项 | 说明 |
|---|---|
| ~~远端空状态「导入」按钮(C3)~~ | ✅ **已过时不需要**（2026-08-08 确认）：per-target 独立后远端面板数据源 = 远端 SSOT（读时自动从该远端 live 幂等同步），SSOT 不会空；按钮置灰是有意设计（自动导入已覆盖），无需实现"远端→本机 DB"导入 |
| ~~远端启动自动导入~~ | ✅ **已完成**（2026-08-08 确认）：即 `sync_remote_live_into_ssot`（providers.rs:108）——每次读远端 SSOT 时自动从该远端 live 幂等同步（additive 全量 upsert / 非 additive 空库导入 default），对应本机启动导入语义 |
| ~~远端批量同步(C4)~~ | ❌ **已过时不需要**（2026-08-08 确认）：per-target 独立后「把本机 current/live 同步到远端」违背独立语义（本机不自动注入远端，核心决策），无此需求 |

### per-target 独立供应商列表 + 切目标自动同步(2026-08-07 已实现 ✅)

> 用户提出的架构方向：目标选择器指向哪，面板就显示哪的供应商列表。
> 目标 = 本机(默认) / 远端宿主机 / 远端某容器，三类各自独立列表。

**最终方案（2026-08-07 定稿并实现）**：
- **全部 7 app** 远端目标独立；**本机保持现有模型完全不变**（本机 SQLite 候选池 + live current，零改动）。
- **远端候选池 = 各目标机器自己的 SSOT**：`~/.cc-switch/providers/{app}.json`（宿主机放宿主机、容器放容器内），
  存完整 `Provider` 记录数组 + `current_provider_id`（`remote/providers.rs`，`FileOps` 支持 SFTP/docker exec）。
- **首次填充对齐本机启动导入**：SSOT 为空时从**该远端自己的 live** 导入——
  additive 3 app 每次读 SSOT 幂等同步 live → SSOT；非 additive 仅空库导入一条 `default`
  （对齐 `should_import_default_config_on_startup`）。本机 DB 的供应商**不会自动注入远端**（独立语义）。
- **后端命令**（`remote/commands.rs` + `remote/providers.rs`）：`get_remote_providers`（列表+current+live_ids 一次返回）、
  `add_remote_provider`（addToLive 对齐本机 add）、`update_remote_provider`（在生效位置才重写 live，additive 改名禁 live 内条目）、
  `delete_remote_provider`（非 additive current 禁删；additive 先移 live）；`switch_remote_provider` 数据源改 SSOT（本机 DB 不参与），
  `get_remote_current_provider` 判定顺序 = 切换记录 → SSOT current → live base_url 兜底（匹配 SSOT 而非本机 DB）；
  `remove_remote_provider_from_live` 移除后同步 SSOT `live_config_managed=false`。
- **前端**：远端目标下 `useRemoteProvidersQuery` 接管面板数据源（`providers`/`currentProviderId`/`liveIds`），
  添加/编辑/删除/复制/切换全走远端命令；本机目标完全走原路径。

| 决策点 | 定稿 |
|---|---|
| 改造范围 | **全部 7 app**（统一 SSOT，additive 与非 additive 一套逻辑；additive 的 live 仍为按钮态依据） |
| 本机定位 | **本机保持现有模型完全不变**（符合第一要素） |
| 候选池存储 | **远端 JSON SSOT**（非本机 DB 分桶；provider 定义即 JSON，与 prompts/skills SSOT 同构） |
| 首次填充 | **对齐本机：从该远端自己的 live 导入**（additive 每次幂等 / 非 additive 空库一次）；不做本机→远端自动注入 |
| 「添加」按钮语义 | 远端面板列表 = 该目标 SSOT；additive 的「添加/移除」由 live 集合（`live_ids`）驱动，对齐本机 |
| 失败/加载态 | 远端目标不可达 → 面板加载态/报错由 `useRemoteProvidersQuery` 自然呈现 |

### 远程前端展示优化 + 容器提速(2026-08-08 已完成 ✅,提交 2d06a468)

> per-target 独立后的体验优化：把「操作后要等第二次 SSH refetch 才刷新」的延迟压掉。

**前端更新策略（对齐本机语义）**：
- 本机 = `invalidateQueries`（refetch）——本地毫秒级无感；远端 = **操作命令在同一条 SSH 连接内
  把最新视图（`RemoteProvidersView`）一次带回，前端 `setQueryData` 写入缓存**——语义与本机 invalidate
  一致（操作成功后缓存 = 最新状态），但免掉第二次 SSH 建连（0.5~2s 大头）。**不是乐观更新**（无回滚风险，
  用的是服务端返回的权威数据）。
- **切换同时更新 current + liveIds**：additive 的「添加」按钮 = 切换（写 live 并启用），切换后
  `liveIds` 追加该 id → 按钮态「添加」→「移除」立即翻转（修复"添加成功但前端不刷新"）。
- **缓存 key 空串归一**：`remoteContainerId ?? "__host__"` → `|| "__host__"`（空串与 undefined 统一），
  修复移除/删除后 invalidate 落空、面板不刷新。

**容器/宿主机读取次数优化（方案 A）**：
- 操作命令入口**空库才 sync**（`load_remote_ssot_for_mutation`：SSOT 非空零额外读取）；
- `build_remote_providers_view` **复用内存 SSOT**（不再重读文件）；
- `sync_remote_live_into_ssot` 改为返回 `(变更数, live_ids)`——`get_remote_providers` 读面板时
  live 只读一次。
- 效果：容器一次添加从 ~7 次 `docker exec` 降到 ~4 次；列表加载从 ~4 次降到 ~2 次。

**UI 调整**：
- 设置页隐藏目标选择器状态栏（`currentView === "settings"` 不渲染 sticky 条）；
- claude 远端安装指令统一 npm（`npm i -g @anthropic-ai/claude-code`，状态栏仅展示/复制，不执行）；
- 清理排查用 `console.log`。

**已知待办**：
- 方案 B：SSH 连接复用（连接池，按 `host_id + container` 缓存 `RemoteSession`，连续操作复用）——
  收益最大（省每次 0.5~2s 握手认证）但风险高（连接生命周期/密码轮换/并发），待用户确认后做；
- 远端 SSOT 读-改-写无并发锁（与 prompts/skills SSOT 同款，单用户 GUI 可接受）；
- `delete_remote_provider` additive 分支多一次 SSH 建连（可复用外层连接）。

### 远程 per-app 扩展(2026-08-07 起,codex/gemini/grok/opencode/openclaw/hermes 已全部完成)

| 事项 | 说明 |
|---|---|
| ~~远程切换扩展到 gemini / grok~~ | ✅ 已完成(2026-08-07) |
| ~~远程切换扩展到 opencode / openclaw / hermes~~ | ✅ 已完成(2026-08-07) |
| 远程 Sessions per-app | ✅ 已完成实现(2026-08-08):claude(`~/.claude/projects`)、grokbuild(`~/.grok/sessions`)、codex(`~/.codex`)、gemini(`~/.gemini/tmp`)、openclaw(`~/.openclaw/agents`)按各自会话目录 read_dir;hermes(`~/.hermes/state.db`)/opencode(`~/.local/share/opencode/opencode.db`)走 SQLite hybrid(commands.rs:1857-1863)。**实测:claude/codex ✅;gemini/grokbuild/openclaw/hermes/opencode 待实测(自查命令见本文档 346 行旁注)** |
| `get_remote_current_provider` 兜底 per-app | 当前仅 claude 有 base_url 兜底;其他 app 只有持久化记录,老数据(本应用切换前已生效的远端)恢复现场缺失 |
| 安装命令 `APP_INSTALL_CMDS` 包名校对 | grok/openclaw/hermes 等包名按常见 npm 名填写,需按各 CLI 官方文档核对 |

### 模块② 悬浮窗组件(加速球) — 待完成

| 事项 | 说明 |
|---|---|
| ~~悬浮窗样式与右键菜单~~ | ✅ 已完成(2026-08-08 核实):右键菜单已完成(FloatingContextMenu.tsx);样式打磨已完成(尺寸/间距/动效均调整到位,2026-09-04 用户确认) |

### 模块③ 本地体验增强 — 待完成

### 前端 UI 刷新策略一致性

| 事项 | 说明 |
|---|---|
| ⚠️ **i18n 国际化未完成项（重点）** | 早期做了一部分（如技能安装/卸载 loading 已 i18n 化），**但仍有内容未完成、未走 t()**：① 悬浮窗全部文案硬编码中文（FloatingContextMenu「设置/隐藏」、FloatingPanel「刚刚/x分钟前/从未更新/暂无数据/已用/剩余/当前供应商/打开主界面/关闭悬浮窗」、FloatingBall「未设置」、FloatingWindowSettings「固定当前位置」）；② 主界面其他组件硬编码中文（toast/按钮/placeholder/tooltip 等，**具体残留位置待统一扫描确认**，2026-08-08 记录——用户反馈"早期做了一些没完成，具体内容忘了但确定有"）。处理方式：统一扫描 src/（含 src/floating/）全部硬编码中文 → 逐个走 t() + 四语 key |
| ~~`handleImport` 远端导入改用 `mergeImportedSkills`~~ | ✅ 已完成(2026-09-04 核实):hook onSuccess 已用 `mergeImportedSkills` 按 id 去重合并(UnifiedSkillsPanel.tsx:534),远端分支一致 |
| ~~`useImportSkillsFromApps` 远端路径返回完整数据~~ | ✅ 已完成(2026-09-04 核实):ZIP 路径 + 目录导入均已返回完整数据含 description(useSkills.ts:602),即时更新 UI 已统一策略 |
| ~~远端「从市场安装」Skills~~ | ✅ 已完成(2026-08-08 核实):`useInstallRemoteSkillFromDiscoverable` + `install_remote_skill_from_discoverable`,SkillsPage.tsx:171 已接入 |
| 远端「更新」Skill | ❌ 未完成(2026-08-08 核实):无 updateRemoteSkill/update_remote_skill;前端远端隐藏更新入口(UnifiedSkillsPanel.tsx:1027) |
| ~~技能安装/卸载 loading 提示多语言适配~~ | ✅ 已完成(2026-08-08 核实):已 i18n(UnifiedSkillsPanel.tsx:375/549 + 四语 key) |
| 悬浮窗文案多语言 | ❌ 未完成(2026-08-08 核实):硬编码中文全部保留(FloatingContextMenu.tsx:33/38、FloatingPanel.tsx、FloatingBall.tsx:121、FloatingWindowSettings.tsx:48-49)——统一 i18n,与其他改进一并处理 |

### 模块④ 基座升级与工程 — 待完成

### 基座裁剪

| 事项 | 说明 |
|---|---|
| ~~移除 Codex / Gemini / Claude Desktop 模块~~ | **不做**。保留完整多应用支持,不裁剪任何模块 |

### 构建与发布

| 事项 | 说明 |
|---|---|
| ~~基座升级 v3.20.1~~ | ✅ 已完成(2026-09-04):merge 上游 v3.20.1 进 fork(40+ commit),解决 3 个冲突(database/mod.rs schema v18、codingPlanProviders.ts pattern、codex_config.rs 重构);适配 API 变更(extract_codex_model→内联、prepare_codex_catalog_plan→prepare_codex_provider_live_config、query_opencode_go 旧版删除);OpenCode Go 从 workspace_id+auth_cookie 切换为 API key 方式;UsageScriptModal.tsx 清理旧凭证字段;编译通过+应用启动正常 |
| 打包测试 | 独立 exe 测试(Win/macOS),README 更新 |

**构建注意事项：**

- **前端修改后必须先 `pnpm run build:renderer`**：Tauri 配置 `frontendDist: "../dist"`，`cargo build` 不会自动编译前端。只跑 `cargo build` 会让 exe 加载 `dist/` 里的旧前端，导致 TypeScript 修改不生效。
- 完整构建流程：
  1. `pnpm run build:renderer`（Vite 打包前端到 `dist/`）
  2. `cargo build --features tauri/custom-protocol`（编译 Rust）
  3. 产物在 `src-tauri/target/debug/cc-switch.exe`
- 在 src-tauri 目录下运行 `cargo build`（仅 Rust 编译；**不含前端构建**）
- 构建前需 `Stop-Process -Name cc-switch -Force` 避免 exe 被占用导致 `os error 5`
- `CARGO_BUILD_JOBS = "2"` 控制并行度（避免 LNK1105 防火墙锁定）

**开发验证流程（改动→检查的最小化路径）：**

原则：**先把整批改动做完，再统一检查一次**，不「编辑→全量检查→编辑→全量检查」地频繁打断（Windows 上 `cargo test` 2~3 分钟、`cargo build` 链接 1 分钟，太慢）。

| 阶段 | 命令 | 作用 | 成本 |
|---|---|---|---|
| 快路径（每次改动后） | `pnpm typecheck` + `pnpm format:check` + 受影响的 `pnpm vitest run <file>` | 前端类型/格式/相关逻辑 | 秒~十几秒 |
| Rust 编译验证（开发期） | `cargo check`（只编译不链接） | 抓编译错误，比 `cargo test` 快 | ~1min |
| 慢路径（整批交付前，跑一次） | 完整 `cargo test` + `cargo clippy` + `pnpm build:renderer` + `cargo build` + 启动 | 功能回归 + lint + 产出可运行 exe | 几分钟 |

- 只改前端/注释/文档时，可完全跳过 cargo 系列。
- 例外：Rust 改动是跨模块签名变更时，提前跑一次 `cargo check` 定位级联编译错误值得。
- 完整 `cargo test` / `cargo build` 前先 `Stop-Process -Name cc-switch -Force`。

---

## 里程碑记录(M0 起)

| 状态 | 里程碑 | 内容 | 验收 |
|---|---|---|---|
| ✅ | M0 | fork + 构建环境搭建 + 基线可编译可运行 | `pnpm install` + `cargo build` 通过,应用能启动 |
| ✅ | M1 | 新增 `remote/` 模块骨架 + 依赖就绪 | russh/keyring 依赖编译通过,空命令可注册 |
| ✅ | M2 | 远程主机基础(CRUD/连接/读远端 settings.json) | hosts 表 + 前端页面 + 密码走钥匙串 |
| ✅ | M3 | 远程切换 + 原子写回 + 生效提示 | env 块替换 + .bak + EffectReport 提示 |
| ✅ | M4 | 冲突环境变量清理 | 扫描 ANTHROPIC_* → 注释 + .bak |
| ✅ | M5 | 远程会话管理 | SFTP 列 ~/.claude/projects/*.jsonl |
| ✅ | 目标选择器 | 本机/服务器选择器 | 头部选择器;选服务器后供应商当前高亮+切换走远端;编辑当前供应商保存后原子写回远端 |
| ✅ | 编辑同步修复 | 编辑供应商同步远端 | 判定改为 `provider.id === currentProviderId`(DB is_current,SSOT),不靠 base_url 猜测 |
| ✅ | exec 通道 | 远端执行命令 | `connection::exec_command`;用于检测安装状态、Docker exec 等 |
| ✅ | 安装检测 | Claude Code 安装检测 | `command -v claude` + 哨兵判断;面板徽标 |
| ✅ | 远程会话 | 远程会话前端闭环 | 目标选服务器 → 浏览/查看/删除远端 jsonl(FileOps) |
| ✅ | 远程 MCP | 远程 MCP 编辑 | 读写远端 `~/.claude.json` mcpServers(原子写回) |
| ✅ | 远程 Prompts | 远程 Prompts 基础管理 | 编辑远端 `~/.claude/CLAUDE.md`（单文件） |
| ✅ | 远程 Prompts SSOT | 远端 Prompts 对称管理 | 远端 `~/.cc-switch/prompts.json` + 启用项原子写入 `~/.claude/CLAUDE.md`,列表/添加/编辑/删除/启用切换全功能,乐观更新 + toast |
| ✅ | 远程 Skills | 远程 Skills 基础管理 | 列出/删除远端 `~/.claude/skills/` 目录 |
| ✅ | 远程 Skills SSOT | 远端 Skills SSOT 重构 | 远端 SSOT + skills.json + symlink/copy,导入已有/ZIP/删除/切换全功能,跟随设置 |
| ✅ | 容器 stdin 管道 | 容器文件写入 stdin 管道 | docker exec -i + channel.data(),无文件大小限制 |
| ✅ | 容器批量扫描 | 导入已有容器批量扫描 | 一个 shell 脚本一次 exec 收集所有源目录技能 |
| ✅ | toggle 合并 | 容器 toggle 合并 exec | 读 JSON → 修改 → 写 JSON + 操作链接合并为一次 exec_with_stdin |
| ✅ | Docker 容器 | Docker 容器目标(方案 B) | FileOps + RemoteTarget + 全部面板泛型 + ZIP/导入支持容器 |
| ✅ | 推送 | 推送到 GitHub fork | `https://github.com/Amengclass/cc-switch` |
| ✅ | 基座升级 v3.19.2 | merge 官方 v3.19.2 进 fork(14 commit) | 编译/测试/clippy 全过;悬浮窗/远端/OpenClaw 保留;修好 3 个遗留 OpenClaw 测试 |
| ✅ | 远端批量开关优化(C+A) | 远端 bulk 命令(单次 SSH round-trip)+ 前端乐观更新 | MCP/Skills 批量一次连接改完;点击瞬间反馈;新增 11 个后端单测 + 前端部分失败用例;全检查通过 |
| ✅ | 悬浮窗(加速球) | 桌面小球 + 悬停面板 | 透明窗口/拖动/吸附/单击开主窗/余量复用主窗口缓存/托盘开关/主题跟随 |
| ✅ | 远程切换提速 + 前端对齐(2026-08-07) | 宿主机一次 exec 原子写(15~20 RTT→1)+ hash 机制预留 + mutation 同构 | 切换 ~10x 提速;isPending 防连点;EffectReport 一次返回当前供应商 |
| ✅ | 远程 per-app 扩展·codex(2026-08-07) | switch_remote_provider 加 app 参数;codex 复用本机 catalog/unified/bearer/auth 全链路(产出逐字节一致);当前供应商记录 per-app | codex 远端切换生效,保留远端 auth.json 登录态;老记录兼容 |
| ✅ | 目标选择器/安装检测全 app(2026-08-07) | TargetBreadcrumb 放开所有 app;check_local/remote_cli_installed(app) 参数化 | 所有 app 页可选服务器/容器;徽标动态显示「{App} 已安装/未安装」+ 按 app 安装命令 |
| ✅ | 悬浮窗样式与右键菜单 | 右键菜单已完成(FloatingContextMenu.tsx,2026-08-08 核实);样式打磨与「快捷切换/打开主界面」菜单增强待做 |
| 🔄 | M6 | 打包测试(密钥认证延后) | 独立 exe + 前端运行 |

---

## 技术选型

| 层 | 选型 | 理由 |
|---|---|---|
| 桌面框架 | **Tauri 2.x + Rust**(跟随 fork) | 已定,沿用原栈 |
| 前端 | **React**(跟随 fork) | 已定,沿用原栈 |
| 存储 | **SQLite**(沿用 `~/.cc-switch/cc-switch.db`) | 兼容既有生态(如 Raycast 扩展读同一 db);新增 `hosts`、`host_switches` 表 |
| SSH 库 | **`russh` + `russh-sftp`**(纯 Rust async) | 原 `ssh2` 在 Windows 需 vendored-openssl(编译要 Perl+NASM,风险高);russh 纯 Rust 无 C 工具链依赖,契合既有 tokio 异步栈 |
| 远程文件 | **`russh-sftp`** | settings.json / shell profile / sessions JSONL 的读写 |
| 原子写回 | 远端**临时文件 + rename** | 与本机 `settings.json` 写入策略一致,中断不留半文件 |
| 凭据加密 | **Windows DPAPI**(`CryptProtectData`) | 原 `keyring` 在非域 Windows 上 `CRED_PERSIST_ENTERPRISE` 只存登录会话、换进程即丢;DPAPI 绑定用户账户、跨进程/重启稳定。密文 base64 存 `~/.cc-switch/remote_passwords.json` |
| env 扫描 | 手写正则 + SFTP 读取 | 无需额外依赖,覆盖 bash/zsh 两类 profile |

**关键取舍说明:**
- **`russh` 而非 `ssh2`**:`ssh2` 在 Windows 上静态编译 OpenSSL 需要 Perl + NASM,构建链风险高;`russh` 纯 Rust,无 C 工具链,与 tokio 栈契合,单机与后续广播并发都适用。
- **密码优先,密钥二期**:贴合「主机名+密码」的最朴素心智;数据模型上已留 `auth_method` 字段,密钥不返工。
- **不 shell 调 `ssh`**:为支持「密码认证 + 程序内读写 + 精确的错误回传」,走 `russh` 而不是调用系统 ssh/sshpass。

---

## 目录结构

> 标注说明:`【保留】` = 上游原有,未改动;`【新】` = 本项目新增;`(删除 ...)` = 计划裁剪。

```
cc-switch/                     # fork 自 farion1231/cc-switch
├── src/                       # React 前端(裁剪后仅 Claude Code)
│   ├── components/
│   ├── pages/
│   │   ├── Providers/         # 【保留】供应商切换(本机)
│   │   ├── Mcp/               # 【保留】MCP 管理(仅 Claude)
│   │   ├── Prompts/           # 【保留】CLAUDE.md 管理
│   │   ├── Skills/            # 【保留】Skills 管理
│   │   ├── Sessions/          # 【保留】本机会话管理
│   │   └── RemoteHosts/       # 【新】远程主机:列表/详情/连接态
│
└── src-tauri/                 # Rust 后端
    ├── src/
    │   ├── commands/
    │   │   ├── provider/      # 【保留】本地切换逻辑
    │   │   ├── mcp/ prompts/ skills/ sessions/   # 【保留】(裁剪非 Claude)
    │   │   └── remote/        # 【新】远程命令薄封装(调 remote/ 模块)
    │   ├── remote/            # 【新】SSH 远程核心
    │   │   ├── mod.rs
    │   │   ├── connection.rs  # 连接池/认证(密码、密钥预留)、超时、重试
    │   │   ├── sftp_io.rs     # SFTP 读写封装(下载/上传/临时文件+rename/备份)
    │   │   ├── settings.rs    # 远端 settings.json 解析/合并/原子写回
    │   │   ├── env_clean.rs   # 【新】shell profile 冲突 env 扫描/清理
    │   │   ├── sessions.rs    # 【新】远端会话 JSONL 浏览/删除/resume
    │   │   └── effect.rs      # 【新】切换生效方式判定与提示文案
    │   ├── fsops.rs           # 【新】FileOps trait(存在/读头尾行/读目录/删除/软链接) + LocalFileOps / RemoteSftpFileOps / DockerExecFileOps 三实现
    │   ├── db/                # SQLite:migrations 新增 hosts / host_switches
    │   └── ...
    └── tauri.conf.json
```

---

## 构建方法(Windows 开发机)

> **必须用 `cargo build`（在 src-tauri 目录下）,不要直接 `cargo build --release`。**
> 踩坑实录:`cargo build --release`(未设 `/DEBUG:NONE`)全量编译,多次撞火绒 `sysdiag` 文件锁(LNK1105 / 错误码 1224),重试 5 次仍失败;改用脚本第 1 次即成功。

**两种运行方式(务必分清):**

### 开发模式:`pnpm tauri dev`(前端 + 后端同时跑)

> 这是 Tauri 的标准开发模式,会同时启动**两个进程**:

| 进程 | 角色 | 说明 |
|---|---|---|
| `node.exe`(Vite 开发服务器,端口 3000) | 前端 | 改 `src/` 代码即时热更新(HMR),无需重编译 |
| `cc-switch.exe`(`src-tauri\target\debug\`) | 后端 Rust 应用(Tauri 壳) | 从 `http://localhost:3000` 加载前端页面,处理 IPC / 系统能力 |

- 改 `src/` 前端代码 → vite 热更新,浏览器/窗口立即生效;
- 改 `src-tauri/src/` Rust 代码 → cargo 增量编译后自动重启应用;
- 关掉应用窗口即整体退出(vite 随之结束)。
- 适合:边改边看、日常开发。首次全量编译约 13 分钟(738 crates),之后增量很快。
- ⚠️ dev 实例同样受 `tauri-plugin-single-instance` 约束:已安装版 `D:\software\cc switch\cc-switch.exe` 若在运行,dev 实例启动即自动退出(单实例锁被占),需先 `Stop-Process -Name cc-switch -Force`。

### 生产构建(独立 exe,单进程,默认)

> 前端先打包,再编译 Rust 并嵌入前端 → 产物自带前端,**不需要** vite 服务器,单进程独立运行。
> 代价:每次改前端都要重新打包,无热重载。

**流程**
1. 前端有改动 → 先 `pnpm build:renderer`(dist 由 `--features tauri/custom-protocol` 嵌入 exe)
2. **必须先杀进程**：`Stop-Process -Name cc-switch -Force`，否则 exe 被锁报 os error 5，cargo 重试 12 次全部 LNK1105
3. 删旧产物：`Remove-Item ...\cc-switch.exe -Force` + `Remove-Item ...\build.log -Force`
4. 跑 `cargo build`（在 src-tauri 目录下）(脚本内部 `Set-Location` 到 `src-tauri`)
5. 启动：`Start-Process -FilePath ...\cc-switch.exe`

> **一键构建命令**（含停进程+清理+编译+启动）：
> ```powershell
> Stop-Process -Name cc-switch -Force -ErrorAction SilentlyContinue; Start-Sleep -Seconds 2; Remove-Item "C:\Users\Ameng\Desktop\claude_woker\cc-switch\src-tauri\target\debug\cc-switch.exe" -Force -ErrorAction SilentlyContinue; Set-Location C:\Users\Ameng\Desktop\claude_woker\cc-switch; pnpm build:renderer; Set-Location src-tauri; cargo build
> ```

**脚本关键点(规避火绒 sysdiag LNK1105)**

| 项 | 值 |
|---|---|
| 构建目标 | debug(不带 `--release`) |
| 链接调试符号 | `RUSTFLAGS=-C link-arg=/DEBUG:NONE` |
| 符号生成 | `CARGO_PROFILE_DEV_DEBUG=0` |
| 并行度 | `CARGO_BUILD_JOBS=2` |
| 重试 | 最多 12 次;`build.log` 出现 `error[` / `error:`(排除 LNK,即真实编译错误)才停,仅 LNK1105 则继续 |
| 失败反馈 | 真实编译错误时**立即打印错误上下文**(错误行附近 ±6 行),不依赖翻 build.log |
| 耗时 | 每次构建打印 `elapsed Ns`,一眼看出是增量还是全量 |
| 产物 | `src-tauri\target\debug\cc-switch.exe`(约 67 MB) |

**为什么有时快有时慢**

| 场景 | 编译量 | 时长 |
|---|---|---|
| 增量(只改 1~2 个 Rust 文件) | 只重编译改的 crate + 链接 | 1~2 分钟 |
| 改了依赖/公共 crate | 连带重编译一批 crate | 5~10 分钟 |
| 全量(首次 / target 被清 / 换了编译模式) | 几百个 crate | 10 分钟+ |
| 撞火绒 LNK1105 | 链接失败 + 重试 + sleep 2s | 每次失败额外 +2s |

> ⚠️ **不要在构建流程里跑 `cargo check`**：`check` 与 `build` 产物格式不同，来回切换会互相触发大面积重编译。要验语法就在**单独一次**里做完，别和 `cargo build` 交叉使用。

---

## 附录

### 冲突环境变量名单(ANTHROPIC_*)

```
ANTHROPIC_API_KEY
ANTHROPIC_AUTH_TOKEN
ANTHROPIC_BASE_URL
ANTHROPIC_MODEL
ANTHROPIC_SMALL_FAST_MODEL
ANTHROPIC_DEFAULT_OPUS_MODEL
ANTHROPIC_DEFAULT_SONNET_MODEL
ANTHROPIC_DEFAULT_HAIKU_MODEL
ANTHROPIC_CUSTOM_HEADERS
ANTHROPIC_PROXY_URL
```

扫描命中规则:`export ANTHROPIC_<上述名单>=...` 或 `set -x ANTHROPIC_...`;命中即提示冲突并纳入清理候选。

### 切换生效方式提示

| 场景 | 提示内容 |
|---|---|
| 本机 · claude 未运行 | 「已写入 `~/.claude/settings.json`,下次启动 `claude` 生效」 |
| 本机 · claude 运行中 | 「热重载已生效;若未开启热重载,重启当前会话生效」 |
| 远程 · claude 未运行 | 「已写入远端 `~/.claude/settings.json`,SSH 登录后运行 `claude` 即生效」 |
| 远程 · claude 运行中 | 「新会话立即生效;已运行会话按热重载生效」 |
| 任一 · 清理过冲突 env | 「已清理冲突变量,请 `source ~/.bashrc` 或重新登录终端;原值已备份至 `settings.bak`」 |

### 风险与对策

| 风险 | 对策 |
|---|---|
| fork 后上游同步成本 | 按需 merge,阶段性质检;关键修复手动 cherry-pick |
| 远端 SFTP 原子 rename 在部分 SFTP-only 服务器覆盖行为不一致 | 先在目标机型验证;失败回滚到 .bak |
| env 清理误删用户配置 | 强制 .bak + diff 预览 + 二次确认,删除而非覆盖 |
| 远端写权限不足(`~/.claude` 不可写) | 切换前预检可写,不可写则报明确错误 |
| 密码明文风险 | DPAPI 加密;提供「每次询问」选项 |
| 热重载对远端是否真生效不确定 | 提示文案不承诺「实时生效」,只描述确定行为,并给出验证命令 |
