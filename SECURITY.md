# Security Policy

Vellora's job includes opening untrusted, potentially malicious PDFs. We treat security reports as top priority.

## Reporting a vulnerability

**Do not open a public issue, PR or discussion.**

Report privately via GitHub: **Security → Report a vulnerability** on this repository (GitHub private vulnerability reporting). If that isn't possible, email the maintainer listed in [MAINTAINERS.md](MAINTAINERS.md).

Please include:
- affected version or commit
- platform
- a description of the impact
- a minimal reproducer (a PDF, if possible)

**Attach malicious PDFs only through the private advisory**, ideally inside an encrypted ZIP. The password can go in the advisory text.

## What happens next

| Step | Target |
|---|---|
| Acknowledge receipt | 3 working days |
| Initial assessment and severity | 10 working days |
| Fix or mitigation for high/critical issues | as fast as practical, aimed at < 90 days |
| Coordinated public disclosure | at fix release, or 90 days after report, whichever is first, unless agreed otherwise |

We credit reporters in the advisory unless you ask us not to.

## Scope

In scope:
- memory-safety bugs
- sandbox escapes
- code execution
- information leaks (including incomplete redaction)
- signature-validation bypasses (e.g. incremental-update or shadow attacks)
- path traversal via attachments
- denial of service that bypasses our resource limits
- unsafe defaults

Bugs in third-party components (PDFium, Qt, OpenSSL, …) are welcome too. We coordinate with upstream.

## Supported versions

Pre-1.0: only the latest `main` and the most recent pre-release receive fixes. A support policy for stable releases will be published at 1.0.

## Hardening notes

- Parsing and rendering run in a separate, resource-limited engine process. Per-OS sandboxing is introduced progressively; see [`docs/architecture/security-model.md`](docs/architecture/security-model.md).
- PDF JavaScript is never executed. Launch actions are never executed. Attachments are never opened automatically.
- Vellora does not phone home: no telemetry and no automatic crash upload.
