//! Link annotations (ISO 32000-2:2020 §12.5.6.5) and the actions they carry (§12.6), read for one
//! page at a time.
//!
//! A link is a `/Subtype /Link` annotation in the page's `/Annots` with a `/Rect` and either a
//! destination (`/Dest`) or an action (`/A`). What this module returns is plain data for the viewer
//! to show and, for internal jumps, follow:
//!
//! - [`LinkAction::GoTo`]: a destination in this document, resolved to a page and a fit;
//! - [`LinkAction::Uri`]: a web address, as text. **Nothing here opens it**; the viewer asks first;
//! - [`LinkAction::Named`]: next, previous, first or last page;
//! - [`LinkAction::Inert`]: every other kind of action (`GoToR`, `Launch`, `JavaScript`,
//!   `SubmitForm`, `ImportData`, ...), by name only. The viewer shows what the link would do and
//!   never does it (CLAUDE.md invariant 8).
//!
//! The rectangle is given in the page **as shown**: points, origin at the top left, after the
//! page's `/CropBox` (else `/MediaBox`) and its `/Rotate`, which is the frame the renderer's page
//! size is measured in.
//!
//! Everything is hostile input: a page lists at most [`MAX_ANNOTATIONS_SCANNED`] annotations, a URI
//! longer than [`MAX_URI_BYTES`] is not offered as one (it would be shown cut), and what cannot be
//! read is skipped rather than failing the page.

use std::sync::Arc;

use crate::dest::{Destination, DestinationResolver, number};
use crate::error::Result;
use crate::object::{Dict, Object, ObjectKind};
use crate::pages::Page;

/// Annotations looked at on one page; the rest are ignored.
pub const MAX_ANNOTATIONS_SCANNED: usize = 1 << 16;
/// Longest URI returned, in bytes. A longer one cannot be shown whole, so it is not returned as a
/// URI.
pub const MAX_URI_BYTES: usize = 2048;
/// Longest name of an inert action, in bytes.
pub const MAX_KIND_BYTES: usize = 64;

/// The page actions of `/S /Named` (§12.6.4.11) that a viewer can do by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedAction {
    /// `NextPage`.
    NextPage,
    /// `PrevPage`.
    PrevPage,
    /// `FirstPage`.
    FirstPage,
    /// `LastPage`.
    LastPage,
}

/// What activating a link does.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkAction {
    /// Go to a place in this document.
    GoTo(Destination),
    /// A `GoTo` whose destination is not a page of this document (or cannot be resolved): the
    /// link goes nowhere.
    Unresolved,
    /// Open a web address, after asking. The text is the document's, not checked beyond its length.
    Uri(String),
    /// A page action of the viewer.
    Named(NamedAction),
    /// An action Vellora never runs, by its name (`/S`, or `Named:<name>` for other named actions).
    Inert(String),
}

/// One link on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// `[left, top, right, bottom]` in points of the page as shown, top left origin.
    pub rect: [f32; 4],
    /// What it does.
    pub action: LinkAction,
}

/// A page's links, a window of them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LinkPage {
    /// The links from the `skip`th, in the order of the page's `/Annots`.
    pub links: Vec<Link>,
    /// More links follow the last one returned.
    pub more: bool,
}

/// The frame a page is shown in: its box and turn.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Frame {
    left: f64,
    bottom: f64,
    right: f64,
    top: f64,
    /// Quarter turns clockwise, 0..=3.
    turns: u8,
}

impl Frame {
    /// A box of four numbers, normalised so that `left < right` and `bottom < top`; `None` if it
    /// is not four finite numbers or is empty.
    fn box_of(object: &Object<'_>) -> Option<[f64; 4]> {
        let ObjectKind::Array(items) = &object.kind else {
            return None;
        };
        let [a, b, c, d] = items.as_slice() else {
            return None;
        };
        let [a, b, c, d] = [a, b, c, d].map(|n| number(n).map(f64::from));
        let (a, b, c, d) = (a?, b?, c?, d?);
        let (left, right) = (a.min(c), a.max(c));
        let (bottom, top) = (b.min(d), b.max(d));
        (right > left && top > bottom).then_some([left, bottom, right, top])
    }

    fn of(page: &Page) -> Self {
        let pick =
            |object: &Option<Arc<Object<'static>>>| object.as_deref().and_then(Frame::box_of);
        // US Letter if the page has no usable box: the same fallback as the renderer's reading.
        let [left, bottom, right, top] = pick(&page.inherited.crop_box)
            .or_else(|| pick(&page.inherited.media_box))
            .unwrap_or([0.0, 0.0, 612.0, 792.0]);
        let degrees = page
            .inherited
            .rotate
            .as_deref()
            .and_then(Object::as_integer)
            .unwrap_or(0);
        let turns = u8::try_from(degrees.div_euclid(90).rem_euclid(4)).unwrap_or(0);
        // Only multiples of 90 turn the page.
        let turns = if degrees.rem_euclid(90) == 0 {
            turns
        } else {
            0
        };
        Self {
            left,
            bottom,
            right,
            top,
            turns,
        }
    }

