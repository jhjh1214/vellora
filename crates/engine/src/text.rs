//! From the characters PDFium extracts to the text of a page as the protocol carries it.
//!
//! PDFium reports UTF-16 code units, one per glyph, with the spaces and line breaks it inferred
//! marked as generated. This module joins surrogate pairs into characters, expands the
//! typographic ligatures (`ﬁ` becomes `f`, `i`, sharing the glyph's box), drops what cannot be
//! text (NUL, other control characters, lone surrogates) and numbers the words and lines.
//!
//! Hyphenation is preserved: a hyphen that breaks a word at the end of a line stays in the text,
//! flagged, so that a viewer can copy it as written or join the word as it chooses.
//!
//! Limits of the box of an expanded ligature: the letters split the glyph's width evenly, left to
//! right, which is exact for horizontal left-to-right text and approximate for the rest.

use vellora_ipc::{TEXT_GENERATED, TEXT_HYPHEN, TextChar};
use vellora_render::TextChar as RawChar;

/// The letters a ligature stands for, if the character is one.
fn ligature(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        // The long s and t, and the plain st: both read "st".
        '\u{FB05}' | '\u{FB06}' => "st",
        _ => return None,
    })
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}' | '\u{85}')
}

fn is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{200B}'
}

/// A box covering both `a` and `b`; an empty box (a generated character) takes the other.
fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let empty = |r: [f32; 4]| r.iter().all(|v| v.abs() < f32::EPSILON);
    if empty(a) {
        return b;
    }
    if empty(b) {
        return a;
    }
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// The characters of `raw` as scalar values, each with the box and flags of the glyph it came from.
fn scalars(raw: &[RawChar]) -> Vec<(char, [f32; 4], f32, u8)> {
    let mut out = Vec::with_capacity(raw.len());
    let mut at = 0;
    while at < raw.len() {
        let unit = raw[at];
        let mut flags = 0;
        if unit.generated {
            flags |= TEXT_GENERATED;
        }
        if unit.hyphen {
            flags |= TEXT_HYPHEN;
        }
        let mut rect = unit.rect;
        let mut size = unit.size;
        let mut code = unit.unicode;
        at += 1;
        // A high surrogate and the low one after it are one character.
        if (0xD800..0xDC00).contains(&code)
            && let Some(low) = raw
                .get(at)
                .filter(|next| (0xDC00..0xE000).contains(&next.unicode))
        {
            code = 0x1_0000 + ((code - 0xD800) << 10) + (low.unicode - 0xDC00);
            rect = union(rect, low.rect);
            size = size.max(low.size);
            at += 1;
        }
        let Some(c) = char::from_u32(code) else {
            continue; // a lone surrogate, or beyond Unicode: not text
        };
        // Control characters other than the white space of a text are not text either. NUL is
        // what PDFium reports for a glyph it has no Unicode value for.
        if c.is_control() && !matches!(c, '\t' | '\n' | '\r') {
            continue;
        }
        out.push((c, rect, size, flags));
    }
    out
}

