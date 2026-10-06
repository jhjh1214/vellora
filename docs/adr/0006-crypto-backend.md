# 0006. Cryptography: OpenSSL 3 for PKI/CMS, RustCrypto for primitives

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

- PDF encryption (Standard Security Handler R2–R6) needs RC4, AES-128/256, MD5 and SHA-2.
- Digital signatures (Phase 9) need CMS/PKCS#7, X.509 chain building, CRL, OCSP, RFC 3161 timestamps and PAdES profiles.
- Security correctness matters more than feature count.

## Decision

- **RustCrypto** crates for the primitives used by PDF encryption in `cos`.
- **OpenSSL 3** (Apache-2.0) for CMS signing and verification, chain validation, CRL, OCSP and TSA. RustCrypto `cms`/`x509-cert` are used for parsing and inspection where pure Rust suffices.
- **Keys:** PKCS#12 files, PKCS#11 tokens (`cryptoki`), Windows CNG and macOS Keychain. Key operations happen in the UI process, never in the engine.
- **We implement only the PDF side:** ByteRange, placeholder reservation, DSS/VRI, DocMDP/FieldMDP evaluation, modification-after-signing analysis. **We never implement crypto primitives or chain building.**

## Consequences

- OpenSSL becomes a native dependency in Phase 9 (packaging on all OSes).
- Verification is implemented before signing.
