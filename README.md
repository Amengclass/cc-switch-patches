<div align="center">

# CC Switch Magic

### 给官方 CC Switch 打的一层补丁 —— 加上 SSH 远程主机统一控制面

[![Build](https://github.com/Amengclass/cc-switch-patches/actions/workflows/build.yml/badge.svg)](https://github.com/Amengclass/cc-switch-patches/actions/workflows/build.yml)
[![Stars](https://img.shields.io/github/stars/Amengclass/cc-switch-patches?style=social)](https://github.com/Amengclass/cc-switch-patches/stargazers)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](#-下载)
[![Built with](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)
[![Upstream](https://img.shields.io/badge/upstream-farion1231%2Fcc--switch-blueviolet.svg)](https://github.com/farion1231/cc-switch)

中文 | [English](README_EN.md) | [更新日志](CHANGELOG.md)

</div>

---

## ✨ 这是什么

**本仓库只有「补丁 + 工具」，没有任何源码，不能直接编译。**

它的用法是：把补丁套到**官方 [CC Switch](https://github.com/farion1231/cc-switch) 源码**上，
就得到带我们全部功能的完整版本。

```
官方源码（某个 tag）
      │  套补丁
      ▼
官方 + 我们的功能  ← 只有这棵“打过补丁的树”才拿去编译
      │  编译
      ▼
  cc-switch-magic.exe
```

> **官方源码永远不被修改** —— 补丁作用在拷贝出来的副本上。
> 因此官方怎么升级，我们都能重新套一遍，不需要长期 `merge`。

## 🎁 比官方多了什么

| 功能 | 说明 |
|---|---|
| 🖥️ **SSH 远程主机控制面** | 在本机 GUI 里直连远程主机 / Docker 容器，直接读写远端 `~/.claude/settings.json`；远端 Provider / MCP / Prompts / Skills / 会话 全覆盖 |
| 🎯 **目标选择器** | 头部「本机 / 服务器 / 容器」连体胶囊，实时探活 |
| 📡 **批量应用** | 一个 Provider 一键推送到多台主机 / 容器，逐落点实时进度 |
| 🔀 **路由接管** | 远端经 SSH 反向隧道走本机代理（模型映射 / 格式转换），per-container DNAT |
| 🔴 **桌面悬浮球** | 常驻桌面、悬停展开、边缘自动收起、路由状态流心同步 |
| 🧩 **OpenClaw 支持** | MCP / 环境变量 / 工具 / Agents 面板，本机与远端对称 |

## 🚀 快速开始

### 方式一：GitHub Actions（推荐 · 本机零依赖）

```
1. 打开 仓库页面 → Actions → 「Build CC Switch Magic」
2. 点「Run workflow」，填两个输入：
     version   = 官方版本号（如 v3.20.1；留空 = base.json 锁定版）
     platforms = 要构建哪些平台：全部 / Windows / Linux / macOS（可逗号分隔多个）
     make_release = 可选，勾了会顺手创建 Release
3. 点绿色「Run workflow」，等 20~40 分钟
4. 进运行详情 → 页面底部 Artifacts → 下载
```

> 只验证某个平台时把 `platforms` 填成 `macOS` 之类即可 —— 其余平台会被跳过，省时间。

或在本地推 tag 自动触发：

```bash
git tag magic-v3.20.1
git push origin magic-v3.20.1
```

### 方式二：本地一键

```powershell
.\magic.ps1 -Version v3.20.1     # 组装 + 验证 + 编译 → exe
```

产物：`cc-switch-build/src-tauri/target/debug/cc-switch-magic.exe`

### 方式三：分步调用脚本

想看清楚每一步时，挨个跑（都是已经验证过的脚本）：

```powershell
# ① 组装：拉官方 v3.20.1 → 套 overlay → 结构化合并 → 打 11 个补丁
.\scripts\apply.ps1 -TargetDir <目标目录> -Version v3.20.1

# ② 验证：42 条功能断言
.\checks\feature-checks.ps1 -TargetDir <目标目录>

# ③ 编译
.\scripts\build.ps1 -TargetDir <目标目录>
```

`apply.ps1` 内部做的就是三件事，等价于官方的「上游永不修改」原则：

| 步骤 | 动作 |
|---|---|
| 套 overlay | 把 `overlay/` 下 59 个文件整份拷进目标树 |
| 结构化合并 | 按 key 把 `structured/` 合并进 i18n / 配置 / 依赖 |
| 打补丁 | `git apply` 11 个 `.patch`（逐行改官方文件） |
## 📦 下载

由 GitHub Actions 自动构建，产物命名与官方一致：

| 平台 | 产物 |
|---|---|
| Windows | `CC-Switch-Magic-<版本>-Windows-Portable.zip`（便携版） / `.msi` / `-Setup.exe` |
| macOS | `CC-Switch-Magic-<版本>-macOS.dmg` |
| Linux | `CC-Switch-Magic-<版本>-Linux.deb` / `-Linux.AppImage` |

→ [查看全部构建产物](https://github.com/Amengclass/cc-switch-patches/actions)

## 🧩 补丁的分层结构

| 层 | 内容 | 数量 | 冲突风险 |
|---|---|---|---|
| `overlay/` | 官方**没有**的文件，整份拷贝 | 60 | **零** |
| `structured/` | i18n / 配置 / 依赖，按 key 合并 | 7 | **零** |
| `anchors/` | 我方**加性**改动按「符号锚点」重放 | 6 份 / 35 条 | **零**（只要锚点还在） |
| `patches/` | 对官方文件的逐行改动，按主题拆分 | 11 | 官方改同一行才会撞 |

> **为什么这样拆**：官方最常改的就是 i18n（近 400 个提交里改了 79 次）和配置文件。
> 走结构化合并后这类**永远不会冲突**；新增文件整份拷贝也**永不冲突**。
>
> **`anchors/` 是给「加性改动」准备的**：像「给函数加 `pub(crate)`」「加个结构体字段」
> 「插一行调用」这种改动，其实**不依赖上下文**，只依赖符号名还在。
> 写成锚点后，官方随便重构都不会撞（实测同一套锚点跨 v3.20.0 ~ v3.20.3 全部命中）。
> 锚点里**不许出现版本号**——版本号会过期，符号名不会。遇到「官方改了接口签名、
> 我们的 overlay 得跟着适配」时，用 `when` 条件锚点，判据是**另一个文件里有没有某段文本**。
>
> 剩下的才走 `patches/`（逐行补丁，带基线版本的上下文行）。
> 所以**新功能的实现方式越「加性」，升级官方时越省事**。

## ➕ 如何增加新功能

**核心**：改「我们那份源码树」，然后重新抽取补丁。

### 第 1 步：改代码

在你本地的**我们的源码树**里直接改（那份 = 官方 + 当前全部补丁）：

| 要加的东西 | 改哪 |
|---|---|
| 前端页面 / 组件 | `src/` |
| 后端命令 / 逻辑 | `src-tauri/src/` |
| 全新文件 | 放正常位置即可 |

**不用操心「这算 overlay 还是算补丁」** —— 下一步会自动判断。

### 第 2 步：本地跑起来看效果

```powershell
pnpm install
pnpm tauri dev        # 开发模式：前端热更新，Rust 自动重编译
```

> 跑之前先 `Stop-Process -Name cc-switch-magic -Force`，否则被单实例锁挡住。

### 第 3 步：重新抽取补丁层

```powershell
.\scripts\repack.ps1 -EditedRepo <我们的源码树>
```

它会自动：扫源码 → 对比官方 → 重新生成 `overlay/` + `patches/` + `structured/`。

### 第 4 步：验证

```powershell
.\scripts\verify.ps1 -ReferenceRepo <我们的源码树>
```

期望 `0 真实差异` —— 证明「官方 + 补丁 = 我们的源码」。

### 第 5 步：推送

```powershell
git add -A
git commit -m "feat: 新功能 xxx"
git push
```

### 两个建议

**① 尽量「追加」而不是「重写」官方文件**

```diff
# 好：追加，官方怎么改都不撞
+ if (isRemote) { ... }

# 差：重写，官方一改就冲突
- 官方原来的 10 行
+ 我们改写的 15 行
```

**② 前端文案走 i18n**，别硬编码中文：

```tsx
t("myFeature.title")   // 然后在 src/i18n/locales/{zh,en,ja,zh-TW}.json 加 key
```

**③ 加性改动写进 `anchors/`，别靠逐行补丁**

「给函数加 `pub(crate)`」「加个结构体字段」「插一行调用」这类改动只依赖**符号名**，
写成锚点后官方重构到旁边也不会撞（`patches/` 的行补丁则会，因为它带着基线版本的上下文行）。

```jsonc
{
  "file": "src-tauri/src/tray.rs",
  "edits": [
    { "why": "悬浮窗要复用托盘函数", "find": "fn emoji_for_utilization(",
      "replace": "pub(crate) fn emoji_for_utilization(", "expect": 1 }
  ]
}
```

两条铁律：**锚点里不许出现版本号**（版本号会过期，符号名不会）；
**`expect` 必须写**（数量对不上就报错，绝不静默跳过）。
遇到「官方改了接口签名、我们的 overlay 得跟着适配」，用 `when` 条件锚点 ——
判据是**另一个文件里有没有某段文本**，不是版本号。

## 🛠️ 脚本清单

| 脚本 | 作用 |
|---|---|
| `magic.ps1` | 一键：准备官方源码 → 组装 → 验证 → 编译 |
| `scripts/apply.ps1` | 官方源码 → 套 overlay → 结构化合并 → 打补丁 → 锚点重放 |
| `scripts/build.ps1` | 前端打包 + Rust 编译 → exe |
| `scripts/verify.ps1` | 五层验证编排（见下） |
| `scripts/repack.ps1` | 改完功能后重新生成补丁层 |
| `scripts/replay-anchors.mjs` | 锚点重放引擎（加性改动，幂等，支持 `when` 条件锚点） |
| `scripts/check-upstream-sync.mjs` | **零丢失门禁**：证明官方新增的每一行都还在 |
| `scripts/compare-trees.mjs` | 树比对（JSON/TOML 走语义比较） |
| `scripts/fix-crlf-patches.mjs` | 防护：CRLF 文件的补丁换行（`git diff --output=` 出问题时兜底） |
| `scripts/_proxy.ps1` | git 代理自动探测（本机有代理就走，CI 直连） |
| `checks/feature-checks.ps1` | 42 条功能断言 |

## ✅ 验证机制（保证「功能一个都没丢」）

| 层 | 手段 | 抓什么 |
|---|---|---|
| **L1 结构** | `cargo check` + `pnpm typecheck` | 官方改动导致的结构断裂 |
| **L2 逻辑** | `cargo test` + `vitest` | 行为回归 |
| **L3 断言** | `checks/feature-checks.ps1`（42 条） | **功能被静默吃掉**（不报编译错的那种） |
| **L4 还原** | `scripts/compare-trees.mjs` | 组装结果 vs 参照源码的逐文件差异 |
| **L5 零丢失** | `scripts/check-upstream-sync.mjs` | **官方的新增被我们丢掉**（升级官方时最危险的错误） |

`verify.ps1 -ReferenceRepo <我们的源码>` 跑 L1~L4；
升级官方时用 `verify.ps1 -UpgradeFrom <官方旧版树> -UpgradeTo <官方新版树>` 跑 L3+L5。

## ❓ FAQ

<details>
<summary><b>官方升级到新版本了，补丁还打得进去吗？</b></summary>

实测官方 v3.20.1 → v3.20.3（跨 2 个版本）：**零冲突**，官方更新零丢失。

流程是固定的三条：

```powershell
# 1) 组装：拉官方新版 → 套 overlay → 结构化合并 → 打补丁 → 锚点重放
.\scripts\apply.ps1 -TargetDir <新目录> -Version v3.20.3
# 2) 零丢失门禁：证明官方新增的每一行都还在
node .\scripts\check-upstream-sync.mjs --from <官方旧版树> --to <官方新版树> `
     --ours <我们的源码> --merged <组装结果> --waivers .\upstream-waivers.json
# 3) 编译
.\scripts\build.ps1 -TargetDir <新目录>
```

`apply.ps1` 会**逐个文件**应用补丁，所以一个文件冲突不会连累同主题的其它几十个文件；
`anchors/` 里那些加性改动则完全不受影响，自动重放。

**万一还是冲突**（官方和我们改了同一行语义）：**不要整个文件取一边** ——
那会把官方在这个文件里的更新静默丢掉。正解是按锚点重贴，或用
`git merge-file` 做三方合并（base=官方旧版, ours=我们, theirs=官方新版），
最后跑 L5 门禁证明没丢东西。
</details>

<details>
<summary><b>为什么编译出的 exe 有 80 MB？</b></summary>

因为那是 **debug 版**。用 `pnpm tauri build`（release）出的安装包约 **13 MB**。

也可以加 `--release`：`cargo build --release --features tauri/custom-protocol`。
</details>

<details>
<summary><b>构建时提示 exe 被占用 / os error 5？</b></summary>

先停掉正在运行的实例：`Stop-Process -Name cc-switch-magic -Force`
（Tauri 有单实例锁，exe 被占用时链接会失败）。
</details>

<details>
<summary><b>为什么 `patches/` 里的补丁打不上？</b></summary>

检查换行符。**`.patch` 必须是 LF**，CRLF 会让 `git apply` 全部失败。
本仓库有 `.gitattributes` 强制字节保真，**别删**。
</details>

## 📁 项目结构

```text
cc-switch-patches/
├── overlay/                      # 官方没有的文件（60 个）→ 整份拷贝
├── structured/                   # i18n / 配置 / 依赖 → 按 key 合并
├── anchors/                      # 加性改动 → 按符号锚点重放（6 份 / 35 条）
├── patches/                      # 对官方文件的逐行改动（11 个主题补丁）
├── checks/feature-checks.ps1     # 42 条功能断言
├── scripts/                      # 组装 / 编译 / 验证 / 重抽取 / 门禁
├── upstream-waivers.json         # L5 门禁的显式豁免表（每条须写明等价改写）
├── .github/workflows/build.yml   # 全平台自动构建
├── magic.ps1                     # 一键入口
├── base.json                     # 基线锁定（官方仓库 / tag / commit）
└── README.md
```

## 🤝 贡献

欢迎提 Issue / PR。改功能请按上面「[如何增加新功能](#-如何增加新功能)」的流程，
并确保 `verify.ps1` 全绿。

## 📄 License

[MIT](LICENSE)

本项目是对 [farion1231/cc-switch](https://github.com/farion1231/cc-switch)（MIT）的补丁层。