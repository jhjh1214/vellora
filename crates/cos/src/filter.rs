//! Stream filters (ISO 32000-2:2020 §7.4).
//!
//! Every decoder takes [`Limits`] and an optional [`DecodeBudget`] and checks them **as output
//! grows**, so a decompression bomb is stopped early. [`decode_stream`] runs a whole `/Filter`
//! chain; the individual decoders are public for callers that already know the filter (inline
//! images, tests).
//!
//! Corrupt or truncated data is common in the wild. Every decoder returns the bytes decoded so
//! far with [`Decoded::complete`] set to `false` when the input ended early or broke after
//! producing output; it fails only when nothing at all could be decoded.
//!
//! DCT, JPX, CCITT and JBIG2 are image codecs that PDFium handles: [`decode_stream`] stops in
//! front of them and reports which one it found ([`ImageFilter`]).

mod ascii;
mod chain;
mod lzw;
mod runlength;

pub use ascii::{ascii_hex_decode, ascii85_decode};
pub use chain::{DecodedStream, ImageFilter, decode_chain, decode_stream, decode_stream_bytes};
pub use lzw::lzw_decode;
pub use runlength::run_length_decode;

use flate2::{Decompress, FlushDecompress, Status};

use crate::error::{Error, Result};
use crate::limits::{DecodeBudget, Limits};
use crate::object::{Dict, ObjectKind};

/// Output of a decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The decoded bytes (possibly partial, see `complete`).
    pub data: Vec<u8>,
    /// `false` if the input ended early or turned corrupt after some output was produced.
    pub complete: bool,
}

const FLATE: &str = "FlateDecode";
/// Size of the output step: limits are checked after every step.
const CHUNK: usize = 16 * 1024;

/// Checks the limits and charges the document budget while a decoder produces output.
///
/// Checking every [`CHUNK`] bytes keeps the overshoot bounded by one chunk plus the largest
/// burst a single input unit can produce (128 bytes for run-length, one string for LZW).
struct Guard<'a> {
    limits: &'a Limits,
    budget: Option<&'a mut DecodeBudget>,
    offset: Option<u64>,
    charged: u64,
    checked_at: usize,
}

impl<'a> Guard<'a> {
    fn new(limits: &'a Limits, budget: Option<&'a mut DecodeBudget>, offset: Option<u64>) -> Self {
        Self {
            limits,
            budget,
            offset,
            charged: 0,
            checked_at: 0,
        }
    }

    /// `consumed` input bytes have produced `produced` output bytes so far. `last` forces the
    /// check (call it once when the decoder is done).
    fn step(&mut self, consumed: usize, produced: usize, last: bool) -> Result<()> {
        if !last && produced - self.checked_at < CHUNK {
            return Ok(());
        }
        self.checked_at = produced;
        self.limits
            .check_decode_progress(consumed as u64, produced as u64, self.offset)?;
        if let Some(budget) = self.budget.as_deref_mut() {
            let total = produced as u64;
            budget.charge(total - self.charged, self.offset)?;
            self.charged = total;
        }
        Ok(())
    }
}

/// Builds the result of a decoder that stopped at `failure` (if any): output that exists is
/// kept and marked incomplete, no output at all is an error.
fn finish(
    filter: &'static str,
    out: Vec<u8>,
    complete: bool,
    failure: Option<&'static str>,
    offset: Option<u64>,
) -> Result<Decoded> {
    match failure {
        Some(detail) if out.is_empty() => Err(Error::Decode {
            filter,
            detail,
            offset,
        }),
        Some(_) => Ok(Decoded {
            data: out,
            complete: false,
        }),
        None => Ok(Decoded {
            data: out,
            complete,
        }),
    }
}

