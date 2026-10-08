//! Tests of the summary and the feature scan on small synthetic files, plus the encrypted
//! fixtures of `vellora-cos`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use vellora_cos::{EncryptionError, Error};
use vellora_inspect::{Features, Options, Summary, inspect};

/// Builds a file with a correct classic cross-reference table; gaps in the object numbers are
/// free entries.
struct Pdf {
    data: Vec<u8>,
    offsets: BTreeMap<u32, usize>,
}

impl Pdf {
    fn new() -> Self {
        Self {
            data: b"%PDF-1.7\n".to_vec(),
            offsets: BTreeMap::new(),
        }
    }

    fn object(&mut self, num: u32, body: &str) -> &mut Self {
        self.offsets.insert(num, self.data.len());
        self.data
            .extend(format!("{num} 0 obj\n{body}\nendobj\n").bytes());
        self
    }

    fn finish(&self) -> Vec<u8> {
        let mut data = self.data.clone();
        let size = self.offsets.keys().next_back().copied().unwrap_or(0) + 1;
        let at = data.len();
        let mut table = format!("xref\n0 {size}\n");
        for num in 0..size {
            match self.offsets.get(&num) {
                Some(offset) => writeln!(table, "{offset:010} 00000 n ").unwrap(),
                None => table.push_str("0000000000 65535 f \n"),
            }
        }
        write!(
            table,
            "trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{at}\n%%EOF\n"
        )
        .unwrap();
        data.extend(table.bytes());
        data
    }
}

/// A one-page document: catalog (1) with `catalog_extra` entries, page tree (2), a page (3) with
/// `page_extra` entries, and the given further objects.
fn doc(catalog_extra: &str, page_extra: &str, objects: &[(u32, &str)]) -> Vec<u8> {
    let mut pdf = Pdf::new();
    pdf.object(
        1,
        &format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
    );
    pdf.object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    pdf.object(
        3,
        &format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] {page_extra} >>"),
    );
    for &(num, body) in objects {
        pdf.object(num, body);
    }
    pdf.finish()
}

/// Reads one flag of a [`Features`].
type Flag = fn(&Features) -> bool;

/// No feature found, and the scan complete.
fn complete_only() -> Features {
    let mut features = Features::default();
    features.complete = true;
    features
}

fn run(data: &[u8]) -> Summary {
    inspect(data, &Options::default()).unwrap()
}

fn features(data: &[u8]) -> Features {
    run(data).features.expect("not locked")
}

/// A page with one annotation (object 10) with the given entries.
fn with_annot(entries: &str) -> Vec<u8> {
    doc(
        "",
        "/Annots [10 0 R]",
        &[(10, &format!("<< /Type /Annot {entries} >>"))],
    )
}

#[test]
fn a_plain_document_has_no_features() {
    let data = doc("", "", &[]);
    let summary = run(&data);
    assert_eq!(summary.version.as_deref(), Some("1.7"));
    assert_eq!(summary.file_size, data.len() as u64);
    assert_eq!(summary.pages, Some(1));
    assert_eq!(summary.objects, 3);
    assert_eq!(summary.revisions, 1);
    assert!(!summary.repaired && summary.repair_reasons.is_empty());
    assert!(summary.encryption.is_none() && !summary.locked);
    assert_eq!(summary.problems, Vec::<String>::new());
    assert_eq!(summary.features, Some(complete_only()));
}

#[test]
fn the_open_action_is_a_javascript_that_runs_on_open() {
    let data = doc(
        "/OpenAction 10 0 R",
        "",
        &[(10, "<< /S /JavaScript /JS (app.alert(1)) >>")],
    );
    let f = features(&data);
    assert!(f.open_action_javascript && f.javascript_actions);
    assert!(!f.names_javascript && !f.additional_actions);

    // A direct dictionary works too, and a destination is not an action.
    let data = doc("/OpenAction << /S /JavaScript /JS (x) >>", "", &[]);
    assert!(features(&data).open_action_javascript);
    let data = doc("/OpenAction [3 0 R /Fit]", "", &[]);
    assert_eq!(features(&data), complete_only());
}

