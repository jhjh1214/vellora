#!/bin/sh
# Builds the macOS disk image from a Release build (unsigned, for testers: M1 task 23c).
#
#   packaging/macos/stage.sh <build-dir> <version> <out-dir>
#
# The shell is not a bundle in CMake (MACOSX_BUNDLE is off), so the bundle is assembled here:
# the shell, the engine and PDFium side by side in Contents/MacOS (where the shell looks for the
# engine and the engine for PDFium), then `macdeployqt` copies the Qt frameworks and plugins into
# the bundle (dynamic linking: the user can replace them; LGPL). The bundle is ad-hoc signed so that
# it starts on Apple silicon; it is not signed with a developer identity and not notarised.
set -eu
build=$1
version=$2
out=$3
root=$(cd "$(dirname "$0")/../.." && pwd)

rm -rf "$out"
mkdir -p "$out/dmg-root"
app="$out/dmg-root/Vellora.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
for f in vellora vellora-engine libpdfium.dylib; do
  [ -f "$build/$f" ] || { echo "missing $build/$f (build the app in Release first)" >&2; exit 1; }
  cp "$build/$f" "$app/Contents/MacOS/"
done

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>vellora</string>
  <key>CFBundleIdentifier</key><string>io.github.jhjh1214.Vellora</string>
  <key>CFBundleName</key><string>Vellora</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <!-- Offered in "Open With" for PDF files; the rank keeps Vellora from becoming the default. -->
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>PDF document</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key><array><string>com.adobe.pdf</string></array>
    </dict>
  </array>
</dict>
</plist>
EOF

# Qt is deployed into the bundle; macdeployqt signs what it touches ad hoc.
macdeployqt "$app" -verbose=1
# One consistent ad-hoc signature over everything, the engine and PDFium included.
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

# Licences beside the bundle on the disk image.
pwsh "$root/packaging/stage-licenses.ps1" -Dest "$out/dmg-root"
ln -s /Applications "$out/dmg-root/Applications"

hdiutil create -volname Vellora -srcfolder "$out/dmg-root" -ov -format UDZO "$out/Vellora-$version-macos-arm64.dmg"
rm -rf "$out/dmg-root"
