# apply.ps1 — 把「官方源码 + 我们的补丁层」组装成一个可构建的源码树
#
# 流程：官方源码 → 套 overlay（整拷）→ 结构化合并（i18n/配置/依赖）→ 打补丁（按主题有序）→ 锚点重放（加性改动）
#
# 用法示例：
#   .\apply.ps1 -TargetDir D:\build\cc-switch -Version v3.20.1     # 自动拉官方 tag 到 TargetDir
#   .\apply.ps1 -TargetDir D:\build\cc-switch -OfficialDir C:\off  # 用已克隆的官方源码做底
#   .\apply.ps1 -TargetDir C:\my-fork -SkipPrepare                 # 组装进「已有的官方检出」（保留 .git）
param(
  [Parameter(Mandatory=$true)][string]$TargetDir,
  [string]$Version,
  [string]$OfficialDir,
  [string]$MagicDir,
  # 跳过第 1 步「准备官方源码」——目标目录已经是官方该版本的检出（比如你自己的 fork 分支）。
  # 不加这个开关时，第 1 步会**删掉整个目录**（含 .git！），所以往 fork 里组装必须带上它。
  [switch]$SkipPrepare
)

$ErrorActionPreference = "Stop"
if (-not $MagicDir) { $MagicDir = Split-Path -Parent (Split-Path -Parent $PSCommandPath) }
. (Join-Path $MagicDir "scripts/_proxy.ps1")
$gitProxyArgs = Get-GitProxyArgs
$base = Get-Content (Join-Path $MagicDir "base.json") -Raw | ConvertFrom-Json
$upstream = $base.upstream
if (-not $Version) { $Version = $base.base_tag }

Write-Host "=== CC Switch Magic 补丁应用 ===" -ForegroundColor Cyan
Write-Host "  官方: $upstream @ $Version"
Write-Host "  目标: $TargetDir"

# ---------- 1) 准备官方源码 ----------
if ($SkipPrepare) {
  Write-Host "`n[1/4] 跳过准备官方基线（-SkipPrepare；目标目录须已是官方该版本的检出）" -ForegroundColor Cyan
  Push-Location $TargetDir
  $cur = git rev-parse --abbrev-ref HEAD 2>$null
  Write-Host "  当前检出: $cur  $(git describe --tags --always 2>$null)"
  Pop-Location
  if ($Version) {
    # 只是提醒：不强制切换，避免把用户的 fork 分支弄乱
    $atTag = git -C $TargetDir describe --tags --exact-match 2>$null
    if ($atTag -and $atTag -ne $Version) {
      Write-Host "  [!] 目标目录当前在 $atTag，但你指定了 $Version —— 请自行确认基线一致" -ForegroundColor Yellow
    }
  }
} elseif ($OfficialDir) {
  Write-Host "`n[1/4] 准备官方基线（来源: $OfficialDir）" -ForegroundColor Cyan
  if (Test-Path $TargetDir) { Remove-Item -LiteralPath $TargetDir -Recurse -Force }
  Copy-Item $OfficialDir $TargetDir -Recurse -Force
  Push-Location $TargetDir
  if ($Version) {
    # 官方仓可能停在 main（= 最新 tag），按 -Version 切到目标 tag
    git checkout -q $Version 2>&1 | Out-Null
    if ($LASTEXITCODE -eq 0) { Write-Host "  已切换到官方 $Version" }
    else { Write-Host "  [!] 切换到 $Version 失败，沿用当前检出（可能是非 git 目录）" -ForegroundColor Yellow; git checkout -- . 2>$null | Out-Null }
  } else {
    git checkout -- . 2>$null | Out-Null
    Write-Host "  (未指定 -Version，沿用来源目录当前检出: $(git rev-parse --abbrev-ref HEAD 2>$null) $(git describe --tags --always 2>$null))"
  }
  Pop-Location
} else {
  Write-Host "`n[1/4] 克隆官方 $Version" -ForegroundColor Cyan
  if (Test-Path $TargetDir) { Remove-Item -LiteralPath $TargetDir -Recurse -Force }
  git @gitProxyArgs `
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
node (Join-Path $MagicDir "scripts/merge-structured.mjs") (Join-Path $MagicDir "structured") $TargetDir

# ---------- 3.5) 删除我们不需要的官方文件 ----------
if ($base.deleted_files -and $base.deleted_files.Count -gt 0) {
  foreach ($rel in $base.deleted_files) {
    $p = Join-Path $TargetDir ($rel -replace '/', '\')
    if (Test-Path $p) { Remove-Item -LiteralPath $p -Force; Write-Host "  删除 $rel" }
  }
}

# ---------- 3.6) 若已装 prettier，规范化格式（对齐官方 prettier 配置，避免 format:check 失败）----------
# prettier 可执行文件在 Windows 是 .cmd，Linux/macOS 无后缀
$prettierRel = "node_modules/.bin/prettier.cmd"
if (-not ($IsWindows -or $env:OS -eq "Windows_NT")) { $prettierRel = "node_modules/.bin/prettier" }
$prettierBin = Join-Path $TargetDir $prettierRel
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

# 由「锚点」接管的文件：它们的改动只依赖符号锚点、与官方版本无关，
# 统一交给第 5 步锚点重放（版本无关 + 幂等），不再走行补丁。
$anchorFiles = @{}
Get-ChildItem (Join-Path $MagicDir "anchors\*.json") -ErrorAction SilentlyContinue | ForEach-Object {
  $spec = Get-Content $_.FullName -Raw | ConvertFrom-Json
  if ($spec.file) { $anchorFiles[$spec.file] = $_.Name }
}

$ok = 0; $anchored = 0; $failedFiles = @(); $failedPatches = @{}
Get-ChildItem (Join-Path $MagicDir "patches\*.patch") | Sort-Object Name | ForEach-Object {
  $patch = $_.FullName
  # 逐个文件应用：git apply 默认对整份补丁原子生效，一个文件冲突会连累同主题其它几十个文件。
  # 用 --include 拆成按文件应用后，冲突被隔离在单个文件里。
  $paths = Select-String -Path $patch -Pattern '^\+\+\+ b/(.+)$' |
             ForEach-Object { $_.Matches[0].Groups[1].Value } | Sort-Object -Unique
  $patchFail = @()
  foreach ($p in $paths) {
    if ($anchorFiles.ContainsKey($p)) {
      $anchored++
      continue
    }
    # 必须显式关掉 autocrlf：目标目录不带 .git 时会退回全局设置（Windows 上常是 true），
    # git apply 会把补丁里的 LF 全部写成 CRLF —— 补丁层是字节保真的，不能让 git 改换行。
    $r = git -c core.autocrlf=false apply --include="$p" --whitespace=nowarn $patch 2>&1
    if ($LASTEXITCODE -eq 0) {
      $ok++
    } else {
      $patchFail += $p
      $failedFiles += $p
      Write-Host "  [FAIL] $p" -ForegroundColor Red
      Write-Host "         $(($r | Select-Object -First 2) -join "`n         ")" -ForegroundColor DarkRed
    }
  }
  if ($patchFail.Count -eq 0) {
    Write-Host "  [OK]   $($_.Name)  ($($paths.Count) 文件)" -ForegroundColor Green
  } else {
    $failedPatches[$_.Name] = $patchFail
    Write-Host "  [PART] $($_.Name)  $($paths.Count - $patchFail.Count)/$($paths.Count) 文件成功" -ForegroundColor Yellow
  }
}
Pop-Location

