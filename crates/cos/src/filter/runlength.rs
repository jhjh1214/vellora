//! `RunLengthDecode` (§7.4.5).

use super::{Decoded, Guard, finish};
use crate::error::Result;
use crate::limits::{DecodeBudget, Limits};

const NAME: &str = "RunLengthDecode";

/// Decodes `RunLengthDecode` data: a length byte `n` of 0..=127 copies the next `n + 1` bytes,
/// 129..=255 repeats the next byte `257 - n` times and 128 ends the data.
///
/// # Errors
/// [`Error::LimitExceeded`](crate::Error::LimitExceeded) from the decode limits (a run of 128
/// bytes costs two input bytes, so the ratio limit is what stops a bomb);
/// [`Error::Decode`](crate::Error::Decode) if the data is cut inside its first run. A later cut
/// keeps the output with `complete == false`, like a missing end marker.
pub fn run_length_decode(
    input: &[u8],
    limits: &Limits,
    budget: Option<&mut DecodeBudget>,
    offset: Option<u64>,
) -> Result<Decoded> {
    let mut guard = Guard::new(limits, budget, offset);
    let mut out = Vec::new();
    let mut pos = 0usize;
    let (mut complete, mut failure) = (false, None);

    while let Some(&length) = input.get(pos) {
        pos += 1;
        match length {
            128 => {
                complete = true;
                break;
            }
            0..=127 => {
                let count = usize::from(length) + 1;
                let available = input.get(pos..).unwrap_or_default();
                let take = count.min(available.len());
                out.extend_from_slice(available.get(..take).unwrap_or_default());
                pos += take;
                if take < count {
                    failure = Some("run-length data ends inside a literal run");
                    break;
                }
            }
            129..=255 => {
                let Some(&byte) = input.get(pos) else {
                    failure = Some("run-length data ends inside a repeat run");
                    break;
                };
                pos += 1;
                out.resize(out.len() + 257 - usize::from(length), byte);
            }
        }
        guard.step(pos, out.len(), false)?;
    }
    guard.step(input.len(), out.len(), true)?;
    finish(NAME, out, complete, failure, offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use crate::limits::LimitKind;

    fn decode(input: &[u8]) -> Result<Decoded> {
        run_length_decode(input, &Limits::default(), None, None)
    }

    #[test]
    fn known_answer_literal_and_repeat_runs() {
        // 2 -> copy 3 bytes; 254 -> repeat 3 times (257 - 254); 128 -> EOD.
        let d = decode(&[2, b'a', b'b', b'c', 254, b'x', 128, b'!']).unwrap();
        assert_eq!(d.data, b"abcxxx");
        assert!(d.complete);
    }

    #[test]
    fn run_length_extremes() {
        assert_eq!(decode(&[0, 7, 128]).unwrap().data, [7]);
        assert_eq!(decode(&[129, 9, 128]).unwrap().data, vec![9; 128]);
        assert_eq!(decode(&[255, 1, 128]).unwrap().data, [1, 1]);
        let mut literal = vec![127];
        literal.extend(0..128u8);
        literal.push(128);
        assert_eq!(
            decode(&literal).unwrap().data,
            (0..128u8).collect::<Vec<_>>()
        );
    }

    #[test]
    fn missing_eod_and_truncated_runs() {
        let d = decode(&[0, 1]).unwrap();
        assert_eq!((d.data.as_slice(), d.complete), (&[1u8][..], false));
        let d = decode(&[0, 1, 3, 5, 6]).unwrap();
        assert_eq!((d.data.as_slice(), d.complete), (&[1u8, 5, 6][..], false));
        let d = decode(&[0, 1, 200]).unwrap();
        assert_eq!((d.data.as_slice(), d.complete), (&[1u8][..], false));
        assert!(matches!(decode(&[200]), Err(Error::Decode { .. })));
        assert_eq!(decode(&[]).unwrap().data, b"");
    }

    #[test]
    fn repeat_bomb_hits_the_ratio_limit() {
        // Two bytes make 128 (a ratio of 64); the ratio cap only applies above a 1 MiB floor.
        let limits = Limits {
            max_decompression_ratio: 8,
            ..Limits::default()
        };
        let input: Vec<u8> = [129u8, 0].repeat(40_000);
        match run_length_decode(&input, &limits, None, None) {
            Err(Error::LimitExceeded { limit, .. }) => {
                assert_eq!(limit, LimitKind::DecompressionRatio);
            }
            other => panic!("expected a ratio error, got {other:?}"),
        }
    }

    #[test]
    fn repeat_bomb_hits_the_stream_size_limit() {
        let limits = Limits {
            max_decoded_stream_bytes: 10_000,
            ..Limits::default()
        };
        let input: Vec<u8> = [129u8, 0].repeat(1000);
        assert!(matches!(
            run_length_decode(&input, &limits, None, None),
            Err(Error::LimitExceeded {
                limit: LimitKind::DecodedStreamBytes,
                ..
            })
        ));
    }

    proptest::proptest! {
        #[test]
        fn never_panics_on_arbitrary_input(
            data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..300),
        ) {
            let _ = decode(&data);
        }
    }
}
