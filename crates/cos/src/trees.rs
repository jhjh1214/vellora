//! Name trees and number trees (ISO 32000-2:2020 §7.9.6 and §7.9.7).
//!
//! Both are balanced trees of dictionaries: an intermediate node has `/Kids` (references to child
//! nodes), a leaf has `/Names` (a name tree: `key value key value ...` with string keys) or `/Nums`
//! (a number tree: the same with integer keys). Every node but the root has `/Limits [least
//! greatest]`, the range of keys in its subtree.
//!
//! Real files break these rules, so the walks here are lenient and bounded:
//!
//! - **Cycles are cut.** A node is visited at most once, and the number of nodes visited by one
//!   walk is limited by [`Limits::max_tree_nodes`](crate::limits::Limits::max_tree_nodes), as is
//!   the depth by [`Limits::max_nesting_depth`](crate::limits::Limits::max_nesting_depth).
//! - **`/Limits` only prune.** A lookup skips a subtree whose limits exclude the key, but a node
//!   without usable limits is searched.
//! - **Odd entries are skipped.** A key of the wrong type, an odd trailing item, or a kid that is
//!   not a dictionary is ignored; a kid that cannot be parsed ends that branch.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use crate::error::Result;
use crate::limits::LimitKind;
use crate::object::{Object, ObjectKind};
use crate::store::ObjectStore;

/// One node waiting to be visited, and how deep it is.
struct Pending {
    node: Arc<Object<'static>>,
    depth: u64,
}

/// The walk state shared by both kinds of tree.
struct Walk<'s, 'a> {
    store: &'s ObjectStore<'a>,
    stack: Vec<Pending>,
    visited: HashSet<u32>,
    nodes: u64,
}

impl<'s, 'a> Walk<'s, 'a> {
    fn new(store: &'s ObjectStore<'a>, root: &Object<'_>) -> Result<Self> {
        let mut visited = HashSet::new();
        if let ObjectKind::Ref(reference) = root.kind {
            visited.insert(reference.num);
        }
        let root = store.deref(root)?;
        Ok(Self {
            store,
            stack: vec![Pending {
                node: root,
                depth: 1,
            }],
            visited,
            nodes: 0,
        })
    }

    /// The next node, counting it against the limits.
    fn next_node(&mut self) -> Result<Option<Pending>> {
        let Some(pending) = self.stack.pop() else {
            return Ok(None);
        };
        self.nodes += 1;
        let limits = self.store.limits();
        limits.check(LimitKind::TreeNodes, self.nodes, None)?;
        limits.check(LimitKind::NestingDepth, pending.depth, None)?;
        Ok(Some(pending))
    }

    /// Queues the kids of `node` (in order, so the first is visited first) that `wanted` accepts
    /// by their dictionaries.
    fn push_kids(
        &mut self,
        node: &Object<'_>,
        depth: u64,
        wanted: &dyn Fn(&Object<'static>) -> bool,
    ) {
        let Some(ObjectKind::Array(kids)) = node
            .as_dict()
            .and_then(|dict| dict.get(b"Kids"))
            .map(|kids| &kids.kind)
        else {
            return;
        };
        let mut found = Vec::new();
        for kid in kids {
            let ObjectKind::Ref(reference) = kid.kind else {
                continue;
            };
            if !self.visited.insert(reference.num) {
                continue;
            }
            // A kid that cannot be read ends that branch only.
            let Ok(object) = self.store.resolve(reference) else {
                continue;
            };
            if object.as_dict().is_some() && wanted(&object) {
                found.push(Pending {
                    node: object,
                    depth: depth + 1,
                });
            }
        }
        self.stack.extend(found.into_iter().rev());
    }
}

/// The items of the array `key` of `node`, if it is one.
fn array_of<'n>(node: &'n Object<'_>, key: &[u8]) -> Option<&'n [Object<'n>]> {
    match &node.as_dict()?.get(key)?.kind {
        ObjectKind::Array(items) => Some(items),
        _ => None,
    }
}

/// The `/Limits` of a node as two byte strings, if it has well-formed ones.
fn string_limits(node: &Object<'_>) -> Option<(Vec<u8>, Vec<u8>)> {
    match array_of(node, b"Limits")? {
        [first, last] => match (&first.kind, &last.kind) {
            (ObjectKind::String(first), ObjectKind::String(last)) => {
                Some((first.to_vec(), last.to_vec()))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The value stored under `key` in the name tree rooted at `root` (a dictionary or a reference to
/// one), or `None`. The value is returned as stored: a reference stays a reference.
///
/// # Errors
///
/// [`Error::LimitExceeded`](crate::Error::LimitExceeded) for a tree with too many nodes or too
/// deep, and the store's errors for a root that cannot be read.
pub fn name_tree_lookup(
    store: &ObjectStore<'_>,
    root: &Object<'_>,
    key: &[u8],
) -> Result<Option<Object<'static>>> {
    let mut walk = Walk::new(store, root)?;
    while let Some(Pending { node, depth }) = walk.next_node()? {
        if let Some(names) = array_of(&node, b"Names") {
            for [name, value] in names.as_chunks::<2>().0 {
                if matches!(&name.kind, ObjectKind::String(name) if name.as_ref() == key) {
                    return Ok(Some(value.clone().into_owned()));
                }
            }
        }
        // A kid whose limits exclude the key cannot hold it; one without limits might.
        let may_hold_key = |kid: &Object<'static>| match string_limits(kid) {
            Some((first, last)) => {
                key.cmp(first.as_slice()) != Ordering::Less
                    && key.cmp(last.as_slice()) != Ordering::Greater
            }
            None => true,
        };
        walk.push_kids(&node, depth, &may_hold_key);
    }
    Ok(None)
}

/// The integer keys and values of the number tree rooted at `root`, sorted by key (equal keys keep
/// the order they have in the tree). At most [`Limits::max_tree_nodes`] entries are collected.
///
/// [`Limits::max_tree_nodes`]: crate::limits::Limits::max_tree_nodes
///
/// # Errors
///
/// [`Error::LimitExceeded`](crate::Error::LimitExceeded) for a tree with too many nodes, entries
/// or levels, and the store's errors for a root that cannot be read.
pub fn number_tree_entries(
    store: &ObjectStore<'_>,
    root: &Object<'_>,
) -> Result<Vec<(i64, Object<'static>)>> {
    let mut walk = Walk::new(store, root)?;
    let mut entries: Vec<(i64, Object<'static>)> = Vec::new();
    while let Some(Pending { node, depth }) = walk.next_node()? {
        if let Some(nums) = array_of(&node, b"Nums") {
            for [key, value] in nums.as_chunks::<2>().0 {
                if let Some(key) = key.as_integer() {
                    let collected = u64::try_from(entries.len()).unwrap_or(u64::MAX);
                    store.limits().check(
                        LimitKind::TreeNodes,
                        collected.saturating_add(1),
                        None,
                    )?;
                    entries.push((key, value.clone().into_owned()));
                }
            }
        }
        walk.push_kids(&node, depth, &|_| true);
    }
    entries.sort_by_key(|(key, _)| *key);
    Ok(entries)
}
