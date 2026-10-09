//! The document outline, or bookmarks (ISO 32000-2:2020 §12.3.3), read a level at a time.
//!
//! The outline is a tree of dictionaries linked the way a file system's directories are: the
//! catalog's `/Outlines` has `/First` and `/Last` items, every item has `/Next` and `/Prev`
//! siblings, `/Parent`, and its own `/First` and `/Last` children. A document can have tens of
//! thousands of items, so nothing here reads the whole tree: [`Outline::children`] returns one
//! page of one level, and the caller keeps going from the last item it got.
//!
//! The tree is hostile input. Siblings are followed with a visited set, so a `/Next` that loops
//! ends the level; how deep a level is comes from its `/Parent` chain, which is cut at
//! [`Limits::max_nesting_depth`](crate::limits::Limits::max_nesting_depth), so a `/First` that
//! points back at an ancestor stops expanding; and one level never yields more than
//! [`Limits::max_tree_nodes`](crate::limits::Limits::max_tree_nodes) items.

use std::collections::HashSet;

use crate::dest::{Destination, DestinationResolver};
use crate::error::Result;
use crate::object::{Dict, ObjRef, ObjectKind};
use crate::textstring::{decode_text_string, truncated};

/// Titles are cut to this many characters.
pub const MAX_TITLE_CHARS: usize = 1024;

/// One item of the outline.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    /// The item's object number, which names it in later requests ([`Outline::children`]).
    pub id: u32,
    /// The title, as plain text; empty if the item has none.
    pub title: String,
    /// Where the item goes; `None` for an item without a destination, or whose destination is not
    /// a page of this document or is another kind of action.
    pub destination: Option<Destination>,
    /// Whether the item has children (`/First`), and the nesting allows to show them.
    pub has_children: bool,
    /// Whether the children are shown when the document is opened (`/Count` greater than zero).
    pub open: bool,
    /// How the title is written (`/F`).
    pub style: TitleStyle,
}

/// The flags of an item's title (`/F`, Table 151).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TitleStyle {
    /// Bold (bit 2).
    pub bold: bool,
    /// Italic (bit 1).
    pub italic: bool,
}

/// One page of one level of the outline.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OutlinePage {
    /// The items, in order.
    pub items: Vec<OutlineItem>,
    /// Whether the level goes on after the last item (ask again with `after` set to it).
    pub more: bool,
}

/// Reads the outline of a document.
#[derive(Debug)]
pub struct Outline<'r, 's, 'a> {
    resolver: &'r DestinationResolver<'s, 'a>,
}

fn reference(dict: &Dict<'_>, key: &[u8]) -> Option<ObjRef> {
    match dict.get(key)?.kind {
        ObjectKind::Ref(reference) => Some(reference),
        _ => None,
    }
}

impl<'r, 's, 'a> Outline<'r, 's, 'a> {
    /// An outline reader over the document `resolver` reads.
    #[must_use]
    pub fn new(resolver: &'r DestinationResolver<'s, 'a>) -> Self {
        Self { resolver }
    }

    /// Up to `limit` items of the level below `parent` (the top level for `None`), starting after
    /// the item `after` (from the first for `None`). `already` is how many items of this level the
    /// caller has seen, which bounds a level whose `/Next` links go round in a circle.
    ///
    /// A `parent` or `after` that is not an outline item gives an empty page.
    ///
    /// # Errors
    ///
    /// The store's errors for an object that cannot be read, and
    /// [`Error::LimitExceeded`](crate::Error::LimitExceeded) when a limit is hit while resolving.
    pub fn children(
        &self,
        parent: Option<u32>,
        after: Option<u32>,
        already: u32,
        limit: usize,
    ) -> Result<OutlinePage> {
        let store = self.resolver.store();
        let max_items = store.limits().max_tree_nodes;
        // The depth of the items of this level: the top level is 1.
        let level = match parent {
            None => 1,
            Some(parent) => self.depth(parent)? + 1,
        };
        let mut next = match after {
            Some(id) => {
                let item = store.resolve(ObjRef::new(id, 0))?;
                item.as_dict().and_then(|dict| reference(dict, b"Next"))
            }
            None => self.first_child(parent)?,
        };
        let mut seen: HashSet<u32> = HashSet::new();
        if let Some(id) = after {
            seen.insert(id);
        }
        let mut page = OutlinePage::default();
        while let Some(current) = next {
            if page.items.len() >= limit {
                page.more = true; // a sibling is still to come
                break;
            }
            let delivered =
                u64::from(already) + u64::try_from(page.items.len()).unwrap_or(u64::MAX);
            if delivered >= max_items || !seen.insert(current.num) {
                break; // a level that is too long, or a loop of /Next links
            }
            let object = store.resolve(current)?;
            let Some(dict) = object.as_dict() else {
                break;
            };
            page.items.push(self.item(current.num, dict, level)?);
            next = reference(dict, b"Next");
        }
        Ok(page)
    }

