//! The engine's launch arguments: what the parent hands over at spawn time.
//!
//! The engine never receives a path (ADR-0004). It gets two inherited open files, named by their
//! handle numbers, and the shape of the tile region; the IPC pipes are its standard input and
//! output. [`LaunchArgs::to_args`] and [`parse`] are each other's inverse, so the engine client
//! builds the command line with the same type the engine parses it with.

use std::time::Duration;

use vellora_shm::{HandleToken, SlotGeometry};

use crate::Deadlines;

/// Flag naming the document file.
pub const FILE_HANDLE: &str = "--file-handle";
/// Flag naming the shared tile region file.
pub const REGION_HANDLE: &str = "--region-handle";
/// Flag giving the number of slots in the region.
pub const REGION_SLOTS: &str = "--region-slots";
/// Flag giving the size of one slot in bytes.
pub const REGION_SLOT_BYTES: &str = "--region-slot-bytes";
/// Flag giving the largest document the engine will map.
pub const MAX_DOCUMENT_BYTES: &str = "--max-document-bytes";

/// Flag giving the per-tile soft deadline in milliseconds.
pub const SOFT_DEADLINE_MS: &str = "--soft-deadline-ms";
/// Flag giving the per-tile hard deadline in milliseconds. The engine aborts itself when a tile
/// is still rendering after it.
pub const HARD_DEADLINE_MS: &str = "--hard-deadline-ms";

/// Largest document accepted when the parent does not say otherwise: PDFium's file access takes
/// the length as a C `unsigned long`, which is 32 bits on Windows (ADR-0014).
pub const DEFAULT_MAX_DOCUMENT_BYTES: u64 = u32::MAX as u64;

/// Everything the parent tells the engine at launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaunchArgs {
    /// The open document file. The `Open` request names it by this same number.
    pub file: HandleToken,
    /// The open tile region file.
    pub region: HandleToken,
    /// How the region is cut into slots.
    pub geometry: SlotGeometry,
    /// Documents larger than this are refused before they are mapped.
    pub max_document_bytes: u64,
    /// How long a tile may take before it is logged (soft) and before the engine aborts (hard).
    pub deadlines: Deadlines,
}

impl LaunchArgs {
    /// The command-line arguments that make [`parse`] return `self`.
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        vec![
            FILE_HANDLE.to_owned(),
            self.file.to_string(),
            REGION_HANDLE.to_owned(),
            self.region.to_string(),
            REGION_SLOTS.to_owned(),
            self.geometry.slot_count().to_string(),
            REGION_SLOT_BYTES.to_owned(),
            self.geometry.slot_bytes().to_string(),
            MAX_DOCUMENT_BYTES.to_owned(),
            self.max_document_bytes.to_string(),
            SOFT_DEADLINE_MS.to_owned(),
            self.deadlines.soft.as_millis().to_string(),
            HARD_DEADLINE_MS.to_owned(),
            self.deadlines.hard.as_millis().to_string(),
        ]
    }
}

/// What the command line asks the process to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invocation {
    /// Serve a document with these arguments.
    Serve(LaunchArgs),
    /// Print the version and exit (`--version`, `-V`).
    Version,
}

