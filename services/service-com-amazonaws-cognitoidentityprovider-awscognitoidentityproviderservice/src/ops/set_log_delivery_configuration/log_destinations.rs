// SPDX-License-Identifier: CC0-1.0

//! Resolving the destination one `LogConfigurationType` names, against the service that
//! holds it.
//!
//! Every destination this service's model gives a log configuration is the ARN of a
//! resource of ANOTHER service — "the Amazon Resource Name (arn) of a CloudWatch Logs log
//! group where your user pool sends logs", "the ARN of an Amazon Data Firehose stream
//! that's the destination for threat protection log export", "the ARN of an Amazon S3
//! bucket that's the destination for threat protection log export" — and a pool cannot
//! export logs to a resource that is not there. So the ARN is resolved where the service
//! that owns the resource holds it, BEFORE anything is stored, and a request naming a
//! destination that does not resolve is refused with [`INVALID_DESTINATION_MESSAGE`].
//!
//! Holding the resource is necessary, not sufficient: a destination resolves only when it
//! also satisfies what its OWN member documents of the resource. For a log group that is
//! "The log group must not be encrypted with Key Management Service and must be in the same
//! Amazon Web Services account as your user pool"
//! (`CloudWatchLogsConfigurationType.LogGroupArn`) — the account is read from the ARN, and
//! whether the group's data is encrypted is read from the owning service's own answer, so
//! neither is a judgement this module makes about state it does not hold. The other two
//! members document no such requirement of the resource, so existence is the whole question
//! for them.
//!
//! Each owning service is reached the way the framework documents a handler reaching
//! another emulated service ([`smithy_server_runtime::OperationContext::callback`]): an
//! AWS SDK client over the in-process callback's HTTP client and credentials, so nothing
//! here opens a socket or leaves the process. The question is put with that service's own
//! read, and only that service's ANSWER refuses anything: a read that reports no such
//! resource is what refuses the request, while every other outcome — an emulator assembled
//! without that service to ask, or any other failure to ask it — establishes nothing about
//! the resource and so refuses nothing (`services/common/src/network.rs` states which
//! resolution is which; this one GATES on the resource and keeps only the ARN the request
//! named, so silence refuses nothing).
//!
//! The one caller-supplied value that reaches a client's AUTHORITY is the Region, narrowed
//! to a Region label first ([`service_common::arn::region_label`], through
//! [`destination_region`]), so no request can re-point one of these clients at a host of
//! its own choosing.

#[allow(unused_imports)]
use super::super::types::*;

/// What Amazon Cognito answers for a `LogConfigurations` collection naming a destination
/// it cannot resolve.
///
/// FIXED text for this error class, with no variable part: the sentence names neither the
/// pool nor the destination ARN, so it is the same for every destination type and every
/// request.
pub const INVALID_DESTINATION_MESSAGE: &str = "Invalid destination in the input.";

/// The ARN vendor of CloudWatch Logs, and the resource type a log group's ARN names.
const LOGS_ARN_NAMESPACE: &str = "logs";
const LOG_GROUP_RESOURCE_TYPE: &str = "log-group";

/// The ARN vendor of Amazon Data Firehose, and the resource type a stream's ARN names.
const FIREHOSE_ARN_NAMESPACE: &str = "firehose";
const DELIVERY_STREAM_RESOURCE_TYPE: &str = "deliverystream";

/// The ARN vendor of Amazon S3, whose bucket ARNs name no resource type at all: the
/// resource field of `arn:<partition>:s3:::<bucket>` IS the bucket.
const S3_ARN_NAMESPACE: &str = "s3";

/// The user pool a collection is being set on, as destination resolution reads it.
///
/// A pool is what the documented requirements are relative to: an ARN carrying no Region
/// falls back to the pool's ([`destination_region`]), and a log group has to be in the
/// pool's account.
pub struct ConfiguredPool<'a> {
    /// The Region the pool was created in.
    pub region: &'a str,
    /// The account the pool's own ARN names ([`account_of`]).
    pub account: &'a str,
}

/// The account the ARN `arn` names, and otherwise the one account this emulator serves.
///
/// A pool's ARN is minted by this service and always carries the account, so the fallback
/// stands in for nothing a request can produce: it only keeps a pool whose ARN cannot be
/// read from making every destination of this account unreachable.
pub fn account_of(arn: &str) -> String {
    ::service_common::arn::parse(arn)
        .map(|parsed| parsed.account.to_string())
        .unwrap_or_else(|| ::engine::emulator_account_id().to_string())
}

