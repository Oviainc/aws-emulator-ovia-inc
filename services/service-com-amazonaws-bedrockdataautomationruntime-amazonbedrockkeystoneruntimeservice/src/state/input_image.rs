// SPDX-License-Identifier: CC0-1.0

//! Whether the asset a job was pointed at is an IMAGE this service can process.
//!
//! A data-automation job does not only have to READ its asset — it has to process it, and
//! an asset the service cannot process is a failure attributable to the request rather
//! than to the service: the job ends in the modelled `ClientError` status carrying
//! [`INVALID_IMAGE_ERROR_TYPE`] and [`INVALID_IMAGE_MESSAGE`]
//! (`crate::state::Invocation::advance_to`).
//!
//! The verdict is read off the asset's OWN BYTES, never off its key, its extension or
//! anything else the request carried: the format comes from the bytes' magic numbers and
//! the dimensions from the image header, both through the `image` crate, which is also how
//! the sibling image services in this workspace read a submitted image. Only the header is
//! read — no frame is decoded, nothing is extracted from the asset and no pixel is
//! inspected, because this service analyses nothing.
//!
//! An asset that is NOT an image ([`ImageVerdict::NotAnImage`]) is left alone: a document,
//! a video or an audio asset is a different modality with its own rules, and nothing an
//! image rule knows says anything about it.

use std::io::Cursor;

use ::image::{ImageFormat, Limits};

/// The `errorType` a job failed on an unprocessable image reports.
///
/// AWS's own type for this failure class, as the behavioural corpus recorded it; a client
/// branches on it beside the `ClientError` status.
pub const INVALID_IMAGE_ERROR_TYPE: &str = "InvalidImage";

/// The `errorMessage` that failure reports.
///
/// AWS's own wording for the class, and ENTIRELY fixed text: it names no file, no format
/// and no dimension, so there is no variable part to fill and nothing the request carried
/// reaches it.
pub const INVALID_IMAGE_MESSAGE: &str = "Image format is invalid or not supported.";

/// The image formats data automation reads: JPEG and PNG, the two image formats AWS
/// publishes as supported input for it. A file whose own magic numbers say it is an image
/// in any OTHER format is an image this service does not support.
const SUPPORTED_IMAGE_FORMATS: &[ImageFormat] = &[ImageFormat::Jpeg, ImageFormat::Png];

/// The smallest an image asset may be on a side, in pixels.
///
/// Data automation processes an image by analysing its content, so it publishes a floor
/// below which an image carries no content to analyse. The behavioural corpus pins the
/// rule at its extreme — a one-pixel image is refused as an invalid image rather than
/// processed — and this is the floor this emulator enforces for the whole class; a
/// recording of an image between this and the published figure would settle the number,
/// and would change only this constant.
pub const MIN_IMAGE_SIDE_PIXELS: u32 = 64;

/// The largest an image asset may be on a side, in pixels.
///
/// The same rule from the other end: an image past the dimensions the service processes
/// is not processed. Stated as this emulator's ceiling for the class, on the same footing
/// as [`MIN_IMAGE_SIDE_PIXELS`].
pub const MAX_IMAGE_SIDE_PIXELS: u32 = 4_096;

/// How much the header read may allocate.
///
/// The decoder's own limits are started from nothing ([`Limits::no_limits`]) rather than
/// from the crate's defaults, so the bound here is this module's: only an image's header
/// is read, which needs no frame-sized buffer.
const MAX_HEADER_ALLOC: u64 = 4 * 1024 * 1024;

/// What one asset's bytes are, as far as the image rule is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageVerdict {
    /// Not an image at all: the bytes' magic numbers are no image format's. The image
    /// rule says nothing about such an asset.
    NotAnImage,
    /// An image in a format this service reads, inside the dimensions it processes.
    Processable,
    /// An image this service cannot process: a format it does not read, a header it
    /// cannot read at all, or dimensions outside
    /// [`MIN_IMAGE_SIDE_PIXELS`]..=[`MAX_IMAGE_SIDE_PIXELS`].
    Invalid,
}

/// Read `bytes` far enough to say which [`ImageVerdict`] the asset they hold earns.
///
/// Cheap and bounded: the format comes from the leading magic numbers and the dimensions
/// from the image header, so the cost does not depend on how large the image is.
#[must_use]
pub fn image_verdict(bytes: &[u8]) -> ImageVerdict {
    let Ok(format) = ::image::guess_format(bytes) else {
        // Nothing here recognises these bytes as an image, so this asset is one of the
        // other modalities (or something the emulator cannot classify) and the image rule
        // does not reach it.
        return ImageVerdict::NotAnImage;
    };
    if !SUPPORTED_IMAGE_FORMATS.contains(&format) {
        // It IS an image, and in a format data automation does not read.
        return ImageVerdict::Invalid;
    }
    let mut reader = ::image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::no_limits();
    limits.max_alloc = Some(MAX_HEADER_ALLOC);
    reader.limits(limits);
    let Ok((width, height)) = reader.into_dimensions() else {
        // The bytes claim an image format and no header can be read out of them: an
        // invalid image, which is exactly what the message for this class says.
        return ImageVerdict::Invalid;
    };
    if within_bounds(width) && within_bounds(height) {
        ImageVerdict::Processable
    } else {
        ImageVerdict::Invalid
    }
}

/// Whether one side of an image is inside the dimensions this service processes.
fn within_bounds(side: u32) -> bool {
    (MIN_IMAGE_SIDE_PIXELS..=MAX_IMAGE_SIDE_PIXELS).contains(&side)
}
