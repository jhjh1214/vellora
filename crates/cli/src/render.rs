//! `vellora render`: pages as PNG files, drawn by the engine (the sandboxed process the desktop app
//! uses). A page is drawn in tiles of 512 x 512 pixels and put together here, so no page is limited
//! by the size of one tile.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use vellora_engine_client::{Client, Event, TileRequest};
use vellora_ipc::{Priority, SlotId, TileRect};

use crate::engine;
use crate::pagespec;

/// The side of a tile in pixels: a slot holds `TILE x TILE x 4` bytes.
const TILE: u32 = 512;
/// The most dots per inch (the engine draws at most 64 pixels to the point).
pub(crate) const MAX_DPI: u32 = 4608;
/// The most pixels of one page image (about 400 MB of RGB).
const MAX_PIXELS: u64 = 1 << 27;

/// Draws `pages` of `file` (a specification; the first page if `None`) at `dpi` and writes them as
/// `page-N.png` in `output` (a directory, made if needed), or as `output` itself if that ends in
/// `.png` and exactly one page is wanted. Returns the files written.
pub(crate) fn run(
    file: &Path,
    pages: Option<&str>,
    dpi: u32,
    output: &Path,
    password: Option<&str>,
) -> Result<Vec<PathBuf>, String> {
    if !(1..=MAX_DPI).contains(&dpi) {
        return Err(format!("--dpi must be between 1 and {MAX_DPI}"));
    }
    let client = engine::open(file, password)?;
    let count = client
        .page_count()
        .ok_or("the engine reported no page count")?;
    let wanted = pagespec::parse(pages.unwrap_or("1"), count).map_err(|error| error.to_string())?;
    let targets = targets(output, &wanted, count)?;

    let scale = f32::from(u16::try_from(dpi).map_err(|_| "--dpi is too large")?) / 72.0;
    let mut written = Vec::new();
    for (&page, target) in wanted.iter().zip(&targets) {
        let (width, height, rgb) = draw(&client, page, scale)?;
        if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        write_png(target, width, height, &rgb)?;
        written.push(target.clone());
    }
    Ok(written)
}

/// Where each wanted page goes.
fn targets(output: &Path, wanted: &[u32], count: u32) -> Result<Vec<PathBuf>, String> {
    let is_png = output
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"));
    if is_png {
        return match wanted {
            [_] => Ok(vec![output.to_path_buf()]),
            _ => Err(format!(
                "{} is a file but {} pages were asked for: give a folder",
                output.display(),
                wanted.len()
            )),
        };
    }
    let digits = count.to_string().len();
    Ok(wanted
        .iter()
        .map(|page| output.join(format!("page-{:0digits$}.png", page + 1)))
        .collect())
}

/// One page as RGB, put together from tiles.
// A page is at most `MAX_PIXELS` pixels (checked before anything is allocated), and the sizes
// come from the engine as positive finite numbers.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn draw(client: &Client, page: u32, scale: f32) -> Result<(u32, u32, Vec<u8>), String> {
    let size = client
        .page_size(page)
        .ok_or_else(|| format!("page {}: its size is not known", page + 1))?;
    let width = (size.width * scale).ceil().max(1.0) as u32;
    let height = (size.height * scale).ceil().max(1.0) as u32;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!(
            "page {} would be {width} x {height} pixels at this resolution: lower --dpi",
            page + 1
        ));
    }
    let mut rgb = vec![0u8; width as usize * height as usize * 3];
    let mut slot = vec![0u8; engine::SLOT_BYTES as usize];
    for y in (0..height).step_by(TILE as usize) {
        for x in (0..width).step_by(TILE as usize) {
            let rect = TileRect {
                x,
                y,
                width: TILE.min(width - x),
                height: TILE.min(height - y),
            };
            let request = client
                .request_tile(&TileRequest {
                    page,
                    scale,
                    rect,
                    slot: SlotId(0),
                    priority: Priority::Visible,
                })
                .map_err(|error| error.to_string())?;
            engine::wait(client, |event| match event {
                Event::TileReady { request: r, .. } if *r == request => Some(Ok(())),
                Event::RequestFailed {
                    request: Some(r),
                    message,
                    ..
                } if *r == request => Some(Err(format!("page {}: {message}", page + 1))),
                _ => None,
            })?;
            client
                .read_slot(SlotId(0), &mut slot)
                .map_err(|error| error.to_string())?;
            // BGRx rows, tight, to RGB rows of the whole page.
            for row in 0..rect.height {
                let from = row as usize * rect.width as usize * 4;
                let to = ((y + row) as usize * width as usize + x as usize) * 3;
                for column in 0..rect.width as usize {
                    let pixel = &slot[from + column * 4..from + column * 4 + 3];
                    let out = &mut rgb[to + column * 3..to + column * 3 + 3];
                    out.copy_from_slice(&[pixel[2], pixel[1], pixel[0]]);
                }
            }
        }
    }
    Ok((width, height, rgb))
}

fn write_png(target: &Path, width: u32, height: u32, rgb: &[u8]) -> Result<(), String> {
    let file = File::create(target).map_err(|error| format!("{}: {error}", target.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| format!("{}: {error}", target.display()))?;
    writer
        .write_image_data(rgb)
        .map_err(|error| format!("{}: {error}", target.display()))?;
    writer
        .finish()
        .map_err(|error| format!("{}: {error}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_page_to_a_png_file_and_several_to_a_folder() {
        let one = targets(Path::new("out/a.PNG"), &[2], 9).unwrap();
        assert_eq!(one, [PathBuf::from("out/a.PNG")]);
        assert!(targets(Path::new("a.png"), &[0, 1], 9).is_err());
        let many = targets(Path::new("pages"), &[0, 9, 11], 120).unwrap();
        assert_eq!(
            many,
            [
                PathBuf::from("pages/page-001.png"),
                PathBuf::from("pages/page-010.png"),
                PathBuf::from("pages/page-012.png"),
            ]
        );
    }
}