    /// The first item below `parent`, or of the top level.
    fn first_child(&self, parent: Option<u32>) -> Result<Option<ObjRef>> {
        let store = self.resolver.store();
        let first_of = |dict: &Dict<'_>| reference(dict, b"First");
        match parent {
            None => {
                let Some(outlines) = self.resolver.catalog_entry(b"Outlines")? else {
                    return Ok(None);
                };
                Ok(outlines.as_dict().and_then(first_of))
            }
            Some(id) => {
                if self.depth(id)? >= u64::from(store.limits().max_nesting_depth) {
                    return Ok(None);
                }
                let object = store.resolve(ObjRef::new(id, 0))?;
                Ok(object.as_dict().and_then(first_of))
            }
        }
    }

    /// How many levels deep the item `id` is, counted by following `/Parent` (at most as many
    /// steps as the nesting limit allows, and never round a loop twice).
    fn depth(&self, id: u32) -> Result<u64> {
        let store = self.resolver.store();
        let limit = u64::from(store.limits().max_nesting_depth);
        let mut seen = HashSet::from([id]);
        let mut current = id;
        let mut depth = 1;
        while depth <= limit {
            let object = store.resolve(ObjRef::new(current, 0))?;
            let Some(parent) = object.as_dict().and_then(|dict| reference(dict, b"Parent")) else {
                return Ok(depth);
            };
            if !seen.insert(parent.num) {
                // A loop: as deep as the limit allows, so that it is not expanded further.
                return Ok(limit);
            }
            current = parent.num;
            depth += 1;
        }
        // Deeper than the limit allows: not expanded further.
        Ok(limit)
    }

    /// The item `id` at depth `level`.
    fn item(&self, id: u32, dict: &Dict<'_>, level: u64) -> Result<OutlineItem> {
        let store = self.resolver.store();
        let title = match dict.get(b"Title") {
            Some(title) => match &store.deref(title)?.kind {
                ObjectKind::String(bytes) => truncated(decode_text_string(bytes), MAX_TITLE_CHARS),
                _ => String::new(),
            },
            None => String::new(),
        };
        // A destination that cannot be resolved is no destination, not a failed item.
        let destination = match (dict.get(b"Dest"), dict.get(b"A")) {
            (Some(dest), _) => self.resolver.resolve(dest),
            (None, Some(action)) => self.resolver.resolve_action(action),
            (None, None) => Ok(None),
        }
        .unwrap_or(None);
        let count = match dict.get(b"Count") {
            Some(count) => store.deref(count)?.as_integer().unwrap_or(0),
            None => 0,
        };
        let flags = match dict.get(b"F") {
            Some(flags) => store.deref(flags)?.as_integer().unwrap_or(0),
            None => 0,
        };
        // Items nested as deep as the limit allows have no children to show.
        let has_children = reference(dict, b"First").is_some()
            && level < u64::from(store.limits().max_nesting_depth);
        Ok(OutlineItem {
            id,
            title,
            destination,
            has_children,
            open: count > 0 && has_children,
            style: TitleStyle {
                bold: flags & 2 != 0,
                italic: flags & 1 != 0,
            },
        })
    }
}
