//! Own fork: pictures in rendered markdown (local images, mermaid diagrams) through the terminal's
//! graphics protocol, which Herdr re-draws natively; without one they stay text placeholders.

use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use image::DynamicImage;
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::sliced::{SlicedImage, SlicedProtocol};
use resvg::{tiny_skia, usvg};

/// The tallest picture in rows, so one diagram never buries the document.
const MAX_ROWS: u16 = 60;

/// Pictures kept across renders, by source, width and background.
const CACHE_SLOTS: usize = 32;

/// A picture sized to whole cells, encoded once for the terminal.
pub struct Picture {
    pub cols: u16,
    pub rows: u16,
    proto: SlicedProtocol,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picture")
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

/// What a picture is drawn from.
#[derive(Debug, Clone, Copy)]
pub enum Source<'a> {
    Mermaid(&'a str),
    File(&'a str),
}

static PICKER: OnceLock<Option<Picker>> = OnceLock::new();
static CACHE: Mutex<Vec<(u64, Arc<Picture>)>> = Mutex::new(Vec::new());

thread_local! {
    /// The folder the document being rendered sits in, for its relative image paths.
    static BASE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Ask the terminal for its graphics protocol and cell size, once: after the alternate screen,
/// before input. Halfblocks are no picture, so they count as none.
pub fn detect() {
    PICKER.get_or_init(|| {
        let picker = Picker::from_query_stdio().ok()?;
        crate::logln!(
            "graphics protocol={:?} font={:?}",
            picker.protocol_type(),
            picker.font_size()
        );
        (picker.protocol_type() != ProtocolType::Halfblocks).then_some(picker)
    });
}

/// Whether pictures draw at all.
pub fn enabled() -> bool {
    PICKER.get().is_some_and(Option::is_some)
}

/// Run `render` with `dir` as the base of relative image paths.
pub fn with_base<T>(dir: Option<PathBuf>, render: impl FnOnce() -> T) -> T {
    let old = BASE.with(|b| b.replace(dir));
    let out = render();
    BASE.with(|b| *b.borrow_mut() = old);
    out
}

/// The picture for `source`, fit to `cols` columns on background `bg`; `None` when the terminal
/// draws none, or the source is no local image or no valid diagram.
pub fn picture(source: Source<'_>, cols: usize, bg: (u8, u8, u8)) -> Option<Arc<Picture>> {
    let picker = PICKER.get()?.as_ref()?;
    let file = match source {
        Source::File(dest) => Some(local_image(dest)?),
        Source::Mermaid(_) => None,
    };
    let mut h = DefaultHasher::new();
    match (source, &file) {
        (Source::Mermaid(text), _) => text.hash(&mut h),
        (_, Some(path)) => {
            path.hash(&mut h);
            std::fs::metadata(path).and_then(|m| m.modified()).ok().hash(&mut h);
        }
        _ => {}
    }
    (cols, bg).hash(&mut h);
    let key = h.finish();
    if let Some((_, pic)) = CACHE.lock().ok()?.iter().find(|(k, _)| *k == key) {
        return Some(pic.clone());
    }
    let font = picker.font_size();
    let img = match &file {
        Some(path) => load_file(path, bg, f32::from(font.height))?,
        None => mermaid(source_text(source), bg, f32::from(font.height))?,
    };
    let pic = Arc::new(fit(picker, img, cols)?);
    let mut cache = CACHE.lock().ok()?;
    cache.push((key, pic.clone()));
    if cache.len() > CACHE_SLOTS {
        cache.remove(0);
    }
    Some(pic)
}

fn source_text(source: Source<'_>) -> &str {
    match source {
        Source::Mermaid(text) | Source::File(text) => text,
    }
}

/// A local png, jpeg or svg the destination names, against the document's folder.
fn local_image(dest: &str) -> Option<PathBuf> {
    let dest = dest.strip_prefix("file://").unwrap_or(dest);
    if dest.contains("://") {
        return None;
    }
    let ext = Path::new(dest).extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "svg") {
        return None;
    }
    let path = Path::new(dest);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        BASE.with(|b| b.borrow().clone())?.join(path)
    };
    path.is_file().then_some(path)
}

/// Downscale to the pane's pixel width, never up, and encode in whole cells.
fn fit(picker: &Picker, img: DynamicImage, cols: usize) -> Option<Picture> {
    let font = picker.font_size();
    let (fw, fh) = (u32::from(font.width.max(1)), u32::from(font.height.max(1)));
    let max_w = u32::try_from(cols).ok()?.max(1) * fw;
    let scale = (f64::from(max_w) / f64::from(img.width().max(1))).min(1.0);
    let w = (f64::from(img.width()) * scale).ceil() as u32;
    let h = (f64::from(img.height()) * scale).ceil() as u32;
    let size = Size::new(w.div_ceil(fw) as u16, (h.div_ceil(fh) as u16).clamp(1, MAX_ROWS));
    let proto = SlicedProtocol::new_with_resize(picker, img, size, Resize::Fit(None)).ok()?;
    let Size { width, height } = proto.size();
    Some(Picture { cols: width, rows: height, proto })
}

/// The system fonts, loaded once: diagrams and svg text need them, Cyrillic included.
fn options() -> &'static usvg::Options<'static> {
    static OPTIONS: OnceLock<usvg::Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut options = usvg::Options::default();
        options.fontdb_mut().load_system_fonts();
        options
    })
}

