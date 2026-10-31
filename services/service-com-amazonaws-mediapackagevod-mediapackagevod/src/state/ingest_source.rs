// SPDX-License-Identifier: CC0-1.0

//! Whether the SOURCE an ingest was submitted with is one this service could read — the
//! one fact the terminal packaging status of an asset's egress endpoints turns on.
//!
//! `CreateAssetRequest$SourceArn` is "ARN of the source object in S3" and
//! `SourceRoleArn` "The IAM role ARN used to access the source S3 bucket", so ingest
//! READS that object AS that role: a packaging job can only end in a playable endpoint
//! where both halves of that sentence hold. Reading the ARN FORM of the two members is
//! what [`crate::state::check_arn_members`] does and is a property of the request; this
//! module asks what the two values NAME, which is a property of the emulator's state:
//!
//! * the role, of the account's IAM records
//!   ([`::service_common::iam_role::role_is_unassumable`] — the shared reading every
//!   service that acts as a caller-supplied role asks its question through);
//! * the object, of the emulator's own Amazon S3, reached the way any emulated service
//!   reaches another: the AWS SDK driven over the operation callback
//!   ([`smithy_server_runtime::OperationContext::callback`]), which routes IN PROCESS —
//!   no socket, no outbound request, no host resolution. An object a caller staged with
//!   `PutObject` is therefore the object this ingest reads.
//!
//! WHAT GROUNDS A FAILURE. An ingest is reported as failed only where one of those two
//! ANSWERED that the source cannot be read — IAM that the account holds no such role or
//! that its trust policy withholds the assume, Amazon S3 that it serves no such bucket,
//! key or version, or that the read is not permitted ([`UNREADABLE_OBJECT_CODES`]).
//! Anything else is this emulator failing to ASK (an assembly with no object store or no
//! IAM to ask, an answer it could not read), which establishes nothing about the source
//! and so refutes nothing: the ingest keeps the outcome it would have had before the
//! question could be put at all.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// The service principal MediaPackage assumes a CALLER-SUPPLIED role as — here the
/// `SourceRoleArn` an asset is ingested with, which the service reads the source object
/// as.
///
/// The same principal the MediaPackage control plane names for every role a caller hands
/// it (`service-com-amazonaws-mediapackage-mediapackage`'s `SERVICE_PRINCIPAL`), because
/// it is the same service: this model is the VOD half of its API surface.
pub const SERVICE_PRINCIPAL: &str = "mediapackage.amazonaws.com";

/// What was established about an asset's source when its ingest was submitted.
///
/// Two states rather than three: an answer that establishes nothing cannot refute the
/// read, so it reads as [`Self::Readable`] — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceReadability {
    /// Nothing this emulator asked refuted that the source can be read.
    Readable,
    /// IAM or Amazon S3 ANSWERED that the source cannot be read: no packaging job could
    /// have read it, so every egress endpoint of the asset ends in the terminal failure
    /// status instead of the playable one.
    Unreadable,
}

/// The Amazon S3 error codes in which the store ANSWERS that it does not serve the
/// object a read named: the three in which it reports that an address holds nothing, the
/// status-only code a `HEAD` of an absent object carries (a HEAD response has no body to
/// name a code in), and the two in which it reports that the read is not permitted. Every
/// other answer is this emulator failing to ask its own object store, which establishes
/// nothing about the object.
const UNREADABLE_OBJECT_CODES: &[&str] = &[
    "NoSuchBucket",
    "NoSuchKey",
    "NoSuchVersion",
    "NotFound",
    "AccessDenied",
    "AllAccessDisabled",
];

/// What the ingest of an asset whose source is `source_arn`, read as `source_role_arn`,
/// established about that source.
///
/// Asked ONCE, by the submit that starts the packaging job ([`crate::ops::create_asset`]),
/// and kept on the asset's record: the job reads the source at the instant it runs, so a
/// later read of the asset reports the outcome of THAT read rather than asking again.
pub async fn source_readability(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source_arn: &str,
    source_role_arn: &str,
) -> SourceReadability {
    // The Region ends up in the AUTHORITY of the endpoint URL each client below is
    // addressed by, so it is held to the Region-label grammar first (the shared reading
    // every service that puts a caller-supplied Region in an authority uses); a value
    // outside it addresses this service's default Region instead of a host the caller
    // chose. The transport is the in-process callback either way.
    let addressed =
        ::service_common::arn::region_label(region).unwrap_or(crate::state::DEFAULT_REGION);
    // The role comes first: the object is read AS the role, so a role this service
    // cannot assume is a source it cannot reach whatever the object store holds.
    let unassumable = ::service_common::iam_role::role_is_unassumable(
        ctx,
        addressed,
        source_role_arn,
        SERVICE_PRINCIPAL,
    )
    .await;
    if unassumable {
        // The caller's own value is rendered in its escaped `Debug` form, so a value
        // carrying a newline cannot forge a log line.
        ::tracing::debug!(
            role = ?source_role_arn,
            "an asset's source role is one this service cannot assume, so its ingest fails"
        );
        return SourceReadability::Unreadable;
    }
    if object_is_unreadable(ctx, addressed, source_arn).await {
        return SourceReadability::Unreadable;
    }
    SourceReadability::Readable
}