/// Builds the text of a page from what PDFium extracted.
pub(crate) fn build(raw: &[RawChar]) -> Vec<TextChar> {
    let mut chars = Vec::with_capacity(raw.len());
    let mut word = 0_u32;
    let mut line = 0_u32;
    let mut started = false; // a non-space character has been seen
    let mut space_seen = false;
    let mut break_pending = false;
    for (c, rect, size, flags) in scalars(raw) {
        // A ligature becomes its letters, which share the glyph's width.
        let letters: Vec<char> = ligature(c).map_or_else(|| vec![c], |s| s.chars().collect());
        let parts = u16::try_from(letters.len()).map_or(1.0, f32::from);
        let width = (rect[2] - rect[0]) / parts;
        for (n, letter) in letters.into_iter().enumerate() {
            let mut piece = rect;
            if parts > 1.0 {
                let from = rect[0] + width * f32::from(u8::try_from(n).unwrap_or(0));
                piece[0] = from;
                piece[2] = from + width;
            }
            if is_space(letter) || is_line_break(letter) {
                space_seen = true;
                break_pending |= is_line_break(letter);
            } else {
                if started && space_seen {
                    word += 1;
                }
                if started && break_pending {
                    line += 1;
                }
                started = true;
                space_seen = false;
                break_pending = false;
            }
            chars.push(TextChar {
                ch: letter,
                rect: piece,
                size,
                flags,
                word,
                line,
            });
        }
    }
    chars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(unicode: u32) -> RawChar {
        RawChar {
            unicode,
            rect: [10.0, 20.0, 18.0, 32.0],
            size: 12.0,
            generated: false,
            hyphen: false,
        }
    }

    fn from_str(text: &str) -> Vec<RawChar> {
        text.encode_utf16().map(|u| raw(u32::from(u))).collect()
    }

    fn text(chars: &[TextChar]) -> String {
        chars.iter().map(|c| c.ch).collect()
    }

    #[test]
    fn plain_text_passes_through() {
        let built = build(&from_str("Hello, wörld"));
        assert_eq!(text(&built), "Hello, wörld");
        assert!(
            built
                .iter()
                .all(|c| c.flags == 0 && (c.size - 12.0).abs() < 1e-4)
        );
    }

    #[test]
    fn surrogate_pairs_become_one_character_with_both_boxes() {
        let mut units = from_str("a😀b");
        assert_eq!(units.len(), 4);
        units[2].rect = [30.0, 18.0, 40.0, 34.0];
        let built = build(&units);
        assert_eq!(text(&built), "a😀b");
        assert_eq!(built[1].rect, [10.0, 18.0, 40.0, 34.0]);
    }

    #[test]
    fn what_cannot_be_text_is_dropped() {
        // A lone high surrogate, a lone low one, NUL, a bell, a value beyond Unicode.
        let mut units = from_str("a");
        units.push(raw(0xD800));
        units.push(raw(u32::from(b'X')));
        units.push(raw(0xDC00));
        units.push(raw(0));
        units.push(raw(7));
        units.push(raw(0x11_0000));
        units.extend(from_str("b\tc"));
        assert_eq!(text(&build(&units)), "aXb\tc");
        // A high surrogate at the very end.
        assert_eq!(text(&build(&[raw(u32::from(b'x')), raw(0xD83D)])), "x");
    }

    #[test]
    fn ligatures_are_expanded_and_share_the_box() {
        let built = build(&[raw(0xFB03), raw(u32::from(b'x'))]);
        assert_eq!(text(&built), "ffix");
        // 8 points wide split three ways, left to right.
        let third = 8.0 / 3.0;
        for (n, c) in built[..3].iter().enumerate() {
            assert!(
                (c.rect[0] - (10.0 + third * f32::from(u8::try_from(n).unwrap()))).abs() < 1e-4,
                "{c:?}"
            );
            assert!((c.rect[2] - c.rect[0] - third).abs() < 1e-4, "{c:?}");
            assert_eq!(c.word, 0);
        }
        for (unit, expected) in [
            (0xFB00, "ff"),
            (0xFB01, "fi"),
            (0xFB02, "fl"),
            (0xFB04, "ffl"),
            (0xFB05, "st"),
            (0xFB06, "st"),
        ] {
            assert_eq!(text(&build(&[raw(unit)])), expected);
        }
    }

    #[test]
    fn words_and_lines_are_numbered() {
        let built = build(&from_str("one two\r\nthree  four\n\nfive"));
        let positions: Vec<(char, u32, u32)> =
            built.iter().map(|c| (c.ch, c.word, c.line)).collect();
        let at = |ch: char, nth: usize| {
            *positions
                .iter()
                .filter(|(c, _, _)| *c == ch)
                .nth(nth)
                .unwrap()
        };
        // Words count up across lines; white space belongs to the word before it.
        assert_eq!(at('o', 0), ('o', 0, 0));
        assert_eq!(at('t', 0), ('t', 1, 0));
        assert_eq!(at(' ', 0), (' ', 0, 0));
        assert_eq!(at('\r', 0), ('\r', 1, 0));
        assert_eq!(at('\n', 0), ('\n', 1, 0));
        // The next line starts at its first letter.
        assert_eq!(at('h', 0), ('h', 2, 1));
        assert_eq!(at('f', 0), ('f', 3, 1));
        // A run of spaces is one gap; a blank line is one line break, not two.
        assert_eq!(at('f', 1), ('f', 4, 2));
        assert_eq!(built.last().map(|c| c.line), Some(2));
    }

    #[test]
    fn leading_white_space_starts_nothing() {
        let built = build(&from_str("  \n a"));
        assert_eq!(built.len(), 5);
        assert!(built.iter().all(|c| c.word == 0 && c.line == 0));
    }

    #[test]
    fn generated_and_hyphen_flags_are_kept() {
        let mut units = from_str("co-");
        units[2].hyphen = true;
        units.push(RawChar {
            unicode: 0x0D,
            rect: [0.0; 4],
            size: 0.0,
            generated: true,
            hyphen: false,
        });
        units.push(RawChar {
            unicode: 0x0A,
            rect: [0.0; 4],
            size: 0.0,
            generated: true,
            hyphen: false,
        });
        units.extend(from_str("op"));
        let built = build(&units);
        assert_eq!(text(&built), "co-\r\nop");
        assert_eq!(built[2].flags, TEXT_HYPHEN);
        assert_eq!(built[3].flags, TEXT_GENERATED);
        assert_eq!(built[3].rect, [0.0; 4]);
        // The word goes on after the break; hyphenation is not undone here.
        assert_eq!((built[5].word, built[5].line), (1, 1));
    }
}
