//! The `cxx` bridge: the whole of what the Qt shell can say to the engine client (ADR-0003).
//!
//! The surface is deliberately small and plain: integers, floats, strings and shared structs, no
//! Qt types, no callbacks. The shell drains [`EngineEvent`]s with `poll_events` from a timer (see
//! the event model in [`crate::client`]); functions that can fail throw a C++ exception carrying
//! the error text.
//!
//! The generated header is `vellora-engine-client/src/bridge.rs.h` (the build script runs
//! `cxx-build` on this file), and the Rust side links as a static library next to the shell.
//!
//! Reading a finished tile's pixels is not part of the bridge yet: the tile cache of task 21 owns
//! the slots and will expose its tiles to C++.

// The bridge macro expands to `unsafe` glue (extern "C" shims and `no_mangle` exports); the
// hand-written code below contains none.
#![allow(unsafe_code)]

use std::env;
use std::path::{Path, PathBuf};

use vellora_ipc::{ErrorKind, PageSize, Priority, RequestId, SlotId, TileRect};

use crate::cache::{DEFAULT_BUDGET_BYTES, TileCache};
use crate::client::{Client, ClientConfig, ClientError, Event, TileRequest};

/// Environment variable that overrides where [`open`] looks for `vellora-engine`. For development
/// and tests; an installed shell finds the engine next to its own executable.
pub const ENGINE_ENV: &str = "VELLORA_ENGINE";

// `cxx` turns enum variants into associated constants without carrying their doc comments over, so
// `missing_docs` flags variants that are described below. The comments on each type say which
// fields an event kind uses; they are the reference for the C++ side.
#[allow(missing_docs)]
#[cxx::bridge(namespace = "vellora")]
mod ffi {
    /// A page size in points; `0 x 0` when unknown.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct PageExtent {
        width: f32,
        height: f32,
    }

    /// What an [`EngineEvent`] is about; the fields it uses are listed per kind.
    #[derive(Debug)]
    enum EventKind {
        /// `page_count`, `repaired` (sent again after each restart).
        Opened,
        /// `request`, `slot`.
        TileReady,
        /// `failure`, `message`; `request` if `has_request`.
        RequestFailed,
        /// `lost`, `will_restart`, `message`.
        EngineCrashed,
        /// Nothing else; `Opened` follows.
        EngineRestarted,
        /// `message`. Every later call fails.
        Failed,
    }

    /// Broad class of a failure (the protocol's `ErrorKind`).
    #[derive(Debug)]
    enum FailureKind {
        None,
        VersionMismatch,
        InvalidRequest,
        OpenFailed,
        RenderFailed,
        Internal,
    }

    /// How urgently a tile is wanted.
    #[derive(Debug)]
    enum TilePriority {
        Visible,
        Prefetch,
        Thumbnail,
    }

    /// One thing that happened. A flat struct, so C++ needs no variant type.
    #[derive(Debug)]
    struct EngineEvent {
        kind: EventKind,
        /// The request an answer belongs to (`TileReady`, and `RequestFailed` if `has_request`).
        request: u64,
        has_request: bool,
        slot: u32,
        page_count: u32,
        repaired: bool,
        will_restart: bool,
        failure: FailureKind,
        message: String,
        /// Requests lost in a crash; ask again after `EngineRestarted`.
        lost: Vec<u64>,
    }

    extern "Rust" {
        /// An open document and the engine process rendering it.
        type EngineClient;

        /// Opens the document at `path` and starts its engine (found through `VELLORA_ENGINE`,
        /// else next to the running executable). `Opened` arrives as an event.
        fn open(path: &str) -> Result<Box<EngineClient>>;

        /// Number of pages; 0 until `Opened`.
        fn page_count(self: &EngineClient) -> u32;

        /// Size of a page in points; `0 x 0` before `Opened` and beyond the first 4096 pages.
        fn page_size(self: &EngineClient, page: u32) -> PageExtent;

        /// Asks for a tile of `page` at `scale` device pixels per point, rendered into `slot`.
        /// Returns the id its answer carries. Throws when the engine is down or the request is
        /// invalid.
        #[allow(clippy::too_many_arguments)]
        fn request_tile(
            self: &EngineClient,
            page: u32,
            scale: f32,
            x: u32,
            y: u32,
            width: u32,
            height: u32,
            slot: u32,
            priority: TilePriority,
        ) -> Result<u64>;

        /// Withdraws a request; it is never reported. `false` if it was not in flight.
        fn cancel(self: &EngineClient, request: u64) -> bool;

        /// Takes every queued event, oldest first. Never blocks.
        fn poll_events(self: &EngineClient) -> Vec<EngineEvent>;

        /// Ends the engine. Safe to call twice; the client is unusable afterwards.
        fn close(self: &mut EngineClient);
    }
}

