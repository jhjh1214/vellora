# Stages the Windows package: the shell, the engine and PDFium side by side, Qt deployed next to
# them (dynamic linking, so a user can replace the Qt libraries: LGPL), and the licence files.
#
#   pwsh packaging/windows/stage.ps1 -BuildDir build -Stage dist/stage
#
# `windeployqt` must be on PATH (the Qt install puts it there). Used by CI only.
param(
    [Parameter(Mandatory)] [string] $BuildDir,
    [Parameter(Mandatory)] [string] $Stage
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

foreach ($name in 'vellora.exe', 'vellora-engine.exe', 'pdfium.dll') {
    $file = Join-Path $BuildDir $name
    if (-not (Test-Path $file)) { throw "missing $file (build the app in Release first)" }
    Copy-Item $file $Stage
}

& windeployqt --release --no-translations --no-opengl-sw --no-system-d3d-compiler `
    --no-compiler-runtime (Join-Path $Stage 'vellora.exe')
if ($LASTEXITCODE -ne 0) { throw "windeployqt failed ($LASTEXITCODE)" }

# The C++ runtime, as DLLs next to the executables: a per-user install cannot run the
# redistributable installer. (`windeployqt` would copy `vc_redist.x64.exe` instead.)
$redist = Get-ChildItem (Join-Path $env:VCToolsRedistDir 'x64') -Directory -Filter 'Microsoft.VC*.CRT' | Select-Object -First 1
if (-not $redist) { throw 'the MSVC redistributable DLLs were not found (is VCToolsRedistDir set?)' }
Copy-Item (Join-Path $redist.FullName '*.dll') $Stage

Copy-Item (Join-Path $root 'packaging/windows/remove-sandbox-profile.ps1') $Stage
& (Join-Path $root 'packaging/stage-licenses.ps1') -Dest $Stage
