# repack.ps1 — 本地改完功能后，重新生成补丁层
#
# 场景：你在一个「已组装」的源码树里改了我们的功能（新增文件 / 改官方文件 / 改 i18n）。
# 本脚本把该树作为「我们的当前状态」，重新生成 overlay + patches + structured。
#
# 用法: .\repack.ps1 -EditedRepo <改过的源码树> [-OfficialDir <官方源码>] [-Version v3.20.1]
param(
  [Parameter(Mandatory=$true)][string]$EditedRepo,
  [string]$OfficialDir,
  [string]$Version,
  [string]$MagicDir
)

$ErrorActionPreference = "Stop"
if (-not $MagicDir) { $MagicDir = Split-Path -Parent (Split-Path -Parent $PSCommandPath) }
$base = Get-Content (Join-Path $MagicDir "base.json") -Raw | ConvertFrom-Json
if (-not $Version) { $Version = $base.base_tag }

# 官方源码：给了就用，没给就从 base.json 重新克隆
if (-not $OfficialDir) {
  $OfficialDir = Join-Path $MagicDir "tmp-official"
  if (Test-Path $OfficialDir) { Remove-Item -LiteralPath $OfficialDir -Recurse -Force }
  Write-Host "克隆官方 $Version 到 $OfficialDir" -ForegroundColor Cyan
  git -c http.proxy=http://127.0.0.1:7897 -c https.proxy=http://127.0.0.1:7897 `
      clone --depth 1 --branch $Version $base.upstream $OfficialDir
}

Write-Host "=== 重新打包补丁层 ===" -ForegroundColor Magenta
Write-Host "  官方: $OfficialDir"
Write-Host "  我们: $EditedRepo"

# 1) 重建 overlay（官方没有的文件 = 纯新增）
Write-Host "`n[1/3] 重建 overlay" -ForegroundColor Cyan
$overlayRoot = Join-Path $MagicDir "overlay"
if (Test-Path $overlayRoot) { Remove-Item -LiteralPath $overlayRoot -Recurse -Force }
New-Item -ItemType Directory -Force -Path $overlayRoot | Out-Null

$skip = '\\\.git\\|\\node_modules\\|\\target\\|\\dist\\|\\\.vite\\|\\tmp-official\\'
$n = 0
Get-ChildItem -Path $EditedRepo -Recurse -File | Where-Object { $_.FullName -notmatch $skip } | ForEach-Object {
  $rel = $_.FullName.Substring($EditedRepo.Length).TrimStart('\')
  $officialPath = Join-Path $OfficialDir $rel
  if (-not (Test-Path $officialPath)) {
    $dst = Join-Path $overlayRoot $rel
    New-Item -ItemType Directory -Force -Path (Split-Path $dst -Parent) | Out-Null
    Copy-Item $_.FullName $dst -Force
    $n++
  }
}
Write-Host "  overlay: $n 个新增文件"

# 2) 重建补丁
Write-Host "`n[2/3] 重建补丁" -ForegroundColor Cyan
& (Join-Path $MagicDir "scripts\gen-patches.ps1") -OfficialDir $OfficialDir -OurDir $EditedRepo -OutDir (Join-Path $MagicDir "patches")

# 3) 重建结构化载荷
Write-Host "`n[3/3] 重建结构化载荷" -ForegroundColor Cyan
node (Join-Path $MagicDir "scripts\gen-structured.mjs") $OfficialDir $EditedRepo (Join-Path $MagicDir "structured")

# 更新 base.json 计数
$base.overlay_count = (Get-ChildItem $overlayRoot -Recurse -File).Count
$base.patch_count = (Get-ChildItem (Join-Path $MagicDir "patches") -Filter *.patch).Count
$base | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 (Join-Path $MagicDir "base.json")

Write-Host "`n=== 重新打包完成 ===" -ForegroundColor Green
Write-Host "  建议接着跑: .\verify.ps1 -ReferenceRepo <改过的源码树>" -ForegroundColor Green
