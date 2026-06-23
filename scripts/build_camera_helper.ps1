# Compile the Java CameraHelper to a dex and place it in client_openxr/assets/
# so cargo-apk bundles it into the APK. Rust loads it at runtime via
# InMemoryDexClassLoader.

# Continue (not Stop): native tools like javac write warnings to stderr, which
# under Stop would abort the script. We check $LASTEXITCODE explicitly instead.
$ErrorActionPreference = "Continue"

$SDK         = $env:ANDROID_HOME
$BUILD_TOOLS = "$SDK\build-tools\37.0.0"
$SRC_DIR     = "E:\CC_Project\ALVR\alvr\client_openxr\java"
$OUT_DIR     = "$env:TEMP\camera_helper_build"
$ASSETS_DIR  = "E:\CC_Project\ALVR\alvr\client_openxr\assets"

# Pick the highest installed android.jar
$androidJar = Get-ChildItem "$SDK\platforms\android-*\android.jar" -ErrorAction SilentlyContinue |
    Sort-Object { [int]($_.Directory.Name -replace 'android-','') } | Select-Object -Last 1
if (-not $androidJar) { throw "android.jar not found under $SDK\platforms" }
Write-Host "Using android.jar: $($androidJar.FullName)"

Remove-Item $OUT_DIR -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $OUT_DIR  | Out-Null
New-Item -ItemType Directory -Force -Path $ASSETS_DIR | Out-Null

# 1. javac
$javaFiles = Get-ChildItem -Recurse $SRC_DIR -Filter *.java | ForEach-Object { $_.FullName }
Write-Host "[javac] compiling $($javaFiles.Count) file(s)..."
& javac -source 8 -target 8 -cp $androidJar.FullName -d $OUT_DIR $javaFiles
if ($LASTEXITCODE -ne 0) { throw "javac failed" }

# 2. d8 -> classes.dex
$classFiles = Get-ChildItem -Recurse $OUT_DIR -Filter *.class | ForEach-Object { $_.FullName }
Write-Host "[d8] dexing $($classFiles.Count) class(es)..."
& "$BUILD_TOOLS\d8.bat" --min-api 26 --output $OUT_DIR $classFiles
if ($LASTEXITCODE -ne 0) { throw "d8 failed" }

# 3. place dex in assets
Copy-Item "$OUT_DIR\classes.dex" "$ASSETS_DIR\camera_helper.dex" -Force
Write-Host "Done: $ASSETS_DIR\camera_helper.dex ($((Get-Item "$ASSETS_DIR\camera_helper.dex").Length) bytes)"
