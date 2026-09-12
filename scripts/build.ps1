# build.ps1 — 构建组装后的源码树，产出 exe
#
# 用法: .\build.ps1 -TargetDir <组装后的源码树> [-Release]
param(
  [Parameter(Mandatory=$true)][string]$TargetDir,
  [switch]$Release
)

$ErrorActionPreference = "Stop"

Write-Host "=== 构建 CC Switch Magic ===" -ForegroundColor Cyan
Write-Host "  源码: $TargetDir"

# 1) 先停掉运行中的实例（否则 exe 被锁 → os error 5 / LNK1105）
$running = Get-Process -Name cc-switch -ErrorAction SilentlyContinue
if ($running) {
  Write-Host "`n[1/3] 停止运行中的 cc-switch ($($running.Count) 个进程)" -ForegroundColor Cyan
  Stop-Process -Name cc-switch -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
} else {
  Write-Host "`n[1/3] 无运行中的 cc-switch 实例"
}

Push-Location $TargetDir

# 2) 前端：Tauri frontendDist 指向 ../dist，cargo 不会自动编译前端，必须先打包
Write-Host "`n[2/3] 构建前端 (pnpm build:renderer)" -ForegroundColor Cyan
if (-not (Test-Path "node_modules")) {
  Write-Host "  首次：pnpm install"
  pnpm install
}
pnpm build:renderer
if ($LASTEXITCODE -ne 0) { Pop-Location; throw "前端构建失败" }

# 3) Rust
Write-Host "`n[3/3] 构建 Rust" -ForegroundColor Cyan
Push-Location "src-tauri"
# 规避火绒 sysdiag 文件锁（LNK1105）：并发降为 2、关调试符号、链接加 /DEBUG:NONE
$env:CARGO_BUILD_JOBS = "2"
if (-not $env:RUSTFLAGS) { $env:RUSTFLAGS = "-C link-arg=/DEBUG:NONE" }
if (-not $env:CARGO_PROFILE_DEV_DEBUG) { $env:CARGO_PROFILE_DEV_DEBUG = "0" }

$sw = [System.Diagnostics.Stopwatch]::StartNew()
if ($Release) {
  # 官方发布用：含前端嵌入（custom-protocol）
  cargo build --release --features tauri/custom-protocol
} else {
  cargo build --features tauri/custom-protocol
}
$code = $LASTEXITCODE
$sw.Stop()
Pop-Location
Pop-Location

if ($code -ne 0) { throw "Rust 构建失败（退出码 $code）" }

$mode = if ($Release) { "release" } else { "debug" }
$exe = Join-Path $TargetDir "src-tauri\target\$mode\cc-switch.exe"
Write-Host "`n=== 构建完成（耗时 $([int]$sw.Elapsed.TotalSeconds)s）===" -ForegroundColor Green
if (Test-Path $exe) { Write-Host "  产物: $exe" -ForegroundColor Green } else { Write-Host "  未找到预期产物: $exe" -ForegroundColor Yellow }
