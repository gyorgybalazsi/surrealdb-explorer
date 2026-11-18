//! The app icon: a dark rounded square with "SurrealDB / Explorer" in
//! SurrealDB magenta. The source is `assets/icon/icon.svg`; the PNGs next to
//! it are rendered from it and embedded here.

use iced::window;

/// Used for the window (title bar, taskbar, Alt-Tab) on Windows and Linux.
const WINDOW_ICON_PNG: &[u8] = include_bytes!("../../assets/icon/icon-256.png");

/// Used for the macOS Dock, which shows icons up to 1024 px on Retina screens.
#[cfg(target_os = "macos")]
const DOCK_ICON_PNG: &[u8] = include_bytes!("../../assets/icon/icon-1024.png");

/// The window icon, or `None` if the embedded PNG cannot be decoded (the
/// window then opens with the system's default icon).
pub fn window_icon() -> Option<window::Icon> {
    let (rgba, width, height) = decode_rgba(WINDOW_ICON_PNG)?;
    window::icon::from_rgba(rgba, width, height).ok()
}

/// Decodes a PNG into 8-bit RGBA pixels.
fn decode_rgba(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // Expand palette / low bit depth images and drop 16-bit channels to 8.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((rgba, info.width, info.height))
}

/// Sets the Dock icon on macOS. A window icon does nothing there, and an app
/// started with `cargo run` (not from a `.app` bundle) would otherwise show
/// the generic executable icon. Must run on the main thread after the app has
/// finished launching (AppKit resets an icon set before that), so it is called
/// when the window opens.
#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("not on the main thread, so the Dock icon was not set");
        return;
    };
    let data = NSData::with_bytes(DOCK_ICON_PNG);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        tracing::warn!("the Dock icon PNG could not be loaded");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: `image` is a valid, retained NSImage and we are on the main thread.
    unsafe { app.setApplicationIconImage(Some(&image)) };
    tracing::debug!("Dock icon set");
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_window_icon_decodes() {
        let (rgba, width, height) = decode_rgba(WINDOW_ICON_PNG).expect("icon PNG decodes");
        assert_eq!((width, height), (256, 256));
        assert_eq!(rgba.len(), 256 * 256 * 4);
        assert!(window_icon().is_some());
    }
}
