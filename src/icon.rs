//! The application icon, embedded in the binary.

use std::io::Cursor;

use anyhow::Context as _;

const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// A decoded image: 8-bit RGBA pixels, row by row.
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Decodes the embedded application icon.
pub fn app_icon() -> anyhow::Result<RgbaImage> {
    decode_rgba(ICON_PNG).context("decoding the application icon")
}

fn decode_rgba(png_bytes: &[u8]) -> anyhow::Result<RgbaImage> {
    let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
    // Expand palette and low bit depths, and reduce 16-bit channels, to 8-bit RGB(A).
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;

    let (color_type, _) = reader.output_color_type();
    anyhow::ensure!(
        color_type == png::ColorType::Rgba,
        "expected an RGBA image, found {color_type:?}"
    );

    let size = reader.output_buffer_size().context("image is too large")?;
    let mut pixels = vec![0; size];
    let frame = reader.next_frame(&mut pixels)?;
    pixels.truncate(frame.buffer_size());

    Ok(RgbaImage {
        width: frame.width,
        height: frame.height,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::app_icon;

    #[test]
    fn embedded_icon_decodes_to_rgba() {
        let icon = app_icon().expect("the embedded icon decodes");
        assert_eq!(icon.pixels.len(), (icon.width * icon.height * 4) as usize);
    }
}
