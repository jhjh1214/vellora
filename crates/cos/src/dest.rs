//! Destinations (ISO 32000-2:2020 §12.3.2): where a link, an outline item or a named destination
//! takes the reader.
//!
//! An explicit destination is an array `[page /Fit operands...]`. The page is a reference to a page
//! object, which this module turns into the page's index in the page tree ([`PageIndex`]); a page
//! given as an integer (which is only valid in a remote destination, but is seen in files) is the
//! index itself. A destination may also be named, by a name (the catalog's `/Dests` dictionary) or
//! a string (the `/Names` → `/Dests` name tree), or be a dictionary with a `/D` entry; or be the
//! `/D` of a `GoTo` action. All of these resolve here.
//!
//! Resolution is lenient and bounded. Whatever cannot be resolved is `None`, not an error, so that
//! one bad destination does not hide the rest of an outline; only a limit (a name tree with too
//! many nodes) is reported as an error.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::Result;
use crate::object::{ObjRef, Object, ObjectKind};
use crate::store::ObjectStore;
use crate::trees::name_tree_lookup;

/// Pages indexed for destinations. A document with more has the destinations to its later pages
/// unresolved.
pub const MAX_INDEXED_PAGES: usize = 1 << 20;

/// How many indirections (a named destination whose value is a dictionary whose `/D` is a
/// destination ...) are followed.
const MAX_INDIRECTIONS: u8 = 4;

/// How a page is shown when a destination is followed (Table 151).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fit {
    /// `/XYZ left top zoom`: the point (`left`, `top`) of the page at the top left of the window,
    /// at `zoom`. A missing operand (`null`, or a zoom of 0) leaves that value as it is.
    Xyz {
        /// Left edge, in default user space units.
        left: Option<f32>,
        /// Top edge, in default user space units.
        top: Option<f32>,
        /// Magnification, 1.0 for 100%.
        zoom: Option<f32>,
    },
    /// `/Fit`: the whole page in the window.
    Fit,
    /// `/FitH top`: the page's width in the window, `top` at the top.
    FitH {
        /// Top edge, in default user space units.
        top: Option<f32>,
    },
    /// `/FitV left`: the page's height in the window, `left` at the left.
    FitV {
        /// Left edge, in default user space units.
        left: Option<f32>,
    },
    /// `/FitR left bottom right top`: the rectangle in the window.
    FitR {
        /// Left edge.
        left: f32,
        /// Bottom edge.
        bottom: f32,
        /// Right edge.
        right: f32,
        /// Top edge.
        top: f32,
    },
    /// `/FitB`: the page's bounding box in the window.
    FitB,
    /// `/FitBH top`: the bounding box's width in the window.
    FitBH {
        /// Top edge, in default user space units.
        top: Option<f32>,
    },
    /// `/FitBV left`: the bounding box's height in the window.
    FitBV {
        /// Left edge, in default user space units.
        left: Option<f32>,
    },
}

/// A resolved destination: a page of this document and how to show it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Destination {
    /// Zero-based index of the page in the page tree.
    pub page: u32,
    /// How to show it.
    pub fit: Fit,
}

/// The page tree's pages by object number, so that a reference to a page object becomes its index.
#[derive(Debug, Default)]
pub struct PageIndex {
    by_object: HashMap<u32, u32>,
}

impl PageIndex {
    /// Walks the page tree (see [`crate::pages`]). Pages beyond [`MAX_INDEXED_PAGES`] are not
    /// indexed. A page that cannot be read still takes no index, as in the page walk.
    #[must_use]
    pub fn build(store: &ObjectStore<'_>) -> Self {
        let mut by_object = HashMap::new();
        for (index, page) in store
            .pages()
            .filter_map(std::result::Result::ok)
            .take(MAX_INDEXED_PAGES)
            .enumerate()
        {
            // `take` keeps the index below 2^20.
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            by_object.entry(page.reference.num).or_insert(index);
        }
        Self { by_object }
    }

