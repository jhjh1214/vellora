//! Answering outline, page label and destination requests, on a thread of their own.
//!
//! The reader thread must stay free to take `Cancel`s and queue tiles, and the outline of a
//! hostile file can take a while to read, so these requests are handed to this thread. It owns
//! what answering them needs: a second `cos` view of the document (the object store borrows the
//! file's bytes, so it lives in this thread's frame rather than in the shared `Document`), the
//! labels once they are read, and an index of the whole outline once somebody asks which section
//! a page is in.
//!
//! All of it is untrusted input read through `cos`'s limits. Whatever cannot be read becomes an
//! `Error` answer for that request (`ReadFailed`); the document and the tiles are unaffected.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use vellora_cos::links::read_links;
use vellora_cos::{
    DestinationResolver, Limits, ObjectStore, Outline, OutlineItem, Page, PageLabels,
};
use vellora_ipc::{
    Destination, ErrorKind, Fit, Link, LinkAction, MAX_OUTLINE_PATH, MAX_OUTLINE_TITLE_BYTES,
    NamedAction, OutlineEntry, Request, RequestId, Response, TextChar, TitleStyle,
};
use zeroize::Zeroizing;

use crate::document::Document;
use crate::session::{error_response, send_shared};
use crate::text;

/// Pages whose objects are kept for reading their links; links on later pages are not offered.
const MAX_LINK_PAGES: usize = 1 << 16;
/// Characters read from one page; a page with more is cut there.
const MAX_PAGE_CHARS: usize = 1 << 19;
/// Characters of text kept for the pages read, over all pages; the oldest page goes first.
const TEXT_CACHE_CHARS: usize = 1 << 20;

/// Items asked of `cos` at a time while the whole outline is walked.
const WALK_CHUNK: usize = 256;

/// What the reader thread sends this one.
pub(crate) enum Message {
    /// The document is open; read it from here. Sent once, before any `Ask`.
    Open(OpenDocument),
    /// A navigation request that passed validation and found a document open.
    Ask(Request),
}

/// What reading the document's structure needs.
pub(crate) struct OpenDocument {
    /// The document: its bytes for cos, and PDFium's handle for the text of a page.
    pub(crate) document: Arc<Document>,
    /// The page count PDFium reported, which is what the UI numbers pages by.
    pub(crate) page_count: u32,
    /// The forms of the password the user typed (empty if none), for a document that is locked.
    /// Wiped when dropped, which happens as soon as the store is unlocked.
    pub(crate) unlock: Vec<Zeroizing<Vec<u8>>>,
}

/// The request's correlation id, for those that are navigation requests.
pub(crate) fn request_id(request: &Request) -> Option<RequestId> {
    match request {
        Request::GetOutline { req_id, .. }
        | Request::GetOutlinePath { req_id, .. }
        | Request::GetPageLabels { req_id, .. }
        | Request::FindPageLabel { req_id, .. }
        | Request::GetLinks { req_id, .. }
        | Request::GetTextPage { req_id, .. } => Some(*req_id),
        _ => None,
    }
}

/// The navigation thread's body: waits for the document, then answers requests until the reader
/// drops its end of the channel.
pub(crate) fn run<W: Write>(messages: &Receiver<Message>, output: &Mutex<W>) {
    let opened = loop {
        match messages.recv() {
            Ok(Message::Open(opened)) => break opened,
            // The reader checks a document is open first; this is only a guard.
            Ok(Message::Ask(request)) => {
                if !reply(
                    output,
                    &refusal(&request, ErrorKind::InvalidRequest, &"no document is open"),
                ) {
                    return;
                }
            }
            Err(_) => return,
        }
    };
    let OpenDocument {
        document,
        page_count,
        unlock,
    } = opened;
    let mapped = Arc::clone(&document.mapped);
    let store = open_store(mapped.as_slice(), &unlock);
    drop(unlock);
    match &store {
        Ok(store) => {
            let mut navigator = Navigator::new(store, page_count, document);
            serve(messages, output, |request| navigator.answer(request));
        }
        Err(reason) => {
            tracing::debug!(%reason, "the document structure cannot be read for navigation");
            // The text of a page comes from PDFium, so it can still be answered.
            let mut text = TextCache::new(document, page_count);
            serve(messages, output, |request| match request {
                Request::GetTextPage {
                    req_id,
                    page,
                    skip,
                    limit,
                } => text.page(*req_id, *page, *skip, *limit),
                _ => refusal(request, ErrorKind::ReadFailed, reason),
            });
        }
    }
}

