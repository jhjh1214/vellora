//! Page labels (ISO 32000-2:2020 §12.4.2): the names a document gives its pages, such as `i`, `ii`,
//! `1`, `2`, `A-1`.
//!
//! The catalog's `/PageLabels` is a number tree from a page index to a dictionary that starts a
//! *range*: from that page on, pages are numbered in a `/S` style (decimal `/D`, roman `/R` and
//! `/r`, letters `/A` and `/a`, or no number at all), after a `/P` prefix, counting from `/St`
//! (default 1), until the next range starts. Letters repeat rather than carry: A to Z, then AA to
//! ZZ, then AAA.
//!
//! Limits of what is shown, so that a hostile range cannot make a label unboundedly long: roman
//! numerals only exist up to 3999, and letters up to [`MAX_LETTER_REPEATS`] repeats; beyond
//! that the number is shown in decimal. Prefixes are cut to [`MAX_PREFIX_CHARS`] characters.

use crate::dest::DestinationResolver;
use crate::error::Result;
use crate::object::ObjectKind;
use crate::textstring::{decode_text_string, truncated};
use crate::trees::number_tree_entries;

/// Prefixes are cut to this many characters.
pub const MAX_PREFIX_CHARS: usize = 64;

/// The most repeats of a letter in an alphabetic label (`ZZZZZZZZ` is 8 x 26 = the 208th page).
pub const MAX_LETTER_REPEATS: u64 = 8;

/// How the numbers of a range are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// No number: the label is the prefix alone.
    None,
    /// `1`, `2`, `3` (`/D`)
    Decimal,
    /// `I`, `II`, `III` (`/R`)
    UpperRoman,
    /// `i`, `ii`, `iii` (`/r`)
    LowerRoman,
    /// `A`, `B`, ..., `Z`, `AA`, `BB` (`/A`)
    UpperAlpha,
    /// `a`, `b`, ..., `z`, `aa`, `bb` (`/a`)
    LowerAlpha,
}

/// A run of pages numbered alike.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Range {
    first_page: u32,
    style: Style,
    prefix: String,
    start: u64,
}

/// The labels of a document's pages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageLabels {
    /// Sorted by `first_page`, which are all different.
    ranges: Vec<Range>,
}

const ROMAN: [(u64, &str); 13] = [
    (1000, "m"),
    (900, "cm"),
    (500, "d"),
    (400, "cd"),
    (100, "c"),
    (90, "xc"),
    (50, "l"),
    (40, "xl"),
    (10, "x"),
    (9, "ix"),
    (5, "v"),
    (4, "iv"),
    (1, "i"),
];

/// `n` in lower-case roman numerals, `None` outside 1 to 3999.
fn roman(mut n: u64) -> Option<String> {
    if !(1..=3999).contains(&n) {
        return None;
    }
    let mut out = String::new();
    for (value, digits) in ROMAN {
        while n >= value {
            out.push_str(digits);
            n -= value;
        }
    }
    Some(out)
}

/// The number written in lower-case roman numerals, if it is canonical (`iv`, not `iiii`).
fn parse_roman(text: &str) -> Option<u64> {
    let mut total: u64 = 0;
    let mut rest = text;
    for (value, digits) in ROMAN {
        while let Some(after) = rest.strip_prefix(digits) {
            total += value;
            rest = after;
        }
    }
    (rest.is_empty() && roman(total).as_deref() == Some(text)).then_some(total)
}

/// `n` as letters: 1 is `a`, 26 is `z`, 27 is `aa`, 28 is `bb`; `None` when that needs more than
/// [`MAX_LETTER_REPEATS`] repeats or `n` is 0.
fn alpha(n: u64) -> Option<String> {
    if n == 0 {
        return None;
    }
    let repeats = (n - 1) / 26 + 1;
    if repeats > MAX_LETTER_REPEATS {
        return None;
    }
    // `(n - 1) % 26` is below 26 and `repeats` is at most 8.
    let letter = char::from(b'a' + u8::try_from((n - 1) % 26).unwrap_or(0));
    Some(std::iter::repeat_n(letter, usize::try_from(repeats).unwrap_or(1)).collect())
}