/// A mermaid diagram in the pane's colors, its text about the size of the terminal's.
fn mermaid(text: &str, bg: (u8, u8, u8), cell_h: f32) -> Option<DynamicImage> {
    let dark = luma(bg) < 0.5;
    let mut opts = mermaid_rs_renderer::RenderOptions::default();
    if dark {
        opts.theme = mermaid_rs_renderer::Theme::dark();
    }
    opts.theme.background = format!("#{:02x}{:02x}{:02x}", bg.0, bg.1, bg.2);
    let svg = mermaid_rs_renderer::render_with_options(text, opts).ok()?;
    let scale = cell_h / (1.25 * 16.0);
    rasterize(&svg, scale, bg)
}

fn load_file(path: &Path, bg: (u8, u8, u8), cell_h: f32) -> Option<DynamicImage> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
        let svg = std::fs::read_to_string(path).ok()?;
        return rasterize(&svg, cell_h / 20.0, bg);
    }
    image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()
}

/// An svg drawn at `scale` over the background, so transparency reads as the pane.
fn rasterize(svg: &str, scale: f32, bg: (u8, u8, u8)) -> Option<DynamicImage> {
    let tree = usvg::Tree::from_str(svg, options()).ok()?;
    let size = tree.size().to_int_size().scale_by(scale.max(0.1))?;
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())?;
    pixmap.fill(tiny_skia::Color::from_rgba8(bg.0, bg.1, bg.2, 255));
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let rgba = image::RgbaImage::from_raw(size.width(), size.height(), pixmap.take())?;
    Some(DynamicImage::ImageRgba8(rgba))
}

fn luma((r, g, b): (u8, u8, u8)) -> f32 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
}

/// One run of a picture's rows on screen: where it starts and which of its rows it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub picture: usize,
    pub y: u16,
    pub first_row: u16,
    pub rows: u16,
}

/// Group screen rows, each a picture's row or none, into runs of consecutive picture rows.
pub fn runs(rows: impl IntoIterator<Item = Option<(usize, u16)>>) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for (y, row) in rows.into_iter().enumerate() {
        let Some((picture, at)) = row else { continue };
        let y = y as u16;
        match out.last_mut() {
            Some(run)
                if run.picture == picture
                    && run.y + run.rows == y
                    && run.first_row + run.rows == at =>
            {
                run.rows += 1;
            }
            _ => out.push(Run { picture, y, first_row: at, rows: 1 }),
        }
    }
    out
}

/// Draw each run's rows of its picture into `area`, the column after the gutter.
pub fn overlay(frame: &mut Frame, pictures: &[Arc<Picture>], runs: &[Run], area: Rect) {
    for run in runs {
        let Some(pic) = pictures.get(run.picture) else { continue };
        let rect = Rect { y: area.y + run.y, height: run.rows, ..area };
        let skip = i16::try_from(run.first_row).unwrap_or(i16::MAX);
        frame.render_widget(SlicedImage::new(&pic.proto, (0, -skip).into()), rect);
    }
}

/// Keep the warm-up off the first diagram: the fonts load on a thread at startup.
pub fn warm() {
    if enabled() {
        std::thread::spawn(|| {
            let _ = options();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_group_into_runs_of_one_picture() {
        let rows = [None, Some((0, 0)), Some((0, 1)), None, Some((0, 2)), Some((1, 0))];
        assert_eq!(
            runs(rows),
            [
                Run { picture: 0, y: 1, first_row: 0, rows: 2 },
                Run { picture: 0, y: 4, first_row: 2, rows: 1 },
                Run { picture: 1, y: 5, first_row: 0, rows: 1 },
            ]
        );
    }

    #[test]
    fn a_mermaid_diagram_rasterizes_with_cyrillic() {
        let img = mermaid("graph LR; A[Агент] --> B[Ядро]", (0x1e, 0x1e, 0x2e), 32.0).unwrap();
        assert!(img.width() > 100 && img.height() > 20, "{}x{}", img.width(), img.height());
    }

    #[test]
    fn only_a_local_image_file_is_a_picture_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), b"x").unwrap();
        let base = Some(dir.path().to_path_buf());
        assert!(with_base(base.clone(), || local_image("a.png")).is_some());
        assert!(with_base(base.clone(), || local_image("b.png")).is_none());
        assert!(with_base(base.clone(), || local_image("https://x.dev/a.png")).is_none());
        assert!(with_base(base, || local_image("a.txt")).is_none());
    }
}