/// Answers each request with `answer`; a panic becomes an `Internal` error for that request.
fn serve<W: Write>(
    messages: &Receiver<Message>,
    output: &Mutex<W>,
    mut answer: impl FnMut(&Request) -> Response,
) {
    while let Ok(message) = messages.recv() {
        let Message::Ask(request) = message else {
            continue; // a second `Open` cannot happen: the reader refuses it
        };
        let response = catch_unwind(AssertUnwindSafe(|| answer(&request))).unwrap_or_else(|_| {
            tracing::error!("a navigation request handler panicked");
            refusal(&request, ErrorKind::Internal, &"internal error")
        });
        if !reply(output, &response) {
            return;
        }
    }
}

/// Sends `response`; `false` when the UI is gone and nothing more can be answered.
fn reply<W: Write>(output: &Mutex<W>, response: &Response) -> bool {
    match send_shared(output, response) {
        Ok(open) => open,
        Err(error) => {
            tracing::error!(%error, "a response could not be sent");
            false
        }
    }
}

fn refusal(request: &Request, kind: ErrorKind, message: &dyn std::fmt::Display) -> Response {
    error_response(request_id(request), kind, message)
}

/// A settled, unlocked `cos` store over the document's bytes.
fn open_store<'a>(
    bytes: &'a [u8],
    unlock: &[Zeroizing<Vec<u8>>],
) -> Result<ObjectStore<'a>, String> {
    let store = ObjectStore::open(bytes, Limits::default()).map_err(|e| e.to_string())?;
    store.settle().map_err(|e| e.to_string())?;
    if store.is_locked() && !unlock.iter().any(|form| store.authenticate(form).is_ok()) {
        return Err("the document cannot be unlocked".to_owned());
    }
    Ok(store)
}

/// One preorder entry of the whole outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Flat {
    id: u32,
    /// Index of the parent in the list, `None` for a top-level item.
    parent: Option<usize>,
    /// The page the item goes to, if it goes to one.
    page: Option<u32>,
}

/// The outline flattened into document order, for finding the section a page is in.
#[derive(Debug, Default)]
struct FlatOutline {
    items: Vec<Flat>,
}

