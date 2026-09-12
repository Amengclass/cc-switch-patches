# CC Switch Magic —— 官方补丁层

> **官方源码就是底座，我们只加一层补丁。** 官方发新版 → 套补丁 → 重新构建 → 产出新 exe。
> 本仓库**不含官方代码**，只含差异部分。

基线官方版本见 [`base.json`](base.json)（当前 `v3.20.1`）。

---

## 为什么不是「官方 exe 上打补丁」

Tauri 应用的 **Rust 后端 + 前端产物都是编译期焊死进 exe 的**，且官方没有任何插件/扩展机制。
所以补丁作用在**源码层**，必须重新构建。

---

## 目录结构

```
├── base.json          # 基线锁定：官方 tag / commit / 计数
├── overlay/           # A 层：55 个纯新增文件（官方没有的路径）→ 整拷，永不冲突
├── patches/           # B 层：10 个按主题拆分的补丁 → 冲突只影响单个主题
├── structured/        # C 层：i18n / 依赖 / 配置 的结构化载荷 → 结构上不可能冲突
│   ├── i18n/*.json        #   只放我们新增/改值的 key
│   ├── cargo-additions.json
│   └── config-overrides.json
├── checks/            # L3 功能断言清单
└── scripts/           # 工具链
```

## 三层设计

| 层 | 内容 | 冲突性 |
|---|---|---|
| **A overlay** | 官方没有的文件（`remote/*`、`floating/*`、`components/remote/*` …） | **永不冲突**（除非官方新建同名文件） |
| **B patches** | 对官方文件的改动，按主题拆 10 个补丁 | 冲突隔离到单个主题 |
| **C structured** | i18n / Cargo.toml / tauri.conf.json | **结构上不可能冲突**（按 key / 依赖名合并） |

---

## 常用命令

### 官方发新版：升级

```powershell
# 一键：拉官方 tag → 套 overlay → 结构化合并 → 打补丁
.\scripts\apply.ps1 -TargetDir D:\build\cc-switch -Version v3.21.0

# 或者：复用已克隆的官方源码
.\scripts\apply.ps1 -TargetDir D:\build\cc-switch -OfficialDir C:\path\to\official
```

### 验证（升级后必跑）

```powershell
# L3 功能断言 + L4 与参考树还原比对（秒级）
.\scripts\verify.ps1 -ReferenceRepo <已知可用的仓库>

# 追加 L1 结构（cargo check / typecheck）+ L2 逻辑（单测）—— 分钟级
.\scripts\verify.ps1 -ReferenceRepo <已知可用的仓库> -WithBuild
```

### 构建产物

```powershell
.\scripts\build.ps1 -TargetDir D:\build\cc-switch          # debug exe
.\scripts\build.ps1 -TargetDir D:\build\cc-switch -Release # release（发布用）
```

### 我们改了功能之后：重新打包补丁

```powershell
# 在一个「已组装」的树里改完后
.\scripts\repack.ps1 -EditedRepo D:\build\cc-switch
```

---

## 四层验证（保证「功能一个都没丢」）

| 层 | 手段 | 抓什么 |
|---|---|---|
| **L1 结构** | `pnpm typecheck` + `cargo check` | 官方改动导致的结构断裂 |
| **L2 逻辑** | `cargo test` + `pnpm test:unit` | 行为回归 |
| **L3 断言** | `checks/feature-checks.ps1`（39 条） | **功能被静默吃掉**（不报编译错的那种） |
| **L4 还原** | `scripts/compare-trees.mjs` | 组装树与已知可用态的逐文件差异 |

**L3 是核心**：官方代码被吃掉不会报错，只会功能消失。断言清单逐条写死「我们的功能应该长什么样」。

---

## 已知的「可接受差异」

比对时以下差异属正常，不算失败：

| 文件 | 原因 |
|---|---|
| `src/i18n/locales/*.json` | 键顺序不同（语义相同）——官方顺序 + 我们的 key 追加在末尾 |
| `src-tauri/Cargo.toml` | 依赖条目顺序不同（语义相同）——我们的依赖插在 `[dependencies]` 开头 |
| `src-tauri/tauri.conf.json` | JSON 数组换行格式不同（语义相同） |
| `src-tauri/Cargo.lock` | 由 cargo 首次构建时自动重生成，本就不打补丁 |

`compare-trees.mjs` 会自动把前三类判为「语义相同」，第四类判为「工具链重生成」。
装了 `prettier` 后 `apply.ps1` 会自动规范化前三类的格式。

---

## 功能清单

我们相对官方新增了什么，见 [`docs/MAGIC.md`](docs/MAGIC.md)。