pub use ffi::{EngineEvent, EventKind, FailureKind, PageExtent, TilePriority};

/// The client behind the bridge's opaque handle. `None` after `close`.
#[derive(Debug)]
pub struct EngineClient {
    client: Option<Client>,
}

/// Opens `path` with the engine found by [`engine_executable`].
///
/// # Errors
///
/// [`ClientError`], which the bridge turns into an exception.
pub fn open(path: &str) -> Result<Box<EngineClient>, ClientError> {
    let geometry = TileCache::geometry_for_budget(DEFAULT_BUDGET_BYTES)?;
    let config = ClientConfig::new(engine_executable(), geometry);
    open_with(config, Path::new(path))
}

/// Opens `path` with an explicit configuration (what [`open`] does after choosing one).
///
/// # Errors
///
/// [`ClientError`] if the document cannot be opened or the engine cannot be started.
pub fn open_with(config: ClientConfig, path: &Path) -> Result<Box<EngineClient>, ClientError> {
    let client = Client::open(config, path)?;
    Ok(Box::new(EngineClient {
        client: Some(client),
    }))
}

/// Where the engine executable is: `$VELLORA_ENGINE`, else `vellora-engine` in the directory of
/// the running executable (the layout of an installed shell; ADR-0004).
#[must_use]
pub fn engine_executable() -> PathBuf {
    if let Some(path) = env::var_os(ENGINE_ENV) {
        return PathBuf::from(path);
    }
    let name = format!("vellora-engine{}", env::consts::EXE_SUFFIX);
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

impl EngineClient {
    /// The client this handle wraps, for Rust callers (tests).
    #[must_use]
    pub fn client(&self) -> Option<&Client> {
        self.client.as_ref()
    }

    /// Number of pages; 0 until the document is open.
    pub fn page_count(&self) -> u32 {
        self.client
            .as_ref()
            .and_then(Client::page_count)
            .unwrap_or(0)
    }

    /// Size of a page in points; zero by zero when unknown.
    pub fn page_size(&self, page: u32) -> PageExtent {
        self.client
            .as_ref()
            .and_then(|client| client.page_size(page))
            .map_or(
                PageExtent {
                    width: 0.0,
                    height: 0.0,
                },
                |PageSize { width, height }| PageExtent { width, height },
            )
    }

    /// Asks for a tile; see [`Client::request_tile`].
    ///
    /// # Errors
    ///
    /// [`ClientError`] when the engine is down, the handle is closed or the request is invalid.
    #[allow(clippy::too_many_arguments)] // The flat argument list is the bridge's contract.
    pub fn request_tile(
        &self,
        page: u32,
        scale: f32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        slot: u32,
        priority: TilePriority,
    ) -> Result<u64, ClientError> {
        let client = self.client.as_ref().ok_or(ClientError::Closed)?;
        let id = client.request_tile(&TileRequest {
            page,
            scale,
            rect: TileRect {
                x,
                y,
                width,
                height,
            },
            slot: SlotId(slot),
            priority: priority.into(),
        })?;
        Ok(id.0)
    }

    /// Withdraws a request; false if it was not in flight.
    pub fn cancel(&self, request: u64) -> bool {
        self.client
            .as_ref()
            .is_some_and(|client| client.cancel(RequestId(request)))
    }

    /// Takes every queued event, oldest first.
    pub fn poll_events(&self) -> Vec<EngineEvent> {
        self.client
            .as_ref()
            .map(Client::poll_events)
            .unwrap_or_default()
            .into_iter()
            .map(EngineEvent::from)
            .collect()
    }

    /// Ends the engine. Safe to call twice.
    pub fn close(&mut self) {
        if let Some(client) = self.client.take() {
            client.close();
        }
    }
}

impl From<TilePriority> for Priority {
    fn from(priority: TilePriority) -> Self {
        match priority {
            TilePriority::Prefetch => Self::Prefetch,
            TilePriority::Thumbnail => Self::Thumbnail,
            // Includes values C++ could smuggle into the shared enum: the most urgent class is
            // the one a bogus value hurts least (it only reorders the caller's own tiles).
            _ => Self::Visible,
        }
    }
}

impl From<ErrorKind> for FailureKind {
    fn from(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::VersionMismatch => Self::VersionMismatch,
            ErrorKind::InvalidRequest => Self::InvalidRequest,
            ErrorKind::OpenFailed => Self::OpenFailed,
            ErrorKind::RenderFailed => Self::RenderFailed,
            ErrorKind::Internal => Self::Internal,
        }
    }
}

