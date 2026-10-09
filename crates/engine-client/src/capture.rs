//! The engine's log lines, read from its standard error.
//!
//! The engine is sandboxed and cannot write files, and it is untrusted: a compromised engine can
//! print anything. So its standard error is a pipe into this process, a thread per engine drains it
//! (a full pipe would stop the engine at its next log line), and what it printed is treated as
//! data: lines are cut at [`MAX_LINE_BYTES`], control characters are replaced, only
//! [`MAX_LOGGED_BYTES`] per engine are forwarded to the log files, and the last
//! [`TAIL_LINES`] stay in memory for crash reports (task 9c).

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::thread;

use tracing::Level;

use crate::logging::{self, LineRing};

/// Lines longer than this are cut.
pub(crate) const MAX_LINE_BYTES: usize = 4096;
/// Most bytes of one engine's output that are written to the log files.
pub(crate) const MAX_LOGGED_BYTES: usize = 1 << 20;
/// Lines kept in memory per client, for a crash report.
pub(crate) const TAIL_LINES: usize = 200;

/// The level an engine line announces (`tracing`'s format puts it after the timestamp), `info` if it
/// does not.
fn level_of(line: &str) -> Level {
    for word in line.split_whitespace().take(3) {
        match word {
            "ERROR" => return Level::ERROR,
            "WARN" => return Level::WARN,
            "DEBUG" => return Level::DEBUG,
            "TRACE" => return Level::TRACE,
            "INFO" => return Level::INFO,
            _ => {}
        }
    }
    Level::INFO
}

/// The line as it is stored: lossily decoded, control characters (other than a tab) replaced.
fn sanitize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|c| if c.is_control() && c != '\t' { '?' } else { c })
        .collect()
}

/// Reads one line of at most [`MAX_LINE_BYTES`] bytes, discarding the rest of a longer one.
/// `None` at end of input.
fn read_line(input: &mut impl BufRead) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    let mut seen_any = false;
    loop {
        let chunk = match input.fill_buf() {
            Ok(chunk) if !chunk.is_empty() => chunk,
            // End of input, or the pipe broke: what was read is the last line.
            _ => return seen_any.then_some(line),
        };
        seen_any = true;
        let (used, done) = chunk
            .iter()
            .position(|&b| b == b'\n')
            .map_or((chunk.len(), false), |at| (at + 1, true));
        let room = MAX_LINE_BYTES.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..used.saturating_sub(usize::from(done)).min(room)]);
        input.consume(used);
        if done {
            return Some(line);
        }
    }
}

/// Starts the thread that drains `stderr` of the engine process `pid`: lines go into `tail`, and,
/// while there are bytes left in the budget, into the log (or to this process's standard error when
/// logging is not set up, as before the pipe existed, so that development runs and tests still show
/// the engine's output).
pub(crate) fn spawn_reader(
    pid: u32,
    stderr: File,
    tail: Arc<LineRing>,
) -> Option<thread::JoinHandle<()>> {
    let spawned = thread::Builder::new()
        .name("vellora-engine-log".into())
        .spawn(move || {
            let mut input = BufReader::new(stderr);
            let mut forwarded = 0_usize;
            let mut cut = false;
            while let Some(bytes) = read_line(&mut input) {
                let line = sanitize(&bytes);
                // In the tail only after it was handed to the log, so that a reader who sees a
                // line in the tail and then flushes the log finds it in the files.
                if forwarded >= MAX_LOGGED_BYTES {
                    if !cut {
                        cut = true;
                        tracing::warn!(pid, "engine output is not logged any more (too much)");
                    }
                    tail.push(format!("[engine {pid}] {line}"));
                    continue;
                }
                forwarded += line.len() + 1;
                if logging::is_started() {
                    match level_of(&line) {
                        Level::ERROR => tracing::error!(target: "engine", pid, "{line}"),
                        Level::WARN => tracing::warn!(target: "engine", pid, "{line}"),
                        Level::INFO => tracing::info!(target: "engine", pid, "{line}"),
                        _ => tracing::debug!(target: "engine", pid, "{line}"),
                    }
                } else {
                    eprintln!("[engine {pid}] {line}");
                }
                tail.push(format!("[engine {pid}] {line}"));
            }
        });
    match spawned {
        Ok(handle) => Some(handle),
        Err(error) => {
            // Without the thread nobody drains the pipe; the engine will stall at its next log
            // line, which the client's deadlines turn into a restart.
            tracing::error!(%error, "cannot start the thread that reads the engine's log");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn lines(input: &[u8]) -> Vec<String> {
        let mut reader = Cursor::new(input.to_vec());
        std::iter::from_fn(|| read_line(&mut reader))
            .map(|l| sanitize(&l))
            .collect()
    }

    #[test]
    fn lines_are_split_on_newlines_and_the_last_may_be_unterminated() {
        assert_eq!(lines(b"one\ntwo\nthree"), ["one", "two", "three"]);
        assert_eq!(lines(b"\n\nx\n"), ["", "", "x"]);
        assert_eq!(lines(b""), Vec::<String>::new());
    }

    #[test]
    fn a_long_line_is_cut_and_the_next_one_is_intact() {
        let mut input = vec![b'a'; MAX_LINE_BYTES * 3 + 7];
        input.extend_from_slice(b"\nnext\n");
        let got = lines(&input);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].len(), MAX_LINE_BYTES);
        assert_eq!(got[1], "next");
    }

    #[test]
    fn control_characters_and_bad_utf8_cannot_reach_the_log() {
        let got = lines(b"esc\x1b[31m red\ttab\r\nnul\0 bell\x07 \xff\xfe end\n");
        assert_eq!(got[0], "esc?[31m red\ttab?");
        assert!(got[1].starts_with("nul? bell? "), "{}", got[1]);
        assert!(got[1].ends_with(" end"));
        assert!(got[1].contains('\u{fffd}'));
    }

    #[test]
    fn the_level_is_read_from_the_line() {
        assert_eq!(
            level_of("2026-10-09T07:27:52.35Z ERROR vellora_engine: x"),
            Level::ERROR
        );
        assert_eq!(
            level_of("2026-10-09T07:27:52.35Z  WARN vellora_engine: x"),
            Level::WARN
        );
        assert_eq!(
            level_of("2026-10-09T07:27:52.35Z DEBUG vellora_engine: x"),
            Level::DEBUG
        );
        assert_eq!(level_of("2026-10-09T07:27:52.35Z TRACE x"), Level::TRACE);
        assert_eq!(level_of("something else entirely"), Level::INFO);
        // Only the start of the line decides, not a word deep inside the message.
        assert_eq!(level_of("a b c d ERROR"), Level::INFO);
    }
}