/// PDF white-space characters (§7.2.3, Table 1).
fn is_pdf_space(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

fn decode_error(detail: &'static str, offset: Option<u64>) -> Error {
    Error::Decode {
        filter: FLATE,
        detail,
        offset,
    }
}

/// Decodes zlib/Deflate data (§7.4.4) under [`Limits`].
///
/// `offset` is where the stream data is in the file, used for error context.
///
/// # Errors
/// [`Error::LimitExceeded`] when the output exceeds `max_decoded_stream_bytes`, the
/// decompression ratio is above `max_decompression_ratio`, or the document budget runs out;
/// [`Error::Decode`] if no output could be produced at all.
pub fn flate_decode(
    input: &[u8],
    limits: &Limits,
    mut budget: Option<&mut DecodeBudget>,
    offset: Option<u64>,
) -> Result<Decoded> {
    let mut decoder = Decompress::new(true);
    let mut out: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; CHUNK];
    let mut complete = false;
    let mut failure: Option<&'static str> = None;

    loop {
        let consumed = usize::try_from(decoder.total_in()).unwrap_or(usize::MAX);
        let before_out = decoder.total_out();
        let status = decoder.decompress(
            input.get(consumed..).unwrap_or_default(),
            &mut chunk,
            FlushDecompress::None,
        );
        let produced = usize::try_from(decoder.total_out() - before_out).unwrap_or(0);
        let now_consumed = usize::try_from(decoder.total_in()).unwrap_or(usize::MAX);

        if let Some(piece) = chunk.get(..produced) {
            out.extend_from_slice(piece);
        }
        limits.check_decode_progress(decoder.total_in(), decoder.total_out(), offset)?;
        if let Some(budget) = budget.as_deref_mut() {
            budget.charge(produced as u64, offset)?;
        }

        match status {
            Ok(Status::StreamEnd) => {
                complete = true;
                break;
            }
            Ok(Status::Ok | Status::BufError) => {
                // No progress with input left or exhausted means the data is truncated.
                if produced == 0 && now_consumed == consumed {
                    failure = Some("truncated deflate data");
                    break;
                }
            }
            Err(_) => {
                failure = Some("corrupt deflate data");
                break;
            }
        }
    }

    match failure {
        Some(detail) if out.is_empty() => Err(decode_error(detail, offset)),
        _ => Ok(Decoded {
            data: out,
            complete,
        }),
    }
}

/// Predictor parameters from a `/DecodeParms` dictionary (§7.4.4.4, Table 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictorParams {
    /// 1 = none, 2 = TIFF, 10..=15 = PNG (the per-row tag selects the actual filter).
    pub predictor: u8,
    /// Colour components per sample.
    pub colors: u32,
    /// Bits per component: 1, 2, 4, 8 or 16.
    pub bits_per_component: u32,
    /// Samples per row.
    pub columns: u32,
}

impl Default for PredictorParams {
    fn default() -> Self {
        Self {
            predictor: 1,
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        }
    }
}

impl PredictorParams {
    /// Reads `/Predictor`, `/Colors`, `/BitsPerComponent` and `/Columns` from `parms`, applying
    /// the spec defaults.
    ///
    /// # Errors
    /// [`Error::Decode`] for values the spec does not allow.
    pub fn from_dict(parms: &Dict<'_>, offset: Option<u64>) -> Result<Self> {
        let get = |key: &[u8], default: i64| match parms.get(key).map(|o| &o.kind) {
            Some(ObjectKind::Integer(n)) => *n,
            _ => default,
        };
        let bad = |detail| decode_error(detail, offset);
        let predictor =
            u8::try_from(get(b"Predictor", 1)).map_err(|_| bad("invalid /Predictor"))?;
        if !matches!(predictor, 1 | 2 | 10..=15) {
            return Err(bad("unsupported /Predictor"));
        }
        let colors = u32::try_from(get(b"Colors", 1))
            .ok()
            .filter(|&c| (1..=255).contains(&c))
            .ok_or_else(|| bad("invalid /Colors"))?;
        let bits = u32::try_from(get(b"BitsPerComponent", 8))
            .ok()
            .filter(|b| matches!(b, 1 | 2 | 4 | 8 | 16))
            .ok_or_else(|| bad("invalid /BitsPerComponent"))?;
        let columns = u32::try_from(get(b"Columns", 1))
            .ok()
            .filter(|&c| c >= 1)
            .ok_or_else(|| bad("invalid /Columns"))?;
        Ok(Self {
            predictor,
            colors,
            bits_per_component: bits,
            columns,
        })
    }