    /// The index of the page object `reference`, if it is a page of the document.
    #[must_use]
    pub fn index_of(&self, reference: ObjRef) -> Option<u32> {
        self.by_object.get(&reference.num).copied()
    }

    /// Number of pages indexed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_object.len()
    }

    /// No pages indexed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_object.is_empty()
    }
}

/// A finite number, as `f32`; `None` for anything else (`null`, a name, infinity).
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn number(object: &Object<'_>) -> Option<f32> {
    let value = match object.kind {
        ObjectKind::Integer(n) => n as f64,
        ObjectKind::Real(r) => r,
        _ => return None,
    };
    let value = value as f32;
    value.is_finite().then_some(value)
}

/// The fit operands of an explicit destination array, after the page.
fn fit_of(items: &[Object<'_>]) -> Option<Fit> {
    let ObjectKind::Name(name) = &items.first()?.kind else {
        return None;
    };
    let operand = |at: usize| items.get(at + 1).and_then(number);
    Some(match name.as_ref() {
        b"XYZ" => Fit::Xyz {
            left: operand(0),
            top: operand(1),
            zoom: operand(2).filter(|zoom| *zoom > 0.0),
        },
        b"Fit" => Fit::Fit,
        b"FitH" => Fit::FitH { top: operand(0) },
        b"FitV" => Fit::FitV { left: operand(0) },
        b"FitR" => match (operand(0), operand(1), operand(2), operand(3)) {
            (Some(left), Some(bottom), Some(right), Some(top)) => Fit::FitR {
                left,
                bottom,
                right,
                top,
            },
            // A rectangle with missing sides is no rectangle: show the page.
            _ => Fit::Fit,
        },
        b"FitB" => Fit::FitB,
        b"FitBH" => Fit::FitBH { top: operand(0) },
        b"FitBV" => Fit::FitBV { left: operand(0) },
        _ => return None,
    })
}

/// Resolves destinations of one document. Cheap to create; the page index is built on first use.
pub struct DestinationResolver<'s, 'a> {
    store: &'s ObjectStore<'a>,
    pages: std::sync::OnceLock<PageIndex>,
}

impl<'s, 'a> DestinationResolver<'s, 'a> {
    /// A resolver over `store`.
    #[must_use]
    pub fn new(store: &'s ObjectStore<'a>) -> Self {
        Self {
            store,
            pages: std::sync::OnceLock::new(),
        }
    }

    /// The page index, built by the first call.
    pub fn pages(&self) -> &PageIndex {
        self.pages.get_or_init(|| PageIndex::build(self.store))
    }

    /// The destination `object` stands for: an explicit array, a name or a string naming one, or a
    /// dictionary with a `/D` entry. `None` if there is none or it cannot be resolved.
    ///
    /// # Errors
    ///
    /// Only the limits: [`Error::LimitExceeded`](crate::Error::LimitExceeded) for a name tree with
    /// too many nodes.
    pub fn resolve(&self, object: &Object<'_>) -> Result<Option<Destination>> {
        self.resolve_at(object, MAX_INDIRECTIONS)
    }

    /// The destination of an action dictionary: the `/D` of a `GoTo` action, `None` for any other
    /// kind of action.
    ///
    /// # Errors
    ///
    /// As [`resolve`](Self::resolve).
    pub fn resolve_action(&self, action: &Object<'_>) -> Result<Option<Destination>> {
        let action = self.store.deref(action)?;
        let Some(dict) = action.as_dict() else {
            return Ok(None);
        };
        match dict.get(b"S").map(|kind| &kind.kind) {
            Some(ObjectKind::Name(kind)) if kind.as_ref() == b"GoTo" => {}
            _ => return Ok(None),
        }
        match dict.get(b"D") {
            Some(destination) => self.resolve(destination),
            None => Ok(None),
        }
    }

    /// The destination of the name `name`: the catalog's `/Dests` dictionary first, then the name
    /// tree.
    ///
    /// # Errors
    ///
    /// As [`resolve`](Self::resolve).
    pub fn resolve_name(&self, name: &[u8]) -> Result<Option<Destination>> {
        self.named(name, true, MAX_INDIRECTIONS)
    }

    fn resolve_at(&self, object: &Object<'_>, left: u8) -> Result<Option<Destination>> {
        if left == 0 {
            return Ok(None);
        }
        let object = self.store.deref(object)?;
        match &object.kind {
            ObjectKind::Array(items) => Ok(self.explicit(items)),
            ObjectKind::Dict(dict) => match dict.get(b"D") {
                Some(inner) => self.resolve_at(inner, left - 1),
                None => Ok(None),
            },
            ObjectKind::Name(name) => self.named(name, true, left - 1),
            ObjectKind::String(name) => self.named(name, false, left - 1),
            _ => Ok(None),
        }
    }

    /// A named destination. A name is looked up in `/Dests` first and a string in the name tree
    /// first; each falls back to the other place, as files mix them up.
    fn named(&self, key: &[u8], name: bool, left: u8) -> Result<Option<Destination>> {
        if left == 0 {
            return Ok(None);
        }
        let Some(catalog) = self.store.root()? else {
            return Ok(None);
        };
        let Some(catalog) = catalog.as_dict() else {
            return Ok(None);
        };
        let from_dict = || -> Result<Option<Object<'static>>> {
            let Some(dests) = catalog.get(b"Dests") else {
                return Ok(None);
            };
            let dests = self.store.deref(dests)?;
            Ok(dests
                .as_dict()
                .and_then(|dests| dests.get(key))
                .map(|value| value.clone().into_owned()))
        };
        let from_tree = || -> Result<Option<Object<'static>>> {
            let Some(names) = catalog.get(b"Names") else {
                return Ok(None);
            };
            let names = self.store.deref(names)?;
            let Some(tree) = names.as_dict().and_then(|names| names.get(b"Dests")) else {
                return Ok(None);
            };
            name_tree_lookup(self.store, tree, key)
        };
        let value = if name {
            from_dict()?.map_or_else(from_tree, |value| Ok(Some(value)))?
        } else {
            from_tree()?.map_or_else(from_dict, |value| Ok(Some(value)))?
        };
        match value {
            // The value of a named destination is an array or a dictionary, never another name.
            Some(value) if !matches!(value.kind, ObjectKind::Name(_) | ObjectKind::String(_)) => {
                self.resolve_at(&value, left)
            }
            _ => Ok(None),
        }
    }

    fn explicit(&self, items: &[Object<'_>]) -> Option<Destination> {
        let first = items.first()?;
        let page = match first.kind {
            ObjectKind::Ref(reference) => self.pages().index_of(reference)?,
            // A page number instead of a page object.
            ObjectKind::Integer(index) => {
                let index = u32::try_from(index).ok()?;
                // Only pages that exist.
                if usize::try_from(index).ok()? >= self.pages().len() {
                    return None;
                }
                index
            }
            _ => return None,
        };
        Some(Destination {
            page,
            fit: fit_of(&items[1..])?,
        })
    }

    /// The store the resolver reads.
    #[must_use]
    pub fn store(&self) -> &'s ObjectStore<'a> {
        self.store
    }

    /// Resolves the catalog entry `key` (a direct or indirect object), if there is one.
    ///
    /// # Errors
    ///
    /// As [`ObjectStore::resolve`].
    pub fn catalog_entry(&self, key: &[u8]) -> Result<Option<Arc<Object<'static>>>> {
        let Some(catalog) = self.store.root()? else {
            return Ok(None);
        };
        let Some(entry) = catalog.as_dict().and_then(|catalog| catalog.get(key)) else {
            return Ok(None);
        };
        let entry = self.store.deref(entry)?;
        Ok(match entry.kind {
            ObjectKind::Null => None,
            _ => Some(entry),
        })
    }
}

impl std::fmt::Debug for DestinationResolver<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DestinationResolver")
            .field("indexed", &self.pages.get().map(PageIndex::len))
            .finish_non_exhaustive()
    }
}
