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
//! What this module does NOT judge is whether the bucket a well-formed destination names is
//! THERE, or whose it is: the bucket lives in Amazon S3, not in this service's state, so
//! both of those are resolved against that service ([`super::referenced_resources`]). What
//! this module DOES judge about it is the one precondition the service evaluates per call
//! before it accepts a definition at all — the destination bucket's own policy must grant
//! this service's principal the object write the delivery performs
//! ([`validate_delivery_grant`]).

use super::{clip_for_message, field_validation_failed};
use crate::ops::types::{DestinationConfigurations, S3Destination, ValidationException};

/// The input member a refused output encoding names: the compression is the member whose
/// value does not go with the format the same request states.
const COMPRESSION_FIELD: &str =
    "Export.DestinationConfigurations.S3Destination.S3OutputConfigurations.Compression";

/// The input member a refused destination Region names.
const S3_REGION_FIELD: &str = "Export.DestinationConfigurations.S3Destination.S3Region";

/// The input member a refused destination BUCKET names.
///
/// `pub(crate)` because the resolution of that bucket against the service that HOLDS it
/// ([`super::referenced_resources`]) names the same member rather than a second spelling of
/// it.
pub(crate) const S3_BUCKET_FIELD: &str =
    "Export.DestinationConfigurations.S3Destination.S3Bucket";

/// The input member a refusal of the destination's stated bucket OWNER names.
pub(crate) const S3_BUCKET_OWNER_FIELD: &str =
    "Export.DestinationConfigurations.S3Destination.S3BucketOwner";

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

/// The AWS service principal a data export is delivered in the name of, and therefore the
/// principal a destination bucket's policy has to grant the write to.
///
/// The service writes the export into the caller's bucket itself, on the cadence the
/// definition states, rather than in the caller's name — so the grant that lets the
/// delivery happen is one the BUCKET's policy carries, naming this service.
pub const DELIVERY_SERVICE_PRINCIPAL: &str = "bcm-data-exports.amazonaws.com";

/// The Amazon S3 action one delivery performs: it writes the export's file into the
/// destination bucket.
pub const DELIVERY_ACTION: &str = "s3:PutObject";

/// The condition key naming the account a delivery is made on behalf of.
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The condition key naming the resource a delivery is made on behalf of: the export being
/// delivered.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The condition key saying whether the request was sent over a secure transport.
///
/// A delivery this service performs answers it rather than leaving it open: the emulator
/// serves every endpoint over TLS, and the write goes through one of them. A bucket whose
/// policy carries the usual clause refusing everything that did NOT arrive that way
/// therefore refuses nothing here — where a key with no value would leave that clause
/// undecided, and an undecided refusal still refuses.
const SECURE_TRANSPORT_KEY: &str = "aws:SecureTransport";

/// The value that key carries, as IAM's boolean literals spell it.
const SECURE_TRANSPORT_VALUE: &str = "true";

/// The condition keys a delivery carries a value for, so a clause over one of them is
/// decided rather than left undecided.
const DELIVERY_CONDITION_KEYS: &[&str] =
    &[SOURCE_ACCOUNT_KEY, SOURCE_ARN_KEY, SECURE_TRANSPORT_KEY];

/// The `Message` a creation refused for the destination bucket's missing delivery grant
/// carries.
///
/// RECORDED AWS behaviour: the refusal is entirely fixed text naming neither the bucket,
/// the action nor the principal — a bucket with no policy at all and one whose policy
/// grants this service nothing are the same answer to the caller, which is that the
/// destination's permissions were checked and did not pass.
pub const BUCKET_PERMISSION_VALIDATION_FAILED: &str = "S3 bucket permission validation failed";

