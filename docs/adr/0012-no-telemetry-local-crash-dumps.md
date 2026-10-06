# 0012. No telemetry; local-only crash dumps

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

Privacy is a core promise. Users open confidential documents. Still, we need crash information to fix bugs.

## Decision

- **No telemetry.** No usage analytics. No automatic network requests. The update check is opt-in.
- Crashes produce minidumps (Breakpad/Crashpad-compatible tooling, BSD/Apache) written **locally**. After a crash, the app offers to open the crash folder and a prefilled GitHub issue. The user chooses what to attach.
- Logs never contain document text at `info` level or above.

## Consequences

Fewer crash reports than with automatic upload. That's the price of trust. The compatibility-report issue template and the corpus process compensate.
