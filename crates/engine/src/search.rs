//! Searching the text of the whole document, on a thread of its own.
//!
//! A search reads the text of each page in turn through the PDFium thread, one page at a time, so
//! a tile that is wanted in between waits for at most one page of text. It does not touch the text
//! the navigator keeps for the page on screen. Hits are sent in batches as they are found, and a
//! progress message goes out from time to time so that a long stretch of pages without a hit still
//! shows that the search is alive.
//!
//! The search text is a regular expression of the `regex` crate whatever the user asked for (plain
//! text is escaped), which matches in time linear in the page's text; its compiled size is capped.
//! A search is cancelled by a newer one, by `Cancel`, or by the end of the session; it looks for
//! that before every page and every [`CANCEL_CHECK_HITS`] hits.
//!
//! What is matched is the text as the user reads it: runs of white space, and the line breaks and
//! spaces PDFium puts between words and lines, count as one space. A hyphen at the end of a line
//! stays, as it does when copying.

use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use regex::{Regex, RegexBuilder};
use vellora_ipc::{
    ErrorKind, MAX_HIT_RECTS, MAX_SEARCH_HITS, MAX_SEARCH_HITS_PER_MESSAGE, MAX_SNIPPET_BYTES,
    RequestId, Response, SearchHit, SearchOutcome, SearchQuery, TEXT_GENERATED, TextChar,
};

use crate::document::Document;
use crate::session::{error_response, send_shared};
use crate::text;

/// Characters read from one page; a page with more is cut there (as for `GetTextPage`).
const MAX_PAGE_CHARS: usize = 1 << 19;
/// Size limit of the compiled expression, in bytes.
const REGEX_SIZE_LIMIT: usize = 1 << 21;
/// Size limit of the lazy DFA's cache, in bytes.
const REGEX_DFA_LIMIT: usize = 1 << 23;
/// Deepest nesting of the expression.
const REGEX_NESTING: u32 = 64;
/// Characters of context before and after a match in a snippet.
const CONTEXT_CHARS: usize = 24;
/// Characters of a match shown in a snippet.
const MATCH_CHARS: usize = 96;
/// A hit batch is sent at once when the previous message left this long ago.
const BATCH_INTERVAL: Duration = Duration::from_millis(30);
/// A progress message is sent when nothing else was for this long.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Hits found in one page between two looks at whether the search is still wanted.
const CANCEL_CHECK_HITS: usize = 256;

/// What the reader thread sends this one.
pub(crate) enum Message {
    /// The document is open; search it. Sent once, before any `Start`.
    Open {
        /// The document, for the text of its pages.
        document: Arc<Document>,
        /// The page count PDFium reported.
        page_count: u32,
    },
    /// Search the document. The reader has already made this the current search.
    Start {
        /// The request.
        req_id: RequestId,
        /// What to look for.
        query: SearchQuery,
    },
}

/// Which search is wanted. Shared between the reader, which says so, and the search thread, which
/// looks before every page.
#[derive(Debug, Default)]
pub(crate) struct Control {
    current: Mutex<Option<u64>>,
}

impl Control {
    /// `req_id` is now the one search that is wanted (and any other is not).
    pub(crate) fn begin(&self, req_id: RequestId) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(req_id.0);
    }

    /// `req_id` is no longer wanted, if it was the current search.
    pub(crate) fn cancel(&self, req_id: RequestId) {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if *current == Some(req_id.0) {
            *current = None;
        }
    }

    /// No search is wanted any more.
    pub(crate) fn stop(&self) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn wanted(&self, req_id: RequestId) -> bool {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) == Some(req_id.0)
    }
}

/// The search thread's body: waits for the document, then runs the searches it is sent until the
/// reader drops its end of the channel.
pub(crate) fn run<W: Write>(messages: &Receiver<Message>, output: &Mutex<W>, control: &Control) {
    let (document, page_count) = loop {
        match messages.recv() {
            Ok(Message::Open {
                document,
                page_count,
            }) => break (document, page_count),
            // The reader checks a document is open first; this is only a guard.
            Ok(Message::Start { req_id, .. }) => {
                let refusal = error_response(
                    Some(req_id),
                    ErrorKind::InvalidRequest,
                    &"no document is open",
                );
                if !matches!(send_shared(output, &refusal), Ok(true)) {
                    return;
                }
            }
            Err(_) => return,
        }
    };
    while let Ok(message) = messages.recv() {
        let Message::Start { req_id, query } = message else {
            continue; // a second `Open` cannot happen: the reader refuses it
        };
        let source = |page: u32| {
            document
                .handle
                .page_text(usize::try_from(page).unwrap_or(usize::MAX), MAX_PAGE_CHARS)
                .map(|raw| text::build(&raw))
        };
        let sent = catch_unwind(AssertUnwindSafe(|| {
            search(req_id, &query, page_count, control, output, source)
        }))
        .unwrap_or_else(|_| {
            tracing::error!("a search panicked");
            let failure = error_response(Some(req_id), ErrorKind::Internal, &"internal error");
            matches!(send_shared(output, &failure), Ok(true))
        });
        if !sent {
            return;
        }
    }
}

