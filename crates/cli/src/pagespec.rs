//! Page specifications: which pages a command works on, written as `1-5,8,10-`.
//!
//! A specification is a comma-separated list of pieces; pieces are `N` (one page), `N-M` (a range),
//! `N-` (from `N` to the last page) and `-M` (from the first page to `M`). Pages count from 1.
//! White space around pieces is ignored. The result is a list of zero-based page indices in the
//! order the pieces name them, each page once (a page named again is left where it first came).
//!
//! The parser knows nothing about documents but their page count, so page operations of later
//! milestones (extract, delete, rotate) can reuse it.

use std::fmt;

/// Why a page specification was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PageSpecError {
    /// Nothing was specified.
    Empty,
    /// A piece that is not a page, a range or an open range.
    Invalid(String),
    /// Page 0, which does not exist (pages count from 1).
    Zero(String),
    /// A page the document does not have.
    OutOfRange { page: u64, count: u32 },
    /// A range that ends before it starts.
    Backwards { first: u64, last: u64 },
    /// The document has no pages.
    NoPages,
}

impl fmt::Display for PageSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "no pages given (for example 1-5,8,10-)"),
            Self::Invalid(piece) => write!(
                f,
                "'{piece}' is not a page or a range of pages (use N, N-M, N- or -M)"
            ),
            Self::Zero(piece) => write!(f, "'{piece}': pages count from 1"),
            Self::OutOfRange { page, count } => {
                write!(f, "page {page} does not exist: the document has {count}")
            }
            Self::Backwards { first, last } => {
                write!(f, "the range {first}-{last} ends before it starts")
            }
            Self::NoPages => write!(f, "the document has no pages"),
        }
    }
}

impl std::error::Error for PageSpecError {}

/// The zero-based pages `spec` names in a document of `page_count` pages.
///
/// # Errors
///
/// [`PageSpecError`] for an empty or malformed specification, a page that is 0 or past the end, a
/// range that runs backwards, or a document without pages.
pub(crate) fn parse(spec: &str, page_count: u32) -> Result<Vec<u32>, PageSpecError> {
    if page_count == 0 {
        return Err(PageSpecError::NoPages);
    }
    let mut pages = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut any = false;
    for raw in spec.split(',') {
        let piece = raw.trim();
        if piece.is_empty() {
            // `1,,2` and a trailing comma are mistakes worth naming; a wholly empty spec is `Empty`.
            if spec.trim().is_empty() {
                continue;
            }
            return Err(PageSpecError::Invalid(piece.to_owned()));
        }
        any = true;
        let (first, last) = bounds(piece, page_count)?;
        for page in first..=last {
            // `bounds` checked the range against `page_count`, so this fits in a `u32`.
            let index = u32::try_from(page - 1).map_err(|_| PageSpecError::OutOfRange {
                page,
                count: page_count,
            })?;
            if seen.insert(index) {
                pages.push(index);
            }
        }
    }
    if any {
        Ok(pages)
    } else {
        Err(PageSpecError::Empty)
    }
}

