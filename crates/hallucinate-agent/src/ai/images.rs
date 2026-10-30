//! Image validation, resizing, and base64 encoding for vision-capable models.

use std::io::Cursor;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, GenericImageView, ImageFormat, imageops::FilterType};

use crate::{
    agent::tools::{
        ToolLimits,
        error::{ImageError, InternalToolError},
        limits::run_blocking,
    },
    ai::types::ImageContent,
};

/// Largest dimension sent to a model when automatic resizing is enabled.
pub const MAX_IMAGE_DIMENSION: u32 = 2_000;
/// Largest encoded image payload accepted before further resizing is attempted.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Load a supported image file and prepare it for an API request.
///
/// File I/O and decoding run behind the shared blocking-work boundary so image
/// attachments cannot stall an async CLI or RPC worker.
pub async fn load_image_file(
    path: impl AsRef<std::path::Path>,
    auto_resize: bool,
) -> Result<ImageContent, InternalToolError> {
    let path = path.as_ref().to_path_buf();
    let display_path = crate::config::display_path(&path, ".");
    let max_source_bytes = ToolLimits::default().max_image_source_bytes;

    run_blocking("image processing", move || {
        let metadata = std::fs::metadata(&path)
            .map_err(|_| ImageError::FailedToInspect(display_path.clone()))?;

        if metadata.len() > max_source_bytes as u64 {
            return Err(ImageError::FileTooLarge {
                display_path: display_path.clone(),
                source_bytes: metadata.len(),
                max_source_bytes,
            })?;
        }

        let bytes =
            std::fs::read(&path).map_err(|_| ImageError::FailedToRead(display_path.clone()))?;

        if bytes.len() > max_source_bytes {
            return Err(ImageError::ConvertedImageLarger(display_path.clone()))?;
        }

        Ok(process_image_bytes(&bytes, auto_resize)
            .map_err(|_| ImageError::FailedToProcess(display_path.clone()))?)
    })
    .await
}

/// Return whether bytes are one of the supported image formats.
pub fn is_supported_image(bytes: &[u8]) -> bool {
    image::guess_format(bytes)
        .ok()
        .and_then(mime_type_for_format)
        .is_some()
}

/// Validate, optionally resize, and base64 encode image bytes.
///
/// PNG, JPEG, GIF, WebP, and BMP are accepted. BMP images are always converted
/// to PNG because the OpenAI chat-completions image format does not support
/// BMP.
pub fn process_image_bytes(
    bytes: &[u8],
    auto_resize: bool,
) -> Result<ImageContent, ImageError> {
    let format = image::guess_format(bytes).map_err(|_| ImageError::InvalidData)?;

    let original_mime_type =
        mime_type_for_format(format).ok_or_else(|| ImageError::UnsupportedFormat)?;

    let decoded = image::load_from_memory_with_format(bytes, format)
        .map_err(|_| ImageError::DecodeFailure)?;

    let (width, height) = decoded.dimensions();

    if width == 0 || height == 0 {
        return Err(ImageError::NonZeroDimensions);
    }

    let requires_conversion = format == ImageFormat::Bmp;

    let exceeds_limits = width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || bytes.len() > MAX_IMAGE_BYTES;

    if !requires_conversion && (!auto_resize || !exceeds_limits) {
        return Ok(ImageContent::new(
            STANDARD.encode(bytes),
            original_mime_type,
        ));
    }

    let mut image = resize_to_fit(decoded, MAX_IMAGE_DIMENSION);

    let mut encoded = encode_png(&image)?;

    if auto_resize {
        // PNG size varies with image data, so progressively reduce dimensions
        // until the inline payload fits the byte limit.
        while encoded.len() > MAX_IMAGE_BYTES && image.width() > 1 && image.height() > 1 {
            let next_width = (image.width() * 4 / 5).max(1);
            let next_height = (image.height() * 4 / 5).max(1);

            image = image.resize_exact(next_width, next_height, FilterType::Lanczos3);

            encoded = encode_png(&image)?;
        }

        if encoded.len() > MAX_IMAGE_BYTES {
            return Err(ImageError::StillAboveMaxBytes(MAX_IMAGE_BYTES));
        }
    }

    Ok(ImageContent::new(STANDARD.encode(encoded), "image/png"))
}

/// Resize an image to fit within the given maximum dimension, preserving aspect
/// ratio.
fn resize_to_fit(
    image: DynamicImage,
    max_dimension: u32,
) -> DynamicImage {
    if image.width() <= max_dimension && image.height() <= max_dimension {
        image
    } else {
        image.resize(max_dimension, max_dimension, FilterType::Lanczos3)
    }
}

/// Encode a `DynamicImage` as a PNG byte buffer.
fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, ImageError> {
    let mut output = Cursor::new(Vec::new());

    image
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|_| ImageError::FailedToEncodeToPNG)?;

    Ok(output.into_inner())
}

/// Return the MIME type string for a supported image format.
fn mime_type_for_format(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::WebP => Some("image/webp"),
        ImageFormat::Bmp => Some("image/bmp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let image = DynamicImage::new_rgba8(width, height);
        encode_png(&image).unwrap()
    }

    #[test]
    fn base64_encodes_supported_images_without_unnecessary_changes() {
        let bytes = png_bytes(2, 2);
        let image = process_image_bytes(&bytes, true).unwrap();

        assert_eq!(image.mime_type, "image/png");
        assert_eq!(STANDARD.decode(image.data).unwrap(), bytes);
    }

    #[test]
    fn resizes_images_that_exceed_the_dimension_limit() {
        let image = process_image_bytes(&png_bytes(2_100, 100), true).unwrap();
        let decoded = STANDARD.decode(image.data).unwrap();
        let resized = image::load_from_memory(&decoded).unwrap();

        assert_eq!(image.mime_type, "image/png");
        assert_eq!(resized.dimensions(), (2_000, 95));
    }

    #[test]
    fn rejects_non_image_data() {
        assert!(process_image_bytes(b"not an image", true).is_err());
    }
}
