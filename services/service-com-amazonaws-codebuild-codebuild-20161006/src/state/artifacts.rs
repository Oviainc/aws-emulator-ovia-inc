// SPDX-License-Identifier: CC0-1.0

//! Where a build's output artifacts go: the rules a [`ProjectArtifacts`] has to satisfy
//! before a build that would upload to it is started.
//!
//! `ProjectArtifacts$type` is the only member the model makes `@required`, and what the
//! rest of the shape MEANS depends on it: "If `type` is set to `S3`, this is the name of
//! the output bucket" (`ProjectArtifacts$location`), while for `CODEPIPELINE` and
//! `NO_ARTIFACTS` the member "is ignored if specified". So the consistency between the
//! type and the destination is the service's rule rather than a modelled constraint, and
//! it is stated once here for every operation that SETS a project's or a build's
//! artifacts.
//!
//! The two halves of the rule are asked at different points, and deliberately:
//!
//! * whether artifacts bound for a bucket NAME one is a question about the request alone
//!   ([`artifacts_location_violation`]), so it is asked wherever artifacts are set —
//!   when a project stores them, and when a start overrides them;
//! * whether that bucket EXISTS is a question for the service that owns buckets
//!   ([`artifacts_bucket_is_absent`]), and it is asked of the destination a START
//!   introduces. A project's own artifacts are not re-resolved at every start: the
//!   bucket a stored project names may be created at any time between the project's
//!   create and a build of it, so re-asking would refuse builds that are AWS's to
//!   accept, and nothing recorded shows CodeBuild resolving a project's stored
//!   destination again per build. The override is the destination the caller hands over
//!   at start time, which is the one that has not been seen before.

use super::ProjectArtifacts;

/// The `ArtifactsType` whose artifacts are uploaded to a bucket: "S3: The build project
/// stores build output in Amazon S3" (`ProjectArtifacts$type`). The other two values
/// name no destination of their own — `CODEPIPELINE` lets CodePipeline manage the
/// location, and `NO_ARTIFACTS` produces no output at all.
pub const ARTIFACTS_TYPE_S3: &str = "S3";

/// What CodeBuild states as the reason artifacts bound for a bucket name none, carried
/// by the modelled `InvalidInputException`.
///
/// Wholly fixed text — recorded behaviour, as the model states no message for the error:
/// it names the member at fault and quotes nothing of the request, so every such refusal
/// reads the same and a client can key on it.
pub const ARTIFACTS_LOCATION_REQUIRED_MESSAGE: &str = "Invalid artifacts: location is required";

/// Why `artifacts` cannot be a build's output artifacts, as the message the modelled
/// `InvalidInputException` carries — or `None` when they can.
///
/// One rule: artifacts of the type that stores build output in a bucket have to say
/// WHICH bucket, because `location` is "the name of the output bucket" for that type and
/// there is no default. A location of nothing but whitespace names no bucket either, the
/// way [`super::inline_buildspec_violation`] reads an all-whitespace buildspec as
/// absent. For every other type the member is documented as ignored, so its absence
/// constrains nothing.
pub fn artifacts_location_violation(artifacts: &ProjectArtifacts) -> Option<&'static str> {
    (artifacts.r#type == ARTIFACTS_TYPE_S3 && artifacts_bucket_of(artifacts).is_none())
        .then_some(ARTIFACTS_LOCATION_REQUIRED_MESSAGE)
}

/// The bucket the artifacts `artifacts` describes are uploaded to, or `None` when they
/// name none — because they are of a type that uploads nothing, or because the location
/// is absent or blank.
///
/// `location` is "the name of the output bucket", and the rest of the object key comes
/// from the separate `path`, `namespaceType` and `name` members; a caller that
/// nevertheless writes a prefix into the location is naming the bucket before the first
/// `/`, so that is where the bucket name ends.
pub fn artifacts_bucket_of(artifacts: &ProjectArtifacts) -> Option<&str> {
    if artifacts.r#type != ARTIFACTS_TYPE_S3 {
        return None;
    }
    let location = artifacts.location.as_deref()?.trim();
    let bucket = location.split('/').next().unwrap_or_default();
    (!bucket.is_empty()).then_some(bucket)
}

/// What CodeBuild states as the reason the bucket a build's artifacts name does not
/// exist, carried by the modelled `InvalidInputException`.
///
/// The fixed text is the sentence around the bucket; the bucket itself is the name the
/// REQUEST gave, which is the one thing a caller needs to fix the request.
pub fn artifacts_bucket_not_found_message(bucket: &str) -> String {
    format!("Bucket {bucket} does not exist")
}

/// Whether S3 reports that `bucket` does not exist.
///
/// A build uploads its output artifacts to the bucket its artifacts name, so a build
/// started against a bucket that does not exist could never upload anything: the
/// destination is resolved against the service that OWNS buckets before the build is
/// minted. S3 is reached the way the framework documents a handler reaching another
/// emulated service ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK
/// client over the in-process callback's HTTP client and credentials provider, exactly
/// as this crate resolves a service role against IAM. The question is put with
/// `HeadBucket`, the operation S3 documents for it ("You can use this operation to
/// determine if a bucket exists"), so this crate names no other service crate and
/// signing, addressing and the typed `NotFound` all come from the SDK.
///
/// `false` means the build may start. That INCLUDES every answer that is not S3
/// reporting the bucket absent — a router with no S3 mounted, or any other failure to
/// ask: nothing is then known about the bucket, and a refusal has to be grounded in what
/// S3 answered rather than assumed.
pub async fn artifacts_bucket_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> bool {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket travels in
        // the path rather than in a per-bucket host the router could not resolve.
        .force_path_style(true)
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given. The same goes for caching the identity: the credentials
        // are handed over directly.
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
        // S3 says this account holds no such bucket: the refusal is grounded.
        Err(error) if bucket_reported_absent(&error) => true,
        // Any other answer is this emulator failing to ask, not S3 reporting a missing
        // bucket, so it refuses nothing — and it is worth a warning, because a
        // deployment with no S3 to ask is one where this gate does not run.
        Err(error) => {
            ::tracing::warn!(
                %bucket,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the build's artifacts bucket could not be resolved against S3"
            );
            false
        }
    }
}

/// Whether an S3 `HeadBucket` failure is S3 reporting that the bucket does not exist
/// (`NotFound`, the only error that operation models), as opposed to the call not
/// reaching S3 at all.
fn bucket_reported_absent(
    error: &::aws_sdk_s3::error::SdkError<
        ::aws_sdk_s3::operation::head_bucket::HeadBucketError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found)
}
