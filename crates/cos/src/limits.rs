//! Resource limits for everything `cos` decodes, parses or follows.
//!
//! Every decoder and every recursion must go through these (CLAUDE.md invariant 5). The defaults
//! are deliberately generous for real documents but finite: a hostile file must hit an error
//! long before it can exhaust memory. Callers (the engine) may tighten them per process.
//!
//! The corpus file `pdfjs-bomb_giant` (123 KB input) is the regression target: an unlimited
//! decoder needed about 9.5 GB for it (ADR-0013).

use crate::error::{Error, Result};

/// Which limit was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LimitKind {
    /// Decoded size of a single stream.
    DecodedStreamBytes,
    /// Total decoded bytes across a document ([`DecodeBudget`]).
    TotalDecodeBytes,
    /// Output/input ratio of a decoder.
    DecompressionRatio,
    /// Nesting depth of arrays and dictionaries.
    NestingDepth,
    /// Number of cross-reference entries.
    XrefEntries,
    /// Length of a string object in bytes.
    StringBytes,
    /// Length of a name object in bytes.
    NameBytes,
    /// Entries in one array.
    ArrayEntries,
    /// Entries in one dictionary.
    DictEntries,
}

impl LimitKind {
    /// Human-readable name, used in error messages.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::DecodedStreamBytes => "decoded stream size",
            Self::TotalDecodeBytes => "total decode budget",
            Self::DecompressionRatio => "decompression ratio",
            Self::NestingDepth => "nesting depth",
            Self::XrefEntries => "xref entry count",
            Self::StringBytes => "string length",
            Self::NameBytes => "name length",
            Self::ArrayEntries => "array entry count",
            Self::DictEntries => "dictionary entry count",
        }
    }
}

/// Output sizes up to this many bytes are never rejected by the ratio limit. Tiny streams can
/// legitimately have extreme ratios (a run of zeros), and the absolute limits still apply.
pub const RATIO_FLOOR_BYTES: u64 = 1024 * 1024;

/// The limits applied while reading one document.
///
/// Construct with [`Limits::default`] and adjust fields; the struct is non-exhaustive so new
/// limits can be added without breaking callers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum decoded size of one stream. Default 256 MiB.
    pub max_decoded_stream_bytes: u64,
    /// Total decoded bytes allowed per document. Default 2 GiB.
    pub max_total_decode_bytes: u64,
    /// Maximum output/input ratio once the output exceeds [`RATIO_FLOOR_BYTES`]. Default 1000.
    pub max_decompression_ratio: u64,
    /// Maximum nesting of arrays and dictionaries. Default 64.
    pub max_nesting_depth: u32,
    /// Maximum number of cross-reference entries. Default 8,388,608 (2^23).
    pub max_xref_entries: u64,
    /// Maximum string length in bytes. Default 16 MiB (embedded data can sit in strings).
    pub max_string_bytes: u64,
    /// Maximum name length in bytes. Default 4096 (the spec asks for 127; real files exceed it).
    pub max_name_bytes: u64,
    /// Maximum entries in one array. Default 1,048,576 (a flat `/Kids` array of a large file).
    pub max_array_entries: u64,
    /// Maximum entries in one dictionary. Default 262,144.
    pub max_dict_entries: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_decoded_stream_bytes: 256 * 1024 * 1024,
            max_total_decode_bytes: 2 * 1024 * 1024 * 1024,
            max_decompression_ratio: 1000,
            max_nesting_depth: 64,
            max_xref_entries: 1 << 23,
            max_string_bytes: 16 * 1024 * 1024,
            max_name_bytes: 4096,
            max_array_entries: 1 << 20,
            max_dict_entries: 1 << 18,
        }
    }
}

