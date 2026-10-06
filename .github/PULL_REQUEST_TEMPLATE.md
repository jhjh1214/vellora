## What and why

<!-- What does this change, and why? Link the issue / milestone task (e.g. "M0 task 6"). -->

## How it was verified

<!-- Commands you ran and their results. Say what you could NOT run. -->

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo deny check`

## Save-fidelity impact (if this touches writing PDFs)

<!-- Save mode (incremental / full rewrite)? Untouched bytes preserved? Signatures affected? Recoverable content? -->

## Checklist

- [ ] Commits follow Conventional Commits and are signed off (`git commit -s`, DCO)
- [ ] Tests added or updated. No assertions weakened, no tests skipped.
- [ ] Docs / ADRs updated if a boundary or documented behaviour changed
- [ ] New dependencies are justified and pass `cargo deny check`
- [ ] No untrusted-input panics, and decoding respects `cos::limits` (CLAUDE.md invariant 5)
