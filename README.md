# CC Switch Magic —— 官方补丁层

> **本目录只有「补丁 + 工具」，没有任何源码，所以不能直接编译。**
> 用法：把补丁打到**官方源码**上 → 得到一棵可编译的树 → 编译它。

---

## 一、先认清目录分工（最容易搞混的地方）

```
C:\Users\Ameng\Desktop\claude_woker\cc_work\
├── cc-switch-refactor\   ← 我们的源码（唯一真相；重构 / 改功能在这里做）
├── cc-switch-patches\      ← 本目录：补丁层（从上面的源码抽出来）
├── cc-switch-official\   ← 官方仓（只读，对照用）
├── cc-switch\            ← 已发布的成品版（GitHub 上那份，【不要动】）
└── cc-switch-build\      ← 组装 + 编译产物（可随时删掉重新生成）
```

| 目录 | 身份 | 能不能改 |
|---|---|---|
| `cc-switch-refactor` | **我们的源码**（当前 = 官方 v3.20.1 + 全部改动） | ✅ 改这个 |
| `cc-switch-patches` | **补丁层**（overlay / patches / structured / scripts） | ✅ 只加脚本和补丁 |
| `cc-switch-official` | 官方 `farion1231/cc-switch` 完整克隆 | ❌ 只读 |
| `cc-switch` | **已发布成品**（`Amengclass/cc-switch`，`0c255bb3`） | ❌ 不动 |
| `cc-switch-build` | 组装+编译的工作目录 | 随便，可删可重建 |

> 补丁 = **「我们的源码」减去「官方源码」**。所以想加功能，改的是 `cc-switch-refactor`，
> 然后 `repack` 重新抽取补丁。

---

## 二、两条流程

### 【A】官方发新版 → 出新的魔改版 exe

```powershell
cd C:\Users\Ameng\Desktop\claude_woker\cc_work\cc-switch-patches

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

---

## 八、一条命令 vs 手动五步

### 日常用法：一条命令

```powershell
cd C:\Users\Ameng\Desktop\claude_woker\cc_work\cc-switch-patches
.\magic.ps1 -Version v3.20.1     # 组装 + 验证 + 编译 → exe
```

`magic.ps1` 把下面五步 + 功能断言 + 编译全包了。**日常只需要这个。**

### 拆解：`magic.ps1` 内部实际做的五步

想完全掌控时，可手动执行：

```powershell
$OFF  = "C:\Users\Ameng\Desktop\claude_woker\cc_work\cc-switch-official"
$DEST = "C:\Users\Ameng\Desktop\claude_woker\cc_work\cc-switch-build"
$M    = "C:\Users\Ameng\Desktop\claude_woker\cc_work\cc-switch-patches"

# ① 复制官方源码（官方目录永远不动）
Copy-Item $OFF $DEST -Recurse -Force

# ② 切到锁定版本
cd $DEST; git checkout v3.20.1

# ③ 套 overlay（58 个官方没有的新文件，整拷 → 永不冲突）
Get-ChildItem "$M\overlay" -Recurse -File | ForEach-Object {
  $rel = $_.FullName.Substring("$M\overlay".Length).TrimStart('\')
  $d = Join-Path $DEST $rel
  New-Item -ItemType Directory -Force -Path (Split-Path $d -Parent) | Out-Null
  Copy-Item $_.FullName $d -Force
}

# ④ 结构化合并（i18n / 依赖 / 配置，按 key 合并 → 结构上不可能冲突）
node "$M\scripts\merge-structured.mjs" "$M\structured" $DEST

# ⑤ 打补丁（对官方文件逐行修改）
Get-ChildItem "$M\patches\*.patch" | Sort-Object Name | ForEach-Object {
  git apply --whitespace=nowarn $_.FullName
}
```

### 三种改动方式，对应三档冲突风险

| 方式 | 本项目数量 | 冲突风险 |
|---|---|---|
| **overlay** 整拷新文件 | 58 | **零**（官方没有这些路径） |
| **结构化合并** i18n/配置/依赖 | 7 | **零**（按 key / 依赖名合并） |
| **补丁** 逐行改官方文件 | 127 | 官方改了同一行才会撞 |

### 补丁文件长什么样

```
--- a/src-tauri/src/commands/mod.rs      ← 改之前（官方）
+++ b/src-tauri/src/commands/mod.rs      ← 改之后（我们）
@@ -26,7 +26,7 @@ mod proxy;
 mod session_manager;                     ← 空格开头 = 上下文（定位用，不改）
-mod stream_check;                        ← 减号 = 删掉官方这行
+pub(crate) mod stream_check;             ← 加号 = 换成这行
```

`git apply` 的动作就是：**找上下文 → 把减号行换成加号行**。
127 个文件的上千处这种改动，合起来就是补丁层。

### 组装完的产物（git 视角）

```
185 files changed, 30588 insertions(+), 1644 deletions(-)
  修改官方文件: 127 个
  新增文件:     58 个
```