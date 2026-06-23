# Push ./anchor_config.json to EVERY connected Quest that has LBEStreaming installed.
#
# 用法 / Usage:
#   .\push_anchor_config.ps1        # 把当前目录的 anchor_config.json 推到所有已装 LBEStreaming 的头显
#
# 把本场地的 golden anchor_config.json 放在当前目录（通常来自 pull_anchor_config.ps1），
# 连上要配置的头显（可多台同时），运行本脚本批量分发。推送后需重启头显里的 app 才会读取新配置。

$ErrorActionPreference = "Stop"
$PKG = "alvr.client.lbestreaming"
$REMOTE_DIR = "/sdcard/Android/data/$PKG/files"
$REMOTE = "$REMOTE_DIR/anchor_config.json"
$LOCAL = ".\anchor_config.json"

function Resolve-Adb {
    $candidates = @(
        (Join-Path $PSScriptRoot "platform-tools\adb.exe"),
        (Join-Path $PSScriptRoot "..\build\alvr_streamer_windows\platform-tools\adb.exe")
    )
    if ($env:ANDROID_HOME) {
        $candidates += (Join-Path $env:ANDROID_HOME "platform-tools\adb.exe")
    }
    foreach ($c in $candidates) { if ($c -and (Test-Path $c)) { return (Resolve-Path $c).Path } }
    $cmd = Get-Command adb -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    throw "adb not found. 把 platform-tools 放到本脚本旁边，或将 adb 加入 PATH。"
}

function Get-ConnectedDevices($adb) {
    (& $adb devices) | Select-Object -Skip 1 |
        Where-Object { $_ -match "`tdevice$" } |
        ForEach-Object { ($_ -split "`t")[0] }
}

$adb = Resolve-Adb
Write-Host "adb: $adb"

if (-not (Test-Path $LOCAL)) {
    throw "当前目录没有 anchor_config.json。先用 pull_anchor_config.ps1 拉取，或把 golden 文件放到这里。"
}

$devices = @(Get-ConnectedDevices $adb)
if ($devices.Count -eq 0) { throw "没有已授权的设备连接。" }

$ok = 0; $skip = 0; $fail = 0
foreach ($serial in $devices) {
    $hasPkg = (& $adb -s $serial shell "pm list packages $PKG") 2>$null
    if ("$hasPkg" -notmatch [regex]::Escape($PKG)) {
        Write-Host "[$serial] 跳过 — 未安装 $PKG" -ForegroundColor Yellow
        $skip++; continue
    }

    # 目录一般已由 app 创建；保险起见尝试创建（失败忽略）。
    & $adb -s $serial shell "mkdir -p '$REMOTE_DIR'" 2>$null | Out-Null
    & $adb -s $serial push $LOCAL $REMOTE | Out-Null
    if ($LASTEXITCODE -eq 0) {
        Write-Host "[$serial] OK — 已推送 anchor_config.json" -ForegroundColor Green
        $ok++
    } else {
        Write-Host "[$serial] 失败 — push 出错（检查 app 是否至少启动过一次以创建目录）" -ForegroundColor Red
        $fail++
    }
}

Write-Host "完成: 推送 $ok / 跳过 $skip / 失败 $fail。推送后请重启各头显里的 app 以加载新配置。" -ForegroundColor Cyan
