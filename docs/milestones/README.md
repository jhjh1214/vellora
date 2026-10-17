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
| 6 | Whose page count wins when PDFium and `cos` disagree on a hostile file (M1 task 6: PDFium 1 page, `cos` 0): (a) the engine reports `cos`'s count and shows only pages `cos` sees; (b) keep PDFium's view with the repair notice, and make save refuse or ask when the notice contains `page-count-mismatch` | M2 tasks 7b (save planner) and 8a (edits are refused while the views disagree) | (b): the user sees what PDFium renders, and no save silently drops pages |
| 7 | Whether M1 task 7's acceptance ("all protected corpus files open with their passwords") counts the two `*_bad_okey` files, whose `/O` entry is broken so that neither `cos` nor PDFium opens them with any password: (a) no, they are malformed files and the gate expects a typed refusal (as now), so task 7 is ticked; (b) yes, which only a different corpus file can satisfy (drop them or replace them with sound protected files) | M1 task 7 (tick) and task 25 | (a): a file that no password opens is not a protected document that fails to open |
| 8 | Timing tests on the macOS CI runner: the 8 ms frame-budget tests cannot pass there (handlers that take 0.5 ms elsewhere show up to 20 ms), so `tst_canvas_render` (task 1) and `tst_thumbnails_frame_time` (task 11) are disabled on macOS CI and run elsewhere; the functional tests all run. Evidence in `app/README.md`. (a) keep it so and measure macOS on real hardware (M6); (b) run them on macOS CI with a larger budget; (c) rent a self-hosted macOS runner | M1 task 11 (taken as (a) to merge), M6 | (a) |
| 9 | Running `docs/dev/accessibility-checklist.md` with NVDA (Windows) and Orca (Linux) and recording the results in a PR: M1 task 17's acceptance needs it and no agent can run a screen reader | M1 task 17 (tick), before the v0.1 release | Do it on the first nightly that has task 17; fix what fails before tagging v0.1 |
| 10 | The performance gate on macOS CI: shared macOS runners vary by 20-80 % between runs of the same code (ADR-0016), so a 10 % gate would fail at random. (a) macOS is measured and compared every night but never fails the job (as now); (b) gate it with a much wider limit (50 %), which only catches large regressions; (c) a self-hosted macOS runner, as in decision 8 | M1 task 21 (tick) | (a), and measure macOS on real hardware at M6 |
| 11 | The published name of the packages: the Flatpak app id (provisional `io.github.jhjh1214.Vellora`, which fixes the reverse-DNS owner), the Windows publisher and product name in the MSI (`Vellora`), and the macOS bundle id | M1 task 23 (provisional names used), before the first release | Keep the provisional names until a project domain or GitHub organisation exists, then change all three together |
| 12 | How the engine gets a second document for import and merge. It cannot be given a handle after it starts (handles are inherited at spawn; the AppContainer and the Linux seccomp filter give it no way to receive one). (a) the UI sends the file's bytes in `Request` chunks and the engine parses them in memory, capped by `--max-document-bytes`; (b) a short-lived second engine reads the source and returns a self-contained mini-PDF of the chosen pages, which the UI relays to the first; (c) spawn the engine with a fixed number of spare inherited handles and restart it for more (an ADR-0015 amendment) | M2 task 8c (blocks 12a, 11b file drops) | (a): simplest, no sandbox change, memory bounded by the existing cap |
| 13 | What undo does after a save. Saving the open file ends the engine and starts a new one on the saved file (the Windows share mode forbids anything else, M2 task 9a), so the `doc` history is gone. (a) the history is cleared by a save (the journal is deleted then anyway, task 10b); (b) the client keeps the saved operations with their inverses and replays them as new edits, so Ctrl+Z after a save still works | M2 task 8b and 9a ("undo after save" in the task 8 acceptance), M2 task 13 | (a), shown in the UI ("Undo history cleared by saving"); (b) is a later feature |
| 14 | What a full rewrite (Save Clean, extract, split) does to an encrypted document. `cos` removes the encryption by default (`EncryptionPolicy::Remove`). (a) keep it (`EncryptionPolicy::Keep`: same passwords and permissions; needs the password the document was opened with); (b) remove it and say so in the save impact | M2 task 7b and 6 | (a): a save never silently removes protection; removing it is M7's explicit feature |
