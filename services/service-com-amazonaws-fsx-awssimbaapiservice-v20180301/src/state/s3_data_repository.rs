// SPDX-License-Identifier: CC0-1.0

//! Resolving the Amazon S3 data repository an association's path names, against the object
//! store that holds it.
//!
//! `CreateDataRepositoryAssociation` "creates a link between a directory on the file system
//! and an Amazon S3 bucket or prefix", and `DataRepositoryPath` is "the path to the Amazon S3
//! data repository that will be linked to the file system ... in the format
//! `s3://bucket-name/prefix/`". The bucket is a resource of ANOTHER service, which this
//! service therefore resolves where that service holds it rather than assuming: Amazon FSx
//! validates its access to the bucket before it will accept the association, so a path naming
//! a bucket that does not exist is refused and nothing is stored.
//!
//! S3 is reached the way the framework documents a handler reaching another emulated service
//! ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client over the
//! in-process callback's HTTP client and credentials, so nothing here opens a socket or leaves
//! the host. The question is put with `HeadBucket`, the operation S3 documents for it ("You
//! can use this operation to determine if a bucket exists"). The one caller-supplied value
//! that reaches the client's AUTHORITY is the signing Region, narrowed first
//! ([`service_common::arn::region_label`]), so no request can re-point this client at a host
//! of its own choosing.

/// What a `DataRepositoryPath` naming an S3 bucket or prefix is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3DataRepositoryPath {
    /// The bucket the path names, between `s3://` and the first following separator.
    pub bucket: String,
    /// The prefix under it, which the documented format makes optional.
    pub prefix: String,
}

/// `path` read as the `s3://bucket-name/prefix/` form
/// `CreateDataRepositoryAssociation$DataRepositoryPath` documents, or `None` when it is not
/// that form at all: the `s3://` scheme followed by a non-empty bucket segment.
///
/// Whether S3 can hold a bucket under that segment is not decided here — that is part of
/// whether it HOLDS one, which [`bucket_is_absent`] answers.
pub fn parse_s3_data_repository_path(path: &str) -> Option<S3DataRepositoryPath> {
    let rest = path.strip_prefix(super::S3_DATA_REPOSITORY_SCHEME)?;
    let (bucket, prefix) = match rest.split_once('/') {
        Some((bucket, prefix)) => (bucket, prefix),
        None => (rest, ""),
    };
    (!bucket.is_empty()).then(|| S3DataRepositoryPath {
        bucket: bucket.to_string(),
        prefix: prefix.to_string(),
    })
}

/// Whether Amazon S3 reports that it holds no bucket under `bucket`.
///
/// `false` means the association may be created. That INCLUDES every answer that is not S3
/// reporting the bucket absent — an emulator assembled with no object store to ask, or any
/// other failure to ask it: nothing is then established about the bucket, and a refusal has to
/// be grounded in what S3 answered rather than assumed. (This service GATES on the bucket
/// existing and keeps only the path the request named, so silence refuses nothing; the
/// reading that refuses on silence belongs to a resolution that keeps something OF the other
/// service's record — `services/common/src/network.rs` states which is which.)
pub async fn bucket_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> bool {
    // A name outside the published general-purpose bucket naming rules
    // ([`service_common::s3_bucket_name::is_well_formed`], the one place this tree states
    // them) names no bucket in any object store, so there is nothing to ask about: S3 holds
    // none under it, whatever it answers.
    if !::service_common::s3_bucket_name::is_well_formed(bucket) {
        return true;
    }
    // Which Region the bucket is looked up in decides nothing here — the emulator serves one
    // object store over the in-process callback — so a scope that carries no Region label
    // falls back to the default rather than putting caller text in the authority.
    let region =
        ::service_common::arn::region_label(region).unwrap_or(crate::state::DEFAULT_REGION);
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket travels in the
        // path rather than in a per-bucket host the router could not resolve.
        .force_path_style(true)
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials the callback
        // hands over are used directly, and they only sign this in-process call.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    let answer = ::aws_sdk_s3::Client::from_conf(config)
        .head_bucket()
        .bucket(bucket)
        .send()
        .await;
    match answer {
        // S3 holds the bucket.
        Ok(_) => false,
        // S3 says it holds no such bucket: the refusal is grounded in its answer.
        Err(error) if reported_absent(&error) => true,
        // Any other answer is this emulator failing to ask, not S3 reporting a missing
        // bucket, so it refuses nothing — and it is worth a warning, because a deployment with
        // no object store to ask is one where this resolution does not run. The caller's own
        // value is rendered in its escaped `Debug` form, so a bucket name carrying a newline
        // cannot forge a log line.
        Err(error) => {
            ::tracing::warn!(
                bucket = ?bucket,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a data repository association's S3 bucket could not be resolved against Amazon S3"
            );
            false
        }
    }
}

/// Whether an S3 `HeadBucket` failure is S3 reporting that the bucket does not exist
/// (`NotFound`, the only error that operation models), as opposed to the call not reaching S3
/// at all.
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

/// The `BadRequest$Message` FSx refuses an association whose S3 bucket it cannot validate
/// access to with.
///
/// FIXED text for this error class, with no variable part: the sentence states that the
/// validation failed and why, and names neither the path nor the bucket.
pub const UNVALIDATED_S3_BUCKET_MESSAGE: &str =
    "Amazon FSx is unable to validate the access to the S3 bucket prefix. S3 bucket does not \
     exist.";

/// The `BadRequest$Message` for a `DataRepositoryPath` that is not an S3 bucket or prefix at
/// all, built from the format the member documents.
pub const MALFORMED_S3_DATA_REPOSITORY_PATH_MESSAGE: &str =
    "Invalid data repository path. The path must be an S3 bucket or prefix in the format \
     s3://bucket-name/prefix/.";