/// Why the command line was refused.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ArgsError {
    /// Started without arguments, as a person would from a shell.
    #[error("vellora-engine is started by Vellora; it is not meant to be run directly")]
    NoArguments,
    /// A flag this version does not know.
    #[error("unknown argument {0:?}")]
    Unknown(String),
    /// A flag given twice.
    #[error("{0} was given more than once")]
    Duplicate(&'static str),
    /// A flag at the end of the line, without its value.
    #[error("{0} needs a value")]
    MissingValue(&'static str),
    /// A required flag that is absent.
    #[error("{0} is required")]
    Missing(&'static str),
    /// A flag whose value cannot be used.
    #[error("{flag}: {reason}")]
    Invalid {
        /// The flag.
        flag: &'static str,
        /// What is wrong with its value.
        reason: String,
    },
}

const FLAGS: [&str; 7] = [
    FILE_HANDLE,
    REGION_HANDLE,
    REGION_SLOTS,
    REGION_SLOT_BYTES,
    MAX_DOCUMENT_BYTES,
    SOFT_DEADLINE_MS,
    HARD_DEADLINE_MS,
];

/// Parses the arguments after the program name. Accepts `--flag value` and `--flag=value`.
///
/// # Errors
///
/// [`ArgsError`] for anything that is not a complete, valid launch.
pub fn parse<I>(args: I) -> Result<Invocation, ArgsError>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter().peekable();
    match args.peek().map(String::as_str) {
        None => return Err(ArgsError::NoArguments),
        Some("--version" | "-V") => return Ok(Invocation::Version),
        Some(_) => {}
    }

    let mut values: [Option<String>; FLAGS.len()] = Default::default();
    while let Some(argument) = args.next() {
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (argument.as_str(), None),
        };
        let index = FLAGS
            .iter()
            .position(|flag| *flag == name)
            .ok_or_else(|| ArgsError::Unknown(argument.clone()))?;
        let flag = FLAGS[index];
        if values[index].is_some() {
            return Err(ArgsError::Duplicate(flag));
        }
        values[index] = Some(
            inline
                .or_else(|| args.next())
                .ok_or(ArgsError::MissingValue(flag))?,
        );
    }

    let [file, region, slots, slot_bytes, max_bytes, soft, hard] = values;
    let required = |value: Option<String>, flag| value.ok_or(ArgsError::Missing(flag));
    let number = |text: String, flag: &'static str| -> Result<u64, ArgsError> {
        text.parse().map_err(|error| ArgsError::Invalid {
            flag,
            reason: format!("{text:?} is not a number: {error}"),
        })
    };
    let small = |value: u64, flag: &'static str| -> Result<u32, ArgsError> {
        u32::try_from(value).map_err(|_| ArgsError::Invalid {
            flag,
            reason: format!("{value} does not fit in 32 bits"),
        })
    };

    let file = HandleToken::new(number(required(file, FILE_HANDLE)?, FILE_HANDLE)?);
    let region = HandleToken::new(number(required(region, REGION_HANDLE)?, REGION_HANDLE)?);
    if file == region {
        return Err(ArgsError::Invalid {
            flag: REGION_HANDLE,
            reason: "must differ from the document handle".to_owned(),
        });
    }
    let slots = small(
        number(required(slots, REGION_SLOTS)?, REGION_SLOTS)?,
        REGION_SLOTS,
    )?;
    let slot_bytes = small(
        number(required(slot_bytes, REGION_SLOT_BYTES)?, REGION_SLOT_BYTES)?,
        REGION_SLOT_BYTES,
    )?;
    let geometry = SlotGeometry::new(slots, slot_bytes).map_err(|error| ArgsError::Invalid {
        flag: REGION_SLOTS,
        reason: error.to_string(),
    })?;
    let max_document_bytes = match max_bytes {
        Some(text) => number(text, MAX_DOCUMENT_BYTES)?,
        None => DEFAULT_MAX_DOCUMENT_BYTES,
    };

    let defaults = Deadlines::default();
    let millis = |text: Option<String>, flag, default: Duration| -> Result<Duration, ArgsError> {
        match text {
            Some(text) => {
                let value = number(text, flag)?;
                if value == 0 {
                    return Err(ArgsError::Invalid {
                        flag,
                        reason: "must be at least 1 ms".to_owned(),
                    });
                }
                Ok(Duration::from_millis(value))
            }
            None => Ok(default),
        }
    };
    let deadlines = Deadlines {
        soft: millis(soft, SOFT_DEADLINE_MS, defaults.soft)?,
        hard: millis(hard, HARD_DEADLINE_MS, defaults.hard)?,
    };

    Ok(Invocation::Serve(LaunchArgs {
        file,
        region,
        geometry,
        max_document_bytes,
        deadlines,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    const VALID: &str =
        "--file-handle 7 --region-handle 8 --region-slots 16 --region-slot-bytes 1048576";

    fn valid() -> LaunchArgs {
        LaunchArgs {
            file: HandleToken::new(7),
            region: HandleToken::new(8),
            geometry: SlotGeometry::new(16, 1 << 20).unwrap(),
            max_document_bytes: DEFAULT_MAX_DOCUMENT_BYTES,
            deadlines: Deadlines::default(),
        }
    }

    #[test]
    fn a_complete_launch_parses_with_the_default_limit() {
        assert_eq!(parse(args(VALID)), Ok(Invocation::Serve(valid())));
    }

    #[test]
    fn to_args_is_the_inverse_of_parse() {
        let launch = LaunchArgs {
            max_document_bytes: 123_456,
            deadlines: Deadlines {
                soft: Duration::from_millis(250),
                hard: Duration::from_millis(1500),
            },
            ..valid()
        };
        assert_eq!(parse(launch.to_args()), Ok(Invocation::Serve(launch)));
    }

    #[test]
    fn the_equals_form_and_any_order_work() {
        let line =
            "--region-slot-bytes=1048576 --region-slots=16 --region-handle=8 --file-handle=7";
        assert_eq!(parse(args(line)), Ok(Invocation::Serve(valid())));
    }

    #[test]
    fn version_is_recognised() {
        assert_eq!(parse(args("--version")), Ok(Invocation::Version));
        assert_eq!(parse(args("-V")), Ok(Invocation::Version));
    }

    #[test]
    fn no_arguments_is_its_own_error() {
        assert_eq!(parse(Vec::new()), Err(ArgsError::NoArguments));
    }

    #[test]
    fn mistakes_are_named() {
        let cases = [
            ("--bogus 1", "unknown argument"),
            (&format!("{VALID} --file-handle 9"), "more than once"),
            ("--file-handle", "needs a value"),
            (
                "--file-handle 7 --region-handle 8 --region-slots 16",
                "--region-slot-bytes is required",
            ),
            (
                "--file-handle x --region-handle 8 --region-slots 1 --region-slot-bytes 4",
                "not a number",
            ),
            (
                "--file-handle 7 --region-handle 7 --region-slots 1 --region-slot-bytes 4",
                "must differ",
            ),
            (
                "--file-handle 7 --region-handle 8 --region-slots 0 --region-slot-bytes 4",
                "at least one slot",
            ),
            (
                "--file-handle 7 --region-handle 8 --region-slots 1 --region-slot-bytes 5",
                "4-byte pixels",
            ),
            (
                "--file-handle 7 --region-handle 8 --region-slots 5000000000 --region-slot-bytes 4",
                "32 bits",
            ),
            (&format!("{VALID} --hard-deadline-ms 0"), "at least 1 ms"),
            (&format!("{VALID} --soft-deadline-ms soon"), "not a number"),
            (
                "--file-handle -1 --region-handle 8 --region-slots 1 --region-slot-bytes 4",
                "not a number",
            ),
        ];
        for (line, expected) in cases {
            let error = parse(args(line)).unwrap_err().to_string();
            assert!(error.contains(expected), "{line:?}: {error}");
        }
    }
}
