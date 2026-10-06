# 0011. XFA forms are out of scope

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

XFA is an XML forms architecture, deprecated in PDF 2.0, large to implement and heavily tied to JavaScript. PDFium's XFA support requires V8, which we exclude for security.

## Decision

- No XFA rendering or editing.
- Vellora **detects** XFA, tells the user clearly, and uses the AcroForm fallback fields when the document has them.
- The Inspector and the CLI report XFA presence.

## Consequences

Some government and legacy forms won't be usable. This is documented. We revisit only with strong user demand and a safe implementation path.
