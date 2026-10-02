//! Turning a file's bytes into something the data viewer can draw. Images today; the viewer
//! is generic over [`Decoded`] so another format is another variant and another `decode`
//! branch, not another panel.

use std::sync::Arc;

use gpui_kit::{Image, ImageFormat, RenderImage};

/// A file's contents, ready to draw.
#[derive(Clone)]
pub enum Decoded {
    /// A raster image, decoded to pixels.
    Raster {
        image: Arc<RenderImage>,
        width: u32,
        height: u32,
        format: &'static str,
    },
    /// An SVG, rendered by gpui itself, so it has no pixel size of its own.
    Svg(Arc<Image>),
}

impl Decoded {
    pub fn describe(&self) -> String {
        match self {
            Decoded::Raster {
                width,
                height,
                format,
                ..
            } => format!("{format} image, {width} × {height}"),
            Decoded::Svg(_) => "SVG image".into(),
        }
    }
}

fn looks_like_svg(bytes: &[u8], name: &str) -> bool {
    if name.to_ascii_lowercase().ends_with(".svg") {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    head.contains("<svg")
}

pub fn decode(bytes: &[u8], name: &str) -> Result<Decoded, String> {
    if looks_like_svg(bytes, name) {
        return Ok(Decoded::Svg(Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            bytes.to_vec(),
        ))));
    }
    let format = image::guess_format(bytes)
        .map_err(|_| "not an image format the viewer knows".to_string())?;
    let mut rgba = image::load_from_memory_with_format(bytes, format)
        .map_err(|e| format!("could not decode the image: {e}"))?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    // gpui's pixel order is BGRA.
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let frame = image::Frame::new(rgba);
    Ok(Decoded::Raster {
        image: Arc::new(RenderImage::new(vec![frame])),
        width,
        height,
        format: match format {
            image::ImageFormat::Png => "PNG",
            image::ImageFormat::Jpeg => "JPEG",
            image::ImageFormat::Gif => "GIF",
            image::ImageFormat::WebP => "WebP",
            image::ImageFormat::Bmp => "BMP",
            image::ImageFormat::Tiff => "TIFF",
            image::ImageFormat::Ico => "ICO",
            _ => "raster",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn decodes_a_png_with_its_size() {
        match decode(&png(3, 2), "x.png").unwrap() {
            Decoded::Raster {
                width,
                height,
                format,
                ..
            } => assert_eq!((width, height, format), (3, 2, "PNG")),
            _ => panic!("expected a raster"),
        }
    }

    #[test]
    fn svg_by_name_or_content() {
        assert!(matches!(
            decode(b"<svg xmlns='x'/>", "noext"),
            Ok(Decoded::Svg(_))
        ));
        assert!(matches!(decode(b"anything", "a.SVG"), Ok(Decoded::Svg(_))));
    }

    #[test]
    fn other_bytes_are_an_error_not_a_panic() {
        assert!(decode(b"plain text, not an image", "a.txt").is_err());
        assert!(decode(&png(3, 2)[..20], "x.png").is_err());
    }
}