/// Whether `configurations` names a destination that is not one `pool` may export to.
///
/// Call it with NO state guard held — it awaits. Every entry is asked about, so the answer
/// is about the whole collection a `SetLogDeliveryConfiguration` request carries, which is
/// the unit that is accepted or refused.
pub async fn names_an_unresolvable_destination(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    pool: &ConfiguredPool<'_>,
    configurations: &[LogConfigurationType],
) -> bool {
    for configuration in configurations {
        if let Some(cloud_watch_logs) = &configuration.cloud_watch_logs_configuration {
            if log_group_is_absent(ctx, pool, cloud_watch_logs.log_group_arn.as_deref()).await {
                return true;
            }
        }
        if let Some(firehose) = &configuration.firehose_configuration {
            if delivery_stream_is_absent(ctx, pool, firehose.stream_arn.as_deref()).await {
                return true;
            }
        }
        if let Some(s3) = &configuration.s3_configuration {
            if bucket_is_absent(ctx, pool, s3.bucket_arn.as_deref()).await {
                return true;
            }
        }
    }
    false
}

/// Whether CloudWatch Logs holds no log group `pool` may export to under `arn`.
///
/// A destination carrying no ARN, one whose ARN is not a CloudWatch Logs log group's, and
/// one naming a group of another account name no log group this pool may export to: there
/// is nothing to ask about, and no log group is there to export to whatever CloudWatch Logs
/// would answer.
async fn log_group_is_absent(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    pool: &ConfiguredPool<'_>,
    arn: Option<&str>,
) -> bool {
    let Some(arn) = arn else {
        return true;
    };
    if log_group_of_account(arn, pool.account).is_none() {
        return true;
    }
    // A `logGroupIdentifier` names a log group "by name or ARN", so which group this ARN
    // addresses is read by the service that holds the group rather than re-read here.
    let answer = logs_client(ctx, destination_region(arn, LOGS_ARN_NAMESPACE, pool.region))
        .describe_log_groups()
        .log_group_identifiers(arn)
        .send()
        .await;
    match answer {
        Ok(answer) => match answer.log_groups().first() {
            // CloudWatch Logs holds no group this ARN addresses.
            None => true,
            // "The log group must not be encrypted with Key Management Service"
            // (`CloudWatchLogsConfigurationType.LogGroupArn`): a group whose ingested data
            // CloudWatch Logs encrypts with a KMS key is not a destination a pool may
            // export to, however much the group itself is there. Which groups those are is
            // the owning service's answer — `LogGroup$kmsKeyId` is "the Amazon Resource
            // Name (ARN) of the KMS key to use when encrypting log data" — not a judgement
            // made here.
            Some(group) => group.kms_key_id().is_some(),
        },
        Err(error) => {
            unresolved(arn, "CloudWatch Logs log group", &error);
            false
        }
    }
}

/// Whether Amazon Data Firehose holds no delivery stream under `arn`.
async fn delivery_stream_is_absent(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    pool: &ConfiguredPool<'_>,
    arn: Option<&str>,
) -> bool {
    let Some(arn) = arn else {
        return true;
    };
    let Some(stream) = named_resource(arn, FIREHOSE_ARN_NAMESPACE, DELIVERY_STREAM_RESOURCE_TYPE)
    else {
        return true;
    };
    let answer = firehose_client(
        ctx,
        destination_region(arn, FIREHOSE_ARN_NAMESPACE, pool.region),
    )
    .describe_delivery_stream()
    .delivery_stream_name(stream)
    .send()
    .await;
    match answer {
        Ok(_) => false,
        // Firehose reporting that it holds no such stream is what refuses the request.
        Err(error)
            if error.as_service_error().is_some_and(
                ::aws_sdk_firehose::operation::describe_delivery_stream::DescribeDeliveryStreamError
                    ::is_resource_not_found_exception,
            ) =>
        {
            true
        }
        Err(error) => {
            unresolved(arn, "Amazon Data Firehose stream", &error);
            false
        }
    }
}

