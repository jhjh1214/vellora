# Roadmap

Each phase is one milestone and ends with a tagged pre-release. Estimates assume one core developer. Full reasoning and risks: [`PLAN.md` §K](PLAN.md). Task-level plans and status: [`milestones/`](milestones/README.md).

| Phase | Milestone | Release | Objective | Key features | Depends on |
|---|---|---|---|---|---|
| **0. Foundations** | [M0](milestones/M0.md) ✅ | — | Walking skeleton + OSS infrastructure | `cos` parser/writers, sandboxed engine, IPC, Qt shell, CLI `inspect`, corpus, fuzzing | — |
| **1. Viewer** | [M1](milestones/M1.md) | v0.1 | A daily-usable fast, safe viewer | OS sandboxes (Win/Linux), passwords, tabs, view modes, thumbnails, outline, links, selection, regex search, printing, accessibility, Inspector v1, CLI `security/render/text`, installers + nightlies | 0 |
| **2. Organize + Save** | [M2](milestones/M2.md) | v0.2 | Faithful structural edits | `doc` + `ops`, page ops, import/merge/split/extract, incremental + Save Clean, save impact, crash-safe journal, Inspector v2 | 1 |
| **3. Annotations & comments** | [M3](milestones/M3.md) | v0.3 | Standard-conformant review markup | `fonts` crate, appearance generation, all markup types, threaded comments panel, XFDF, flatten | 2 |
| **4. Object editing T1–2** | [M4](milestones/M4.md) | v0.4 | Prove content-stream surgery | `content` crate, object selection, add text/image, move/resize/delete, watermark, header/footer, Bates | 2, 3 |
| **5. Compare foundation** | [M5](milestones/M5.md) | **v0.5 = MVP** | The defining feature | `diff` crate, page alignment, reflow-proof word diff, change review with sidecar, reports, CLI compare, MVP release | 1, 3 |
| 6. Text editing T3–4 | [M6](milestones/M6.md) | v0.6 | In-place text editing + macOS release | Font analysis, run editing, block reflow, find & replace, macOS sandbox, notarised universal build | 4 |
| 7. Redaction + sanitisation | [M7](milestones/M7.md) | v0.7 | Real removal | `/Redact` marks, removal per channel, verification, sanitise profiles, AES-256 password protection | 4 |
| 8. Forms | [M8](milestones/M8.md) | v0.8 | AcroForm fill + designer | Native fill and appearances, native AF* functions, designer, data exchange, flatten, XFA handling | 3 |
| 9. Signatures | [M9](milestones/M9.md) | v0.9 | Correct verify, then sign | Strict integrity, trust/revocation/timestamps, attack detection, PAdES B-B → B-LTA, hardware keys, Fill & Sign | 2, 8 |
| 10. OCR | [M10](milestones/M10.md) | v0.10 | Searchable scans | Tesseract worker, invisible text layer, orientation/deskew, language packs | 1, 4 |
| — Release hardening | [M11](milestones/M11.md) | **v1.0** | Trustworthy 1.0 | Audits, signed distribution, OSS-Fuzz, PDFium from source, translations, docs, website, soak tests | 0–10 |
| 11. Compare v2 | after 1.0 | | Visual + object diff | Pixel diff regions; image/annot/field/font/metadata diff | 5 |
| 12. Optimisation | after 1.0 | | Explain and reduce size | Size attribution, dedupe, recompress, presets, linearisation | 2 |
| 13. Preflight + accessibility | after 1.0 | | Professional validation | Own checks + veraPDF; tag-tree editor; PDF/UA audit | 4, 12 |
| 14. Automation, conversion, plugins | after 1.0 | | Ecosystem | JSON pipelines over `ops`; LibreOffice conversion; out-of-process plugin protocol | all |

**v1.0** = Phases 0–10, plus macOS parity and completed accessibility and security audits (M11).
