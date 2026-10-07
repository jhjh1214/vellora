// Test arithmetic on small, known values (tile sizes, pixel counts, scales).
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::PathBuf;
use std::thread;

use super::*;
use crate::test_support::{
    SAMPLE_PAGE_SIZES, golden_pdf, library_path, sample_pdf, serial, with_renderer,
};

const BLACK: [u8; 4] = [0, 0, 0, 0];
const WHITE: [u8; 4] = [255, 255, 255, 0];

fn request(page: usize, scale: f32, (x, y, width, height): (u32, u32, u32, u32)) -> TileRequest {
    TileRequest {
        page,
        scale,
        rect: TileRect {
            x,
            y,
            width,
            height,
        },
    }
}

/// Renders into a tightly packed buffer.
fn render(doc: &DocHandle, request: TileRequest) -> Vec<u8> {
    let (width, height) = (request.rect.width as usize, request.rect.height as usize);
    let mut buffer = vec![0xEE; width * height * 4];
    doc.render_tile(request, &mut buffer, width * 4).unwrap();
    buffer
}

/// The pixel at (`x`, `y`) of a tightly packed tile `width` pixels wide, as B, G, R, unused. The
/// fourth byte is whatever PDFium leaves there, so compare three channels.
fn pixel(buffer: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
    let at = (y * width + x) * 4;
    [buffer[at], buffer[at + 1], buffer[at + 2]]
}

fn bgr(color: [u8; 4]) -> [u8; 3] {
    [color[0], color[1], color[2]]
}

#[test]
fn opens_a_document_and_reports_its_pages() {
    let pdf = sample_pdf();
    with_renderer(|renderer| {
        let doc = renderer.open(pdf).unwrap();
        assert_eq!(doc.page_count(), 3);
        for (index, (width, height)) in SAMPLE_PAGE_SIZES.into_iter().enumerate() {
            assert_eq!(doc.page_size(index).unwrap(), (width, height));
        }
        let err = doc.page_size(3).unwrap_err();
        assert!(
            matches!(err, Error::PageOutOfRange { page: 3, count: 3 }),
            "{err:?}"
        );
    });
}

#[test]
fn a_rotated_page_reports_its_displayed_size() {
    with_renderer(|renderer| {
        let doc = renderer.open(golden_pdf()).unwrap();
        assert_eq!(doc.page_size(0).unwrap(), (220.0, 140.0));
        assert_eq!(
            doc.page_size(1).unwrap(),
            (200.0, 160.0),
            "160 x 200, /Rotate 90"
        );
    });
}

#[test]
fn a_document_that_is_not_a_pdf_is_a_typed_error_and_the_renderer_survives() {
    with_renderer(|renderer| {
        let err = renderer.open(b"this is not a PDF".to_vec()).err().unwrap();
        assert!(matches!(err, Error::Open(LastError::Format)), "{err:?}");
        let doc = renderer.open(sample_pdf()).unwrap();
        assert_eq!(doc.page_count(), 3);
    });
}

#[test]
fn renders_the_page_through_scale_and_tile_origin() {
    with_renderer(|renderer| {
        let doc = renderer.open(sample_pdf()).unwrap();
        // Page 0 is 200 x 100 with a black rectangle at (20,20)-(180,80); at scale 2 it covers
        // 40..360 x 40..160 of a 400 x 200 image.
        let full = render(&doc, request(0, 2.0, (0, 0, 400, 200)));
        assert_eq!(pixel(&full, 400, 200, 100), bgr(BLACK));
        assert_eq!(pixel(&full, 400, 20, 20), bgr(WHITE));
        assert_eq!(pixel(&full, 400, 380, 180), bgr(WHITE));

        // A tile is exactly the crop of the full render, wherever it sits.
        for rect in [(0, 0, 100, 100), (30, 50, 200, 80), (300, 100, 100, 100)] {
            let tile = render(&doc, request(0, 2.0, rect));
            let (x, y, width, height) = rect;
            for row in 0..height as usize {
                let from = ((y as usize + row) * 400 + x as usize) * 4;
                let expected = &full[from..from + width as usize * 4];
                let got = &tile[row * width as usize * 4..][..width as usize * 4];
                assert_eq!(got, expected, "tile {rect:?}, row {row}");
            }
        }

        // Beyond the page there is only white.
        let outside = render(&doc, request(0, 1.0, (500, 500, 16, 16)));
        assert!(
            outside
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| bgr(WHITE) == [p[0], p[1], p[2]])
        );
    });
}