/// The number written as repeated lower-case letters, if it is canonical.
fn parse_alpha(text: &str) -> Option<u64> {
    let first = text.chars().next()?;
    if !first.is_ascii_lowercase() || text.chars().any(|c| c != first) {
        return None;
    }
    let repeats = u64::try_from(text.chars().count()).ok()?;
    (repeats <= MAX_LETTER_REPEATS)
        .then(|| u64::from(u32::from(first) - u32::from('a')) + 1 + 26 * (repeats - 1))
}

impl Style {
    fn from_name(name: &[u8]) -> Self {
        match name {
            b"D" => Self::Decimal,
            b"R" => Self::UpperRoman,
            b"r" => Self::LowerRoman,
            b"A" => Self::UpperAlpha,
            b"a" => Self::LowerAlpha,
            _ => Self::None,
        }
    }

    /// `n` written in this style (decimal when the style cannot write it).
    fn format(self, n: u64) -> String {
        let (text, upper) = match self {
            Self::None => return String::new(),
            Self::Decimal => return n.to_string(),
            Self::UpperRoman => (roman(n), true),
            Self::LowerRoman => (roman(n), false),
            Self::UpperAlpha => (alpha(n), true),
            Self::LowerAlpha => (alpha(n), false),
        };
        match text {
            Some(text) if upper => text.to_ascii_uppercase(),
            Some(text) => text,
            None => n.to_string(),
        }
    }

    /// The number that `text` (no prefix) writes in this style, read without regard to case.
    fn parse(self, text: &str) -> Option<u64> {
        match self {
            Self::None => text.is_empty().then_some(0),
            Self::Decimal => {
                if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                text.parse().ok()
            }
            Self::UpperRoman | Self::LowerRoman => parse_roman(&text.to_ascii_lowercase()),
            Self::UpperAlpha | Self::LowerAlpha => parse_alpha(&text.to_ascii_lowercase()),
        }
    }
}

impl Range {
    fn label(&self, page: u32) -> String {
        let n = self
            .start
            .saturating_add(u64::from(page.saturating_sub(self.first_page)));
        let mut label = self.prefix.clone();
        label.push_str(&self.style.format(n));
        label
    }
}

impl PageLabels {
    /// The labels the catalog's `/PageLabels` defines, or `None` if it has none.
    ///
    /// # Errors
    ///
    /// [`Error::LimitExceeded`](crate::Error::LimitExceeded) for a number tree with too many nodes
    /// or entries, and the store's errors for objects that cannot be read.
    pub fn read(resolver: &DestinationResolver<'_, '_>) -> Result<Option<Self>> {
        let Some(root) = resolver.catalog_entry(b"PageLabels")? else {
            return Ok(None);
        };
        let store = resolver.store();
        let mut ranges: Vec<Range> = Vec::new();
        for (key, value) in number_tree_entries(store, &root)? {
            let Ok(first_page) = u32::try_from(key) else {
                continue; // a negative page, or one beyond any document
            };
            if ranges
                .last()
                .is_some_and(|last| last.first_page == first_page)
            {
                continue; // the same page twice: the first wins
            }
            let value = store.deref(&value)?;
            let Some(dict) = value.as_dict() else {
                continue;
            };
            let style = match dict.get(b"S").map(|style| &style.kind) {
                Some(ObjectKind::Name(name)) => Style::from_name(name),
                _ => Style::None,
            };
            let prefix = match dict.get(b"P") {
                Some(prefix) => match &store.deref(prefix)?.kind {
                    ObjectKind::String(bytes) => {
                        truncated(decode_text_string(bytes), MAX_PREFIX_CHARS)
                    }
                    _ => String::new(),
                },
                None => String::new(),
            };
            let start = match dict.get(b"St") {
                Some(start) => store.deref(start)?.as_integer(),
                None => None,
            };
            // The first number is at least 1; anything else is a broken entry.
            let start = u64::try_from(start.unwrap_or(1)).unwrap_or(1).max(1);
            ranges.push(Range {
                first_page,
                style,
                prefix,
                start,
            });
        }
        Ok((!ranges.is_empty()).then_some(Self { ranges }))
    }

