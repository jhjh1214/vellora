//! The rendering regression suite (M1 task 20): the first two pages of ~40 corpus documents drawn
//! by `vellora render` at 96 dpi and compared with golden images.
//!
//! The goldens are not in the repository. `cargo xtask corpus fetch --manifest
//! tests/corpus/golden-manifest.toml` and `cargo xtask goldens fetch` bring the documents and the
//! goldens, and the suite is run with
//!
//! ```sh
//! cargo test -p vellora-cli --test goldens -- --ignored --nocapture
//! ```
//!
//! It fails, never skips, without them. How far a page may differ is in `tests/goldens/tolerance.toml`,
//! per operating system. `a_broken_render_fails_the_suite` proves the comparison is not vacuous: it
//! runs on the small golden document of the repository and needs nothing fetched.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // image sizes are small

mod common;

use std::path::{Path, PathBuf};

use common::{read_png, root, stderr, stdout, with_engine};

/// How far a rendering may be from its golden.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tolerance {
    /// A pixel differs when any channel is further than this from the golden.
    pixel_delta: u8,
    /// The most pixels that may differ, as a share of the page.
    max_ratio: f64,
}

fn tolerance() -> Tolerance {
    let text = std::fs::read_to_string(root("../../tests/goldens/tolerance.toml")).unwrap();
    let table: toml::Table = text.parse().unwrap();
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let entry = table[os].as_table().unwrap();
    Tolerance {
        pixel_delta: u8::try_from(entry["pixel_delta"].as_integer().unwrap()).unwrap(),
        max_ratio: entry["max_ratio"].as_float().unwrap(),
    }
}

/// An RGB image.
#[derive(Clone)]
struct Image {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl Image {
    fn load(path: &Path) -> Self {
        let (width, height, rgb) = read_png(path);
        Self { width, height, rgb }
    }
}

/// What comparing a page with its golden found.
#[derive(Debug, PartialEq)]
struct Verdict {
    same_size: bool,
    differing: usize,
    ratio: f64,
    worst: u8,
}

impl Verdict {
    fn passes(&self, tolerance: Tolerance) -> bool {
        self.same_size && self.ratio <= tolerance.max_ratio
    }
}

fn compare(actual: &Image, golden: &Image, tolerance: Tolerance) -> Verdict {
    if (actual.width, actual.height) != (golden.width, golden.height) {
        return Verdict {
            same_size: false,
            differing: usize::MAX,
            ratio: 1.0,
            worst: 255,
        };
    }
    let mut differing = 0usize;
    let mut worst = 0u8;
    for (a, g) in actual
        .rgb
        .as_chunks::<3>()
        .0
        .iter()
        .zip(golden.rgb.as_chunks::<3>().0)
    {
        let delta = a
            .iter()
            .zip(g)
            .map(|(&a, &g)| a.abs_diff(g))
            .max()
            .unwrap_or(0);
        worst = worst.max(delta);
        if delta > tolerance.pixel_delta {
            differing += 1;
        }
    }
    let pixels = (actual.rgb.len() / 3).max(1);
    Verdict {
        same_size: true,
        differing,
        ratio: differing as f64 / pixels as f64,
        worst,
    }
}

/// Ways to break a rendering, applied to the pixels before they are compared: the test-only knob
/// that shows the suite notices a renderer that goes wrong.
#[derive(Debug, Clone, Copy)]
enum Break {
    /// Everything moved two pixels to the right.
    Shifted,
    /// A corner of the page painted in the wrong colours.
    InvertedCorner,
    /// A blank page.
    Blank,
    /// The page drawn at a slightly different resolution (the image is a different size).
    Resized,
}

fn broken(image: &Image, how: Break) -> Image {
    let mut out = image.clone();
    let (width, height) = (image.width as usize, image.height as usize);
    match how {
        Break::Shifted => {
            for y in 0..height {
                for x in 0..width {
                    let from = x.saturating_sub(2);
                    let src = (y * width + from) * 3;
                    let dst = (y * width + x) * 3;
                    out.rgb[dst..dst + 3].copy_from_slice(&image.rgb[src..src + 3]);
                }
            }
        }
        Break::InvertedCorner => {
            for y in 0..height / 3 {
                for x in 0..width / 3 {
                    for c in 0..3 {
                        let i = (y * width + x) * 3 + c;
                        out.rgb[i] = 255 - out.rgb[i];
                    }
                }
            }
        }
        Break::Blank => out.rgb.fill(255),
        Break::Resized => {
            out.width += 1;
            out.rgb.extend([255, 255, 255].repeat(out.height as usize));
            // Row by row: the extra column on each row.
            let mut rows = Vec::with_capacity(out.rgb.len());
            for y in 0..height {
                rows.extend(&image.rgb[y * width * 3..(y + 1) * width * 3]);
                rows.extend([255u8, 255, 255]);
            }
            out.rgb = rows;
        }
    }
    out
}

/// Draws `pages` of `pdf` at `dpi` with the CLI; the files written, in page order.
fn render(pdf: &Path, pages: &str, dpi: u32, dir: &Path) -> Vec<PathBuf> {
    let out = with_engine()
        .arg("render")
        .arg(pdf)
        .args(["--pages", pages, "--dpi", &dpi.to_string(), "-o"])
        .arg(dir)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}: {}",
        pdf.display(),
        stderr(&out)
    );
    stdout(&out).lines().map(PathBuf::from).collect()
}