/// Judge the destination bucket's own POLICY, the way the service does before it accepts a
/// definition: `Ok(())` when the policy grants [`DELIVERY_SERVICE_PRINCIPAL`] the
/// [`DELIVERY_ACTION`] on the objects a delivery writes, and the modelled
/// `ValidationException` (HTTP 400) when it does not.
///
/// RECORDED AWS behaviour: this precondition is evaluated PER CALL, not once per bucket —
/// the identical call refused against an unpolicied bucket succeeds as soon as the policy
/// is put on that same bucket — and a definition it refuses is not persisted, so a client
/// whose destination could never be written to learns that from the create rather than
/// from a delivery that silently never happens. The decision is
/// [`::service_common::iam_authorization`]'s resource-policy evaluation, the same one every
/// other service judges a bucket policy with, and it carries the condition keys a
/// cross-service write is made under ([`DELIVERY_CONDITION_KEYS`]), so a policy scoped to
/// this account and to this service's exports decides as it would in production.
///
/// `export_arn` is the export the delivery is for: the ARN the create just minted, or the
/// ARN of the export an update replaces.
///
/// A destination whose POLICY this emulator's object store can say nothing about — no object
/// store on this engine at all, or a bucket it does not hold — is not refused here: a refusal
/// from this function has to be grounded in a policy that was read. Whether the bucket is
/// there at all is a different fact about a different service, resolved against that service
/// before this runs ([`super::referenced_resources::destination_bucket_verdict`]), and a
/// delivery that finds the bucket gone later reports it as the delivery it could not make.
pub fn validate_delivery_grant<Eng: Send + Sync + 'static>(
    engine: &Eng,
    destination: &DestinationConfigurations,
    export_arn: &str,
) -> Result<(), ValidationException> {
    let s3 = &destination.s3_destination;
    let Some(objects) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return Ok(());
    };
    // The object store is read, the one document this decision is about is taken out of it,
    // and the lock is released before the policy is evaluated: the two services' stores are
    // never held at once.
    let policy = {
        let store = objects.lock();
        match store.bucket(&s3.s3_bucket) {
            // A bucket this emulator holds either carries a policy or carries none, and a
            // bucket with none grants nothing — which is exactly the recorded refusal.
            Some(bucket) => bucket.policy.clone().unwrap_or_default(),
            None => return Ok(()),
        }
    };
    let principals = [DELIVERY_SERVICE_PRINCIPAL.to_string()];
    let context = [
        (
            SOURCE_ACCOUNT_KEY.to_string(),
            ::engine::emulator_account_id().to_string(),
        ),
        (SOURCE_ARN_KEY.to_string(), export_arn.to_string()),
        (
            SECURE_TRANSPORT_KEY.to_string(),
            SECURE_TRANSPORT_VALUE.to_string(),
        ),
    ];
    let objects_written = delivery_object_arn(s3);
    let request = ::service_common::iam_authorization::Request::new(
        DELIVERY_ACTION,
        &objects_written,
        &principals,
    )
    .with_context(&context)
    .with_published_keys(DELIVERY_CONDITION_KEYS);
    if ::service_common::iam_authorization::resource_decision(&policy, &request)
        == ::service_common::iam_authorization::Decision::Allowed
    {
        return Ok(());
    }
    Err(delivery_grant_missing())
}

/// The ARN of the objects a delivery to `s3` writes: the destination bucket and the prefix
/// under it the export's files are written below ("The S3 path prefix you want prepended to
/// the name of your data export" — the modelled doc for `S3Prefix`).
///
/// Spelled through Amazon S3's own object-ARN helper, so this service names an object the
/// way that service names one, and ending in the key separator because what is written
/// under the prefix is a path and not the prefix itself: a policy that grants the bucket's
/// objects (`arn:aws:s3:::<bucket>/*`) covers it, while one that grants an unrelated prefix
/// does not.
fn delivery_object_arn(s3: &S3Destination) -> String {
    let prefix = s3.s3_prefix.trim_matches('/');
    let key = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/")
    };
    ::service_com_amazonaws_s3_amazons3::state::object_arn(&s3.s3_bucket, &key)
}

/// The modelled 400 for a destination whose bucket policy does not grant this service the
/// delivery write.
///
/// The member at fault is the destination's `S3Bucket`: the bucket named is the one whose
/// policy has to carry the grant, and it is what the caller changes (either by naming
/// another bucket or by putting the policy on this one).
fn delivery_grant_missing() -> ValidationException {
    field_validation_failed(
        S3_BUCKET_FIELD,
        BUCKET_PERMISSION_VALIDATION_FAILED.to_string(),
        format!(
            "The bucket policy must grant {DELIVERY_SERVICE_PRINCIPAL} the {DELIVERY_ACTION} action on the destination."
        ),
    )
}

/// The modelled 400 for an `S3Region` that names no Amazon Web Services Region.
///
/// The sentence is built from the request — it echoes the offending value, clipped to the
/// message member's own modelled bound — because that value is the only part of the
/// request the caller has to change.
fn unknown_destination_region(region: &str) -> ValidationException {
    field_validation_failed(
        S3_REGION_FIELD,
        format!("S3Region {} is invalid", clip_for_message(region)),
        "Must name the Amazon Web Services Region the S3 bucket is in.".to_string(),
    )
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
    field_validation_failed(
        COMPRESSION_FIELD,
        format!(
            "Compression {} is invalid for Format {}",
            clip_for_message(compression),
            clip_for_message(format)
        ),
        requirement,
    )
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
