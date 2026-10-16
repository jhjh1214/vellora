//! Feature detection: reads the catalog, the pages' annotations and actions, the form fields and
//! the bookmarks, and sets a flag for each risky or notable thing it finds.
//!
//! Only dictionaries are read. Actions are classified by their `/S` name and never run; streams
//! (including any script text) are never decoded. Every walk is bounded: objects are visited
//! once, nesting is capped, and the whole scan has a budget of object reads.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use vellora_cos::{Dict, Object, ObjectKind, ObjectStore};

use crate::Features;
use crate::summary::{MAX_PAGE_SIZES, PageSizeCount, StringList};

/// Object reads one scan may do. A page with a few annotations costs a handful; this allows
/// documents of a million pages and stops one that lists the same few objects through millions of
/// distinct references.
const READ_BUDGET: u64 = 1 << 22;

/// Deepest chain of actions (`/Next`), form field parents and kids followed.
const MAX_DEPTH: usize = 32;

/// Messages kept in `Summary::problems`.
pub const MAX_PROBLEMS: usize = 16;

type Value = Arc<Object<'static>>;

#[allow(clippy::struct_excessive_bools)] // one flag per thing the scan notes
pub(crate) struct Scanner<'s, 'a> {
    store: &'s ObjectStore<'a>,
    pub(crate) features: Features,
    pub(crate) pages: u64,
    pub(crate) problems: Vec<String>,
    /// The catalog says the document is tagged.
    pub(crate) tagged: bool,
    /// The addresses of `URI` actions.
    pub(crate) uris: StringList,
    /// The names of embedded files.
    pub(crate) embedded_files: StringList,
    /// Distinct page sizes (hundredths of a point, as displayed) and their page counts.
    sizes: BTreeMap<(i64, i64), u64>,
    pub(crate) sizes_truncated: bool,
    /// Object numbers already looked at, so shared and cyclic structures are read once.
    visited: HashSet<u32>,
    reads_left: u64,
    /// The read budget ran out.
    exhausted: bool,
    /// Some object could not be read.
    unreadable: bool,
}

impl<'s, 'a> Scanner<'s, 'a> {
    pub(crate) fn new(store: &'s ObjectStore<'a>) -> Self {
        Self {
            store,
            features: Features::default(),
            pages: 0,
            problems: Vec::new(),
            tagged: false,
            uris: StringList::default(),
            embedded_files: StringList::default(),
            sizes: BTreeMap::new(),
            sizes_truncated: false,
            visited: HashSet::new(),
            reads_left: READ_BUDGET,
            exhausted: false,
            unreadable: false,
        }
    }

    pub(crate) fn run(&mut self) {
        match self.store.root() {
            Ok(Some(root)) => self.catalog(&root),
            Ok(None) => self.problem("the document has no catalog (/Root)".to_owned()),
            Err(error) => {
                self.unreadable = true;
                self.problem(format!("catalog: {error}"));
            }
        }
        self.walk_pages();
        self.features.complete = !self.exhausted && !self.unreadable;
    }

    /// The page sizes found, most pages first, then smallest first.
    #[allow(clippy::cast_precision_loss)] // hundredths are clamped far below 2^53
    pub(crate) fn page_sizes(&self) -> Vec<PageSizeCount> {
        let mut sizes: Vec<PageSizeCount> = self
            .sizes
            .iter()
            .map(|(&(width, height), &pages)| PageSizeCount {
                width: width as f64 / 100.0,
                height: height as f64 / 100.0,
                pages,
            })
            .collect();
        sizes.sort_by(|a, b| {
            b.pages
                .cmp(&a.pages)
                .then(a.width.total_cmp(&b.width))
                .then(a.height.total_cmp(&b.height))
        });
        sizes
    }

    fn problem(&mut self, message: String) {
        if self.problems.len() < MAX_PROBLEMS {
            self.problems.push(message);
        }
    }

    /// Spends one read; `false` once the budget is gone.
    fn spend(&mut self) -> bool {
        if self.reads_left == 0 {
            self.exhausted = true;
            return false;
        }
        self.reads_left -= 1;
        true
    }

    /// The value of `object`, following a reference. `None` for `null`, a missing object, an
    /// unreadable one (noted as a problem) or when the budget is spent. The first element is the
    /// object number when `object` is a reference.
    fn follow(&mut self, object: &Object<'_>) -> Option<(Option<u32>, Value)> {
        let number = match object.kind {
            ObjectKind::Ref(reference) => Some(reference.num),
            _ => None,
        };
        if !self.spend() {
            return None;
        }
        match self.store.deref(object) {
            Ok(value) if matches!(value.kind, ObjectKind::Null) => None,
            Ok(value) => Some((number, value)),
            Err(error) => {
                self.unreadable = true;
                let what =
                    number.map_or_else(|| "a direct object".to_owned(), |n| format!("object {n}"));
                self.problem(format!("{what}: {error}"));
                None
            }
        }
    }