// ---- the suite -----------------------------------------------------------------------------

#[test]
#[ignore = "needs the corpus subset and the goldens; run with --ignored (see the top of the file)"]
fn the_rendering_matches_the_goldens() {
    let manifest_text = std::fs::read_to_string(root("../../tests/goldens/manifest.toml")).unwrap();
    let manifest: toml::Table = manifest_text.parse().unwrap();
    let dpi = u32::try_from(manifest["dpi"].as_integer().unwrap()).unwrap();
    let docs = manifest["doc"].as_array().unwrap();
    let corpus = root("../../tests/corpus-data");
    let goldens = root("../../tests/golden-data");
    let tolerance = tolerance();
    println!("tolerance: {tolerance:?}");

    let scratch = tempfile::tempdir().unwrap();
    let (mut pages_compared, mut failures) = (0usize, Vec::new());
    let mut worst_ratio = 0f64;
    for doc in docs {
        let id = doc["id"].as_str().unwrap();
        let pages = u32::try_from(doc["pages"].as_integer().unwrap()).unwrap();
        let pdf = corpus.join(format!("{id}.pdf"));
        assert!(
            pdf.is_file(),
            "{} is missing: run `cargo xtask corpus fetch --manifest tests/corpus/golden-manifest.toml`",
            pdf.display()
        );
        let files = render(&pdf, &format!("1-{pages}"), dpi, &scratch.path().join(id));
        assert_eq!(files.len(), pages as usize, "{id}");
        for (page, file) in (1..).zip(&files) {
            let golden_path = goldens.join(format!("{id}.p{page}.png"));
            assert!(
                golden_path.is_file(),
                "{} is missing: run `cargo xtask goldens fetch`",
                golden_path.display()
            );
            let verdict = compare(&Image::load(file), &Image::load(&golden_path), tolerance);
            pages_compared += 1;
            worst_ratio = worst_ratio.max(verdict.ratio);
            // The share of differing pixels at other thresholds, to choose a tolerance from.
            let at = |delta: u8| {
                compare(
                    &Image::load(file),
                    &Image::load(&golden_path),
                    Tolerance {
                        pixel_delta: delta,
                        ..tolerance
                    },
                )
                .ratio
            };
            println!(
                "{id} p{page}: {} of the pixels differ ({} pixels, worst channel {}); at delta 16/32/64/96/128/192: {:.5} {:.5} {:.5} {:.5} {:.5} {:.5}",
                verdict.ratio,
                verdict.differing,
                verdict.worst,
                at(16),
                at(32),
                at(64),
                at(96),
                at(128),
                at(192)
            );
            if !verdict.passes(tolerance) {
                failures.push(format!(
                    "{id} p{page}: same size {}, {:.4} of the pixels differ (limit {})",
                    verdict.same_size, verdict.ratio, tolerance.max_ratio
                ));
            }
        }
    }
    println!("{pages_compared} pages compared, worst share of differing pixels {worst_ratio}");
    let expected: usize = docs
        .iter()
        .map(|d| usize::try_from(d["pages"].as_integer().unwrap()).unwrap())
        .sum();
    assert_eq!(pages_compared, expected);
    assert!(pages_compared >= 40, "the suite holds about 40 documents");
    assert!(
        failures.is_empty(),
        "{} page(s) differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The comparison is not vacuous: the renderings of the repository's golden document pass, and
/// each way of breaking them fails. No fetched data is needed.
#[test]
fn a_broken_render_fails_the_suite() {
    let tolerance = tolerance();
    let pdf = root("../engine/tests/fixtures/golden.pdf");
    let scratch = tempfile::tempdir().unwrap();
    // The images of the render crate are drawn at 1.5 pixels to the point: 108 dpi.
    let files = render(&pdf, "1-3", 108, scratch.path());
    assert_eq!(files.len(), 3);
    for (page, file) in (1..).zip(&files) {
        let golden = Image::load(&root(&format!("../render/testdata/golden/page-{page}.png")));
        let rendered = Image::load(file);
        let honest = compare(&rendered, &golden, tolerance);
        assert!(honest.passes(tolerance), "page {page}: {honest:?}");
        for how in [
            Break::Shifted,
            Break::InvertedCorner,
            Break::Blank,
            Break::Resized,
        ] {
            let verdict = compare(&broken(&rendered, how), &golden, tolerance);
            assert!(
                !verdict.passes(tolerance),
                "page {page}: {how:?} was not noticed: {verdict:?}"
            );
        }
    }
}

#[test]
fn the_tolerances_are_set_for_every_system_and_are_strict_enough_to_mean_something() {
    let text = std::fs::read_to_string(root("../../tests/goldens/tolerance.toml")).unwrap();
    let table: toml::Table = text.parse().unwrap();
    for os in ["windows", "linux", "macos"] {
        let entry = table[os].as_table().unwrap();
        let delta = entry["pixel_delta"].as_integer().unwrap();
        let ratio = entry["max_ratio"].as_float().unwrap();
        assert!((1..=96).contains(&delta), "{os}: pixel_delta {delta}");
        assert!(ratio > 0.0 && ratio <= 0.05, "{os}: max_ratio {ratio}");
    }
}