    /// The label of `page` (zero-based); `None` for a page before the first range, which has none.
    #[must_use]
    pub fn label(&self, page: u32) -> Option<String> {
        let at = self
            .ranges
            .partition_point(|range| range.first_page <= page);
        self.ranges
            .get(at.checked_sub(1)?)
            .map(|range| range.label(page))
    }

    /// The first page of a document of `page_count` pages whose label is `text`: an exact match,
    /// else one that differs in case only (`IV` for `iv`).
    #[must_use]
    pub fn find(&self, text: &str, page_count: u32) -> Option<u32> {
        for exact in [true, false] {
            for (at, range) in self.ranges.iter().enumerate() {
                let end = self
                    .ranges
                    .get(at + 1)
                    .map_or(page_count, |next| next.first_page.min(page_count));
                let Some(rest) = text.get(range.prefix.len()..) else {
                    continue;
                };
                let head = &text[..range.prefix.len()];
                if !(if exact {
                    head == range.prefix
                } else {
                    head.eq_ignore_ascii_case(&range.prefix)
                }) {
                    continue;
                }
                let Some(n) = range.style.parse(rest) else {
                    continue;
                };
                let page = if range.style == Style::None {
                    range.first_page
                } else {
                    let Some(offset) = n.checked_sub(range.start) else {
                        continue;
                    };
                    let Ok(offset) = u32::try_from(offset) else {
                        continue;
                    };
                    let Some(page) = range.first_page.checked_add(offset) else {
                        continue;
                    };
                    page
                };
                if page >= end {
                    continue;
                }
                let label = range.label(page);
                if (exact && label == text) || (!exact && label.eq_ignore_ascii_case(text)) {
                    return Some(page);
                }
            }
        }
        None
    }

    /// The labels of `count` pages from `first` (zero-based), stopping at `page_count`. A page
    /// without a label gets its number (counting from 1).
    #[must_use]
    pub fn window(&self, first: u32, count: u32, page_count: u32) -> Vec<String> {
        (first..first.saturating_add(count).min(page_count))
            .map(|page| {
                self.label(page)
                    .unwrap_or_else(|| (u64::from(page) + 1).to_string())
            })
            .collect()
    }

