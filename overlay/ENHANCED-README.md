# CC Switch Magic（增强版）

> 基于 [farion1231/cc-switch](https://github.com/farion1231/cc-switch) 的 **Fork 增强版**：保持上游全部功能**零裁剪**（纯 `git merge` 即可同步上游），在之上新增 **SSH 远程主机统一控制面**。
>
> **定位**：「**一处配置、多机生效**」——在本机 GUI 上定义 Provider，直接应用到本机 + 任意远程主机 / Docker 容器；不是单纯的文件同步工具。

---

## 与上游的差异

| | 上游（原生 cc-switch） | 本增强版 |
|---|---|---|
| 配置作用范围 | 仅本机 | 本机 + 任意远程主机 + Docker 容器 |
| 远程管理 | 无 | SSH 直连远端，直接读写远端 `~/.claude/settings.json` 等 |
| 一处配置多机 | 无 | 一条命令把 Provider 广播到多台主机/容器 |

> 上游功能一项都不裁剪；远程能力作为独立新增层叠加。若想还原原始体验，可在设置关掉「启用远端功能」总开关（见下）。

---

## 核心特性速览

### 1. SSH 远程主机管理
- 连接 · 主机 CRUD · DPAPI 密码加密 · 读远端配置文件
- 切换 Provider：env 合并 → 原子写回（临时文件 + rename）→ 清理冲突环境变量 → 明确生效提示
- 检测远端 CLI 安装状态（各 app 独立）

### 2. 目标选择器 + Docker 容器目标
- 头部「本机 / 服务器 / 容器」连体胶囊选择器，实时探活（在线绿点 / 离线灰点）
- Provider / MCP / Prompts / Skills / 会话 均可作用于服务器或容器内

### 3. 远程 Provider 全能力
- 远程候选池 = **各目标机器自己的 SSOT**（`~/.cc-switch/providers/{app}.json`），本机不自动注入远端（独立语义）
- 添加 / 编辑 / 删除 / 复制 / 切换 全走远端；支持 7 个 app（claude / codex / gemini / grokbuild / opencode / openclaw / hermes）

### 4. 批量应用 Provider
- 全屏面板：多选宿主机/容器 + 搜索 + 容器数 + 已选清单 + 逐落点实时进度推送
- 后端广播命令（抽单机逻辑复用），一键把**一个 Provider**推送到多个落点
- **池的来源** = 本机「当前 app」的供应商（`useProvidersQuery(activeApp)`），底部固定框有「当前 app 标识」徽章

### 5. 路由托管 / 接管（走本机代理）
- 反向隧道按意图对账 + 端口动态化；per-container DNAT
- 各 app 独立开启远端接管（`route_proxy_container_apps`），隧道失败自动降级直连并提示
- **claude-desktop 不参与远端**：远端无此 app，其配置归并到 claude，AppSwitcher 过滤 + 该标签下隐藏全部远端入口

### 6. 远端功能总开关
- 设置 → 通用 → 远端设置：「启用远端功能」**默认开启**
- 关闭即**还原原生 cc-switch**：隐藏远程主机导航、目标选择器、批量应用、远端状态栏等全部远端入口（而不删除数据）
- 另可关闭某台主机的「禁用」软开关（目标选择器不再显示、不可操作，但保留数据可随时启回）

### 7. 远程 Session / MCP / Prompts / Skills
- 会话：远端 `~/.claude/projects/*.jsonl` 浏览/查看/删除（SQLite hybrid 支持 hermes / opencode）
- MCP：读写远端 `~/.claude.json` mcpServers（原子写回）
- Prompts：远端 `~/.claude/CLAUDE.md` + 远端 `~/.cc-switch/prompts.json` SSOT
- Skills：远端 SSOT + skills.json + symlink/copy，与本地完全对称

### 8. 桌面悬浮窗增强
- 悬浮球：吸附/拖动、透明度可调、路由纳管理状态显示（接管 app 显示流心动画边框）

---

## 文档导航

| 文档 | 内容 |
|---|---|
| [docs/enhanced-plan.md](docs/enhanced-plan.md) | 完整设计与核查档案（功能模块、已完成/待完成、实现细节） |
| **README.md / README_ZH.md** | 上游官方文档（本增强版不覆盖） |
| session-manager.md | 会话管理说明 |
| docs/guides/ | 路由、认证、模型可见性等专项指南 |
