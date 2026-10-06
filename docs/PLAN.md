# Vellora — Technical Plan

> **Status:** approved 2026-10-07. This is the founding plan. It's preserved as written so the reasoning stays traceable.
> Decisions in it are binding through the ADRs in [`docs/adr/`](adr/). To change a decision, write a new ADR that supersedes the old one; don't edit this file.
> Where this file and an ADR disagree, the newer ADR wins.

## Context

Goal: a local-first, open-source PDF workstation that can replace most serious Acrobat Pro workflows.

**Decisions already made (2026-10-07):**
- **License posture:** permissive. The project is MPL-2.0, which rules out MuPDF, Poppler and Ghostscript as linked dependencies.
- **Languages:** a Rust core for all code we write that touches hostile input, plus a C++/Qt 6 desktop shell.
- **Resourcing:** one core developer now. The repo, process and boundaries must be production-grade OSS from day one, so that growth needs no structural refactor.

**Project name: Vellora** (vellum + -ora). The CLI binary is `vellora`, crates are prefixed `vellora-`, and both the repo and the folder are `vellora`. Every `pdfstudio` in the original brief becomes `vellora` ("PDF Studio" is Qoppa's trademark).

**This session's job after approval** is the **Bootstrap** (section P): create the repo, the governance files, the architecture docs and ADRs, a compiling workspace skeleton, CI, and a step-by-step M0 task breakdown. That way a fresh Sonnet 5.5 session can start implementing with no further design work.

---

## A. Executive summary

Build a **desktop PDF workstation**. Its thesis is three properties that no existing open-source tool combines:

1. **Faithful.** Editing never silently degrades a document. Every save shows its *save impact*: incremental or full rewrite, which signatures are affected, and whether deleted content stays recoverable. Untouched content stays byte-identical.
2. **Safe.** PDFs are treated as hostile. Parsing and rendering run in a sandboxed, resource-limited engine process. Our own parser and writer are memory-safe Rust. JavaScript, launch actions and attachments never execute.
3. **Fast at scale.** Lazy everything. A 10,000-page or 500 MB file opens in about a second and scrolls at display rate.

**Engine strategy (options B + C, moving deliberately towards D):**
- **PDFium** (BSD/Apache) is the *read-only* rasteriser, text-geometry provider and form-interaction engine.
- We **own the object layer and the write path** in Rust: COS parser, revision-aware object store, incremental writer, content-stream surgery, redaction and signatures.

We never save through PDFium. Its content regenerator rewrites whole page streams, and in doing so drops `Tc`/`Tw` spacing operators and converts colour spaces to device colours. That alone disqualifies it as an editing engine.

**MVP (≈12 months solo).** A viewer and page organiser that beats existing OSS on speed and fidelity, with:
- standard-conformant annotations and threaded comments
- object-level editing (add text and images; move, resize or delete images and paths; delete text runs)
- a developer-grade Inspector and security report
- a **text compare with change-by-change review**, the feature that shows at a glance why the project exists
- a CLI that shares the same core

In-place editing of existing text, redaction, forms, signatures and OCR follow in dependency order. Each depends on infrastructure the MVP proves.

---

## B. Competitive analysis

| Product | Model | Strengths | Weaknesses relevant to us |
|---|---|---|---|
| **Adobe Acrobat Pro** | Proprietary subscription | Reference fidelity, signatures (AATL), preflight, accessibility, compare, XFA | Heavy, slow, account and telemetry pressure, cluttered tool hub, cloud upsell |
| **PDF-XChange Editor** | Proprietary (Windows) | Very fast, dense professional UI, strong editing and OCR | Windows-only, closed |
| **Foxit PDF Editor / Nitro / Kofax Power PDF** | Proprietary | Broad Acrobat parity | Closed, subscription drift |
| **ABBYY FineReader PDF / Draftable** | Proprietary | Best-in-class OCR / compare | Single-strength, closed |
| **Bluebeam Revu** | Proprietary | Review and markup workflows (AEC) | Domain-specific, Windows-centric |
| **Qoppa PDF Studio / Master PDF Editor** | Proprietary, cross-platform | Linux support, broad features | Closed; Java (Qoppa); mixed fidelity |
| **Okular / Evince (Papers)** | GPL, Poppler | Good viewers, annotations, signature verify (Okular) | Not editors; no page organisation; no compare or redaction |
| **SumatraPDF** | GPL, MuPDF | Extremely fast viewer | Windows viewer only |
| **PDF Arranger** | GPL, pikepdf/qpdf | Excellent page organisation | Pages only |
| **Stirling PDF** | MIT core, open-core since 2025 | Huge breadth of server-side tools | Web/server app, tool-at-a-time, not an interactive workstation; enterprise features paywalled |
| **PDF4QT** | **MIT since April 2025** (was LGPLv3), C++/Qt, own engine | **Closest prior art:** editing, signatures, redaction, optimisation, compare, XFA read-only | Single maintainer; C++ parsing of hostile input with no process isolation; Windows/Linux only. **MIT means its algorithms can legally be studied and ported (with attribution in NOTICE).** It's the primary reference implementation for our own engine work. |
| **Open PDF Studio** (OpenAEC) | LGPLv3; Tauri 2 + SolidJS + Rust; multi-process PDFium workers | Active, broad feature set (annotations, measurement, CAD, OCR, compare) | Webview UI; AEC/CAD focus; AI/MCP features; LGPL. Direct competitor on "Rust + PDFium"; we differentiate on faithful editing, sandboxing, signatures and native pro UX. |
| **LibreOffice Draw** | MPL | Can "edit" PDFs | Re-flows them on import: lossy reconstruction, not faithful editing |
| **Xournal++** | GPL | Pen annotation | Annotation overlay only |
| **pdfcpu, qpdf, pikepdf, OCRmyPDF** | Apache / Apache / MPL / MPL | Excellent CLIs and libraries | No GUI; single concern each |

**The actual gap:** no open-source, cross-platform desktop app combines faithful content editing, real redaction, correct signature validation, PDF compare/review, process isolation for hostile input, and a permissive license with a professional, keyboard-first UX. Each OSS tool solves one slice.

**Where Acrobat is unnecessarily complicated, and where we can do better:**
- Tool hub and 30-panel sprawl → **modes** (View, Organize, Annotate, Edit, Review, Redact, Forms), plus a command palette.
- Opaque saves → a **save-impact bar**.
- Redaction as a buried workflow → mark → review → apply → verify.
- Compare as a static report → a navigable PR-style review.
- Inspection hidden in "Preflight > Browse internal structure" → a first-class Inspector.

---

## C. Feature matrix

Difficulty: S/M/L/XL. Priority: P0 = MVP, P1 = v1.0, P2 = after 1.0, P3 = later or optional.

| Feature | Acrobat Pro | Existing OSS | Our target | Diff. | Prio |
|---|---|---|---|---|---|
| Rendering, navigation, thumbnails, outline, links | ✓ | ✓ (Okular, Sumatra) | ✓ + 10k-page smooth | M | P0 |
| Text selection / copy | ✓ | ✓ | ✓ | S | P0 |
| Search (case, whole word, regex, all pages) | partial regex | partial | ✓ streaming results | M | P0 |
| Page ops (reorder/rotate/delete/insert/extract/merge/split/import) | ✓ | ✓ (PDF Arranger) | ✓ multi-select, grid mode | M | P0 |
| Crop / resize page boxes | ✓ | partial | ✓ (with "crop ≠ removal" warning) | S | P1 |
| Annotations (markup, ink, shapes, FreeText, notes, replies, state) | ✓ | ✓ (Okular, partial) | ✓ standard /Annot, own appearance streams | M | P0 |
| Stamps, callouts, arrows, measurement | ✓ | partial | ✓ | M | P1 |
| Add text / image | ✓ | partial | ✓ (appended content stream) | M | P0 |
| Move/resize/delete existing images and paths | ✓ | rare | ✓ (operator-range surgery) | L | P0 |
| Edit existing text in place | ✓ | PDF4QT/Stirling (lossy) | ✓ tiered, with honest degradation | XL | P1 |
| Paragraph reflow editing | ✓ | ✗ | within detected blocks only | XL | P2 |
| Inspector (objects, fonts, images, size attribution) | buried | qpdf --json, CLI tools | ✓ first-class | M | P0 |
| Security analysis report | partial | peepdf/pdfid (CLI) | ✓ | S | P0 |
| Text compare + change review | ✓ | ✗ (diff-pdf: visual only) | ✓ PR-style | L | P0 |
| Visual compare | ✓ | diff-pdf | ✓ | M | P1 |
| Object-level compare (images, annots, fields, fonts, metadata) | partial | ✗ | ✓ | L | P2 |
| Real redaction (text, images, vectors, annots, metadata) | ✓ | MuPDF-based only | ✓ + post-apply verification | XL | P1 |
| Sanitisation (JS, embedded files, actions, metadata, hidden OCG) | ✓ | partial | ✓ | M | P1 |
| AcroForm fill | ✓ | ✓ | ✓ | M | P1 |
| Form designer | ✓ | PDF4QT (basic) | ✓ | L | P1 |
| Form JS (calc / format / validate) | ✓ full JS | partial | native AF* functions; full JS opt-in later | L | P2 |
| XFA | ✓ (legacy) | ✗ | **out of scope**: detect, warn, use AcroForm fallback | — | — |
| Signature verification (PAdES, chain, OCSP/CRL, DocMDP) | ✓ | Okular (basic) | ✓ incl. attack-class checks | L | P1 |
| Signing (PKCS#12, PKCS#11 tokens, OS stores, TSA, LTV) | ✓ | partial | ✓ B-B → B-T → B-LT → B-LTA | XL | P1/P2 |
| Visual / typed / image "signatures" | ✓ | ✓ | ✓, labelled clearly as non-cryptographic | S | P1 |
| OCR (searchable layer, deskew, orientation) | ✓ | OCRmyPDF (CLI) | ✓ Tesseract, background | L | P1 |
| Optimisation | ✓ | qpdf / Ghostscript | ✓ size attribution + presets; lossy only on request | L | P2 |
| Preflight PDF/A, PDF/X | ✓ | veraPDF (PDF/A, UA) | own checks + optional veraPDF | XL | P2 |
| Accessibility audit / tag editor | ✓ | veraPDF (UA) | ✓ | XL | P2 |
| Metadata (Info + XMP) | ✓ | ExifTool | ✓ | S | P1 |
| Attachments / portfolios | ✓ | partial | attachments ✓, portfolios read-only | M | P2 |
| Conversion Office/HTML → PDF | ✓ | LibreOffice | delegate to external LibreOffice / browser | S | P3 |
| PDF → images / text | ✓ | ✓ | ✓ | S | P1 |
| PDF → Word / Excel | ✓ | weak | out of scope initially | XL | P3 |
| CLI | ✗ (Actions) | ✓ per tool | ✓ unified, shares core | M | P0 |
| Pipelines / batch | Action Wizard | scripts | JSON pipelines over the same ops | M | P2 |
| Plugins | ✓ (C SDK) | ✗ | out-of-process boundary | L | P3 |

---

## D. Technology evaluation

### D1. PDF libraries and dependency evaluation

| Library | License | Role in our design | Notes |
|---|---|---|---|
| **PDFium** | BSD-3 / Apache-2.0 | **Adopt:** rendering, text geometry (FPDFText char boxes), annotation and form rendering, form interaction | Chromium-maintained and fuzzed continuously (OSS-Fuzz). Best real-world compatibility. **Not thread-safe:** one instance per process. Build without V8 or XFA. Its editing write-back is lossy (whole-stream regeneration, drops `Tc`/`Tw`, converts `cs/scn` to device colours) → never used for saving. Use pinned prebuilt binaries (bblanchon/pdfium-binaries) with checksums; building from source needs depot_tools. |
| **MuPDF** | AGPL-3.0 / commercial | **Rejected** (license) | Technically the most complete: real redaction, signing, content filtering, structured text. Would force the project to AGPL. |
| **Poppler** | GPL-2/3 | **Rejected** (license) | Good renderer; signing via NSS. |
| **qpdf** | Apache-2.0 | **Test oracle** (`--check`, `--json`); possible later runtime use for linearisation | Superb structural library. Doesn't write incremental updates, which is a core need. Duplicates our COS layer. |
| **PDF4QT engine (Pdf4QtLibCore)** | MIT | **Reference / port source**, not linked | A complete C++ engine (parser, writer, signatures via OpenSSL, redaction, optimiser, compare). Not linked: its core depends on Qt types, it's C++ hostile-input parsing, and it has a single maintainer. Port algorithms (e.g. optimiser, compare, signature handling) into Rust with attribution. |
| **PoDoFo** | LGPL-2.0 | Not used | Object-level and incremental, with signing; LGPL and C++ hostile-input parsing; small maintainer base. |
| **Apache PDFBox** | Apache-2.0 | Reference only | Most complete permissive engine, but JVM. Excellent reference for signing and preflight logic. |
| **PDF.js** | Apache-2.0 | Reference + test corpus | Browser engine; its test corpus is valuable. |
| **lopdf** (Rust) | MIT | Reference / possible fork source | Eager whole-document load (bad for 500 MB), weak recovery, no byte-span retention for lossless content round-trips. |
| **hayro / hayro-syntax** (Rust) | Apache/MIT | **Evaluate in M0** as the COS parser foundation; watch hayro as a future pure-Rust renderer | v0.7 (June 2026), >1000-file regression suite. Lazy parser. Renderer not yet performance-tuned; lacks encryption, blending and knockout groups. |
| **pdf-writer / krilla** (Rust) | MIT/Apache | Possible helper for emitting new objects and fonts | Typst's PDF stack. |
| **Ghostscript** | AGPL | Optional **external executable** only (user-installed), never linked | Useful for some PDF/X and PS conversions. |
| **Cairo / Skia** | LGPL/MPL / BSD | Not needed | PDFium has its own rasteriser (Skia backend optional). The Qt canvas composites tiles. |
| **FreeType** | FTL / GPL-2 (dual) | Inside PDFium (take the FTL option; credit in docs) | |
| **HarfBuzz / harfrust** | MIT | Shaping for *new* text we lay out (text boxes, FreeText, edits) | harfrust is the Rust port. |
| **skrifa / read-fonts / ttf-parser; subsetter** | MIT/Apache | Font metrics, glyph coverage, subsetting for edits | Pure Rust; parses hostile embedded fonts safely. |
| **lcms2** | MIT | Colour management for preflight (inside PDFium already) | |
| **libjpeg-turbo, OpenJPEG, libpng, zlib** | BSD-style / BSD-2 / libpng / zlib | Inside PDFium; Rust equivalents (`zune-jpeg`, `png`, `flate2`/zlib-rs) in our core | JPX decoding is a known CVE hotspot → sandbox. |
| **Tesseract 5** | Apache-2.0 | **Adopt** for OCR (C API, sandboxed worker) | Mature; LSTM models are Apache. |
| **OCRmyPDF** | MPL-2.0 | Reference design (hOCR → invisible text layer) | Python + Ghostscript, so we don't bundle it. |
| **PaddleOCR / docTR via ONNX Runtime** | Apache | Possible later OCR provider (plugin) | Heavy runtime; higher accuracy on hard scans. |
| **LibreOffice** | MPL-2.0 | Optional external `soffice --headless` for Office → PDF | Don't build a converter. |
| **ExifTool** | GPL / Artistic (Perl) | Not bundled | XMP via our own code or Adobe XMP Toolkit (BSD) bindings. |
| **OpenSSL 3** | Apache-2.0 | **Adopt** for CMS/PKCS#7, X.509 chain validation, CRL, OCSP, RFC 3161 timestamps | Most mature CMS and OCSP implementation. RustCrypto (`cms`, `x509-cert`) is used for parsing and inspection where pure Rust is enough. |
| **PKCS#11 (`cryptoki`), Windows CNG, macOS Keychain** | Apache/MIT; OS | Hardware tokens and OS certificate stores for signing | PKCS#11 *is* the extension point for HSMs and smartcards. |
| **veraPDF** | MPL-2.0 / GPL-3 (dual) | Optional external validator for PDF/A and PDF/UA (Java) | Industry reference. Use for CI conformance tests too. |
| **Arlington PDF Model** | Apache-2.0 | Machine-readable PDF 2.0 object model → drives Inspector validation and typed accessors | PDF Association. |
| **Qt 6 (LTS, 6.8+)** | LGPLv3 / GPL / commercial | UI shell, dynamically linked | Use only LGPL modules. Avoid GPL-only add-ons (e.g. KDDockWidgets is GPL/commercial; use QDockWidget). |
| **cxx** (Rust↔C++) | MIT/Apache | In-process boundary between the Qt shell and the Rust client library | Stable and widely used. **Prefer cxx over cxx-qt** (v0.10, pre-1.0). We don't need Rust-defined QObjects. |

### D2. Language

| Option | Memory safety on hostile input | Ecosystem fit | Contributor pool | Verdict |
|---|---|---|---|---|
| **Rust (core)** | ✓ | Good PDF/font/crypto crates; cargo-fuzz; cargo-deny license policy | Large and growing | **Chosen** for everything we write that parses or rewrites PDF bytes |
| **C++20 (shell)** | ✗ | Qt is native C++ | Large | **Chosen** for the UI only. The UI never parses PDF data. |
| C | ✗ | — | — | No |
| Java / Kotlin (PDFBox) | ✓ | Strong PDF libs | Large | JVM footprint and startup contradict the performance goal |
| Go (pdfcpu) | ✓ | Decent | Medium | Weak desktop UI and C++ interop story |

### D3. UI toolkit

| Option | Pro-desktop density / docking | Accessibility | Canvas / GPU | Native file handling | License | Verdict |
|---|---|---|---|---|---|---|
| **Qt 6 Widgets + QRhiWidget canvas** | ✓✓ | ✓ mature (UIA, NSAccessibility, AT-SPI) | ✓ (RHI: D3D11/12, Metal, Vulkan, GL) | ✓ | LGPLv3 | **Chosen** |
| Qt Quick / QML | ✓ | ✓ | ✓✓ | ✓ | LGPLv3 | Weaker for dense, tool-heavy desktop UIs; can host the canvas later if needed |
| GTK 4 | ✓ on Linux | good on Linux, weak elsewhere | ✓ | weak on Windows/macOS | LGPL | Cross-platform quality gap |
| Tauri (webview) | web-style | ✓ (browser a11y) | tiles via IPC/canvas; extra copies | ✓ | MIT/Apache | Webview UI tends to drift into "SaaS dashboard"; WebKitGTK inconsistency on Linux |
| Flutter | custom | improving | ✓ | partial | BSD | Non-native feel, large engine, weak desktop text/a11y maturity |
| Slint / egui / iced | limited | AccessKit (improving) | ✓ | partial | GPL-or-commercial / MIT | Not yet at professional density, docking or native-dialog parity |
| Native per platform | ✓✓ | ✓✓ | ✓ | ✓ | — | 3× UI cost; impossible solo |

### D4. OCR

| Engine | License | Notes | Verdict |
|---|---|---|---|
| **Tesseract 5** | Apache-2.0 | 100+ languages, OSD orientation detection, hOCR/TSV with word boxes | **Default** |
| PaddleOCR / docTR (ONNX) | Apache | Better on hard layouts; heavy runtime | Later provider |
| EasyOCR | Apache | PyTorch | No |

### D5. Cryptography

OpenSSL 3 (CMS, OCSP, TSA, X509_STORE with CRL) is the backend for signing and verification. RustCrypto (`sha2`, `aes`, `rc4`, `md5`) handles PDF encryption (Standard Security Handler R2–R6). Platform key stores and `cryptoki` provide keys.

**We implement:** the PDF-side logic (ByteRange, placeholder reservation, DSS/VRI, DocMDP/FieldMDP evaluation, modification-after-signing analysis).
**We never implement:** crypto primitives, ASN.1 signature math, or chain building.

### D6. IPC and build

| Concern | Choice |
|---|---|
| UI ↔ engine IPC | Rust on both ends. The Qt app links a Rust **client** static library via cxx; the protocol is `serde` + `postcard` over pipes, versioned. This avoids maintaining a cross-language schema. |
| Bulk data (tiles) | Shared memory (named sections / memfd) with a slot ring; IPC carries handles only. |
| Build | Cargo workspace + CMake for the app (Corrosion to drive Cargo from CMake). vcpkg or the Qt online installer for Qt in CI. |

---

## E. PDF engine recommendation (deep)

### E1. These are different problems

| Capability | What it actually requires | Who does it |
|---|---|---|
| **Rendering** | Interpret content streams and fonts; tolerate broken files | PDFium |
| **Parsing** | Tokenise, resolve the xref (tables, streams, hybrids, repair), decrypt, decode filters, resolve objects lazily | **Ours** (`cos`) |
| **Object editing** | Mutate objects while tracking revision, generation and identity | Ours |
| **Content editing** | Tokenise content streams *keeping byte spans*, interpret state (CTM, text matrix, fonts) to compute glyph and object bboxes, and patch operator ranges | Ours (`content`) |
| **Layout reconstruction** | Glyphs → words → lines → blocks → reading order (heuristic, never exact) | Ours (geometry from our interpreter; PDFium text used as a cross-check) |
| **Incremental update** | Append changed objects + new xref section + trailer `/Prev`. Original bytes untouched. **The only save that preserves existing signatures.** | Ours |
| **Full rewrite** | Serialise a reachable object graph, garbage-collect, optionally renumber, object streams, linearise | Ours (qpdf as oracle; linearisation later) |
| **Object identity** | Keep object numbers stable across incremental saves (required for signatures, structure tree refs, annotation `/P` and `/IRT` links) | Ours |
| **Preserving fonts** | Never re-encode existing text; edits reuse the embedded subset if glyphs exist, else add a new font resource | Ours |
| **Preserving appearance** | Never regenerate untouched streams. Annotation and field appearance streams are regenerated only for changed items. | Ours |
| **Preserving signatures** | Incremental only. DocMDP/FieldMDP-aware warnings before saving. | Ours |

**Rendering ≠ safe editing.** PDFium proves the point: it renders superbly, but its write-back rewrites every operator in an edited stream.

### E2. Options assessed

- **A. Build entirely on PDFium.** Fast start, but there's no faithful editing, no real redaction and no signing. It would lock in fidelity loss. ✗
- **B. Our own document model on one engine.** Necessary, but one engine can't be both renderer and faithful writer under our license constraint.
- **C. Combine specialised engines.** PDFium renders; our core writes. ✓
- **D. Own the editing pipeline.** ✓ This is where the differentiation and the risk both live.

**Recommendation: C + D, with a hard rule.** `cos` is **the authority** on document structure and is the only writer. PDFium is a pure function: (document bytes revision) → pixels, glyph geometry, form interaction state.

### E3. How the two engines stay consistent

- PDFium loads the **current in-memory revision** through `FPDF_FILEACCESS`. That revision is the original file (memory-mapped) followed by the pending incremental sections we've serialised. After a commit, the engine re-opens the revision. That's cheap: lazy xref, and only the invalidated pages re-render.
- **Edit previews** composite in the UI before the commit, so typing never waits for a re-open.
- **Differential parsing** is both a security and a fidelity risk, because two parsers may see different documents. Mitigations:
  - On open, cross-check page count, page-tree object refs and per-page `/Contents` refs between `cos` and PDFium.
  - On mismatch, or if either side needed repair, mark the document **Repaired**. Saving it forces a full rewrite of the normalised `cos` view, which PDFium then re-loads.
  - Signature validation uses only `cos` with strict rules.
- **Long term (D):** if hayro matures, it becomes an optional pure-Rust renderer. That removes the dual-parser problem and the C++ attack surface. No commitment now.

### E4. Build vs adopt for `cos`

The M0 spike evaluates `hayro-syntax` against a fresh implementation. Criteria:
1. lazy, mmap-friendly object access
2. raw byte-span retention for streams and content tokens
3. xref repair quality on the malformed corpus
4. ability to support a revision-aware writer
5. license and maintainer health

Record the outcome as an ADR. A fork or upstream contributions are acceptable; we must not be blocked on an upstream API.

---

## F. Architecture

### F1. Process architecture

```
┌──────────────────────── UI process (C++ / Qt 6, unsandboxed, owns user FS access) ───────────────────────┐
│  Shell: windows, modes, panels, canvas (QRhiWidget), command registry, palette, a11y bridge               │
│  ↕ cxx (in-process)                                                                                        │
│  engine-client (Rust staticlib): typed API, IPC, shared-memory tile cache, view-model data (no PDF parsing)│
└──────────────▲───────────────────────────────────────────────────────────────────────────────────────────┘
               │ pipes (postcard msgs) + shared memory (tiles) + passed file handles
┌──────────────┴──────── Engine process, one per document (Rust host; sandboxed; no FS/network) ───────────┐
│  Session: cos ObjectStore + revisions + history       Job scheduler (priority, cancellation, deadlines)    │
│  content / layout / ops / diff / inspect               PDFium (single thread per process)                  │
│  Watchdog: memory cap (job object / rlimit / cgroup), per-job timeouts → crash isolated, auto-restart      │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────┘
   Extra render processes (read-only, same revision via shared memory) for parallel tiles — later.
   OCR worker (Tesseract) — separate sandboxed process (Phase 8).
   Signing — key operations in the UI/broker process (needs OS stores / PKCS#11); engine only prepares the
   ByteRange digest and embeds the returned CMS blob.
CLI: links core crates in-process (memory-safe Rust); commands that need PDFium spawn the engine host.
```

**Why this shape:**
- The memory-safety boundary (Rust vs C++) lines up with the trust boundary (engine vs UI).
- PDFium's lack of thread safety is handled by processes rather than locks.
- A renderer crash costs one page's tiles, not the user's session.
- The UI never touches untrusted bytes, so the a11y bridge, file dialogs and key stores live outside the sandbox.

### F2. Rust crate boundaries

Start with **few crates** and split only when a boundary is proven:

| Crate | Responsibility | Depends on |
|---|---|---|
| `cos` | Lexer, parser, xref (table/stream/hybrid/repair), lazy ObjectStore, filters, decryption, writer (incremental + full), resource limits | — |
| `content` | Content-stream tokeniser with byte spans, graphics-state interpreter, page objects with provenance, `ContentPatch` application, glyph geometry | `cos`, font crates |
| `doc` | Typed views over COS (Catalog, PageTree, Page, Annot, Field, Outline, Font, XObject), transactions → ChangeSets, history, save planner | `cos`, `content` |
| `ops` | Serialisable `Operation`s (page ops, annotation ops, edit ops) applied as transactions. **The same ops back the GUI commands, the CLI and future pipelines.** | `doc` |
| `inspect` | Object browser data, size attribution, feature and security analysis | `doc` |
| `diff` | Page alignment, text diff, change model (later: visual and object diff) | `doc`, `content` |
| `render` | Safe PDFium wrapper. Only linked into the engine host. | PDFium |
| `engine` | Engine host binary: IPC server, scheduler, sandbox entry, watchdog | all of the above |
| `ipc` + `engine-client` | Protocol types; client staticlib + cxx bridge for Qt | `ipc` |
| `cli` | `vellora` binary | `ops`, `inspect`, `diff`, `engine` (spawn) |

**No god object.** `Document` is a thin session handle. Semantics live in *views* computed over COS objects. Views are never parallel copies that must be kept in sync.

### F3. UI architecture

- **Command registry.** Each action is `{id, title, shortcut, context predicate, handler}`. Menus, toolbars, shortcuts, the command palette and customisable keymaps (JSON) all derive from it. Command ids match `ops` names wherever they correspond, so GUI, CLI and pipelines speak one vocabulary.
- **Modes** (View / Organize / Annotate / Edit / Review / Redact / Forms) scope the tools and prevent accidental destructive edits.
- **Layout:** native menu bar, one contextual toolbar for the active mode, left panel (Pages / Outline / Annotations / Attachments / Layers), right properties panel for the current selection, status bar (page, zoom, save impact).
- **Organize** is a full-window page grid, not a sidebar. That scales to multi-select of hundreds of pages.
- **Canvas:** GPU-composited tiles (QRhiWidget). Placeholders appear immediately at the correct page size. Progressive resolution: thumbnail-scaled first, then full tile.
- **Accessibility of the app:** Qt widgets cover most of it. The canvas gets a custom `QAccessibleInterface` exposing page text in reading order, links, annotations and form fields. Also: high-contrast palette, honour system font scaling and reduced motion, and full keyboard reachability. Screen-reader passes on NVDA, VoiceOver and Orca are part of release criteria.

---

## G. Data / document model

```
ByteSource          immutable original (mmap or read handle); never modified while open
  └─ Revision chain   original revisions (from the file's own incremental history; inspectable)
       └─ Pending ChangeSets   our edits, in memory: {new objects, replaced objects, freed refs}
            └─ Views (lazy, cached, keyed by (objref, change-epoch)): PageTree, Page, PageObjects, TextLayout, Annots…
```

- **Non-destructive by construction.** The base file is never mutated. Edits are ChangeSets, an overlay of object replacements. This gives us "show original", "revert", diff-against-original and undo for free.
- **Transactions:**
  - `ops::Operation` → `doc.transact(|tx| …)` → `ChangeSet`.
  - Undo pops a ChangeSet. Redo re-applies it.
  - History is linear per session.
- **Content edits** are `ContentPatch`es: (stream objref, operator range) → replacement operators. Applying one produces a new stream object.
  - Untouched operators keep their **original bytes**: we splice spans, we don't re-serialise.
  - New content goes into a separate appended stream wrapped in `q … Q` wherever possible, so the original stream isn't touched at all.
- **Page objects with provenance.** The interpreter produces `TextRun | Image | Path | Shading | FormXObject` items, each carrying bbox, CTM, graphics state and `(stream, op range, nesting path)`. Selection and hit-testing use these. Edits target provenance.
- **Save planner** → `SaveImpact { mode: Incremental | FullRewrite, reasons[], signatures: {intact, permitted-change, invalidated}, recoverable_content: bool, lossy_steps[] }`. The UI shows this before saving; the CLI prints it, and `--require-incremental` turns any downgrade into an error.
- **Saving:** write to a temp file in the same directory, fsync, atomic rename. Incremental output = copy of the original + appended section (an in-place append optimisation can come later). Never overwrite the source until the new file is complete and passes a self-check (re-parse, page count).
- **Review state** (diff "reviewed" marks, dismissals): kept in a sidecar `*.pdfreview.json` by default, so comparing two documents never modifies either. It can be exported as standard annotations in the new PDF. Comments are always standard PDF annotations (`/IRT` replies, `/State`/`/StateModel` for status), so they stay portable to Acrobat.

### G1. Fidelity matrix (what "editing" means)

"Sig" = existing signatures. Incremental saves keep signed bytes cryptographically intact; whether the change is *permitted* depends on DocMDP/FieldMDP.

| Operation | Save | Untouched content | Fonts | Annots / forms | Sig | User-facing warning |
|---|---|---|---|---|---|---|
| Add / modify annotation, reply, status | Incr. | byte-identical | — | preserved | intact; permitted if P=3 | — |
| Fill form field | Incr. | byte-identical | AP may add font | preserved | intact; permitted if P≥2 | — |
| Rotate page | Incr. (`/Rotate`) | byte-identical | ✓ | ✓ | flagged as modification | "Changes signed document" |
| Reorder / insert / delete pages | Incr. (page tree) | byte-identical | ✓ | ✓ (annots move with page) | flagged | **Delete:** "Deleted pages remain recoverable — use Save Clean" |
| Import / merge pages | Incr. or new file | deep-copied resources, deduped | ✓ | annots kept; imported signature values stripped | flagged | form-field name collisions resolved explicitly |
| Crop | Incr. (`/CropBox`) | content kept | ✓ | ✓ | flagged | "Cropping hides, does not remove" |
| Add text / image | Incr., new appended stream | byte-identical | new font subset | ✓ | flagged | — |
| Move / resize / delete image or path | Incr., one stream rewritten by splice | other operators byte-identical | ✓ | ✓ | flagged | Delete: recoverable unless Save Clean |
| Edit existing text | Incr., local splice | identical elsewhere | reuse subset, or add a font if glyphs are missing (shown) | ✓ | flagged | "Font substituted" when it happens |
| Redact | **Full rewrite, mandatory**, all prior revisions dropped, GC | touched images re-encoded | subsets may shrink | annots/fields in area removed | **all removed** | Review screen + verification report |
| Sanitise | Full rewrite | per option | ✓ | per option | removed | itemised list |
| Optimise | Full rewrite | lossless by default; lossy only per preset | subset / dedupe | ✓ | removed | before/after size and quality preset |
| Sign | **Incremental, mandatory** | byte-identical | ✓ | ✓ | prior sigs intact | — |
| OCR | Incr., new invisible-text stream per page | byte-identical | GlyphLess font added | ✓ | flagged | — |
| Metadata edit | Incr. | ✓ | ✓ | ✓ | flagged | — |
| Save Clean (explicit) | Full rewrite, GC | visually identical | ✓ | ✓ | removed | "Removes signatures and edit history" |

### G2. Direct text editing: tiers and honest degradation

| Tier | Capability | Technique | Limits shown to the user |
|---|---|---|---|
| 0 | Overlay (annotation / FreeText) | Standard annotations | Always available |
| 1 | Add text / images | New appended content stream; harfrust shaping; subset embed | — |
| 2 | Move, resize or delete existing objects | Operator-range splice; wrap in `q cm … Q`; remove ranges; split `TJ` arrays when deleting part of a run | Objects inside patterns, shadings or Type3 glyph procs are not selectable |
| 3 | Edit a text run in place (single line) | Re-encode with the run's font. If glyphs are missing from the subset: (a) embed a full matching font if installed and fsType allows, else (b) substitute and flag. Adjust following glyphs on the same line. | Badge on edited runs whose font changed |
| 4 | Edit within a reconstructed block (multi-line reflow) | Layout analysis → block → re-lay-out only inside the block's bounds | Only blocks detected with high confidence; otherwise offer Tier 3 |
| ✗ | Not editable | Text drawn as paths, text in images (→ offer OCR), Type3 bitmap fonts, missing ToUnicode with no recoverable mapping | Clear explanation in the properties panel |

---

## H. Security model

### H1. Threats → mitigations

| Threat | Mitigation |
|---|---|
| Parser / renderer memory corruption (PDFium, FreeType, OpenJPEG, libjpeg) | Engine process: no filesystem (the broker passes handles), no network, restricted token. Per-platform sandbox: **Windows** AppContainer + Job object; **Linux** seccomp-bpf + Landlock + namespaces (Flatpak portal compatible); **macOS** sandbox profile for the helper. Crash → auto-restart and re-open. Track PDFium releases monthly. |
| Our own parser bugs | Rust, `#![forbid(unsafe_code)]` in `cos`/`content`/`doc`, fuzzing in CI and nightly, later OSS-Fuzz. |
| Decompression bombs, memory exhaustion | Decode limits: max decoded size per stream, max total, ratio cap. Image dimension caps. Job-object / cgroup memory cap on the engine. Streaming decode where possible. |
| CPU exhaustion (deep nesting, huge content streams, xref loops) | Recursion and depth limits; reference-cycle detection; per-job deadlines with cancellation; watchdog kills runaway renders. |
| Malicious fonts / images | Parsed only inside the sandbox (PDFium) or by memory-safe Rust crates (our font handling). |
| JavaScript | PDFium built **without V8**. Form calculations use native implementations of Acrobat's AF* functions. JS actions are surfaced in the security report and never run. |
| Launch / GoToR / ImportData / SubmitForm / URI actions | Launch never executes. URIs need explicit confirmation showing the full URL. SubmitForm is disabled by default. Remote GoTo prompts. |
| Embedded files, path traversal | Never auto-open. Save via a save dialog only. Filenames sanitised (strip separators, reserved names, NTFS ADS, control chars). Never execute. |
| External references (remote images/fonts, XFA fetches, OCSP/TSA during verification) | No network by default for document content. Revocation checks are explicit user actions or an opt-in setting; the network goes through the broker, never the engine. |
| Signature spoofing (USF, ISA, SWA, shadow attacks; Ruhr-Uni Bochum "PDF Insecurity" research) | Strict ByteRange (must cover the whole file except the Contents hole; single hole). Re-parse the signed revision independently and **render the signed revision vs the current one**. Classify post-signature changes against DocMDP/FieldMDP. Reject overlapping or unsigned trailing objects that change visible content. Show "valid but modified after signing" distinctly. |
| Visual "signature" confusion | Visual and typed signatures are labelled as appearance only; cryptographic status is shown separately. |
| Redaction leaks (text under boxes, prior revisions, metadata, OCR layer, form values, bookmarks/outline text, structure-tree ActualText/Alt, thumbnails, attachments) | Redaction is a full rewrite plus GC plus scrubbing of those locations. Post-apply verification: independent extraction (PDFium text + our interpreter + image-pixel check) in each redacted region must return nothing. Report shown. |
| Supply chain | `cargo-deny` (licenses + advisories), `cargo-vet` or audits for new crates, pinned PDFium binaries with SHA-256, SBOM (CycloneDX) per release, reproducible builds as a goal, signed releases. |
| Untrusted plugins (future) | Out of process only, under the same sandbox and broker-mediated capabilities. No in-process native plugins. |

---

## I. Testing strategy

| Layer | What | Tooling |
|---|---|---|
| Unit | Lexer, xref, filters, encryption vectors (R2–R6), writer, ops, diff algorithms | `cargo test`, `proptest` (parse(write(x)) == x) |
| Invariants (golden) | For every op × corpus document: output passes `qpdf --check`; page count is as expected; **untouched pages render pixel-identical before vs after**; the original-bytes prefix is byte-identical after an incremental save; extracted text is unchanged outside edit regions; existing signatures still validate after an incremental save | Golden harness in `tests/` over corpus manifests |
| Rendering regression | Render corpus pages at fixed DPI with pinned PDFium; compare to approved PNGs with a perceptual threshold; review UI for diffs | Same harness; updated on PDFium bumps |
| Integration | CLI end-to-end; engine IPC; crash/restart; cancellation | `assert_cmd`; engine test client |
| UI | Command registry, keyboard reachability, a11y tree snapshots | Qt Test; manual screen-reader checklists per release |
| Fuzzing | Lexer, xref repair, object parser, content tokeniser, decrypt, writer round-trip, ops on fuzzed docs; differential (cos vs qpdf object graph) | `cargo-fuzz` (smoke in CI, long runs nightly); apply to OSS-Fuzz once public |
| Security | Known-malicious and CVE reproducer corpus; bomb tests; signature attack test sets; redaction leak tests | Kept in a separate, restricted corpus repo |
| Performance | Open time, first-page time, scroll frame times, memory high-water on large docs | Benchmark harness + CI trend tracking (fails on >10% regression) |
| Conformance (later) | PDF/A and PDF/UA outputs validated by veraPDF; Isartor / veraPDF corpora | CI job |

**Compatibility corpus.** Real documents, not just synthetic ones, in a **separate repository**, fetched by manifest (URL + SHA-256 + license + category).
- **Public sources:** pdf.js test corpus, PDFium test corpus, DARPA SafeDocs corpora, GovDocs1 sample, veraPDF / Isartor suites, PDF Association samples, Arlington test files.
- **Categories:** forms (AcroForm/XFA), scans, contracts, textbooks, image-heavy, many-fonts, many-annotations, encrypted, signed (incl. PAdES-LTV), attachments, malformed, producers (Word, LibreOffice, InDesign, LaTeX, Chrome, scanners, Acrobat).
- **Synthetic generators** for scale: 10k-page, 500 MB image-heavy, 50k annotations.

---

## J. MVP (exact scope)

**Thesis demonstrated:** fast on huge files; faithful saves you can see; safe against hostile PDFs; review/compare that nobody else open-source offers.

**In scope:**
1. **Viewer:** open (incl. encrypted with password), tiled async rendering, continuous / single / two-up, zoom/fit, thumbnails, outline, internal and external links (confirmed), page jump, rotate view, dark-UI with true-colour page.
2. **Text:** selection, copy; search with case, whole word, regex; results panel; streaming over all pages.
3. **Organize mode:** multi-select (ranges such as `3,7,9-14`), drag reorder, rotate, delete, duplicate, insert blank, import pages from another PDF, extract/export selection, merge, split.
4. **Annotations:** highlight, underline, strikeout, ink, rectangle/ellipse/line/arrow, FreeText, sticky note with threaded replies, author/date, status (Accepted/Rejected/Completed via `/StateModel`); annotations panel with filter; generated appearance streams.
5. **Object editing (Tiers 1–2):** select page objects with bbox outlines; add text box and image; move/resize/delete images and paths; delete text runs. In-place text editing is **not** in the MVP; it's the first post-MVP phase.
6. **Save:** incremental by default, **Save Clean**, Save As; save-impact bar; atomic writes; full undo/redo.
7. **Inspector (read-only):** document summary, object tree, fonts (embedded/subset/type), images (size, DPI, filter), **size attribution**, annotations, attachments list, metadata, encryption, revision history, signatures listed (unverified status shown as such).
8. **Security report:** JS, launch/URI/submit actions, embedded files, external references, OCG hidden content, incremental-update count, repair status.
9. **Compare (foundation):** page alignment (handles inserted, deleted and moved pages); word-level text diff with normalisation (whitespace, hyphenation, ligatures); change categories (added / removed / modified / **number changed**); synced side-by-side view; change navigator (prev/next, `12/17`); comment on a change (creates an annotation); mark reviewed/dismissed (sidecar).
10. **CLI:** `inspect`, `security`, `merge`, `split`, `extract`, `rotate`, `delete-pages`, `compare --format text|json`, `render`. `--json` everywhere; non-zero exit codes are meaningful.
11. **Platforms:** Windows 10/11 and Linux (Flatpak) at MVP; macOS builds in CI with release at MVP+1 (signing and notarisation cost and effort; see N).

**Out of MVP:** in-place text editing, redaction, sanitisation, forms, signatures (sign or verify), OCR, visual or object diff, optimisation, preflight, accessibility tooling, conversion, pipelines, plugins.

**Performance targets (MVP acceptance, mid-range laptop, SSD):**

| Metric | Target |
|---|---|
| App start → window ready | < 400 ms (cold < 1 s) |
| Open 10-page text PDF → first page sharp | < 300 ms |
| Open 10,000-page PDF → first page | < 1.5 s, no full parse |
| Open 500 MB image-heavy PDF → first page | < 2 s; RSS bounded by tile-cache budget |
| Scroll | 60 fps compositing; placeholders never block; sharp tiles typically < 100 ms |
| UI thread | no task > 16 ms (debug watchdog asserts) |
| Idle memory, 1 doc open (excl. tile cache, default cap 256 MB) | < 150 MB total across processes |
| Search 1,000-page text doc | first hit < 200 ms; full scan < 3 s, streaming |
| Page reorder on 5,000-page doc | grid interaction instant; save incremental < 2 s |
| Compare two 100-page contracts | < 5 s to change list |

---

## K. Roadmap

Solo estimates are deliberately conservative. Every phase ends with a tagged pre-release.

| Phase | Objective | Key features | Depends on | Main risks | Tests | Acceptance |
|---|---|---|---|---|---|---|
| **0. Foundations** (~8–10 wk) | Walking skeleton + OSS infrastructure | see **O** | — | cos parser scope creep; PDFium packaging | fuzz targets, corpus open-rate, round-trip | see O |
| **1. Viewer** → v0.1 (~8 wk) | A daily-usable fast viewer | rendering modes, thumbnails, outline, links, selection, search, Inspector v1, security report, CLI `inspect/security/render` | 0 | tile scheduler tuning; a11y of canvas | perf benchmarks; rendering regression | Perf targets for viewing met; opens ≥ 98% of corpus; no UI crash on malicious set |
| **2. Organize + Save** → v0.2 (~6 wk) | Faithful structural edits | Organize grid, all page ops, import/merge/split, incremental + Save Clean, save-impact, undo | 1 | resource deep-copy on import (shared resources, name collisions, struct tree) | invariants: byte-identical prefix, pixel-identical pages, qpdf --check | All page ops on corpus pass invariants; CLI page ops |
| **3. Annotations & comments** → v0.3 (~8 wk) | Standard-conformant review markup | markup / ink / shapes / FreeText / notes / replies / status; appearance generation; annotations panel | 2 | appearance streams that look right in Acrobat and other viewers | cross-viewer render checks (PDFium + pdf.js + Acrobat manual) | Round-trips with Acrobat: replies, status, author preserved |
| **4. Object editing T1–2** → v0.4 (~10 wk) | Prove content-stream surgery | content interpreter with provenance; object selection; add text/image (harfrust + subsetting); move/resize/delete | 2 | interpreter correctness (Form XObjects, clipping, inline images) | untouched-operator byte identity; render diff limited to edit bbox | Edits on corpus never change pixels outside edit bbox |
| **5. Compare foundation** → **v0.5 = MVP** (~8 wk) | The defining feature, v1 | alignment, text diff, categories, navigator, sync view, sidecar review, CLI compare | 1, 3 | reading-order noise causing false diffs | labelled diff corpus (contract revisions with known changes); precision/recall | ≥ 95% recall / ≥ 90% precision on labelled set |
| **6. Text editing T3–4** (~12 wk) | In-place text editing | run editing, font reuse/substitution, block detection + local reflow | 4 | font subsets missing glyphs; layout heuristics | per-producer corpus (Word, LaTeX, InDesign) edit tests | Single-line edits pixel-faithful when glyphs exist; substitution always flagged |
| **7. Redaction + sanitisation** (~10 wk) | Real removal | mark / review / apply; text/image/vector/annotation/metadata scrub; verification report; sanitise profiles | 4 (glyph geometry, splice) | missed leak channels | leak test suite (each channel), independent extraction | Zero recoverable content on leak suite; external audit invited |
| **8. Forms** (~10 wk) | AcroForm fill + designer | fill (PDFium interaction, our AP + value write), AF* native calc/format/validate, designer, XFA detect / warn | 3 | appearance fidelity; calc order | form corpus round-trip in Acrobat | Corpus forms fill / save / reopen identically in Acrobat |
| **9. Signatures** (~14 wk) | Correct verify, then sign | verify (chain, OCSP/CRL, TSA, DocMDP, attack checks, signed-revision view); sign PKCS#12 / PKCS#11 / OS stores; B-B, B-T, then B-LT / B-LTA | 2, 8 | crypto/PKI edge cases; attack classes | PDF Insecurity test sets; ETSI plugtest-style samples; Acrobat validation interop | Validates and flags all known attack samples; our signatures valid in Acrobat |
| **10. OCR** (~8 wk) | Searchable scans | Tesseract worker, deskew, OSD, invisible layer, background queue, page selection | 1, 4 | quality/speed trade-offs; language packs | OCR corpus WER; layer alignment | Text layer selection aligns with image; UI responsive during OCR |
| **11. Compare v2 + review** (~8 wk) | Visual + object diff | pixel diff with AA tolerance → regions; image/annot/field/font/metadata diff; review export | 5 | false positives from rendering noise | labelled visual set | Change regions match labels; review export round-trips |
| **12. Optimisation** (~8 wk) | Explain and reduce size | size attribution → actions; dedupe; recompress; downsample presets; font subset; object streams | 2, Inspector | quality loss; breaking structure | SSIM thresholds; qpdf --check; veraPDF where applicable | Lossless preset never changes pixels |
| **13. Preflight + accessibility** (~16 wk+) | Professional validation | own rule checks (fonts, DPI, colour, boxes), veraPDF integration (PDF/A, PDF/UA), tag-tree viewer/editor, alt text, reading order, language | 4, 12 | sheer spec size | veraPDF corpora | Accurate reports vs veraPDF; fixes produce passing files |
| **14. Automation, conversion, plugins** | Ecosystem | JSON pipelines over `ops`; LibreOffice external conversion; out-of-process plugin protocol (OCR, validators, exporters) | all | API stability commitments | pipeline golden tests | Documented, versioned plugin protocol |

**v1.0** = Phases 0–10 plus macOS parity, plus the a11y and security audits.

---

## L. Repository structure

```
vellora/
├─ Cargo.toml                 # workspace
├─ CMakeLists.txt             # top-level: builds app/, drives cargo via Corrosion
├─ crates/
│  ├─ cos/  content/  doc/  ops/  inspect/  diff/
│  ├─ render/                 # safe PDFium wrapper (engine only)
│  ├─ engine/                 # engine host binary (IPC server, scheduler, sandbox, watchdog)
│  ├─ ipc/  engine-client/    # protocol + client staticlib with cxx bridge
│  └─ cli/
├─ app/                       # C++/Qt 6 shell (src/, resources/, i18n/, tests/)
├─ third_party/               # pinned binary manifests (pdfium.lock w/ SHA-256), license texts
├─ tests/                     # golden/invariant harness, corpus manifests (corpus itself in separate repo)
├─ fuzz/                      # cargo-fuzz targets
├─ bench/                     # perf harness + large synthetic doc generators
├─ docs/
│  ├─ architecture/           # overview, process model, data model, security model, fidelity matrix
│  ├─ adr/                    # NNNN-title.md (MADR format) — every decision in this plan becomes an ADR
│  ├─ rfcs/                   # feature proposals for large changes
│  └─ user/                   # user manual (mdBook)
├─ packaging/                 # flatpak manifest, WiX/MSIX, macOS dmg/notarisation scripts
├─ .github/
│  ├─ workflows/              # ci.yml, nightly.yml, fuzz.yml, release.yml
│  ├─ ISSUE_TEMPLATE/         # bug (with "attach PDF? privacy" guidance), feature, compatibility report, config.yml
│  ├─ PULL_REQUEST_TEMPLATE.md
│  └─ CODEOWNERS
├─ LICENSE (MPL-2.0)  NOTICE  THIRD_PARTY_LICENSES (generated by cargo-about + Qt/PDFium list)
├─ README.md  CONTRIBUTING.md  CODE_OF_CONDUCT.md (Contributor Covenant 2.1)  SECURITY.md
├─ GOVERNANCE.md  MAINTAINERS.md  CHANGELOG.md  deny.toml  rustfmt.toml  .clang-format  .clang-tidy
```

### L1. Licensing strategy

- **Project license: MPL-2.0.**
  - File-level copyleft keeps improvements to *our* files open.
  - It still permits proprietary plugins, embedding the core crates in other products, and commercial redistribution.
  - It's compatible with Apache/BSD/MIT dependencies and with LGPL Qt (dynamic).
  - It's GPL-compatible via §3.3, so GPL downstream projects can still use us.
  - Rejected alternatives: AGPL/GPL (rules out an ecosystem of embedders and proprietary plugins, which we chose not to restrict) and Apache-2.0 (would allow closed forks of the core files; acceptable, but MPL better fits "keep the workstation open").
- **Contributions: DCO sign-off** (`Signed-off-by`), not a CLA. That's friendlier for contributors; the trade-off is that relinquishing relicensing flexibility needs consensus (see N).
- **Dependency policy, enforced by `cargo-deny`:**
  - Allow: MIT, Apache-2.0, BSD-2/3, ISC, Zlib, MPL-2.0, Unicode-3.0, BSL-1.0, OFL-1.1 (fonts).
  - LGPL only for Qt, dynamically linked. Ship Qt as shared libraries so users can relink; no static Qt.
  - Deny: GPL, AGPL, SSPL, and "non-commercial" licenses.
  - AGPL/GPL tools (Ghostscript, ExifTool) only as optional, user-installed external executables invoked at arm's length; never bundled.
- **Proprietary plugins:** allowed by MPL and by the out-of-process protocol.
- **Downstream forks:** must keep modified MPL files open; new files can be under any license.

### L2. OSS process (in place from day one)

- **CONTRIBUTING:** build instructions per OS, DCO, Conventional Commits, PR size guidance, how to add a corpus file (license required), how to write an ADR / RFC, "good first issue" areas (Inspector checks, CLI commands, preflight rules, translations).
- **RFC process** for user-visible features or new dependencies; **ADR** for every architectural decision.
- **GOVERNANCE:** BDFL-for-now, with a written path to a maintainer council when ≥ 3 regular maintainers exist.
- **SECURITY.md:** GitHub private vulnerability reporting, 90-day disclosure, supported versions, separate restricted corpus for reproducers.
- **CI (GitHub Actions, Win/Linux/macOS):** rustfmt, clippy (`-D warnings`), clang-format / clang-tidy, tests, invariant harness on a corpus subset, cargo-deny, cargo-audit, fuzz smoke (60 s per target), ASan/UBSan build of the C++ shell, perf benchmarks (trend alerts).
- **Nightly:** full corpus run, long fuzzing, nightly builds published as pre-releases (signed).
- **Releases:** SemVer; release branches for stable; changelog generated from Conventional Commits and edited by hand; artifacts signed (Windows Authenticode, macOS notarisation, Linux via Flathub + checksums/minisign); CycloneDX SBOM attached.
- **Crash reporting (privacy-respecting):** minidumps (Breakpad/Crashpad-compatible, BSD/Apache) written **locally only**. After a crash the app offers to open the crash folder and a prefilled GitHub issue; the user chooses what to attach. Dumps from the engine process never contain document bytes beyond the stack; we document that. **No telemetry, no auto-upload, no update ping** unless the user opts into an update check.
- **Docs:** architecture docs kept current as part of review ("docs or it didn't happen" for boundary changes); user manual; CLI reference generated from clap.

---

## M. Major technical risks (ranked)

| # | Risk | Prob. | Impact | Mitigation |
|---|---|---|---|---|
| 1 | Solo-developer scope: phases slip, MVP never ships | High | Critical | Strict phase gating; each phase ships a usable release; MVP excludes T3 editing, forms, signatures and OCR; contributor-friendly boundaries (CLI, Inspector checks, rules) |
| 2 | Content interpreter / surgery correctness across real-world producers | High | High | Pixel-identical-outside-bbox invariant over a large corpus; differential checks against PDFium text geometry; refuse edits on low-confidence objects rather than corrupt |
| 3 | In-place text editing quality (subset fonts, missing glyphs, layout) | High | High | Tiered design; honest flags; ships after MVP; per-producer test sets |
| 4 | Differential parsing between `cos` and PDFium | Medium | High | Open-time cross-check; Repaired mode forces normalised rewrite; signatures validated only via `cos` |
| 5 | Redaction leaks via an overlooked channel | Medium | Critical | Mandatory full rewrite; channel checklist; independent verification; leak test suite; external review before 1.0 |
| 6 | Signature validation subtleties / attack classes | Medium | Critical | Delegate crypto to OpenSSL; implement against published attack research; interop with Acrobat; verify-only before signing |
| 7 | `cos` parser scope (xref repair, encryption, edge cases) consumes Phase 0 | Medium | High | Evaluate hayro-syntax first; time-box; qpdf as oracle; PDFium still renders files `cos` repairs poorly (warn, read-only) |
| 8 | Per-platform sandboxing complexity | Medium | Medium | Ship process isolation + resource limits first; harden per OS incrementally; evaluate existing crates (e.g. birdcage) before writing our own |
| 9 | Re-open-after-commit latency on huge files | Medium | Medium | Measure in M0; UI-side previews; batch commits; keep PDFium's doc open and add revision-aware reload only if needed |
| 10 | Rust↔C++ boundary friction (build, debugging, contributors) | Medium | Medium | Narrow cxx API; UI is a client of an RPC-shaped API; Corrosion build; documented dev setup scripts |
| 11 | PDFium API churn / binary availability | Low–Med | Medium | `render` crate isolates it; pinned versions; can build from source if prebuilt stops |
| 12 | Externally modified file while mmapped (SIGBUS, network drives) | Medium | Medium | Read-handle-based ByteSource with change detection; copy-on-open for network/removable volumes |
| 13 | Qt LGPL compliance on macOS / Windows packaging | Low | Medium | Shared Qt libs, relinking notes in docs, license screen |
| 14 | Name / trademark conflict | High (current name) | Medium | Choose a name before the public repo |

---

## N. Open questions (need your decision)

1. ~~Project name~~ → **Vellora** (decided). Still to do before going public: a formal trademark search; reserving the crates.io name with a placeholder publish is your call.
2. **DCO vs CLA.** I recommend DCO. A CLA would preserve future relicensing (e.g. dual licensing) at the cost of contributor friction.
3. **Form JavaScript.** Confirm "native AF* functions only, no JS engine" for 1.0. A sandboxed JS engine (e.g. QuickJS) could be an opt-in later.
4. **Review-state storage.** Confirm sidecar `.pdfreview.json` by default, with an annotation export option.
5. **Platform baselines and release order.** Windows 10 22H2+ / Linux Flatpak first, macOS 13+ at MVP+1? Code-signing certificate and Apple Developer account costs are yours to decide.
6. **PDFium sourcing.** Pinned prebuilt binaries (fast, a third-party build pipeline) vs building from source in CI (slow, fully auditable). I recommend prebuilt for Phase 0–1, with source builds before 1.0.
7. **Bundled fonts** for substitution and new text: Noto subset (OFL, adds ~20–60 MB) vs system fonts only.
8. **PDF4QT** is now MIT, so porting its algorithms with attribution is license-clean. Do you also want to open a dialogue with its maintainer (shared test corpus, upstream fixes)?
9. **Minimum Qt version.** 6.8 LTS (recommended; QRhiWidget available since 6.7).

---

## O. First implementation milestone — M0 "Walking skeleton" (~8–10 weeks)

**Goal:** prove the process architecture, the license-clean dependency stack and the `cos` writer invariants end to end, with production OSS scaffolding in place.

1. **Repository and governance:**
   - LICENSE (MPL-2.0), NOTICE, README, CONTRIBUTING (with DCO), CODE_OF_CONDUCT, SECURITY.md, GOVERNANCE.md
   - issue/PR templates, CODEOWNERS
   - ADRs 0001–0008 recording this plan's decisions: license, engine split, Rust/Qt split, process model, IPC, crypto backend, dependency policy, MVP scope
2. **CI** on Windows, Linux and macOS: fmt, clippy, tests, `cargo-deny` with the license allowlist, `cargo-audit`, a fuzz smoke job, and the C++ build with clang-tidy.
3. **Spike → ADR:** `hayro-syntax` vs our own parser, against the criteria in E4 (time-boxed to 1 week).
4. **`cos` v0:**
   - lexer and object parser
   - xref tables, xref streams, object streams, hybrid files
   - recovery scan for broken xref
   - lazy ObjectStore over mmap/read handle
   - filters: Flate, LZW, ASCIIHex, ASCII85, RunLength, plus predictors (DCT/JPX passed through)
   - Standard Security Handler decryption R2–R6
   - resource limits
   - **incremental writer** and **full writer**
5. **Invariant tests:**
   - `parse(write(doc))` is structurally equal
   - after an incremental save, the original byte prefix is identical
   - outputs pass `qpdf --check`
   - fuzz targets for lexer, xref and parser
6. **Engine host:**
   - Rust binary loading pinned PDFium
   - IPC v0: `open`, `page_count`, `page_size`, `render_tile`, `cancel`, `close`
   - tiles over shared memory
   - priority queue with cancellation
   - watchdog with auto-restart
   - Job-object (Windows) and rlimit (Linux) memory caps
   - open-time `cos`↔PDFium page-tree cross-check
7. **Qt shell skeleton:**
   - open dialog, continuous scroll with placeholders and async tiles, zoom, status bar
   - command registry with three commands and a palette stub
   - the engine crash is shown as a recoverable error
8. **CLI:** `vellora inspect <file> [--json]`, showing version, page count, object count, encryption, revisions, and the presence of JS, embedded files, launch actions, AcroForm or XFA.
9. **Corpus v0 + benchmarks:**
   - a separate corpus repo with a manifest of about 200 public PDFs across all categories (URL, SHA-256, license)
   - synthetic generators for 10k-page and 500 MB files
   - a benchmark harness: open time, first-page time, peak RSS

**Acceptance criteria for M0:**
- CI is green on all three OSes.
- `cargo-deny` passes. The generated third-party license list is complete.
- The engine opens ≥ 95% of corpus v0 without crashing. On the malicious/malformed subset, the UI process **never** crashes.
- Every incremental write in tests keeps the original byte prefix identical and passes `qpdf --check`.
- The 10,000-page synthetic document shows its first page in < 1.5 s on the dev machine, and its scrolling never blocks the UI thread (watchdog log is clean).
- Re-open-after-commit latency is measured on the 10k / 500 MB docs and recorded in an ADR, which confirms or revises the E3 strategy.

---

## P. Bootstrap (what this Opus session does after approval)

**Goal:** after this, you can open a new Sonnet 5.5 session in `vellora/` and say *"Implement M0 following CLAUDE.md and docs/milestones/M0.md"*, and it can work task by task without design decisions.

**Location:** `c:\Users\User\Documents\Open Source Projects\vellora\`

### P1. What gets created

**Governance and community (root):**
- `LICENSE`: the full MPL-2.0 text, fetched verbatim from mozilla.org.
- `NOTICE`: copyright plus the attribution policy for ported code, e.g. PDF4QT (MIT).
- `README.md`: vision, status "pre-alpha / M0", the three thesis pillars, build pointer, license.
- `CONTRIBUTING.md`: setup, DCO sign-off, Conventional Commits, PR rules, ADR/RFC process, corpus contribution rules, good-first-issue areas.
- `CODE_OF_CONDUCT.md`: Contributor Covenant 2.1, verbatim. Contact: your email or a placeholder you confirm.
- `SECURITY.md`: GitHub private vulnerability reporting, supported versions, 90-day disclosure, hostile-PDF reproducer handling.
- `GOVERNANCE.md`: BDFL now, path to a maintainer council. `MAINTAINERS.md`.
- `CHANGELOG.md` (Keep a Changelog format) and `SUPPORT.md`.

**Agent handoff:**
- `CLAUDE.md` (project-level). Rules every implementation session follows:
  - the architecture invariants: `cos` is the only writer; PDFium is never used to save; the UI never parses PDF bytes; `#![forbid(unsafe_code)]` outside `render`, `engine-client` and FFI crates; resource limits on all decoding
  - the definition of done: fmt, clippy `-D warnings`, tests, `cargo deny` all pass, and docs or ADRs are updated
  - the task workflow: pick the next unchecked task in `docs/milestones/M0.md`, implement it, verify it, tick it
  - commit conventions; no new dependency without a license check against `deny.toml`
  - escalation rules: if a task contradicts an ADR, stop and ask; don't redesign silently
- `docs/milestones/M0.md`: the M0 work broken into ordered, small tasks (≈25–35). Each task has its goal, the files and crates it touches, step notes, **acceptance checks (exact commands)** and its dependencies. It follows section O's order: scaffolding check → `cos` lexer → parser → xref → recovery → filters → crypt → writers → invariants → fuzz → `render` → engine IPC → watchdog → client → Qt skeleton → CLI inspect → corpus and benchmarks.

**Design documentation:**
- `docs/PLAN.md`: this full plan, A–P.
- `docs/architecture/`:
  - `overview.md`
  - `process-model.md`
  - `data-model.md` (layers, ChangeSets, save planner, fidelity matrix, editing tiers)
  - `security-model.md`
  - `performance-targets.md`
  - `dependency-policy.md` (allowed licenses, linked vs external tools)
  - `testing-strategy.md`
- `docs/adr/` in MADR format:
  - `0000-template`
  - 0001 License: MPL-2.0 + DCO
  - 0002 Engine split: PDFium read-only, `cos` sole writer
  - 0003 Rust core + C++/Qt 6 Widgets shell; cxx, not cxx-qt
  - 0004 Process model and sandboxing
  - 0005 IPC: postcard + shared-memory tiles, with a Rust client library in the UI
  - 0006 Crypto backend: OpenSSL 3 + RustCrypto
  - 0007 Dependency license policy
  - 0008 MVP scope
  - 0009 Non-destructive document model and save modes
  - 0010 OCR engine: Tesseract
  - 0011 XFA out of scope
  - 0012 No telemetry; local-only crash dumps
  - 0013 Pending: `cos` foundation, hayro-syntax vs our own (to be decided in M0 task 1)
- `docs/rfcs/0000-template.md` and `docs/roadmap.md` (phases table from K).
- `docs/dev/`:
  - `setup.md`: Windows/Linux/macOS toolchains — rustup, MSVC/clang, CMake ≥ 3.28, Qt 6.8 LTS, prebuilt PDFium, qpdf for tests
  - `coding-standards.md`: Rust and C++ style, error handling with `thiserror` in libraries and `anyhow` only in binaries, logging with `tracing`, no panics on untrusted input
  - `testing.md`

**Repo hygiene:**
- `.gitignore`, `.gitattributes` (LF normalisation; `*.pdf binary`), `.editorconfig`
- `rust-toolchain.toml` (pinned stable), `rustfmt.toml`, `.clang-format`, `deny.toml` (license allowlist + advisories + bans)

**`.github/`:**
- `workflows/ci.yml`: Rust fmt, clippy, test on Windows, Linux and macOS, plus `cargo-deny`. The C++/Qt job is added by M0 task "Qt skeleton", so CI is green from the first commit.
- `ISSUE_TEMPLATE/`: `bug_report.yml` (with a privacy warning about attaching PDFs), `feature_request.yml`, `compatibility_report.yml` ("this PDF renders or behaves wrong"), `config.yml` (security reports go to private advisories)
- `PULL_REQUEST_TEMPLATE.md` (DCO, tests, docs/ADR checkboxes, save-fidelity impact)
- `CODEOWNERS`, `dependabot.yml` (cargo + actions)

**Workspace skeleton** (compiles and tests green; **no product logic**):
- Root `Cargo.toml` workspace with shared lints (`unsafe_code = "forbid"` by default; clippy pedantic subset) and `[workspace.package]` (MPL-2.0, edition 2024, rust-version).
- Only the crates M0 needs: `crates/cos`, `crates/inspect`, `crates/render`, `crates/ipc`, `crates/engine` (bin `vellora-engine`), `crates/engine-client`, `crates/cli` (bin `vellora`).
  - Each has a `Cargo.toml` and a `lib.rs`/`main.rs` whose crate-level doc comment states its responsibility and boundaries (copied from F2).
  - Each has a single smoke test.
  - `content`, `doc`, `ops` and `diff` are documented in the architecture but created only when their phase starts. That's in line with the no-speculative-architecture rule.
- Placeholder directories with READMEs explaining what goes there and which M0 task fills them: `app/`, `fuzz/`, `bench/`, `tests/`, `third_party/`, `packaging/`.

### P2. Git and GitHub

- `git init -b main` inside `vellora/`.
- One initial commit: `chore: initialize vellora repository foundation`. Conventional Commits; **no Co-Authored-By or any AI/tool mention**, per your global CLAUDE.md, which overrides the default attribution.
- Create the GitHub repo with `gh repo create vellora --private --source . --push`. It's **private**, per your global rule; you flip it public when ready. Set the description and topics (pdf, rust, qt, pdf-editor, local-first).
- If `gh` isn't installed or authenticated, I stop after the local commit and tell you the exact commands.

### P3. Bootstrap verification

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` pass locally. If Rust isn't installed on this machine, I'll report that rather than claim green.
- `cargo deny check` passes if cargo-deny is available; otherwise CI is the check.
- After pushing, watch the first CI run (`gh run watch`) and report its real result.
- Read the final file tree and diff before reporting. Check that `docs/milestones/M0.md` tasks map one-to-one onto the section O deliverables and acceptance criteria.

---

## Verification of this plan (how we'll know the approach holds)

- **M0** directly tests the riskiest architectural assumptions: dual-parser consistency, re-open latency, process isolation, the incremental-writer invariant and the license gate. If any fails its acceptance criterion, we revisit E/F before Phase 1.
- Each later phase has measurable acceptance criteria (K) checked by the invariant harness, rendering regression, perf benchmarks and labelled corpora. No phase is "done" without those runs.
- Cross-viewer checks (Acrobat, pdf.js, PDFium) for anything we write: annotations, forms, signatures.

## Sources consulted (2026-10-07)

- hayro status: https://github.com/laurenzv/hayro , https://lib.rs/crates/hayro
- PDFium content regeneration limits: https://pdfium.googlesource.com/pdfium/+/refs/heads/main/public/fpdf_edit.h , https://groups.google.com/g/pdfium/c/VitvR3GgFX4 , https://github.com/SlyWombat/MegaPDF/issues/118
- MuPDF license: https://mupdf.readthedocs.io/en/1.26.11/license.html
- CXX-Qt releases: https://github.com/KDAB/cxx-qt
- lopdf: https://docs.rs/lopdf/latest/lopdf/struct.Document.html
- veraPDF licensing: https://verapdf.org/home/ , https://github.com/veraPDF/veraPDF-library
- Stirling PDF licensing: https://github.com/Stirling-Tools/Stirling-PDF/discussions/4332
- PDF4QT (MIT since 2025-04-27): https://github.com/JakubMelka/PDF4QT
- Open PDF Studio (OpenAEC): https://github.com/OpenAEC-Foundation/open-pdf-studio
- Everything else (library licenses and capabilities, attack research) is from established knowledge and **must be re-verified** when the corresponding ADR is written.
