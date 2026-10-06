# Governance

## Current model: benevolent maintainer

Vellora currently has one lead maintainer (see [MAINTAINERS.md](MAINTAINERS.md)), who has final say on technical direction, releases and moderation. Decisions are made in public: in issues, PRs, ADRs and RFCs.

## How decisions are made

| Kind of change | Mechanism |
|---|---|
| Bug fix, small feature, docs | Pull request + maintainer review |
| Architectural decision, new dependency, protocol or file-format change | [ADR](docs/adr/) accepted in a PR |
| Large or user-visible feature, cross-cutting change | [RFC](docs/rfcs/) with a public comment period (≥ 7 days) |
| License change | Not possible without the consent of all copyright holders (we use DCO, not a CLA) |

## Becoming a maintainer

Contributors with a sustained record of high-quality contributions and reviews may be invited as maintainers by the existing maintainers. Maintainers can approve and merge PRs in their areas (see `.github/CODEOWNERS`).

## Transition to a maintainer council

When there are at least **three active maintainers**, governance moves to a maintainer council:
- decisions by lazy consensus, falling back to a simple majority vote
- the lead maintainer role becomes rotating or elected
- this document is updated by RFC to reflect that

## Code of Conduct

All participation is governed by the [Code of Conduct](CODE_OF_CONDUCT.md). Maintainers enforce it.
