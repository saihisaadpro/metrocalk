//! M11.3 inc.2 (ADR-042) — decode a Radiance `.hdr` environment map (an equirectangular HDR panorama)
//! into a linear RGB float buffer the renderer turns into its IBL env + skybox. Like every decoder in this
//! crate (invariant 5) the foreign `image::` type stays inside: only the plain [`HdrEnv`] crosses out, so
//! the env can equally be fed from a content-addressed store handle (the bytes) or a file on disk.

use std::io::Cursor;

use image::ImageFormat;

use crate::source::ImportError;

/// A decoded equirectangular HDR panorama: `width × height` linear-RGB texels, row-major, top row first.
pub struct HdrEnv {
    pub width: u32,
    pub height: u32,
    /// Linear radiance per texel (`[r, g, b]`); HDR, so values exceed 1.0 (a sky's sun).
    pub pixels: Vec<[f32; 3]>,
}

/// Decode Radiance `.hdr` bytes into an [`HdrEnv`]. Errors (not panics) on malformed input or an empty image.
pub fn load_hdr_equirect(bytes: &[u8]) -> Result<HdrEnv, ImportError> {
    let img = image::ImageReader::with_format(Cursor::new(bytes), ImageFormat::Hdr)
        .decode()
        .map_err(|e| ImportError::Malformed(format!("hdr decode: {e}")))?
        .to_rgb32f();
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 {
        return Err(ImportError::Malformed("hdr decode: empty image".into()));
    }
    let pixels = img.pixels().map(|p| [p[0], p[1], p[2]]).collect();
    Ok(HdrEnv {
        width,
        height,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgb};

    /// Encode a tiny HDR gradient (with an above-1.0 "sun" texel) to `.hdr`, decode it back, and check the
    /// dimensions + that the HDR highlight survived (Radiance RGBE is lossy, so values are approximate).
    #[test]
    #[allow(clippy::cast_precision_loss)] // tiny fixed-size test buffer; the index casts are exact
    fn hdr_round_trips_dimensions_and_keeps_an_hdr_highlight() {
        let (w, h) = (4u32, 2u32);
        let mut buf = ImageBuffer::<Rgb<f32>, Vec<f32>>::new(w, h);
        for (x, _y, px) in buf.enumerate_pixels_mut() {
            *px = Rgb([0.2 * x as f32, 0.3, 0.5]);
        }
        buf.put_pixel(0, 0, Rgb([8.0, 7.0, 6.0])); // an HDR "sun" ≫ 1
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb32F(buf)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Hdr)
            .expect("encode hdr");

        let env = load_hdr_equirect(&bytes).expect("decode hdr");
        assert_eq!((env.width, env.height), (w, h));
        assert_eq!(env.pixels.len(), (w * h) as usize);
        assert!(env.pixels[0][0] > 4.0, "the HDR sun survived (>1.0)");
    }

    #[test]
    fn malformed_bytes_error_not_panic() {
        assert!(load_hdr_equirect(b"not an hdr file").is_err());
    }
}
