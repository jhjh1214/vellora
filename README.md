# Vellora

**A fast, faithful, local-first PDF workstation.** Open source. No account, no cloud, no telemetry.

> **Status: pre-alpha (Milestone M0, "walking skeleton").** Nothing here is usable yet.
> The architecture and the roadmap are public and stable; see [`docs/PLAN.md`](docs/PLAN.md).

Vellora aims to replace the majority of serious Adobe Acrobat Pro workflows: viewing, page management, direct editing, annotation and review, PDF comparison, forms, digital signatures, real redaction, OCR, inspection, preflight and automation. It is *not* an Acrobat clone. It is built around three properties no existing open-source tool combines:

| Pillar | What it means |
|---|---|
| **Faithful** | Edits never silently degrade a document. Untouched content stays byte-identical. Every save tells you its impact: incremental or full rewrite, which signatures are affected, and whether deleted content stays recoverable. |
| **Safe** | PDFs are treated as hostile input. Parsing and rendering run in a sandboxed, resource-limited engine process. Our parser and writer are memory-safe Rust. JavaScript, launch actions and attachments never execute. |
| **Fast at scale** | Lazy everything. 10,000-page and 500 MB documents open in about a second and scroll smoothly. |

## Architecture at a glance

```
Qt 6 desktop shell (C++)  ──cxx──  engine-client (Rust)
                                        │  IPC + shared-memory tiles
                         sandboxed engine process (Rust)
                           ├─ cos      — our PDF object layer: parser + the ONLY writer
                           └─ PDFium   — read-only rasteriser / text geometry / forms
```

- [Architecture overview](docs/architecture/overview.md)
- [Architecture decision records](docs/adr/)
- [Roadmap](docs/roadmap.md)
- [Current milestone: M0](docs/milestones/M0.md)

## Building

See [`docs/dev/setup.md`](docs/dev/setup.md). Short version, for the Rust workspace:

```sh
cargo build --workspace
cargo test --workspace
```

The Qt desktop app (`app/`) arrives during M0.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) first: we use DCO sign-off and Conventional Commits, and architectural changes go through ADRs or RFCs. Please follow our [Code of Conduct](CODE_OF_CONDUCT.md).

**Security issues:** please don't open a public issue. See [SECURITY.md](SECURITY.md).

## License

Vellora is licensed under the [Mozilla Public License 2.0](LICENSE). Third-party notices: [NOTICE](NOTICE).
