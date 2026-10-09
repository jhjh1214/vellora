# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Page thumbnails: a sidebar on the left (View → Page Thumbnails, F4) with a thumbnail and the page label for every page, drawn lazily as you scroll; the page you are reading is highlighted, a click or the arrow, Page Up/Down, Home and End keys jump to a page, and the size is set with the slider or Ctrl+wheel. Thumbnails have a cache of their own (64 MB) and are rendered after every visible tile, so a fling through a 10,000-page document does not slow the page you are reading.
- View modes: single page or continuous scrolling, one or two pages a row (with an optional cover page), the view can be turned in quarter turns (never written to the file), fit width, fit page, zoom presets from 25% to 6400%, zoom to a dragged rectangle (hold Z) and to the cursor (Ctrl+wheel). Old tiles stay on screen, scaled, until the sharp ones arrive. The arrangement is remembered for each document and for new ones.
- Crash reports, all local: if the application crashes, a monitor process writes a minidump (the stacks of the threads, not the heap) into the crash folder, and the next start offers to open the folder or a prefilled GitHub issue (nothing is attached or uploaded). If the sandboxed engine crashes or panics, the app records what happened with the engine's last 200 log lines. See ADR-0019.
- Logs: the application writes rotating log files (at most 10 MiB) to the platform's log folder, including the sandboxed engine's log lines, which the UI process now reads from the engine's standard error. Help → Open Log Folder shows the folder; `VELLORA_LOG_DIR` and `VELLORA_LOG` change where and how much.
- Help → About Vellora: version, commit and build date, the MPL-2.0 license, the Qt LGPL notice with how to replace Qt, and a viewer for the licenses of every bundled component (Rust crates, PDFium and the libraries inside it, Qt). `cargo test` fails when `THIRD_PARTY_LICENSES` lacks a crate that is linked.
- Tabs: every document opens in its own tab with its own engine process (Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+W, Ctrl+Shift+T to reopen). Files can be given on the command line, dropped on the window or chosen from File → Open Recent (15 files, unreadable ones greyed out); a second launch hands its files to the running window. The window size and, for each file, the page and zoom it was left at are remembered.
- Encrypted documents: the engine opens them with a password (user or owner, any revision of the Standard Security Handler). A document that needs one is refused with a typed `PasswordRequired`, a wrong password with `WrongPassword`; the shell asks in a modal prompt with three attempts and never logs or stores the password. Revision 6 passwords are SASLprep-normalised, older ones are also tried as Latin-1 (protocol v3).
- Repository foundation: governance documents, architecture documentation and ADRs, Rust workspace skeleton, CI.
- `cargo xtask pdfium fetch`: downloads the PDFium build pinned in `third_party/pdfium.lock`, verifies its SHA-256 and extracts it.
- `vellora-render`: thin bindings to PDFium, loaded at run time (ADR-0014).
- `vellora inspect <file> [--json] [--password <pw>]`: version, pages, objects, revisions, encryption, repairs and risky features (JavaScript, launch/URI/submit/remote actions, embedded files, forms, XFA, layers, signature fields). Output formats in `docs/user/cli.md`.
- `cargo xtask bench`: synthetic 10,000-page and ~500 MB documents and a harness for open → first page, peak memory and re-open-after-commit latency (ADR-0016).
- Corpus gate: `crates/engine/tests/corpus.rs` opens every corpus v0 document in the engine and renders page 1 (97.0% of the non-password files open, no engine crash or hang).
- `THIRD_PARTY_LICENSES` (cargo-about for Rust crates, plus PDFium and Qt notices); milestone M1 (viewer) task plan.

### Milestone M0 (walking skeleton) complete
All acceptance criteria in `docs/milestones/M0.md` are ticked with evidence. Not tagged.
