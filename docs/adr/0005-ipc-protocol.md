# 0005. IPC: postcard over pipes, tiles in shared memory, Rust on both ends

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

The UI (C++/Qt) and the engine (Rust) need a versioned, efficient protocol. Pixels are large; control messages are small. A cross-language schema (protobuf, Cap'n Proto, FlatBuffers) adds tooling and drift risk.

## Decision

- Both protocol endpoints are **Rust**: `vellora-engine`, and `vellora-engine-client` linked into the UI. C++ talks to the client through `cxx`.
- Messages are `serde` types in `vellora-ipc`, encoded with **`postcard`** and framed with a length prefix over anonymous pipes (stdin/stdout of the engine, or OS pipes passed at spawn).
- Every frame has a size cap, checked before allocation. Every message is validated.
- The first exchange is a **protocol-version handshake**. A mismatch is a fatal, user-visible error.
- **Tiles travel through shared memory:** a client-owned slot ring. Messages carry only slot descriptors.
- **Requests** carry an id; responses and events reference it. **Cancellation** is a first-class message.

## Alternatives considered

| Option | Why not |
|---|---|
| protobuf / Cap'n Proto / FlatBuffers | Cross-language schema and codegen not needed when both ends are Rust |
| JSON | Slow and large for geometry-heavy messages; still fine for CLI `--json` output |
| gRPC | Network-oriented; heavy |

## Consequences

- Protocol evolution is a Rust-only change. Version bumps are explicit.
- The C++ side needs no knowledge of the wire format.