    /// A point of user space, in the page as shown.
    fn point(&self, x: f64, y: f64) -> (f64, f64) {
        let (width, height) = (self.right - self.left, self.top - self.bottom);
        // Upright: origin at the top left of the box.
        let (u, v) = (x - self.left, self.top - y);
        match self.turns {
            0 => (u, v),
            1 => (height - v, u),
            2 => (width - u, height - v),
            _ => (v, width - u),
        }
    }

    /// A rectangle of user space (any two opposite corners), `[left, top, right, bottom]` in the
    /// page as shown.
    #[allow(clippy::cast_possible_truncation)]
    fn rect(&self, corners: [f64; 4]) -> [f32; 4] {
        let (x0, y0) = self.point(corners[0], corners[1]);
        let (x1, y1) = self.point(corners[2], corners[3]);
        [
            x0.min(x1) as f32,
            y0.min(y1) as f32,
            x0.max(x1) as f32,
            y0.max(y1) as f32,
        ]
    }
}

/// The name in `/S` of an action dictionary, cut to [`MAX_KIND_BYTES`].
fn kind_of(dict: &Dict<'_>) -> Option<String> {
    match dict.get(b"S").map(|s| &s.kind)? {
        ObjectKind::Name(name) => {
            let end = name.len().min(MAX_KIND_BYTES);
            Some(String::from_utf8_lossy(&name[..end]).into_owned())
        }
        _ => None,
    }
}

/// What an action dictionary does.
fn classify(resolver: &DestinationResolver<'_, '_>, action: &Object<'_>) -> Result<LinkAction> {
    let store = resolver.store();
    let action = store.deref(action)?;
    let Some(dict) = action.as_dict() else {
        return Ok(LinkAction::Unresolved);
    };
    let Some(kind) = kind_of(dict) else {
        return Ok(LinkAction::Unresolved);
    };
    Ok(match kind.as_str() {
        "GoTo" => resolver
            .resolve_action(&action)?
            .map_or(LinkAction::Unresolved, LinkAction::GoTo),
        "URI" => match dict.get(b"URI") {
            Some(uri) => match &store.deref(uri)?.kind {
                ObjectKind::String(bytes) if bytes.len() <= MAX_URI_BYTES => {
                    // A URI is 7-bit ASCII; bytes beyond that are shown as they decode.
                    LinkAction::Uri(String::from_utf8_lossy(bytes).into_owned())
                }
                _ => LinkAction::Inert("URI".to_owned()),
            },
            None => LinkAction::Inert("URI".to_owned()),
        },
        "Named" => match dict.get(b"N") {
            Some(name) => match &store.deref(name)?.kind {
                ObjectKind::Name(name) => match name.as_ref() {
                    b"NextPage" => LinkAction::Named(NamedAction::NextPage),
                    b"PrevPage" => LinkAction::Named(NamedAction::PrevPage),
                    b"FirstPage" => LinkAction::Named(NamedAction::FirstPage),
                    b"LastPage" => LinkAction::Named(NamedAction::LastPage),
                    other => {
                        let end = other.len().min(MAX_KIND_BYTES - 6);
                        LinkAction::Inert(format!(
                            "Named:{}",
                            String::from_utf8_lossy(&other[..end])
                        ))
                    }
                },
                _ => LinkAction::Inert("Named".to_owned()),
            },
            None => LinkAction::Inert("Named".to_owned()),
        },
        _ => LinkAction::Inert(kind),
    })
}

/// The links of `page`: at most `limit` of them after skipping the first `skip`, with whether more
/// follow.
///
/// # Errors
///
/// The store's errors for the `/Annots` entry; an annotation that cannot be read is skipped.
pub fn read_links(
    resolver: &DestinationResolver<'_, '_>,
    page: &Page,
    skip: usize,
    limit: usize,
) -> Result<LinkPage> {
    let store = resolver.store();
    let Some(dict) = page.object.as_dict() else {
        return Ok(LinkPage::default());
    };
    let Some(annots) = dict.get(b"Annots") else {
        return Ok(LinkPage::default());
    };
    let annots = store.deref(annots)?;
    let ObjectKind::Array(items) = &annots.kind else {
        return Ok(LinkPage::default());
    };
    let frame = Frame::of(page);
    let mut out = LinkPage::default();
    let mut seen = 0_usize;
    for item in items.iter().take(MAX_ANNOTATIONS_SCANNED) {
        let Ok(annotation) = store.deref(item) else {
            continue;
        };
        let Some(link) = read_link(resolver, &frame, &annotation) else {
            continue;
        };
        if seen >= skip {
            if out.links.len() >= limit {
                out.more = true;
                break;
            }
            out.links.push(link);
        }
        seen += 1;
    }
    Ok(out)
}