#[test]
fn a_javascript_action_elsewhere_is_not_an_open_action() {
    let data = with_annot("/Subtype /Link /A << /S /JavaScript /JS (x) >>");
    let f = features(&data);
    assert!(f.javascript_actions);
    assert!(!f.open_action_javascript);
}

#[test]
fn document_level_scripts_and_attachments_come_from_the_names_tree() {
    let data = doc(
        "/Names 10 0 R",
        "",
        &[(10, "<< /JavaScript << /Names [(a) 11 0 R] >> >>")],
    );
    let f = features(&data);
    assert!(f.names_javascript && !f.embedded_files);

    let data = doc("/Names << /EmbeddedFiles << /Names [] >> >>", "", &[]);
    let f = features(&data);
    assert!(f.embedded_files && !f.names_javascript);
}

#[test]
fn a_file_attachment_annotation_is_an_embedded_file() {
    assert!(features(&with_annot("/Subtype /FileAttachment /FS 11 0 R")).embedded_files);
    assert!(!features(&with_annot("/Subtype /Text")).embedded_files);
}

#[test]
fn additional_actions_are_found_on_catalog_page_annotation_and_field() {
    let aa = "/AA << /O << /S /URI /URI (http://example.com) >> >>";
    assert!(features(&doc(aa, "", &[])).additional_actions);
    assert!(features(&doc("", aa, &[])).additional_actions);
    assert!(features(&with_annot(aa)).additional_actions);
    let form = doc(
        "/AcroForm << /Fields [10 0 R] >>",
        "",
        &[(10, &format!("<< /FT /Tx /T (f) {aa} >>"))],
    );
    let f = features(&form);
    assert!(f.additional_actions && f.uri_actions && f.acroform);

    // An action dictionary alone is not `/AA`.
    assert!(!features(&with_annot("/A << /S /URI /URI (x) >>")).additional_actions);
}

#[test]
fn each_action_type_sets_its_own_flag() {
    let cases: [(&str, Flag); 4] = [
        ("<< /S /Launch /F (calc.exe) >>", |f| f.launch_actions),
        ("<< /S /URI /URI (http://example.com) >>", |f| f.uri_actions),
        ("<< /S /SubmitForm /F (http://example.com) >>", |f| {
            f.submit_form_actions
        }),
        ("<< /S /GoToR /F (other.pdf) /D [0 /Fit] >>", |f| {
            f.goto_remote_actions
        }),
    ];
    for (i, (action, flag)) in cases.iter().enumerate() {
        let f = features(&with_annot(&format!("/Subtype /Link /A {action}")));
        assert!(flag(&f), "{action}");
        let others = [
            f.launch_actions,
            f.uri_actions,
            f.submit_form_actions,
            f.goto_remote_actions,
        ];
        assert_eq!(
            others.iter().filter(|&&b| b).count(),
            1,
            "case {i}: only its own flag"
        );
    }
}

#[test]
fn actions_chained_with_next_are_found_and_loops_end() {
    let data =
        with_annot("/Subtype /Link /A << /S /URI /URI (x) /Next [<< /S /Launch >> 11 0 R] >>");
    let f = features(&doc(
        "",
        "/Annots [10 0 R]",
        &[
            (
                10,
                "<< /Subtype /Link /A << /S /URI /URI (x) /Next 11 0 R >> >>",
            ),
            (11, "<< /S /SubmitForm /Next [12 0 R] >>"),
            (12, "<< /S /GoToR /Next 11 0 R >>"),
        ],
    ));
    assert!(f.uri_actions && f.submit_form_actions && f.goto_remote_actions);
    assert!(f.complete);
    // A direct chain.
    assert!(features(&data).launch_actions);

    // An action that is its own `/Next`.
    let f = features(&doc(
        "/OpenAction 10 0 R",
        "",
        &[(10, "<< /S /JavaScript /Next 10 0 R >>")],
    ));
    assert!(f.open_action_javascript && f.complete);
}