#[test]
fn rows_are_written_at_the_stride_and_padding_is_untouched() {
    with_renderer(|renderer| {
        let doc = renderer.open(sample_pdf()).unwrap();
        let (width, height, stride) = (100_usize, 50_usize, 100 * 4 + 24);
        let mut padded = vec![0xAB; stride * height];
        let rect = (50, 20, 100, 50);
        doc.render_tile(request(0, 1.0, rect), &mut padded, stride)
            .unwrap();
        let packed = render(&doc, request(0, 1.0, rect));
        for row in 0..height {
            assert_eq!(
                &padded[row * stride..][..width * 4],
                &packed[row * width * 4..][..width * 4],
                "row {row}"
            );
            if row + 1 < height {
                assert!(
                    padded[row * stride + width * 4..(row + 1) * stride]
                        .iter()
                        .all(|&b| b == 0xAB),
                    "padding after row {row} was written"
                );
            }
        }
        // The last row needs no padding: a buffer that ends right after its pixels is enough.
        let mut tight = vec![0xAB; stride * (height - 1) + width * 4];
        doc.render_tile(request(0, 1.0, rect), &mut tight, stride)
            .unwrap();
        assert_eq!(&tight[..padded.len() - 24], &padded[..padded.len() - 24]);
    });
}

#[test]
fn bad_requests_are_rejected_and_leave_the_buffer_alone() {
    with_renderer(|renderer| {
        let doc = renderer.open(sample_pdf()).unwrap();
        let mut buffer = vec![0x5A; 64 * 64 * 4];
        let ok = request(0, 1.0, (0, 0, 64, 64));
        let invalid = |request: TileRequest, len: usize, stride: usize, buffer: &mut [u8]| {
            let err = doc
                .render_tile(request, &mut buffer[..len], stride)
                .unwrap_err();
            assert!(
                matches!(err, Error::InvalidRequest(_)),
                "{request:?}: {err:?}"
            );
        };

        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY, MAX_SCALE + 1.0] {
            invalid(TileRequest { scale, ..ok }, buffer.len(), 256, &mut buffer);
        }
        let rect = |x, y, width, height| TileRequest {
            rect: TileRect {
                x,
                y,
                width,
                height,
            },
            ..ok
        };
        invalid(rect(0, 0, 0, 64), buffer.len(), 256, &mut buffer);
        invalid(rect(0, 0, 64, 0), buffer.len(), 256, &mut buffer);
        invalid(
            rect(0, 0, MAX_TILE_SIDE + 1, 1),
            buffer.len(),
            256,
            &mut buffer,
        );
        invalid(
            rect(0, 0, MAX_TILE_SIDE, MAX_TILE_SIDE + 1),
            buffer.len(),
            256,
            &mut buffer,
        );
        invalid(rect(1 << 25, 0, 64, 64), buffer.len(), 256, &mut buffer);
        invalid(rect(0, u32::MAX, 64, 64), buffer.len(), 256, &mut buffer);
        // Stride shorter than a row, buffer shorter than the rows, and an overflowing stride.
        invalid(ok, buffer.len(), 255, &mut buffer);
        invalid(ok, buffer.len() - 1, 256, &mut buffer);
        invalid(ok, 0, 256, &mut buffer);
        invalid(ok, buffer.len(), usize::MAX, &mut buffer);
        assert!(
            buffer.iter().all(|&b| b == 0x5A),
            "a rejected request wrote pixels"
        );

        // Not a request problem, but still an error and still no write.
        let err = doc
            .render_tile(TileRequest { page: 3, ..ok }, &mut buffer, 256)
            .unwrap_err();
        assert!(
            matches!(err, Error::PageOutOfRange { page: 3, count: 3 }),
            "{err:?}"
        );
        assert!(buffer.iter().all(|&b| b == 0x5A));

        // The largest allowed tile works, and the renderer is fine afterwards.
        let mut big = vec![0; 2048 * 2048 * 4];
        let rect = TileRect {
            x: 0,
            y: 0,
            width: 2048,
            height: 2048,
        };
        doc.render_tile(TileRequest { rect, ..ok }, &mut big, 2048 * 4)
            .unwrap();
        assert_eq!(doc.page_count(), 3);
    });
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/golden")
}

