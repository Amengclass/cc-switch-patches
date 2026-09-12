# verify.ps1 — 四层验证编排（升级后跑一次，确认「功能一个都没丢」）
#
#   L1 结构: 组装产物能否编译（可选，耗时，加 -WithBuild 开启）
#   L2 逻辑: 单元测试（可选，加 -WithBuild 开启）
#   L3 断言: 功能清单逐条核对（checks/feature-checks.ps1）
#   L4 还原: 组装树 vs 参考树（我们已知可用的仓库）逐文件比对
#
# 用法:
#   .\verify.ps1 -ReferenceRepo <我们的仓库>                 # L3+L4（推荐，秒级）
#   .\verify.ps1 -ReferenceRepo <我们的仓库> -WithBuild      # 追加 L1+L2（分钟级）
param(
  [Parameter(Mandatory=$true)][string]$ReferenceRepo,
  [string]$Version,
  [string]$OfficialDir,
  [string]$WorkDir = "$env:TEMP\cc-switch-verify",
  [switch]$WithBuild
)

$ErrorActionPreference = "Stop"
$MagicDir = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$exitCode = 0

Write-Host "========== CC Switch Magic 验证 ==========" -ForegroundColor Magenta

# ---------- 组装 ----------
if (Test-Path $WorkDir) { Remove-Item -LiteralPath $WorkDir -Recurse -Force }
$applyParams = @{ TargetDir = $WorkDir; MagicDir = $MagicDir }
if ($OfficialDir) { $applyParams.OfficialDir = $OfficialDir }
if ($Version) { $applyParams.Version = $Version }
& "$MagicDir\scripts\apply.ps1" @applyParams
if ($LASTEXITCODE -ne 0) {
  Write-Host "`n[!] 补丁应用存在冲突 —— 升级未完成" -ForegroundColor Yellow
  $exitCode = 2
}

# ---------- L4 还原比对 ----------
Write-Host "`n----- L4: 组装树 vs 参考树 -----" -ForegroundColor Cyan
node "$MagicDir\scripts\compare-trees.mjs" $ReferenceRepo $WorkDir
if ($LASTEXITCODE -ne 0) {
  Write-Host "[!] L4 未通过：组装树与参考树存在真实差异" -ForegroundColor Yellow
  if ($exitCode -eq 0) { $exitCode = 1 }
}

# ---------- L3 功能断言 ----------
Write-Host "`n----- L3: 功能断言 -----" -ForegroundColor Cyan
& "$MagicDir\checks\feature-checks.ps1" -TargetDir $WorkDir
if ($LASTEXITCODE -ne 0) {
  Write-Host "[!] L3 未通过：有功能断言失败" -ForegroundColor Yellow
  if ($exitCode -eq 0) { $exitCode = 1 }
}

# ---------- L1/L2 编译与测试（可选）----------
if ($WithBuild) {
  Write-Host "`n----- L1: 结构（cargo check + typecheck）-----" -ForegroundColor Cyan
  Push-Location $WorkDir
  if (-not (Test-Path "node_modules")) { pnpm install --frozen-lockfile }
  pnpm typecheck
  if ($LASTEXITCODE -ne 0) { Write-Host "[!] typecheck 失败" -ForegroundColor Yellow; $exitCode = 1 }
  Push-Location "src-tauri"
  cargo check
  if ($LASTEXITCODE -ne 0) { Write-Host "[!] cargo check 失败" -ForegroundColor Yellow; $exitCode = 1 }
  Pop-Location

  Write-Host "`n----- L2: 逻辑（单元测试）-----" -ForegroundColor Cyan
  pnpm test:unit
  if ($LASTEXITCODE -ne 0) { Write-Host "[!] 前端单测失败" -ForegroundColor Yellow; $exitCode = 1 }
  Push-Location "src-tauri"
  cargo test
  if ($LASTEXITCODE -ne 0) { Write-Host "[!] Rust 测试失败" -ForegroundColor Yellow; $exitCode = 1 }
  Pop-Location
  Pop-Location
}

Write-Host "`n========== 验证结束（退出码 $exitCode）==========" -ForegroundColor Magenta
if ($exitCode -eq 0) { Write-Host "  全部通过：结构 / 功能断言 / 还原比对 一致" -ForegroundColor Green }
exit $exitCode
