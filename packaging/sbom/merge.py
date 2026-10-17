#!/usr/bin/env python3
"""Builds the CycloneDX SBOM that ships with each nightly (M1 task 23c).

`cargo cyclonedx` describes the Rust crates (one document per workspace member). This script
merges the documents of the two shipped Rust components (the engine and the engine client linked
into the shell) into one, and adds what cargo cannot know: PDFium (version and checksum from
`third_party/pdfium.lock`) and Qt (the version CI builds against), both of which are shipped as
shared libraries.

    merge.py --cyclonedx-dir <workspace> --pdfium-lock third_party/pdfium.lock \
             --qt-version 6.8.3 --version <vellora version> --out sbom.cdx.json
"""

import argparse
import json
import pathlib
import re
import sys
import tomllib

SHIPPED_CRATES = ("vellora-engine", "vellora-engine-client")


def load_boms(workspace: pathlib.Path) -> list[dict]:
    # The file names depend on the tool's version, so documents are found by the crate they describe.
    by_name: dict[str, dict] = {}
    for path in sorted(workspace.glob("crates/*/*.cdx.json")):
        bom = json.loads(path.read_text(encoding="utf-8"))
        name = bom.get("metadata", {}).get("component", {}).get("name")
        if name:
            by_name.setdefault(name, bom)
    missing = [crate for crate in SHIPPED_CRATES if crate not in by_name]
    if missing:
        sys.exit(f"no CycloneDX document for {missing} under {workspace}/crates (run cargo cyclonedx)")
    return [by_name[crate] for crate in SHIPPED_CRATES]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cyclonedx-dir", required=True, type=pathlib.Path)
    parser.add_argument("--pdfium-lock", required=True, type=pathlib.Path)
    parser.add_argument("--qt-version", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--out", required=True, type=pathlib.Path)
    args = parser.parse_args()

    lock = tomllib.loads(args.pdfium_lock.read_text(encoding="utf-8"))
    pdfium_version = lock["version"]
    if not re.fullmatch(r"[0-9][0-9A-Za-z.\-]*", args.qt_version):
        sys.exit(f"odd Qt version {args.qt_version!r}")

    components: dict[str, dict] = {}
    for bom in load_boms(args.cyclonedx_dir):
        # The document's own subject (the crate) is a component of the product as well.
        subject = bom.get("metadata", {}).get("component")
        for component in ([subject] if subject else []) + bom.get("components", []):
            key = component.get("purl") or f"{component['name']}@{component.get('version', '')}"
            components.setdefault(key, component)

    manual = [
        {
            "type": "library",
            "bom-ref": f"pdfium@{pdfium_version}",
            "name": "PDFium",
            "version": pdfium_version,
            "description": "PDF renderer, a prebuilt shared library from bblanchon/pdfium-binaries "
            "(V8 and XFA disabled), loaded by the engine process only",
            "licenses": [{"license": {"id": "Apache-2.0"}}, {"license": {"id": "BSD-3-Clause"}}],
            "externalReferences": [
                {"type": "website", "url": "https://pdfium.googlesource.com/pdfium/"},
                {"type": "distribution", "url": "https://github.com/bblanchon/pdfium-binaries"},
            ],
            "properties": [{"name": "vellora:pin", "value": "third_party/pdfium.lock"}],
        },
        {
            "type": "library",
            "bom-ref": f"qt@{args.qt_version}",
            "name": "Qt",
            "version": args.qt_version,
            "description": "Qt Base (Core, Gui, Widgets, Network, PrintSupport) and Qt Svg, dynamically linked "
            "(LGPL-3.0); on Linux the KDE runtime provides it",
            "licenses": [{"license": {"id": "LGPL-3.0-only"}}],
            "externalReferences": [
                {"type": "website", "url": "https://www.qt.io/"},
                {"type": "vcs", "url": "https://code.qt.io/cgit/qt/qtbase.git"},
            ],
        },
    ]

    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "bom-ref": f"vellora@{args.version}",
                "name": "Vellora",
                "version": args.version,
                "licenses": [{"license": {"id": "MPL-2.0"}}],
            },
            "tools": {"components": [{"type": "application", "name": "cargo-cyclonedx"}]},
        },
        "components": manual + sorted(components.values(), key=lambda c: (c["name"], c.get("version", ""))),
    }
    args.out.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"{args.out}: {len(document['components'])} components")


if __name__ == "__main__":
    main()
