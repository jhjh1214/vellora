# Document and data model

## Layers

```
ByteSource            immutable original (mmap or read handle) — never modified while open
  └─ Revision chain    the file's own incremental history (inspectable; signatures cover prefixes)
       └─ Pending ChangeSets   our edits, in memory: {new objects, replaced objects, freed refs}
            └─ Views (lazy, cached, keyed by (objref, change-epoch))
                 PageTree · Page · PageObjects · TextLayout · Annots · Fields · Outline …
```

- **COS layer (`cos`):** objects (`Null, Bool, Int, Real, String, Name, Array, Dict, Stream, Ref`), xref index, lazy resolution with cycle and depth limits, decoding with limits. Object identity is `(object number, generation)` and is **stable across incremental saves**.
- **Views (`doc`, Phase 2):** typed accessors computed *over* COS objects. They are never parallel copies that must be kept in sync. There's no god object: `Document` is a thin session handle.
- **Content (`content`, Phase 4):**
  - content streams tokenised **with byte spans**
  - an interpreter tracking graphics state (CTM, text matrix, font, clip) to produce page objects (`TextRun | Image | Path | Shading | FormXObject`), each with bbox and **provenance** `(stream objref, operator range, nesting path)`
  - selection and editing target provenance

## Transactions and history

```
ops::Operation ──apply──▶ doc.transact(|tx| …) ──▶ ChangeSet { new, replaced, freed }
```

- ChangeSets layer over the base. The base is never mutated, so "show original", "revert" and "diff against original" are free.
- Undo pops a ChangeSet; redo re-applies it. History is linear per session.
- Derived views are invalidated by change epoch, per affected object and page.

## Content edits

- A `ContentPatch` replaces an operator range in one stream: `(stream objref, op range) → new operators`. Applying it **splices bytes**. Untouched operators keep their original bytes.
- New content (added text or images, OCR layers) goes into a **new stream appended** to the page's `/Contents`, wrapped in `q … Q`, so the original stream is untouched.

## Saving

| Mode | When | Properties |
|---|---|---|
| **Incremental** (default when possible) | Annotations, form fill, page-tree edits, content patches, signing (mandatory), OCR | Original bytes are a byte-identical prefix. Existing signatures stay cryptographically intact. Old content remains recoverable from earlier revisions. |
| **Full rewrite** | Redaction (mandatory), sanitise, optimise, explicit **Save Clean**, *Repaired* documents | Garbage-collects unreachable objects and drops earlier revisions. Removes signatures. |

The **save planner** computes `SaveImpact { mode, reasons[], signatures: {intact | permitted-change | invalidated}, recoverable_content, lossy_steps[] }` before writing:
- The UI shows it in the save-impact bar.
- The CLI prints it, and `--require-incremental` makes any downgrade an error.

**Write protocol:** the engine produces bytes. The UI writes `target.tmp` in the same directory, fsyncs, and renames atomically. The new file must re-parse and pass a self-check before the rename. The source file is never overwritten in place.

## Fidelity matrix

"Sig" = existing signatures. Incremental saves keep signed bytes intact; whether a change is *permitted* depends on DocMDP/FieldMDP.

| Operation | Save | Untouched content | Sig | Warning shown |
|---|---|---|---|---|
| Add/modify annotation, reply, status | Incr. | byte-identical | intact; permitted if P=3 | — |
| Fill form field | Incr. | byte-identical | intact; permitted if P≥2 | — |
| Rotate page | Incr. (`/Rotate`) | byte-identical | flagged | "Changes signed document" |
| Reorder / insert / delete pages | Incr. | byte-identical | flagged | Delete: "pages remain recoverable — use Save Clean" |
| Import / merge pages | Incr. or new file | resources deep-copied and deduped | flagged | field-name collisions resolved explicitly; imported signature values stripped |
| Crop | Incr. (`/CropBox`) | content kept | flagged | "Cropping hides, does not remove" |
| Add text / image | Incr. (appended stream) | byte-identical | flagged | — |
| Move/resize/delete image or path | Incr. (one stream spliced) | other operators byte-identical | flagged | Delete: recoverable unless Save Clean |
| Edit existing text | Incr. (local splice) | identical elsewhere | flagged | "Font substituted" when it happens |
| Redact | **Full rewrite, mandatory** | touched images re-encoded | **removed** | review screen + verification report |
| Sanitise / Optimise | Full rewrite | lossless unless a lossy preset is chosen | removed | itemised report |
| Sign | **Incr., mandatory** | byte-identical | prior intact | — |
| OCR | Incr. (new invisible-text stream) | byte-identical | flagged | — |
| Save Clean | Full rewrite | visually identical | removed | "Removes signatures and edit history" |

## Direct text editing tiers

| Tier | Capability | Technique |
|---|---|---|
| 0 | Overlay (annotation / FreeText) | Standard annotations |
| 1 | Add text / images | Appended stream; harfrust shaping; font subset embedding |
| 2 | Move/resize/delete existing objects | Operator-range splice; `q cm … Q` wrapping; `TJ` splitting |
| 3 | Edit a single-line text run in place | Re-encode with the run's font. If glyphs are missing: embed the full font if installed and permitted, otherwise substitute and **flag it** |
| 4 | Edit within a reconstructed block | Re-lay-out within block bounds; high-confidence blocks only |
| ✗ | Not editable: text as paths, text in images (→ OCR), Type3 bitmap fonts, unrecoverable encodings | Explained in the UI, never faked |

## Review state

- Comments are **standard PDF annotations** (`/IRT` replies, `/State` + `/StateModel`), portable to Acrobat.
- Compare-session state (reviewed or dismissed changes) lives in a sidecar `*.pdfreview.json` by default, so comparing never modifies either input. It can optionally be exported as annotations.
