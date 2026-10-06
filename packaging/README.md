# packaging/ — distribution

Populated as releases begin (Phase 1+).

| Platform | Format | Channel |
|---|---|---|
| Windows 10 22H2+ / 11 | MSIX or MSI (WiX), Authenticode-signed | GitHub Releases, winget |
| Linux | Flatpak | Flathub (primary); AppImage optional |
| macOS 13+ | notarised `.dmg` | GitHub Releases (from MVP+1) |

Every release ships with: SHA-256 checksums, a CycloneDX SBOM, `THIRD_PARTY_LICENSES`, and Qt shared libraries (dynamic linking; LGPL relinking notice).
