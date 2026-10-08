//! Generators for the synthetic documents.
//!
//! Every byte of PDF structure is produced by the `cos` writers (CLAUDE.md invariant 1): a tiny
//! seed with no cross-reference is repaired and rewritten by [`write_full`], and the pages are
//! then appended as one incremental section. The only hand-written PDF text is that seed and the
//! page content streams, which are stream *data*.
//!
//! Output is deterministic: no clock, no randomness beyond a fixed-seed generator.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use vellora_cos::object::{Dict, Object, ObjectKind};
use vellora_cos::write::{Changes, FullOptions, NewObject, incremental_update, write_full};
use vellora_cos::{Limits, ObjRef, ObjectStore};

use crate::Result;

/// Pages of the text document (the M0 target is "10,000 pages").
pub(crate) const TEXT_PAGES: u32 = 10_000;
/// Pages of the image document: one raw RGB image each.
pub(crate) const IMAGE_PAGES: u32 = 102;
/// Image side in pixels; 1280 x 1280 x 3 bytes is 4.7 MiB per page, so the file is ~500 MB.
const IMAGE_SIDE: u32 = 1280;
/// Pages per intermediate `/Pages` node, as real producers do, instead of one flat array.
const NODE_FANOUT: usize = 100;

/// The two documents of M0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// 10,000 pages of text.
    Text,
    /// ~500 MB of uncompressed images.
    Images,
}

impl Kind {
    pub(crate) const ALL: [Self; 2] = [Self::Text, Self::Images];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Text => "text-10k",
            Self::Images => "images-500mb",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    pub(crate) fn file_name(self) -> String {
        format!("{}.pdf", self.name())
    }
}

/// A seed with no cross-reference: opening it takes the recovery path, and `write_full` turns it
/// into a clean file.
const SEED: &[u8] = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 3 >>\n";

fn name(text: &str) -> Object<'static> {
    Object::new(ObjectKind::Name(text.as_bytes().to_vec().into()))
}

fn int(n: i64) -> Object<'static> {
    Object::new(ObjectKind::Integer(n))
}

fn reference(target: ObjRef) -> Object<'static> {
    Object::new(ObjectKind::Ref(target))
}

