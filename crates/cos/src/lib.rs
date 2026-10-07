//! # vellora-cos — the PDF object layer
//!
//! **Responsibility:** everything between raw PDF bytes and PDF objects, in both
//! directions:
//!
//! - lexer and object parser (ISO 32000-2 §7.2–7.3)
//! - cross-reference tables, cross-reference streams, object streams and hybrid
//!   files (§7.5), plus a recovery scan for damaged files
//! - a lazy, revision-aware object store over a memory-mapped or read-handle
//!   byte source
//! - stream filters (§7.4) and Standard Security Handler decryption R2–R6 (§7.6)
//! - **the only PDF writer in Vellora:** incremental updates (§7.5.6) and full
//!   rewrites
//! - resource limits for every decode and recursion (`limits` module)
//!
//! **Boundaries:**
//!
//! - No rendering, no content-stream interpretation (that is `vellora-content`,
//!   created in Phase 4), no UI concerns.
//! - Treats all input as hostile. It never panics on input and never allocates
//!   without bound (CLAUDE.md invariant 5).
//! - `unsafe` is forbidden.
//!
//! **Status:** in progress (`docs/milestones/M0.md`). The parser is written from scratch
//! (ADR-0013). Done so far:
//!
//! - [`error`], [`limits`] and [`source`] (M0 task 3)
//! - the [`lexer`] (task 4) and the [`object`] model with its [`parser`] (task 5)
//! - cross-reference tables and the trailer chain in [`xref`] (task 6)
//! - cross-reference streams, hybrid files and [`objstm`] object streams (task 7)
//! - the [`recovery`] scan that rebuilds a damaged file's cross-reference, and [`Xref::open`]
//!   which falls back to it (task 8)
//! - the lazy [`ObjectStore`] with its bounded caches, and the lazy page-tree walk in [`pages`]
//!   (task 9)
//! - every stream filter of §7.4 except the image codecs, and filter chains, in [`filter`]
//!   (task 10)
//! - the Standard Security Handler (revisions 2–6) in [`crypt`], used by the [`ObjectStore`]
//!   (task 11)

pub mod crypt;
pub mod error;
pub mod filter;
pub mod lexer;
pub mod limits;
pub mod object;
pub mod objstm;
pub mod pages;
pub mod parser;
pub mod recovery;
pub mod source;
pub mod store;
pub mod xref;

pub use crypt::{CryptMethod, Decryptor, Encryption, EncryptionInfo, PasswordRole};
pub use error::{EncryptionError, Error, Result, SyntaxKind};
pub use limits::{DecodeBudget, LimitKind, Limits};
pub use object::{Dict, DictEntry, IndirectObject, ObjRef, Object, ObjectKind, Recovery, Stream};
pub use objstm::ObjectStream;
pub use pages::{Inherited, Page, Pages};
pub use parser::{LengthResolver, Parser};
pub use recovery::{RepairReason, object_header_at, rebuild};
pub use source::{ByteSource, FileSource, MemorySource};
pub use store::ObjectStore;
pub use xref::{Revision, SectionKind, Startxref, Xref, XrefEntry, XrefSection, XrefWarning};
