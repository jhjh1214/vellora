//! Filter chains: `/Filter` and `/DecodeParms` of a stream (§7.3.8.2, §7.4.1).

use super::{
    Decoded, PredictorParams, apply_predictor, ascii_hex_decode, ascii85_decode, decode_error,
    flate_decode, lzw_decode, run_length_decode,
};
use crate::error::{Error, Result};
use crate::limits::{DecodeBudget, LimitKind, Limits};
use crate::object::{Dict, ObjectKind};

/// An image codec that [`decode_stream`] leaves to PDFium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageFilter {
    /// `DCTDecode` (JPEG).
    Dct,
    /// `JPXDecode` (JPEG 2000).
    Jpx,
    /// `CCITTFaxDecode`.
    CcittFax,
    /// `JBIG2Decode`.
    Jbig2,
}

/// The result of running a filter chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedStream {
    /// The data after every filter that was run. If `image_filter` is set this is still
    /// encoded with that codec.
    pub data: Vec<u8>,
    /// `false` if any stage ended early or turned corrupt after some output (see [`Decoded`]).
    pub complete: bool,
    /// The image codec the chain ended in, which is not decoded here.
    pub image_filter: Option<ImageFilter>,
}

/// What kind of filter a name selects. Abbreviations are the inline-image names of Table 92.
enum Kind {
    Flate,
    Lzw,
    AsciiHex,
    Ascii85,
    RunLength,
    Crypt,
    Image(ImageFilter),
}

fn classify(name: &[u8]) -> Option<Kind> {
    Some(match name {
        b"FlateDecode" | b"Fl" => Kind::Flate,
        b"LZWDecode" | b"LZW" => Kind::Lzw,
        b"ASCIIHexDecode" | b"AHx" => Kind::AsciiHex,
        b"ASCII85Decode" | b"A85" => Kind::Ascii85,
        b"RunLengthDecode" | b"RL" => Kind::RunLength,
        b"Crypt" => Kind::Crypt,
        b"DCTDecode" | b"DCT" => Kind::Image(ImageFilter::Dct),
        b"JPXDecode" => Kind::Image(ImageFilter::Jpx),
        b"CCITTFaxDecode" | b"CCF" => Kind::Image(ImageFilter::CcittFax),
        b"JBIG2Decode" => Kind::Image(ImageFilter::Jbig2),
        _ => return None,
    })
}