impl FlatOutline {
    /// Walks the whole outline, a level at a time and never twice through the same item, until
    /// the end or `max_items`. An unreadable part ends the walk and the items so far are kept: a
    /// section that is a little off is better than none.
    fn build(outline: &Outline<'_, '_, '_>, page_count: u32, limits: &Limits) -> Self {
        struct Level {
            parent: Option<u32>,
            parent_index: Option<usize>,
            pending: VecDeque<OutlineItem>,
            more: bool,
            last: Option<u32>,
            seen: u32,
        }
        impl Level {
            /// The next chunk of this level.
            fn fetch(&mut self, outline: &Outline<'_, '_, '_>) -> bool {
                let Ok(page) = outline.children(self.parent, self.last, self.seen, WALK_CHUNK)
                else {
                    self.more = false;
                    return false;
                };
                self.seen = self
                    .seen
                    .saturating_add(u32::try_from(page.items.len()).unwrap_or(u32::MAX));
                self.last = page.items.last().map(|item| item.id).or(self.last);
                self.more = page.more;
                self.pending = page.items.into();
                true
            }
        }

        let max_items = usize::try_from(limits.max_tree_nodes).unwrap_or(usize::MAX);
        let max_depth = usize::try_from(limits.max_nesting_depth).unwrap_or(usize::MAX);
        let mut items: Vec<Flat> = Vec::new();
        let mut visited: HashSet<u32> = HashSet::new();
        let mut top = Level {
            parent: None,
            parent_index: None,
            pending: VecDeque::new(),
            more: false,
            last: None,
            seen: 0,
        };
        top.fetch(outline);
        let mut stack = vec![top];
        while let Some(level) = stack.last_mut() {
            if let Some(item) = level.pending.pop_front() {
                if items.len() >= max_items {
                    break;
                }
                if !visited.insert(item.id) {
                    continue; // reached twice: a loop, or two parents sharing a child
                }
                let index = items.len();
                items.push(Flat {
                    id: item.id,
                    parent: level.parent_index,
                    page: item
                        .destination
                        .map(|d| d.page)
                        .filter(|page| *page < page_count),
                });
                if item.has_children && stack.len() < max_depth {
                    let mut child = Level {
                        parent: Some(item.id),
                        parent_index: Some(index),
                        pending: VecDeque::new(),
                        more: false,
                        last: None,
                        seen: 0,
                    };
                    child.fetch(outline);
                    stack.push(child);
                }
            } else if level.more && level.fetch(outline) {
                // The next chunk of the same level is now pending.
            } else {
                stack.pop();
            }
        }
        Self { items }
    }

    /// The ids from a top-level item down to the item that starts the section `page` is in: the
    /// item that goes to the latest page at or before `page`, and of those the last in document
    /// order, which is the deepest when a chapter and its first section start on one page.
    fn path_to(&self, page: u32) -> Vec<u32> {
        let mut best: Option<(u32, usize)> = None;
        for (index, item) in self.items.iter().enumerate() {
            if let Some(target) = item.page
                && target <= page
                && best.is_none_or(|(latest, _)| target >= latest)
            {
                best = Some((target, index));
            }
        }
        let mut path = Vec::new();
        let mut at = best.map(|(_, index)| index);
        while let Some(index) = at {
            let Some(item) = self.items.get(index) else {
                break;
            };
            path.push(item.id);
            if path.len() >= MAX_OUTLINE_PATH {
                break;
            }
            at = item.parent;
        }
        path.reverse();
        path
    }
}

/// The reading state for one document.
struct Navigator<'s, 'a> {
    resolver: DestinationResolver<'s, 'a>,
    page_count: u32,
    /// Read on first use. `Err` is the reason the labels cannot be read, kept so that a hostile
    /// number tree is walked once.
    labels: Option<Result<Option<PageLabels>, String>>,
    flat: Option<FlatOutline>,
    /// The page objects in order, up to `MAX_LINK_PAGES`, read on first use.
    pages: Option<Vec<Page>>,
    text: TextCache,
}

/// The text of the pages read lately. It needs PDFium only, not `cos`, so it works for a file that
/// `cos` cannot read.
struct TextCache {
    document: Arc<Document>,
    page_count: u32,
    /// Oldest first in `order`, at most `TEXT_CACHE_CHARS` characters in all (the newest page is
    /// kept whatever its size).
    texts: HashMap<u32, Arc<Vec<TextChar>>>,
    order: VecDeque<u32>,
    chars: usize,
}

