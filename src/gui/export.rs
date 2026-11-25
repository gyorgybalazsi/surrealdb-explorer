//! A picture of the whole graph: everything the canvas would show if it were
//! big enough, not only what is scrolled or panned into view. It is drawn by
//! the same code as the canvas, offscreen with iced's software renderer, so
//! it looks just like the window.

use std::future::Future as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iced::advanced::graphics::geometry::Renderer as _;
use iced::advanced::renderer::Headless;
use iced::mouse::Cursor;
use iced::widget::canvas::Program;
use iced::{Color, Font, Rectangle, Size, Theme, Vector};

use crate::config::Visualization;
use crate::gui::app::Selection;
use crate::gui::canvas::{CANTON_MARGIN, Camera, GraphView, Interaction};
use crate::gui::model::Graph;

/// Pixels per canvas pixel: sharp on a Retina screen and when zoomed in a document.
const PIXEL_RATIO: f32 = 2.0;
/// The longest side, in pixels: the largest image Claude takes (8000 × 8000).
const MAX_SIDE: f32 = 8000.0;
/// At most this many pixels (about 160 MB of RGBA), so a huge graph can't
/// run the app out of memory; it is drawn smaller instead.
const MAX_PIXELS: f32 = 40_000_000.0;
/// Room around the standard view's records, in canvas pixels: their labels
/// hang below them and spread sideways.
const STANDARD_MARGIN: f32 = 110.0;
/// Room below the Canton view's cards for its legend.
const LEGEND_ROOM: f32 = 40.0;

/// What the picture shows: the graph as the window draws it, with the
/// selection (the hover is left out: it only follows the mouse).
pub struct Scene<'a> {
    pub graph: &'a Graph,
    pub visualization: Visualization,
    pub selection: Option<Selection>,
    /// The window's camera: the standard view keeps its zoom.
    pub camera: Camera,
    /// The Canton view's zoom and the width its layout fills, in canvas pixels.
    pub canton: (f32, f32),
    /// The Canton view's content height, in canvas pixels.
    pub canton_height: f32,
}

/// An image in RGBA, 8 bits a channel, row by row.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Scene<'_> {
    /// The picture's size in canvas pixels, and the camera that fits the whole
    /// graph in it. `None` when there is nothing to draw.
    fn frame(&self) -> Option<(Size, Camera)> {
        if self.visualization == Visualization::Canton {
            if !self.graph.nodes.iter().any(|n| n.card.shown) {
                return None;
            }
            let (scale, width) = self.canton;
            let w = width + 2.0 * CANTON_MARGIN.0;
            let h = self.canton_height + LEGEND_ROOM;
            // As the window's `scroll_canton` at the top: the layout's corner at the margins.
            let offset = Vector::new(CANTON_MARGIN.0 - w / 2.0, CANTON_MARGIN.1 - h / 2.0);
            return Some((Size::new(w, h), Camera { offset, scale }));
        }
        let (min, max) = self.graph.bounds()?;
        let scale = self.camera.scale;
        let size = (max - min) * scale;
        let w = size.x + 2.0 * STANDARD_MARGIN;
        let h = size.y + 2.0 * STANDARD_MARGIN;
        Some((Size::new(w, h), Camera { offset: (min + max) * (-0.5 * scale), scale }))
    }

    /// Draws the whole graph. `None` when there is nothing to draw.
    pub fn render(&self) -> Result<Option<Image>, String> {
        let Some((size, camera)) = self.frame() else { return Ok(None) };
        let ratio = pixel_ratio(size);
        let physical = iced::Size::new(
            ((size.width * ratio).round() as u32).max(1),
            ((size.height * ratio).round() as u32).max(1),
        );

        // The software renderer: no GPU, so no texture size limit, and the
        // same pixels on every machine.
        let mut renderer = software_renderer().ok_or("the software renderer is not available")?;
        let view = GraphView {
            graph: self.graph,
            camera,
            hover: None,
            selection: self.selection,
            visualization: self.visualization,
            known_size: size,
            // No scrollbar: the picture shows all of it.
            scroll: None,
        };
        let bounds = Rectangle::new(iced::Point::ORIGIN, size);
        for geometry in view.draw(&Interaction::Idle, &renderer, &Theme::Light, bounds, Cursor::Unavailable) {
            renderer.draw_geometry(geometry);
        }
        let rgba = renderer.screenshot(physical, ratio, Color::WHITE);
        Ok(Some(Image { width: physical.width, height: physical.height, rgba }))
    }
}

/// iced's software renderer (tiny-skia). Creating it doesn't wait on
/// anything, so its future is ready at once: it is polled here rather than
/// run on an executor, which may not be entered from the window's update.
fn software_renderer() -> Option<iced::Renderer> {
    let future = std::pin::pin!(iced::Renderer::new(Font::DEFAULT, 16.into(), Some("tiny-skia")));
    match future.poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
        std::task::Poll::Ready(renderer) => renderer,
        std::task::Poll::Pending => None,
    }
}

/// Device pixels per canvas pixel for a picture `size` canvas pixels big:
/// [`PIXEL_RATIO`], or less if that would pass [`MAX_SIDE`] or [`MAX_PIXELS`].
fn pixel_ratio(size: Size) -> f32 {
    let side = MAX_SIDE / size.width.max(size.height).max(1.0);
    let area = (MAX_PIXELS / (size.width * size.height).max(1.0)).sqrt();
    PIXEL_RATIO.min(side).min(area)
}

