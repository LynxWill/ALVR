# Manual implementation of cargo xtask prepare-deps --platform windows
# Bypasses the `unzip` requirement by using PowerShell's Expand-Archive

$ErrorActionPreference = "Stop"

$ALVR_ROOT   = "E:\CC_Project\ALVR"
$DEPS_PATH   = "$ALVR_ROOT\deps\windows"
$TEMP_DIR    = "$env:TEMP\alvr_deps_download"

# VCVARS for cmake (needed to build libvpl)
$VCVARS = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"

Write-Host "=== ALVR Windows Deps Setup ===" -ForegroundColor Cyan
Write-Host "deps_path: $DEPS_PATH"

# --------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------
function Download-And-Extract {
    param([string]$Url, [string]$DestDir, [string]$Label)

    $tmpZip = "$TEMP_DIR\$Label.zip"
    Write-Host "`n[+] Downloading $Label ..." -ForegroundColor Green
    Invoke-WebRequest -Uri $Url -OutFile $tmpZip -UseBasicParsing
    Write-Host "[+] Extracting $Label to $DestDir ..."
    New-Item -ItemType Directory -Force -Path $DestDir | Out-Null
    Expand-Archive -Path $tmpZip -DestinationPath $DestDir -Force
    Remove-Item $tmpZip -Force
    Write-Host "[+] $Label done."
}

# --------------------------------------------------------------------------
# Prepare directories
# --------------------------------------------------------------------------
New-Item -ItemType Directory -Force -Path $TEMP_DIR | Out-Null
New-Item -ItemType Directory -Force -Path $DEPS_PATH | Out-Null

# --------------------------------------------------------------------------
# 1. x264
# --------------------------------------------------------------------------
Write-Host "`n=== 1/4  x264 ===" -ForegroundColor Yellow

$X264_VERSION  = "0.164"
$X264_REVISION = "3086"
$X264_URL = "https://github.com/ShiftMediaProject/x264/releases/download/$X264_VERSION.r$X264_REVISION/libx264_$($X264_VERSION).r$($X264_REVISION)_msvc16.zip"
$X264_DEST = "$DEPS_PATH\x264"

Remove-Item $X264_DEST -Recurse -Force -ErrorAction SilentlyContinue
$tmpZip = "$TEMP_DIR\x264.zip"
Invoke-WebRequest -Uri $X264_URL -OutFile $tmpZip -UseBasicParsing
New-Item -ItemType Directory -Force -Path $X264_DEST | Out-Null
Expand-Archive -Path $tmpZip -DestinationPath $X264_DEST -Force
Remove-Item $tmpZip -Force

# Write x264.pc (goes in deps root, not deps/windows)
$X264_PC = @"

