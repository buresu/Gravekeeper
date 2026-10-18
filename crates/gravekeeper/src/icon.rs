//! The Gravekeeper icon, drawn in code so no image decoder is needed at runtime.
//!
//! The same function produces the window/tray icon and the packaged icon files
//! (`cargo run -p gravekeeper --example gen_icons`).

/// Supersampling factor per axis for anti-aliased edges.
const SAMPLES: u32 = 4;

#[derive(Clone, Copy)]
struct Rgb(f32, f32, f32);

const STONE: Rgb = Rgb(0.62, 0.66, 0.71);
const STONE_LIGHT: Rgb = Rgb(0.78, 0.81, 0.85);
const OUTLINE: Rgb = Rgb(0.17, 0.19, 0.22);
const CROSS: Rgb = Rgb(0.24, 0.27, 0.31);
const GRASS: Rgb = Rgb(0.30, 0.62, 0.36);

/// RGBA pixels (row-major, 4 bytes per pixel) of a `size`×`size` icon.
pub fn rgba(size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    let step = 1.0 / (size * SAMPLES) as f32;
    for py in 0..size {
        for px in 0..size {
            let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px * SAMPLES + sx) as f32 * step + step / 2.0;
                    let y = (py * SAMPLES + sy) as f32 * step + step / 2.0;
                    if let Some(Rgb(cr, cg, cb)) = shade(x, y) {
                        r += cr;
                        g += cg;
                        b += cb;
                        a += 1.0;
                    }
                }
            }
            let n = (SAMPLES * SAMPLES) as f32;
            // Premultiplied average, converted back to straight alpha.
            let (r, g, b) = if a > 0.0 {
                (r / a, g / a, b / a)
            } else {
                (0.0, 0.0, 0.0)
            };
            let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            out.extend_from_slice(&[to_u8(r), to_u8(g), to_u8(b), to_u8(a / n)]);
        }
    }
    out
}

/// Color at a point in the unit square, or `None` for transparent.
fn shade(x: f32, y: f32) -> Option<Rgb> {
    // Grass strip at the bottom.
    if y > 0.84 && y < 0.95 && (x - 0.5).abs() < 0.44 {
        return Some(GRASS);
    }
    // Tombstone: a rectangle with a semicircular top.
    let (cx, half_w, top, bottom) = (0.5, 0.30, 0.10, 0.88);
    let arch_y = top + half_w;
    let inside = |inset: f32| {
        let w = half_w - inset;
        if y < arch_y {
            (x - cx).powi(2) + (y - arch_y).powi(2) <= w * w
        } else {
            (x - cx).abs() <= w && y <= bottom - inset
        }
    };
    if !inside(0.0) {
        return None;
    }
    if !inside(0.035) {
        return Some(OUTLINE);
    }
    // Engraved cross.
    let bar = 0.045;
    if ((x - cx).abs() < bar && y > 0.26 && y < 0.66)
        || ((y - 0.38).abs() < bar && (x - cx).abs() < 0.15)
    {
        return Some(CROSS);
    }
    // Light from the upper left.
    let t = ((x - 0.2) * 0.6 + (y - 0.1) * 0.8).clamp(0.0, 1.0);
    Some(mix(STONE_LIGHT, STONE, t))
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    Rgb(
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_transparent_corners_and_an_opaque_center() {
        let size = 32;
        let px = rgba(size);
        assert_eq!(px.len(), (size * size * 4) as usize);
        let alpha = |x: u32, y: u32| px[((y * size + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(16, 20), 255);
        // Edges are anti-aliased: some pixels are partially covered.
        assert!(px.chunks(4).any(|p| p[3] > 0 && p[3] < 255));
    }
}
