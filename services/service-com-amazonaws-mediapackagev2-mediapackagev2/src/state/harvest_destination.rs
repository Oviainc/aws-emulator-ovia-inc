// SPDX-License-Identifier: CC0-1.0

//! Pre-flighting a harvest job's DESTINATION, before the submission is accepted.
//!
//! A harvest PRODUCES an object. `CreateHarvestJob` "Creates a new harvest job to export
//! content from a MediaPackage v2 channel to an S3 bucket", `Destination$S3Destination` is
//! "The configuration for exporting harvested content to an S3 bucket", and that
//! configuration's `BucketName` is "The name of an S3 bucket within which harvested content
//! will be exported"
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackagev2#S3DestinationConfig",model_path="services/service-com-amazonaws-mediapackagev2-mediapackagev2/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json")`).
//! A submission whose destination could never take that object therefore asks for work the
//! service could not carry out, so it is refused — `ValidationException` (400) with the
//! modelled `HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION` discriminator — before anything
//! is stored, so no later read reports a job whose export was impossible from the moment it
//! was submitted.
//!
//! ## Where the destination is resolved
//!
//! In MEMORY, against Amazon S3's own store in the SAME shared engine this service's store
//! lives in ([`s3_state`]), reached through the object store's own accessor. No socket, no
//! outbound request, no host resolution and no caller-named endpoint: the bucket this reads
//! is the record a client's own `CreateBucket` wrote, and a value from the request is only
//! ever a key into that store. The sibling MediaPackage v1 crate resolves a harvest job's
//! destination role against the IAM store through exactly this seam.
//!
//! Two things make a destination one the harvest could not be written to, and both are read
//! off that store: the object store holds no bucket of that name at all, or it holds one in
//! a DIFFERENT Region from the one the submission is served in — the harvest and its
//! destination have to be in the same Region, which is what the refusal's own sentence tells
//! the caller.
//!
//! Nothing here WRITES: a pre-flight leaves the destination, and the object store, exactly
//! as the caller left them.

use super::*;

/// The member path a refusal about the harvest destination is keyed by, in the form this
/// service's validation messages name members at ([`super::validation_message`]) — the name
/// MediaPackageV2 words this error class at.
pub const HARVEST_DESTINATION_MEMBER: &str = "s3DestinationConfig";

/// The `ValidationExceptionType` the model declares for a destination the harvest could not
/// be written to (the `ValidationExceptionType` enum's
/// `HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION` value).
pub const VALIDATION_HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION: &str =
    "HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION";

/// The principal MediaPackageV2 writes a harvest to the destination as: this service's own
/// service principal, built from its signing name so the sentence below and the service's
/// identity cannot disagree.
pub fn service_principal() -> String {
    format!("{SIGNING_NAME}.amazonaws.com")
}

/// The requirement a destination the harvest could not be written to has failed: fixed text
/// for the error class, naming what the caller has to put right — the write access the
/// bucket has to admit for this service's principal, and the Region it has to be in.
pub fn writable_destination_requirement() -> String {
    format!(
        "Unable to write to S3 destination bucket. Verify MediaPackageV2 service principal \
         {} is allow listed write access to the bucket and the bucket region is in the same \
         region.",
        service_principal()
    )
}

/// The refusal a submission whose destination the harvest could not be written to is
/// answered with, in the envelope every validation failure of this service uses.
pub fn unwritable_destination() -> ValidationException {
    validation_failed(
        HARVEST_DESTINATION_MEMBER,
        &writable_destination_requirement(),
        VALIDATION_HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION,
    )
}

/// Amazon S3's store in the SAME shared engine, which is where the account's buckets live.
///
/// This is the object store's OWN accessor, called rather than copied, so what this service
/// resolves a destination against is the very record `CreateBucket` wrote. `None` has the
/// same meaning as for [`super::service_state`]: the engine carries no such store.
pub fn s3_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_com_amazonaws_s3_amazons3::ComAmazonawsS3Amazons3State>>
{
    ::service_com_amazonaws_s3_amazons3::state::service_state(engine)
}

/// `None` when nothing establishes that the harvest could not be written to `bucket` in
/// `region`; otherwise the refusal that answers the submission.
///
/// The object store's guard is taken and released HERE, before this service's own guard is
/// taken by the caller, so no two orders of the same two locks exist. It READS: a pre-flight
/// leaves the destination exactly as the caller left it.
///
/// An engine carrying no object store establishes nothing about the destination, so it
/// refuses nothing: a refusal has to be grounded in what the store holds rather than
/// assumed.
pub fn destination_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    bucket: &str,
) -> Option<ValidationException> {
    let store = s3_state(engine)?;
    let held_in = store.lock().bucket(bucket).map(|held| held.region.clone());
    match held_in {
        // The object store holds the bucket, in the Region this harvest is being submitted
        // in: the destination is one the export could be written to as far as this emulator
        // can establish.
        Some(held) if held == region => None,
        // No such bucket, or one of another Region.
        _ => Some(unwritable_destination()),
    }
}