/// The first and last page (counting from 1) a piece names.
fn bounds(piece: &str, page_count: u32) -> Result<(u64, u64), PageSpecError> {
    let invalid = || PageSpecError::Invalid(piece.to_owned());
    let number = |text: &str| -> Result<u64, PageSpecError> {
        // Digits only: no sign, no `+`, nothing a parser might accept and a person would not mean.
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        // A number too long for u64 is out of range, not a syntax error.
        let page: u64 = text.parse().unwrap_or(u64::MAX);
        if page == 0 {
            return Err(PageSpecError::Zero(piece.to_owned()));
        }
        Ok(page)
    };
    let count = u64::from(page_count);
    let check = |page: u64| {
        if page > count {
            Err(PageSpecError::OutOfRange {
                page,
                count: page_count,
            })
        } else {
            Ok(page)
        }
    };
    let (first, last) = match piece.split_once('-') {
        None => {
            let page = check(number(piece)?)?;
            (page, page)
        }
        Some((start, end)) => {
            if end.contains('-') {
                return Err(invalid());
            }
            let first = if start.trim().is_empty() {
                1
            } else {
                check(number(start.trim())?)?
            };
            let last = if end.trim().is_empty() {
                count
            } else {
                check(number(end.trim())?)?
            };
            if start.trim().is_empty() && end.trim().is_empty() {
                return Err(invalid()); // a lone "-"
            }
            (first, last)
        }
    };
    if first > last {
        return Err(PageSpecError::Backwards { first, last });
    }
    Ok((first, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(spec: &str, count: u32) -> Vec<u32> {
        parse(spec, count).unwrap()
    }

    #[test]
    fn pages_ranges_and_open_ranges() {
        assert_eq!(ok("3", 10), [2]);
        assert_eq!(ok("2-4", 10), [1, 2, 3]);
        assert_eq!(ok("8-", 10), [7, 8, 9]);
        assert_eq!(ok("-3", 10), [0, 1, 2]);
        assert_eq!(ok("1-5,8,10-", 12), [0, 1, 2, 3, 4, 7, 9, 10, 11]);
        assert_eq!(ok("1-", 1), [0]);
        assert_eq!(ok(" 2 , 4 - 5 ", 5), [1, 3, 4]);
    }

    #[test]
    fn the_order_given_is_kept_and_a_page_comes_once() {
        assert_eq!(ok("5,1,3", 5), [4, 0, 2]);
        assert_eq!(ok("1-3,2,3-4", 5), [0, 1, 2, 3]);
        assert_eq!(ok("4-5,1-5", 5), [3, 4, 0, 1, 2]);
    }

    #[test]
    fn the_last_page_is_page_count() {
        assert_eq!(ok("10", 10), [9]);
        assert_eq!(ok("1-10", 10), (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn mistakes_are_named() {
        assert_eq!(parse("", 5), Err(PageSpecError::Empty));
        assert_eq!(parse("  ", 5), Err(PageSpecError::Empty));
        assert_eq!(parse("0", 5), Err(PageSpecError::Zero("0".into())));
        assert_eq!(parse("0-3", 5), Err(PageSpecError::Zero("0-3".into())));
        assert_eq!(
            parse("6", 5),
            Err(PageSpecError::OutOfRange { page: 6, count: 5 })
        );
        assert_eq!(
            parse("2-9", 5),
            Err(PageSpecError::OutOfRange { page: 9, count: 5 })
        );
        assert_eq!(
            parse("4-2", 5),
            Err(PageSpecError::Backwards { first: 4, last: 2 })
        );
        for bad in [
            "a", "1-b", "-", "1--3", "1-2-3", "+1", "1.5", "1 2", "1,,2", "1,", ",1",
        ] {
            assert!(
                matches!(parse(bad, 5), Err(PageSpecError::Invalid(_))),
                "{bad:?} gave {:?}",
                parse(bad, 5)
            );
        }
        // A number too big for any integer is simply a page the document does not have.
        assert!(matches!(
            parse("99999999999999999999999", 5),
            Err(PageSpecError::OutOfRange { .. })
        ));
        assert_eq!(parse("1", 0), Err(PageSpecError::NoPages));
    }

    #[test]
    fn the_messages_say_what_to_do() {
        assert!(PageSpecError::Empty.to_string().contains("1-5,8,10-"));
        assert!(
            PageSpecError::OutOfRange { page: 9, count: 5 }
                .to_string()
                .contains("has 5")
        );
        assert!(
            PageSpecError::Zero("0".into())
                .to_string()
                .contains("from 1")
        );
    }

    #[test]
    fn a_huge_document_is_not_expanded_beyond_its_pages() {
        let all = ok("1-", 100_000);
        assert_eq!(all.len(), 100_000);
        assert_eq!(all.last(), Some(&99_999));
    }
}
