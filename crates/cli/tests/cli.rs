//! End-to-end tests for the `vellora` binary.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn vellora() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vellora"))
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("utf-8 output")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("utf-8 output")
}

#[test]
fn version_flag_prints_package_version() {
    let out = vellora().arg("--version").output().expect("run vellora");
    assert!(out.status.success());
    assert_eq!(
        stdout(&out).trim(),
        format!("vellora {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_lists_the_inspect_command() {
    let out = vellora().arg("--help").output().expect("run vellora");
    assert!(out.status.success());
    assert!(stdout(&out).contains("inspect"));
}

#[test]
fn unknown_command_is_a_usage_error() {
    let out = vellora()
        .arg("no-such-command")
        .output()
        .expect("run vellora");
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("'no-such-command'"));
}

#[test]
fn no_arguments_is_a_usage_error() {
    let out = vellora().output().expect("run vellora");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn inspect_without_a_file_is_a_usage_error() {
    let out = vellora().arg("inspect").output().expect("run vellora");
    assert_eq!(out.status.code(), Some(2));
}

// --- vellora inspect ---------------------------------------------------------------------

/// Builds a file with a correct classic cross-reference table.
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

/// A one-page document with `catalog_extra` in the catalog, `page_extra` in the page and the
/// given further objects.
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

/// A document whose only page has one annotation with the given entries.
fn annot(entries: &str) -> Vec<u8> {
    doc(
        "",
        "/Annots [10 0 R]",
        &[(10, &format!("<< /Type /Annot {entries} >>"))],
    )
}

/// Writes `data` to a new temporary directory.
fn write(data: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.pdf");
    std::fs::write(&path, data).unwrap();
    (dir, path)
}

fn run_inspect(path: &Path, args: &[&str]) -> Output {
    vellora()
        .arg("inspect")
        .arg(path)
        .args(args)
        .output()
        .expect("run vellora")
}

/// The `--json` report of `data`, which must succeed.
fn report(data: &[u8]) -> Value {
    let (_dir, path) = write(data);
    let out = run_inspect(&path, &["--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    serde_json::from_str(&stdout(&out)).expect("valid JSON")
}

/// The names of the features that are `true`, without `complete`.
fn flags(report: &Value) -> Vec<String> {
    let mut names: Vec<String> = report["features"]
        .as_object()
        .expect("features object")
        .iter()
        .filter(|&(name, value)| name != "complete" && value == &Value::Bool(true))
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

fn cos_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../cos/tests/fixtures/encryption")
        .join(format!("{name}.pdf"))
}

#[test]
fn every_feature_flag_is_set_by_its_fixture() {
    let cases: Vec<(&str, Vec<u8>, &[&str])> = vec![
        (
            "JavaScript on open",
            doc("/OpenAction << /S /JavaScript /JS (x) >>", "", &[]),
            &["javascript_actions", "open_action_javascript"],
        ),
        (
            "JavaScript action",
            annot("/Subtype /Link /A << /S /JavaScript /JS (x) >>"),
            &["javascript_actions"],
        ),
        (
            "document-level JavaScript",
            doc("/Names << /JavaScript << /Names [] >> >>", "", &[]),
            &["names_javascript"],
        ),
        (
            "additional actions",
            doc("", "/AA << /O << /S /Hide >> >>", &[]),
            &["additional_actions"],
        ),
        (
            "launch",
            annot("/Subtype /Link /A << /S /Launch /F (calc.exe) >>"),
            &["launch_actions"],
        ),
        (
            "uri",
            annot("/Subtype /Link /A << /S /URI /URI (http://example.com) >>"),
            &["uri_actions"],
        ),
        (
            "submit form",
            annot("/Subtype /Link /A << /S /SubmitForm /F (http://example.com) >>"),
            &["submit_form_actions"],
        ),
        (
            "go to remote",
            annot("/Subtype /Link /A << /S /GoToR /F (other.pdf) /D [0 /Fit] >>"),
            &["goto_remote_actions"],
        ),
        (
            "embedded files tree",
            doc("/Names << /EmbeddedFiles << /Names [] >> >>", "", &[]),
            &["embedded_files"],
        ),
        (
            "file attachment annotation",
            annot("/Subtype /FileAttachment"),
            &["embedded_files"],
        ),
        (
            "acroform",
            doc("/AcroForm << /Fields [] >>", "", &[]),
            &["acroform"],
        ),
        (
            "xfa",
            doc("/AcroForm << /Fields [] /XFA [] >>", "", &[]),
            &["acroform", "xfa"],
        ),
        (
            "optional content",
            doc("/OCProperties << /OCGs [] /D << >> >>", "", &[]),
            &["optional_content"],
        ),
        (
            "signature field",
            doc(
                "/AcroForm << /Fields [10 0 R] >>",
                "",
                &[(10, "<< /FT /Sig /T (s) >>")],
            ),
            &["acroform", "signature_fields"],
        ),
    ];
    for (name, data, expected) in cases {
        let report = report(&data);
        assert_eq!(flags(&report), expected, "{name}");
        assert_eq!(report["features"]["complete"], Value::Bool(true), "{name}");
    }
}

#[test]
fn a_plain_document_has_no_flags() {
    let report = report(&doc("", "", &[]));
    assert_eq!(flags(&report), Vec::<String>::new());
    assert_eq!(report["pages"], 1);
    assert_eq!(report["version"], "1.7");
}

#[test]
fn json_report_of_a_plain_document() {
    let out = {
        let (_dir, path) = write(&doc("", "", &[]));
        run_inspect(&path, &["--json"])
    };
    insta::assert_snapshot!("plain_json", stdout(&out));
}

#[test]
fn json_report_of_a_document_with_many_features() {
    let data = doc(
        "/OpenAction 10 0 R /Names << /EmbeddedFiles << /Names [] >> >> \
         /AcroForm << /Fields [11 0 R] /XFA [] >> /OCProperties << /OCGs [] /D << >> >>",
        "/Annots [12 0 R]",
        &[
            (
                10,
                "<< /S /JavaScript /JS (x) /Next << /S /Launch /F (a) >> >>",
            ),
            (11, "<< /FT /Sig /T (s) >>"),
            (
                12,
                "<< /Subtype /Link /A << /S /URI /URI (http://example.com) >> >>",
            ),
        ],
    );
    let (_dir, path) = write(&data);
    insta::assert_snapshot!(
        "many_features_json",
        stdout(&run_inspect(&path, &["--json"]))
    );
}

#[test]
fn json_report_of_a_repaired_document() {
    let mut data = doc("", "", &[]);
    let at = data.windows(9).rposition(|w| w == b"startxref").unwrap();
    data.truncate(at);
    data.extend(b"startxref\n7\n%%EOF\n");
    let (_dir, path) = write(&data);
    let out = run_inspect(&path, &["--json"]);
    assert_eq!(out.status.code(), Some(0));
    insta::assert_snapshot!("repaired_json", stdout(&out));
}

#[test]
fn json_report_of_an_encrypted_document_that_needs_a_password() {
    let out = run_inspect(&cos_fixture("r4-aes-128-user-password"), &["--json"]);
    // The report is still made; the note about the password goes to stderr.
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stderr(&out).contains("--password"));
    insta::assert_snapshot!("locked_json", stdout(&out));
}

#[test]
fn text_report() {
    let data = doc(
        "/OpenAction << /S /JavaScript /JS (x) >>",
        "/Annots [10 0 R]",
        &[(
            10,
            "<< /Subtype /Link /A << /S /URI /URI (http://example.com) >> >>",
        )],
    );
    let (_dir, path) = write(&data);
    let out = run_inspect(&path, &[]);
    assert_eq!(out.status.code(), Some(0));
    // The file name is the only part that depends on the machine.
    let text = stdout(&out).replace(&path.display().to_string(), "<file>");
    insta::assert_snapshot!("text_report", text);
}

#[test]
fn text_report_of_an_encrypted_document() {
    let out = run_inspect(&cos_fixture("r3-rc4-128"), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("Standard handler, V2 R3, 128-bit key"),
        "{text}"
    );
    assert!(text.contains("Unlocked with user"), "{text}");
}

#[test]
fn passwords_unlock_and_a_wrong_one_fails() {
    let path = cos_fixture("r6-aes-256-user-password");

    let out = run_inspect(&path, &["--json", "--password", "user-pw"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let report: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["locked"], false);
    assert_eq!(report["encryption"]["unlocked_with"], "user");
    assert!(report["pages"].is_u64());

    let out = run_inspect(&path, &["--json", "--password", "owner-pw"]);
    let report: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["encryption"]["unlocked_with"], "owner");

    let out = run_inspect(&path, &["--json", "--password", "wrong"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert!(
        stderr(&out).contains("incorrect password"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_missing_file_fails_with_exit_code_1() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_inspect(&dir.path().join("missing.pdf"), &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).starts_with("vellora: "), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

#[test]
fn a_file_that_is_not_a_pdf_fails_with_exit_code_1() {
    for bytes in [&b""[..], b"just some text", &[0u8; 100]] {
        let (_dir, path) = write(bytes);
        for args in [&[][..], &["--json"][..]] {
            let out = run_inspect(&path, args);
            assert_eq!(out.status.code(), Some(1), "{bytes:?}");
            assert_eq!(stdout(&out), "");
            assert_ne!(stderr(&out), "");
        }
    }
}