#[test]
fn bookmarks_can_launch() {
    let f = features(&doc(
        "/Outlines 10 0 R",
        "",
        &[
            (10, "<< /Type /Outlines /First 11 0 R /Last 12 0 R >>"),
            (11, "<< /Title (a) /Next 12 0 R >>"),
            (
                12,
                "<< /Title (b) /Prev 11 0 R /Next 11 0 R /A << /S /Launch /F (x) >> >>",
            ),
        ],
    ));
    assert!(f.launch_actions && f.complete);
}

#[test]
fn forms_xfa_layers_and_signatures() {
    let f = features(&doc("/AcroForm << /Fields [] >>", "", &[]));
    assert!(f.acroform && !f.xfa && !f.signature_fields);

    let f = features(&doc(
        "/AcroForm << /Fields [] /XFA [(template) 10 0 R] >>",
        "",
        &[(10, "<< /Length 0 >>")],
    ));
    assert!(f.acroform && f.xfa);

    let f = features(&doc("/OCProperties << /OCGs [] /D << >> >>", "", &[]));
    assert!(f.optional_content);

    // A signature field in the form, one nested below a parent that carries the type, and a
    // widget on a page whose parent carries it.
    let f = features(&doc(
        "/AcroForm << /Fields [10 0 R] >>",
        "",
        &[
            (10, "<< /T (p) /Kids [11 0 R] >>"),
            (11, "<< /FT /Sig /T (s) >>"),
        ],
    ));
    assert!(f.signature_fields);
    let f = features(&doc(
        "/AcroForm << /Fields [10 0 R] >>",
        "",
        &[
            (10, "<< /FT /Sig /T (p) /Kids [11 0 R] >>"),
            (11, "<< /T (s) >>"),
        ],
    ));
    assert!(f.signature_fields);
    let f = features(&doc(
        "",
        "/Annots [11 0 R]",
        &[
            (10, "<< /FT /Sig /Kids [11 0 R] >>"),
            (11, "<< /Subtype /Widget /Parent 10 0 R >>"),
        ],
    ));
    assert!(f.signature_fields);
    let f = features(&doc(
        "/AcroForm << /Fields [10 0 R] >>",
        "",
        &[(10, "<< /FT /Tx >>")],
    ));
    assert!(!f.signature_fields);
}

#[test]
fn cyclic_structures_end() {
    // Parents that point at each other, kids that include their ancestor, a page tree loop.
    let mut pdf = Pdf::new();
    pdf.object(
        1,
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [10 0 R] >> >>",
    );
    pdf.object(2, "<< /Type /Pages /Kids [3 0 R 2 0 R] /Count 1 >>");
    pdf.object(3, "<< /Type /Page /Parent 2 0 R /Annots [11 0 R] >>");
    pdf.object(10, "<< /Kids [10 0 R 12 0 R] /Parent 12 0 R >>");
    pdf.object(11, "<< /Subtype /Widget /Parent 12 0 R >>");
    pdf.object(12, "<< /Parent 11 0 R /Kids [10 0 R] >>");
    let summary = run(&pdf.finish());
    assert_eq!(summary.pages, Some(1));
    let f = summary.features.unwrap();
    assert!(f.acroform && f.complete && !f.signature_fields);
}

#[test]
fn an_unreadable_object_is_reported_and_the_scan_is_incomplete() {
    // Object 10 is the annotation array; it does not parse.
    let data = doc("", "/Annots 10 0 R", &[(10, "[ << /Subtype /Link /A <<")]);
    let summary = run(&data);
    let f = summary.features.unwrap();
    assert!(!f.complete);
    assert!(!summary.problems.is_empty(), "{summary:?}");
}

#[test]
fn revisions_are_counted() {
    let mut data = doc("", "", &[]);
    let first = data.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
    let new_obj = data.len();
    data.extend(b"4 0 obj\n(x)\nendobj\n");
    let section = data.len();
    data.extend(format!("xref\n4 1\n{new_obj:010} 00000 n \n").bytes());
    data.extend(
        format!("trailer\n<< /Size 5 /Root 1 0 R /Prev {first} >>\nstartxref\n{section}\n%%EOF\n")
            .bytes(),
    );
    let summary = run(&data);
    assert_eq!(summary.revisions, 2);
    assert_eq!(summary.objects, 4);
    assert!(!summary.repaired);
}