/// Whether Amazon S3 ANSWERED that it does not serve the object `source_arn` names.
///
/// `false` for everything that is not such an answer, including an assembly with no
/// object store to ask: nothing about the object is established then, so nothing is
/// refuted.
async fn object_is_unreadable(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source_arn: &str,
) -> bool {
    let Some((bucket, key)) = object_address(source_arn) else {
        // `SourceArn` is "ARN of the source object in S3": a value whose resource field
        // names a bucket and no key within it names no object at all, which the form of
        // the value settles on its own — there is nothing to ask the store about.
        return true;
    };
    let answer = s3_client(ctx, region)
        .head_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await;
    match answer {
        // The store serves the object: the ingest read its source.
        Ok(_) => false,
        Err(error) => {
            let code = error.code().unwrap_or_default();
            if UNREADABLE_OBJECT_CODES.contains(&code) {
                ::tracing::debug!(
                    bucket = ?bucket,
                    key = ?key,
                    code = ?code,
                    "an asset's source object is one Amazon S3 does not serve, so its ingest \
                     fails"
                );
                return true;
            }
            ::tracing::debug!(
                bucket = ?bucket,
                key = ?key,
                code = ?error.code(),
                "nothing answered the read of an asset's source object, so its ingest is left \
                 as it was"
            );
            false
        }
    }
}

/// The bucket and the key within it that an S3 OBJECT ARN names
/// (`arn:aws:s3:::<bucket>/<key>`), or `None` for a value that names no object.
///
/// The six ARN fields come from the one shared ARN parse
/// ([`::service_common::arn::parse_of`]), which also settles that the value is an ARN of
/// the `s3` vendor at all; what is read here is the resource field, whose first segment is
/// the bucket and whose remainder is the key — a key may itself carry `/`, so only the
/// FIRST separator divides them.
fn object_address(source_arn: &str) -> Option<(&str, &str)> {
    let arn = ::service_common::arn::parse_of(source_arn, "s3")?;
    let (bucket, key) = arn.resource.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then_some((bucket, key))
}

/// The Amazon S3 client this service reads an asset's source object with, addressed to
/// `region`.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url` below is
/// an ADDRESS the in-process router reads rather than a host any resolver looks up: this
/// client reaches the emulator's own Amazon S3 and nothing else, whatever bucket or key a
/// caller names. The one caller-supplied value that reaches its authority is the signing
/// region, and it is narrowed before it gets here — [`source_readability`] holds it to the
/// Region-label grammar and otherwise addresses this service's default Region.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which would
/// need a sleep implementation this client is not given. The same goes for caching the
/// identity: the credentials the callback hands over are used directly, and they only sign
/// this operation's in-process calls.
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
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::object_address;

    /// One row of the table below: the case, the value read, and the `(bucket, key)` pair
    /// the reading must report. Named so the table's type stays legible.
    type Row = (&'static str, &'static str, Option<(&'static str, &'static str)>);

    /// The bucket and key an S3 object ARN names, over the forms the member admits — and
    /// nothing from a value that names no object within a bucket.
    #[test]
    fn an_s3_object_arn_names_the_bucket_and_key_the_ingest_reads() {
        // (case, value, the (bucket, key) pair the reading must report)
        let rows: Vec<Row> = vec![
            (
                "a bare key",
                "arn:aws:s3:::vod-sources/movie.smil",
                Some(("vod-sources", "movie.smil")),
            ),
            (
                // A key may carry `/`: only the first separator divides it from the bucket.
                "a key under a prefix",
                "arn:aws:s3:::vod-sources/2026/01/movie.smil",
                Some(("vod-sources", "2026/01/movie.smil")),
            ),
            (
                "another partition",
                "arn:aws-cn:s3:::vod-sources/movie.smil",
                Some(("vod-sources", "movie.smil")),
            ),
            ("a bucket and no key", "arn:aws:s3:::vod-sources", None),
            ("an empty key", "arn:aws:s3:::vod-sources/", None),
            ("an empty bucket", "arn:aws:s3::://movie.smil", None),
            (
                "another vendor's resource",
                "arn:aws:iam::000000000000:role/vod-ingest",
                None,
            ),
            ("not an ARN at all", "vod-sources/movie.smil", None),
            ("the empty string", "", None),
        ];

        for (case, value, expected) in rows {
            let read = object_address(value);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
