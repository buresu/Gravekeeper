//! The Gravekeeper icon, scaled down from the artwork in `packaging/icons/`.
//!
//! `packaging/icons/1024x1024.png` is the source; the other packaged icon files are
//! generated from it (`cargo run -p gravekeeper --example gen_icons`). The window and
//! tray icons are scaled at runtime from the embedded 256×256 PNG.

/// Embedded for the window and tray, which need at most 128×128.
const EMBEDDED: &[u8] = include_bytes!("../../../packaging/icons/256x256.png");

/// RGBA pixels (row-major, 4 bytes per pixel) of a `size`×`size` icon, up to 256×256.
pub fn rgba(size: u32) -> Vec<u8> {
    let (pixels, src_size) = decode(EMBEDDED);
    downscale(&pixels, src_size, size)
}

/// Decodes a square 8-bit RGBA PNG into its pixels and side length.
pub fn decode(png_data: &[u8]) -> (Vec<u8>, u32) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_data))
        .read_info()
        .expect("icon PNG header");
    let mut pixels = vec![0; reader.output_buffer_size().expect("icon PNG size")];
    let info = reader.next_frame(&mut pixels).expect("icon PNG data");
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgba, png::BitDepth::Eight),
        "icon PNG must be 8-bit RGBA"
    );
    assert_eq!(info.width, info.height, "icon PNG must be square");
    pixels.truncate(info.buffer_size());
    (pixels, info.width)
}

/// Scales square RGBA pixels down to `size`, averaging the source area each pixel covers.
///
/// Colors are weighted by alpha, so transparent pixels do not darken or lighten the edges.
pub fn downscale(pixels: &[u8], src_size: u32, size: u32) -> Vec<u8> {
    assert!(
        size > 0 && size <= src_size,
        "cannot scale {src_size} to {size}"
    );
    let spans: Vec<Vec<(usize, f64)>> = (0..size).map(|i| span(i, src_size, size)).collect();
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for ys in &spans {
        for xs in &spans {
            let mut sum = [0.0; 4];
            for &(sy, wy) in ys {
                for &(sx, wx) in xs {
                    let p = &pixels[(sy * src_size as usize + sx) * 4..][..4];
                    let a = f64::from(p[3]) * wx * wy;
                    for c in 0..3 {
                        sum[c] += f64::from(p[c]) * a;
                    }
                    sum[3] += a;
                }
            }
            // Premultiplied average, converted back to straight alpha.
            let a = sum[3];
            let straight = |v: f64| if a > 0.0 { (v / a).round() as u8 } else { 0 };
            out.extend_from_slice(&[
                straight(sum[0]),
                straight(sum[1]),
                straight(sum[2]),
                a.round().clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    out
}

/// Source pixels covered by output pixel `i`, with the fraction of the output pixel each covers.
fn span(i: u32, src_size: u32, size: u32) -> Vec<(usize, f64)> {
    let scale = f64::from(src_size) / f64::from(size);
    let (start, end) = (f64::from(i) * scale, f64::from(i + 1) * scale);
    (start.floor() as usize..end.ceil() as usize)
        .map(|s| {
            let covered = (s as f64 + 1.0).min(end) - (s as f64).max(start);
            (s, covered / scale)
        })
        .filter(|&(_, w)| w > 0.0)
        .collect()
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
        assert_eq!(alpha(16, 16), 255);
        // Edges are anti-aliased: some pixels are partially covered.
        assert!(px.chunks(4).any(|p| p[3] > 0 && p[3] < 255));
    }

    #[test]
    fn downscaling_ignores_the_color_of_transparent_pixels() {
        // One opaque black pixel next to three transparent white ones.
        let px = [
            0, 0, 0, 255, 255, 255, 255, 0, 255, 255, 255, 0, 255, 255, 255, 0,
        ];
        assert_eq!(downscale(&px, 2, 1), [0, 0, 0, 64]);
    }

    #[test]
    fn downscaling_by_a_fractional_factor_keeps_full_coverage() {
        let px = [200u8, 100, 50, 255].repeat(3 * 3);
        assert_eq!(downscale(&px, 3, 2), [200, 100, 50, 255].repeat(2 * 2));
    }
}
