# magic.ps1 —— 一键：组装 + 验证 + 编译
#
# 用法（在 cc-switch-patches 目录下）：
#   .\magic.ps1                    # 用官方【最新版】重新组装 + 编译出 exe
#   .\magic.ps1 -Version v3.20.1   # 指定官方某个版本
#   .\magic.ps1 -SkipBuild         # 只组装+验证，不编译（快，用来查补丁打不打得进）
#   .\magic.ps1 -ForceDownload     # 强制重新克隆官方源码
param(
  [string]$Version,
  [string]$TargetDir,
  [string]$OfficialDir,
  [switch]$SkipBuild,
  [switch]$ForceDownload
)

$ErrorActionPreference = "Stop"
$MagicDir = Split-Path -Parent $PSCommandPath
$CcwRoot  = Split-Path -Parent $MagicDir
if (-not $TargetDir)   { $TargetDir   = Join-Path $CcwRoot "cc-switch-build" }
if (-not $OfficialDir) { $OfficialDir = Join-Path $CcwRoot "cc-switch-official" }
. (Join-Path $MagicDir "scripts/_proxy.ps1")
$gitProxyArgs = Get-GitProxyArgs

function Step($n, $t) { Write-Host "`n[$n] $t" -ForegroundColor Cyan }

Write-Host "===================================================" -ForegroundColor Magenta
Write-Host "  CC Switch Magic 一键构建" -ForegroundColor Magenta
Write-Host "===================================================" -ForegroundColor Magenta

# ---------- 0) 准备官方源码 ----------
Step "0/3" "准备官方源码"
if ($ForceDownload -or -not (Test-Path $OfficialDir)) {
  Write-Host "  克隆官方仓库 → $OfficialDir"
  git @gitProxyArgs `
      clone https://github.com/farion1231/cc-switch.git $OfficialDir
} else {
  Write-Host "  已有: $OfficialDir"
  # 顺手同步一下 tag（失败不阻断）
  Write-Host "  同步官方 tag…"
  git -C $OfficialDir @gitProxyArgs `
      fetch --tags 2>&1 | Select-Object -Last 2
}

# 没指定版本 → 用官方最新 tag
if (-not $Version) {
  $Version = (git -C $OfficialDir tag --sort=-v:refname | Select-Object -First 1)
  if (-not $Version) { throw "官方仓库里没有 tag，请用 -Version 显式指定" }
}
Write-Host "  目标官方版本: $Version" -ForegroundColor Yellow

# ---------- 1) 组装（打补丁） ----------
Step "1/3" "组装：官方源码 + 补丁 → $TargetDir"
& (Join-Path $MagicDir "scripts/apply.ps1") -TargetDir $TargetDir -OfficialDir $OfficialDir -Version $Version
if ($LASTEXITCODE -ne 0) {
  Write-Host "`n✗ 补丁存在冲突，组装未完成。请解冲突后重新运行。" -ForegroundColor Red
  Write-Host "  冲突只影响报错的那些主题，其余补丁已应用。" -ForegroundColor Yellow
  exit 2
}

# ---------- 2) 验证（功能断言） ----------
Step "2/3" "验证：功能断言"
& (Join-Path $MagicDir "checks/feature-checks.ps1") -TargetDir $TargetDir
if ($LASTEXITCODE -ne 0) {
  Write-Host "`n✗ 功能断言未通过，请检查上面的 [FAIL] 项。" -ForegroundColor Red
  exit 3
}

if ($SkipBuild) {
  Write-Host "`n✓ 组装 + 验证完成（-SkipBuild：已跳过编译）" -ForegroundColor Green
  Write-Host "  组装结果: $TargetDir" -ForegroundColor Green
  exit 0
}

# ---------- 3) 编译 ----------
Step "3/3" "编译：前端 + Rust → exe"
& (Join-Path $MagicDir "scripts/build.ps1") -TargetDir $TargetDir
if ($LASTEXITCODE -ne 0) {
  Write-Host "`n✗ 编译失败，请看上面的错误。" -ForegroundColor Red
  exit 4
}

$exe = (Get-ChildItem (Join-Path $TargetDir "src-tauri/target/debug") -Filter *.exe -ErrorAction SilentlyContinue | Select-Object -First 1).FullName
Write-Host "`n===================================================" -ForegroundColor Green
Write-Host "  ✓ 全部完成" -ForegroundColor Green
Write-Host "===================================================" -ForegroundColor Green
Write-Host "  官方版本: $Version"
Write-Host "  exe:      $exe" -ForegroundColor Green
Write-Host "  双击即可运行。" -ForegroundColor Green