/// Runs one search. `false` when the UI is gone and nothing more can be sent.
fn search<W: Write, E: std::fmt::Display>(
    req_id: RequestId,
    query: &SearchQuery,
    page_count: u32,
    control: &Control,
    output: &Mutex<W>,
    mut source: impl FnMut(u32) -> Result<Vec<TextChar>, E>,
) -> bool {
    let send = |response: &Response| match send_shared(output, response) {
        Ok(open) => open,
        Err(error) => {
            tracing::error!(%error, "a response could not be sent");
            false
        }
    };
    let regex = match compile(query) {
        Ok(regex) => regex,
        Err(reason) => {
            return send(&error_response(
                Some(req_id),
                ErrorKind::InvalidRequest,
                &reason,
            ));
        }
    };

    let mut pending: Vec<SearchHit> = Vec::new();
    let mut total = 0_u32;
    let mut pages_done = 0_u32;
    let mut last_sent: Option<Instant> = None;
    let mut outcome = SearchOutcome::Finished;
    'pages: for page in 0..page_count {
        if !control.wanted(req_id) {
            outcome = SearchOutcome::Cancelled;
            break;
        }
        match source(page) {
            Ok(chars) => {
                let haystack = Haystack::new(&chars);
                for (n, found) in regex.find_iter(&haystack.text).enumerate() {
                    if n % CANCEL_CHECK_HITS == CANCEL_CHECK_HITS - 1 && !control.wanted(req_id) {
                        outcome = SearchOutcome::Cancelled;
                        break 'pages;
                    }
                    if found.is_empty() {
                        continue;
                    }
                    if let Some(hit) = haystack.hit(page, &chars, found.start(), found.end()) {
                        pending.push(hit);
                        total += 1;
                        if pending.len() == MAX_SEARCH_HITS_PER_MESSAGE
                            && !send(&batch(req_id, &mut pending, pages_done))
                        {
                            return false;
                        }
                        if total >= MAX_SEARCH_HITS {
                            outcome = SearchOutcome::TooManyHits;
                            break;
                        }
                    }
                }
            }
            // A page PDFium cannot read has no text to find; the rest is still searched.
            Err(error) => tracing::debug!(page, %error, "the text of a page cannot be read"),
        }
        pages_done += 1;
        if outcome == SearchOutcome::TooManyHits {
            break;
        }
        let quiet = last_sent.map_or(Duration::MAX, |at| at.elapsed());
        let due = if pending.is_empty() {
            quiet >= PROGRESS_INTERVAL
        } else {
            // The first hit goes out at once; later ones in batches.
            last_sent.is_none() || quiet >= BATCH_INTERVAL
        };
        if due {
            last_sent = Some(Instant::now());
            if !send(&batch(req_id, &mut pending, pages_done)) {
                return false;
            }
        }
    }
    if !pending.is_empty() && !send(&batch(req_id, &mut pending, pages_done)) {
        return false;
    }
    send(&Response::SearchDone {
        req_id,
        outcome,
        hits: total,
        pages_done,
    })
}

fn batch(req_id: RequestId, pending: &mut Vec<SearchHit>, pages_done: u32) -> Response {
    Response::SearchHits {
        req_id,
        hits: std::mem::take(pending),
        pages_done,
    }
}

/// The expression for a query, or why it cannot be used.
fn compile(query: &SearchQuery) -> Result<Regex, String> {
    let body = if query.regex {
        query.text.clone()
    } else {
        regex::escape(&query.text)
    };
    let pattern = if query.whole_word {
        format!(r"\b(?:{body})\b")
    } else {
        body
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!query.case_sensitive)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_LIMIT)
        .nest_limit(REGEX_NESTING)
        .build()
        .map_err(|error| {
            // The message of a syntax error quotes the whole expression; keep it short.
            let mut text = error.to_string();
            if text.len() > 512 {
                let mut end = 512;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            text
        })
}