fn bgrx_to_rgb(bgrx: &[u8]) -> Vec<u8> {
    bgrx.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect()
}

fn read_png(path: &PathBuf) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut data).unwrap();
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgb, png::BitDepth::Eight),
        "{}",
        path.display()
    );
    data.truncate(info.buffer_size());
    (info.width, info.height, data)
}

fn write_png(path: &PathBuf, width: u32, height: u32, rgb: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path).unwrap()), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(rgb)
        .unwrap();
}

/// How far `actual` is from `golden`, both RGB: the mean absolute channel difference, and the
/// share of pixels where any channel differs by more than `LOUD`.
fn difference(actual: &[u8], golden: &[u8]) -> (f64, f64) {
    const LOUD: u8 = 32;
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
        .filter(|(a, g)| a.iter().zip(g.iter()).any(|(&a, &g)| a.abs_diff(g) > LOUD))
        .count();
    (
        total as f64 / actual.len() as f64,
        loud as f64 / (actual.len() / 3) as f64,
    )
}

#[test]
fn golden_renders_match_the_committed_images() {
    // Anti-aliasing and rounding may differ slightly between operating systems, so the images are
    // compared with a tolerance: an average error under half a grey level, and under 0.5% of
    // pixels off by more than 32 levels. A missing shape or a shifted edge is far outside that.
    const MEAN_LIMIT: f64 = 0.5;
    const LOUD_LIMIT: f64 = 0.005;
    let bless = std::env::var_os("VELLORA_BLESS_GOLDEN").is_some();

    with_renderer(|renderer| {
        let doc = renderer.open(golden_pdf()).unwrap();
        for page in 0..doc.page_count() {
            let (points_w, points_h) = doc.page_size(page).unwrap();
            let scale = 1.5;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (width, height) = (
                (points_w * scale).ceil() as u32,
                (points_h * scale).ceil() as u32,
            );
            let actual = bgrx_to_rgb(&render(&doc, request(page, scale, (0, 0, width, height))));
            let path = golden_dir().join(format!("page-{}.png", page + 1));
            if bless {
                write_png(&path, width, height, &actual);
                continue;
            }
            let (golden_w, golden_h, golden) = read_png(&path);
            assert_eq!((golden_w, golden_h), (width, height), "page {page} size");
            let (mean, loud) = difference(&actual, &golden);
            assert!(
                mean <= MEAN_LIMIT && loud <= LOUD_LIMIT,
                "page {page}: mean difference {mean:.3} (limit {MEAN_LIMIT}), {:.3}% of pixels \
                 differ loudly (limit {}%); re-bless with VELLORA_BLESS_GOLDEN=1 only if the new \
                 rendering is intended",
                loud * 100.0,
                LOUD_LIMIT * 100.0
            );
        }
    });
}