prefix=$($X264_DEST.Replace('\', '/'))
exec_prefix=`${prefix}/bin/x64
libdir=`${prefix}/lib/x64
includedir=`${prefix}/include

Name: x264
Description: x264 library
Version: $X264_VERSION
Libs: -L`${libdir} -lx264
Cflags: -I`${includedir}
"@
Set-Content -Path "$ALVR_ROOT\deps\x264.pc" -Value $X264_PC -Encoding ascii
Write-Host "[+] x264.pc written to $ALVR_ROOT\deps\x264.pc"

# --------------------------------------------------------------------------
# 2. Vulkan Headers
# --------------------------------------------------------------------------
Write-Host "`n=== 2/4  Vulkan Headers ===" -ForegroundColor Yellow

$VK_VERSION = "1.4.338"
$VK_URL  = "https://github.com/KhronosGroup/Vulkan-Headers/archive/refs/tags/v$VK_VERSION.zip"
$VK_DEST = "$DEPS_PATH\vulkan-headers"

Remove-Item $VK_DEST -Recurse -Force -ErrorAction SilentlyContinue
$tmpZip = "$TEMP_DIR\vulkan-headers.zip"
Invoke-WebRequest -Uri $VK_URL -OutFile $tmpZip -UseBasicParsing
New-Item -ItemType Directory -Force -Path $VK_DEST | Out-Null
Expand-Archive -Path $tmpZip -DestinationPath $VK_DEST -Force
Remove-Item $tmpZip -Force

# Rename extracted folder to "src"
$extractedName = "Vulkan-Headers-$VK_VERSION"
if (Test-Path "$VK_DEST\$extractedName") {
    Rename-Item "$VK_DEST\$extractedName" "src"
}
# Move include dir up
if (Test-Path "$VK_DEST\src\include") {
    Move-Item "$VK_DEST\src\include" "$VK_DEST\include"
}

# Write vulkan.pc
$VK_PC_DIR = "$VK_DEST\lib\pkgconfig"
New-Item -ItemType Directory -Force -Path $VK_PC_DIR | Out-Null
$VK_DEST_FORWARD = $VK_DEST.Replace('\', '/')
Set-Content -Path "$VK_PC_DIR\vulkan.pc" -Encoding ascii -Value @"
prefix=$VK_DEST_FORWARD
includedir=`${prefix}/include

Name: Vulkan-Headers
Description: Vulkan Header files
Version: $VK_VERSION
Cflags: -I`${includedir}
"@
Write-Host "[+] vulkan.pc written."

# --------------------------------------------------------------------------
# 3. FFmpeg (pre-built GPL shared — BtbN build)
# --------------------------------------------------------------------------
Write-Host "`n=== 3/4  FFmpeg ===" -ForegroundColor Yellow

$FF_NAME = "ffmpeg-n8.1-latest-win64-gpl-shared-8.1"
$FF_URL  = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/$FF_NAME.zip"
$FF_DEST = "$DEPS_PATH\ffmpeg"

Remove-Item $FF_DEST -Recurse -Force -ErrorAction SilentlyContinue
$tmpZip = "$TEMP_DIR\ffmpeg.zip"
Write-Host "[+] Downloading FFmpeg (~100 MB) ..."
Invoke-WebRequest -Uri $FF_URL -OutFile $tmpZip -UseBasicParsing
Write-Host "[+] Extracting FFmpeg ..."
Expand-Archive -Path $tmpZip -DestinationPath $DEPS_PATH -Force
Remove-Item $tmpZip -Force

if (Test-Path "$DEPS_PATH\$FF_NAME") {
    Rename-Item "$DEPS_PATH\$FF_NAME" "ffmpeg"
    Write-Host "[+] FFmpeg extracted to $FF_DEST"
}

# --------------------------------------------------------------------------
# 4. libvpl (Intel oneVPL) — download source + cmake build
# --------------------------------------------------------------------------
Write-Host "`n=== 4/4  libvpl ===" -ForegroundColor Yellow

$VPL_VERSION = "2.15.0"
$VPL_URL  = "https://github.com/intel/libvpl/archive/refs/tags/v$VPL_VERSION.zip"
$VPL_DEST = "$DEPS_PATH\libvpl"

Remove-Item $VPL_DEST -Recurse -Force -ErrorAction SilentlyContinue
$tmpZip = "$TEMP_DIR\libvpl.zip"
Invoke-WebRequest -Uri $VPL_URL -OutFile $tmpZip -UseBasicParsing
Expand-Archive -Path $tmpZip -DestinationPath $DEPS_PATH -Force
Remove-Item $tmpZip -Force

if (Test-Path "$DEPS_PATH\libvpl-$VPL_VERSION") {
    Rename-Item "$DEPS_PATH\libvpl-$VPL_VERSION" "libvpl"
}

$INSTALL_PREFIX = "$VPL_DEST\alvr_build"
Write-Host "[+] Building libvpl with cmake + MSVC ..."

# Use VS developer bat to have cl.exe in path
$cmakeScript = @"
call "$VCVARS"
cd /d "$VPL_DEST"
cmake -B build -DUSE_MSVC_STATIC_RUNTIME=ON -DCMAKE_INSTALL_PREFIX="$INSTALL_PREFIX"
cmake --build build --config Release
cmake --install build --config Release
"@
$tmpBat = "$TEMP_DIR\build_libvpl.bat"
Set-Content -Path $tmpBat -Value $cmakeScript -Encoding ascii
Write-Host "[+] Running cmake build (this may take a few minutes) ..."
cmd /c $tmpBat
if ($LASTEXITCODE -ne 0) {
    Write-Host "[!] libvpl cmake build failed (exit $LASTEXITCODE)" -ForegroundColor Red
    Write-Host "[!] libvpl is used for Intel ARC encoding only. Continuing anyway..." -ForegroundColor Yellow
} else {
    Write-Host "[+] libvpl built successfully."
}

# --------------------------------------------------------------------------
# Set PKG_CONFIG_PATH
# --------------------------------------------------------------------------
Write-Host "`n=== Setting PKG_CONFIG_PATH ===" -ForegroundColor Yellow
$pkgPaths = @(
    $DEPS_PATH,
    "$VK_DEST\lib\pkgconfig",
    "$INSTALL_PREFIX\lib\pkgconfig"
) -join ";"

[System.Environment]::SetEnvironmentVariable("PKG_CONFIG_PATH", $pkgPaths, "User")
$env:PKG_CONFIG_PATH = $pkgPaths
Write-Host "[+] PKG_CONFIG_PATH = $pkgPaths"

# --------------------------------------------------------------------------
# Summary
# --------------------------------------------------------------------------
Write-Host "`n=== All dependencies prepared! ===" -ForegroundColor Cyan
Write-Host "  x264:    $X264_DEST"
Write-Host "  Vulkan:  $VK_DEST"
Write-Host "  FFmpeg:  $FF_DEST"
Write-Host "  libvpl:  $VPL_DEST"
Write-Host ""
Write-Host "Next: run 'cargo xtask build-streamer --gpl' in a VS2022 Developer PowerShell"
