# Builds build/justos-connect.apk without Gradle: aapt2, javac, d8, zipalign, apksigner.
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File build.ps1
# Env: ANDROID_HOME (default %LOCALAPPDATA%\Android\Sdk), JAVA_HOME (default ~\.jdks\jbr-21.0.11),
#      BUILD_TOOLS (default 36.0.0), PLATFORM (default android-36),
#      CONNECT_KEYSTORE (default ~\.justos-connect.keystore), CONNECT_KEYSTORE_PASS (default justos-connect).
$ErrorActionPreference = 'Stop'

$Root = $PSScriptRoot
$Sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } else { Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
$BuildTools = if ($env:BUILD_TOOLS) { $env:BUILD_TOOLS } else { '36.0.0' }
$Platform = if ($env:PLATFORM) { $env:PLATFORM } else { 'android-36' }
if (-not $env:JAVA_HOME) { $env:JAVA_HOME = Join-Path $env:USERPROFILE '.jdks\jbr-21.0.11' }
$Bt = Join-Path $Sdk "build-tools\$BuildTools"
$AndroidJar = Join-Path $Sdk "platforms\$Platform\android.jar"
$Java = Join-Path $env:JAVA_HOME 'bin'

$Keystore = if ($env:CONNECT_KEYSTORE) { $env:CONNECT_KEYSTORE } else { Join-Path $env:USERPROFILE '.justos-connect.keystore' }
if (-not $env:CONNECT_KEYSTORE_PASS) { $env:CONNECT_KEYSTORE_PASS = 'justos-connect' }

foreach ($p in @($Bt, $AndroidJar, (Join-Path $Java 'javac.exe'))) {
    if (-not (Test-Path $p)) { throw "Missing: $p" }
}

function Invoke-Step([string]$Name, [scriptblock]$Body) {
    Write-Host "==> $Name"
    & $Body
    if ($LASTEXITCODE -ne 0) { throw "$Name failed (exit $LASTEXITCODE)" }
}

$Final = Join-Path $Root 'build'
New-Item -ItemType Directory -Force $Final | Out-Null
# aapt2 hangs when writing to UNC paths (\\wsl.localhost\...), so a project on a share
# is copied to a local work folder and only the finished APK is copied back.
if ($Root.StartsWith('\\')) {
    $Out = Join-Path $env:TEMP 'justos-connect-build'
    $Stage = Join-Path $Out 'project'
    if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
    New-Item -ItemType Directory -Force $Stage | Out-Null
    Copy-Item -Recurse (Join-Path $Root 'res'), (Join-Path $Root 'src') $Stage
    Copy-Item (Join-Path $Root 'AndroidManifest.xml') $Stage
    $Src = $Stage
} else {
    $Out = $Final
    $Src = $Root
}
$Gen = Join-Path $Out 'gen'
$Classes = Join-Path $Out 'classes'
$Dex = Join-Path $Out 'dex'
$ResZip = Join-Path $Out 'res.zip'
$Unsigned = Join-Path $Out 'unsigned.apk'
$Aligned = Join-Path $Out 'aligned.apk'
$Apk = Join-Path $Out 'justos-connect.apk'

foreach ($d in @($Gen, $Classes, $Dex)) {
    if (Test-Path $d) { Remove-Item -Recurse -Force $d }
    New-Item -ItemType Directory -Force $d | Out-Null
}
foreach ($f in @($ResZip, $Unsigned, $Aligned, $Apk, "$Apk.idsig")) {
    if (Test-Path $f) { Remove-Item -Force $f }
}

Invoke-Step 'aapt2 compile' {
    & (Join-Path $Bt 'aapt2.exe') compile --dir (Join-Path $Src 'res') -o $ResZip
}
Invoke-Step 'aapt2 link' {
    & (Join-Path $Bt 'aapt2.exe') link -o $Unsigned -I $AndroidJar `
        --manifest (Join-Path $Src 'AndroidManifest.xml') `
        --java $Gen --min-sdk-version 26 --target-sdk-version 36 `
        --version-code 1 --version-name 1.0 $ResZip
}

$Sources = @(Get-ChildItem -Recurse -Filter *.java (Join-Path $Src 'src'), $Gen | ForEach-Object { $_.FullName })
$ArgFile = Join-Path $Out 'sources.txt'
[IO.File]::WriteAllLines($ArgFile, ($Sources | ForEach-Object { '"' + ($_ -replace '\\', '/') + '"' }))
Invoke-Step 'javac' {
    & (Join-Path $Java 'javac.exe') --release 17 -encoding UTF-8 -Xlint:-options `
        -classpath $AndroidJar -d $Classes "@$ArgFile"
}

$ClassesJar = Join-Path $Out 'classes.jar'
if (Test-Path $ClassesJar) { Remove-Item -Force $ClassesJar }
Invoke-Step 'jar' { & (Join-Path $Java 'jar.exe') --create --file $ClassesJar -C $Classes . }
Invoke-Step 'd8' {
    & (Join-Path $Bt 'd8.bat') --release --min-api 26 --lib $AndroidJar --output $Dex $ClassesJar
}

# Add classes.dex at the root of the APK; aapt keeps resources.arsc uncompressed.
Invoke-Step 'add dex' {
    Push-Location $Dex
    try { & (Join-Path $Bt 'aapt.exe') add $Unsigned 'classes.dex' | Out-Null } finally { Pop-Location }
}
Invoke-Step 'zipalign' { & (Join-Path $Bt 'zipalign.exe') -p -f 4 $Unsigned $Aligned }

if (-not (Test-Path $Keystore)) {
    Write-Host "==> Creating signing key at $Keystore"
    & (Join-Path $Java 'keytool.exe') -genkeypair -keystore $Keystore -alias justos `
        -keyalg RSA -keysize 2048 -validity 10000 `
        -storepass $env:CONNECT_KEYSTORE_PASS -keypass $env:CONNECT_KEYSTORE_PASS `
        -dname 'CN=JustOS Connect' -noprompt
    if ($LASTEXITCODE -ne 0) { throw "keytool failed (exit $LASTEXITCODE)" }
}
Invoke-Step 'apksigner sign' {
    & (Join-Path $Bt 'apksigner.bat') sign --ks $Keystore --ks-key-alias justos `
        --ks-pass env:CONNECT_KEYSTORE_PASS --key-pass env:CONNECT_KEYSTORE_PASS `
        --out $Apk $Aligned
}
Invoke-Step 'apksigner verify' { & (Join-Path $Bt 'apksigner.bat') verify --verbose $Apk }

Remove-Item -Force $Unsigned, $Aligned
if ($Out -ne $Final) {
    Copy-Item -Force $Apk $Final
    $Apk = Join-Path $Final 'justos-connect.apk'
}
$Size = (Get-Item $Apk).Length
Write-Host ("BUILD OK: {0} ({1:N0} bytes, {2:N1} KiB)" -f $Apk, $Size, ($Size / 1KB))