impl Limits {
    /// The configured maximum for a count- or size-style limit.
    ///
    /// [`LimitKind::DecompressionRatio`] returns the maximum ratio.
    #[must_use]
    pub fn max(&self, kind: LimitKind) -> u64 {
        match kind {
            LimitKind::DecodedStreamBytes => self.max_decoded_stream_bytes,
            LimitKind::TotalDecodeBytes => self.max_total_decode_bytes,
            LimitKind::DecompressionRatio => self.max_decompression_ratio,
            LimitKind::NestingDepth => u64::from(self.max_nesting_depth),
            LimitKind::XrefEntries => self.max_xref_entries,
            LimitKind::StringBytes => self.max_string_bytes,
            LimitKind::NameBytes => self.max_name_bytes,
            LimitKind::ArrayEntries => self.max_array_entries,
            LimitKind::DictEntries => self.max_dict_entries,
        }
    }

    /// Fails with [`Error::LimitExceeded`] when `value` is greater than the limit for `kind`.
    /// A value equal to the limit is allowed.
    ///
    /// # Errors
    /// [`Error::LimitExceeded`] when the limit is exceeded.
    pub fn check(&self, kind: LimitKind, value: u64, offset: Option<u64>) -> Result<()> {
        let max = self.max(kind);
        if value > max {
            return Err(Error::LimitExceeded {
                limit: kind,
                max,
                value,
                offset,
            });
        }
        Ok(())
    }

    /// Checks a decoder's progress against the stream-size and ratio limits.
    ///
    /// Call it as output grows (not only at the end), so a bomb is stopped early.
    /// `input_bytes` is the number of encoded bytes consumed so far.
    ///
    /// # Errors
    /// [`Error::LimitExceeded`] for [`LimitKind::DecodedStreamBytes`] or
    /// [`LimitKind::DecompressionRatio`].
    pub fn check_decode_progress(
        &self,
        input_bytes: u64,
        output_bytes: u64,
        offset: Option<u64>,
    ) -> Result<()> {
        self.check(LimitKind::DecodedStreamBytes, output_bytes, offset)?;
        if output_bytes > RATIO_FLOOR_BYTES {
            // Integer ratio rounded up; an empty input counts as one byte.
            let ratio = output_bytes.div_ceil(input_bytes.max(1));
            self.check(LimitKind::DecompressionRatio, ratio, offset)?;
        }
        Ok(())
    }
}

/// Tracks the total decoded bytes of one document against
/// [`Limits::max_total_decode_bytes`].
#[derive(Debug, Clone)]
pub struct DecodeBudget {
    max: u64,
    used: u64,
}

