# Adds the licence files every package ships to $Dest: Vellora's own licence, the third-party
# notices and the texts Qt's LGPL asks for (LGPL-3.0 and the GPL-3.0 it refers to), plus where to get
# Qt's source. The GNU texts are downloaded here, not committed, and checked against a pinned hash.
param([Parameter(Mandatory)] [string] $Dest)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$dir = Join-Path $Dest 'licenses'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item (Join-Path $root 'LICENSE') (Join-Path $Dest 'LICENSE.txt')
Copy-Item (Join-Path $root 'THIRD_PARTY_LICENSES') (Join-Path $Dest 'THIRD_PARTY_LICENSES.txt')
$texts = @{
    'LGPL-3.0.txt' = @('https://www.gnu.org/licenses/lgpl-3.0.txt', 'e3a994d82e644b03a792a930f574002658412f62407f5fee083f2555c5f23118')
    'GPL-3.0.txt'  = @('https://www.gnu.org/licenses/gpl-3.0.txt', '3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986')
}
foreach ($name in $texts.Keys) {
    $file = Join-Path $dir $name
    Invoke-WebRequest -Uri $texts[$name][0] -OutFile $file -MaximumRetryCount 5 -RetryIntervalSec 5
    $hash = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLowerInvariant()
    if ($hash -ne $texts[$name][1]) { throw "$name has hash $hash, expected $($texts[$name][1])" }
}
@'
Vellora uses the Qt toolkit (https://www.qt.io), licensed under the GNU LGPL version 3 (see
LGPL-3.0.txt and GPL-3.0.txt). The Qt libraries are shipped as separate shared libraries so that
you can replace them with your own build of a compatible Qt 6.8 version. Qt's source code is
available from https://download.qt.io/official_releases/qt/6.8/.
'@ | Set-Content -Encoding utf8 (Join-Path $dir 'Qt-NOTICE.txt')
