//! End-to-end tests of `vellora security`, `vellora render` and `vellora text` (M1 task 19).
//!
//! `render` and `text` start the real engine, so the engine executable and PDFium must exist: the
//! engine is looked for next to the `vellora` binary (it is built by `cargo test --workspace`, or
//! `cargo build -p vellora-engine`) and PDFium in `third_party/` (`cargo xtask pdfium fetch`). They
//! fail, never skip, when either is missing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // test sizes are tiny

mod common;

use common::{plain, read_png, root, stderr, stdout, with_engine};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value;

const GOLDEN_PDF: &str = "../engine/tests/fixtures/golden.pdf";

// ---- documents -------------------------------------------------------------------------------

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

/// A one-page document with extra catalog and page entries and further objects.
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

fn write(name: &str, data: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, data).unwrap();
    (dir, path)
}

fn cos_fixture(name: &str) -> PathBuf {
    root("../cos/tests/fixtures/encryption").join(name)
}

// ---- vellora security ------------------------------------------------------------------------

fn security(path: &Path, args: &[&str]) -> Output {
    plain()
        .arg("security")
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn a_document_without_risky_features_exits_0() {
    let (_dir, path) = write("plain.pdf", &doc("", "", &[]));
    let out = security(&path, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("Verdict  no risky features found"));
    assert!(!stdout(&out).contains("Findings"));
}

#[test]
fn risky_features_exit_3_and_are_explained() {
    let data = doc(
        "/OpenAction 10 0 R /Names << /EmbeddedFiles 11 0 R >>",
        "/Annots [12 0 R]",
        &[
            (10, "<< /S /JavaScript /JS (app.alert\\(1\\)) >>"),
            (11, "<< /Names [(payload.exe) 13 0 R] >>"),
            (
                12,
                "<< /Type /Annot /Subtype /Link /A << /S /Launch /F (calc.exe) >> >>",
            ),
            (13, "<< >>"),
        ],
    );
    let (_dir, path) = write("risky.pdf", &data);
    let out = security(&path, &[]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    insta::assert_snapshot!(
        "security_risky_text",
        stdout(&out).replace(path.to_str().unwrap(), "<file>")
    );

    let out = security(&path, &["--json"]);
    assert_eq!(out.status.code(), Some(3));
    let report: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["risky"], true);
    assert_eq!(report["complete"], true);
    let ids: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "open_action_javascript",
            "javascript_actions",
            "launch_actions",
            "embedded_files"
        ]
    );
    assert_eq!(report["embedded_files"]["items"][0], "payload.exe");
    for finding in report["findings"].as_array().unwrap() {
        assert_ne!(finding["explanation"].as_str().unwrap(), "");
    }
}

