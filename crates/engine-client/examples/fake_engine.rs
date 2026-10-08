//! A stand-in for `vellora-engine` that misbehaves on purpose, for the client's deadline tests
//! (`tests/deadlines.rs`). Not part of the product: it is an example only so that `cargo test`
//! builds it next to the tests that start it.
//!
//! The behaviour comes from `VELLORA_FAKE_ENGINE` (the launch arguments the client passes are
//! ignored):
//!
//! - `mute`: starts and says nothing.
//! - `no-open`: says `Hello`, reads the client's requests, and never answers `Open`.
//! - `wedged`: opens a one-page document, then reads requests and never answers any of them.

use std::io::{self, BufReader, BufWriter};
use std::thread;
use std::time::Duration;

use vellora_ipc::{PROTOCOL_VERSION, PageSize, Request, Response, read_frame, write_frame};

fn main() {
    let mode = std::env::var("VELLORA_FAKE_ENGINE").unwrap_or_default();
    if mode == "mute" {
        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    }

    let mut out = BufWriter::new(io::stdout());
    let mut input = BufReader::new(io::stdin());
    let hello = Response::Hello {
        protocol_version: PROTOCOL_VERSION,
    };
    if write_frame(&mut out, &hello).is_err() {
        return;
    }
    // Ends when the client closes the pipe or kills this process.
    while let Ok(Some(request)) = read_frame::<_, Request>(&mut input) {
        if mode == "wedged" && matches!(request, Request::Open { .. }) {
            let opened = Response::Opened {
                page_count: 1,
                page_sizes: vec![PageSize {
                    width: 100.0,
                    height: 100.0,
                }],
                repaired: false,
            };
            if write_frame(&mut out, &opened).is_err() {
                return;
            }
        }
    }
}
