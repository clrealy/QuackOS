//! The pointer cursor.
//!
//! Two sources:
//!   * the client's own cursor surface (`wl_pointer.set_cursor`) - most apps do this,
//!     and it's how you get an I-beam over text;
//!   * Carbs' own arrow, for the desktop background and for any client that never
//!     sets one.
//!
//! The arrow is drawn here rather than loaded from an XCursor theme: it is original
//! art, it scales to any size, and it cannot go missing on a fresh install.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                render_elements,
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                Kind,
            },
            ImportAll, ImportMem, Renderer,
        },
    },
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    utils::{Point, Scale, Transform},
    wayland::compositor::with_states,
};

use crate::state::Carbs;

render_elements! {
    /// Everything Carbs draws on top of the window stack.
    pub CarbsRenderElement<R> where R: ImportAll + ImportMem;
    /// A client's own cursor surface.
    Surface=WaylandSurfaceRenderElement<R>,
    /// Carbs' built-in arrow.
    Memory=MemoryRenderBufferRenderElement<R>,
}

impl Carbs {
    /// The cursor, as render elements to draw over everything else.
    pub fn cursor_elements<R>(&self, renderer: &mut R, scale: Scale<f64>) -> Vec<CarbsRenderElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Clone + Send + 'static,
    {
        let Some(pointer) = self.seat.get_pointer() else {
            return Vec::new();
        };
        let location = pointer.current_location();

        match &self.cursor_status {
            CursorImageStatus::Hidden => Vec::new(),

            // The client gave us an image; honour its hotspot.
            CursorImageStatus::Surface(surface) => {
                let hotspot = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<CursorImageSurfaceData>()
                        .map(|data| data.lock().unwrap().hotspot)
                        .unwrap_or_default()
                });
                let position = (location - hotspot.to_f64()).to_physical(scale).to_i32_round();
                render_elements_from_surface_tree(renderer, surface, position, scale, 1.0, Kind::Cursor)
            }

            // No client image (or a named one we don't theme yet): our own arrow.
            CursorImageStatus::Named(_) => {
                let (hx, hy) = self.cursor_arrow.hotspot;
                let position = location.to_physical(scale).to_i32_round::<i32>() - Point::from((hx, hy));
                match MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    position.to_f64(),
                    &self.cursor_arrow.buffer,
                    None,
                    None,
                    None,
                    Kind::Cursor,
                ) {
                    Ok(element) => vec![CarbsRenderElement::Memory(element)],
                    Err(err) => {
                        tracing::warn!("couldn't upload the cursor: {err:?}");
                        Vec::new()
                    }
                }
            }
        }
    }
}

/// The arrow in design space: tip at the origin, 11 x 19 units.
/// A plain left-pointing arrow - no one's trademark, just a triangle with a tail.
const ARROW: [(f32, f32); 7] = [
    (0.0, 0.0),   // tip (the hotspot)
    (0.0, 16.6),  // straight down the left edge
    (3.9, 12.9),  // in to the notch, bottom-left of the head
    (6.4, 18.8),  // out to the tail's tip
    (8.7, 17.7),  // across the end of the tail
    (6.2, 12.1),  // back up the tail's right side
    (11.0, 12.1), // out to the head's right corner, then home to the tip
];
const DESIGN_W: f32 = 11.0;
const DESIGN_H: f32 = 19.0;

/// A rasterised arrow plus where its tip sits inside the bitmap.
pub struct Arrow {
    pub buffer: MemoryRenderBuffer,
    /// Offset from the bitmap's top-left to the arrow's tip, in buffer pixels.
    pub hotspot: (i32, i32),
}

/// Rasterises the arrow at `height` pixels tall.
///
/// Works off a signed distance to the outline rather than sampling coverage, so the
/// black border stays solid even at the tip, where the shape is a single sharp point.
///
/// Returns premultiplied ARGB8888 (memory order B, G, R, A), its width and height,
/// and the hotspot.
pub fn rasterize(height: u32) -> (Vec<u8>, u32, u32, (i32, i32)) {
    let height = height.clamp(8, 512);
    let scale = height as f32 / DESIGN_H;
    // Outline thickness in pixels, and the margin that keeps it inside the bitmap.
    let outline = (height as f32 / 14.0).max(1.0);
    let pad = outline.ceil() as i32 + 1;

    let w = (DESIGN_W * scale).ceil() as i32 + 2 * pad;
    let h = (DESIGN_H * scale).ceil() as i32 + 2 * pad;

    let poly: Vec<(f32, f32)> = ARROW
        .iter()
        .map(|&(x, y)| (x * scale + pad as f32, y * scale + pad as f32))
        .collect();

    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Signed distance to the outline: negative inside the arrow.
            let distance = distance_to_outline(px, py, &poly)
                * if point_in_polygon(px, py, &poly) { -1.0 } else { 1.0 };

            // The silhouette is the arrow grown by `outline`; the white core is the
            // arrow itself. The half-pixel gives a soft edge on both.
            let alpha = (0.5 + outline - distance).clamp(0.0, 1.0);
            let white = (0.5 - distance).clamp(0.0, 1.0);

            // Premultiplied ARGB8888, which in memory is B, G, R, A.
            let value = (white * alpha * 255.0).round() as u8;
            let o = ((y * w + x) * 4) as usize;
            pixels[o] = value;
            pixels[o + 1] = value;
            pixels[o + 2] = value;
            pixels[o + 3] = (alpha * 255.0).round() as u8;
        }
    }

    (pixels, w as u32, h as u32, (pad, pad))
}