impl TextCache {
    fn new(document: Arc<Document>, page_count: u32) -> Self {
        Self {
            document,
            page_count,
            texts: HashMap::new(),
            order: VecDeque::new(),
            chars: 0,
        }
    }
    /// A window of the characters of `page`: from PDFium once, then from the cache.
    fn page(&mut self, req_id: RequestId, page: u32, skip: u32, limit: u32) -> Response {
        let page_count = self.page_count;
        if page >= page_count {
            return error_response(
                Some(req_id),
                ErrorKind::InvalidRequest,
                &format_args!("page {page} is out of range: the document has {page_count} pages"),
            );
        }
        let text = match self.cached(page) {
            Ok(text) => text,
            Err(error) => return error_response(Some(req_id), ErrorKind::ReadFailed, &error),
        };
        let total = text.len();
        let start = usize::try_from(skip).map_or(total, |skip| skip.min(total));
        let end =
            usize::try_from(limit).map_or(total, |limit| start.saturating_add(limit).min(total));
        Response::TextPage {
            req_id,
            page,
            // `start` and `total` are at most `MAX_PAGE_CHARS`.
            skip: u32::try_from(start).unwrap_or(u32::MAX),
            total: u32::try_from(total).unwrap_or(u32::MAX),
            chars: text[start..end].to_vec(),
        }
    }

    /// The text of `page`, read from PDFium if it is not kept.
    fn cached(&mut self, page: u32) -> Result<Arc<Vec<TextChar>>, vellora_render::Error> {
        if let Some(text) = self.texts.get(&page) {
            return Ok(Arc::clone(text));
        }
        let raw = self
            .document
            .handle
            .page_text(usize::try_from(page).unwrap_or(usize::MAX), MAX_PAGE_CHARS)?;
        let text = Arc::new(text::build(&raw));
        self.chars += text.len();
        self.texts.insert(page, Arc::clone(&text));
        self.order.push_back(page);
        // The oldest pages make room; the page just read always stays.
        while self.chars > TEXT_CACHE_CHARS && self.order.len() > 1 {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(gone) = self.texts.remove(&oldest) {
                self.chars -= gone.len();
            }
        }
        Ok(text)
    }
}

impl<'s, 'a> Navigator<'s, 'a> {
    fn new(store: &'s ObjectStore<'a>, page_count: u32, document: Arc<Document>) -> Self {
        Self {
            resolver: DestinationResolver::new(store),
            page_count,
            labels: None,
            flat: None,
            pages: None,
            text: TextCache::new(document, page_count),
        }
    }

    fn answer(&mut self, request: &Request) -> Response {
        match *request {
            Request::GetOutline {
                req_id,
                parent,
                after,
                already,
                limit,
            } => self.outline(req_id, parent, after, already, limit),
            Request::GetOutlinePath { req_id, page } => {
                let flat = self.flat.get_or_insert_with(|| {
                    FlatOutline::build(
                        &Outline::new(&self.resolver),
                        self.page_count,
                        self.resolver.store().limits(),
                    )
                });
                Response::OutlinePath {
                    req_id,
                    path: flat.path_to(page),
                }
            }
            Request::GetPageLabels {
                req_id,
                first,
                count,
            } => self.page_labels(req_id, first, count),
            Request::GetLinks {
                req_id,
                page,
                skip,
                limit,
            } => self.links(req_id, page, skip, limit),
            Request::GetTextPage {
                req_id,
                page,
                skip,
                limit,
            } => self.text.page(req_id, page, skip, limit),
            Request::FindPageLabel { req_id, ref text } => {
                let page_count = self.page_count;
                match self.labels() {
                    Ok(labels) => Response::PageFound {
                        req_id,
                        page: labels.and_then(|labels| labels.find(text, page_count)),
                    },
                    Err(reason) => error_response(Some(req_id), ErrorKind::ReadFailed, &reason),
                }
            }
            _ => refusal(
                request,
                ErrorKind::InvalidRequest,
                &"not a navigation request",
            ),
        }
    }

    fn outline(
        &self,
        req_id: RequestId,
        parent: Option<u32>,
        after: Option<u32>,
        already: u32,
        limit: u32,
    ) -> Response {
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        match Outline::new(&self.resolver).children(parent, after, already, limit) {
            Ok(page) => Response::Outline {
                req_id,
                items: page
                    .items
                    .into_iter()
                    .map(|item| entry(item, self.page_count))
                    .collect(),
                more: page.more,
            },
            Err(error) => error_response(Some(req_id), ErrorKind::ReadFailed, &error),
        }
    }