/// The text of a page as it is searched, and where each of its characters came from.
struct Haystack {
    text: String,
    /// Byte offset in `text` of each of its characters.
    offsets: Vec<u32>,
    /// Index in the page's characters of the one each character of `text` stands for (the first
    /// of a run of white space).
    source: Vec<u32>,
}

impl Haystack {
    fn new(chars: &[TextChar]) -> Self {
        let mut text = String::with_capacity(chars.len());
        let mut offsets = Vec::with_capacity(chars.len());
        let mut source = Vec::with_capacity(chars.len());
        let mut in_space = true; // white space at the start is dropped too
        for (index, c) in chars.iter().enumerate() {
            let space = c.ch.is_whitespace();
            if space && in_space {
                continue;
            }
            in_space = space;
            offsets.push(u32::try_from(text.len()).unwrap_or(u32::MAX));
            source.push(u32::try_from(index).unwrap_or(u32::MAX));
            text.push(if space { ' ' } else { c.ch });
        }
        Self {
            text,
            offsets,
            source,
        }
    }

    /// Byte offset of character `index` of `text` (the end of the text for one past the last).
    fn byte(&self, index: usize) -> usize {
        self.offsets.get(index).map_or(self.text.len(), |at| {
            usize::try_from(*at).unwrap_or(usize::MAX)
        })
    }

    /// The hit for the match at bytes `start..end` of `text`; `None` if it covers no character.
    fn hit(&self, page: u32, chars: &[TextChar], start: usize, end: usize) -> Option<SearchHit> {
        let first = self
            .offsets
            .partition_point(|at| usize::try_from(*at).is_ok_and(|at| at < start));
        let after = self
            .offsets
            .partition_point(|at| usize::try_from(*at).is_ok_and(|at| at < end));
        if first >= after {
            return None;
        }
        let from = usize::try_from(*self.source.get(first)?).ok()?;
        let to = usize::try_from(*self.source.get(after - 1)?).ok()?;
        let covered = chars.get(from..=to)?;

        let shown = after.min(first + MATCH_CHARS);
        let before = first.saturating_sub(CONTEXT_CHARS);
        let mut tail = (shown + CONTEXT_CHARS).min(self.offsets.len());
        let begin = self.byte(before);
        // The context after the match gives way when the snippet would be too long.
        let mut stop = self.byte(tail);
        while stop - begin > MAX_SNIPPET_BYTES && tail > shown {
            tail -= 1;
            stop = self.byte(tail);
        }
        let snippet = self.text.get(begin..stop)?.to_owned();
        Some(SearchHit {
            page,
            first: u32::try_from(from).ok()?,
            count: u32::try_from(to - from + 1).ok()?,
            rects: rects(covered),
            match_start: u32::try_from(self.byte(first) - begin).ok()?,
            match_len: u32::try_from(self.byte(shown) - self.byte(first)).ok()?,
            snippet,
        })
    }
}

