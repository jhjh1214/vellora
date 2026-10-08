# Milestones

**Current milestone: [M1 — Viewer (v0.1)](M1.md)**

Each milestone is one roadmap phase broken into tasks. Every task is sized for one implementation session (see the session protocol in [`CLAUDE.md`](../../CLAUDE.md)). From M2 on, each milestone starts with **Task 0: refine**, which reconciles the plan with the code as it actually is before any feature work starts.

| Milestone | Release | Phase ([roadmap](../roadmap.md)) | Status |
|---|---|---|---|
| [M0](M0.md) | — | 0. Foundations (walking skeleton) | **Done** (2026-10-08) |
| [M1](M1.md) | v0.1 | 1. Viewer | **Current** |
| [M2](M2.md) | v0.2 | 2. Organize + Save | Planned |
| [M3](M3.md) | v0.3 | 3. Annotations & comments | Planned |
| [M4](M4.md) | v0.4 | 4. Object editing (tiers 1–2) | Planned |
| [M5](M5.md) | **v0.5 = MVP** | 5. Compare foundation | Planned |
| [M6](M6.md) | v0.6 | 6. Text editing (tiers 3–4) + macOS release | Planned |
| [M7](M7.md) | v0.7 | 7. Redaction, sanitisation, password protection | Planned |
| [M8](M8.md) | v0.8 | 8. Forms | Planned |
| [M9](M9.md) | v0.9 | 9. Digital signatures | Planned |
| [M10](M10.md) | v0.10 | 10. OCR | Planned |
| [M11](M11.md) | **v1.0** | Release hardening: audits, signing, distribution, docs | Planned |

After 1.0, roadmap phases 11–14 (compare v2, optimisation, preflight and accessibility tooling, automation and plugins) get their own milestone files.

## Rules for every milestone

- **The milestone file is the only hand-off between sessions.** Keep it accurate and short.
- **Notes:** at most 5 `Note:` lines per task, each at most two lines long. Anything longer belongs in an ADR, an architecture doc or [`../backlog.md`](../backlog.md), and the note links to it.
- **Splitting:** a task too large for one session is split in the file (e.g. 7a, 7b) in its own small PR before implementation starts.
- **Deviations:** a deviation from a task's *Do* list needs a `Note:` starting with `Deviation:`. A deviation from an ADR needs a new ADR.
- **Decisions:** a choice the task doesn't make and that changes architecture, security, licensing or user-visible behaviour is not made by the session. The session stops and lists it under *Decisions needed* below. If the maintainer has pre-authorised the recommended option in the session prompt, record it as a `Note: Decision (delegated):` and, if architectural, as an ADR.
- **Acceptance criteria are not edited to fit results.** If a criterion can't be met, the milestone stays open: leave the box unchecked, record the evidence, and add the item under *Decisions needed*.
- **Small gaps found along the way** go to [`../backlog.md`](../backlog.md) with a link to where they were found, not into an endless note.

## Decisions needed (maintainer)

These are open items no session may decide on its own. Remove an entry once it's decided and recorded.

| # | Decision | Needed by | Recommendation |
|---|---|---|---|
| 1 | Windows code signing: SignPath Foundation (free for OSS, needs an application) vs a paid certificate vs unsigned | M1 task 23 (nightlies may stay unsigned); required for M11 | Apply to SignPath Foundation now; unsigned nightlies until accepted |
| 2 | Apple Developer Program membership (USD 99/year) for notarised macOS builds | M6 task 3 | Yes, before the macOS release |
| 3 | Bundled fallback font for generated text (Noto Sans subset, OFL, adds 5–20 MB) vs system fonts only | M3 task 2 | Bundle a Latin/Greek/Cyrillic Noto Sans subset; use system fonts for other scripts |
| 4 | Network features (opt-in update check, revocation/OCSP and TSA for signatures, OCR language downloads), all in the UI process, each needing an ADR | M9 task 1 (writes the network ADR), M10 task 7, M11 task 6 | Allow them as explicit, opt-in, UI-process-only features |
| 5 | Project website and docs hosting (GitHub Pages from `docs/user` via mdBook) | M11 task 9 | GitHub Pages |
| 6 | Whose page count wins when PDFium and `cos` disagree on a hostile file (M1 task 6: PDFium 1 page, `cos` 0): (a) the engine reports `cos`'s count and shows only pages `cos` sees; (b) keep PDFium's view with the repair notice, and make save refuse or ask when the notice contains `page-count-mismatch` | M2 task 1 (save) | (b): the user sees what PDFium renders, and no save silently drops pages |
