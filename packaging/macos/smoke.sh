#!/bin/sh
# Smoke test of the disk image: mount it, copy the app out (as a user dragging it to Applications
# would), unmount, start the copy with `--diagnostics-script scroll-zoom` on a fixture (exit code 0:
# the engine starts and renders from inside the bundle), and check the bundled licence files.
#
#   packaging/macos/smoke.sh <image.dmg> <fixture.pdf>
set -eu
dmg=$1
fixture=$2
work=$(mktemp -d)
mnt="$work/mnt"
mkdir -p "$mnt"
trap 'hdiutil detach "$mnt" -quiet 2>/dev/null || true; rm -rf "$work"' EXIT

hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$mnt" -quiet
for f in LICENSE.txt THIRD_PARTY_LICENSES.txt licenses/LGPL-3.0.txt licenses/Qt-NOTICE.txt; do
  [ -f "$mnt/$f" ] || { echo "::error::the image lacks $f"; exit 1; }
done
cp -R "$mnt/Vellora.app" "$work/Vellora.app"
hdiutil detach "$mnt" -quiet

app="$work/Vellora.app/Contents/MacOS"
for f in vellora vellora-engine libpdfium.dylib; do
  [ -f "$app/$f" ] || { echo "::error::the bundle lacks $f"; exit 1; }
done
[ -d "$work/Vellora.app/Contents/Frameworks/QtCore.framework" ] || { echo "::error::Qt was not deployed into the bundle"; exit 1; }

"$app/vellora" --diagnostics-script scroll-zoom "$fixture" | tee "$work/out.txt"
grep -q 'diagnostics-script scroll-zoom:' "$work/out.txt"
echo "macOS smoke test passed"
