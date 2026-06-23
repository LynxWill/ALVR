# Pull anchor_config.json from a connected LBEStreaming Quest into the CURRENT directory.
#
# 用法 / Usage:
#   .\pull_anchor_config.ps1                 # 从唯一连接的设备拉取
#   .\pull_anchor_config.ps1 -Serial XXXX    # 指定设备序列号（多台连接时）
#
# 在母机上跑完配置向导后，用本脚本把作者好的 anchor_config.json 拉到当前目录，
# 作为本场地的 golden 文件，再用 push_anchor_config.ps1 分发到其它头显。

param([string]$Serial = "")

$ErrorActionPreference = "Stop"
$PKG = "alvr.client.lbestreaming"
$REMOTE = "/sdcard/Android/data/$PKG/files/anchor_config.json"

function Resolve-Adb {
    # 优先用随包/streamer 目录里的 adb（离线），否则用 PATH 或 SDK 里的。
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

$devices = @(Get-ConnectedDevices $adb)
if ($devices.Count -eq 0) { throw "没有已授权的设备连接（先确认头显里允许 USB 调试）。" }

if (-not $Serial) {
    if ($devices.Count -gt 1) {
        throw "连接了多台设备: $($devices -join ', ')。请用 -Serial <序列号> 指定母机。"
    }
    $Serial = $devices[0]
}

# 确认设备上存在配置文件（没跑过向导就不会有）
$check = (& $adb -s $Serial shell "[ -f '$REMOTE' ] && echo FOUND") 2>$null
if ("$check" -notmatch "FOUND") {
    throw "设备 $Serial 上没有 anchor_config.json。请先在母机上跑一遍配置向导。"
}

& $adb -s $Serial pull $REMOTE ".\anchor_config.json"
if ($LASTEXITCODE -ne 0) { throw "adb pull 失败。" }

Write-Host "已从 $Serial 拉取 anchor_config.json 到当前目录。" -ForegroundColor Green