/// Whether Amazon S3 holds no bucket under `arn`.
async fn bucket_is_absent(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    pool: &ConfiguredPool<'_>,
    arn: Option<&str>,
) -> bool {
    let Some(arn) = arn else {
        return true;
    };
    let Some(bucket) = s3_bucket(arn) else {
        return true;
    };
    // "All bucket names in the response begin with the specified bucket name prefix"
    // (`ListBucketsRequest$Prefix`), so the bucket this ARN names is the one the answer
    // carries under exactly that name.
    let answer = s3_client(ctx, destination_region(arn, S3_ARN_NAMESPACE, pool.region))
        .list_buckets()
        .prefix(bucket)
        .send()
        .await;
    match answer {
        Ok(answer) => !answer
            .buckets()
            .iter()
            .any(|held| held.name() == Some(bucket)),
        Err(error) => {
            unresolved(arn, "Amazon S3 bucket", &error);
            false
        }
    }
}

/// The name `arn` gives the resource it names, when it is an ARN of `vendor`'s
/// `resource_type` and names a resource at all.
///
/// `None` therefore covers a value that is not an ARN, one of another service, one of
/// another resource type of that service, and one whose resource segment is empty — each
/// of which names no destination of the kind the member documents.
fn named_resource<'a>(arn: &'a str, vendor: &str, resource_type: &str) -> Option<&'a str> {
    if !::service_common::arn::is_resource_arn_of(arn, vendor) {
        return None;
    }
    let name = ::service_common::arn::resource_id_of_type(arn, resource_type)?;
    (!name.is_empty()).then_some(name)
}

/// The log group `arn` names in `pool_account`, when it names one at all.
///
/// Beside what every destination ARN has to name ([`named_resource`]), a log group carries
/// the one requirement this member DOCUMENTS of the resource's own identity: the group
/// "must be in the same Amazon Web Services account as your user pool"
/// (`CloudWatchLogsConfigurationType.LogGroupArn`), so a group of ANOTHER account names no
/// destination this pool may export to, whoever holds it.
fn log_group_of_account<'a>(arn: &'a str, pool_account: &str) -> Option<&'a str> {
    let name = named_resource(arn, LOGS_ARN_NAMESPACE, LOG_GROUP_RESOURCE_TYPE)?;
    (::service_common::arn::parse(arn)?.account == pool_account).then_some(name)
}

/// The bucket an `S3ConfigurationType.BucketArn` names.
///
/// `S3ArnType`'s pattern is `^arn:[\w+=/,.@-]+:[\w+=/,.@-]+:::[\w+=/,.@-]+…$` — no Region
/// and no account — so the resource field is the bucket itself. A value carrying a `/`
/// addresses an object under a bucket rather than the bucket a log export is written to,
/// and so names no destination.
fn s3_bucket(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse_of(arn, S3_ARN_NAMESPACE)?;
    (!parsed.resource.contains('/')).then_some(parsed.resource)
}

/// The Region the client resolving `arn` is pointed at: the Region `arn` itself names, and
/// otherwise the Region of the pool being configured.
///
/// A Region reaches an endpoint URL's authority, so it is a Region LABEL or nothing: both
/// readings narrow it ([`service_common::arn::region_of`] and
/// [`service_common::arn::region_label`]), and a value outside that grammar falls back to
/// this service's own default rather than to caller text.
fn destination_region<'a>(arn: &'a str, vendor: &str, pool_region: &'a str) -> &'a str {
    ::service_common::arn::region_of(arn, vendor)
        .or_else(|| ::service_common::arn::region_label(pool_region))
        .unwrap_or(crate::state::DEFAULT_REGION)
}

/// Trace a destination this emulator could not ask the owning service about, which is a
/// deployment with no such service mounted rather than a destination reported missing.
///
/// The caller's own ARN is rendered in its escaped `Debug` form, so a value carrying a
/// newline cannot forge a log line.
fn unresolved<E: ::std::error::Error + 'static>(arn: &str, destination: &str, error: &E) {
    ::tracing::warn!(
        destination_arn = ?arn,
        destination = %destination,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "a user pool's log delivery destination could not be resolved against the service \
         that holds it"
    );
}