impl DecodeBudget {
    /// A fresh budget sized from `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Self {
        Self {
            max: limits.max_total_decode_bytes,
            used: 0,
        }
    }

    /// Bytes charged so far.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.used
    }

    /// Charges `bytes` of decoded output.
    ///
    /// # Errors
    /// [`Error::LimitExceeded`] for [`LimitKind::TotalDecodeBytes`] when the budget would be
    /// exceeded. Nothing is charged in that case.
    pub fn charge(&mut self, bytes: u64, offset: Option<u64>) -> Result<()> {
        let total = self.used.saturating_add(bytes);
        if total > self.max {
            return Err(Error::LimitExceeded {
                limit: LimitKind::TotalDecodeBytes,
                max: self.max,
                value: total,
                offset,
            });
        }
        self.used = total;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [LimitKind; 9] = [
        LimitKind::DecodedStreamBytes,
        LimitKind::TotalDecodeBytes,
        LimitKind::DecompressionRatio,
        LimitKind::NestingDepth,
        LimitKind::XrefEntries,
        LimitKind::StringBytes,
        LimitKind::NameBytes,
        LimitKind::ArrayEntries,
        LimitKind::DictEntries,
    ];

    #[test]
    fn documented_defaults() {
        let l = Limits::default();
        assert_eq!(l.max_decoded_stream_bytes, 256 * 1024 * 1024);
        assert_eq!(l.max_total_decode_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(l.max_decompression_ratio, 1000);
        assert_eq!(l.max_nesting_depth, 64);
        assert_eq!(l.max_xref_entries, 8_388_608);
        assert_eq!(l.max_string_bytes, 16 * 1024 * 1024);
        assert_eq!(l.max_name_bytes, 4096);
        assert_eq!(l.max_array_entries, 1_048_576);
        assert_eq!(l.max_dict_entries, 262_144);
    }

    #[test]
    fn every_default_limit_is_enforced_at_the_boundary() {
        let limits = Limits::default();
        for kind in ALL {
            let max = limits.max(kind);
            assert!(max > 0, "{kind:?} has a zero limit");
            limits
                .check(kind, max, None)
                .unwrap_or_else(|e| panic!("{kind:?} at max: {e}"));
            let err = limits.check(kind, max + 1, Some(42)).unwrap_err();
            match err {
                Error::LimitExceeded {
                    limit,
                    max: m,
                    value,
                    offset,
                } => {
                    assert_eq!((limit, m, value, offset), (kind, max, max + 1, Some(42)));
                }
                other => panic!("{kind:?}: wrong error {other:?}"),
            }
        }
    }

    #[test]
    fn limits_can_be_tightened() {
        let limits = Limits {
            max_nesting_depth: 2,
            ..Limits::default()
        };
        assert!(limits.check(LimitKind::NestingDepth, 2, None).is_ok());
        assert!(limits.check(LimitKind::NestingDepth, 3, None).is_err());
    }

    #[test]
    fn decode_progress_enforces_stream_size() {
        let limits = Limits {
            max_decoded_stream_bytes: 100,
            ..Limits::default()
        };
        assert!(limits.check_decode_progress(100, 100, None).is_ok());
        let err = limits.check_decode_progress(100, 101, None).unwrap_err();
        assert!(matches!(
            err,
            Error::LimitExceeded {
                limit: LimitKind::DecodedStreamBytes,
                ..
            }
        ));
    }

    #[test]
    fn decode_progress_enforces_ratio_only_above_the_floor() {
        let limits = Limits::default();
        // 1 KiB in, 1 MiB out is a 1024:1 ratio but exactly at the floor: allowed.
        assert!(
            limits
                .check_decode_progress(1024, RATIO_FLOOR_BYTES, None)
                .is_ok()
        );
        // One byte more crosses the floor and the ratio (1025 > 1000) applies.
        let err = limits
            .check_decode_progress(1024, RATIO_FLOOR_BYTES + 1, None)
            .unwrap_err();
        assert!(matches!(
            err,
            Error::LimitExceeded {
                limit: LimitKind::DecompressionRatio,
                max: 1000,
                ..
            }
        ));
        // 2 KiB in, same output: ratio ~512, fine.
        assert!(
            limits
                .check_decode_progress(2048, RATIO_FLOOR_BYTES + 1, None)
                .is_ok()
        );
    }

    #[test]
    fn decode_progress_handles_zero_input() {
        let limits = Limits::default();
        assert!(
            limits
                .check_decode_progress(0, RATIO_FLOOR_BYTES, None)
                .is_ok()
        );
        assert!(
            limits
                .check_decode_progress(0, RATIO_FLOOR_BYTES + 1, None)
                .is_err()
        );
    }

    #[test]
    fn decode_budget_is_enforced_and_does_not_charge_on_failure() {
        let limits = Limits {
            max_total_decode_bytes: 10,
            ..Limits::default()
        };
        let mut budget = DecodeBudget::new(&limits);
        budget.charge(6, None).unwrap();
        budget.charge(4, None).unwrap();
        assert_eq!(budget.used(), 10);
        let err = budget.charge(1, Some(9)).unwrap_err();
        assert!(matches!(
            err,
            Error::LimitExceeded {
                limit: LimitKind::TotalDecodeBytes,
                max: 10,
                value: 11,
                ..
            }
        ));
        assert_eq!(budget.used(), 10);
        // Saturating add: a huge charge cannot wrap around.
        assert!(budget.charge(u64::MAX, None).is_err());
    }

    #[test]
    fn default_budget_uses_the_two_gib_default() {
        let mut budget = DecodeBudget::new(&Limits::default());
        assert!(budget.charge(2 * 1024 * 1024 * 1024, None).is_ok());
        assert!(budget.charge(1, None).is_err());
    }
}