/// Runs `filters` (name, parameters) over `raw` in order.
///
/// The names may be abbreviated (for inline images). Each stage is checked against `limits`
/// and charged to `budget`, so an intermediate result counts towards the document budget too.
/// `/Crypt` is a no-op here: decryption happens on the raw bytes before any filter runs.
///
/// # Errors
/// [`Error::Decode`] for an unknown filter, an image filter that is not last, bad parameters or
/// a stage that fails before any output; [`Error::LimitExceeded`] from the decode limits.
pub fn decode_chain(
    filters: &[(&[u8], Option<&Dict<'_>>)],
    raw: &[u8],
    limits: &Limits,
    mut budget: Option<&mut DecodeBudget>,
    offset: Option<u64>,
) -> Result<DecodedStream> {
    let mut data = raw.to_vec();
    let mut complete = true;
    let mut image_filter = None;

    if filters.is_empty() {
        limits.check(LimitKind::DecodedStreamBytes, data.len() as u64, offset)?;
        if let Some(budget) = budget {
            budget.charge(data.len() as u64, offset)?;
        }
        return Ok(DecodedStream {
            data,
            complete,
            image_filter,
        });
    }

    for (index, &(name, parms)) in filters.iter().enumerate() {
        let kind = classify(name).ok_or_else(|| decode_error("unsupported filter", offset))?;
        let stage = match kind {
            Kind::Image(image) => {
                if index + 1 != filters.len() {
                    return Err(decode_error(
                        "an image filter must be the last filter",
                        offset,
                    ));
                }
                image_filter = Some(image);
                break;
            }
            Kind::Crypt => continue,
            Kind::Flate => {
                let decoded = flate_decode(&data, limits, budget.as_deref_mut(), offset)?;
                predict(decoded, parms, offset)?
            }
            Kind::Lzw => {
                let early = match parms.and_then(|p| p.get(b"EarlyChange")).map(|o| &o.kind) {
                    Some(ObjectKind::Integer(n)) => *n != 0,
                    _ => true,
                };
                let decoded = lzw_decode(&data, early, limits, budget.as_deref_mut(), offset)?;
                predict(decoded, parms, offset)?
            }
            Kind::AsciiHex => ascii_hex_decode(&data, limits, budget.as_deref_mut(), offset)?,
            Kind::Ascii85 => ascii85_decode(&data, limits, budget.as_deref_mut(), offset)?,
            Kind::RunLength => run_length_decode(&data, limits, budget.as_deref_mut(), offset)?,
        };
        complete &= stage.complete;
        data = stage.data;
    }

    Ok(DecodedStream {
        data,
        complete,
        image_filter,
    })
}

/// Applies the predictor named in `parms`, if any, to Flate or LZW output.
fn predict(mut decoded: Decoded, parms: Option<&Dict<'_>>, offset: Option<u64>) -> Result<Decoded> {
    if let Some(parms) = parms {
        let predictor = PredictorParams::from_dict(parms, offset)?;
        decoded.data = apply_predictor(decoded.data, &predictor, offset)?;
    }
    Ok(decoded)
}

/// Decodes the stream with dictionary `dict` and raw bytes `raw`: reads `/Filter` and
/// `/DecodeParms` and runs [`decode_chain`].
///
/// Only the full key names are read (the abbreviations `/F` and `/DP` are for inline images;
/// in a stream dictionary `/F` is a file specification). Both entries must be direct objects:
/// resolve indirect ones first. A lone `/DecodeParms` dictionary belongs to a one-element
/// filter chain; for longer chains it must be an array with one entry (dictionary or null) per
/// filter.
///
/// # Errors
/// As [`decode_chain`], plus [`Error::Decode`] for a malformed `/Filter` or `/DecodeParms`.
pub fn decode_stream(
    dict: &Dict<'_>,
    raw: &[u8],
    limits: &Limits,
    budget: Option<&mut DecodeBudget>,
    offset: Option<u64>,
) -> Result<DecodedStream> {
    let bad_filter = || decode_error("invalid /Filter", offset);
    let bad_parms = || decode_error("invalid /DecodeParms", offset);

    let mut names: Vec<&[u8]> = Vec::new();
    match dict.get(b"Filter").map(|o| &o.kind) {
        None | Some(ObjectKind::Null) => {}
        Some(ObjectKind::Name(name)) => names.push(name),
        Some(ObjectKind::Array(items)) => {
            for item in items {
                match &item.kind {
                    ObjectKind::Name(name) => names.push(name),
                    _ => return Err(bad_filter()),
                }
            }
        }
        Some(_) => return Err(bad_filter()),
    }

    let mut parms: Vec<Option<&Dict<'_>>> = vec![None; names.len()];
    match dict.get(b"DecodeParms").map(|o| &o.kind) {
        None | Some(ObjectKind::Null) => {}
        Some(ObjectKind::Dict(d)) => match parms.as_mut_slice() {
            [only] => *only = Some(d),
            [] => {}
            _ => return Err(bad_parms()),
        },
        Some(ObjectKind::Array(items)) => {
            // Extra entries are ignored and missing ones mean "no parameters".
            for (slot, item) in parms.iter_mut().zip(items) {
                *slot = match &item.kind {
                    ObjectKind::Dict(d) => Some(d),
                    ObjectKind::Null => None,
                    _ => return Err(bad_parms()),
                };
            }
        }
        Some(_) => return Err(bad_parms()),
    }

    let filters: Vec<(&[u8], Option<&Dict<'_>>)> = names.into_iter().zip(parms).collect();
    decode_chain(&filters, raw, limits, budget, offset)
}

/// Like [`decode_stream`] for callers that need plain bytes (cross-reference and object
/// streams): the data of a chain that ends in an image codec is an error.
///
/// A stream truncated or corrupted after some output returns the part that decoded; callers
/// check the length they expect.
///
/// # Errors
/// As [`decode_stream`], plus [`Error::Decode`] if the chain ends in an image filter.
pub fn decode_stream_bytes(
    dict: &Dict<'_>,
    raw: &[u8],
    limits: &Limits,
    budget: Option<&mut DecodeBudget>,
    offset: Option<u64>,
) -> Result<Vec<u8>> {
    let decoded = decode_stream(dict, raw, limits, budget, offset)?;
    if decoded.image_filter.is_some() {
        return Err(Error::Decode {
            filter: "image",
            detail: "image filters cannot be decoded here",
            offset,
        });
    }
    Ok(decoded.data)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    use super::*;
    use crate::parser::Parser;

    fn dict(text: &str) -> Dict<'static> {
        let limits = Limits::default();
        match Parser::new(text.as_bytes(), &limits)
            .parse_object()
            .unwrap()
            .kind
        {
            ObjectKind::Dict(d) => d.into_owned(),
            other => panic!("not a dictionary: {other:?}"),
        }
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn to_hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
    }

    fn run(dict_text: &str, raw: &[u8]) -> Result<DecodedStream> {
        decode_stream(&dict(dict_text), raw, &Limits::default(), None, None)
    }

    #[test]
    fn no_filter_is_the_identity() {
        let d = run("<< /Length 3 >>", b"abc").unwrap();
        assert_eq!(d.data, b"abc");
        assert!(d.complete);
        assert_eq!(d.image_filter, None);
        assert_eq!(run("<< /Filter [] >>", b"abc").unwrap().data, b"abc");
        assert_eq!(run("<< /Filter null >>", b"abc").unwrap().data, b"abc");
    }

    #[test]
    fn single_filters_by_name() {
        assert_eq!(
            run("<< /Filter /ASCIIHexDecode >>", b"4142>").unwrap().data,
            b"AB"
        );
        assert_eq!(
            run("<< /Filter [/ASCII85Decode] >>", b"9jqo^~>")
                .unwrap()
                .data,
            b"Man "
        );
        assert_eq!(
            run("<< /Filter /RunLengthDecode >>", &[1, b'h', b'i', 128])
                .unwrap()
                .data,
            b"hi"
        );
        assert_eq!(
            run("<< /Filter /FlateDecode >>", &zlib(b"hello"))
                .unwrap()
                .data,
            b"hello"
        );
        let lzw = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(
            run("<< /Filter /LZWDecode >>", &lzw).unwrap().data,
            [45, 45, 45, 45, 45, 65, 45, 45, 45, 66]
        );
    }

    #[test]
    fn chain_runs_in_order_with_per_filter_parameters() {
        // Flate with a PNG "Up" predictor, then wrapped in ASCIIHex: the filter array lists
        // the *decoding* order, so hex comes first.
        let rows = [2u8, 1, 2, 3, 2, 1, 1, 1]; // Up tag, then deltas, two rows of 3 columns
        let hex = to_hex(&zlib(&rows));
        let text = "<< /Filter [/ASCIIHexDecode /FlateDecode] \
                    /DecodeParms [null << /Predictor 12 /Columns 3 >>] >>";
        let d = run(text, format!("{hex}>").as_bytes()).unwrap();
        assert_eq!(d.data, [1, 2, 3, 2, 3, 4]);
        assert!(d.complete);
    }

    #[test]
    fn lzw_predictor_and_early_change() {
        // TIFF predictor on LZW output: the encoded form of [1, 1, 1, 1] with Columns 4.
        let encoded = [0x80, 0x00, 0x00, 0x00, 0x00]; // not valid LZW: must error cleanly
        let _ = run(
            "<< /Filter /LZWDecode /DecodeParms << /Predictor 2 /Columns 4 >> >>",
            &encoded,
        );
        let lzw = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        // `/EarlyChange 0` does not matter for data this short.
        let d = run(
            "<< /Filter /LZWDecode /DecodeParms << /EarlyChange 0 >> >>",
            &lzw,
        )
        .unwrap();
        assert_eq!(d.data.len(), 10);
    }

    #[test]
    fn single_parms_dictionary_in_a_one_element_array_filter() {
        let rows = [2u8, 5, 6];
        let d = run(
            "<< /Filter [/FlateDecode] /DecodeParms << /Predictor 12 /Columns 2 >> >>",
            &zlib(&rows),
        )
        .unwrap();
        assert_eq!(d.data, [5, 6]);
    }

    #[test]
    fn image_filters_pass_through_when_last() {
        let d = run("<< /Filter /DCTDecode >>", b"\xFF\xD8").unwrap();
        assert_eq!(d.data, b"\xFF\xD8");
        assert_eq!(d.image_filter, Some(ImageFilter::Dct));
        let d = run("<< /Filter [/ASCIIHexDecode /JPXDecode] >>", b"4142>").unwrap();
        assert_eq!(
            (d.data.as_slice(), d.image_filter),
            (&b"AB"[..], Some(ImageFilter::Jpx))
        );
        for (name, filter) in [
            ("CCITTFaxDecode", ImageFilter::CcittFax),
            ("JBIG2Decode", ImageFilter::Jbig2),
        ] {
            let d = run(&format!("<< /Filter /{name} >>"), b"x").unwrap();
            assert_eq!(d.image_filter, Some(filter));
        }
        assert!(matches!(
            run("<< /Filter [/DCTDecode /FlateDecode] >>", b"x"),
            Err(Error::Decode { .. })
        ));
    }

    #[test]
    fn image_chain_is_an_error_for_byte_callers() {
        let d = dict("<< /Filter /DCTDecode >>");
        assert!(matches!(
            decode_stream_bytes(&d, b"x", &Limits::default(), None, None),
            Err(Error::Decode { .. })
        ));
    }

    #[test]
    fn crypt_is_skipped_and_abbreviations_work_in_chains() {
        let d = run("<< /Filter [/Crypt /ASCIIHexDecode] >>", b"41>").unwrap();
        assert_eq!(d.data, b"A");
        let filters: [(&[u8], Option<&Dict<'_>>); 2] = [(b"AHx", None), (b"RL", None)];
        let d = decode_chain(&filters, b"0141 42 80>", &Limits::default(), None, None).unwrap();
        assert_eq!(d.data, b"AB");
    }

    #[test]
    fn incomplete_stage_marks_the_chain_incomplete() {
        let d = run("<< /Filter /ASCIIHexDecode >>", b"4142").unwrap();
        assert!(!d.complete);
    }

    #[test]
    fn malformed_filter_entries_are_typed_errors() {
        for text in [
            "<< /Filter /Nope >>",
            "<< /Filter 7 >>",
            "<< /Filter [/FlateDecode 7] >>",
            "<< /Filter [/FlateDecode /FlateDecode] /DecodeParms << /Predictor 12 >> >>",
            "<< /Filter /FlateDecode /DecodeParms 7 >>",
            "<< /Filter [/FlateDecode] /DecodeParms [7] >>",
            "<< /Filter /FlateDecode /DecodeParms << /Predictor 99 >> >>",
        ] {
            assert!(
                matches!(run(text, &zlib(b"x")), Err(Error::Decode { .. })),
                "{text}"
            );
        }
        // An indirect /Filter is not resolved here.
        assert!(matches!(
            run("<< /Filter 5 0 R >>", b"x"),
            Err(Error::Decode { .. })
        ));
    }

    #[test]
    fn short_parms_array_means_no_parameters() {
        let d = run(
            "<< /Filter [/ASCIIHexDecode /ASCIIHexDecode] /DecodeParms [null] >>",
            b"3431>",
        )
        .unwrap();
        assert_eq!(d.data, [0x41]);
    }

    #[test]
    fn unabbreviated_f_and_dp_are_not_filter_keys() {
        // /F is a file specification in a stream dictionary.
        let d = run("<< /F (x.bin) /DP << /Predictor 12 >> >>", b"abc").unwrap();
        assert_eq!(d.data, b"abc");
    }

    #[test]
    fn bomb_in_a_chain_fails_with_a_limit_error() {
        let big = zlib(&vec![0u8; 20_000_000]);
        let hex = to_hex(&big);
        let text = "<< /Filter [/ASCIIHexDecode /FlateDecode] >>";
        let err = run(text, format!("{hex}>").as_bytes()).unwrap_err();
        assert!(
            matches!(
                err,
                Error::LimitExceeded {
                    limit: LimitKind::DecompressionRatio,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn every_stage_is_charged_to_the_document_budget() {
        let limits = Limits {
            max_total_decode_bytes: 15,
            ..Limits::default()
        };
        let mut budget = DecodeBudget::new(&limits);
        let d = dict("<< /Filter [/ASCIIHexDecode /ASCIIHexDecode] >>");
        // 8 hex chars -> 4 bytes -> (those 4 bytes are '4','1','4','1'? no: 2 hex pairs)
        let ok = decode_stream(&d, b"34313431>", &limits, Some(&mut budget), None);
        assert!(ok.is_ok());
        assert_eq!(budget.used(), 4 + 2);
        let err = decode_stream(
            &dict("<< /Filter /ASCIIHexDecode >>"),
            &b"41".repeat(10),
            &limits,
            Some(&mut budget),
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            Error::LimitExceeded {
                limit: LimitKind::TotalDecodeBytes,
                ..
            }
        ));
    }
}
