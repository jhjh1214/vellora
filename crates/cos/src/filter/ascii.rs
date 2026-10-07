//! `ASCIIHexDecode` (§7.4.2) and `ASCII85Decode` (§7.4.3).

use super::{Decoded, Guard, finish, is_pdf_space};
use crate::error::Result;
use crate::limits::{DecodeBudget, Limits};

const HEX: &str = "ASCIIHexDecode";
const A85: &str = "ASCII85Decode";

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decodes `ASCIIHexDecode` data: hex digit pairs, white space ignored, `>` ends the data and
/// a last odd digit counts as if followed by `0`.
///
/// # Errors
/// [`Error::LimitExceeded`](crate::Error::LimitExceeded) from the decode limits;
/// [`Error::Decode`](crate::Error::Decode) if the first byte that is not a digit, white space
/// or `>` comes before any output. Later bad bytes end the data with `complete == false`.
pub fn ascii_hex_decode(
    input: &[u8],
    limits: &Limits,
    budget: Option<&mut DecodeBudget>,
    offset: Option<u64>,
) -> Result<Decoded> {
    let mut guard = Guard::new(limits, budget, offset);
    let mut out = Vec::with_capacity(input.len() / 2);
    let mut high: Option<u8> = None;
    let (mut complete, mut failure) = (false, None);

    for (i, &byte) in input.iter().enumerate() {
        if is_pdf_space(byte) {
            continue;
        }
        if byte == b'>' {
            complete = true;
            break;
        }
        let Some(value) = hex_value(byte) else {
            failure = Some("invalid character in hexadecimal data");
            break;
        };
        match high.take() {
            Some(h) => out.push(h << 4 | value),
            None => high = Some(value),
        }
        guard.step(i + 1, out.len(), false)?;
    }
    if failure.is_none()
        && let Some(h) = high
    {
        out.push(h << 4);
    }
    guard.step(input.len(), out.len(), true)?;
    finish(HEX, out, complete, failure, offset)
}

/// Decodes `ASCII85Decode` data: groups of five characters `!`..=`u` make four bytes, `z`
/// stands for four zero bytes, white space is ignored and `~>` ends the data. A final group of
/// `n` characters (2..=4) gives `n - 1` bytes. A leading `<~` is skipped, as other readers do.
///
/// # Errors
/// [`Error::LimitExceeded`](crate::Error::LimitExceeded) from the decode limits;
/// [`Error::Decode`](crate::Error::Decode) if a bad group (invalid character, value above
/// 2^32 - 1, `z` inside a group, a single dangling digit) comes before any output. Later ones
/// end the data with `complete == false`.
pub fn ascii85_decode(
    input: &[u8],
    limits: &Limits,
    budget: Option<&mut DecodeBudget>,
    offset: Option<u64>,
) -> Result<Decoded> {
    let mut guard = Guard::new(limits, budget, offset);
    let mut out = Vec::with_capacity(input.len() / 5 * 4);
    let mut group = [0u8; 5];
    let mut n = 0usize;
    let (mut complete, mut failure) = (false, None);

    let start = {
        let lead = input.iter().take_while(|&&b| is_pdf_space(b)).count();
        if input.get(lead..lead + 2) == Some(b"<~".as_slice()) {
            lead + 2
        } else {
            0
        }
    };

    for (i, &byte) in input.iter().enumerate().skip(start) {
        match byte {
            _ if is_pdf_space(byte) => continue,
            b'~' => {
                complete = true;
                break;
            }
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                if let Some(slot) = group.get_mut(n) {
                    *slot = byte - b'!';
                }
                n += 1;
                if n == 5 {
                    let Some(value) = group_value(group) else {
                        failure = Some("ASCII85 group is larger than 2^32 - 1");
                        break;
                    };
                    out.extend_from_slice(&value.to_be_bytes());
                    n = 0;
                }
            }
            _ => {
                failure = Some("invalid character in ASCII85 data");
                break;
            }
        }
        guard.step(i + 1, out.len(), false)?;
    }

    if failure.is_none() && n > 0 {
        if n == 1 {
            failure = Some("dangling ASCII85 digit");
        } else {
            // §7.4.3: pad the short group with `u`, keep n - 1 bytes.
            let mut padded = group;
            for slot in padded.iter_mut().skip(n) {
                *slot = b'u' - b'!';
            }
            match group_value(padded) {
                Some(value) => {
                    let bytes = value.to_be_bytes();
                    out.extend_from_slice(bytes.get(..n - 1).unwrap_or_default());
                }
                None => failure = Some("ASCII85 group is larger than 2^32 - 1"),
            }
        }
    }
    guard.step(input.len(), out.len(), true)?;
    finish(A85, out, complete, failure, offset)
}

