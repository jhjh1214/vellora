# packaging/ — distribution

Built and smoke-tested by `.github/workflows/packaging.yml` (on pull requests that touch packaging,
and from `nightly.yml`). Nightly artifacts are unsigned.

- `windows/`: the per-user WiX MSI (`vellora.wxs`), `stage.ps1` (copies the app, runs `windeployqt`),
  `smoke.ps1` (install, run `--diagnostics-script`, association, uninstall, nothing left behind).
- `stage-licenses.ps1`: the licence files every package ships (the LGPL-3.0 and GPL-3.0 texts are downloaded and hash-checked).

Planned channels:

| Platform | Format | Channel |
|---|---|---|
| Windows 10 22H2+ / 11 | MSIX or MSI (WiX), Authenticode-signed | GitHub Releases, winget |
| Linux | Flatpak | Flathub (primary); AppImage optional |
| macOS 13+ | notarised `.dmg` | GitHub Releases (from MVP+1) |

Every release ships with: SHA-256 checksums, a CycloneDX SBOM, `THIRD_PARTY_LICENSES`, and Qt shared libraries (dynamic linking; LGPL relinking notice).
