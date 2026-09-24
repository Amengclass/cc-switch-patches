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
  $n = (Get-ChildItem (Join-Path $TargetDir "src-tauri/src/remote") -File -Filter *.rs -ErrorAction SilentlyContinue).Count
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
Check "schema 含 remote_hosts 表" { Contains "src-tauri/src/remote/schema.rs" "remote_hosts" }
Check "schema 含 remote_current_providers 表" { Contains "src-tauri/src/remote/schema.rs" "remote_current_providers" }
Check "schema 含 enabled_openclaw 列" { Contains "src-tauri/src/remote/schema.rs" "enabled_openclaw" }

# ---------- E. 后端：OpenClaw ----------
Check "官方 schema.rs 保留 P2 单点调用" { Contains "src-tauri/src/database/schema.rs" "remote::schema::ensure" }
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
Check "应用名 = CC Switch Magic" { Contains "src-tauri/tauri.conf.json" "CC Switch Magic" }
Check "窗口标题 = CC Switch Magic" { Contains "src-tauri/tauri.windows.conf.json" "CC Switch Magic" }
Check "Cargo.toml 含 macos-private-api 特性（macOS 透明窗口必需）" { Contains "src-tauri/Cargo.toml" "macos-private-api" }
Check "tauri.conf 开启 macOSPrivateApi" { Contains "src-tauri/tauri.conf.json" "macOSPrivateApi" }
Check "窗口尺寸 920x650" { Contains "src-tauri/tauri.windows.conf.json" '"width":\s*920' }

# ---------- J. 远端支持 Mcode（官方 v3.20.4 新增的第十个受管应用） ----------
# 官方 v3.20.4 引入 MiniMax Code（app_type = mcode，配置在 ~/.minimax）。
# 这些断言保证「远端控制面也覆盖了它」不会被后续升级悄悄吃掉。
Check "app_config 含 AppType::Mcode" { Contains "src-tauri/src/app_config.rs" "AppType::Mcode" }
Check "远端 mcode 模块存在" { Exists "src-tauri/src/remote/mcode.rs" }
Check "远端 mcode 已注册到 mod.rs" { Contains "src-tauri/src/remote/mod.rs" "pub mod mcode;" }
Check "远端切换写回支持 mcode" { Contains "src-tauri/src/remote/providers.rs" "apply_mcode_provider_settings" }
Check "远端提示词路径指向 .minimax" { Contains "src-tauri/src/remote/prompt.rs" '\.minimax' }
Check "远端 MCP 路径指向 .minimax" { Contains "src-tauri/src/remote/mcp.rs" '\.minimax' }
Check "远端 skills 路径指向 .minimax" { Contains "src-tauri/src/remote/skill.rs" '\.minimax/skills' }
Check "远端 CLI 探测用 mcode 二进制名" { Contains "src-tauri/src/remote/commands.rs" 'Some\("mcode"\)' }
Check "远端 additive 判定含 mcode" { Contains "src-tauri/src/remote/providers.rs" 'matches!\(' 1 }

# 回归断言：prompt.rs 的兜底分支必须不再静默写 Claude。
# 以前是 `_ => (".claude", "CLAUDE.md")` —— 任何未列出的 app 都会把提示词写进
# Claude 的配置，污染别的应用且不报错。现已改成 Option + 调用方报错。
Check "远端提示词路径不再静默兜底到 claude" {
  $p = Join-Path $TargetDir "src-tauri/src/remote/prompt.rs"
  if (-not (Test-Path $p)) { return @($false, "文件不存在") }
  # 只匹配【代码行】（行首是 _ =>），注释里提到旧写法不算 —— 否则会误报。
  foreach ($line in (Get-Content $p)) {
    if ($line -match '^\s*_ => \("\.claude", "CLAUDE\.md"\)') {
      return @($false, "危险兜底仍在：未识别的 app 会被静默写进 Claude 配置")
    }
  }
  return @($true, "")
}

# 回归断言：mcp.rs 的 read_live_servers 必须覆盖 pi。
# 写入走 pi_config_path、读回却落空 → 导入时静默读不到 pi 的 live 服务器。
Check "远端 MCP read_live_servers 覆盖 pi" {
  $p = Join-Path $TargetDir "src-tauri/src/remote/mcp.rs"
  if (-not (Test-Path $p)) { return @($false, "文件不存在") }
  $t = Get-Content $p -Raw
  if ($t -match '"pi" => read_json_field_map\(fs, &pi_config_path') {
    return @($true, "")
  }
  return @($false, "read_live_servers 缺口 pi 分支")
}

Write-Host "`n===== L3 功能断言 =====" -ForegroundColor Cyan
foreach ($r in $results) {
  if ($r.StartsWith("  [FAIL]")) { Write-Host $r -ForegroundColor Red } else { Write-Host $r -ForegroundColor DarkGray }
}
Write-Host "`n  通过 $pass / 失败 $fail" -ForegroundColor $(if ($fail -eq 0) { "Green" } else { "Red" })
if ($fail -gt 0) { exit 1 } else { exit 0 }
