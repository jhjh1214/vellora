# Smoke test of the Windows MSI:
#   1. install it silently, as the current user, without elevation requests;
#   2. start the installed app with `--diagnostics-script scroll-zoom` on a fixture (exit code 0,
#      which needs the engine to start in its AppContainer from the installed folder);
#   3. install again with the .pdf association feature, check what it registers, then uninstall;
#   4. check that the uninstall left nothing of Vellora behind.
#
#   pwsh packaging/windows/smoke.ps1 -Msi dist/vellora.msi -Fixture crates/engine/tests/fixtures/golden.pdf
param(
    [Parameter(Mandatory)] [string] $Msi,
    [Parameter(Mandatory)] [string] $Fixture
)
$ErrorActionPreference = 'Stop'
$Msi = (Resolve-Path $Msi).Path
$Fixture = (Resolve-Path $Fixture).Path
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Vellora'
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$logs = Join-Path (Split-Path $Msi) 'msi-logs'
New-Item -ItemType Directory -Force -Path $logs | Out-Null

function Fail([string] $message) { Write-Host "::error::$message"; exit 1 }

# What exists under the places an install may write to, by name.
function Snapshot {
    $roots = @($installDir, (Join-Path $env:LOCALAPPDATA 'Packages'), $startMenu)
    $items = foreach ($root in $roots) {
        if (Test-Path $root) {
            Get-ChildItem -Recurse -Force -ErrorAction SilentlyContinue $root |
                Where-Object { $_.FullName -match 'Vellora' } | ForEach-Object { $_.FullName }
        }
    }
    @($items | Sort-Object)
}
function RegistrySnapshot {
    $keys = @('HKCU:\Software\Vellora', 'HKCU:\Software\Classes\Vellora.Pdf')
    $found = @($keys | Where-Object { Test-Path $_ })
    $found += @(Get-ChildItem -Recurse -ErrorAction SilentlyContinue 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' |
        Where-Object { (Get-ItemProperty $_.PSPath).DisplayName -eq 'Vellora' } | ForEach-Object { $_.Name })
    if ((Get-ItemProperty -ErrorAction SilentlyContinue 'HKCU:\Software\Classes\.pdf\OpenWithProgids').'Vellora.Pdf' -ne $null) { $found += 'OpenWithProgids\Vellora.Pdf' }
    if ((Get-ItemProperty -ErrorAction SilentlyContinue 'HKCU:\Software\RegisteredApplications').Vellora) { $found += 'RegisteredApplications\Vellora' }
    $found
}
function Msiexec([string[]] $arguments, [string] $name) {
    $p = Start-Process msiexec.exe -ArgumentList ($arguments + @('/qn', '/l*v', (Join-Path $logs "$name.log"))) -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "msiexec $name failed with $($p.ExitCode); see msi-logs/$name.log" }
}
function RunScript {
    $out = Join-Path $logs 'script-out.txt'
    $err = Join-Path $logs 'script-err.txt'
    $p = Start-Process (Join-Path $installDir 'vellora.exe') `
        -ArgumentList @('--diagnostics-script', 'scroll-zoom', "`"$Fixture`"") `
        -RedirectStandardOutput $out -RedirectStandardError $err -PassThru
    if (-not $p.WaitForExit(180000)) { $p.Kill(); Fail 'the diagnostics script did not finish in 180 s' }
    Get-Content $out, $err | Write-Host
    if ($p.ExitCode -ne 0) { Fail "vellora --diagnostics-script exited with $($p.ExitCode)" }
    if (-not (Select-String -Path $out -Pattern 'diagnostics-script scroll-zoom:' -Quiet)) { Fail 'no report on stdout' }
}

$filesBefore = Snapshot
$registryBefore = RegistrySnapshot

# 1 and 2: the default install.
Msiexec @('/i', "`"$Msi`"") 'install'
foreach ($name in 'vellora.exe', 'vellora-engine.exe', 'pdfium.dll', 'LICENSE.txt', 'THIRD_PARTY_LICENSES.txt', 'licenses\LGPL-3.0.txt', 'platforms\qwindows.dll') {
    if (-not (Test-Path (Join-Path $installDir $name))) { Fail "the install lacks $name" }
}
if (-not (Test-Path (Join-Path $startMenu 'Vellora.lnk'))) { Fail 'no Start menu shortcut' }
if (Test-Path 'HKCU:\Software\Classes\Vellora.Pdf') { Fail 'the .pdf association is registered without being asked for' }
RunScript
Msiexec @('/x', "`"$Msi`"") 'uninstall-1'

# 3: with the association.
Msiexec @('/i', "`"$Msi`"", 'ADDLOCAL=Main,PdfAssociation') 'install-pdf'
if (-not (Test-Path 'HKCU:\Software\Classes\Vellora.Pdf\shell\open\command')) { Fail 'the association was not registered' }
if ($null -eq (Get-ItemProperty 'HKCU:\Software\Classes\.pdf\OpenWithProgids').'Vellora.Pdf') { Fail 'Vellora is not in the Open with list' }
$default = (Get-ItemProperty -ErrorAction SilentlyContinue 'HKCU:\Software\Classes\.pdf').'(default)'
if ($default -eq 'Vellora.Pdf') { Fail 'the installer took over the default application for .pdf' }
Msiexec @('/x', "`"$Msi`"") 'uninstall-2'

# 4: nothing left.
$filesAfter = Snapshot
$registryAfter = RegistrySnapshot
$leftFiles = @($filesAfter | Where-Object { $filesBefore -notcontains $_ })
$leftKeys = @($registryAfter | Where-Object { $registryBefore -notcontains $_ })
if (Test-Path $installDir) { $leftFiles += $installDir }
if ($leftFiles.Count -gt 0) { Fail ("the uninstall left files behind:`n" + ($leftFiles -join "`n")) }
if ($leftKeys.Count -gt 0) { Fail ("the uninstall left registry entries behind:`n" + ($leftKeys -join "`n")) }
Write-Host 'smoke test passed: install, run, association, uninstall leaves nothing'
