<div align="center">

# CC Switch Magic

### A patch layer for the official CC Switch — adds a unified SSH remote-host control plane

[![Build](https://github.com/Amengclass/cc-switch-patches/actions/workflows/build.yml/badge.svg)](https://github.com/Amengclass/cc-switch-patches/actions/workflows/build.yml)
[![Stars](https://img.shields.io/github/stars/Amengclass/cc-switch-patches?style=social)](https://github.com/Amengclass/cc-switch-patches/stargazers)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](#-download)
[![Built with](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)
[![Upstream](https://img.shields.io/badge/upstream-farion1231%2Fcc--switch-blueviolet.svg)](https://github.com/farion1231/cc-switch)

[中文](README.md) | English | [Changelog](CHANGELOG.md)

</div>

---

## ✨ What is this

**This repository contains only patches and tooling — no source code, and it cannot be compiled directly.**

The idea: apply these patches on top of the official
[CC Switch](https://github.com/farion1231/cc-switch) source, and you get a complete build
with all of our added features.

```
Official source (some tag)
      │  apply patches
      ▼
Official + our features   ← only this patched tree gets compiled
      │  build
      ▼
  cc-switch-magic.exe
```

> **The official source is never modified** — patches are applied to a copy.
> So whenever upstream releases a new version, we simply re-apply.

## 🎁 What it adds on top of upstream

| Feature | Description |
|---|---|
| 🖥️ **SSH remote-host control plane** | Connect to remote hosts / Docker containers from the local GUI; read & write remote `~/.claude/settings.json`. Covers remote providers / MCP / Prompts / Skills / sessions |
| 🎯 **Target selector** | A combined "Local / Server / Container" pill in the header, with live reachability probing |
| 📡 **Batch apply** | Push one provider to many hosts / containers at once, with per-target progress |
| 🔀 **Route takeover** | Remote traffic goes through a local proxy over an SSH reverse tunnel; per-container DNAT |
| 🔴 **Desktop floating widget** | Always-on desktop ball, hover to expand, auto-collapse at screen edges, live route status |
| 🧩 **OpenClaw support** | MCP / env / tools / agents panels, mirrored for local and remote |

## 🚀 Quick start

### Option 1 — GitHub Actions (recommended, zero local setup)

```
1. Open the repo → Actions → "Build CC Switch Magic"
2. Click "Run workflow" and fill in the inputs:
     version   = official tag (e.g. v3.20.1; empty = the version pinned in base.json)
     platforms = which platforms to build: 全部 (all) / Windows / Linux / macOS (comma-separated)
     make_release = optional; when checked it also creates a Release
3. Click the green "Run workflow" and wait 20–40 minutes
4. Open the run → scroll to Artifacts → download
```

> To verify a single platform, set `platforms` to e.g. `macOS` — the others are skipped, saving time.

Or push a tag to trigger it locally:

```bash
git tag magic-v3.20.1
git push origin magic-v3.20.1
```

### Option 2 — One command locally

```powershell
.\magic.ps1 -Version v3.20.1     # assemble + verify + build → exe
```

Output: `cc-switch-build/src-tauri/target/debug/cc-switch-magic.exe`

### Option 3 — Step by step

Prefer to see each stage? Run the (already verified) scripts one at a time:

```powershell
# 1) Assemble: clone upstream v3.20.1 → overlay → structured merge → apply 11 patches
.\scripts\apply.ps1 -TargetDir <target-dir> -Version v3.20.1

# 2) Verify: 42 feature assertions
.\checks\feature-checks.ps1 -TargetDir <target-dir>

# 3) Build
.\scripts\build.ps1 -TargetDir <target-dir>
```

Internally `apply.ps1` does three things, matching the "upstream is never modified" rule:

| Stage | Action |
|---|---|
| Overlay | Copy the 59 files under `overlay/` into the target tree |
| Structured merge | Merge `structured/` into i18n / config / deps by key |
| Patches | `git apply` the 11 `.patch` files (line-level edits to upstream) |
## 📦 Download

Built automatically by GitHub Actions, named after the upstream convention:

| Platform | Artifact |
|---|---|
| Windows | `CC-Switch-Magic-<version>-Windows-Portable.zip` (portable) / `.msi` / `-Setup.exe` |
| macOS | `CC-Switch-Magic-<version>-macOS.dmg` |
| Linux | `CC-Switch-Magic-<version>-Linux.deb` / `-Linux.AppImage` |

→ [See all build artifacts](https://github.com/Amengclass/cc-switch-patches/actions)

## 🧩 Three-layer patch structure

| Layer | Content | Count | Conflict risk |
|---|---|---|---|
| `overlay/` | Files upstream does **not** have — copied wholesale | 59 | **None** |
| `patches/` | Line-level edits to upstream files, split by topic | 11 | Only if upstream edits the same line |
| `structured/` | i18n / config / deps, merged by key | 7 | **None** |

> **Why split this way**: i18n is the file upstream touches most (79 times in the last
> ~400 commits). Merging by key makes those **conflict-free forever**; new files copied
> wholesale are conflict-free too. Only the line-level edits can conflict — and they are
> split into 11 independent topics.

## ➕ How to add a new feature

**The core idea**: edit *our* source tree, then re-extract the patches.

### Step 1 — Edit the code

Work in your local *our-source* tree (that one = upstream + all current patches):

| What you are adding | Where |
|---|---|
| Frontend pages / components | `src/` |
| Backend commands / logic | `src-tauri/src/` |
| Brand-new files | Just put them where they belong |

**You don't have to decide "overlay or patch"** — the next step works it out automatically.

### Step 2 — Run it locally

```powershell
pnpm install
pnpm tauri dev        # dev mode: frontend hot-reloads, Rust recompiles
```

> Run `Stop-Process -Name cc-switch-magic -Force` first, or the single-instance lock blocks it.

### Step 3 — Re-extract the patch layer

```powershell
.\scripts\repack.ps1 -EditedRepo <our-source-tree>
```

It will scan the tree, diff against upstream, and regenerate `overlay/` + `patches/` + `structured/`.

### Step 4 — Verify

```powershell
.\scripts\verify.ps1 -ReferenceRepo <our-source-tree>
```

You want to see `0 real differences` — proving "upstream + patches == our source".

### Step 5 — Push

```powershell
git add -A
git commit -m "feat: my new feature"
git push
```

### Two tips

**1. Prefer appending over rewriting upstream files**

```diff
# Good: an append — survives upstream edits
+ if (isRemote) { ... }

# Bad: a rewrite — conflicts as soon as upstream touches it
- 10 original lines
+ 15 rewritten lines
```

**2. Route frontend strings through i18n** instead of hard-coding Chinese:

```tsx
t("myFeature.title")   // then add the key to src/i18n/locales/{zh,en,ja,zh-TW}.json
```

## 🛠️ Scripts

| Script | Purpose |
|---|---|
| `magic.ps1` | One command: prepare upstream → assemble → verify → build |
| `scripts/apply.ps1` | Upstream → overlay → structured merge → apply patches |
| `scripts/build.ps1` | Frontend bundle + Rust build → exe |
| `scripts/verify.ps1` | Orchestrates the four verification layers |
| `scripts/repack.ps1` | Regenerate the patch layer after a feature change |
| `scripts/compare-trees.mjs` | Tree diff (JSON/TOML compared semantically) |
| `scripts/_proxy.ps1` | Auto-detects a local git proxy (direct on CI) |
| `checks/feature-checks.ps1` | 42 feature assertions |

## ✅ Verification (so nothing silently disappears)

| Layer | Method | Catches |
|---|---|---|
| **L1 structure** | `cargo check` + `pnpm typecheck` | Structural breakage from upstream changes |
| **L2 logic** | `cargo test` + `vitest` | Behaviour regressions |
| **L3 assertions** | `checks/feature-checks.ps1` (42 checks) | **Features silently dropped** (no compile error) |
| **L4 fidelity** | `scripts/compare-trees.mjs` | File-by-file diff vs the reference source |

`verify.ps1 -ReferenceRepo <our source>` runs all four layers; without `-ReferenceRepo`
it runs L1–L3 only (**use this when upgrading to a new upstream version**).

## ❓ FAQ

<details>
<summary><b>Upstream released a new version — will the patches still apply?</b></summary>

Mostly yes. Measured on upstream v3.20.1 → v3.20.3 (two versions apart):
**8 of 11 patches applied cleanly, 2 conflicted** (touching 3 files, one of them only 2 lines).

When that happens `apply.ps1` reports only the conflicting topic; the rest still apply.
Resolve the conflict, then re-run `repack`.
</details>

<details>
<summary><b>Why is the built exe ~80 MB?</b></summary>

Because it is a **debug** build. `pnpm tauri build` (release) produces an installer of
about **13 MB**.

You can also build release directly:
`cargo build --release --features tauri/custom-protocol`.
</details>

<details>
<summary><b>Build fails with "exe is in use" / os error 5?</b></summary>

Stop the running instance first: `Stop-Process -Name cc-switch-magic -Force`
(Tauri uses a single-instance lock, and linking fails while the exe is in use).
</details>

<details>
<summary><b>Why won't the patches in <code>patches/</code> apply?</b></summary>

Check line endings. **`.patch` files must use LF**; CRLF makes `git apply` fail on every
patch. This repo ships a `.gitattributes` enforcing byte fidelity — **do not delete it**.
</details>

## 📁 Project structure

```text
cc-switch-patches/
├── overlay/                      # Files upstream does not have (59) → copied wholesale
├── patches/                      # Line-level edits to upstream (11 topic patches)
├── structured/                   # i18n / config / deps → merged by key
├── checks/feature-checks.ps1     # 42 feature assertions
├── scripts/                      # assemble / build / verify / repack
├── .github/workflows/build.yml   # Multi-platform automated build
├── magic.ps1                     # One-command entry point
├── base.json                     # Pinned baseline (upstream repo / tag / commit)
└── README.md
```

## 🤝 Contributing

Issues and PRs are welcome. For feature work, follow
[How to add a new feature](#-how-to-add-a-new-feature) above and make sure `verify.ps1`
passes all four layers.

## 📄 License

[MIT](LICENSE)

This project is a patch layer for [farion1231/cc-switch](https://github.com/farion1231/cc-switch) (MIT).