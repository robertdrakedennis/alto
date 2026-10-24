//! Screenshot files: RGB8 PNGs.

/// Encode `rgb` (`w*h*3` bytes, top row first) as a PNG file.
pub fn write_rgb_png(path: &std::path::Path, w: u32, h: u32, rgb: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        rgb.len() == w as usize * h as usize * 3,
        "png: rgb buffer size mismatch"
    );
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgb)?;
    Ok(())
}