    fn page_labels(&mut self, req_id: RequestId, first: u32, count: u32) -> Response {
        let page_count = self.page_count;
        let labels = match self.labels() {
            Ok(labels) => labels,
            Err(reason) => return error_response(Some(req_id), ErrorKind::ReadFailed, &reason),
        };
        let defined = labels.is_some();
        let listed = match labels {
            Some(labels) => labels.window(first, count, page_count),
            None => (first..first.saturating_add(count).min(page_count))
                .map(|page| (u64::from(page) + 1).to_string())
                .collect(),
        };
        Response::PageLabels {
            req_id,
            first,
            defined,
            labels: listed,
        }
    }

    /// A window of the links of `page`. A page the document has no object for (past the cached
    /// ones) has none.
    fn links(&mut self, req_id: RequestId, page: u32, skip: u32, limit: u32) -> Response {
        let page_count = self.page_count;
        if page >= page_count {
            return error_response(
                Some(req_id),
                ErrorKind::InvalidRequest,
                &format_args!("page {page} is out of range: the document has {page_count} pages"),
            );
        }
        let pages = self.pages.get_or_insert_with(|| {
            self.resolver
                .store()
                .pages()
                .filter_map(Result::ok)
                .take(MAX_LINK_PAGES)
                .collect()
        });
        let Some(object) = usize::try_from(page).ok().and_then(|at| pages.get(at)) else {
            return Response::Links {
                req_id,
                page,
                links: Vec::new(),
                more: false,
            };
        };
        let skip = usize::try_from(skip).unwrap_or(usize::MAX);
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        match read_links(&self.resolver, object, skip, limit) {
            Ok(read) => Response::Links {
                req_id,
                page,
                links: read
                    .links
                    .into_iter()
                    .filter_map(|link| link_of(link, page_count))
                    .collect(),
                more: read.more,
            },
            Err(error) => error_response(Some(req_id), ErrorKind::ReadFailed, &error),
        }
    }

    /// The labels, read on first use.
    fn labels(&mut self) -> Result<Option<&PageLabels>, String> {
        let read = self
            .labels
            .get_or_insert_with(|| PageLabels::read(&self.resolver).map_err(|e| e.to_string()));
        match read {
            Ok(labels) => Ok(labels.as_ref()),
            Err(reason) => Err(reason.clone()),
        }
    }
}

/// A link as the protocol carries it. A jump to a page the document does not have (PDFium and
/// `cos` can disagree about a damaged page tree) goes nowhere; a rectangle that is not finite
/// cannot be placed, so the link is dropped.
fn link_of(link: vellora_cos::Link, page_count: u32) -> Option<Link> {
    if !link.rect.iter().all(|v| v.is_finite()) {
        return None;
    }
    let action = match link.action {
        vellora_cos::LinkAction::GoTo(d) if d.page < page_count => LinkAction::GoTo(Destination {
            page: d.page,
            fit: fit(d.fit),
        }),
        vellora_cos::LinkAction::GoTo(_) | vellora_cos::LinkAction::Unresolved => {
            LinkAction::Unresolved
        }
        vellora_cos::LinkAction::Uri(uri) => LinkAction::Uri(uri),
        vellora_cos::LinkAction::Named(named) => LinkAction::Named(match named {
            vellora_cos::NamedAction::NextPage => NamedAction::NextPage,
            vellora_cos::NamedAction::PrevPage => NamedAction::PrevPage,
            vellora_cos::NamedAction::FirstPage => NamedAction::FirstPage,
            vellora_cos::NamedAction::LastPage => NamedAction::LastPage,
        }),
        vellora_cos::LinkAction::Inert(kind) => LinkAction::Inert(kind),
    };
    Some(Link {
        rect: link.rect,
        action,
    })
}

