# Compiles Protocol.java with the test and runs it on the desktop JDK.
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$app = Split-Path -Parent $here
$jdk = if ($env:JAVA_HOME) { $env:JAVA_HOME } else { 'C:\Users\matth\.jdks\jbr-21.0.11' }
$out = Join-Path $app 'build\test-classes'
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
& "$jdk\bin\javac.exe" --release 17 -d $out "$app\src\dev\justos\connect\Protocol.java" "$here\ProtocolTest.java"
if ($LASTEXITCODE -ne 0) { throw 'javac failed' }
& "$jdk\bin\java.exe" -cp $out ProtocolTest
if ($LASTEXITCODE -ne 0) { throw 'test failed' }
