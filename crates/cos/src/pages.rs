//! Lazy page-tree traversal (ISO 32000-2:2020 §7.7.3).
//!
//! [`Pages`] walks the tree from the catalog's `/Pages` in document order, one page per
//! [`Iterator::next`]: asking for the first page of a 10,000-page document reads the catalog, the
//! root node and one page. `/Count` is not used or trusted.
//!
//! - **Inherited attributes** `/MediaBox`, `/CropBox`, `/Rotate` and `/Resources` (§7.7.3.4) are
//!   resolved on the way down: a page's own value wins, then the nearest ancestor's. A `null`
//!   value counts as absent.
//! - **Loops are cut.** Every node is visited at most once, so a cyclic tree (or a page listed
//!   twice) yields each page once and ends. Depth is limited by
//!   [`Limits::max_nesting_depth`](crate::limits::Limits::max_nesting_depth).
//! - **Missing or broken kids are skipped**, as the spec treats a missing object as `null`: a
//!   kid that is not a reference, resolves to `null`, or is not a dictionary yields nothing. A
//!   kid that cannot be parsed yields one `Err` and the walk continues with its siblings.
//! - A node with a `/Kids` array is an intermediate node, whatever its `/Type` says. Any other
//!   dictionary is a page unless its `/Type` is `/Pages` (an empty intermediate node).

use std::collections::HashSet;
use std::sync::Arc;

use crate::error::Result;
use crate::limits::LimitKind;
use crate::object::{Dict, ObjRef, Object, ObjectKind};
use crate::store::ObjectStore;

/// Page attributes a page inherits from its ancestors (§7.7.3.4). Each is `None` when neither the
/// page nor an ancestor sets it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inherited {
    /// `/MediaBox`
    pub media_box: Option<Arc<Object<'static>>>,
    /// `/CropBox`
    pub crop_box: Option<Arc<Object<'static>>>,
    /// `/Rotate`
    pub rotate: Option<Arc<Object<'static>>>,
    /// `/Resources`
    pub resources: Option<Arc<Object<'static>>>,
}

impl Inherited {
    /// `self` overridden by whatever `node` sets itself.
    fn overridden_by(&self, store: &ObjectStore<'_>, node: &Dict<'_>) -> Result<Self> {
        let pick = |key: &[u8], inherited: &Option<Arc<Object<'static>>>| {
            let Some(own) = node.get(key) else {
                return Ok(inherited.clone());
            };
            let value = store.deref(own)?;
            Ok(match value.kind {
                ObjectKind::Null => inherited.clone(),
                _ => Some(value),
            })
        };
        Ok(Self {
            media_box: pick(b"MediaBox", &self.media_box)?,
            crop_box: pick(b"CropBox", &self.crop_box)?,
            rotate: pick(b"Rotate", &self.rotate)?,
            resources: pick(b"Resources", &self.resources)?,
        })
    }
}

/// One page found by [`Pages`].
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// The page object's reference.
    pub reference: ObjRef,
    /// The page dictionary.
    pub object: Arc<Object<'static>>,
    /// Attributes with inheritance applied.
    pub inherited: Inherited,
}

/// An intermediate node whose kids are being walked.
struct Frame {
    node: Arc<Object<'static>>,
    /// Index of the next kid to look at.
    next: usize,
    inherited: Inherited,
}

enum Walk {
    NotStarted,
    Running(Vec<Frame>),
    Done,
}

/// Iterator over the pages of a document; see the [module documentation](self). Create it with
/// [`ObjectStore::pages`].
pub struct Pages<'s, 'a> {
    store: &'s ObjectStore<'a>,
    walk: Walk,
    visited: HashSet<u32>,
}

fn kids<'n>(node: &'n Object<'_>) -> Option<&'n [Object<'n>]> {
    match &node.as_dict()?.get(b"Kids")?.kind {
        ObjectKind::Array(items) => Some(items),
        _ => None,
    }
}

impl<'s, 'a> Pages<'s, 'a> {
    pub(crate) fn new(store: &'s ObjectStore<'a>) -> Self {
        Self {
            store,
            walk: Walk::NotStarted,
            visited: HashSet::new(),
        }
    }

    /// The root `/Pages` node, as the first frame.
    fn start(&mut self) -> Result<Vec<Frame>> {
        let Some(root) = self.store.root()? else {
            return Ok(Vec::new());
        };
        let Some(pages) = root.as_dict().and_then(|catalog| catalog.get(b"Pages")) else {
            return Ok(Vec::new());
        };
        if let ObjectKind::Ref(reference) = pages.kind {
            self.visited.insert(reference.num);
        }
        let node = self.store.deref(pages)?;
        let Some(dict) = node.as_dict().filter(|_| kids(&node).is_some()) else {
            return Ok(Vec::new());
        };
        // The root node can set inheritable attributes too.
        let inherited = Inherited::default().overridden_by(self.store, dict)?;
        Ok(vec![Frame {
            node,
            next: 0,
            inherited,
        }])
    }

    fn advance(&mut self, frames: &mut Vec<Frame>) -> Option<Result<Page>> {
        loop {
            let frame = frames.last_mut()?;
            let node = Arc::clone(&frame.node);
            let Some(kid) = kids(&node).and_then(|k| k.get(frame.next)) else {
                frames.pop();
                continue;
            };
            frame.next += 1;
            let parent = frame.inherited.clone();

            // Direct objects in /Kids are not allowed; a number already seen is a loop or a
            // page listed twice.
            let ObjectKind::Ref(reference) = kid.kind else {
                continue;
            };
            if !self.visited.insert(reference.num) {
                continue;
            }
            let object = match self.store.resolve(reference) {
                Ok(object) => object,
                Err(error) => return Some(Err(error)),
            };
            let Some(dict) = object.as_dict() else {
                continue;
            };
            let inherited = match parent.overridden_by(self.store, dict) {
                Ok(inherited) => inherited,
                Err(error) => return Some(Err(error)),
            };

            if kids(&object).is_some() {
                let depth = frames.len() as u64 + 1;
                let limits = self.store.limits();
                if let Err(error) = limits.check(LimitKind::NestingDepth, depth, None) {
                    return Some(Err(error));
                }
                frames.push(Frame {
                    node: object,
                    next: 0,
                    inherited,
                });
            } else if !matches!(
                dict.get(b"Type").map(|t| &t.kind),
                Some(ObjectKind::Name(name)) if name.as_ref() == b"Pages"
            ) {
                return Some(Ok(Page {
                    reference,
                    object,
                    inherited,
                }));
            }
        }
    }
}

impl Iterator for Pages<'_, '_> {
    type Item = Result<Page>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut frames = match std::mem::replace(&mut self.walk, Walk::Done) {
            Walk::NotStarted => match self.start() {
                Ok(frames) => frames,
                Err(error) => return Some(Err(error)),
            },
            Walk::Running(frames) => frames,
            Walk::Done => return None,
        };
        let item = self.advance(&mut frames);
        if item.is_some() {
            self.walk = Walk::Running(frames);
        }
        item
    }
}

impl std::fmt::Debug for Pages<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pages")
            .field("visited", &self.visited.len())
            .finish_non_exhaustive()
    }
}
