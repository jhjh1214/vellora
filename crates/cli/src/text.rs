//! `vellora text`: the text of pages in reading order (the order PDFium extracts it), read by the
//! engine. Pages are separated by a form feed (U+000C), as `pdftotext` does, and line breaks are
//! `\n`.

use std::io::Write;
use std::path::Path;

use vellora_engine_client::{Client, Event};

use crate::engine;
use crate::pagespec;

/// Characters asked for at a time (the protocol's most).
const WINDOW: u32 = 8192;

/// Prints the text of `pages` (a specification, all pages if `None`) of `file` to `out`.
pub(crate) fn run(
    file: &Path,
    pages: Option<&str>,
    password: Option<&str>,
    out: &mut impl Write,
) -> Result<(), String> {
    let client = engine::open(file, password)?;
    let count = client
        .page_count()
        .ok_or("the engine reported no page count")?;
    let wanted =
        pagespec::parse(pages.unwrap_or("1-"), count).map_err(|error| error.to_string())?;
    for (n, &page) in wanted.iter().enumerate() {
        let text = page_text(&client, page)?;
        if n > 0 {
            out.write_all(b"\x0c")
                .map_err(|error| write_error(&error))?;
        }
        out.write_all(text.as_bytes())
            .map_err(|error| write_error(&error))?;
    }
    out.flush().map_err(|error| write_error(&error))
}

fn write_error(error: &std::io::Error) -> String {
    format!("cannot write the text: {error}")
}

/// `text` with every control character except the line feed and the tab replaced by a space. The
/// text comes from the file, and printing an escape (or a form feed, which separates the pages
/// here) from it would let the file steer the terminal or the page breaks.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// All the text of one page.
fn page_text(client: &Client, page: u32) -> Result<String, String> {
    let mut text = String::new();
    let mut skip = 0u32;
    loop {
        let request = client
            .request_text_page(page, skip, WINDOW)
            .map_err(|error| error.to_string())?;
        let (total, chars) = engine::wait(client, |event| match event {
            Event::TextPage {
                request: r,
                total,
                chars,
                ..
            } if *r == request => Some(Ok((*total, chars.clone()))),
            Event::RequestFailed {
                request: Some(r),
                message,
                ..
            } if *r == request => Some(Err(format!("page {}: {message}", page + 1))),
            _ => None,
        })?;
        skip += u32::try_from(chars.len()).unwrap_or(u32::MAX);
        text.extend(chars.iter().map(|c| c.ch));
        if chars.is_empty() || skip >= total {
            break;
        }
    }
    // PDFium ends a line with CR LF; a lone CR is a line end too.
    Ok(printable(&text.replace("\r\n", "\n").replace('\r', "\n")))
}

#[cfg(test)]
mod tests {
    use super::printable;

    #[test]
    fn controls_from_the_file_are_not_printed() {
        assert_eq!(printable("a\u{1b}[2Jb"), "a [2Jb");
        assert_eq!(printable("x\u{c}y\u{7}z\u{9b}w\u{0}"), "x y z w ");
        // Line ends, tabs and every printable character stay.
        assert_eq!(
            printable("one\ntwo\tthree caf\u{e9} \u{4e2d}"),
            "one\ntwo\tthree caf\u{e9} \u{4e2d}"
        );
    }
}
