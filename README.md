# CC Switch Magic —— 官方补丁层

> **本目录只有「补丁 + 工具」，没有任何源码，所以不能直接编译。**
> 用法：把补丁打到**官方源码**上 → 得到一棵可编译的树 → 编译它。

---

## 一、先认清目录分工（最容易搞混的地方）

```
C:\Users\Ameng\Desktop\claude_woker\cc_work\
├── cc-switch-refactor\   ← 我们的源码（唯一真相；重构 / 改功能在这里做）
├── cc-switch-magic\      ← 本目录：补丁层（从上面的源码抽出来）
├── cc-switch-official\   ← 官方仓（只读，对照用）
├── cc-switch\            ← 已发布的成品版（GitHub 上那份，【不要动】）
└── cc-switch-build\      ← 组装 + 编译产物（可随时删掉重新生成）
```

| 目录 | 身份 | 能不能改 |
|---|---|---|
| `cc-switch-refactor` | **我们的源码**（当前 = 官方 v3.20.1 + 全部改动） | ✅ 改这个 |
| `cc-switch-magic` | **补丁层**（overlay / patches / structured / scripts） | ✅ 只加脚本和补丁 |
| `cc-switch-official` | 官方 `farion1231/cc-switch` 完整克隆 | ❌ 只读 |
| `cc-switch` | **已发布成品**（`Amengclass/cc-switch`，`0c255bb3`） | ❌ 不动 |
| `cc-switch-build` | 组装+编译的工作目录 | 随便，可删可重建 |

> 补丁 = **「我们的源码」减去「官方源码」**。所以想加功能，改的是 `cc-switch-refactor`，
> 然后 `repack` 重新抽取补丁。

---

## 二、两条流程

### 【A】官方发新版 → 出新的魔改版 exe

```powershell
cd C:\Users\Ameng\Desktop\claude_woker\cc_work\cc-switch-magic

# ① 组装：拉官方 vX.Y.Z → 套 overlay + 结构化合并 + 打补丁
.\scripts\apply.ps1 -TargetDir ..\cc-switch-build -Version v3.21.0

# ② 验证：功能断言 + 编译（升级后必跑）
.\scripts\verify.ps1 -OfficialDir ..\cc-switch-build

# ③ 出 exe
.\scripts\build.ps1 -TargetDir ..\cc-switch-build
# 产物：..\cc-switch-build\src-tauri\target\debug\cc-switch.exe
```

> 第 ① 步若报 `[FAIL] xxx.patch` —— 就是**该主题撞了官方改动**，只影响那一个补丁，
> 其余照常应用。手工解冲突后重新 `repack` 即可。

### 【B】我们改了功能 → 更新补丁层

```powershell
# ① 在 cc-switch-refactor 里改代码（改完提交）

# ② 重新抽取补丁层
.\scripts\repack.ps1 -EditedRepo ..\cc-switch-refactor

# ③ 验证：官方 + 补丁 是否 == 我们的源码
.\scripts\verify.ps1 -ReferenceRepo ..\cc-switch-refactor
```

---

## 三、三层补丁结构

| 层 | 内容 | 为什么这样 |
|---|---|---|
| **A `overlay/`** | 官方**没有**的文件（58 个，含 `remote/*`、`floating/*`、`magic.rs` …） | 整文件拷贝 → **永不冲突** |
| **B `patches/`** | 对官方文件的改动，按主题拆成 10 个 `.patch` | 冲突隔离在单个主题内 |
| **C `structured/`** | i18n / `Cargo.toml` / `tauri.conf.json` | 按 key / 依赖名合并 → **结构上不可能冲突** |

> C 层是有意设计：官方最常改的就是 **i18n（近 400 提交里改了 79 次）**和配置文件，
> 走结构化合并后这些**永远不会冲突**。

---

## 四、脚本清单

| 脚本 | 作用 |
|---|---|
| `scripts/apply.ps1` | 官方源码 → overlay → 结构化合并 → 打补丁（得到可编译的树） |
| `scripts/build.ps1` | 前端打包 + Rust 编译 → exe |
| `scripts/verify.ps1` | 四层验证编排（L1 编译 / L2 测试 / L3 断言 / L4 还原比对） |
| `scripts/repack.ps1` | 改完功能后重新生成 overlay + patches + structured |
| `scripts/gen-patches.ps1` | 由「官方 vs 我们」生成主题补丁（`repack` 会调它） |
| `scripts/gen-structured.mjs` | 生成 i18n delta |
| `scripts/merge-structured.mjs` | 合并 i18n / 配置 / 依赖到目标树 |
| `scripts/compare-trees.mjs` | 树比对（JSON/TOML 走语义比较，区分「真差异」与「仅格式」） |
| `checks/feature-checks.ps1` | 40 条功能断言 |

**`verify.ps1` 参数**：
- `-ReferenceRepo <我们的源码>` → 跑 L4（组装结果 vs 我们的源码逐文件比对）；**仅在我们想确认「补丁完整」时用**
- 不传 `-ReferenceRepo` → 跳过 L4，只跑 L1/L2/L3（**升级官方新版时用这个**）

---

## 五、验证机制（保证「功能一个都没丢」）

| 层 | 手段 | 抓什么 |
|---|---|---|
| **L1 结构** | `cargo check` + `pnpm typecheck` | 官方改动导致的结构断裂 |
| **L2 逻辑** | `cargo test` + `vitest` | 行为回归 |
| **L3 断言** | `checks/feature-checks.ps1`（40 条） | **功能被静默吃掉**（这种不会报编译错） |
| **L4 还原** | `scripts/compare-trees.mjs` | 组装结果 vs 参照源码的逐文件差异 |

「可接受的差异」只有 3 类，`compare-trees.mjs` 会自动识别：
`i18n/*.json` 键顺序、`Cargo.toml` 条目顺序、`tauri.conf.json` 数组换行格式
（都是语义相同），外加 `Cargo.lock` 由 cargo 自动重生成。

---

## 六、当前进度

| 阶段 | 状态 |
|---|---|
| **P1** 补丁流水线 | ✅ 完成并验证通过 |
| **P2** 收敛侵入面 | 🔄 已完成 3 个最高风险文件 |
| **P3** CI 自动化 | ⬜ 未开始 |

**P2 已收敛**（让官方升级时冲突更少）：

| 文件 | 改前 | 改后 |
|---|---|---|
| `src-tauri/src/database/schema.rs` | 92 行 | **4 行** |
| `src-tauri/src/lib.rs` | 162 行 | **125 行** |
| `src-tauri/src/services/proxy.rs` | 47 行 | **6 行** |

手法：把逻辑搬进 overlay 模块（`remote::schema` / `magic` / `remote::hooks`），
官方文件里只留**一行调用**。

---

## 七、注意事项

- **`patches/*.patch` 必须是 LF 换行**（CRLF 会让 `git apply` 全部失败）。
  本目录有 `.gitattributes` 强制字节保真，别删。
- `apply.ps1` 会**先 `git checkout <Version>`** 再打补丁——因为官方仓可能停在 `main`（最新版）。
- 本仓库**不含官方代码**，官方源码由 `apply.ps1` 按需从 `base.json` 里的 `upstream` 拉取。