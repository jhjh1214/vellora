# Security model

**Assumption:** every PDF is potentially hostile. Parser bugs will exist, in PDFium, its codecs, and our code. The architecture limits what a successful exploit gains and how much a malicious file can consume.

## Trust boundaries

```
 user + OS  ──trusted──  UI process  ══pipes/shm══  engine process (untrusted, sandboxed)  ──▶ PDFium, codecs
```

- The engine gets **no paths, no network, no new handles**. The UI passes it exactly the file handle(s) it needs.
- The UI never parses PDF bytes. It treats engine replies as untrusted data: size-capped and validated.

## Threats and mitigations

| Threat | Mitigation |
|---|---|
| Memory corruption in PDFium / FreeType / OpenJPEG / libjpeg | Engine isolation + resource limits (M0) → per-OS sandbox (progressive: Windows AppContainer + restricted token + Job object; Linux seccomp-bpf + Landlock + namespaces; macOS sandbox profile). Watchdog restart. Monthly PDFium updates. |
| Bugs in our parser | Rust with `unsafe` forbidden; no panics on input; continuous fuzzing (cargo-fuzz in CI and nightly → OSS-Fuzz once public) |
| Decompression bombs / memory exhaustion | `cos::limits`: per-stream and total decode caps, ratio cap, image dimension caps. Process memory cap. Streaming decode. |
| CPU exhaustion (deep nesting, loops, giant content streams) | Depth limits, reference-cycle detection, job deadlines + cancellation, watchdog kill |
| Malicious fonts / images | Decoded only inside the engine (PDFium) or by memory-safe Rust crates |
| JavaScript | PDFium built **without V8**. Form calculations later use native AF* function implementations. JS is reported, never run. |
| Launch / GoToR / ImportData / SubmitForm / URI actions | Launch never executes. URIs need confirmation showing the full URL. SubmitForm is off by default. Remote GoTo prompts. |
| Embedded files, path traversal | Never auto-opened or executed. Save only via a dialog. Filenames sanitised (separators, reserved names, NTFS ADS, control characters). |
| External references (remote content, OCSP/TSA) | No network for document content. Revocation and timestamp requests (Phase 9) are explicit or opt-in and go through the UI process, never the engine. |
| Signature spoofing (USF, ISA, SWA, shadow attacks) | Strict ByteRange rules; independent re-parse of the signed revision; render signed vs current revision; DocMDP/FieldMDP classification of later changes; "valid but modified after signing" shown distinctly |
| Visual-signature confusion | Appearance-only signatures are labelled; cryptographic status is shown separately |
| Redaction leaks | Mandatory full rewrite + GC. Scrub metadata, outline, structure tree (Alt/ActualText), form values, thumbnails, OCR layers and attachments. Post-apply verification with independent extractors. |
| Differential parsing (`cos` vs PDFium disagree) | Open-time cross-check → *Repaired* mode forces a normalised rewrite. Signatures are validated only through `cos`. |
| Supply chain | `cargo-deny` (licenses, advisories, sources), reviewed dependency additions, PDFium pinned by SHA-256, CycloneDX SBOM per release, signed releases, reproducible builds as a goal |
| Future plugins | Out-of-process only, same sandbox, broker-mediated capabilities. No in-process native plugins. |

## Privacy

- **No telemetry.** No automatic crash upload. The update check is opt-in.
- Crash dumps are written locally. The user decides whether to attach them to an issue.
- Logs never contain document text at `info` level or above.

## Sandbox status tracker

| Platform | Process isolation | Resource limits | Handle-only FS access | OS sandbox |
|---|---|---|---|---|
| Windows | M0 | M0 (Job object) | M0 | Phase 1–2 |
| Linux | M0 | M0 (rlimit) | M0 | Phase 1–2 |
| macOS | M0 | M0 (rlimit) | M0 | before macOS release |

### Known gaps in the engine boundary

| Gap | Effect | Planned fix |
|---|---|---|
| The engine holds a writable handle to the shared tile region, so a compromised engine on Linux or macOS can shrink the file | The UI faults (SIGBUS) when it reads a lost page: a denial of service of the UI process, no data exposure or corruption. Not possible on Windows | Sealed anonymous region (`memfd` + `F_SEAL_SHRINK` on Linux; evaluate `shm_open` on macOS), see ADR-0015 |
| The engine maps the document, so a file truncated by another process kills the engine (not the UI) | The client restarts the engine | Parent denies writers on the file where the OS allows it (task 20) |