    /// The value of `key` in `dict`, resolved.
    fn entry(&mut self, dict: &Dict<'_>, key: &[u8]) -> Option<Value> {
        let value = dict.get(key)?;
        self.follow(value).map(|(_, value)| value)
    }

    /// Like [`follow`](Self::follow), but each object number is handed out once.
    fn follow_once(&mut self, object: &Object<'_>) -> Option<Value> {
        let (number, value) = self.follow(object)?;
        match number {
            Some(number) if !self.visited.insert(number) => None,
            _ => Some(value),
        }
    }

    fn catalog(&mut self, root: &Value) {
        let Some(catalog) = root.as_dict() else {
            self.problem("the catalog is not a dictionary".to_owned());
            return;
        };
        // The open action first: its objects are then classified as "runs on open".
        if let Some(action) = catalog.get(b"OpenAction") {
            self.action(action, true, 0);
        }
        if let Some(aa) = catalog.get(b"AA") {
            self.additional_actions(aa);
        }
        if let Some(names) = self.entry(catalog, b"Names")
            && let Some(names) = names.as_dict()
        {
            if self.entry(names, b"JavaScript").is_some() {
                self.features.names_javascript = true;
            }
            if self.entry(names, b"EmbeddedFiles").is_some() {
                self.features.embedded_files = true;
                // The reference itself, so that the root counts as visited if a kid names it.
                if let Some(tree) = names.get(b"EmbeddedFiles") {
                    self.embedded_name_tree(tree);
                }
            }
        }
        if let Some(mark_info) = self.entry(catalog, b"MarkInfo")
            && let Some(mark_info) = mark_info.as_dict()
            && let Some(marked) = self.entry(mark_info, b"Marked")
        {
            self.tagged = matches!(marked.kind, ObjectKind::Bool(true));
        }
        if self.entry(catalog, b"OCProperties").is_some() {
            self.features.optional_content = true;
        }
        if let Some(form) = self.entry(catalog, b"AcroForm")
            && let Some(form) = form.as_dict()
        {
            self.acroform(form);
        }
        if let Some(outlines) = self.entry(catalog, b"Outlines")
            && let Some(outlines) = outlines.as_dict()
        {
            self.outlines(outlines);
        }
    }

