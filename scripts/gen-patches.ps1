# gen-patches.ps1 — 从「官方 v3.20.1 + 我们的文件」生成按主题拆分的补丁
# 用法: .\gen-patches.ps1 -OfficialDir <官方源码目录> -OurDir <我们的仓库> -OutDir <输出 patches 目录>
param(
  [Parameter(Mandatory=$true)][string]$OfficialDir,
  [Parameter(Mandatory=$true)][string]$OurDir,
  [Parameter(Mandatory=$true)][string]$OutDir,
  [string]$BuildDir = "$env:TEMP\cc-switch-magic-build"
)

$ErrorActionPreference = "Stop"

# --- 结构化处理的文件（不走行级补丁，由 structured/ 合并脚本负责）---
$structured = @(
  "src/i18n/locales/zh.json",
  "src/i18n/locales/en.json",
  "src/i18n/locales/ja.json",
  "src/i18n/locales/zh-TW.json",
  "src-tauri/Cargo.lock",
  "src-tauri/tauri.conf.json",
  "src-tauri/tauri.windows.conf.json"
)

# --- 主题 -> 路径前缀/精确路径 的映射（顺序 = 应用顺序）---
$topics = [ordered]@{
  "0100-rust-lib"        = @("src-tauri/src/lib.rs")
  "0110-rust-db"         = @("src-tauri/src/database/")
  "0120-rust-adaptations"= @("src-tauri/src/")
  "0130-rust-tests"      = @("src-tauri/tests/")
  "0200-ts-app"          = @("src/App.tsx")
  "0210-ts-hooks-api"    = @("src/hooks/", "src/lib/")
  "0220-ts-panels"       = @("src/components/")
  "0230-ts-misc"         = @("src/index.css", "src/types.ts", "src/config/", "src/contexts/", "vite.config.ts")
  "0240-ts-tests"        = @("tests/")
  "0310-rust-cargo"     = @("src-tauri/Cargo.toml")
  "0300-misc"            = @(".gitignore", "pnpm-workspace.yaml")
}

# overlay 路径（这些是纯新增，不进补丁）
$overlayPrefixes = @(
  "src-tauri/src/remote/", "src-tauri/sqlite-helper/", "src-tauri/resources/sqlite-helper",
  "src/floating/", "src/floating-main.tsx", "src/floating.html",
  "src/components/remote/", "src-tauri/src/floating.rs", "src-tauri/src/fsops.rs"
)

Write-Host "== 1) 建立官方基线工作区 ==" -ForegroundColor Cyan
if (Test-Path $BuildDir) { Remove-Item $BuildDir -Recurse -Force }
Copy-Item $OfficialDir $BuildDir -Recurse -Force
# 清理官方 clone 的 .git 状态噪声（保留 .git 以便 git diff）
Push-Location $BuildDir
git checkout -- . 2>$null | Out-Null

Write-Host "== 2) 覆盖我们的文件 ==" -ForegroundColor Cyan
$changed = @()
Get-ChildItem -Path $OurDir -Recurse -File |
  Where-Object { $_.FullName -notmatch '\\\.git\\' -and $_.FullName -notmatch '\\node_modules\\' -and $_.FullName -notmatch '\\target\\' -and $_.FullName -notmatch '\\dist\\' -and $_.FullName -notmatch '\\\.claude\\' } |
  ForEach-Object {
    $rel = $_.FullName.Substring($OurDir.Length).TrimStart('\') -replace '\\','/'
    $changed += $rel
  }

# 只覆盖「官方已有」的文件（新增文件由 overlay 负责）；同时排除 overlay 路径
$modified = $changed | Where-Object { (Test-Path (Join-Path $BuildDir $_)) }
foreach ($rel in $modified) {
  $src = Join-Path $OurDir ($rel -replace '/','\')
  $dst = Join-Path $BuildDir ($rel -replace '/','\')
  New-Item -ItemType Directory -Force -Path (Split-Path $dst -Parent) | Out-Null
  Copy-Item $src $dst -Force
}
Write-Host "   覆盖 $($modified.Count) 个文件"

Write-Host "== 3) 按主题生成补丁 ==" -ForegroundColor Cyan
if (!(Test-Path $OutDir)) { New-Item -ItemType Directory -Force -Path $OutDir | Out-Null }
$assigned = @{}
foreach ($topic in $topics.Keys) {
  $paths = @()
  foreach ($p in $topics[$topic]) {
    if ($p.EndsWith("/")) {
      $paths += ($modified | Where-Object { $_ -like "$p*" })
    } else {
      $paths += ($modified | Where-Object { $_ -eq $p })
    }
  }
  # 排除：结构化文件 + 已被前面主题认领的文件（保证主题互斥，否则同一文件进两个补丁 → 二次应用冲突）
  $paths = $paths | Where-Object { $structured -notcontains $_ -and -not $assigned.ContainsKey($_) } | Sort-Object -Unique
  # 记录已分配
  foreach ($p in $paths) { $assigned[$p] = $true }
  if ($paths.Count -eq 0) { Write-Host "   [跳过] $topic (无文件)"; continue }

  $patchFile = Join-Path $OutDir "$topic.patch"
  # 关键：用 git 自己写文件（--output），不经 PowerShell 文本管道。
  # 否则非 UTF-8 字节（如 .gitignore 里的 GBK 中文）会被 PowerShell 按 UTF-8 解码成 U+FFFD 而损坏。
  if (Test-Path $patchFile) { Remove-Item $patchFile -Force }
  git diff --no-color --output="$patchFile" -- $paths
  if ((Test-Path $patchFile) -and (Get-Item $patchFile).Length -gt 0) {
    $lineCount = (Get-Content $patchFile | Measure-Object -Line).Lines
    Write-Host "   [生成] $topic.patch  ($($paths.Count) 文件, $lineCount 行)"
  } else {
    if (Test-Path $patchFile) { Remove-Item $patchFile -Force }
    Write-Host "   [空]   $topic (无差异)"
  }
}

Write-Host "== 4) 未分配的「实际改动」文件（需人工归类的漏网）==" -ForegroundColor Yellow
$reallyChanged = git diff --name-only
$leftover = $reallyChanged | Where-Object { -not $assigned.ContainsKey($_) -and $structured -notcontains $_ } | Sort-Object
Write-Host "   实际改动文件总数: $($reallyChanged.Count)（其中结构化处理 $($structured.Count) 个）"
if ($leftover) { foreach ($l in $leftover) { Write-Host "   [漏网] $l" } } else { Write-Host "   (无 —— 所有改动文件都已归类)" }

Pop-Location
Write-Host "== 完成 ==" -ForegroundColor Green
