> **官方 CC Switch `__VER__` + 本补丁层**，全平台自动构建。
> 补丁层 commit `__SHA__` · [构建日志](__RUN__)

## 🎁 比官方多了什么

| 功能 | 说明 |
|---|---|
| 🖥️ **SSH 远程主机控制面** | 在本机 GUI 里直连远程主机 / Docker 容器，直接读写远端 `~/.claude/settings.json`；远端 Provider / MCP / Prompts / Skills / 会话 全覆盖 |
| 🎯 **目标选择器** | 头部「本机 / 服务器 / 容器」连体胶囊，实时探活 |
| 📡 **批量应用** | 一个 Provider 一键推送到多台主机 / 容器，逐落点实时进度 |
| 🔀 **路由接管** | 远端经 SSH 反向隧道走本机代理（模型映射 / 格式转换），per-container DNAT |
| 🔴 **桌面悬浮球** | 常驻桌面、悬停展开、边缘自动收起、路由状态流心同步 |
| 🧩 **OpenClaw 支持** | MCP / 环境变量 / 工具 / Agents 面板，本机与远端对称 |

## 📦 下载哪个

| 平台 | 文件 | 说明 |
|---|---|---|
| Windows | `…-Windows-Setup.exe` | 安装版（推荐） |
| Windows | `…-Windows.msi` | MSI 安装包 |
| Windows | `…-Windows-Portable.zip` | 免安装，解压即用 |
| macOS | `…-macOS.dmg` | **未签名**：首次打开请右键 → 打开 |
| Linux | `…-Linux.deb` | Debian / Ubuntu |
| Linux | `…-Linux.AppImage` | 通用，`chmod +x` 后直接运行 |

`identifier` 保持 `com.ccswitch.desktop`，**与原版 CC Switch 共用同一份配置目录**
（`~/.cc-switch/`），可以平滑替换。

## ✅ 本版验证

- **组装**：官方源码 + 补丁层，121 个文件全部命中，零冲突
- **功能断言**：42 条全绿（`checks/feature-checks.ps1`）
- **官方更新零丢失**：门禁通过 —— 官方在本版的新增一处都没被我们的补丁挤掉

## 📝 上游对应版本

官方 `__VER__` 自己的更新内容，见 [上游 Release](https://github.com/farion1231/cc-switch/releases/tag/__VER__)。

---

由 [cc-switch-patches](https://github.com/Amengclass/cc-switch-patches) 自动构建 ·
补丁层说明见 [README](https://github.com/Amengclass/cc-switch-patches#readme)