/// Base-85 value of five digits, if it fits in 32 bits.
fn group_value(group: [u8; 5]) -> Option<u32> {
    let value = group
        .iter()
        .fold(0u64, |acc, &digit| acc * 85 + u64::from(digit));
    u32::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use crate::limits::LimitKind;

    fn hex(input: &[u8]) -> Result<Decoded> {
        ascii_hex_decode(input, &Limits::default(), None, None)
    }

    fn a85(input: &[u8]) -> Result<Decoded> {
        ascii85_decode(input, &Limits::default(), None, None)
    }

    #[test]
    fn hex_known_answer() {
        let d = hex(b"48 65 6C6c\n6F>").unwrap();
        assert_eq!(d.data, b"Hello");
        assert!(d.complete);
    }

    #[test]
    fn hex_odd_digit_is_padded_with_zero() {
        assert_eq!(hex(b"7>").unwrap().data, [0x70]);
        assert_eq!(hex(b"414>").unwrap().data, [0x41, 0x40]);
    }

    #[test]
    fn hex_without_eod_is_incomplete_and_data_after_eod_is_ignored() {
        let d = hex(b"4142").unwrap();
        assert_eq!((d.data.as_slice(), d.complete), (&b"AB"[..], false));
        assert_eq!(hex(b"41>42").unwrap().data, b"A");
        assert_eq!(hex(b"").unwrap().data, b"");
    }

    #[test]
    fn hex_bad_character_keeps_what_came_before() {
        let d = hex(b"4142zz43").unwrap();
        assert_eq!((d.data.as_slice(), d.complete), (&b"AB"[..], false));
        assert!(matches!(hex(b"zz"), Err(Error::Decode { .. })));
    }

    #[test]
    fn a85_known_answer() {
        // "Man " encodes to `9jqo^` (the classic base-85 example).
        let d = a85(b"9jqo^~>").unwrap();
        assert_eq!(d.data, b"Man ");
        assert!(d.complete);
    }

    #[test]
    fn a85_partial_final_group() {
        // Computed with Python's `base64.a85encode`, which pads and trims the same way.
        assert_eq!(a85(b"9jn~>").unwrap().data, b"Ma");
        assert_eq!(a85(b"9`~>").unwrap().data, b"M");
        assert_eq!(a85(b"~>").unwrap().data, b"");
    }

    #[test]
    fn a85_z_white_space_and_leading_marker() {
        assert_eq!(a85(b"z~>").unwrap().data, [0, 0, 0, 0]);
        assert_eq!(a85(b"<~ 9j\nqo ^ ~>").unwrap().data, b"Man ");
        assert_eq!(a85(b"9jqo^zz~>").unwrap().data.len(), 12);
    }

    #[test]
    fn a85_errors_and_partial_output() {
        // `z` inside a group, an out-of-alphabet byte and a value over 2^32 - 1.
        assert!(matches!(a85(b"9jz~>"), Err(Error::Decode { .. })));
        assert!(matches!(a85(b"9j\x7f~>"), Err(Error::Decode { .. })));
        assert!(matches!(a85(b"uuuuu~>"), Err(Error::Decode { .. })));
        // A dangling digit after good output keeps the output.
        let d = a85(b"9jqo^9~>").unwrap();
        assert_eq!((d.data.as_slice(), d.complete), (&b"Man "[..], false));
        // No EOD marker: data is kept, flagged incomplete.
        let d = a85(b"9jqo^").unwrap();
        assert_eq!((d.data.as_slice(), d.complete), (&b"Man "[..], false));
    }

    #[test]
    fn a85_z_bomb_hits_the_stream_size_limit() {
        // `z` is four bytes out per byte in, so the size limit is what stops it.
        let limits = Limits {
            max_decoded_stream_bytes: 1000,
            ..Limits::default()
        };
        let input = vec![b'z'; 100_000];
        assert!(matches!(
            ascii85_decode(&input, &limits, None, None),
            Err(Error::LimitExceeded {
                limit: LimitKind::DecodedStreamBytes,
                ..
            })
        ));
    }

    proptest::proptest! {
        #[test]
        fn never_panic_on_arbitrary_input(
            data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..300),
        ) {
            let _ = hex(&data);
            let _ = a85(&data);
        }
    }
}
