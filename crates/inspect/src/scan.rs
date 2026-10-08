//! Feature detection: reads the catalog, the pages' annotations and actions, the form fields and
//! the bookmarks, and sets a flag for each risky or notable thing it finds.
//!
//! Only dictionaries are read. Actions are classified by their `/S` name and never run; streams
//! (including any script text) are never decoded. Every walk is bounded: objects are visited
//! once, nesting is capped, and the whole scan has a budget of object reads.

use std::collections::HashSet;
use std::sync::Arc;

use vellora_cos::{Dict, Object, ObjectKind, ObjectStore};

use crate::Features;

/// Object reads one scan may do. A page with a few annotations costs a handful; this allows
/// documents of a million pages and stops one that lists the same few objects through millions of
/// distinct references.
const READ_BUDGET: u64 = 1 << 22;

/// Deepest chain of actions (`/Next`), form field parents and kids followed.
const MAX_DEPTH: usize = 32;

/// Messages kept in `Summary::problems`.
pub const MAX_PROBLEMS: usize = 16;

type Value = Arc<Object<'static>>;

pub(crate) struct Scanner<'s, 'a> {
    store: &'s ObjectStore<'a>,
    pub(crate) features: Features,
    pub(crate) pages: u64,
    pub(crate) problems: Vec<String>,
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
            }
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
            Some(b"FileAttachment") => self.features.embedded_files = true,
            Some(b"Widget") if self.effective_field_type(dict).as_deref() == Some(b"Sig") => {
                self.features.signature_fields = true;
            }
            _ => {}
        }
        self.node_actions(dict);
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
            Some(b"URI") => self.features.uri_actions = true,
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