fn array(items: Vec<Object<'static>>) -> Object<'static> {
    Object::new(ObjectKind::Array(items))
}

fn dict(entries: Vec<(&str, Object<'static>)>) -> Dict<'static> {
    let mut dict = Dict::default();
    for (key, value) in entries {
        dict.set(key.as_bytes(), value);
    }
    dict
}

fn value(entries: Vec<(&str, Object<'static>)>) -> NewObject {
    NewObject::Value(Object::new(ObjectKind::Dict(dict(entries))))
}

fn media_box() -> Object<'static> {
    array(vec![int(0), int(0), int(612), int(792)])
}

/// xorshift64*: deterministic filler for image data.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn fill(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(8) {
            let bytes = self.next().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}

/// The content stream of text page `page`: a heading and 45 lines of filler.
fn text_content(page: u32) -> Vec<u8> {
    let mut out = format!("BT /F1 18 Tf 50 740 Td (Page {}) Tj ET\n", page + 1).into_bytes();
    for line in 0..45_u32 {
        let y = 710 - 15 * i64::from(line);
        out.extend_from_slice(
            format!(
                "BT /F1 10 Tf 50 {y} Td (Line {line} of page {}: the quick brown fox jumps over the lazy dog.) Tj ET\n",
                page + 1
            )
            .as_bytes(),
        );
    }
    out
}

fn stream(data: Vec<u8>, extra: Vec<(&str, Object<'static>)>) -> NewObject {
    NewObject::Stream {
        dict: dict(extra),
        data,
    }
}

/// The content stream and resources of text page `index`.
fn text_page(
    changes: &mut Changes,
    store: &ObjectStore<'_>,
    font: ObjRef,
    index: u32,
) -> (ObjRef, Dict<'static>) {
    let content = changes.add_object(store, stream(text_content(index), vec![]));
    let fonts = dict(vec![("F1", reference(font))]);
    (
        content,
        dict(vec![("Font", Object::new(ObjectKind::Dict(fonts)))]),
    )
}

/// The content stream and resources of an image page: one raw RGB image scaled to the page.
fn image_page(
    changes: &mut Changes,
    store: &ObjectStore<'_>,
    noise: &mut Noise,
) -> (ObjRef, Dict<'static>) {
    let mut pixels = vec![0_u8; IMAGE_SIDE as usize * IMAGE_SIDE as usize * 3];
    noise.fill(&mut pixels);
    let image = changes.add_object(
        store,
        stream(
            pixels,
            vec![
                ("Type", name("XObject")),
                ("Subtype", name("Image")),
                ("Width", int(i64::from(IMAGE_SIDE))),
                ("Height", int(i64::from(IMAGE_SIDE))),
                ("ColorSpace", name("DeviceRGB")),
                ("BitsPerComponent", int(8)),
            ],
        ),
    );
    let content = changes.add_object(
        store,
        stream(b"q 612 0 0 792 0 0 cm /Im0 Do Q\n".to_vec(), vec![]),
    );
    let images = dict(vec![("Im0", reference(image))]);
    (
        content,
        dict(vec![("XObject", Object::new(ObjectKind::Dict(images)))]),
    )
}

/// Builds the pages of `kind` as one set of changes over `store`, and the root's replacement.
fn build_changes(store: &ObjectStore<'_>, kind: Kind) -> Result<Changes> {
    let root = store.root_ref().ok_or("the seed has no /Root reference")?;
    let mut changes = Changes::new();
    // Hands out the next free number now and fills the object in later, so that parents can be
    // referred to by the pages that precede them.
    let placeholder = || NewObject::Value(Object::new(ObjectKind::Null));

    let font = changes.add_object(
        store,
        value(vec![
            ("Type", name("Font")),
            ("Subtype", name("Type1")),
            ("BaseFont", name("Helvetica")),
        ]),
    );
    let pages_root = changes.add_object(store, placeholder());
    let count = match kind {
        Kind::Text => TEXT_PAGES,
        Kind::Images => IMAGE_PAGES,
    };
    let nodes: Vec<ObjRef> = (0..count.div_ceil(u32::try_from(NODE_FANOUT)?))
        .map(|_| changes.add_object(store, placeholder()))
        .collect();

    let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
    let mut kids_of_node: Vec<Vec<Object<'static>>> = vec![Vec::new(); nodes.len()];
    for index in 0..count {
        let node_index = index as usize / NODE_FANOUT;
        let parent = nodes[node_index];
        let (content, resources) = match kind {
            Kind::Text => text_page(&mut changes, store, font, index),
            Kind::Images => image_page(&mut changes, store, &mut noise),
        };
        let page = changes.add_object(
            store,
            value(vec![
                ("Type", name("Page")),
                ("Parent", reference(parent)),
                ("MediaBox", media_box()),
                ("Resources", Object::new(ObjectKind::Dict(resources))),
                ("Contents", reference(content)),
            ]),
        );
        kids_of_node[node_index].push(reference(page));
    }

    for (node, kids) in nodes.iter().zip(kids_of_node) {
        let in_node = i64::try_from(kids.len())?;
        changes.set_object(
            node.num,
            value(vec![
                ("Type", name("Pages")),
                ("Parent", reference(pages_root)),
                ("Kids", array(kids)),
                ("Count", int(in_node)),
            ]),
        );
    }
    changes.set_object(
        pages_root.num,
        value(vec![
            ("Type", name("Pages")),
            ("Kids", array(nodes.iter().map(|&n| reference(n)).collect())),
            ("Count", int(i64::from(count))),
        ]),
    );
    changes.set_object(
        root.num,
        value(vec![
            ("Type", name("Catalog")),
            ("Pages", reference(pages_root)),
        ]),
    );
    Ok(changes)
}

/// The clean seed file and the section that adds the pages of `kind` to it.
fn build(kind: Kind) -> Result<(Vec<u8>, Vec<u8>)> {
    let limits = Limits::default();
    let seed = ObjectStore::open(SEED, limits.clone())?;
    let base = write_full(&seed, &FullOptions::default())?;
    let store = ObjectStore::open(&base, limits)?;
    let appended = incremental_update(&store, &build_changes(&store, kind)?)?;
    Ok((base, appended))
}

/// Writes the document of `kind` to `path`, replacing any file there. Returns its size.
pub(crate) fn generate(kind: Kind, path: &Path) -> Result<u64> {
    let (base, appended) = build(kind)?;
    let mut file = BufWriter::new(File::create(path)?);
    file.write_all(&base)?;
    file.write_all(&appended)?;
    file.flush()?;
    file.get_ref().sync_all()?;
    Ok((base.len() + appended.len()) as u64)
}

/// The appended bytes of a "commit" to the document in `original`: a new document information
/// dictionary, plus (if `payload` is non-zero) a stream of that many bytes, as an inserted image
/// would be.
pub(crate) fn commit_section(original: &[u8], payload: usize) -> Result<Vec<u8>> {
    let store = ObjectStore::open(original, Limits::default())?;
    let mut changes = Changes::new();
    let info = changes.add_object(
        &store,
        value(vec![("Title", string("Edited by the benchmark"))]),
    );
    changes.set_info(info);
    if payload > 0 {
        let mut data = vec![0_u8; payload];
        Noise(42).fill(&mut data);
        changes.add_object(&store, stream(data, vec![]));
    }
    Ok(incremental_update(&store, &changes)?)
}

fn string(text: &str) -> Object<'static> {
    Object::new(ObjectKind::String(text.as_bytes().to_vec().into()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The text document is a clean two-revision file with every page reachable, and a commit
    /// on top of it keeps the bytes before it.
    #[test]
    fn text_document_and_commit_are_well_formed() {
        let (base, appended) = build(Kind::Text).unwrap();
        let mut document = base;
        document.extend_from_slice(&appended);

        let store = ObjectStore::open(&document, Limits::default()).unwrap();
        assert_eq!(store.repaired(), vec![]);
        assert_eq!(store.revision_count(), 2);
        assert_eq!(store.pages().count(), TEXT_PAGES as usize);

        let section = commit_section(&document, 1000).unwrap();
        let mut committed = document.clone();
        committed.extend_from_slice(&section);
        assert!(committed.starts_with(&document));
        let store = ObjectStore::open(&committed, Limits::default()).unwrap();
        assert_eq!(store.repaired(), vec![]);
        assert_eq!(store.revision_count(), 3);
    }
}