#[test]
fn the_difference_measure_notices_what_it_should() {
    let flat = vec![200_u8; 30 * 3];
    assert_eq!(difference(&flat, &flat), (0.0, 0.0));
    let mut shifted = flat.clone();
    shifted[..3].copy_from_slice(&[0, 0, 0]);
    let (mean, loud) = difference(&shifted, &flat);
    assert!((mean - 600.0 / 90.0).abs() < 1e-9 && (loud - 1.0 / 30.0).abs() < 1e-9);
    let slightly = flat.iter().map(|&v| v - 1).collect::<Vec<_>>();
    assert_eq!(difference(&slightly, &flat), (1.0, 0.0));
}

#[test]
fn many_threads_share_one_renderer() {
    let pdf = sample_pdf();
    with_renderer(|renderer| {
        let first = renderer.open(pdf.clone()).unwrap();
        let second = renderer.open(pdf).unwrap();
        thread::scope(|scope| {
            for worker in 0..8_usize {
                let (first, second) = (&first, &second);
                scope.spawn(move || {
                    for round in 0..20_usize {
                        let doc = if (worker + round) % 2 == 0 {
                            first
                        } else {
                            second
                        };
                        let page = (worker + round) % 3;
                        let scale = 0.5 + (round % 4) as f32 * 0.5;
                        let (points_w, points_h) = SAMPLE_PAGE_SIZES[page];
                        let (width, height) =
                            ((points_w * scale) as u32, (points_h * scale) as u32);
                        let tile = render(doc, request(page, scale, (0, 0, width, height)));
                        let (w, h) = (width as usize, height as usize);
                        // The rectangle (20,20)-(180,80) is black; the middle of the page's
                        // top-left corner is not. Every page has both, so a tile of another page
                        // or scale would put them elsewhere.
                        let inside = pixel(
                            &tile,
                            w,
                            (100.0 * scale) as usize,
                            (h as f32 - 50.0 * scale) as usize,
                        );
                        assert_eq!(inside, bgr(BLACK), "worker {worker} round {round}");
                        assert_eq!(pixel(&tile, w, 1, 1), bgr(WHITE));
                        assert_eq!(doc.page_size(page).unwrap(), SAMPLE_PAGE_SIZES[page]);
                    }
                });
            }
        });
    });
}

#[test]
fn only_one_renderer_exists_at_a_time_and_handles_outlive_a_dropped_one() {
    let _serial = serial();
    let renderer = Renderer::start(library_path()).unwrap();
    let doc = renderer.open(sample_pdf()).unwrap();

    let second = Renderer::start(library_path());
    assert!(matches!(second, Err(Error::AlreadyLoaded)));

    drop(renderer);
    // The documents were closed with the renderer, and nothing hangs or crashes.
    assert!(matches!(doc.page_size(0), Err(Error::Closed)));
    let mut buffer = vec![0; 16 * 16 * 4];
    let err = doc
        .render_tile(request(0, 1.0, (0, 0, 16, 16)), &mut buffer, 64)
        .unwrap_err();
    assert!(matches!(err, Error::Closed), "{err:?}");
    drop(doc);

    // The claim was released, so a new renderer works.
    let again = Renderer::start(library_path()).unwrap();
    assert_eq!(again.open(sample_pdf()).unwrap().page_count(), 3);
}

#[test]
fn a_renderer_that_cannot_load_the_library_reports_why_and_releases_the_claim() {
    let guard = serial();
    let err = Renderer::start("definitely-not-a-library.so")
        .err()
        .unwrap();
    assert!(matches!(err, Error::Load { .. }), "{err:?}");
    drop(guard);
    with_renderer(|renderer| assert_eq!(renderer.open(sample_pdf()).unwrap().page_count(), 3));
}

#[test]
fn dropping_a_handle_closes_its_document_and_others_keep_working() {
    with_renderer(|renderer| {
        let keep = renderer.open(sample_pdf()).unwrap();
        for _ in 0..50 {
            let temporary = renderer.open(sample_pdf()).unwrap();
            assert_eq!(temporary.page_count(), 3);
        }
        assert_eq!(keep.page_size(2).unwrap(), (612.0, 792.0));
    });
}
