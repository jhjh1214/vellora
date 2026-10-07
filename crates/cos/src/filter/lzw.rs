//! `LZWDecode` (§7.4.4.2).

use std::cmp::Ordering;

use super::{Decoded, Guard, finish};
use crate::error::Result;
use crate::limits::{DecodeBudget, Limits};

const NAME: &str = "LZWDecode";
const CLEAR: u16 = 256;
const EOD: u16 = 257;
const FIRST_FREE: u16 = 258;
const TABLE_SIZE: usize = 4096;

/// The string table: entry `c` is the string of entry `prefix[c]` followed by `suffix[c]`.
struct Table {
    prefix: [u16; TABLE_SIZE],
    suffix: [u8; TABLE_SIZE],
    /// First byte of each entry's string (needed for the next entry).
    first: [u8; TABLE_SIZE],
    length: [u16; TABLE_SIZE],
    next: u16,
}

impl Table {
    fn new() -> Self {
        let mut table = Self {
            prefix: [0; TABLE_SIZE],
            suffix: [0; TABLE_SIZE],
            first: [0; TABLE_SIZE],
            length: [1; TABLE_SIZE],
            next: FIRST_FREE,
        };
        for byte in 0..=255u8 {
            table.suffix[usize::from(byte)] = byte;
            table.first[usize::from(byte)] = byte;
        }
        table
    }

    fn slot(code: u16) -> usize {
        usize::from(code) & (TABLE_SIZE - 1)
    }

    /// Appends the string of `code` to `out`.
    fn emit(&self, code: u16, out: &mut Vec<u8>) {
        let len = usize::from(self.length[Self::slot(code)]);
        let start = out.len();
        out.resize(start + len, 0);
        let mut current = code;
        for k in (0..len).rev() {
            let slot = Self::slot(current);
            if let Some(byte) = out.get_mut(start + k) {
                *byte = self.suffix[slot];
            }
            current = self.prefix[slot];
        }
    }

    /// Adds `string(prev) + first byte of the next string` if there is room.
    fn add(&mut self, prev: u16, first_of_next: u8) {
        if usize::from(self.next) >= TABLE_SIZE {
            return;
        }
        let slot = Self::slot(self.next);
        let prev_slot = Self::slot(prev);
        self.prefix[slot] = prev;
        self.suffix[slot] = first_of_next;
        self.first[slot] = self.first[prev_slot];
        self.length[slot] = self.length[prev_slot].saturating_add(1);
        self.next += 1;
    }

    /// Code width in bits for the current table size. With `/EarlyChange 1` (the default) the
    /// width grows one code early, as in TIFF.
    fn width(&self, early_change: bool) -> u32 {
        let size = usize::from(self.next) + usize::from(early_change);
        match size {
            0..512 => 9,
            512..1024 => 10,
            1024..2048 => 11,
            _ => 12,
        }
    }
}

