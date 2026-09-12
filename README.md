# CC Switch Magic —— 官方补丁层

> **本仓库只有「补丁 + 工具」，没有任何源码，所以不能直接编译。**
> 用法：把补丁套到**官方源码**上 → 得到一棵可编译的树 → 编译它。

---

## 一、怎么工作的

```
官方源码（farion1231/cc-switch @ 某个 tag）
        │  ① 套用本仓库的补丁（apply）
        ▼
   一棵「官方 + 我们的改动」的源码树
        │  ② 编译（build）
        ▼
   cc-switch-magic.exe（窗口标题 CC Switch Magic）
```

**补丁 = 「我们的源码」减去「官方源码」**。所以想加功能，改的是我们那份源码树，
改完用 `repack` 重新抽取补丁（见下文「两条流程」）。

## 二、本仓库的结构

| 目录 / 文件 | 内容 | 冲突风险 |
|---|---|---|
| `overlay/` | 官方**没有**的文件（59 个：`remote/*`、`floating/*`、`magic.rs` …） | **零**（整份拷贝） |
| `patches/` | 对官方文件的改动，按主题拆成 **11 个** `.patch` | 官方改同一行才会撞 |
| `structured/` | i18n / 配置 / 依赖，按 key / 依赖名合并 | **零**（结构上不可能冲突） |
| `scripts/` | `apply` / `build` / `verify` / `repack` / `gen-patches` | — |
| `checks/` | 42 条功能断言（L3） | — |
| `magic.ps1` | **一键**：组装 + 验证 + 编译 | — |
| `base.json` | 基线锁定：官方仓库地址 / tag / commit | — |
| `.github/workflows/build.yml` | GitHub Actions：网页上点一下即出 exe | — |

> **三层设计的意义**：官方最常改的就是 i18n（近 400 提交里改了 79 次）和配置文件，
> 走结构化合并后**永远不会冲突**；新增文件整份拷贝也**永不冲突**；
> 只有「对官方文件的逐行修改」才有冲突可能，而它们被拆成 11 个互不相干的小主题。

---

## 三、两条流程

### 【A】官方发新版 → 出新的魔改版 exe

**方式一：本地一键**

```powershell
.\magic.ps1 -Version v3.21.0     # 组装 + 验证 + 编译 → exe
```

**方式二：GitHub 网页（本机什么都不用装）**

```
仓库页面 → Actions → Build CC Switch Magic → Run workflow → 填版本号
→ 等 20~40 分钟 → 页面下方 Artifacts 下载 exe
```

### 【B】改了功能 → 更新补丁层

```powershell
# ① 在我们的源码树里改代码
# ② 重新抽取补丁层（把源码树路径传给 -EditedRepo）
.\scripts\repack.ps1 -EditedRepo <我们的源码树>

# ③ 验证：官方 + 补丁 是否 == 我们的源码树
.\scripts\verify.ps1 -ReferenceRepo <我们的源码树>
```

---

## 四、脚本清单

| 脚本 | 作用 |
|---|---|
| `magic.ps1` | 一键：准备官方源码 → 组装 → 验证断言 → 编译 |
| `scripts/apply.ps1` | 官方源码 → 套 overlay → 结构化合并 → 打补丁（得到可编译的树） |
| `scripts/build.ps1` | 前端打包 + Rust 编译 → exe |
| `scripts/verify.ps1` | 四层验证编排（L1 编译 / L2 测试 / L3 断言 / L4 还原比对） |
| `scripts/repack.ps1` | 改完功能后重新生成 overlay + patches + structured |
| `scripts/gen-patches.ps1` | 由「官方 vs 我们」生成主题补丁（`repack` 会调它） |
| `scripts/gen-structured.mjs` | 生成 i18n delta |
| `scripts/merge-structured.mjs` | 合并 i18n / 配置 / 依赖到目标树 |
| `scripts/compare-trees.mjs` | 树比对（JSON/TOML 走语义比较，区分「真差异」与「仅格式」） |
| `scripts/_proxy.ps1` | git 代理自动探测（本机有 Clash 就走，CI 上直连） |
| `checks/feature-checks.ps1` | 42 条功能断言 |

