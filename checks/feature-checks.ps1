# feature-checks.ps1 — L3 功能断言（防止「功能被静默吃掉」）
#
# 为什么需要：官方代码被 merge/apply 吃掉时**不会报编译错**，只会功能消失。
# 这里逐条写死「我们的功能应该长什么样」，任何一次升级后自动核对。
#
# 用法: .\feature-checks.ps1 -TargetDir <组装后的源码树>
param([Parameter(Mandatory=$true)][string]$TargetDir)

$fail = 0
$pass = 0
$results = @()

function Check($name, [scriptblock]$test) {
  $ok = $false
  $detail = ""
  try {
    $r = & $test
    if ($r -is [array]) { $ok = [bool]$r[0]; $detail = $r[1] } else { $ok = [bool]$r }
  } catch { $detail = $_.Exception.Message }
  if ($ok) { $script:pass++; $script:results += "  [PASS] $name" }
  else { $script:fail++; $script:results += "  [FAIL] $name" + $(if ($detail) { "  ($detail)" } else { "" }) }
}

function Exists($rel) { Test-Path (Join-Path $TargetDir ($rel -replace '/', '\')) }
function Contains($rel, $pattern, $min = 1) {
  $p = Join-Path $TargetDir ($rel -replace '/', '\')
  if (-not (Test-Path $p)) { return @($false, "文件不存在") }
  $n = (Select-String -Path $p -Pattern $pattern | Measure-Object).Count
  if ($n -ge $min) { return @($true, "") } else { return @($false, "命中 $n 次，期望 >= $min") }
}

# ---------- A. overlay 文件完整性 ----------
$overlayRoot = Join-Path (Split-Path $PSScriptRoot -Parent) "overlay"
$overlayFiles = Get-ChildItem -Path $overlayRoot -Recurse -File
Check "overlay 文件齐全 ($($overlayFiles.Count) 个)" {
  $missing = @()
  foreach ($f in $overlayFiles) {
    $rel = $f.FullName.Substring($overlayRoot.Length).TrimStart('\')
    if (-not (Exists $rel)) { $missing += $rel }
  }
  if ($missing.Count -eq 0) { $true } else { @($false, "缺 $($missing.Count) 个: $($missing[0..([Math]::Min(2,$missing.Count-1))] -join ', ')") }
}

# ---------- B. 后端：远程控制面 ----------
Check "后端 remote 模块存在" { Exists "src-tauri/src/remote/commands.rs" }
Check "后端 remote 模块 23 个文件" {
  $n = (Get-ChildItem (Join-Path $TargetDir "src-tauri\src\remote") -File -Filter *.rs -ErrorAction SilentlyContinue).Count
  if ($n -ge 20) { $true } else { @($false, "只有 $n 个 .rs") }
}
Check "lib.rs 注册 remote 命令" { Contains "src-tauri/src/lib.rs" "remote::commands::" 50 }
Check "lib.rs 注册远端 providers 切换命令" { Contains "src-tauri/src/lib.rs" "switch_remote_provider" }
Check "lib.rs 注册批量应用命令" { Contains "src-tauri/src/lib.rs" "broadcast_switch_provider" }
Check "proxy 远端路由模块存在" { Exists "src-tauri/src/proxy/remote_route.rs" }
Check "FileOps 抽象存在" { Exists "src-tauri/src/fsops.rs" }

# ---------- C. 后端：悬浮球 ----------
Check "floating 模块存在" { Exists "src-tauri/src/floating.rs" }
Check "lib.rs 注册悬浮窗钩子" { Contains "src-tauri/src/lib.rs" "floating::" }
Check "悬浮窗三窗口 label 常量" { Contains "src-tauri/src/floating.rs" "BALL_LABEL" }

# ---------- D. 后端：数据库 ----------
Check "schema 含 remote_hosts 表" { Contains "src-tauri/src/database/schema.rs" "remote_hosts" }
Check "schema 含 remote_current_providers 表" { Contains "src-tauri/src/database/schema.rs" "remote_current_providers" }
Check "schema 含 enabled_openclaw 列" { Contains "src-tauri/src/database/schema.rs" "enabled_openclaw" }

# ---------- E. 后端：OpenClaw ----------
Check "openclaw MCP 模块存在" { Exists "src-tauri/src/mcp/openclaw.rs" }
Check "openclaw 配置模块存在" { Exists "src-tauri/src/openclaw_config.rs" }

# ---------- F. 前端：悬浮窗 ----------
Check "floating.html 入口存在" { Exists "src/floating.html" }
Check "floating-main.tsx 存在" { Exists "src/floating-main.tsx" }
Check "悬浮球组件存在" { Exists "src/floating/FloatingBall.tsx" }
Check "悬浮面板组件存在" { Exists "src/floating/FloatingPanel.tsx" }
Check "vite 配置含 floating 入口" { Contains "vite.config.ts" "floating" }

# ---------- G. 前端：远端 UI ----------
Check "目标选择器组件存在" { Exists "src/components/remote/TargetBreadcrumb.tsx" }
Check "远程主机面板存在" { Exists "src/components/remote/RemoteHostsPanel.tsx" }
Check "批量应用面板存在" { Exists "src/components/remote/BatchApplyPanel.tsx" }
Check "远端 API 层存在" { Exists "src/lib/api/remote.ts" }
Check "远端类型定义存在" { Exists "src/types/remote.ts" }
Check "悬浮窗设置面板存在" { Exists "src/components/settings/FloatingWindowSettings.tsx" }
Check "App.tsx 挂载目标选择器" { Contains "src/App.tsx" "TargetBreadcrumb" }
Check "App.tsx 挂载远程主机面板" { Contains "src/App.tsx" "RemoteHostsPanel" }

# ---------- H. i18n ----------
Check "i18n 含远端功能开关文案 (zh)" { Contains "src/i18n/locales/zh.json" "featureEnabled" }
Check "i18n 含悬浮窗文案 (zh)" { Contains "src/i18n/locales/zh.json" "floating" }
Check "i18n 四语齐全" {
  $miss = @()
  foreach ($l in @("zh", "en", "ja", "zh-TW")) { if (-not (Exists "src/i18n/locales/$l.json")) { $miss += $l } }
  if ($miss.Count -eq 0) { $true } else { @($false, "缺 $($miss -join ',')") }
}

# ---------- I. 结构化合并结果 ----------
Check "Cargo.toml 含 russh 依赖" { Contains "src-tauri/Cargo.toml" "russh" }
Check "Cargo.toml 含 russh-sftp 依赖" { Contains "src-tauri/Cargo.toml" "russh-sftp" }
Check "Cargo.toml 含 keyring 依赖" { Contains "src-tauri/Cargo.toml" "keyring" }
Check "Cargo.toml 含 tar 依赖" { Contains "src-tauri/Cargo.toml" "tar\s*=" }
Check "Cargo.toml features 含 Win32_Security_Cryptography" { Contains "src-tauri/Cargo.toml" "Win32_Security_Cryptography" }
Check "tauri.conf createUpdaterArtifacts=false" { Contains "src-tauri/tauri.conf.json" '"createUpdaterArtifacts":\s*false' }
Check "窗口尺寸 920x650" { Contains "src-tauri/tauri.windows.conf.json" '"width":\s*920' }

Write-Host "`n===== L3 功能断言 =====" -ForegroundColor Cyan
foreach ($r in $results) {
  if ($r.StartsWith("  [FAIL]")) { Write-Host $r -ForegroundColor Red } else { Write-Host $r -ForegroundColor DarkGray }
}
Write-Host "`n  通过 $pass / 失败 $fail" -ForegroundColor $(if ($fail -eq 0) { "Green" } else { "Red" })
if ($fail -gt 0) { exit 1 } else { exit 0 }