# ---------- 5) 锚点重放（加性改动的抗重构形态）----------
# 官方重构会让行补丁的上下文失配 → 那些补丁整个 FAIL。但我们的很多改动是**加性**的
# （加 pub(crate)、加结构体字段、加一行调用、加个菜单项）—— 只依赖锚点还在，不依赖上下文。
# 锚点重放是幂等的：行补丁已经改过的地方会自动跳过，所以可以无条件跑在 patches 之后。
Write-Host "`n[5/5] 锚点重放（加性改动）" -ForegroundColor Cyan
node (Join-Path $MagicDir "scripts/replay-anchors.mjs") $TargetDir
$anchorFailed = ($LASTEXITCODE -ne 0)
if ($anchorFailed) {
  Write-Host "  [!] 有锚点未命中 —— 官方大概率动了锚点所在位置" -ForegroundColor Yellow
  Write-Host "      正解：按锚点人工重贴（禁止整文件覆盖，那会丢掉官方更新）" -ForegroundColor Yellow
}

Write-Host ""
if ($failedFiles.Count -eq 0 -and -not $anchorFailed) {
  Write-Host "=== 完成：行补丁 $ok 个文件 + 锚点接管 $anchored 个文件，全部命中 ===" -ForegroundColor Green
} else {
  if ($anchored -gt 0) {
    Write-Host "  （另有 $anchored 个文件由锚点接管，见第 5 步）" -ForegroundColor DarkGray
  }
  if ($failedFiles.Count -gt 0) {
    Write-Host "=== 完成：$ok 个文件成功 / $($failedFiles.Count) 个冲突 ===" -ForegroundColor Yellow
    Write-Host "  冲突文件：" -ForegroundColor Yellow
    $failedFiles | Sort-Object -Unique | ForEach-Object { Write-Host "    - $_" -ForegroundColor Yellow }
  }
  Write-Host "  提示：冲突已隔离在单个文件，同主题其它文件不受影响。" -ForegroundColor Yellow
  Write-Host "  下一步：不要整文件覆盖！" -ForegroundColor Yellow
  Write-Host "    1) 加性改动  → 写进 anchors/*.json，锚点重放自动搞定（本次锚点已跑）" -ForegroundColor Yellow
  Write-Host "    2) 重构改动  → 用 git merge-file 做三方合并（base=官方旧版, ours=我们, theirs=官方新版）" -ForegroundColor Yellow
  Write-Host "    3) 跑 scripts/check-upstream-sync.mjs 证明官方更新零丢失" -ForegroundColor Yellow
  exit 2
}