/// An outline item as the protocol carries it. A destination to a page the document does not
/// have (PDFium and `cos` can disagree about a damaged page tree) is no destination.
fn entry(item: OutlineItem, page_count: u32) -> OutlineEntry {
    OutlineEntry {
        id: item.id,
        title: truncated(item.title, MAX_OUTLINE_TITLE_BYTES),
        destination: item
            .destination
            .filter(|d| d.page < page_count)
            .map(|d| Destination {
                page: d.page,
                fit: fit(d.fit),
            }),
        has_children: item.has_children,
        open: item.open,
        style: TitleStyle {
            bold: item.style.bold,
            italic: item.style.italic,
        },
    }
}

fn fit(fit: vellora_cos::Fit) -> Fit {
    use vellora_cos::Fit as C;
    match fit {
        C::Xyz { left, top, zoom } => Fit::Xyz { left, top, zoom },
        C::Fit => Fit::Fit,
        C::FitH { top } => Fit::FitH { top },
        C::FitV { left } => Fit::FitV { left },
        C::FitR {
            left,
            bottom,
            right,
            top,
        } => Fit::FitR {
            left,
            bottom,
            right,
            top,
        },
        C::FitB => Fit::FitB,
        C::FitBH { top } => Fit::FitBH { top },
        C::FitBV { left } => Fit::FitBV { left },
    }
}

/// `text` cut to at most `max` bytes at a character boundary.
fn truncated(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_cut_on_a_character_boundary() {
        let cut = truncated(
            "é".repeat(MAX_OUTLINE_TITLE_BYTES),
            MAX_OUTLINE_TITLE_BYTES - 1,
        );
        assert_eq!(cut.len(), MAX_OUTLINE_TITLE_BYTES - 2);
        assert!(cut.chars().all(|c| c == 'é'));
        assert_eq!(truncated("abc".to_owned(), 3), "abc");
    }

    fn flat(entries: &[(u32, Option<usize>, Option<u32>)]) -> FlatOutline {
        FlatOutline {
            items: entries
                .iter()
                .map(|&(id, parent, page)| Flat { id, parent, page })
                .collect(),
        }
    }

    #[test]
    fn the_section_is_the_latest_start_at_or_before_the_page_and_the_deepest_on_a_tie() {
        // Chapter 1 (page 0) > 1.1 (page 0), 1.2 (page 5); chapter 2 (page 10) > 2.1 (page 10).
        let outline = flat(&[
            (1, None, Some(0)),
            (2, Some(0), Some(0)),
            (3, Some(0), Some(5)),
            (4, None, Some(10)),
            (5, Some(3), Some(10)),
        ]);
        assert_eq!(outline.path_to(0), [1, 2]);
        assert_eq!(outline.path_to(4), [1, 2]);
        assert_eq!(outline.path_to(5), [1, 3]);
        assert_eq!(outline.path_to(9), [1, 3]);
        assert_eq!(outline.path_to(10), [4, 5]);
        assert_eq!(outline.path_to(500), [4, 5]);
    }

    #[test]
    fn items_without_a_page_and_pages_before_the_first_item_have_no_section() {
        let outline = flat(&[(1, None, None), (2, None, Some(3)), (3, Some(1), None)]);
        assert_eq!(outline.path_to(2), Vec::<u32>::new());
        assert_eq!(outline.path_to(3), [2]);
        assert_eq!(FlatOutline::default().path_to(0), Vec::<u32>::new());
    }

    #[test]
    fn an_unsorted_outline_still_finds_the_closest_start() {
        let outline = flat(&[(1, None, Some(8)), (2, None, Some(2)), (3, None, Some(5))]);
        assert_eq!(outline.path_to(6), [3]);
        assert_eq!(outline.path_to(9), [1]);
        assert_eq!(outline.path_to(2), [2]);
    }

    #[test]
    fn a_parent_chain_that_loops_ends_at_the_path_cap() {
        // Not reachable from `build` (parents always come earlier), but the cap must hold.
        let outline = flat(&[(1, Some(0), Some(0))]);
        assert_eq!(outline.path_to(0).len(), MAX_OUTLINE_PATH);
    }
}
