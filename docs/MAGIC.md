# 我们比官方多了什么（MAGIC）

> 基线：官方 `v3.20.1`。本文是**人读清单**，机器校验见 `checks/feature-checks.ps1`。
> 规模：**新增 55 个文件 / 修改 128 个 / 删除 1 个**；官方 1235 个文件中 **1106 个与我们逐字节相同**。

---

## 一、SSH 远程主机统一控制面（核心）

> 定位「**一处配置、多机生效**」：在本机 GUI 定义 Provider，直接应用到本机 + 任意远程主机 / Docker 容器。

### 远程主机管理
- SSH 连接 · 主机 CRUD（名称/host/port/用户名/密码）
- 密码用 **Windows DPAPI** 加密存 `~/.cc-switch/remote_passwords.json`
- 测试连接（russh，含超时控制）
- 读取远端 `~/.claude/settings.json` 并展示当前 Provider / 模型 / env
- 检测远端 CLI 安装状态（各 app 独立）

### 远程切换 Provider
- env 块合并 → **原子写回**（远端临时文件 + rename）→ 失败回滚（`.bak`）
- 切换后明确提示生效方式（本机/远端、claude 运行中/未运行）
- 切换提速：宿主机从多趟 SFTP（15~20 RTT）改为**一次 exec + stdin 管道**（~1 RTT）
- 支持全部 7 个 app：claude / codex / gemini / grokbuild / opencode / openclaw / hermes
- 产出与本机逐字节一致（复用本机 catalog / live config / auth 判定链路）

### 目标选择器 + Docker 容器
- 头部连体胶囊选择器：**本机 / 服务器 / 容器**，实时探活（在线绿点 / 离线灰点）
- 容器目标：`docker ps` 列表 + 容器下拉
- 各面板均已按目标分流

### 远程功能面板
| 面板 | 能力 |
|---|---|
| Sessions | 浏览 / 查看消息 / 删除远端 `~/.claude/projects/*.jsonl`（含 SQLite hybrid：hermes / opencode） |
| MCP | 读写远端 `~/.claude.json` 的 `mcpServers`（原子写回） |
| Prompts | 远端 `~/.cc-switch/prompts.json` SSOT + 启用项写 `~/.claude/CLAUDE.md` |
| Skills | 远端 SSOT + `skills.json` + symlink/copy，与本机完全对称（含 ZIP 安装 / 导入 / 更新） |

### 批量应用
- 全屏面板：多选宿主机/容器 + 搜索 + 逐落点实时进度
- 后端广播命令一次连接改完，一键把**一个 Provider**推到多个落点

### 路由托管 / 接管
- SSH 反向隧道按意图对账（不重建连接）+ 端口动态化
- per-container DNAT（按容器 IP），容器×app 独立接管开关
- 隧道失败自动降级直连 + 警告提示 + DB 开关回退
- **本机代理按「远端各自」的当前供应商路由**（`PROXY_MANAGED:<host_id>` token 标记）
- 宿主机仅需开放 22 端口

### 远端功能总开关
- 设置 → 通用 → 远端设置：「启用远端功能」默认开启
- 关闭即**还原原生 cc-switch**（隐藏全部远端入口，不删数据）

---

## 二、桌面悬浮球（加速球）

- 桌面常驻透明小球 + 悬停展开面板（各 app 当前供应商 / 模型 / 余量）
- 独立窗口：透明 + 置顶 + 无边框 + 跳过任务栏；`floating.html` 独立 Vite 入口
- 拖动：Rust 端 `GetCursorPos` 全局光标轮询 + `GetAsyncKeyState`，绕开 WebView 事件
- 单击开主窗；松手边缘吸附（速度可调：快/中/慢/关闭）
- **边缘收起**：拖到屏幕边缘收成细条胶囊，悬停展开
- 右键菜单（设置 / 隐藏），与面板互斥
- 余量显示：**只读 `UsageCache` 缓存，绝不主动查 API**
- 显示目标：置顶 app 优先，否则跟随最近活跃
- 路由接管状态实时同步（流心动画边框）

---

## 三、本地体验增强

- **图像拦截钩子**：三道闸防止纯文本模型因上下文含图片块触发 400
  （PreToolUse `Read` / PreToolUse `*screenshot*` / PostToolUse `mcp__*`）
- **更新提示回归上游**：用官方 `UpdateBadge`，新增 `show_update_badge` 开关
- **窗口尺寸** 920×650（改 `tauri.windows.conf.json`，平台配置会 merge 覆盖主 config）
- Sonner toast portaling 修复、目标选择器可滚动、聚焦样式统一

---

## 四、其它

- OpenClaw 支持（MCP 模块 + 配置读写 + 远端 env/tools/agents 面板）
- 独立子 crate `src-tauri/sqlite-helper/`（远端 SQLite 会话直查）

---

## 我们删掉的官方文件（1 个）

| 文件 | 原因 |
|---|---|
| `session-manager.md` | 已过时，被 `docs/enhanced-plan.md` 取代 |
