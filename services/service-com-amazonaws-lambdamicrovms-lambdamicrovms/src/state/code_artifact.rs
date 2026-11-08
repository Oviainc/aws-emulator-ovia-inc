// SPDX-License-Identifier: CC0-1.0

//! Where a MicroVM image version's code comes from: the object Amazon S3 holds at the
//! address `codeArtifact$uri` named, which is what says whether the version's build can
//! produce an image at all.
//!
//! `CodeArtifact$uri` is "The URI of the code artifact in Amazon S3", and
//! `CreateMicrovmImage` documents the build it starts as asynchronous — "the image
//! transitions from CREATING to CREATED on success, or CREATE_FAILED on failure". A
//! build that cannot FETCH its code artifact produces nothing, so resolving that address
//! is what decides the branch; this emulator holds an Amazon S3 of its own, reached the
//! way any emulated service reaches another, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which routes IN PROCESS: no
//! socket, no outbound request, no host resolution. An object a caller staged with
//! `PutObject` is therefore the artifact a build fetches.
//!
//! ## What grounds a settled failure
//!
//! The two ways a resolution produces no artifact are kept apart, because they are
//! different claims:
//!
//! * Amazon S3 ANSWERED that it serves no such bucket or object, or that the read is not
//!   permitted — a fact about the `uri` the version carries, so the build settles on the
//!   FAILURE branch with the reason for that answer
//!   ([`CodeArtifactVerdict::Unfetchable`]);
//! * nothing answered the read at all — this emulator was assembled with no object store
//!   to ask — so NOTHING about the artifact is established
//!   ([`CodeArtifactVerdict::Unestablished`]), and a failure that was never established
//!   is not reported as one.
//!
//! The answer is taken when the build is SCHEDULED and settled on the record when the
//! clock makes the build due
//! ([`super::ComAmazonawsLambdamicrovmsLambdamicrovmsState::materialize_due`]), so a
//! caller reading straight after the create still observes the build in flight and no
//! handler sleeps. The object's BYTES are never read into this process: what the build
//! establishes here is whether the artifact is served, and the guest that would run
//! those bytes is not something this emulator runs (see `super::microvm_endpoint`).

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// The `stateReason` a build settles with when Amazon S3 answers that it holds no such
/// BUCKET: the whole fixed sentence AWS words this failure class as, which has no
/// variable parts.
pub const BUCKET_ABSENT_REASON: &str = "The S3 bucket does not exist.";

/// The `stateReason` a build settles with when the bucket is there and the OBJECT is
/// not — the same sentence as [`BUCKET_ABSENT_REASON`] one level down, for the other
/// half of an object address.
pub const OBJECT_ABSENT_REASON: &str = "The S3 object does not exist.";

/// The `stateReason` a build settles with when Amazon S3 answers that the read of the
/// artifact is not permitted: the artifact may well be there, and what the build
/// establishes is that it may not fetch it.
pub const ACCESS_DENIED_REASON: &str = "Access to the S3 object was denied.";

/// What a resolution of a version's code artifact established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeArtifactVerdict {
    /// The object store served the artifact, so the build has code to build.
    Fetched,
    /// The store ANSWERED that the artifact cannot be fetched; the clause is the reason
    /// the version and every build of it settle with.
    Unfetchable(&'static str),
    /// Nothing established whether the artifact is there: no object store answered.
    Unestablished,
}

impl CodeArtifactVerdict {
    /// The `stateReason` the build this verdict was taken for settles with, and `None`
    /// for a build that settles successfully.
    ///
    /// An [`Self::Unestablished`] resolution reports no failure: a build is refuted only
    /// by an answer from the store that owns the artifact, never by this emulator failing
    /// to ask it.
    pub fn failure_reason(&self) -> Option<String> {
        match self {
            Self::Unfetchable(reason) => Some((*reason).to_string()),
            Self::Fetched | Self::Unestablished => None,
        }
    }
}

/// The Amazon S3 error codes in which the store ANSWERS that it does not serve the
/// artifact a version named, each mapped to the reason the build settles with. Every
/// other answer is this emulator failing to ask its own object store, which establishes
/// nothing about the artifact.
const UNFETCHABLE_ARTIFACT_CODES: &[(&str, &str)] = &[
    ("NoSuchBucket", BUCKET_ABSENT_REASON),
    ("NoSuchKey", OBJECT_ABSENT_REASON),
    ("NoSuchVersion", OBJECT_ABSENT_REASON),
    ("NotFound", OBJECT_ABSENT_REASON),
    ("AccessDenied", ACCESS_DENIED_REASON),
    ("AllAccessDisabled", ACCESS_DENIED_REASON),
];