    /// The names in the `/EmbeddedFiles` name tree (§7.9.6): `/Names` holds name and file
    /// specification pairs, `/Kids` the subtrees. The listing is bounded like every other walk.
    fn embedded_name_tree(&mut self, root: &Object<'_>) {
        let mut stack: Vec<(Object<'static>, usize)> = vec![(root.clone().into_owned(), 1)];
        while let Some((node, depth)) = stack.pop() {
            let Some(value) = self.follow_once(&node) else {
                continue;
            };
            let Some(dict) = value.as_dict() else {
                continue;
            };
            if let Some(names) = self.entry(dict, b"Names")
                && let ObjectKind::Array(items) = &names.kind
            {
                for pair in items.chunks(2) {
                    if let Some(key) = pair.first().and_then(string_bytes) {
                        self.embedded_files
                            .push(vellora_cos::textstring::decode_text_string(&key));
                    }
                }
            }
            if depth >= MAX_DEPTH {
                continue;
            }
            if let Some(kids) = self.entry(dict, b"Kids")
                && let ObjectKind::Array(kids) = &kids.kind
            {
                for kid in kids.iter().rev() {
                    stack.push((kid.clone().into_owned(), depth + 1));
                }
            }
        }
    }

    /// The size a page is shown at, counted once per page.
    fn count_page_size(&mut self, page: &vellora_cos::Page) {
        let boxed = page
            .inherited
            .crop_box
            .as_ref()
            .or(page.inherited.media_box.as_ref());
        let Some(rect) = boxed.and_then(|b| rectangle(b)) else {
            return;
        };
        let turns = page
            .inherited
            .rotate
            .as_ref()
            .and_then(|r| r.as_integer())
            .map_or(0, |degrees| (degrees.rem_euclid(360) / 90) % 4);
        let (mut width, mut height) = (rect.0, rect.1);
        if turns % 2 == 1 {
            std::mem::swap(&mut width, &mut height);
        }
        let key = (hundredths(width), hundredths(height));
        if let Some(count) = self.sizes.get_mut(&key) {
            *count += 1;
        } else if self.sizes.len() < MAX_PAGE_SIZES {
            self.sizes.insert(key, 1);
        } else {
            self.sizes_truncated = true;
        }
    }

    fn acroform(&mut self, form: &Dict<'_>) {
        self.features.acroform = true;
        if self.entry(form, b"XFA").is_some() {
            self.features.xfa = true;
        }
        let Some(fields) = self.entry(form, b"Fields") else {
            return;
        };
        let ObjectKind::Array(fields) = &fields.kind else {
            return;
        };
        // Depth-first with an explicit stack (field trees can be as deep as an attacker likes);
        // each entry carries the field type inherited from its ancestors.
        let mut stack: Vec<(Object<'static>, Option<Vec<u8>>, usize)> = fields
            .iter()
            .rev()
            .map(|field| (field.clone().into_owned(), None, 1))
            .collect();
        while let Some((field, inherited_type, depth)) = stack.pop() {
            let Some(value) = self.follow_once(&field) else {
                continue;
            };
            let Some(dict) = value.as_dict() else {
                continue;
            };
            let field_type = name_of(dict, b"FT").or(inherited_type);
            if field_type.as_deref() == Some(b"Sig") {
                self.features.signature_fields = true;
            }
            self.node_actions(dict);
            if depth >= MAX_DEPTH {
                continue;
            }
            if let Some(kids) = self.entry(dict, b"Kids")
                && let ObjectKind::Array(kids) = &kids.kind
            {
                for kid in kids.iter().rev() {
                    stack.push((kid.clone().into_owned(), field_type.clone(), depth + 1));
                }
            }
        }
    }

    /// Bookmarks can carry actions (a bookmark that launches a program is a classic).
    fn outlines(&mut self, outlines: &Dict<'_>) {
        let mut stack: Vec<Object<'static>> = Vec::new();
        if let Some(first) = outlines.get(b"First") {
            stack.push(first.clone().into_owned());
        }
        while let Some(item) = stack.pop() {
            let Some(value) = self.follow_once(&item) else {
                continue;
            };
            let Some(dict) = value.as_dict() else {
                continue;
            };
            if let Some(action) = dict.get(b"A") {
                self.action(action, false, 0);
            }
            for key in [&b"Next"[..], b"First"] {
                if let Some(next) = dict.get(key) {
                    stack.push(next.clone().into_owned());
                }
            }
        }
    }

    /// Pages are walked lazily by the store; each page's own actions and annotations are scanned
    /// as it comes.
    fn walk_pages(&mut self) {
        let store = self.store;
        for page in store.pages() {
            if !self.spend() {
                break;
            }
            let page = match page {
                Ok(page) => page,
                Err(error) => {
                    self.unreadable = true;
                    self.problem(format!("page tree: {error}"));
                    continue;
                }
            };
            self.pages += 1;
            self.count_page_size(&page);
            let Some(dict) = page.object.as_dict() else {
                continue;
            };
            if let Some(aa) = dict.get(b"AA") {
                self.additional_actions(aa);
            }
            let Some(annots) = self.entry(dict, b"Annots") else {
                continue;
            };
            let ObjectKind::Array(annots) = &annots.kind else {
                continue;
            };
            for annot in annots {
                self.annotation(annot);
            }
        }
    }

    fn annotation(&mut self, annot: &Object<'_>) {
        let Some(value) = self.follow_once(annot) else {
            return;
        };
        let Some(dict) = value.as_dict() else {
            return;
        };
        match name_of(dict, b"Subtype").as_deref() {
            Some(b"FileAttachment") => {
                self.features.embedded_files = true;
                self.attachment_name(dict);
            }
            Some(b"Widget") if self.effective_field_type(dict).as_deref() == Some(b"Sig") => {
                self.features.signature_fields = true;
            }
            _ => {}
        }
        self.node_actions(dict);
    }

    /// The name of a file attachment annotation's file: its file specification's `/UF` or `/F`,
    /// or the specification itself when it is a string.
    fn attachment_name(&mut self, annotation: &Dict<'_>) {
        let Some(spec) = self.entry(annotation, b"FS") else {
            return;
        };
        let name = match &spec.kind {
            ObjectKind::String(bytes) => Some(bytes.to_vec()),
            ObjectKind::Dict(dict) => [&b"UF"[..], b"F"]
                .iter()
                .find_map(|key| self.entry(dict, key).as_deref().and_then(string_of)),
            _ => None,
        };
        if let Some(name) = name {
            self.embedded_files
                .push(vellora_cos::textstring::decode_text_string(&name));
        }
    }

    /// `/FT`, taken from the nearest ancestor (`/Parent`) that has one (§12.7.4.1).
    fn effective_field_type(&mut self, dict: &Dict<'_>) -> Option<Vec<u8>> {
        if let Some(own) = name_of(dict, b"FT") {
            return Some(own);
        }
        let mut current = self.entry(dict, b"Parent")?;
        for _ in 0..MAX_DEPTH {
            let parent = current.as_dict()?;
            if let Some(found) = name_of(parent, b"FT") {
                return Some(found);
            }
            current = self.entry(parent, b"Parent")?;
        }
        None
    }

    /// The `/A` and `/AA` of an annotation or form field.
    fn node_actions(&mut self, dict: &Dict<'_>) {
        if let Some(action) = dict.get(b"A") {
            self.action(action, false, 0);
        }
        if let Some(aa) = dict.get(b"AA") {
            self.additional_actions(aa);
        }
    }

    /// An `/AA` dictionary: its presence is a feature, each entry is an action.
    fn additional_actions(&mut self, aa: &Object<'_>) {
        let Some((_, value)) = self.follow(aa) else {
            return;
        };
        let Some(dict) = value.as_dict() else {
            return;
        };
        self.features.additional_actions = true;
        for entry in &dict.entries {
            self.action(&entry.value, false, 0);
        }
    }

    /// An action (§12.6): `open` marks one that runs when the document is opened.
    fn action(&mut self, action: &Object<'_>, open: bool, depth: usize) {
        if depth >= MAX_DEPTH {
            return;
        }
        if let Some(value) = self.follow_once(action) {
            self.classify(&value, open, depth);
        }
    }

    /// Classifies a resolved action by `/S` and follows `/Next`, which is an action or an array
    /// of them.
    fn classify(&mut self, value: &Value, open: bool, depth: usize) {
        let Some(dict) = value.as_dict() else {
            return;
        };
        match name_of(dict, b"S").as_deref() {
            Some(b"JavaScript") => {
                self.features.javascript_actions = true;
                self.features.open_action_javascript |= open;
            }
            Some(b"Launch") => self.features.launch_actions = true,
            Some(b"URI") => {
                self.features.uri_actions = true;
                if let Some(uri) = self.entry(dict, b"URI").as_deref().and_then(string_of) {
                    self.uris
                        .push(vellora_cos::textstring::decode_text_string(&uri));
                }
            }
            Some(b"SubmitForm") => self.features.submit_form_actions = true,
            Some(b"GoToR") => self.features.goto_remote_actions = true,
            _ => {}
        }
        let Some(next) = dict.get(b"Next") else {
            return;
        };
        if depth + 1 >= MAX_DEPTH {
            return;
        }
        if let Some(next) = self.follow_once(next) {
            match &next.kind {
                ObjectKind::Array(items) => {
                    for item in items {
                        self.action(item, open, depth + 1);
                    }
                }
                _ => self.classify(&next, open, depth + 1),
            }
        }
    }
}

/// The name stored under `key`, if the value is a direct name.
fn name_of(dict: &Dict<'_>, key: &[u8]) -> Option<Vec<u8>> {
    match &dict.get(key)?.kind {
        ObjectKind::Name(name) => Some(name.to_vec()),
        _ => None,
    }
}

/// The bytes of a string object.
fn string_of(object: &Object<'_>) -> Option<Vec<u8>> {
    match &object.kind {
        ObjectKind::String(bytes) => Some(bytes.to_vec()),
        _ => None,
    }
}

/// The bytes of `object` if it is a direct string.
fn string_bytes(object: &Object<'_>) -> Option<Vec<u8>> {
    string_of(object)
}

/// The width and height of a rectangle array of four numbers; `None` for anything else, an
/// unresolved reference included.
fn rectangle(object: &Object<'_>) -> Option<(f64, f64)> {
    let ObjectKind::Array(items) = &object.kind else {
        return None;
    };
    let number = |index: usize| match &items.get(index)?.kind {
        #[allow(clippy::cast_precision_loss)] // page coordinates are small
        ObjectKind::Integer(n) => Some(*n as f64),
        ObjectKind::Real(r) if r.is_finite() => Some(*r),
        _ => None,
    };
    let (x1, y1, x2, y2) = (number(0)?, number(1)?, number(2)?, number(3)?);
    Some(((x2 - x1).abs(), (y2 - y1).abs()))
}

/// A length in points as whole hundredths, clamped so that nothing overflows.
fn hundredths(points: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)] // clamped first
    let value = (points * 100.0).round().clamp(0.0, 1.0e12) as i64;
    value
}
