//! Turning a file's bytes into something the data viewer can show: images, Markdown (rendered)
//! and property lists (binary and XML, as a tree). The viewer is generic over [`Decoded`] so
//! another format is another variant and another `decode` branch, not another panel.

use std::sync::Arc;

use gpui_kit::{Image, ImageFormat, RenderImage};

use super::plist;

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
    /// Markdown text: shown rendered, or as the source.
    Markdown(Arc<String>),
    /// A property list, binary or XML, read into a tree.
    Plist(Arc<plist::Document>),
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
            Decoded::Markdown(text) => format!(
                "Markdown, {} line{}",
                text.lines().count(),
                if text.lines().count() == 1 { "" } else { "s" }
            ),
            Decoded::Plist(doc) => match &doc.archive {
                Some(_) => format!("keyed archive, {}", doc.raw.summary()),
                None => format!("property list, {}", doc.raw.summary()),
            },
        }
    }
}

/// Whether a name is a Markdown file's.
pub fn is_markdown_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["md", "markdown", "mdown", "mkd", "mkdn"]
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

fn looks_like_svg(bytes: &[u8], name: &str) -> bool {
    if name.to_ascii_lowercase().ends_with(".svg") {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    head.contains("<svg")
}

pub fn decode(bytes: &[u8], name: &str) -> Result<Decoded, String> {
    // A property list is known by its content: `bplist` first, or XML with a `<plist>` root.
    if bytes.starts_with(b"bplist") || plist::looks_like_xml_plist(bytes) {
        return plist::Document::read(bytes)
            .map(|doc| Decoded::Plist(Arc::new(doc)))
            .map_err(|e| format!("could not read the property list: {e}"));
    }
    // Markdown is plain text, so only its name says so.
    if is_markdown_name(name) {
        return Ok(Decoded::Markdown(Arc::new(
            String::from_utf8_lossy(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))
                .into_owned(),
        )));
    }
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
    fn markdown_is_known_by_its_name_and_a_property_list_by_its_content() {
        match decode(b"# Title\n\ntext\n", "notes.MD").unwrap() {
            Decoded::Markdown(t) => assert_eq!(t.as_str(), "# Title\n\ntext\n"),
            _ => panic!("expected Markdown"),
        }
        assert_eq!(
            decode(b"# a\nb", "x.markdown").unwrap().describe(),
            "Markdown, 2 lines"
        );
        let binary = include_bytes!("../../tests/fixtures/sample.bplist");
        let xml = include_bytes!("../../tests/fixtures/sample.plist");
        for (bytes, name) in [
            (&binary[..], "Info"),
            (&xml[..], "a.plist"),
            (&binary[..], "x.md"),
        ] {
            match decode(bytes, name).unwrap() {
                Decoded::Plist(doc) => assert_eq!(doc.raw.kind(), "dict", "{name}"),
                _ => panic!("{name}: expected a property list"),
            }
        }
        assert!(
            decode(&binary[..20], "broken.plist").is_err(),
            "a damaged one says so"
        );
        // A Markdown file that is really an image is still an image only if it is not named so;
        // a text file with `.txt` is still not anything this viewer knows.
        assert!(decode(b"# not named for it", "a.txt").is_err());
    }

    #[test]
    fn other_bytes_are_an_error_not_a_panic() {
        assert!(decode(b"plain text, not an image", "a.txt").is_err());
        assert!(decode(&png(3, 2)[..20], "x.png").is_err());
    }
}
