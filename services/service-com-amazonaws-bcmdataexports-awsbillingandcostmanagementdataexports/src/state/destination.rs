// SPDX-License-Identifier: CC0-1.0

//! The `DestinationConfigurations` a submitted `Export` carries: the S3 destination an
//! export is delivered to, and the refusals for a destination this service could not
//! deliver to.
//!
//! `DestinationConfigurations` is `@required` on `Export`, and its `S3Destination`
//! states WHERE a delivery is written (`S3Bucket`, `S3Prefix`, `S3Region`,
//! `S3BucketOwner`) and IN WHAT ENCODING (`S3OutputConfigurations`: `OutputType`,
//! `Format`, `Compression`, `Overwrite` — smithy_query shape `S3Destination`). The model
//! constrains each member on its own (each output member is an enum, `S3BucketOwner` is a
//! 12-digit account id), and the boundary validator already refuses a value outside one
//! of those; what it cannot express is a COMBINATION of members that no export could be
//! delivered under. Two such facts are checked here:
//!
//! * the output ENCODING — "The file format for the data export" paired with "The
//!   compression type for the data export" (the modelled docs for `Format` and
//!   `Compression`) — because a data export is written as ONE file per delivery, and the
//!   only encodings the service produces are the published combinations in
//!   [`SUPPORTED_OUTPUT_ENCODINGS`]: a format compressed in a way the service does not
//!   produce names a file it could never write;
//! * the `S3Region` — "The S3 bucket Region" (the modelled doc for `S3Region`) — whose
//!   own `@pattern ^[\S\s]*$` admits any text, so a value that names no Amazon Web
//!   Services Region at all names no bucket to deliver to.
//!
//! Both are read from the SUBMITTED definition and neither touches the store, so they are
//! judged identically by every operation that accepts an export definition — `CreateExport`
//! and `UpdateExport`, the second of which "[updates] an existing data export by
//! overwriting all export parameters" and therefore re-judges the replacement destination
//! exactly as a create's.
//!
//! What this module does NOT judge is whether a well-formed destination is one the
//! caller's credentials may write: the bucket's existence and its bucket policy live in
//! Amazon S3, not in this service's state, so a destination inside the rules above is
//! accepted and the delivery it owes is recorded like any other.

use super::{clip_for_message, VALIDATION_REASON_FIELD_FAILED};
use crate::ops::types::{
    DestinationConfigurations, ValidationException, ValidationExceptionField,
};

/// The input member a refused output encoding names: the compression is the member whose
/// value does not go with the format the same request states.
const COMPRESSION_FIELD: &str =
    "Export.DestinationConfigurations.S3Destination.S3OutputConfigurations.Compression";

/// The input member a refused destination Region names.
const S3_REGION_FIELD: &str = "Export.DestinationConfigurations.S3Destination.S3Region";

/// The encodings a data export is delivered in: each `FormatOption` the service writes,
/// with the `CompressionOption` values it writes that format under.
///
/// SOURCE: the AWS Billing and Cost Management user guide's data-export delivery options,
/// which publish the compression-and-file-format choices as the pairs
/// `Parquet - parquet`, `gzip - text/csv` and `zip - text/csv`; the enum spellings are
/// the modelled ones (`crate::ops::types::FORMAT_OPTION_VALUES`,
/// `COMPRESSION_OPTION_VALUES`). Every published `FormatOption` appears here, so a format
/// missing from this table is a format this service does not write at all.
pub const SUPPORTED_OUTPUT_ENCODINGS: &[(&str, &[&str])] = &[
    ("TEXT_OR_CSV", &["GZIP", "ZIP"]),
    ("PARQUET", &["PARQUET"]),
];

/// The compressions this service writes `format` under, or `None` when it writes no file
/// in that format.
pub fn supported_compressions(format: &str) -> Option<&'static [&'static str]> {
    SUPPORTED_OUTPUT_ENCODINGS
        .iter()
        .find(|(published, _)| *published == format)
        .map(|(_, compressions)| *compressions)
}