#[test]
fn a_damaged_cross_reference_is_reported_as_repaired_with_reasons() {
    let mut data = doc("", "", &[]);
    // Break the startxref offset.
    let at = data.windows(9).rposition(|w| w == b"startxref").unwrap();
    data.truncate(at);
    data.extend(b"startxref\n7\n%%EOF\n");
    let summary = run(&data);
    assert!(summary.repaired);
    assert_ne!(summary.repair_reasons, Vec::<String>::new());
    assert_eq!(summary.pages, Some(1));
    assert_eq!(summary.revisions, 1);
}

#[test]
fn input_that_is_not_a_pdf_is_a_typed_error() {
    for data in [&b""[..], b"hello", b"%PDF-1.7\n", &[0xFF; 300]] {
        assert!(inspect(data, &Options::default()).is_err(), "{data:?}");
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../cos/tests/fixtures/encryption")
        .join(format!("{name}.pdf"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn options(password: &str) -> Options {
    let mut options = Options::default();
    options.password = Some(password.to_owned());
    options
}

#[test]
fn encryption_is_described() {
    let summary = run(&fixture("r3-rc4-128"));
    let e = summary.encryption.expect("encrypted");
    assert_eq!((e.handler, e.revision, e.key_bits), ("Standard", 3, 128));
    assert_eq!((e.stream_method, e.string_method), ("rc4", "rc4"));
    assert!(e.encrypt_metadata);
    assert_eq!(e.unlocked_with, Some("user"));
    assert!(!summary.locked);
    assert!(summary.pages.is_some() && summary.features.is_some());

    let e = run(&fixture("r6-aes-256")).encryption.unwrap();
    assert_eq!((e.revision, e.key_bits), (6, 256));
    assert_eq!((e.stream_method, e.string_method), ("aes-256", "aes-256"));

    let e = run(&fixture("r4-aes-128-no-metadata")).encryption.unwrap();
    assert_eq!(e.stream_method, "aes-128");
    assert!(!e.encrypt_metadata);
}

#[test]
fn a_document_that_needs_a_password_is_reported_locked() {
    let summary = run(&fixture("r4-aes-128-user-password"));
    assert!(summary.locked);
    assert_eq!(summary.pages, None);
    assert_eq!(summary.features, None);
    let e = summary.encryption.expect("encryption is still described");
    assert_eq!(e.unlocked_with, None);
    assert_eq!(e.revision, 4);
}

#[test]
fn passwords_unlock_and_wrong_ones_are_a_typed_error() {
    let data = fixture("r4-aes-128-user-password");
    let summary = inspect(&data, &options("user-pw")).unwrap();
    assert!(!summary.locked);
    assert_eq!(
        summary.encryption.as_ref().unwrap().unlocked_with,
        Some("user")
    );
    assert!(summary.pages.is_some_and(|pages| pages >= 1));
    assert!(summary.features.is_some_and(|f| f.complete));

    let summary = inspect(&data, &options("owner-pw")).unwrap();
    assert_eq!(summary.encryption.unwrap().unlocked_with, Some("owner"));

    for name in ["r4-aes-128-user-password", "r6-aes-256-user-password"] {
        let error = inspect(&fixture(name), &options("nope")).unwrap_err();
        assert!(
            matches!(
                error,
                Error::Encryption {
                    kind: EncryptionError::IncorrectPassword
                }
            ),
            "{name}: {error}"
        );
    }
}

#[test]
fn a_wrong_password_fails_even_when_the_empty_one_would_open_the_file() {
    let error = inspect(&fixture("r3-rc4-128"), &options("nope")).unwrap_err();
    assert!(matches!(
        error,
        Error::Encryption {
            kind: EncryptionError::IncorrectPassword
        }
    ));
}

#[test]
fn a_password_on_an_unencrypted_document_is_accepted() {
    let summary = inspect(&doc("", "", &[]), &options("whatever")).unwrap();
    assert!(summary.encryption.is_none());
}