impl Arrow {
    /// Builds the arrow bitmap for a given cursor height in pixels.
    pub fn new(height: u32) -> Self {
        let (pixels, w, h, hotspot) = rasterize(height);
        let buffer = MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (w as i32, h as i32),
            1,
            Transform::Normal,
            None,
        );
        Self { buffer, hotspot }
    }
}

/// Shortest distance from a point to the polygon's outline (always positive).
fn distance_to_outline(x: f32, y: f32, poly: &[(f32, f32)]) -> f32 {
    let mut best = f32::MAX;
    let n = poly.len();
    for i in 0..n {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
        let (ex, ey) = (bx - ax, by - ay);
        let len2 = ex * ex + ey * ey;
        // Project onto the segment, clamped to its ends.
        let t = if len2 > 0.0 {
            (((x - ax) * ex + (y - ay) * ey) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (dx, dy) = (x - (ax + t * ex), y - (ay + t * ey));
        best = best.min((dx * dx + dy * dy).sqrt());
    }
    best
}

/// Ray-casting point-in-polygon test.
fn point_in_polygon(x: f32, y: f32, poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(pixels: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let o = ((y * w + x) * 4) as usize;
        (pixels[o], pixels[o + 1], pixels[o + 2], pixels[o + 3])
    }

    #[test]
    fn arrow_has_sane_dimensions() {
        let (pixels, w, h, hotspot) = rasterize(24);
        assert_eq!(pixels.len(), (w * h * 4) as usize);
        assert!(w > 8 && w < 64, "width {w}");
        assert!(h >= 24 && h < 64, "height {h}");
        assert!(hotspot.0 > 0 && hotspot.1 > 0, "hotspot {hotspot:?} must sit inside the outline");
    }

    #[test]
    fn hotspot_is_on_the_tip() {
        let (pixels, w, _h, (hx, hy)) = rasterize(24);
        // The tip itself is opaque...
        let (_, _, _, a) = px(&pixels, w, hx as u32, hy as u32);
        assert!(a > 128, "tip should be solid, got alpha {a}");
        // ...and a few pixels above it is empty space.
        let (_, _, _, a_above) = px(&pixels, w, hx as u32, 0);
        assert_eq!(a_above, 0, "there should be nothing above the tip");
    }

    #[test]
    fn arrow_is_white_with_a_black_outline() {
        let (pixels, w, h, _) = rasterize(32);
        let mut white = 0;
        let mut black = 0;
        let mut clear = 0;
        for y in 0..h {
            for x in 0..w {
                let (b, _, _, a) = px(&pixels, w, x, y);
                if a < 32 {
                    clear += 1;
                } else if b > 200 {
                    white += 1;
                } else if b < 64 {
                    black += 1;
                }
            }
        }
        assert!(white > 100, "expected a white body, got {white} px");
        assert!(black > 40, "expected a black outline, got {black} px");
        assert!(clear > 100, "expected transparent space around it, got {clear} px");
    }

    #[test]
    fn premultiplied_never_exceeds_alpha() {
        // A premultiplied pixel with a colour channel above its alpha renders wrong.
        let (pixels, w, h, _) = rasterize(28);
        for i in 0..(w * h) as usize {
            let o = i * 4;
            let (b, g, r, a) = (pixels[o], pixels[o + 1], pixels[o + 2], pixels[o + 3]);
            assert!(b <= a && g <= a && r <= a, "pixel {i}: colour {b},{g},{r} over alpha {a}");
        }
    }

    #[test]
    fn scales_without_panicking() {
        for size in [8, 16, 24, 32, 48, 64, 96, 128] {
            let (pixels, w, h, hotspot) = rasterize(size);
            assert_eq!(pixels.len(), (w * h * 4) as usize, "size {size}");
            assert!(hotspot.0 < w as i32 && hotspot.1 < h as i32, "size {size}");
        }
        // And clamps instead of exploding on nonsense.
        let (_, w, _, _) = rasterize(0);
        assert!(w > 0);
        let (_, w, _, _) = rasterize(100_000);
        assert!(w < 1024);
    }
}

#[cfg(test)]
mod preview {
    /// `cargo test -- --ignored --nocapture preview` to eyeball the arrow.
    #[test]
    #[ignore]
    fn print_arrow() {
        let (pixels, w, h, hotspot) = super::rasterize(24);
        println!("{w}x{h}, hotspot {hotspot:?}");
        for y in 0..h {
            let mut line = String::new();
            for x in 0..w {
                let o = ((y * w + x) * 4) as usize;
                let (v, a) = (pixels[o], pixels[o + 3]);
                line.push(match (a, v) {
                    (0..=20, _) => ' ',
                    (_, 180..=255) => '#',
                    (_, 60..=179) => '+',
                    _ => '.',
                });
            }
            println!("|{line}|");
        }
    }
}
