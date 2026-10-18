//! Writes the packaged icon files to `packaging/icons/` from the icon drawn in `src/icon.rs`:
//! PNGs for Linux, `gravekeeper.ico` for Windows and `gravekeeper.icns` for macOS.
//!
//!     cargo run -p gravekeeper --example gen_icons

#[path = "../src/icon.rs"]
mod icon;

use std::path::Path;

const PNG_SIZES: [u32; 8] = [16, 32, 48, 64, 128, 256, 512, 1024];
const ICO_SIZES: [u32; 5] = [16, 32, 48, 64, 256];
/// ICNS element types holding PNG data, by pixel size.
const ICNS_TYPES: [(&[u8; 4], u32); 7] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
];

fn png(size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer
        .write_image_data(&icon::rgba(size))
        .expect("png data");
    writer.finish().expect("png finish");
    out
}

/// An ICO file whose images are PNG-compressed (supported since Windows Vista).
fn ico() -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = ICO_SIZES.iter().map(|&s| (s, png(s))).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in &images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &images {
        out.extend_from_slice(data);
    }
    out
}

fn icns() -> Vec<u8> {
    let mut body = Vec::new();
    for (kind, size) in ICNS_TYPES {
        let data = png(size);
        body.extend_from_slice(kind);
        body.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(&data);
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/icons");
    std::fs::create_dir_all(&dir).expect("create packaging/icons");
    for size in PNG_SIZES {
        std::fs::write(dir.join(format!("{size}x{size}.png")), png(size)).expect("write png");
    }
    std::fs::write(dir.join("gravekeeper.ico"), ico()).expect("write ico");
    std::fs::write(dir.join("gravekeeper.icns"), icns()).expect("write icns");
    println!("wrote icons to {}", dir.display());
}
