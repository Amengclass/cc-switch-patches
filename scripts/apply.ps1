# apply.ps1 — 把「官方源码 + 我们的补丁层」组装成一个可构建的源码树
#
# 流程：官方源码 → 套 overlay（整拷）→ 结构化合并（i18n/配置/依赖）→ 打补丁（按主题有序）
#
# 用法示例：
#   .\apply.ps1 -TargetDir D:\build\cc-switch -Version v3.20.1     # 自动拉官方 tag 到 TargetDir
#   .\apply.ps1 -TargetDir D:\build\cc-switch -OfficialDir C:\off  # 用已克隆的官方源码做底
param(
  [Parameter(Mandatory=$true)][string]$TargetDir,
  [string]$Version,
  [string]$OfficialDir,
  [string]$MagicDir
)

$ErrorActionPreference = "Stop"
if (-not $MagicDir) { $MagicDir = Split-Path -Parent (Split-Path -Parent $PSCommandPath) }
$base = Get-Content (Join-Path $MagicDir "base.json") -Raw | ConvertFrom-Json
$upstream = $base.upstream
if (-not $Version) { $Version = $base.base_tag }

Write-Host "=== CC Switch Magic 补丁应用 ===" -ForegroundColor Cyan
Write-Host "  官方: $upstream @ $Version"
Write-Host "  目标: $TargetDir"

# ---------- 1) 准备官方源码 ----------
if ($OfficialDir) {
  Write-Host "`n[1/4] 准备官方基线（来源: $OfficialDir）" -ForegroundColor Cyan
  if (Test-Path $TargetDir) { Remove-Item -LiteralPath $TargetDir -Recurse -Force }
  Copy-Item $OfficialDir $TargetDir -Recurse -Force
  Push-Location $TargetDir
  git checkout -- . 2>$null | Out-Null
  Pop-Location
} else {
  Write-Host "`n[1/4] 克隆官方 $Version" -ForegroundColor Cyan
  if (Test-Path $TargetDir) { Remove-Item -LiteralPath $TargetDir -Recurse -Force }
  git -c http.proxy=http://127.0.0.1:7897 -c https.proxy=http://127.0.0.1:7897 `
      clone --depth 1 --branch $Version $upstream $TargetDir
}

# ---------- 2) 套 overlay ----------
Write-Host "`n[2/4] 套 overlay（整文件覆盖）" -ForegroundColor Cyan
$overlayRoot = Join-Path $MagicDir "overlay"
$overlayCount = 0
Get-ChildItem -Path $overlayRoot -Recurse -File | ForEach-Object {
  $rel = $_.FullName.Substring($overlayRoot.Length).TrimStart('\')
  $dst = Join-Path $TargetDir $rel
  New-Item -ItemType Directory -Force -Path (Split-Path $dst -Parent) | Out-Null
  Copy-Item $_.FullName $dst -Force
  $overlayCount++
}
Write-Host "  覆盖 $overlayCount 个文件"

# ---------- 3) 结构化合并 ----------
Write-Host "`n[3/4] 结构化合并（i18n / 配置 / 依赖）" -ForegroundColor Cyan
node (Join-Path $MagicDir "scripts\merge-structured.mjs") (Join-Path $MagicDir "structured") $TargetDir

# ---------- 3.5) 删除我们不需要的官方文件 ----------
if ($base.deleted_files -and $base.deleted_files.Count -gt 0) {
  foreach ($rel in $base.deleted_files) {
    $p = Join-Path $TargetDir ($rel -replace '/', '\')
    if (Test-Path $p) { Remove-Item -LiteralPath $p -Force; Write-Host "  删除 $rel" }
  }
}

# ---------- 3.6) 若已装 prettier，规范化格式（对齐官方 prettier 配置，避免 format:check 失败）----------
$prettierBin = Join-Path $TargetDir "node_modules\.bin\prettier.cmd"
if (Test-Path $prettierBin) {
  Push-Location $TargetDir
  & $prettierBin --write "src/i18n/locales/*.json" "src-tauri/tauri.conf.json" "src-tauri/tauri.windows.conf.json" 2>&1 | Out-Null
  Pop-Location
  Write-Host "  已用 prettier 规范化结构化文件"
} else {
  Write-Host "  (未装 prettier，跳过格式规范化；语义不受影响)"
}

# ---------- 4) 打补丁 ----------
Write-Host "`n[4/4] 应用补丁（按主题有序）" -ForegroundColor Cyan
Push-Location $TargetDir
$ok = 0; $failed = @()
Get-ChildItem (Join-Path $MagicDir "patches\*.patch") | Sort-Object Name | ForEach-Object {
  $r = git apply --whitespace=nowarn $_.FullName 2>&1
  if ($LASTEXITCODE -eq 0) {
    Write-Host "  [OK]   $($_.Name)" -ForegroundColor Green
    $ok++
  } else {
    Write-Host "  [FAIL] $($_.Name)" -ForegroundColor Red
    Write-Host "         $(($r | Select-Object -First 3) -join "`n         ")" -ForegroundColor DarkRed
    $failed += $_.Name
  }
}
Pop-Location

Write-Host ""
if ($failed.Count -eq 0) {
  Write-Host "=== 完成：$ok 个补丁全部干净应用 ===" -ForegroundColor Green
} else {
  Write-Host "=== 完成：$ok 成功 / $($failed.Count) 失败 ===" -ForegroundColor Yellow
  Write-Host "  冲突补丁：$($failed -join ', ')" -ForegroundColor Yellow
  Write-Host "  提示：冲突只影响上述主题，其余补丁已应用；解冲突后重新生成对应补丁即可。" -ForegroundColor Yellow
  exit 2
}
