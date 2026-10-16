# Run by the uninstaller: removes the engine's AppContainer profile ("Vellora.Engine", registered on
# the first start; ADR-0017), which holds a registry mapping and a folder under
# %LOCALAPPDATA%\Packages.
$ErrorActionPreference = 'SilentlyContinue'
Add-Type -Namespace Vellora -Name Profile -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("userenv.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int DeleteAppContainerProfile(string name);
'@
[void][Vellora.Profile]::DeleteAppContainerProfile('Vellora.Engine')
$leftover = Join-Path $env:LOCALAPPDATA 'Packages\Vellora.Engine'
if (Test-Path $leftover) { Remove-Item -Recurse -Force $leftover }
exit 0
