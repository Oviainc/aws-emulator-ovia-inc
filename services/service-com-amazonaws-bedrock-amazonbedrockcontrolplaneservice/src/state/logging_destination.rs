// SPDX-License-Identifier: CC0-1.0

//! Whether the Amazon S3 destinations a model invocation logging configuration names are
//! destinations Amazon Bedrock would deliver into.
//!
//! A logging destination is a bucket Amazon Bedrock itself writes the invocation records
//! into, and it writes them from the Region the configuration was installed in: delivery is
//! regional, so a bucket that lives in another Region is not a destination this
//! configuration could ever be served from, whatever policy that bucket carries. A
//! configuration naming one is therefore refused as input rather than installed — the
//! modelled `ValidationException` (`@httpError(400)`) — because installing it would leave
//! an operator believing an audit trail exists where no record is ever delivered.
//!
//! Both destinations a configuration can carry are judged, since both are buckets Amazon
//! Bedrock delivers into: `loggingConfig.s3Config`, the object destination, and
//! `cloudWatchConfig.largeDataDeliveryS3Config`, the bucket the CloudWatch Logs destination
//! spills a large record into — the two `S3Config` members the modelled `LoggingConfig` and
//! `CloudWatchConfig` shapes of this service declare.
//!
//! WHERE a bucket lives is a question the object store ANSWERS, asked over the in-process
//! callback every cross-service call of an emulated service uses
//! ([`super::model_invocation_run::s3_client`] — no socket, no outbound request, no host
//! resolution; the caller's value reaches it only as a bucket name). The answer is
//! `GetBucketLocation`'s own, whose `locationConstraint` is the bucket's Region and is
//! absent for the one Region S3 reports no constraint for
//! (`GetBucketLocationOutput$LocationConstraint`: "Buckets in Region us-east-1 have a
//! LocationConstraint of null").
//!
//! Refused only on what the object store ANSWERED: a bucket it holds, in a Region that is
//! not the one this request was signed for. A bucket it says it does not hold, an emulator
//! assembled with no Amazon S3 to ask, and an answer that could not be read all leave the
//! configuration admitted, because a refusal has to be grounded — the same rule the rest of
//! this service's cross-service decisions follow ([`super::model_source`],
//! [`super::model_invocation_run`]).

use crate::ops::types::LoggingConfig;

use super::model_invocation_run::s3_client;

/// The Region a bucket lives in when `GetBucketLocation` reports no constraint for it:
/// "Buckets in Region us-east-1 have a LocationConstraint of null"
/// (`GetBucketLocationOutput$LocationConstraint`), which is why that Region is not one of
/// the values `BucketLocationConstraint` carries.
const UNCONSTRAINED_BUCKET_REGION: &str = "us-east-1";

/// What Amazon Bedrock answers for a logging destination whose bucket it could not validate
/// as one it may deliver into — here, a bucket that lives outside the Region the
/// configuration is being installed in.
///
/// The sentence names the BUCKET the request named and nothing else: the fault is about
/// that destination, so the bucket is the one variable part and it comes from the call being
/// served rather than from one recorded run.
#[must_use]
pub fn cross_region_bucket_message(bucket: &str) -> String {
    format!(
        "Failed to validate permissions for bucket: {bucket}. Verify the S3 bucket policy \
         is correct."
    )
}

/// Why the Amazon S3 destinations `config` names are not destinations this Region's model
/// invocation logging can deliver into, or `None` when every bucket it names is one it can
/// — which is the ordinary case, and the answer whenever nothing could be established.
///
/// The destinations are asked about in the order the configuration states them, so a
/// configuration wrong in more than one is refused on the first one READ. A destination the
/// object store established nothing about is SKIPPED rather than ending the check, so the
/// destination beside it is still judged.
pub async fn cross_region_logging_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    config: &LoggingConfig,
) -> Option<String> {
    let destinations = [
        config.s3_config.as_ref(),
        config
            .cloud_watch_config
            .as_ref()
            .and_then(|cloud_watch| cloud_watch.large_data_delivery_s3_config.as_ref()),
    ];
    for destination in destinations.into_iter().flatten() {
        let bucket = destination.bucket_name.as_str();
        let Some(bucket_region) = bucket_region(ctx, region, bucket).await else {
            continue;
        };
        if bucket_region != region {
            return Some(cross_region_bucket_message(bucket));
        }
    }
    None
}

/// The Region this emulator's own Amazon S3 says `bucket` lives in, or `None` when it
/// established none: it holds no such bucket, there was no object store to ask, or the
/// answer could not be read.
async fn bucket_region(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> Option<String> {
    match s3_client(ctx, region)
        .get_bucket_location()
        .bucket(bucket)
        .send()
        .await
    {
        Ok(answer) => Some(
            answer
                .location_constraint()
                .map(|constraint| constraint.as_str().to_string())
                .filter(|constraint| !constraint.is_empty())
                .unwrap_or_else(|| UNCONSTRAINED_BUCKET_REGION.to_string()),
        ),
        Err(error) => {
            // Not the object store reporting a bucket's Region, so it establishes nothing
            // about this destination — and it is worth a warning, because an emulator with
            // no object store mounted to ask is one where this pre-condition cannot be
            // decided. The caller's own value is rendered in its escaped `Debug` form, so a
            // bucket name carrying a newline cannot forge a log line.
            ::tracing::warn!(
                bucket = ?bucket,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the logging destination's bucket could not be located in this emulator's \
                 Amazon S3"
            );
            None
        }
    }
}
