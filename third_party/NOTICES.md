PDFium and Qt (binary dependencies, not Rust crates)
====================================================

PDFium
------
Vellora loads PDFium at run time from the prebuilt, V8-free and XFA-free build pinned in
`third_party/pdfium.lock` (https://github.com/bblanchon/pdfium-binaries, chromium/8086, PDFium
157.0.8086.0). PDFium is licensed under the Apache License 2.0 and BSD-3-Clause terms of the PDFium
Authors; the build bundles the libraries listed below. Their license texts follow in the section
"PDFium bundled license texts".

Bundled in the PDFium build: abseil, agg23, dragonbox, fast_float, freetype, harfbuzz, icu,
lcms, libjpeg-turbo, libopenjpeg, libpng, llvm-libc, simdutf, zlib.

Qt
--
The desktop shell links Qt 6.8 (LTS) Widgets, Network, Gui and Core dynamically. Qt is available under the
GNU Lesser General Public License version 3 (LGPL-3.0) with the Qt exceptions; Vellora uses only
LGPL-licensed Qt modules (ADR-0007). Qt's source code and license texts are available at
https://code.qt.io/ and https://www.qt.io/licensing. Because Qt is linked dynamically, you may
replace the Qt libraries shipped with a Vellora release by a modified version of Qt under the terms
of the LGPL. Release packages must include the Qt license files that come with the Qt installation
they were built from.
