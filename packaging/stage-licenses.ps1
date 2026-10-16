# Adds the licence files every package ships to $Dest: Vellora's own licence, the third-party
# notices and the texts Qt's LGPL asks for (LGPL-3.0 and the GPL-3.0 it refers to), plus where to get
# Qt's source. The texts (SPDX licence list, pinned commit) are downloaded here, not committed, and checked against a pinned hash.
param([Parameter(Mandatory)] [string] $Dest)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$dir = Join-Path $Dest 'licenses'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item (Join-Path $root 'LICENSE') (Join-Path $Dest 'LICENSE.txt')
Copy-Item (Join-Path $root 'THIRD_PARTY_LICENSES') (Join-Path $Dest 'THIRD_PARTY_LICENSES.txt')
$texts = @{
    'LGPL-3.0.txt' = @('https://raw.githubusercontent.com/spdx/license-list-data/31ba1a50e5397e00a304dbadc76531740e89ee48/text/LGPL-3.0-only.txt', '996af0513df21f7496288951c41428a03c174e9e4a9d63665c57d670f845ccb1')
    'GPL-3.0.txt'  = @('https://raw.githubusercontent.com/spdx/license-list-data/31ba1a50e5397e00a304dbadc76531740e89ee48/text/GPL-3.0-only.txt', 'fb981668c18a279e285fc4d83fba1e836cc84dd4daa73c9697d3cfd2d8aca6e0')
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