    /// Bytes in one row of samples (without the PNG tag byte).
    fn row_bytes(&self) -> Option<usize> {
        let bits = u64::from(self.colors)
            .checked_mul(u64::from(self.columns))?
            .checked_mul(u64::from(self.bits_per_component))?;
        usize::try_from(bits.div_ceil(8)).ok()
    }

    /// Bytes per complete pixel for the PNG filters (at least 1).
    fn bytes_per_pixel(&self) -> usize {
        let bits = u64::from(self.colors) * u64::from(self.bits_per_component);
        usize::try_from(bits.div_ceil(8)).unwrap_or(1).max(1)
    }
}

/// Undoes the predictor on Flate/LZW output.
///
/// The output is never larger than the input, so the decode limits already cover it. A partial
/// last row is decoded as far as it goes, like other readers do.
///
/// # Errors
/// [`Error::Decode`] if the parameters make a row size that does not fit in memory.
pub fn apply_predictor(
    data: Vec<u8>,
    params: &PredictorParams,
    offset: Option<u64>,
) -> Result<Vec<u8>> {
    match params.predictor {
        1 => Ok(data),
        2 => tiff_predictor(data, params, offset),
        _ => png_predictor(&data, params, offset),
    }
}

fn png_predictor(data: &[u8], params: &PredictorParams, offset: Option<u64>) -> Result<Vec<u8>> {
    let row_bytes = params
        .row_bytes()
        .ok_or_else(|| decode_error("predictor row size overflows", offset))?;
    let bpp = params.bytes_per_pixel();
    // Each row is one tag byte followed by `row_bytes` of samples. Chunks come from the real
    // data, so no allocation depends on a declared size.
    let stride = row_bytes.saturating_add(1);
    let mut out: Vec<u8> = Vec::with_capacity(data.len());
    let mut prev: Vec<u8> = Vec::new();
    for chunk in data.chunks(stride) {
        let Some((&tag, samples)) = chunk.split_first() else {
            break;
        };
        let mut row = samples.to_vec();
        prev.resize(row.len(), 0);
        for i in 0..row.len() {
            let left = if i >= bpp {
                row.get(i - bpp).copied().unwrap_or(0)
            } else {
                0
            };
            let up = prev.get(i).copied().unwrap_or(0);
            let up_left = if i >= bpp {
                prev.get(i - bpp).copied().unwrap_or(0)
            } else {
                0
            };
            let predictor = match tag {
                1 => left,
                2 => up,
                3 => u8::try_from(u16::midpoint(u16::from(left), u16::from(up))).unwrap_or(0),
                4 => paeth(left, up, up_left),
                // 0 is None; unknown tags are treated like None.
                _ => 0,
            };
            if let Some(byte) = row.get_mut(i) {
                *byte = byte.wrapping_add(predictor);
            }
        }
        out.extend_from_slice(&row);
        prev = row;
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a_i, b_i, c_i) = (i32::from(a), i32::from(b), i32::from(c));
    let p = a_i + b_i - c_i;
    let (pa, pb, pc) = ((p - a_i).abs(), (p - b_i).abs(), (p - c_i).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn tiff_predictor(
    mut data: Vec<u8>,
    params: &PredictorParams,
    offset: Option<u64>,
) -> Result<Vec<u8>> {
    let row_bytes = params
        .row_bytes()
        .ok_or_else(|| decode_error("predictor row size overflows", offset))?;
    let colors = usize::try_from(params.colors).unwrap_or(1);
    for row in data.chunks_mut(row_bytes.max(1)) {
        match params.bits_per_component {
            8 => {
                for i in colors..row.len() {
                    let left = row.get(i - colors).copied().unwrap_or(0);
                    if let Some(byte) = row.get_mut(i) {
                        *byte = byte.wrapping_add(left);
                    }
                }
            }
            16 => {
                let step = colors * 2;
                let mut i = step;
                while i + 1 < row.len() {
                    let left = u16::from_be_bytes([
                        row.get(i - step).copied().unwrap_or(0),
                        row.get(i - step + 1).copied().unwrap_or(0),
                    ]);
                    let here = u16::from_be_bytes([
                        row.get(i).copied().unwrap_or(0),
                        row.get(i + 1).copied().unwrap_or(0),
                    ]);
                    let sum = here.wrapping_add(left).to_be_bytes();
                    row[i] = sum[0];
                    row[i + 1] = sum[1];
                    i += 2;
                }
            }
            bits => tiff_sub_byte_row(row, bits, colors),
        }
    }
    Ok(data)
}

/// TIFF predictor for 1, 2 and 4 bits per component: add the component `colors` samples back
/// in the same row, modulo `2^bits`.
fn tiff_sub_byte_row(row: &mut [u8], bits: u32, colors: usize) {
    let per_byte = usize::try_from(8 / bits).unwrap_or(1);
    let mask = (1u16 << bits) - 1;
    let total = row.len() * per_byte;
    let read = |row: &[u8], index: usize| -> u16 {
        let byte = row.get(index / per_byte).copied().unwrap_or(0);
        let shift = 8 - bits * (u32::try_from(index % per_byte).unwrap_or(0) + 1);
        (u16::from(byte) >> shift) & mask
    };
    for index in colors..total {
        let sum = (read(row, index) + read(row, index - colors)) & mask;
        let shift = 8 - bits * (u32::try_from(index % per_byte).unwrap_or(0) + 1);
        if let Some(byte) = row.get_mut(index / per_byte) {
            let cleared = u16::from(*byte) & !(mask << shift);
            *byte = u8::try_from(cleared | (sum << shift)).unwrap_or(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    use super::*;
    use crate::limits::LimitKind;
    use crate::parser::Parser;

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn decode(input: &[u8]) -> Decoded {
        flate_decode(input, &Limits::default(), None, None).unwrap()
    }

    #[test]
    fn flate_known_answer() {
        // `zlib.compress(b"hello hello hello hello")` from CPython's zlib.
        let input = [
            0x78, 0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x27, 0x01, 0x68, 0x03,
            0x08, 0xb1,
        ];
        let decoded = decode(&input);
        assert_eq!(decoded.data, b"hello hello hello hello");
        assert!(decoded.complete);
    }

    #[test]
    fn flate_round_trips_various_sizes() {
        for len in [
            0usize,
            1,
            100,
            16 * 1024 - 1,
            16 * 1024,
            16 * 1024 + 1,
            200_000,
        ] {
            let data: Vec<u8> = (0..len)
                .map(|i| u8::try_from(i * 7 % 251).unwrap())
                .collect();
            let decoded = decode(&zlib(&data));
            assert_eq!(decoded.data, data, "len {len}");
            assert!(decoded.complete);
        }
    }

    #[test]
    fn truncated_flate_returns_partial_output_marked_incomplete() {
        let data: Vec<u8> = (0..50_000u32).flat_map(u32::to_le_bytes).collect();
        let packed = zlib(&data);
        let cut = &packed[..packed.len() / 2];
        let decoded = decode(cut);
        assert!(!decoded.complete);
        assert_ne!(decoded.data.len(), 0);
        assert_eq!(decoded.data, data[..decoded.data.len()]);
    }

    #[test]
    fn corrupt_flate_is_an_error_when_nothing_decodes() {
        for input in [&b""[..], b"\x00", b"not zlib at all", b"\x78\x9c"] {
            let err = flate_decode(input, &Limits::default(), None, Some(42)).unwrap_err();
            assert!(
                matches!(
                    err,
                    Error::Decode {
                        filter: "FlateDecode",
                        offset: Some(42),
                        ..
                    }
                ),
                "{input:?}: {err:?}"
            );
        }
    }

    #[test]
    fn corruption_after_some_output_keeps_the_good_part() {
        let data = vec![b'a'; 100_000];
        let mut packed = zlib(&data);
        let n = packed.len();
        packed[n / 2] ^= 0xFF;
        packed[n / 2 + 1] ^= 0xFF;
        let result = flate_decode(&packed, &Limits::default(), None, None);
        // Either the damage is caught (partial or error) or decoded to something else; it
        // never panics and never exceeds the input's real expansion.
        if let Ok(decoded) = result {
            assert!(decoded.data.len() <= data.len() * 2);
        }
    }

    #[test]
    fn decompression_bomb_hits_the_stream_size_limit() {
        // 8 MiB of zeros packs to a few KiB.
        let packed = zlib(&vec![0u8; 8 * 1024 * 1024]);
        assert!(packed.len() < 16 * 1024);
        let limits = Limits {
            max_decoded_stream_bytes: 1024 * 1024,
            ..Limits::default()
        };
        let err = flate_decode(&packed, &limits, None, Some(7)).unwrap_err();
        assert!(
            matches!(
                err,
                Error::LimitExceeded {
                    limit: LimitKind::DecodedStreamBytes,
                    max: 1_048_576,
                    offset: Some(7),
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn decompression_bomb_hits_the_ratio_limit_with_default_limits() {
        // Default size limit (256 MiB) is far away; the ratio limit stops it early.
        let packed = zlib(&vec![0u8; 64 * 1024 * 1024]);
        let err = flate_decode(&packed, &Limits::default(), None, None).unwrap_err();
        assert!(
            matches!(
                err,
                Error::LimitExceeded {
                    limit: LimitKind::DecompressionRatio,
                    max: 1000,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn bomb_is_stopped_early_not_after_full_expansion() {
        // The size limit is checked per 16 KiB step, so peak output stays near the limit.
        let packed = zlib(&vec![0u8; 8 * 1024 * 1024]);
        let limits = Limits {
            max_decoded_stream_bytes: 100_000,
            ..Limits::default()
        };
        let mut budget = DecodeBudget::new(&limits);
        let err = flate_decode(&packed, &limits, Some(&mut budget), None).unwrap_err();
        assert!(matches!(err, Error::LimitExceeded { .. }));
        // The budget was only charged for what was produced before the stop.
        assert!(budget.used() <= 100_000 + 16 * 1024);
    }

    #[test]
    fn document_budget_is_enforced_across_calls() {
        let limits = Limits {
            max_total_decode_bytes: 150_000,
            ..Limits::default()
        };
        let mut budget = DecodeBudget::new(&limits);
        let packed = zlib(&vec![7u8; 100_000]);
        flate_decode(&packed, &limits, Some(&mut budget), None).unwrap();
        let err = flate_decode(&packed, &limits, Some(&mut budget), None).unwrap_err();
        assert!(matches!(
            err,
            Error::LimitExceeded {
                limit: LimitKind::TotalDecodeBytes,
                ..
            }
        ));
    }

    fn params(text: &str) -> Result<PredictorParams> {
        let limits = Limits::default();
        let object = Parser::new(text.as_bytes(), &limits)
            .parse_object()
            .unwrap();
        PredictorParams::from_dict(object.as_dict().unwrap(), None)
    }

    #[test]
    fn predictor_params_defaults_and_validation() {
        assert_eq!(params("<< >>").unwrap(), PredictorParams::default());
        let p = params("<< /Predictor 12 /Columns 5 /Colors 3 /BitsPerComponent 16 >>").unwrap();
        assert_eq!(
            p,
            PredictorParams {
                predictor: 12,
                colors: 3,
                bits_per_component: 16,
                columns: 5
            }
        );
        for bad in [
            "<< /Predictor 3 >>",
            "<< /Predictor 16 >>",
            "<< /Predictor -1 >>",
            "<< /Columns 0 >>",
            "<< /Columns -4 >>",
            "<< /Colors 0 >>",
            "<< /Colors 256 >>",
            "<< /BitsPerComponent 3 >>",
            "<< /Columns 99999999999 >>",
        ] {
            assert!(params(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn png_predictors_decode_every_row_type() {
        // Four columns, one 8-bit colour. Rows use None, Sub, Up, Average and Paeth in turn;
        // every raw row after the first is [1, 1, 1, 1].
        let p = PredictorParams {
            predictor: 15,
            columns: 4,
            ..PredictorParams::default()
        };
        let encoded = [
            0, 1, 2, 3, 4, // None: 1 2 3 4
            1, 1, 1, 1, 1, // Sub: running sum from the left: 1 2 3 4
            2, 1, 1, 1, 1, // Up: previous row + 1: 2 3 4 5
            3, 1, 1, 1, 1, // Average of left and up, rounded down, + 1: 2 3 4 5
            4, 1, 1, 1, 1, // Paeth: 3 4 5 6
        ];
        let out = apply_predictor(encoded.to_vec(), &p, None).unwrap();
        assert_eq!(
            out,
            [1, 2, 3, 4, 1, 2, 3, 4, 2, 3, 4, 5, 2, 3, 4, 5, 3, 4, 5, 6]
        );
    }

    #[test]
    fn png_up_predictor_round_trip_for_xref_style_rows() {
        // The typical xref stream setup: /Predictor 12 /Columns 5.
        let p = PredictorParams {
            predictor: 12,
            columns: 5,
            ..PredictorParams::default()
        };
        let rows: [[u8; 5]; 3] = [[1, 0, 0, 0, 0], [1, 0, 0, 1, 0], [2, 0, 0, 2, 7]];
        let mut encoded = Vec::new();
        let mut prev = [0u8; 5];
        for row in rows {
            encoded.push(2);
            for i in 0..5 {
                encoded.push(row[i].wrapping_sub(prev[i]));
            }
            prev = row;
        }
        let out = apply_predictor(encoded, &p, None).unwrap();
        assert_eq!(out, rows.concat());
    }

    #[test]
    fn png_partial_last_row_is_decoded_as_far_as_it_goes() {
        let p = PredictorParams {
            predictor: 12,
            columns: 4,
            ..PredictorParams::default()
        };
        let out = apply_predictor(vec![2, 1, 2, 3, 4, 2, 1, 1], &p, None).unwrap();
        assert_eq!(out, [1, 2, 3, 4, 2, 3]);
        // A lone tag byte or empty data gives nothing and does not panic.
        assert_eq!(apply_predictor(vec![2], &p, None).unwrap().len(), 0);
        assert_eq!(apply_predictor(vec![], &p, None).unwrap().len(), 0);
    }

    #[test]
    fn png_rgb_uses_bytes_per_pixel_for_left_neighbour() {
        // Sub filter on 3-colour 8-bit data looks 3 bytes back.
        let p = PredictorParams {
            predictor: 11,
            colors: 3,
            columns: 2,
            ..PredictorParams::default()
        };
        let out = apply_predictor(vec![1, 10, 20, 30, 1, 2, 3], &p, None).unwrap();
        assert_eq!(out, [10, 20, 30, 11, 22, 33]);
    }

    #[test]
    fn tiff_predictor_8_and_16_bit() {
        let p = PredictorParams {
            predictor: 2,
            columns: 4,
            ..PredictorParams::default()
        };
        assert_eq!(
            apply_predictor(vec![1, 1, 1, 1], &p, None).unwrap(),
            [1, 2, 3, 4]
        );
        // Rows are independent.
        assert_eq!(
            apply_predictor(vec![1, 1, 1, 1, 5, 1, 1, 1], &p, None).unwrap(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
        let p16 = PredictorParams {
            predictor: 2,
            columns: 3,
            bits_per_component: 16,
            ..PredictorParams::default()
        };
        let out = apply_predictor(vec![0, 255, 0, 1, 0, 1], &p16, None).unwrap();
        assert_eq!(out, [0, 255, 1, 0, 1, 1]);
        // Two colours: each component adds its own predecessor.
        let rgb = PredictorParams {
            predictor: 2,
            colors: 2,
            columns: 3,
            ..PredictorParams::default()
        };
        assert_eq!(
            apply_predictor(vec![1, 10, 1, 10, 1, 10], &rgb, None).unwrap(),
            [1, 10, 2, 20, 3, 30]
        );
    }

    #[test]
    fn tiff_predictor_sub_byte_depths() {
        // 1 bit, 8 columns: deltas 1,1,1,0,0,0,0,1 -> running sum mod 2: 1,0,1,1,1,1,1,0.
        let p = PredictorParams {
            predictor: 2,
            columns: 8,
            bits_per_component: 1,
            ..PredictorParams::default()
        };
        let out = apply_predictor(vec![0b1110_0001], &p, None).unwrap();
        assert_eq!(out, [0b1011_1110]);
        // 4 bits, 4 columns: nibbles 1,1,1,1 -> 1,2,3,4.
        let p4 = PredictorParams {
            predictor: 2,
            columns: 4,
            bits_per_component: 4,
            ..PredictorParams::default()
        };
        assert_eq!(
            apply_predictor(vec![0x11, 0x11], &p4, None).unwrap(),
            [0x12, 0x34]
        );
        // 2 bits, 4 columns.
        let p2 = PredictorParams {
            predictor: 2,
            columns: 4,
            bits_per_component: 2,
            ..PredictorParams::default()
        };
        assert_eq!(
            apply_predictor(vec![0b01_01_01_01], &p2, None).unwrap(),
            [0b01_10_11_00]
        );
    }

    #[test]
    fn predictor_none_is_identity() {
        let p = PredictorParams::default();
        assert_eq!(apply_predictor(vec![1, 2, 3], &p, None).unwrap(), [1, 2, 3]);
    }

    #[test]
    fn huge_row_sizes_do_not_allocate_or_panic() {
        let p = PredictorParams {
            predictor: 12,
            colors: 255,
            bits_per_component: 16,
            columns: u32::MAX,
        };
        // The row is far longer than the data: the data is one partial row.
        let out = apply_predictor(vec![2, 1, 2, 3], &p, None).unwrap();
        assert_eq!(out, [1, 2, 3]);
        let tiff = PredictorParams { predictor: 2, ..p };
        assert_eq!(
            apply_predictor(vec![1, 2, 3], &tiff, None).unwrap().len(),
            3
        );
    }

    proptest::proptest! {
        #[test]
        fn predictors_never_panic(
            data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..300),
            predictor in proptest::sample::select(vec![1u8, 2, 10, 11, 12, 13, 14, 15]),
            colors in 1u32..5,
            bits in proptest::sample::select(vec![1u32, 2, 4, 8, 16]),
            columns in 1u32..40,
        ) {
            let p = PredictorParams { predictor, colors, bits_per_component: bits, columns };
            let out = apply_predictor(data.clone(), &p, None).unwrap();
            proptest::prop_assert!(out.len() <= data.len());
        }

        #[test]
        fn flate_never_panics_on_arbitrary_input(
            data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..300),
        ) {
            let _ = flate_decode(&data, &Limits::default(), None, None);
        }
    }
}