/// Whether this service delivers a file in `format` compressed as `compression`.
pub fn delivers_output_encoding(format: &str, compression: &str) -> bool {
    supported_compressions(format).is_some_and(|compressions| compressions.contains(&compression))
}

/// Judge a submitted `DestinationConfigurations` the way every operation that accepts an
/// export definition must: `Ok(())` for a destination this service could deliver to, and
/// the modelled `ValidationException` (HTTP 400) for one it could not.
///
/// Nothing here reads or writes the store, so a refusal leaves whatever was stored before
/// exactly as it was.
pub fn validate_destination_configurations(
    destination: &DestinationConfigurations,
) -> Result<(), ValidationException> {
    let s3 = &destination.s3_destination;
    // "The S3 bucket Region": a value that names no Region names no bucket. The catalog is
    // `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may carry",
    // "sorted for binary search"), the same catalog every other region-valued input of
    // this emulator is read against, rather than a second list of Region codes kept here.
    if ::emulator_telemetry::policy::KNOWN_REGIONS
        .binary_search(&s3.s3_region.as_str())
        .is_err()
    {
        return Err(unknown_destination_region(&s3.s3_region));
    }

    let output = &s3.s3_output_configurations;
    if !delivers_output_encoding(&output.format, &output.compression) {
        return Err(unsupported_output_encoding(&output.format, &output.compression));
    }
    Ok(())
}

/// The modelled 400 for an `S3Region` that names no Amazon Web Services Region.
///
/// The sentence is built from the request — it echoes the offending value, clipped to the
/// message member's own modelled bound — because that value is the only part of the
/// request the caller has to change.
fn unknown_destination_region(region: &str) -> ValidationException {
    ValidationException {
        message: format!("S3Region {} is invalid", clip_for_message(region)),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: S3_REGION_FIELD.to_string(),
            message: "Must name the Amazon Web Services Region the S3 bucket is in."
                .to_string(),
        }]),
    }
}

/// The modelled 400 for a `Format`/`Compression` pair this service does not write,
/// naming the compressions the stated format IS written under.
fn unsupported_output_encoding(format: &str, compression: &str) -> ValidationException {
    let requirement = match supported_compressions(format) {
        Some(compressions) => format!(
            "Must be one of [{}] when Format is {format}.",
            compressions.join(", ")
        ),
        // No published compression goes with this format, so there is nothing to offer:
        // the formats this service writes are the ones the table publishes.
        None => format!(
            "Format must be one of [{}].",
            SUPPORTED_OUTPUT_ENCODINGS
                .iter()
                .map(|(published, _)| *published)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    ValidationException {
        message: format!(
            "Compression {} is invalid for Format {}",
            clip_for_message(compression),
            clip_for_message(format)
        ),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: COMPRESSION_FIELD.to_string(),
            message: requirement,
        }]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{COMPRESSION_OPTION_VALUES, FORMAT_OPTION_VALUES};

    /// Every value [`SUPPORTED_OUTPUT_ENCODINGS`] names is a value the MODEL publishes,
    /// so a renamed or dropped enum value cannot leave a row of the table standing as
    /// dead data that silently refuses everything.
    #[test]
    fn the_published_encodings_name_only_modelled_values() {
        for (format, compressions) in SUPPORTED_OUTPUT_ENCODINGS {
            assert!(
                FORMAT_OPTION_VALUES.contains(format),
                "case {format:?}: expected a modelled FormatOption, got one outside {FORMAT_OPTION_VALUES:?}"
            );
            assert!(
                !compressions.is_empty(),
                "case {format:?}: a published format must be delivered under some compression"
            );
            for compression in *compressions {
                assert!(
                    COMPRESSION_OPTION_VALUES.contains(compression),
                    "case {:?}: expected a modelled CompressionOption, got one outside {COMPRESSION_OPTION_VALUES:?}",
                    (format, compression)
                );
            }
        }
    }
}