/// Decodes `LZWDecode` data: variable-width codes of 9 to 12 bits, most significant bit first,
/// with 256 = clear-table and 257 = end of data. `early_change` is the `/EarlyChange` entry
/// of the decode parameters (default `true`). Predictors are applied by the caller.
///
/// # Errors
/// [`Error::LimitExceeded`](crate::Error::LimitExceeded) from the decode limits (every code
/// can expand to a 4095-byte string, so the ratio limit matters here);
/// [`Error::Decode`](crate::Error::Decode) if the data is invalid before any output. A later
/// invalid code ends the data with `complete == false`, like a missing end-of-data code.
pub fn lzw_decode(
    input: &[u8],
    early_change: bool,
    limits: &Limits,
    budget: Option<&mut DecodeBudget>,
    offset: Option<u64>,
) -> Result<Decoded> {
    let mut guard = Guard::new(limits, budget, offset);
    let mut out = Vec::new();
    let mut table = Table::new();
    let mut prev: Option<u16> = None;
    let mut width = 9u32;
    let (mut acc, mut bits, mut pos) = (0u32, 0u32, 0usize);
    let (mut complete, mut failure) = (false, None);

    'codes: loop {
        while bits < width {
            let Some(&byte) = input.get(pos) else {
                break 'codes;
            };
            pos += 1;
            acc = acc << 8 | u32::from(byte);
            bits += 8;
        }
        bits -= width;
        let code = u16::try_from(acc >> bits & ((1 << width) - 1)).unwrap_or(0);
        acc &= (1 << bits) - 1;

        match code {
            CLEAR => {
                table = Table::new();
                prev = None;
                width = 9;
                continue;
            }
            EOD => {
                complete = true;
                break;
            }
            _ => {}
        }

        match prev {
            None => {
                // After a clear, the first code must be a literal (nothing is in the table).
                if code >= CLEAR {
                    failure = Some("LZW code is not a literal after a table clear");
                    break;
                }
                table.emit(code, &mut out);
            }
            Some(last) => {
                match code.cmp(&table.next) {
                    Ordering::Less => {
                        table.emit(code, &mut out);
                        table.add(last, table.first[Table::slot(code)]);
                    }
                    Ordering::Equal => {
                        // The string being defined: previous string plus its own first byte.
                        let first = table.first[Table::slot(last)];
                        table.add(last, first);
                        table.emit(code, &mut out);
                    }
                    Ordering::Greater => {
                        failure = Some("LZW code is not in the table");
                        break;
                    }
                }
                width = table.width(early_change);
            }
        }
        prev = Some(code);
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

    fn decode(input: &[u8], early: bool) -> Result<Decoded> {
        lzw_decode(input, early, &Limits::default(), None, None)
    }

    /// Test-only encoder: the textbook algorithm, with the code width switched the way a
    /// decoder (one table entry behind) will expect it.
    fn encode(data: &[u8], early: bool) -> Vec<u8> {
        use std::collections::HashMap;

        struct Bits {
            out: Vec<u8>,
            acc: u32,
            n: u32,
        }
        impl Bits {
            fn put(&mut self, code: u16, width: u32) {
                self.acc = self.acc << width | u32::from(code);
                self.n += width;
                while self.n >= 8 {
                    self.n -= 8;
                    self.out.push(u8::try_from(self.acc >> self.n).unwrap());
                    self.acc &= (1 << self.n) - 1;
                }
            }
        }
        // The encoder's table is one entry ahead of the decoder's.
        let width_for = |next: usize| match next - 1 + usize::from(early) {
            0..512 => 9,
            512..1024 => 10,
            1024..2048 => 11,
            _ => 12,
        };

        let mut bits = Bits {
            out: Vec::new(),
            acc: 0,
            n: 0,
        };
        let mut dict: HashMap<(u16, u8), u16> = HashMap::new();
        let mut next = 258usize;
        bits.put(CLEAR, 9);
        let mut current: Option<u16> = None;
        for &byte in data {
            let Some(cur) = current else {
                current = Some(u16::from(byte));
                continue;
            };
            if let Some(&code) = dict.get(&(cur, byte)) {
                current = Some(code);
                continue;
            }
            bits.put(cur, width_for(next));
            if next < 4094 {
                dict.insert((cur, byte), u16::try_from(next).unwrap());
                next += 1;
            } else {
                bits.put(CLEAR, width_for(next));
                dict.clear();
                next = 258;
            }
            current = Some(u16::from(byte));
        }
        if let Some(cur) = current {
            bits.put(cur, width_for(next));
            next += 1;
        }
        bits.put(EOD, width_for(next));
        if bits.n > 0 {
            bits.put(0, 8 - bits.n);
        }
        bits.out
    }

    #[test]
    fn spec_example() {
        // ISO 32000-2 §7.4.4.2: 45 45 45 45 45 65 45 45 45 66 encodes to these bytes.
        let encoded = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        let d = decode(&encoded, true).unwrap();
        assert_eq!(d.data, [45, 45, 45, 45, 45, 65, 45, 45, 45, 66]);
        assert!(d.complete);
    }

    #[test]
    fn test_encoder_agrees_with_the_spec_example() {
        let data = [45, 45, 45, 45, 45, 65, 45, 45, 45, 66];
        assert_eq!(
            encode(&data, true),
            [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01]
        );
    }

    #[test]
    fn round_trips_across_every_width_change_and_table_reset() {
        // Pseudo-random bytes grow the table fast; repeated text exercises the KwKwK case.
        let mut state = 0x1234_5678u32;
        let mut noise = Vec::new();
        for _ in 0..40_000 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            noise.push((state >> 24) as u8 & 0x3f);
        }
        let text = b"abababababababab the quick brown fox ".repeat(500);
        for early in [true, false] {
            for data in [
                &noise[..],
                &text[..],
                &vec![7u8; 100_000][..],
                &[][..],
                &[9][..],
            ] {
                let decoded = decode(&encode(data, early), early).unwrap();
                assert_eq!(decoded.data, data, "early_change = {early}");
                assert!(decoded.complete);
            }
        }
    }

    #[test]
    fn wrong_early_change_does_not_decode_large_data_correctly() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let encoded = encode(&data, true);
        let wrong = decode(&encoded, false);
        assert!(wrong.map_or(true, |d| d.data != data));
    }

    #[test]
    fn missing_eod_and_garbage() {
        // The spec example without its last two bytes: partial output, incomplete.
        let d = decode(&[0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C], true).unwrap();
        assert!(!d.complete);
        // Six of the seven codes are there: 45, 45 45, 45 45, 65, 45 45 45.
        assert_eq!(d.data.len(), 9);
        // The first code (511) is neither a literal nor in the table.
        assert!(decode(&[0xFF, 0xFF, 0xFF], true).is_err());
        assert_eq!(decode(&[], true).unwrap().data, b"");
    }

    #[test]
    fn lzw_bomb_hits_a_limit() {
        let bomb = encode(&vec![0u8; 5_000_000], true);
        assert!(bomb.len() < 20_000);
        match lzw_decode(&bomb, true, &Limits::default(), None, None) {
            Err(Error::LimitExceeded { limit, .. }) => {
                assert_eq!(limit, LimitKind::DecompressionRatio);
            }
            other => panic!("expected a ratio error, got {other:?}"),
        }
        let tight = Limits {
            max_decoded_stream_bytes: 100_000,
            ..Limits::default()
        };
        assert!(matches!(
            lzw_decode(&bomb, true, &tight, None, None),
            Err(Error::LimitExceeded {
                limit: LimitKind::DecodedStreamBytes,
                ..
            })
        ));
    }

    proptest::proptest! {
        #[test]
        fn never_panics_on_arbitrary_input(
            data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..400),
            early in proptest::prelude::any::<bool>(),
        ) {
            let _ = decode(&data, early);
        }

        #[test]
        fn round_trips_arbitrary_data(
            data in proptest::collection::vec(0u8..8, 0..3000),
        ) {
            let decoded = decode(&encode(&data, true), true).unwrap();
            proptest::prop_assert_eq!(decoded.data, data);
        }
    }
}