/// What the build of a version whose `codeArtifact$uri` is `uri` can establish about its
/// artifact, read through the emulator's own Amazon S3.
///
/// A `uri` that is not an S3 object URI addresses no object at all, so nothing is asked
/// of the store: such a request is refused on its own shape
/// ([`super::CODE_ARTIFACT_URI_REQUIREMENT`]) before a build is ever scheduled.
pub async fn resolve(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    uri: &str,
) -> CodeArtifactVerdict {
    let Some((bucket, key)) = super::s3_object_uri(uri) else {
        return CodeArtifactVerdict::Unestablished;
    };
    let region = super::request_region(ctx);
    artifact_verdict(&s3_client(ctx, &region), bucket, key).await
}

/// Ask `client` whether it serves the object `key` of `bucket`.
///
/// The read is `GetObject`, the operation a build fetches its artifact with, because its
/// refusals are what tell the two halves of an object address apart: a HEAD of an absent
/// object carries no body at all, so a 404 to one says nothing about WHICH of the bucket
/// and the key is missing. The answer's body is dropped unread — the bytes are the
/// guest's, not this emulator's.
async fn artifact_verdict(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> CodeArtifactVerdict {
    let error = match client.get_object().bucket(bucket).key(key).send().await {
        Ok(_) => return CodeArtifactVerdict::Fetched,
        Err(error) => error,
    };
    let code = error.code().unwrap_or_default();
    if let Some((_, reason)) = UNFETCHABLE_ARTIFACT_CODES
        .iter()
        .find(|(answered, _)| *answered == code)
    {
        // The store itself says it serves no such artifact, or that this read is not
        // permitted: the settled failure is grounded in its answer.
        return CodeArtifactVerdict::Unfetchable(reason);
    }
    // Any other answer is this emulator failing to ask its own object store rather than
    // the store reporting an unfetchable artifact — and it is worth a warning, because a
    // deployment with no Amazon S3 mounted to ask is one where no build's artifact is
    // ever established. The caller's own values are rendered in their escaped `Debug`
    // form, so a bucket or key carrying a newline cannot forge a log line.
    ::tracing::warn!(
        bucket = ?bucket,
        key = ?key,
        code = ?error.code(),
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "a MicroVM image code artifact could not be resolved against the emulator's Amazon S3"
    );
    CodeArtifactVerdict::Unestablished
}

/// The Amazon S3 client a code artifact is resolved with, addressed to `region`.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url` below
/// is an ADDRESS the in-process router reads rather than a host any resolver looks up:
/// this client reaches the emulator's own Amazon S3 and nothing else, whatever bucket or
/// key a caller names. The one caller-supplied value that reaches its authority is the
/// signing region, which [`super::request_region`] takes from the request's own
/// credential scope and otherwise answers the default region with.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which
/// would need a sleep implementation this client is not given. The same goes for caching
/// the identity: the credentials the callback hands over are used directly, and they
/// only sign this operation's in-process calls.
fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: only an answer from the object store settles a build on the failure branch,
    /// and the reason it settles with is the reason for that answer.
    #[test]
    fn only_an_answered_refusal_is_a_failure_reason() {
        let rows: &[(&str, CodeArtifactVerdict, Option<&str>)] = &[
            ("the artifact is served", CodeArtifactVerdict::Fetched, None),
            (
                "no such bucket",
                CodeArtifactVerdict::Unfetchable(BUCKET_ABSENT_REASON),
                Some(BUCKET_ABSENT_REASON),
            ),
            (
                "no such object",
                CodeArtifactVerdict::Unfetchable(OBJECT_ABSENT_REASON),
                Some(OBJECT_ABSENT_REASON),
            ),
            (
                "the read is not permitted",
                CodeArtifactVerdict::Unfetchable(ACCESS_DENIED_REASON),
                Some(ACCESS_DENIED_REASON),
            ),
            (
                "nothing answered the read",
                CodeArtifactVerdict::Unestablished,
                None,
            ),
        ];
        for (case, verdict, expected) in rows {
            let actual = verdict.failure_reason();
            let expected = expected.map(::std::string::ToString::to_string);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