#[test]
fn links_alone_are_noted_but_not_risky() {
    let data = doc(
        "",
        "/Annots [10 0 R]",
        &[(
            10,
            "<< /Type /Annot /Subtype /Link /A << /S /URI /URI (https://example.org/a) >> >>",
        )],
    );
    let (_dir, path) = write("links.pdf", &data);
    let out = security(&path, &[]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(
        text.contains("Also present\n  Links to addresses"),
        "{text}"
    );
    assert!(
        text.contains("Addresses in links (1)\n  https://example.org/a"),
        "{text}"
    );
    let json: Value = serde_json::from_str(&stdout(&security(&path, &["--json"]))).unwrap();
    assert_eq!(json["risky"], false);
    assert_eq!(json["findings"].as_array().unwrap().len(), 0);
    assert_eq!(json["notes"][0]["id"], "uri_actions");
}

#[test]
fn a_scan_that_could_not_read_everything_does_not_say_all_is_well() {
    // A catalog whose dictionary has a key that is not a name cannot be read: nothing is found,
    // but only because nothing could be read.
    let data = doc("1 2", "", &[]);
    let (_dir, path) = write("unreadable.pdf", &data);
    let json = security(&path, &["--json"]);
    let report: Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(report["complete"], false, "{report}");
    assert_eq!(report["risky"], false);
    assert_eq!(json.status.code(), Some(1), "{}", stderr(&json));
    assert!(stderr(&json).contains("incomplete"), "{}", stderr(&json));
    let text = security(&path, &[]);
    assert_eq!(text.status.code(), Some(1));
    assert!(
        stdout(&text).contains("cannot be promised"),
        "{}",
        stdout(&text)
    );
}

#[test]
fn text_from_the_file_cannot_rewrite_the_line() {
    // An address with an escape, a right-to-left override and a line separator in it.
    let data = doc(
        "",
        "/Annots [10 0 R]",
        &[(
            10,
            "<< /Type /Annot /Subtype /Link /A << /S /URI /URI <EFBBBF68747470733A2F2F612E6578616D706C65E280AE2F6578E280A82F> >> >>",
        )],
    );
    let (_dir, path) = write("spoof.pdf", &data);
    let text = stdout(&security(&path, &[]));
    let line = text
        .lines()
        .find(|l| l.contains("https://a.example"))
        .expect("the address is listed");
    assert!(
        !line.contains('\u{202E}') && !line.contains('\u{2028}'),
        "{line:?}"
    );
}

#[test]
fn an_encrypted_document_needs_its_password_to_be_assessed() {
    let path = cos_fixture("r6-aes-256-user-password.pdf");
    let out = security(&path, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--password"), "{}", stderr(&out));
    let out = security(&path, &["--password", "user-pw"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = security(&path, &["--password", "wrong"]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn security_errors_and_usage() {
    let out = security(Path::new("/no/such/file.pdf"), &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).starts_with("vellora: "));
    let (_dir, path) = write("junk.pdf", b"this is not a pdf");
    assert_eq!(security(&path, &[]).status.code(), Some(1));
    let out = plain().arg("security").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

// ---- vellora text ----------------------------------------------------------------------------

fn text(args: &[&str]) -> Output {
    with_engine().arg("text").args(args).output().unwrap()
}

/// Three pages of Helvetica text: two lines on the first, one on the others.
fn text_document() -> (tempfile::TempDir, PathBuf) {
    let contents = [
        "BT /F1 24 Tf 72 700 Td (First page, first line) Tj 0 -40 Td (Second line of page one) Tj ET",
        "BT /F1 24 Tf 72 700 Td (Page two) Tj ET",
        "BT /F1 24 Tf 72 700 Td (The last page) Tj ET",
    ];
    let mut pdf = Pdf::new();
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    pdf.object(2, "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>");
    for (n, _) in contents.iter().enumerate() {
        pdf.object(
            3 + n as u32,
            &format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R /Resources << /Font << /F1 9 0 R >> >> >>",
                6 + n
            ),
        );
    }
    for (n, content) in contents.iter().enumerate() {
        pdf.object(
            6 + n as u32,
            &format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        );
    }
    pdf.object(
        9,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    );
    write("text.pdf", &pdf.finish())
}

#[test]
fn the_text_of_the_pages_is_printed_with_a_form_feed_between_them() {
    let (_dir, path) = text_document();
    let out = text(&[path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let all = stdout(&out);
    let pages: Vec<&str> = all.split('\u{c}').collect();
    assert_eq!(pages.len(), 3, "three pages: {all:?}");
    // Line ends are \n, never CR LF; the order of the lines is the reading order.
    assert!(!all.contains('\r'));
    assert!(pages[0].starts_with("First page, first line"), "{all:?}");
    assert!(pages[0].contains("\nSecond line of page one"), "{all:?}");
    assert_eq!(pages[1].trim_end(), "Page two");
    assert_eq!(pages[2].trim_end(), "The last page");
    insta::assert_snapshot!("text_all_pages", all.replace('\u{c}', "<FF>"));

    // A selection is in the order given, a page once.
    let out = text(&[path.to_str().unwrap(), "--pages", "3,1,3"]);
    assert_eq!(out.status.code(), Some(0));
    let picked = stdout(&out);
    let parts: Vec<&str> = picked.split('\u{c}').collect();
    assert_eq!(parts, [pages[2], pages[0]]);
    // One page has no form feed.
    let out = text(&[path.to_str().unwrap(), "--pages", "2"]);
    assert_eq!(stdout(&out), pages[1]);
    // An open range.
    let out = text(&[path.to_str().unwrap(), "--pages", "2-"]);
    assert_eq!(stdout(&out).split('\u{c}').count(), 2);
}

#[test]
fn text_refuses_a_page_that_is_not_there_or_not_a_page() {
    let (_dir, path) = text_document();
    for (spec, message) in [
        ("4", "page 4 does not exist: the document has 3"),
        ("0", "pages count from 1"),
        ("3-1", "ends before it starts"),
        ("x", "is not a page"),
    ] {
        let out = text(&[path.to_str().unwrap(), "--pages", spec]);
        assert_eq!(out.status.code(), Some(1), "{spec}");
        assert!(stderr(&out).contains(message), "{spec}: {}", stderr(&out));
        assert_eq!(stdout(&out), "");
    }
}

#[test]
fn text_of_a_protected_document_needs_the_password() {
    let path = cos_fixture("r6-aes-256-user-password.pdf");
    let out = text(&[path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--password"), "{}", stderr(&out));
    let out = text(&[path.to_str().unwrap(), "--password", "wrong"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("not correct"), "{}", stderr(&out));
    let out = text(&[path.to_str().unwrap(), "--password", "user-pw"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn text_without_the_engine_says_where_it_looked() {
    let out = plain()
        .env_remove("VELLORA_ENGINE")
        .env("VELLORA_ENGINE", "/no/such/engine")
        .args(["text"])
        .arg(root(GOLDEN_PDF))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_ne!(stderr(&out), "");
}

// ---- vellora render --------------------------------------------------------------------------

fn render(args: &[&str]) -> Output {
    with_engine().arg("render").args(args).output().unwrap()
}

/// The measure of the render crate's golden test: the mean absolute channel difference, and the
/// share of pixels where a channel is more than 32 levels off.
fn difference(actual: &[u8], golden: &[u8]) -> (f64, f64) {
    assert_eq!(actual.len(), golden.len());
    let total: u64 = actual
        .iter()
        .zip(golden)
        .map(|(&a, &g)| u64::from(a.abs_diff(g)))
        .sum();
    let loud = actual
        .as_chunks::<3>()
        .0
        .iter()
        .zip(golden.as_chunks::<3>().0)
        .filter(|(a, g)| a.iter().zip(g.iter()).any(|(&a, &g)| a.abs_diff(g) > 32))
        .count();
    (
        total as f64 / actual.len() as f64,
        loud as f64 / (actual.len() / 3) as f64,
    )
}

#[test]
fn a_page_rendered_by_the_cli_matches_the_golden_image() {
    let golden = root(GOLDEN_PDF);
    let dir = tempfile::tempdir().unwrap();
    // The golden images are drawn at 1.5 pixels to the point: 108 dpi.
    let out = render(&[
        golden.to_str().unwrap(),
        "--pages",
        "1-3",
        "--dpi",
        "108",
        "-o",
        dir.path().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let written: Vec<String> = stdout(&out)
        .lines()
        .map(|l| {
            Path::new(l)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(written, ["page-1.png", "page-2.png", "page-3.png"]);
    for page in 1..=3 {
        let (width, height, actual) = read_png(&dir.path().join(format!("page-{page}.png")));
        let (golden_width, golden_height, expected) =
            read_png(&root(&format!("../render/testdata/golden/page-{page}.png")));
        assert_eq!(
            (width, height),
            (golden_width, golden_height),
            "page {page}"
        );
        let (mean, loud) = difference(&actual, &expected);
        assert!(mean <= 0.5, "page {page}: mean difference {mean}");
        assert!(loud <= 0.005, "page {page}: {loud} of the pixels are off");
    }
}

#[test]
fn a_page_larger_than_a_tile_is_put_together_from_tiles() {
    // 150 dpi: the first page is 458 x 292 pixels (one tile); 400 dpi needs several.
    let golden = root(GOLDEN_PDF);
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("big.png");
    let out = render(&[
        golden.to_str().unwrap(),
        "--dpi",
        "400",
        "-o",
        target.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let (width, height, pixels) = read_png(&target);
    assert!(width > 512 && height > 512, "{width} x {height}");
    // No seam: the page is white where it has no ink, in every tile.
    let white = pixels
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p.iter().all(|&c| c > 250))
        .count();
    assert!(white as f64 / (f64::from(width) * f64::from(height)) > 0.5);
    // Smaller at lower resolution, by the ratio.
    let small = dir.path().join("small.png");
    let out = render(&[
        golden.to_str().unwrap(),
        "--dpi",
        "100",
        "-o",
        small.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0));
    let (small_width, _, _) = read_png(&small);
    let ratio = f64::from(width) / f64::from(small_width);
    assert!((ratio - 4.0).abs() < 0.1, "{ratio}");
}

#[test]
fn render_says_what_is_wrong_with_the_request() {
    let golden = root(GOLDEN_PDF);
    let g = golden.to_str().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_str().unwrap();
    let file = dir.path().join("one.png");
    let f = file.to_str().unwrap();
    for (args, message) in [
        (vec![g, "--dpi", "0", "-o", d], "--dpi must be between"),
        (vec![g, "--dpi", "99999", "-o", d], "--dpi"),
        (vec![g, "--pages", "9", "-o", d], "page 9 does not exist"),
        (vec![g, "--pages", "1-2", "-o", f], "give a folder"),
    ] {
        let out = render(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(stderr(&out).contains(message), "{args:?}: {}", stderr(&out));
    }
    // A file that is not there, and the output missing altogether (a usage error).
    let out = render(&["/no/such/file.pdf", "-o", d]);
    assert_eq!(out.status.code(), Some(1));
    let out = render(&[g]);
    assert_eq!(out.status.code(), Some(2));
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
}

#[test]
fn a_protected_document_is_rendered_with_its_password() {
    let path = cos_fixture("r6-aes-256-user-password.pdf");
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_str().unwrap();
    let out = render(&[path.to_str().unwrap(), "-o", d]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--password"));
    let out = render(&[path.to_str().unwrap(), "--password", "user-pw", "-o", d]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(dir.path().join("page-1.png").is_file());
}

#[test]
fn help_lists_the_new_commands() {
    let out = plain().arg("--help").output().unwrap();
    let help = stdout(&out);
    for command in ["inspect", "security", "render", "text"] {
        assert!(help.contains(command), "{command} is not in the help");
    }
}
