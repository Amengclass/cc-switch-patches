# Changelog

本项目的所有重要变更都记录在此。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。
本项目的「版本号」对应**所基于的官方版本**（如 `3.20.1` = 基于官方 v3.20.1）。

## [Unreleased]

### Added

- 正式开源：中英双语 README、`LICENSE`（MIT）、本更新日志
- 全平台自动构建（GitHub Actions）：Windows 便携版 + `.msi` + `-Setup.exe`、macOS `.dmg`、Linux `.deb` + `.AppImage`，命名遵循官方规则
- `scripts/_proxy.ps1`：git 代理自动探测（本机有代理就走，CI 直连）
- 42 条功能断言（`checks/feature-checks.ps1`）

### Changed

- 应用显示名与窗口标题改为 **CC Switch Magic**（走结构化覆盖，长期生效）
- `Cargo.toml` 改为**补丁机制**（`0310-rust-cargo`，含包名 `cc-switch-magic` 与新增依赖）
- 仓库改名 `cc-switch-magic` → `cc-switch-patches`（避免与编译产物同名）
- 脚本路径改用正斜杠，支持在 Linux / macOS runner 上组装
- P2 收敛侵入面：把逻辑从官方文件搬进 overlay 模块，官方文件只留一行调用
  - `database/schema.rs` 92 → 4 行
  - `lib.rs` 162 → 125 行
  - `services/proxy.rs` 47 → 6 行
  - `tray.rs` 43 → 31 行
  - `useProviderActions.ts` 59 → 20 行

### Fixed

- `App.tsx` 给 `UsageScriptModal` 传了它不接受的 props（TypeScript 类型错误）
- 补丁主题前缀重叠 —— 同一文件被写进两个补丁，导致二次应用冲突
- overlay 文件按**原字节**恢复（避免 git 往返改变换行符）
- 树比对排除规则改为按路径段判断（顶层 `node_modules` / `target` 之前未被排除）
- 补丁写入改用 `git diff --output`，避免 PowerShell 文本管道破坏非 UTF-8 字节

## [3.20.4-1] - 2026-09-28

基于官方 **v3.20.4** 的补丁层版本。

### Added

- 远端（SSH 主机 / Docker 容器）支持官方 v3.20.4 新增的第 10 个托管应用 **MiniMax Code（mcode）**：供应商读写、MCP、Skills、提示词四项能力；会话（只读 SQLite）跟随官方现状，暂不介入
- `remote/mcode.rs`：远端 `~/.minimax/config.yaml` 的读写 + `kind: custom` 过滤（加法模式，不整文件覆盖）
- 功能断言扩到 55 条（新增 J 节：11 条 mcode 断言 + 2 条回归断言）

### Fixed

- 切换页面 / 切换 app 时页面「闪一下，同一份数据像加载了两次」：`App.tsx` 的视图切换与供应商列表原本用 `AnimatePresence mode="wait"`，会先淡出并卸载旧页面、再挂载新页面，中间留出一段空白窗口（新页面又是挂载后才发请求）；改为直接渲染、即时切换
- 远端提示词路径不再静默兜底到 `claude` —— 未知应用返回错误，而不是写错文件
- 远端 MCP / Skills 的 `pi` 此前漏在清单外，已补上
- Mcode 无自动安装命令时，「安装」按钮不再弹出空白命令框，改为提示「当前版本暂未支持」

### Changed

- 补丁基线从官方 v3.20.3 升到 **v3.20.4**（`base.json`）

## [3.20.1-1] - 2026-09-12

基于官方 **v3.20.1** 的首个补丁层版本。

### Added

- 三层补丁结构：`overlay/`（55 个新文件）+ `patches/`（10 个主题补丁）+ `structured/`（i18n / 配置 / 依赖）
- 四层验证：结构（`cargo check` / `typecheck`）、逻辑（测试）、断言、还原比对
- 工具链：`apply` / `build` / `verify` / `repack` / `gen-patches` / `compare-trees`

[Unreleased]: https://github.com/Amengclass/cc-switch-patches/commits/main
[3.20.4-1]: https://github.com/Amengclass/cc-switch-patches/releases
[3.20.1-1]: https://github.com/Amengclass/cc-switch-patches/releases