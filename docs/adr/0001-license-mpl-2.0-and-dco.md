# 0001. License: MPL-2.0, contributions under DCO

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

The license decides which PDF engines we can use and who can build on us.
- MuPDF (the most capable open engine) is AGPL, which would force the whole application to AGPL.
- We want an ecosystem in which commercial embedders and proprietary plugin authors can participate, while improvements to Vellora's own files stay open.

## Decision

- The project is licensed under **MPL-2.0**.
- Contributions use the **Developer Certificate of Origin** (`Signed-off-by`), not a CLA.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| AGPL-3.0 + MuPDF | Fastest route to features (redaction, signing, content filtering) | Every embedder and plugin must be AGPL-compatible; no permissive SDK; commercial use of the core needs an Artifex license |
| GPL-3.0 | Strong copyleft; could use Poppler | No MuPDF anyway; blocks proprietary plugins and embedders |
| Apache-2.0 / MIT | Maximum reuse | Closed forks of our core files are allowed |
| **MPL-2.0** | File-level copyleft; allows proprietary plugins and embedding; compatible with Apache/BSD/MIT and LGPL Qt (dynamic); GPL-compatible (§3.3) | Rules out MuPDF, Poppler and Ghostscript as linked dependencies, so we must own the editing pipeline |
| CLA instead of DCO | Allows future relicensing | Contributor friction; trust cost |

## Consequences

- PDF engine: PDFium (permissive) for rendering, plus our own write path (ADR-0002).
- Relicensing later would require consent from all contributors.
- See the [dependency policy](../architecture/dependency-policy.md) and ADR-0007.