/// CloudWatch Logs' client for `region`, over the in-process callback into this same
/// emulator.
///
/// The transport is in-process, so this disables what a network client needs and this one
/// cannot have: retries, timeouts, stalled-stream protection and identity caching each
/// need a sleep implementation the callback transport is not given, and there is nothing
/// to retry behind it anyway. The same client policy this crate's Lambda-trigger
/// invocation builds (`src/state/lambda_triggers.rs`).
fn logs_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_cloudwatchlogs::Client {
    let config = ::aws_sdk_cloudwatchlogs::Config::builder()
        .behavior_version(::aws_sdk_cloudwatchlogs::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_cloudwatchlogs::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!(
            "https://{LOGS_ARN_NAMESPACE}.{region}.amazonaws.com"
        ))
        .retry_config(::aws_sdk_cloudwatchlogs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudwatchlogs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudwatchlogs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudwatchlogs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudwatchlogs::Client::from_conf(config)
}

/// Amazon Data Firehose's client for `region`, over the same in-process callback and under
/// the same client policy as [`logs_client`].
fn firehose_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_firehose::Client {
    let config = ::aws_sdk_firehose::Config::builder()
        .behavior_version(::aws_sdk_firehose::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_firehose::config::Region::new(region.to_string()))
        .endpoint_url(format!(
            "https://{FIREHOSE_ARN_NAMESPACE}.{region}.amazonaws.com"
        ))
        .retry_config(::aws_sdk_firehose::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_firehose::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_firehose::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_firehose::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_firehose::Client::from_conf(config)
}

/// Amazon S3's client for `region`, over the same in-process callback and under the same
/// client policy as [`logs_client`].
///
/// Path style, because the emulator serves one endpoint for every service: the bucket
/// travels in the path rather than in a per-bucket host the router could not resolve.
fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{S3_ARN_NAMESPACE}.{region}.amazonaws.com"))
        .force_path_style(true)
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What each destination member's ARN has to name, before any service is asked: the
    /// vendor and the resource type the member documents, and a non-empty resource.
    #[test]
    fn only_an_arn_of_the_documented_resource_names_a_destination() {
        // (case, the ARN, the name it gives a log group)
        let log_groups: [(&str, &str, Option<&str>); 6] = [
            (
                "a log group ARN",
                "arn:aws:logs:us-east-1:000000000000:log-group:/aws/cognito/pool",
                Some("/aws/cognito/pool"),
            ),
            (
                "a log group ARN with the reported :* suffix",
                "arn:aws:logs:eu-west-2:000000000000:log-group:/aws/vendedlogs/pool:*",
                Some("/aws/vendedlogs/pool:*"),
            ),
            (
                "another partition",
                "arn:aws-us-gov:logs:us-gov-west-1:000000000000:log-group:pool",
                Some("pool"),
            ),
            (
                "a log group ARN naming no group",
                "arn:aws:logs:us-east-1:000000000000:log-group:",
                None,
            ),
            (
                "another resource of the same service",
                "arn:aws:logs:us-east-1:000000000000:destination:pool",
                None,
            ),
            ("not an ARN at all", "/aws/cognito/pool", None),
        ];
        for (case, arn, expected) in log_groups {
            let actual = named_resource(arn, LOGS_ARN_NAMESPACE, LOG_GROUP_RESOURCE_TYPE);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the log group {expected:?}, got {actual:?}"
            );
        }

        // (case, the ARN, the name it gives a delivery stream)
        let streams: [(&str, &str, Option<&str>); 4] = [
            (
                "a delivery stream ARN",
                "arn:aws:firehose:us-east-1:000000000000:deliverystream/cognito",
                Some("cognito"),
            ),
            (
                "a stream ARN of another service",
                "arn:aws:kinesis:us-east-1:000000000000:stream/cognito",
                None,
            ),
            (
                "a log group ARN in the stream member",
                "arn:aws:logs:us-east-1:000000000000:log-group:/aws/cognito/pool",
                None,
            ),
            (
                "a delivery stream ARN naming no stream",
                "arn:aws:firehose:us-east-1:000000000000:deliverystream/",
                None,
            ),
        ];
        for (case, arn, expected) in streams {
            let actual =
                named_resource(arn, FIREHOSE_ARN_NAMESPACE, DELIVERY_STREAM_RESOURCE_TYPE);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the delivery stream {expected:?}, got {actual:?}"
            );
        }

        // (case, the ARN, the bucket it names)
        let buckets: [(&str, &str, Option<&str>); 4] = [
            (
                "a bucket ARN",
                "arn:aws:s3:::cognito-user-pool-logs",
                Some("cognito-user-pool-logs"),
            ),
            (
                "an object ARN under a bucket",
                "arn:aws:s3:::cognito-user-pool-logs/pool/",
                None,
            ),
            (
                "a bucket ARN of another service",
                "arn:aws:s3express:::cognito-user-pool-logs",
                None,
            ),
            ("not an ARN at all", "cognito-user-pool-logs", None),
        ];
        for (case, arn, expected) in buckets {
            let actual = s3_bucket(arn);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the bucket {expected:?}, got {actual:?}"
            );
        }
    }

    /// Which log groups are in the pool's own account, which the member documents as the
    /// only ones a pool may export to.
    #[test]
    fn only_a_log_group_of_the_pools_account_names_a_destination() {
        let pool_account = "000000000000";
        // (case, the ARN, the name it gives a log group of the pool's account)
        let cases: [(&str, &str, Option<&str>); 5] = [
            (
                "a log group of the pool's own account",
                "arn:aws:logs:us-east-1:000000000000:log-group:/aws/cognito/pool",
                Some("/aws/cognito/pool"),
            ),
            (
                "a log group of another account",
                "arn:aws:logs:us-east-1:123456789012:log-group:/aws/cognito/pool",
                None,
            ),
            (
                "another account, in another Region and partition",
                "arn:aws-cn:logs:cn-north-1:111122223333:log-group:/aws/cognito/pool",
                None,
            ),
            (
                "the pool's account, in another Region",
                "arn:aws:logs:eu-west-2:000000000000:log-group:/aws/cognito/pool",
                Some("/aws/cognito/pool"),
            ),
            (
                "an ARN carrying no account at all",
                "arn:aws:logs:us-east-1::log-group:/aws/cognito/pool",
                None,
            ),
        ];
        for (case, arn, expected) in cases {
            let actual = log_group_of_account(arn, pool_account);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the log group {expected:?}, got {actual:?}"
            );
        }
    }

    /// Which account a destination's requirements are judged against: the one the pool's
    /// own ARN names.
    #[test]
    fn the_pools_account_is_the_one_its_own_arn_names() {
        // (case, the pool's ARN, the account it names)
        let cases: [(&str, &str, &str); 3] = [
            (
                "a user pool ARN",
                "arn:aws:cognito-idp:us-east-1:000000000000:userpool/us-east-1_abcd1234",
                "000000000000",
            ),
            (
                "a user pool ARN of another account",
                "arn:aws:cognito-idp:eu-west-2:123456789012:userpool/eu-west-2_abcd1234",
                "123456789012",
            ),
            (
                "a value that is not an ARN falls back to the account this emulator serves",
                "us-east-1_abcd1234",
                ::engine::emulator_account_id(),
            ),
        ];
        for (case, arn, expected) in cases {
            let actual = account_of(arn);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the account {expected:?}, got {actual:?}"
            );
        }
    }

    /// Which Region a destination is resolved in, and what keeps caller text out of the
    /// endpoint authority.
    #[test]
    fn the_resolved_region_is_the_arns_own_or_the_pools() {
        // (case, the ARN, the pool's Region, the Region the client is pointed at)
        let cases: [(&str, &str, &str, &str); 5] = [
            (
                "the ARN's own Region",
                "arn:aws:logs:eu-west-2:000000000000:log-group:pool",
                "us-west-2",
                "eu-west-2",
            ),
            (
                "an ARN carrying no Region falls back to the pool's",
                "arn:aws:s3:::cognito-user-pool-logs",
                "us-west-2",
                "us-west-2",
            ),
            (
                "a Region field outside the label grammar",
                "arn:aws:logs:not/a/region:000000000000:log-group:pool",
                "us-west-2",
                "us-west-2",
            ),
            (
                "neither names a Region label",
                "arn:aws:s3:::cognito-user-pool-logs",
                "Not A Region",
                crate::state::DEFAULT_REGION,
            ),
            (
                "an ARN of another vendor",
                "arn:aws:kinesis:eu-west-2:000000000000:stream/pool",
                "us-west-2",
                "us-west-2",
            ),
        ];
        for (case, arn, pool_region, expected) in cases {
            let actual = destination_region(arn, LOGS_ARN_NAMESPACE, pool_region);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the Region {expected:?}, got {actual:?}"
            );
        }
    }
}