    /// Number of ranges.
    #[must_use]
    pub fn range_count(&self) -> usize {
        self.ranges.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(ranges: &[(u32, Style, &str, u64)]) -> PageLabels {
        PageLabels {
            ranges: ranges
                .iter()
                .map(|&(first_page, style, prefix, start)| Range {
                    first_page,
                    style,
                    prefix: prefix.into(),
                    start,
                })
                .collect(),
        }
    }

    #[test]
    fn roman_numerals_are_canonical_and_bounded() {
        for (n, text) in [
            (1, "i"),
            (4, "iv"),
            (9, "ix"),
            (14, "xiv"),
            (40, "xl"),
            (90, "xc"),
            (400, "cd"),
            (1994, "mcmxciv"),
            (3999, "mmmcmxcix"),
        ] {
            assert_eq!(roman(n).as_deref(), Some(text), "{n}");
            assert_eq!(parse_roman(text), Some(n), "{text}");
        }
        assert_eq!(roman(0), None);
        assert_eq!(roman(4000), None);
        // Not canonical: reading them back would give a label that is not the one shown.
        for text in ["iiii", "vv", "im", "", "ic", "xxxx"] {
            assert_eq!(parse_roman(text), None, "{text}");
        }
    }

    #[test]
    fn letters_repeat_instead_of_carrying() {
        for (n, text) in [
            (1, "a"),
            (26, "z"),
            (27, "aa"),
            (28, "bb"),
            (52, "zz"),
            (53, "aaa"),
        ] {
            assert_eq!(alpha(n).as_deref(), Some(text), "{n}");
            assert_eq!(parse_alpha(text), Some(n), "{text}");
        }
        assert_eq!(alpha(0), None);
        assert_eq!(alpha(26 * 8).as_deref(), Some("zzzzzzzz"));
        assert_eq!(alpha(26 * 8 + 1), None);
        for text in ["", "ab", "aA", "a1", "aaaaaaaaa"] {
            assert_eq!(parse_alpha(text), None, "{text}");
        }
    }

    #[test]
    fn every_style_writes_its_numbers() {
        assert_eq!(Style::Decimal.format(12), "12");
        assert_eq!(Style::UpperRoman.format(14), "XIV");
        assert_eq!(Style::LowerRoman.format(14), "xiv");
        assert_eq!(Style::UpperAlpha.format(28), "BB");
        assert_eq!(Style::LowerAlpha.format(28), "bb");
        assert_eq!(Style::None.format(7), "");
        // What a style cannot write is written in decimal.
        assert_eq!(Style::UpperRoman.format(4000), "4000");
        assert_eq!(Style::LowerAlpha.format(500), "500");
    }

    #[test]
    fn labels_follow_the_ranges() {
        let labels = labels(&[
            (0, Style::LowerRoman, "", 1),
            (4, Style::Decimal, "", 1),
            (10, Style::Decimal, "A-", 1),
            (12, Style::None, "Index", 1),
            (13, Style::UpperAlpha, "", 3),
        ]);
        let all: Vec<String> = (0..16).map(|page| labels.label(page).unwrap()).collect();
        assert_eq!(
            all,
            [
                "i", "ii", "iii", "iv", "1", "2", "3", "4", "5", "6", "A-1", "A-2", "Index", "C",
                "D", "E"
            ]
        );
    }

    #[test]
    fn pages_before_the_first_range_have_no_label() {
        let labels = labels(&[(2, Style::Decimal, "", 1)]);
        assert_eq!(labels.label(0), None);
        assert_eq!(labels.label(1), None);
        assert_eq!(labels.label(2).as_deref(), Some("1"));
        assert_eq!(labels.window(0, 4, 4), ["1", "2", "1", "2"]);
    }

    #[test]
    fn a_window_is_cut_at_the_end_of_the_document() {
        let labels = labels(&[(0, Style::Decimal, "p", 1)]);
        assert_eq!(labels.window(1, 100, 3), ["p2", "p3"]);
        assert_eq!(labels.window(5, 3, 3), Vec::<String>::new());
        assert_eq!(labels.window(u32::MAX, u32::MAX, 3), Vec::<String>::new());
    }

    #[test]
    fn labels_are_found_by_their_text() {
        let labels = labels(&[
            (0, Style::LowerRoman, "", 1),
            (4, Style::Decimal, "", 1),
            (10, Style::Decimal, "A-", 1),
            (12, Style::None, "Index", 1),
        ]);
        for (text, page) in [
            ("i", 0),
            ("iv", 3),
            ("1", 4),
            ("6", 9),
            ("A-1", 10),
            ("A-2", 11),
            ("Index", 12),
        ] {
            assert_eq!(labels.find(text, 14), Some(page), "{text}");
        }
        // Case is ignored when nothing matches exactly.
        assert_eq!(labels.find("IV", 14), Some(3));
        assert_eq!(labels.find("a-2", 14), Some(11));
        assert_eq!(labels.find("index", 14), Some(12));
        // Pages that do not exist, and labels that are not any page's.
        assert_eq!(labels.find("7", 14), None); // page 10 is "A-1": the decimal range ends there
        assert_eq!(labels.find("A-3", 14), None);
        assert_eq!(labels.find("v", 14), None);
        assert_eq!(labels.find("A-2", 11), None);
        assert_eq!(labels.find("", 14), None);
        assert_eq!(labels.find("iiii", 14), None);
        // A text that cuts a multi-byte character where the prefix ends does not panic.
        assert_eq!(labels.find("\u{E9}", 14), None);
    }

    #[test]
    fn the_first_page_with_a_label_wins_and_exact_beats_case() {
        let labels = labels(&[(0, Style::LowerAlpha, "", 1), (5, Style::UpperAlpha, "", 1)]);
        // "A" is the sixth page's label; "a" is the first's; each is found exactly.
        assert_eq!(labels.find("a", 10), Some(0));
        assert_eq!(labels.find("A", 10), Some(5));
        // "b" matches exactly page 1; "B" is page 6 exactly, never page 1 by case.
        assert_eq!(labels.find("B", 10), Some(6));
        assert_eq!(labels.find("b", 10), Some(1));
    }
}
