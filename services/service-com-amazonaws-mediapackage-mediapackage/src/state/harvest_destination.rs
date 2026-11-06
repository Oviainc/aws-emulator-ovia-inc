// SPDX-License-Identifier: CC0-1.0

//! Pre-flighting a HarvestJob's DESTINATION, before the submission is accepted.
//!
//! A harvest PRODUCES an object: `S3Destination$BucketName` is "The name of an S3 bucket
//! within which harvested content will be exported" and `S3Destination$ManifestKey` "The key
//! in the specified S3 bucket where the harvested top-level manifest will be placed"
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#S3Destination",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
//! and [`super::harvest_export`] is what writes it. A submission whose destination could
//! never take that object therefore asks for work this service could not carry out: it is
//! refused with the modelled `UnprocessableEntityException` (422), keyed by the member the
//! caller has to fix, before anything is stored — so no later read reports a job whose
//! harvest was impossible from the moment it was submitted.
//!
//! Two things about the destination bucket are established here, in this order, which is
//! the order the caller has to fix them in:
//!
//! 1. the NAME has to be one Amazon S3 holds a bucket under at all. The published
//!    general-purpose bucket naming rules are the shared reading every service that binds a
//!    caller-supplied bucket name uses ([`::service_common::s3_bucket_name`]), and a value
//!    outside them names no bucket anywhere, so no provisioning could make such a
//!    submission servable. Decided from the request alone;
//! 2. the BUCKET has to be one the account holds, which is asked OF Amazon S3 rather than
//!    re-decided here: `HeadBucket` is the operation S3 documents for it ("You can use this
//!    operation to determine if a bucket exists"), put through the in-process callback
//!    ([`smithy_server_runtime::OperationContext::callback`]) exactly as the export's own
//!    write is. So the bucket resolved is the one a client's own `CreateBucket` created, and
//!    every per-bucket rule stays the object store's own.
//!
//! An answer that is not S3 reporting the bucket absent refuses nothing: an emulator
//! assembled with no object store to ask, or any other failure to ask it, establishes
//! nothing about the bucket, and a refusal has to be grounded in what S3 answered rather
//! than assumed.
//!
//! Nothing here WRITES to the destination. Whether the harvested manifest is actually
//! placed is decided when the clock makes the harvest due, by the export itself
//! ([`super::harvest_export`]), which is also what a destination that stops being writable
//! AFTER the submission reports: a job whose manifest the destination did not take reaches
//! `FAILED`.

use super::{member_invalid, ServiceError, SERVICE_PRINCIPAL};

/// The wire path of the destination bucket member, which a refusal about it is keyed by
/// (`S3Destination$BucketName` is `@jsonName("bucketName")` inside `s3Destination`).
pub const HARVEST_BUCKET_MEMBER: &str = "s3Destination.bucketName";

/// The requirement a value that is not an Amazon S3 bucket NAME has failed: the member's
/// whole accepted domain in one sentence, with the published bounds read off the shared
/// rule rather than restated, so the sentence and the rule it states cannot disagree.
pub fn bucket_name_requirement() -> String {
    let min = ::service_common::s3_bucket_name::MIN_BUCKET_NAME_CHARS;
    let max = ::service_common::s3_bucket_name::MAX_BUCKET_NAME_CHARS;
    format!(
        "must be an S3 bucket name of {min} to {max} characters of lowercase letters, \
         numbers, dots and hyphens"
    )
}

/// The requirement a destination MediaPackage could not write the harvest to has failed.
///
/// FIXED text for the error class, naming what the caller has to put right, with the
/// principal the bucket has to admit ([`SERVICE_PRINCIPAL`]) in it — the shape this
/// service's recorded refusal about a destination role takes
/// ([`super::assumable_role_requirement`]).
pub fn writable_bucket_requirement() -> String {
    format!(
        "must be an S3 bucket MediaPackage can write to. Please ensure that the bucket \
         exists and that {SERVICE_PRINCIPAL} is allow listed write access to it"
    )
}

/// Refuse a destination whose bucket NAME is not one Amazon S3 holds a bucket under.
///
/// Decided from the request alone, so it is the same answer in every assembly and needs no
/// store: it is judged with the submission's other members, before any other service is
/// reached.
pub fn check_bucket_name(bucket: &str) -> Result<(), ServiceError> {
    if ::service_common::s3_bucket_name::is_well_formed(bucket) {
        return Ok(());
    }
    Err(member_invalid(
        HARVEST_BUCKET_MEMBER,
        bucket_name_requirement(),
    ))
}

/// `None` when nothing establishes that the harvest could not be written to `bucket`;
/// otherwise the refusal that answers the submission.
///
/// The question is put to Amazon S3 with `HeadBucket` over the in-process callback, with no
/// store guard held — the object store's front door must not wait on this service's lock —
/// and it READS: a pre-flight leaves the destination exactly as the caller left it.
pub async fn bucket_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> Option<ServiceError> {
    // The Region ends up in the AUTHORITY of the endpoint URL the client is addressed by, so
    // it is held to the Region-label grammar first (the shared reading every service that
    // puts a stored Region in an authority uses); anything outside it addresses this
    // service's default Region instead. The transport is the in-process callback either way,
    // so this client reaches the emulator's own Amazon S3 and nothing else, whatever bucket
    // a submission named.
    let region = ::service_common::arn::region_label(region).unwrap_or(super::DEFAULT_REGION);
    let answer = super::harvest_export::s3_client(ctx, region)
        .head_bucket()
        .bucket(bucket)
        .send()
        .await;
    match answer {
        // S3 holds the bucket, so the destination is one the harvest can be written to as
        // far as this emulator can establish; whether the write then succeeds is the
        // export's own answer.
        Ok(_) => None,
        // S3 says it holds no such bucket: the refusal is grounded in its answer.
        Err(error) if reported_absent(&error) => Some(member_invalid(
            HARVEST_BUCKET_MEMBER,
            writable_bucket_requirement(),
        )),
        // Any other answer is this emulator failing to ask rather than S3 reporting a
        // missing bucket, so it refuses nothing. The destination's own value is rendered in
        // its escaped `Debug` form, so a bucket name carrying a newline cannot forge a log
        // line.
        Err(error) => {
            ::tracing::debug!(
                bucket = ?bucket,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a harvest job's destination bucket could not be resolved against Amazon S3, \
                 so the submission is not refused for it"
            );
            None
        }
    }
}

/// Whether an S3 `HeadBucket` failure is S3 reporting that the bucket does not exist
/// (`NotFound`, the only error that operation models), as opposed to the call not reaching
/// S3 at all.
fn reported_absent<R>(
    error: &::aws_sdk_s3::error::SdkError<::aws_sdk_s3::operation::head_bucket::HeadBucketError, R>,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found)
}
