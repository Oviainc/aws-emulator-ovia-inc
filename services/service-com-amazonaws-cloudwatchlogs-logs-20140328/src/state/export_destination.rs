// SPDX-License-Identifier: CC0-1.0

//! Resolving the Amazon S3 bucket an export task writes to, against the object store that
//! holds it.
//!
//! `CreateExportTask` "creates an export task so that you can efficiently export data from
//! a log group to an Amazon S3 bucket", and its `destination` is "the name of S3 bucket for
//! the exported log data" — a resource of ANOTHER service, which this service therefore
//! resolves where that service holds it rather than assuming. S3 is reached the way the
//! framework documents a handler reaching another emulated service
//! ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client over the
//! in-process callback's HTTP client and credentials, so nothing here opens a socket or
//! leaves the host. The question is put with `HeadBucket`, the operation S3 documents for
//! it ("You can use this operation to determine if a bucket exists").
//!
//! The one caller-supplied value that reaches the client's AUTHORITY is the signing Region,
//! and it is narrowed before it gets there ([`service_common::arn::region_label`]), so no
//! request can re-point this client at a host of its own choosing: it reaches the emulator's
//! own Amazon S3 and nothing else, whatever bucket a caller names. The client itself is the
//! shared callback client every call to another service of this emulator is made through
//! ([`crate::state::callback_client`]).

use super::callback_client::callback_client;

/// Whether Amazon S3 reports that it holds no bucket under `bucket`.
///
/// `false` means the export may be created. That INCLUDES every answer that is not S3
/// reporting the bucket absent — an emulator assembled with no object store to ask, or any
/// other failure to ask it: nothing is then established about the bucket, and a refusal has
/// to be grounded in what S3 answered rather than assumed.
pub async fn bucket_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> bool {
    if bucket.is_empty() {
        return false;
    }
    // Which Region the bucket is looked up in decides nothing here — the emulator serves one
    // object store over the in-process callback — so a scope that carries no Region label
    // falls back to the default rather than putting caller text in the authority.
    let region =
        ::service_common::arn::region_label(region).unwrap_or(crate::state::DEFAULT_REGION);
    // The emulator serves one endpoint for every service, so the bucket travels in the path
    // rather than in a per-bucket host the router could not resolve.
    let client = callback_client!(aws_sdk_s3, ctx.callback(), "s3", region, force_path_style(true));
    let answer = client.head_bucket().bucket(bucket).send().await;
    match answer {
        // S3 holds the bucket.
        Ok(_) => false,
        // S3 says it holds no such bucket: the refusal is grounded in its answer.
        Err(error) if reported_absent(&error) => true,
        // Any other answer is this emulator failing to ask, not S3 reporting a missing
        // bucket, so it refuses nothing — and it is worth a warning, because a deployment
        // with no object store to ask is one where this resolution does not run. The
        // caller's own value is rendered in its escaped `Debug` form, so a bucket name
        // carrying a newline cannot forge a log line.
        Err(error) => {
            ::tracing::warn!(
                bucket = ?bucket,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an export task's destination bucket could not be resolved against Amazon S3"
            );
            false
        }
    }
}

/// Whether an S3 `HeadBucket` failure is S3 reporting that the bucket does not exist
/// (`NotFound`, the only error that operation models), as opposed to the call not reaching
/// S3 at all.
fn reported_absent(
    error: &::aws_sdk_s3::error::SdkError<
        ::aws_sdk_s3::operation::head_bucket::HeadBucketError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found)
}
