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

## [3.20.1-1] - 2026-09-12

基于官方 **v3.20.1** 的首个补丁层版本。

### Added

- 三层补丁结构：`overlay/`（55 个新文件）+ `patches/`（10 个主题补丁）+ `structured/`（i18n / 配置 / 依赖）
- 四层验证：结构（`cargo check` / `typecheck`）、逻辑（测试）、断言、还原比对
- 工具链：`apply` / `build` / `verify` / `repack` / `gen-patches` / `compare-trees`

[Unreleased]: https://github.com/Amengclass/cc-switch-patches/commits/main
[3.20.1-1]: https://github.com/Amengclass/cc-switch-patches/releases