/// One box per line of `covered`, at most [`MAX_HIT_RECTS`] (the rest joins the last).
fn rects(covered: &[TextChar]) -> Vec<[f32; 4]> {
    let mut out: Vec<(u32, [f32; 4])> = Vec::new();
    for c in covered {
        // A generated character has no box.
        if c.flags & TEXT_GENERATED != 0 || c.rect.iter().all(|v| *v == 0.0) {
            continue;
        }
        let full = out.len() >= MAX_HIT_RECTS;
        match out.last_mut() {
            Some((line, rect)) if *line == c.line || full => {
                rect[0] = rect[0].min(c.rect[0]);
                rect[1] = rect[1].min(c.rect[1]);
                rect[2] = rect[2].max(c.rect[2]);
                rect[3] = rect[3].max(c.rect[3]);
            }
            _ => out.push((c.line, c.rect)),
        }
    }
    out.into_iter().map(|(_, rect)| rect).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<TextChar> {
        let mut line = 0;
        text.chars()
            .enumerate()
            .map(|(n, ch)| {
                if ch == '\n' {
                    line += 1;
                }
                #[allow(clippy::cast_precision_loss)]
                let x = n as f32;
                TextChar {
                    ch,
                    rect: [
                        x,
                        0.0,
                        x + 1.0,
                        10.0 * (f32::from(u8::try_from(line).unwrap()) + 1.0),
                    ],
                    size: 10.0,
                    flags: if ch == '\n' { TEXT_GENERATED } else { 0 },
                    word: 0,
                    line,
                }
            })
            .collect()
    }

    fn query(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.into(),
            case_sensitive: false,
            whole_word: false,
            regex: false,
        }
    }

    /// Every hit of `query` in `text`, as (first, count, snippet-match).
    fn find(text: &str, query: &SearchQuery) -> Vec<(u32, u32, String)> {
        let chars = chars(text);
        let haystack = Haystack::new(&chars);
        let regex = compile(query).unwrap();
        regex
            .find_iter(&haystack.text)
            .filter_map(|m| haystack.hit(0, &chars, m.start(), m.end()))
            .map(|hit| {
                let start = hit.match_start as usize;
                let end = start + hit.match_len as usize;
                (hit.first, hit.count, hit.snippet[start..end].to_owned())
            })
            .collect()
    }

    #[test]
    fn plain_text_is_found_whatever_its_case_unless_asked_otherwise() {
        let text = "One fish, two Fish, red fish";
        assert_eq!(find(text, &query("fish")).len(), 3);
        let exact = SearchQuery {
            case_sensitive: true,
            ..query("Fish")
        };
        assert_eq!(find(text, &exact), [(14, 4, "Fish".to_owned())]);
    }

    #[test]
    fn plain_text_is_not_an_expression() {
        assert_eq!(find("a.c abc a.c", &query("a.c")).len(), 2);
        assert_eq!(find("1+1 11", &query("1+1")).len(), 1);
    }

    #[test]
    fn whole_words_stop_at_word_boundaries() {
        let words = SearchQuery {
            whole_word: true,
            ..query("cat")
        };
        let hits = find("cat concat cat. cats", &words);
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), [0, 11]);
    }

    #[test]
    fn expressions_match_and_whole_word_applies_to_the_whole_expression() {
        let re = SearchQuery {
            regex: true,
            ..query(r"gr[ae]y|colou?r")
        };
        assert_eq!(find("grey gray color colour green", &re).len(), 4);
        let words = SearchQuery {
            whole_word: true,
            regex: true,
            ..query("a|ab")
        };
        assert_eq!(find("ab a b ba", &words).len(), 2);
    }

    #[test]
    fn a_match_may_span_lines_because_a_break_is_a_space() {
        let hits = find("the quick\nbrown fox", &query("quick brown"));
        assert_eq!(hits, [(4, 11, "quick brown".to_owned())]);
        // Runs of white space count as one.
        let hits = find("a \n  \n b", &query("a b"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, 8, "the match covers the text it spans");
    }

    #[test]
    fn a_hit_has_one_box_per_line_and_none_for_a_break() {
        let covered = chars("ab\ncd");
        let boxes = rects(&covered);
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0], [0.0, 0.0, 2.0, 10.0]);
        assert_eq!(boxes[1][0], 3.0);
        // More lines than the cap are joined into the last box.
        let many = chars(&"x\n".repeat(MAX_HIT_RECTS + 5));
        assert_eq!(rects(&many).len(), MAX_HIT_RECTS);
    }

    #[test]
    fn a_snippet_is_the_match_with_context_and_fits_the_limit() {
        let text = format!("{}needle{}", "b".repeat(100), "a".repeat(100));
        let chars = chars(&text);
        let haystack = Haystack::new(&chars);
        let found = compile(&query("needle"))
            .unwrap()
            .find(&haystack.text)
            .unwrap();
        let hit = haystack.hit(3, &chars, found.start(), found.end()).unwrap();
        assert_eq!(hit.page, 3);
        assert_eq!(hit.snippet.len(), CONTEXT_CHARS * 2 + 6);
        assert_eq!(
            &hit.snippet[hit.match_start as usize..][..hit.match_len as usize],
            "needle"
        );
        // A very long match of wide characters is cut and the whole still fits.
        let wide = "é".repeat(500);
        let chars = chars_of(&wide);
        let haystack = Haystack::new(&chars);
        let hit = haystack.hit(0, &chars, 0, haystack.text.len()).unwrap();
        assert!(hit.snippet.len() <= MAX_SNIPPET_BYTES);
        assert_eq!(hit.count, 500);
        assert_eq!(hit.match_len as usize, MATCH_CHARS * 2);
    }

    fn chars_of(text: &str) -> Vec<TextChar> {
        chars(text)
    }

    #[test]
    fn expressions_that_are_invalid_or_too_big_are_refused_with_a_reason() {
        for bad in ["(", "a{1000}{1000}", r"(?<=a)b", r"(a)\1"] {
            let query = SearchQuery {
                regex: true,
                ..query(bad)
            };
            let reason = compile(&query).unwrap_err();
            assert!(!reason.is_empty() && reason.len() <= 512, "{bad}: {reason}");
        }
    }

    #[test]
    fn a_pathological_expression_runs_in_linear_time() {
        let text = "a".repeat(200_000);
        let re = SearchQuery {
            regex: true,
            ..query("(a*)*b")
        };
        let started = Instant::now();
        assert_eq!(find(&text, &re).len(), 0);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_control_follows_the_newest_search() {
        let control = Control::default();
        control.begin(RequestId(1));
        assert!(control.wanted(RequestId(1)));
        control.begin(RequestId(2));
        assert!(!control.wanted(RequestId(1)) && control.wanted(RequestId(2)));
        control.cancel(RequestId(1)); // not the current one: no effect
        assert!(control.wanted(RequestId(2)));
        control.cancel(RequestId(2));
        assert!(!control.wanted(RequestId(2)));
        control.begin(RequestId(3));
        control.stop();
        assert!(!control.wanted(RequestId(3)));
    }

    #[test]
    fn a_search_reports_hits_progress_and_the_end() {
        let control = Control::default();
        let id = RequestId(5);
        control.begin(id);
        let out = Mutex::new(Vec::new());
        let pages = ["no match here", "a needle in a haystack", "needle needle"];
        let sent = search(id, &query("needle"), 3, &control, &out, |page| {
            Ok::<_, String>(chars(pages[page as usize]))
        });
        assert!(sent);
        let wire = out.into_inner().unwrap();
        let mut cursor = std::io::Cursor::new(wire);
        let mut hits = 0;
        let mut done = None;
        while let Some(response) = vellora_ipc::read_frame::<_, Response>(&mut cursor).unwrap() {
            match response {
                Response::SearchHits { hits: found, .. } => hits += found.len(),
                Response::SearchDone {
                    outcome,
                    hits,
                    pages_done,
                    ..
                } => done = Some((outcome, hits, pages_done)),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(hits, 3);
        assert_eq!(done, Some((SearchOutcome::Finished, 3, 3)));
    }

    #[test]
    fn a_search_that_is_no_longer_wanted_stops_at_the_next_page() {
        let control = Control::default();
        let id = RequestId(6);
        control.begin(id);
        let out = Mutex::new(Vec::new());
        let mut read = Vec::new();
        let sent = search(id, &query("x"), 100, &control, &out, |page| {
            read.push(page);
            if page == 2 {
                control.begin(RequestId(7)); // a newer search
            }
            Ok::<_, String>(chars("x"))
        });
        assert!(sent);
        assert_eq!(read, [0, 1, 2], "no page is read after the change");
        let wire = out.into_inner().unwrap();
        let mut cursor = std::io::Cursor::new(wire);
        let mut last = None;
        while let Some(response) = vellora_ipc::read_frame::<_, Response>(&mut cursor).unwrap() {
            last = Some(response);
        }
        assert_eq!(
            last,
            Some(Response::SearchDone {
                req_id: id,
                outcome: SearchOutcome::Cancelled,
                hits: 3,
                pages_done: 3
            })
        );
    }

    #[test]
    fn a_bad_expression_is_answered_with_an_error() {
        let control = Control::default();
        let id = RequestId(8);
        control.begin(id);
        let out = Mutex::new(Vec::new());
        let bad = SearchQuery {
            regex: true,
            ..query("(")
        };
        assert!(search(id, &bad, 1, &control, &out, |_| Ok::<_, String>(
            chars("x")
        )));
        let wire = out.into_inner().unwrap();
        let mut cursor = std::io::Cursor::new(wire);
        let response = vellora_ipc::read_frame::<_, Response>(&mut cursor)
            .unwrap()
            .unwrap();
        assert!(matches!(
            response,
            Response::Error {
                req_id: Some(RequestId(8)),
                kind: ErrorKind::InvalidRequest,
                ..
            }
        ));
    }
}
