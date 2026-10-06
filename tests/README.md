# tests/ — cross-crate tests and corpus

- `corpus/manifest.toml` (M0 task 1): real-world PDFs, listed by URL + SHA-256 + license + categories. They're downloaded with `cargo xtask corpus fetch` into `corpus-data/` (git-ignored). **PDFs are never committed here.**
- `invariants/` (M0 task 13): the invariant harness. Write → `qpdf --check` → re-parse; incremental prefix identity; later, pixel identity of untouched pages.
- `fixtures/`: small generated fixtures (< 100 KB), each documented with its generator and license.

## Adding a corpus document

1. The document must be redistributable, with a known license, and contain no personal data.
2. Add a `[[doc]]` entry with `id`, `url`, `sha256`, `license`, `categories` and `notes`.
3. Malicious or crash reproducers never go here. Use the restricted security corpus (see SECURITY.md).
