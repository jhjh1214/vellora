# Roadmap

Each phase ends with a tagged pre-release. Estimates assume one core developer. Full reasoning, risks and acceptance criteria: [`PLAN.md` §K](PLAN.md).

| Phase | Release | Objective | Key features | Depends on |
|---|---|---|---|---|
| **0. Foundations** | — | Walking skeleton + OSS infrastructure | [`milestones/M0.md`](milestones/M0.md) | — |
| **1. Viewer** | v0.1 | A daily-usable fast viewer | Rendering modes, thumbnails, outline, links, selection, search, Inspector v1, security report, CLI `inspect/security/render` | 0 |
| **2. Organize + Save** | v0.2 | Faithful structural edits | Organize grid, page ops, import/merge/split, incremental + Save Clean, save impact, undo (creates `doc`, `ops`) | 1 |
| **3. Annotations & comments** | v0.3 | Standard-conformant review markup | Markup, ink, shapes, FreeText, notes, replies, status, appearance generation | 2 |
| **4. Object editing T1–2** | v0.4 | Prove content-stream surgery | Interpreter with provenance, object selection, add text/image, move/resize/delete (creates `content`) | 2 |
| **5. Compare foundation** | **v0.5 = MVP** | The defining feature, v1 | Page alignment, word diff, change categories, navigator, sidecar review, CLI compare (creates `diff`) | 1, 3 |
| 6. Text editing T3–4 | | In-place text editing | Run editing, font reuse/substitution, block reflow | 4 |
| 7. Redaction + sanitisation | | Real removal | Mark → review → apply → verify; sanitise profiles | 4 |
| 8. Forms | | AcroForm fill + designer | Fill, native AF* scripts, designer, XFA detection | 3 |
| 9. Signatures | | Correct verify, then sign | PAdES verify + attack checks; sign via PKCS#12/#11/OS stores; B-B → B-LTA | 2, 8 |
| 10. OCR | | Searchable scans | Tesseract worker, deskew, OSD, invisible text layer | 1, 4 |
| 11. Compare v2 | | Visual + object diff | Pixel diff regions; image/annot/field/font/metadata diff | 5 |
| 12. Optimisation | | Explain and reduce size | Size attribution, dedupe, recompress, presets | 2 |
| 13. Preflight + accessibility | | Professional validation | Own checks + veraPDF; tag-tree editor; PDF/UA audit | 4, 12 |
| 14. Automation, conversion, plugins | | Ecosystem | JSON pipelines over `ops`; LibreOffice conversion; out-of-process plugin protocol | all |

**v1.0** = Phases 0–10, plus macOS parity and completed accessibility and security audits.