impl EngineEvent {
    fn empty(kind: EventKind) -> Self {
        Self {
            kind,
            request: 0,
            has_request: false,
            slot: 0,
            page_count: 0,
            repaired: false,
            will_restart: false,
            failure: FailureKind::None,
            message: String::new(),
            lost: Vec::new(),
        }
    }
}

impl From<Event> for EngineEvent {
    fn from(event: Event) -> Self {
        match event {
            Event::Opened {
                page_count,
                repaired,
                ..
            } => Self {
                page_count,
                repaired,
                ..Self::empty(EventKind::Opened)
            },
            Event::TileReady { request, slot } => Self {
                request: request.0,
                has_request: true,
                slot: slot.0,
                ..Self::empty(EventKind::TileReady)
            },
            Event::RequestFailed {
                request,
                kind,
                message,
            } => Self {
                request: request.map_or(0, |id| id.0),
                has_request: request.is_some(),
                failure: kind.into(),
                message,
                ..Self::empty(EventKind::RequestFailed)
            },
            Event::EngineCrashed {
                crash,
                lost,
                will_restart,
            } => Self {
                will_restart,
                message: crash.to_string(),
                lost: lost.into_iter().map(|id| id.0).collect(),
                ..Self::empty(EventKind::EngineCrashed)
            },
            Event::EngineRestarted => Self::empty(EventKind::EngineRestarted),
            Event::Failed { reason } => Self {
                message: reason,
                ..Self::empty(EventKind::Failed)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use vellora_ipc::RequestId;

    use super::*;
    use crate::process::{Crash, Termination};

    #[test]
    fn the_header_is_generated_in_the_build() {
        // `build.rs` points at the header `cxx-build` wrote for this file.
        let header = fs::read_to_string(env!("VELLORA_BRIDGE_HEADER"))
            .expect("the build generates the C++ header");
        for expected in [
            "namespace vellora",
            "struct EngineEvent",
            "enum class EventKind",
            "struct EngineClient final : public ::rust::Opaque",
            "::rust::Box<::vellora::EngineClient> open(::rust::Str path)",
            "request_tile(",
            "poll_events()",
            "void close()",
        ] {
            assert!(header.contains(expected), "header lacks `{expected}`");
        }
    }

    #[test]
    fn events_convert_field_by_field() {
        let tile = EngineEvent::from(Event::TileReady {
            request: RequestId(7),
            slot: SlotId(3),
        });
        assert_eq!(
            (tile.kind, tile.request, tile.has_request, tile.slot),
            (EventKind::TileReady, 7, true, 3)
        );

        let failed = EngineEvent::from(Event::RequestFailed {
            request: None,
            kind: ErrorKind::OpenFailed,
            message: "no pages".into(),
        });
        assert_eq!(
            (failed.kind, failed.has_request, failed.failure),
            (EventKind::RequestFailed, false, FailureKind::OpenFailed)
        );
        assert_eq!(failed.message, "no pages");

        let crashed = EngineEvent::from(Event::EngineCrashed {
            crash: Crash {
                termination: Termination::Failure(3),
                killed_by_client: false,
            },
            lost: vec![RequestId(4), RequestId(9)],
            will_restart: true,
        });
        assert_eq!(crashed.kind, EventKind::EngineCrashed);
        assert!(crashed.will_restart);
        assert_eq!(crashed.lost, [4, 9]);
        assert!(
            crashed.message.contains("Failure(3)"),
            "{}",
            crashed.message
        );

        let opened = EngineEvent::from(Event::Opened {
            page_count: 10_000,
            page_sizes: Vec::new(),
            repaired: true,
        });
        assert_eq!((opened.page_count, opened.repaired), (10_000, true));
    }

    #[test]
    fn a_closed_handle_answers_without_failing() {
        let mut handle = EngineClient { client: None };
        assert_eq!(handle.page_count(), 0);
        assert_eq!(
            handle.page_size(0),
            PageExtent {
                width: 0.0,
                height: 0.0
            }
        );
        assert!(handle.poll_events().is_empty());
        assert!(!handle.cancel(1));
        assert!(matches!(
            handle.request_tile(0, 1.0, 0, 0, 8, 8, 0, TilePriority::Visible),
            Err(ClientError::Closed)
        ));
        handle.close();
        handle.close();
    }

    #[test]
    fn the_engine_is_found_next_to_the_executable_by_default() {
        // Only the shape is checked: the environment override is not set from a test.
        let path = engine_executable();
        assert!(
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("vellora-engine"))
                || env::var_os(ENGINE_ENV).is_some(),
            "{}",
            path.display()
        );
    }
}