/// One annotation as a link, or `None` if it is not a visible link with a usable rectangle.
fn read_link(
    resolver: &DestinationResolver<'_, '_>,
    frame: &Frame,
    annotation: &Object<'_>,
) -> Option<Link> {
    let store = resolver.store();
    let dict = annotation.as_dict()?;
    match &store.deref(dict.get(b"Subtype")?).ok()?.kind {
        ObjectKind::Name(name) if name.as_ref() == b"Link" => {}
        _ => return None,
    }
    // Hidden (bit 2) and NoView (bit 6) annotations are not shown, so not clickable either.
    if let Some(flags) = dict.get(b"F").and_then(|f| store.deref(f).ok()) {
        let flags = flags.as_integer().unwrap_or(0);
        if flags & 0b10 != 0 || flags & 0b10_0000 != 0 {
            return None;
        }
    }
    let rect_object = store.deref(dict.get(b"Rect")?).ok()?;
    let rect = Frame::box_of(&rect_object)?;
    // An action wins over a destination; a link with neither does nothing and is not a link.
    let action = match (dict.get(b"A"), dict.get(b"Dest")) {
        (Some(action), _) => classify(resolver, action).ok()?,
        (None, Some(dest)) => resolver
            .resolve(dest)
            .ok()?
            .map_or(LinkAction::Unresolved, LinkAction::GoTo),
        (None, None) => return None,
    };
    Some(Link {
        rect: frame.rect(rect),
        action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(turns: u8) -> Frame {
        // A box that does not start at the origin, 200 wide and 100 high.
        Frame {
            left: 10.0,
            bottom: 20.0,
            right: 210.0,
            top: 120.0,
            turns,
        }
    }

    #[test]
    fn an_upright_page_has_its_origin_at_the_top_left_of_the_box() {
        let f = frame(0);
        // The box's top left corner is the origin; one point right and down from it is (1, 1).
        assert_eq!(f.point(10.0, 120.0), (0.0, 0.0));
        assert_eq!(f.point(11.0, 119.0), (1.0, 1.0));
        assert_eq!(f.rect([60.0, 70.0, 20.0, 110.0]), [10.0, 10.0, 50.0, 50.0]);
    }

    #[test]
    fn a_turned_page_turns_the_rectangle_with_it() {
        // The top left corner of a 200 x 100 page, 10 points in each way, as a 20 x 20 box.
        let corner = [10.0, 100.0, 30.0, 120.0];
        // Turned a quarter clockwise the page is 100 wide and 200 high and the corner is at the
        // top right; half a turn, at the bottom right; three quarters, at the bottom left.
        assert_eq!(frame(0).rect(corner), [0.0, 0.0, 20.0, 20.0]);
        assert_eq!(frame(1).rect(corner), [80.0, 0.0, 100.0, 20.0]);
        assert_eq!(frame(2).rect(corner), [180.0, 80.0, 200.0, 100.0]);
        assert_eq!(frame(3).rect(corner), [0.0, 180.0, 20.0, 200.0]);
    }

    #[test]
    fn boxes_are_normalised_and_bad_ones_refused() {
        let array = |items: &[ObjectKind<'static>]| {
            Object::new(ObjectKind::Array(
                items.iter().cloned().map(Object::new).collect(),
            ))
        };
        let reversed = array(&[
            ObjectKind::Integer(200),
            ObjectKind::Real(100.5),
            ObjectKind::Integer(0),
            ObjectKind::Integer(0),
        ]);
        assert_eq!(Frame::box_of(&reversed), Some([0.0, 0.0, 200.0, 100.5]));
        let empty = array(&[
            ObjectKind::Integer(5),
            ObjectKind::Integer(5),
            ObjectKind::Integer(5),
            ObjectKind::Integer(9),
        ]);
        assert_eq!(Frame::box_of(&empty), None);
        let short = array(&[ObjectKind::Integer(1), ObjectKind::Integer(2)]);
        assert_eq!(Frame::box_of(&short), None);
        let text = array(&[
            ObjectKind::Integer(0),
            ObjectKind::Integer(0),
            ObjectKind::Name(b"x".as_slice().into()),
            ObjectKind::Integer(9),
        ]);
        assert_eq!(Frame::box_of(&text), None);
    }
}