**`verify.ps1` 参数**：
- `-ReferenceRepo <我们的源码树>` → 跑 L4（组装结果 vs 我们的源码逐文件比对）
- 不传 `-ReferenceRepo` → 跳过 L4，只跑 L1/L2/L3（**升级官方新版时用这个**）

---

## 五、验证机制（保证「功能一个都没丢」）

| 层 | 手段 | 抓什么 |
|---|---|---|
| **L1 结构** | `cargo check` + `pnpm typecheck` | 官方改动导致的结构断裂 |
| **L2 逻辑** | `cargo test` + `vitest` | 行为回归 |
| **L3 断言** | `checks/feature-checks.ps1`（42 条） | **功能被静默吃掉**（这种不会报编译错） |
| **L4 还原** | `scripts/compare-trees.mjs` | 组装结果 vs 参照源码的逐文件差异 |

「可接受的差异」只有 3 类，`compare-trees.mjs` 会自动识别：
`i18n/*.json` 键顺序、`Cargo.toml` 条目顺序、`tauri.conf.json` 数组换行格式
（都是语义相同），外加 `Cargo.lock` 由 cargo 自动重生成。

---

## 六、拆解：`magic.ps1` 内部实际做的五步

想完全掌控时可手动执行（把三个路径换成你自己的）：

```powershell
$OFF  = "<官方源码目录>"
$DEST = "<组装目标目录>"
$M    = "<本仓库目录>"

# ① 复制官方源码（官方目录永远不动）
Copy-Item $OFF $DEST -Recurse -Force

# ② 切到目标版本
cd $DEST; git checkout v3.20.1

# ③ 套 overlay（59 个官方没有的新文件，整拷 → 永不冲突）
Get-ChildItem "$M\overlay" -Recurse -File | ForEach-Object {
  $rel = $_.FullName.Substring("$M\overlay".Length).TrimStart('\')
  $d = Join-Path $DEST $rel
  New-Item -ItemType Directory -Force -Path (Split-Path $d -Parent) | Out-Null
  Copy-Item $_.FullName $d -Force
}

# ④ 结构化合并（i18n / 配置 / 依赖，按 key 合并 → 结构上不可能冲突）
node "$M\scripts\merge-structured.mjs" "$M\structured" $DEST

# ⑤ 打补丁（对官方文件逐行修改）
Get-ChildItem "$M\patches\*.patch" | Sort-Object Name | ForEach-Object {
  git apply --whitespace=nowarn $_.FullName
}
```

### 三种改动方式，对应三档冲突风险

| 方式 | 本项目数量 | 冲突风险 |
|---|---|---|
| **overlay** 整拷新文件 | 59 | **零**（官方没有这些路径） |
| **结构化合并** i18n/配置/依赖 | 7 | **零**（按 key / 依赖名合并） |
| **补丁** 逐行改官方文件 | 120 | 官方改了同一行才会撞 |

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

---

## 七、注意事项

- **`patches/*.patch` 必须是 LF 换行**（CRLF 会让 `git apply` 全部失败）。
  仓库有 `.gitattributes` 强制字节保真，别删。
- `apply.ps1` 会**先 `git checkout <Version>`** 再打补丁 —— 因为官方仓可能停在 `main`（最新版）。
- 本仓库**不含官方代码**，官方源码由 `apply.ps1` 按需从 `base.json` 里的 `upstream` 拉取。
- `Cargo.toml` 走**补丁**（`0310-rust-cargo`，含包名 `cc-switch-magic` 与新增依赖）；
  代价是官方改 `Cargo.toml` 时该补丁可能冲突。