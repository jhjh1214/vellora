//! PDF text strings (ISO 32000-2:2020 §7.9.2.2): the strings that hold text for people, such as
//! outline titles and page-label prefixes.
//!
//! A text string is `UTF-16BE` when it starts with the byte order mark `FE FF`, `UTF-8` when it
//! starts with `EF BB BF` (PDF 2.0), and `PDFDocEncoding` otherwise. Files in the wild also carry
//! little-endian UTF-16 with `FF FE`, which is accepted. Decoding never fails: what cannot be
//! decoded becomes U+FFFD, and control characters are replaced by a space, because the result is
//! shown in a user interface.

/// `PDFDocEncoding` for the bytes `0x80..=0x9E` (Table D.2); `0x9F` is undefined.
const PDF_DOC_HIGH: [char; 31] = [
    '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}', '\u{2044}',
    '\u{2039}', '\u{203A}', '\u{2212}', '\u{2030}', '\u{201E}', '\u{201C}', '\u{201D}', '\u{2018}',
    '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}', '\u{FB02}', '\u{0141}', '\u{0152}', '\u{0160}',
    '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}', '\u{0153}', '\u{0161}', '\u{017E}',
];

/// `PDFDocEncoding` for the bytes `0x18..=0x1F` (accents).
const PDF_DOC_ACCENTS: [char; 8] = [
    '\u{02D8}', '\u{02C7}', '\u{02C6}', '\u{02D9}', '\u{02DD}', '\u{02DB}', '\u{02DA}', '\u{02DC}',
];

fn pdf_doc_char(byte: u8) -> char {
    match byte {
        0x18..=0x1F => PDF_DOC_ACCENTS[usize::from(byte - 0x18)],
        0x80..=0x9E => PDF_DOC_HIGH[usize::from(byte - 0x80)],
        0x9F | 0xAD => char::REPLACEMENT_CHARACTER,
        0xA0 => '\u{20AC}',
        _ => char::from(byte),
    }
}

/// `text` with every control character replaced by a space (tab and line breaks included), so that
/// a title can never move the cursor or break a line of the user interface.
fn without_controls(text: String) -> String {
    if text.chars().any(char::is_control) {
        text.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    } else {
        text
    }
}

fn utf16(bytes: &[u8], big_endian: bool) -> String {
    // A last odd byte is dropped.
    let units = bytes.as_chunks::<2>().0.iter().map(|&pair| {
        if big_endian {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        }
    });
    char::decode_utf16(units)
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Decodes a PDF text string. See the [module documentation](self).
#[must_use]
pub fn decode_text_string(bytes: &[u8]) -> String {
    let text = if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        utf16(rest, true)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        utf16(rest, false)
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        bytes.iter().map(|&byte| pdf_doc_char(byte)).collect()
    };
    without_controls(text)
}

/// `text` cut to at most `max_chars` characters, with an ellipsis if anything was cut.
#[must_use]
pub fn truncated(text: String, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        None => text,
        Some((end, _)) => {
            let mut short = text;
            short.truncate(end);
            short.push('\u{2026}');
            short
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_latin_1_come_through() {
        assert_eq!(decode_text_string(b"Chapter 1"), "Chapter 1");
        assert_eq!(decode_text_string(b"caf\xE9 \xFC"), "caf\u{E9} \u{FC}");
    }

    #[test]
    fn pdf_doc_encoding_differs_from_latin_1_where_it_should() {
        // Bullet, ellipsis, em dash, fi ligature, Euro, caron and the undefined byte.
        assert_eq!(
            decode_text_string(b"\x80\x83\x84\x93\xA0\x19\x9F"),
            "\u{2022}\u{2026}\u{2014}\u{FB01}\u{20AC}\u{2C7}\u{FFFD}"
        );
    }

    #[test]
    fn utf_16_with_a_byte_order_mark_in_both_orders() {
        assert_eq!(
            decode_text_string(b"\xFE\xFF\x00H\x00i\x20\xAC"),
            "Hi\u{20AC}"
        );
        assert_eq!(
            decode_text_string(b"\xFF\xFEH\x00i\x00\xAC\x20"),
            "Hi\u{20AC}"
        );
        // A surrogate pair, and a lone surrogate and a stray last byte.
        assert_eq!(decode_text_string(b"\xFE\xFF\xD8\x3D\xDE\x00"), "\u{1F600}");
        assert_eq!(
            decode_text_string(b"\xFE\xFF\xD8\x3D\x00A\x00"),
            "\u{FFFD}A"
        );
    }

    #[test]
    fn utf_8_with_a_byte_order_mark_and_bad_utf_8() {
        assert_eq!(
            decode_text_string(b"\xEF\xBB\xBFna\xC3\xAFve"),
            "na\u{EF}ve"
        );
        assert_eq!(decode_text_string(b"\xEF\xBB\xBFa\xFFb"), "a\u{FFFD}b");
    }

    #[test]
    fn control_characters_become_spaces() {
        assert_eq!(decode_text_string(b"a\x00b\nc\td\x7F"), "a b c d ");
        assert_eq!(decode_text_string(b"\xFE\xFF\x00a\x00\n\x00b"), "a b");
    }

    #[test]
    fn empty_input_is_an_empty_string() {
        assert_eq!(decode_text_string(b""), "");
        assert_eq!(decode_text_string(b"\xFE\xFF"), "");
    }

    #[test]
    fn truncation_is_by_characters_and_marks_the_cut() {
        assert_eq!(truncated("abc".into(), 3), "abc");
        assert_eq!(truncated("abcd".into(), 3), "abc\u{2026}");
        // Multi-byte characters are never cut in half.
        assert_eq!(
            truncated("\u{E9}\u{E9}\u{E9}".into(), 2),
            "\u{E9}\u{E9}\u{2026}"
        );
        assert_eq!(truncated(String::new(), 0), "");
        assert_eq!(truncated("a".into(), 0), "\u{2026}");
    }
}