impl Image {
    /// The image as a PNG file's bytes.
    pub fn png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        // Fast compression: a big graph would otherwise take seconds.
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&self.rgba).map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
        Ok(out)
    }

    /// Puts the image on the system clipboard, to paste into Claude or a document.
    pub fn copy_to_clipboard(&self) -> Result<(), String> {
        // Kept for the app's lifetime: on Linux the clipboard's contents
        // belong to the program that copied them and go when it lets go.
        static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);
        let mut slot = CLIPBOARD.lock().map_err(|_| "the clipboard is not available")?;
        if slot.is_none() {
            *slot = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        let clipboard = slot.as_mut().ok_or("the clipboard is not available")?;
        clipboard
            .set_image(arboard::ImageData {
                width: self.width as usize,
                height: self.height as usize,
                bytes: std::borrow::Cow::Borrowed(&self.rgba),
            })
            .map_err(|e| e.to_string())
    }

    /// Saves the image as a PNG in `folder`, named after `stem` (with a
    /// number added if that name is taken). Returns where it went.
    pub fn save_png(&self, folder: &Path, stem: &str) -> Result<PathBuf, String> {
        let bytes = self.png()?;
        let path = free_name(folder, stem);
        std::fs::write(&path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
        Ok(path)
    }
}

/// `folder/stem.png`, or `stem-2.png`, `stem-3.png`… if it is taken.
fn free_name(folder: &Path, stem: &str) -> PathBuf {
    let mut path = folder.join(format!("{stem}.png"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{stem}-{n}.png"));
        n += 1;
    }
    path
}

/// Where saved pictures go: the Downloads folder, else the home folder,
/// else the folder the app runs from.
pub fn pictures_folder() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    match home {
        Some(home) if home.join("Downloads").is_dir() => home.join("Downloads"),
        Some(home) => home,
        None => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_pictures_are_drawn_at_twice_the_size() {
        assert_eq!(pixel_ratio(Size::new(800.0, 600.0)), 2.0);
    }

    #[test]
    fn big_pictures_stay_within_claudes_limit() {
        let ratio = pixel_ratio(Size::new(900.0, 20_000.0));
        assert!(20_000.0 * ratio <= MAX_SIDE + 0.5);
        let ratio = pixel_ratio(Size::new(7000.0, 7000.0));
        assert!(7000.0 * 7000.0 * ratio * ratio <= MAX_PIXELS + 1.0);
    }

    /// Two records and a relation, laid out wider than any window would show.
    fn graph() -> Graph {
        use crate::graph::{GraphData, GraphEdge, GraphNode};
        let node = |id: &str| GraphNode { id: id.into(), table: "person".into(), data: serde_json::json!({}) };
        let data = GraphData {
            nodes: vec![node("person:a"), node("person:b")],
            edges: vec![GraphEdge {
                id: "knows:1".into(),
                from: "person:a".into(),
                to: "person:b".into(),
                label: "knows".into(),
                data: serde_json::json!({}),
            }],
            truncated: Vec::new(),
        };
        let mut graph = Graph::new(data, None);
        graph.nodes[0].pos = Vector::new(-1500.0, 0.0);
        graph.nodes[1].pos = Vector::new(1500.0, 40.0);
        graph
    }

    #[test]
    fn the_picture_holds_the_whole_graph() {
        let graph = graph();
        let scene = Scene {
            graph: &graph,
            visualization: Visualization::Standard,
            selection: None,
            camera: Camera::default(),
            canton: (1.0, 800.0),
            canton_height: 0.0,
        };
        let image = scene.render().unwrap().unwrap();
        // 3000 canvas pixels between the records, plus the margins, at twice the size.
        assert_eq!(image.width, ((3000.0 + 2.0 * STANDARD_MARGIN) * PIXEL_RATIO) as u32);
        assert_eq!(image.rgba.len(), (image.width * image.height * 4) as usize);
        // Both records are drawn: the picture is not just the background.
        let pixel = |x: u32, y: u32| {
            let i = ((y * image.width + x) * 4) as usize;
            [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]
        };
        let background = pixel(1, 1);
        let mid_y = image.height / 2 - (20.0 * PIXEL_RATIO) as u32;
        let left = (STANDARD_MARGIN * PIXEL_RATIO) as u32;
        assert_ne!(pixel(left, mid_y), background, "the left record is missing");
        assert_ne!(pixel(image.width - left, image.height / 2 + (20.0 * PIXEL_RATIO) as u32), background, "the right record is missing");
        assert!(image.png().unwrap().starts_with(b"\x89PNG"));
    }

    #[test]
    fn an_empty_graph_has_no_picture() {
        let graph = Graph::new(crate::graph::GraphData::default(), None);
        let scene = Scene {
            graph: &graph,
            visualization: Visualization::Standard,
            selection: None,
            camera: Camera::default(),
            canton: (1.0, 800.0),
            canton_height: 0.0,
        };
        assert!(scene.render().unwrap().is_none());
    }

    #[test]
    fn saving_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let image = Image { width: 2, height: 1, rgba: vec![255; 8] };
        let first = image.save_png(dir.path(), "graph").unwrap();
        let second = image.save_png(dir.path(), "graph").unwrap();
        assert_eq!(first.file_name().unwrap(), "graph.png");
        assert_eq!(second.file_name().unwrap(), "graph-2.png");
    }
}
