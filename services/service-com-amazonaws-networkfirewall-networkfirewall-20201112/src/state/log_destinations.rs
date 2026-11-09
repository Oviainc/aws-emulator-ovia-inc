// SPDX-License-Identifier: CC0-1.0

//! Whether a firewall's log destination is one this service may attach log
//! delivery to, as the service that OWNS that destination reports it.
//!
//! A log destination is not a resource of this service: an Amazon S3 bucket is
//! Amazon S3's, a log group Amazon CloudWatch Logs's, a delivery stream Amazon
//! Data Firehose's. `UpdateLoggingConfiguration` attaches log delivery to the one
//! the caller names, and for an Amazon S3 bucket that attachment is a change to
//! the BUCKET's own policy — which is why configuring an S3 destination needs the
//! caller to hold `s3:GetBucketPolicy` and `s3:PutBucketPolicy` on that bucket,
//! and why a bucket whose own policy denies the caller those two permissions is
//! one no log delivery can be attached to (`LogDestinationPermissionException`,
//! client, HTTP 400, "Unable to send logs to a configured logging destination").
//!
//! So the question is put where the answer lives, reached the way any emulated
//! service reaches another: the AWS SDK driven over the emulator's own in-process
//! transport through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), built by the
//! workspace's one construction of that client
//! ([`service_common::s3_client::s3_client`]). No socket is opened, no name is
//! resolved, and the endpoint it addresses is built from the Region alone — no
//! part of a caller's request is a destination.
//!
//! This service keeps only the destination the caller named: it writes no log
//! file and reports nothing it read off a destination. So a destination this
//! emulator cannot ask about — no Amazon S3 mounted in this process, an answer
//! that could not be read, a bucket carrying no policy at all — refuses nothing,
//! and only the owner's own answer grounds a refusal. The same holds for the
//! destination types whose owner keeps no per-destination policy document to
//! decide against: a CloudWatch Logs log group and a Firehose delivery stream
//! carry none, so naming one is not refused here.

use ::service_common::iam_authorization::{
    account_principals, resource_decision, Decision, Request,
};

use crate::ops::types::LogDestinationConfig;

/// `LogDestinationType` members
/// (`smithy_query(query="shape",shape="LogDestinationType")`).
pub const LOG_DESTINATION_S3: &str = "S3";
/// See [`LOG_DESTINATION_S3`].
pub const LOG_DESTINATION_CLOUDWATCH_LOGS: &str = "CloudWatchLogs";
/// See [`LOG_DESTINATION_S3`].
pub const LOG_DESTINATION_FIREHOSE: &str = "KinesisDataFirehose";

/// The `LogDestination` map key each destination type names its destination by,
/// from `LogDestinationConfig$LogDestination`: "for an Amazon S3 bucket, provide
/// the name of the bucket, with key `bucketName`"; "for a CloudWatch log group,
/// provide the name of the CloudWatch log group, with key `logGroup`"; "for a
/// Firehose delivery stream, provide the name of the delivery stream, with key
/// `deliveryStream`".
pub const DESTINATION_KEY_BUCKET: &str = "bucketName";
/// See [`DESTINATION_KEY_BUCKET`].
pub const DESTINATION_KEY_LOG_GROUP: &str = "logGroup";
/// See [`DESTINATION_KEY_BUCKET`].
pub const DESTINATION_KEY_DELIVERY_STREAM: &str = "deliveryStream";

/// The `LogDestination` key a destination of `destination_type` is named by, or
/// `None` for a type this service knows no key for.
pub fn destination_key(destination_type: &str) -> Option<&'static str> {
    match destination_type {
        LOG_DESTINATION_S3 => Some(DESTINATION_KEY_BUCKET),
        LOG_DESTINATION_CLOUDWATCH_LOGS => Some(DESTINATION_KEY_LOG_GROUP),
        LOG_DESTINATION_FIREHOSE => Some(DESTINATION_KEY_DELIVERY_STREAM),
        _ => None,
    }
}

/// The destination `config` names — the value its type's key carries — or `None`
/// where the map carries no value for that key.
pub fn named_destination(config: &LogDestinationConfig) -> Option<&str> {
    let key = destination_key(&config.log_destination_type)?;
    config.log_destination.get(key).map(String::as_str)
}

/// What a `LogDestination` map that names no destination is refused with: a fixed
/// stem naming the key the destination type is named by, the type in parameter
/// brackets, and the input path the map sits on
/// ([`crate::state::parameter_context_message`]).
///
/// The rule is `LogDestinationConfig$LogDestination`'s own: the map is "provided
/// in a key:value mapping that is specific to the chosen destination type", one
/// documented key per type, so a map carrying none of them names nothing to send
/// logs to (`InvalidRequestException`, client, HTTP 400).
pub fn missing_destination_key_message(destination_type: &str, key: &str) -> String {
    crate::state::parameter_context_message(
        &format!("LogDestination must provide the key {key}"),
        destination_type,
        "LoggingConfiguration.LogDestinationConfigs.LogDestination",
    )
}

/// Why the destination `config` names cannot be sent logs, as the message the
/// refusal carries, or `None` when it names one.
///
/// The check is `LogDestinationConfig$LogDestination`'s documented one: the map
/// names its destination under the key its destination type publishes
/// ([`destination_key`]), so a map carrying no value for that key names nothing.
/// A `LogDestinationType` outside the three the model declares is refused by the
/// boundary validator before a handler runs, so it reaches no check here.
pub fn destination_breach(config: &LogDestinationConfig) -> Option<String> {
    let key = destination_key(&config.log_destination_type)?;
    (!config.log_destination.contains_key(key))
        .then(|| missing_destination_key_message(&config.log_destination_type, key))
}

/// What a destination the caller may not have log delivery attached to is refused
/// with, naming that destination.
///
/// The model fixes the classification and the status only
/// (`smithy_query(query="shape",shape="LogDestinationPermissionException")`:
/// client error, HTTP 400, member `Message`), so the wording is the recorded one:
/// a fixed sentence naming the destination the attachment was refused on — which
/// is the one thing the error code alone cannot tell a caller that named several
/// destinations — and then the fixed instruction to check that destination's
/// permissions. Nothing else from the request appears in it.
pub fn destination_permission_message(destination: &str) -> String {
    format!(
        "Access Denied for LogDestination: {destination}. Please check LogDestination permission"
    )
}

/// The two permissions attaching log delivery to an Amazon S3 bucket needs: the
/// bucket's policy is READ and then WRITTEN, because the log-delivery grant is a
/// statement in that policy.
const GET_BUCKET_POLICY_ACTION: &str = "s3:GetBucketPolicy";
const PUT_BUCKET_POLICY_ACTION: &str = "s3:PutBucketPolicy";

/// Amazon S3's ARN vendor, which a bucket's own ARN names (`arn:aws:s3:::<bucket>`
/// IS the bucket).
const S3_ARN_NAMESPACE: &str = "s3";

/// Whether the destination `config` names is one log delivery may be attached to,
/// or the destination to refuse the attachment on.
///
/// Call it with NO store guard held — it awaits. `caller_arn` is any ARN of this
/// account (the firewall's own), which is where the calling account's principal
/// spellings are read from.
pub async fn attachment_refused(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    caller_arn: &str,
    config: &LogDestinationConfig,
) -> Option<String> {
    // Only Amazon S3 keeps a policy document on the destination itself, so it is
    // the only destination type whose owner can answer this question; see the
    // module docs.
    if config.log_destination_type != LOG_DESTINATION_S3 {
        return None;
    }
    let bucket = named_destination(config)?;
    let client = s3(ctx, region)?;
    let policy = match client.get_bucket_policy().bucket(bucket).send().await {
        Ok(answer) => answer.policy().map(str::to_string),
        // Amazon S3 answers `NoSuchBucketPolicy` for a bucket carrying no policy
        // at all, and that bucket denies nothing: the attachment would add the
        // first statement to it.
        Err(error) if carries_no_policy(&error) => None,
        Err(error) => {
            unresolved(bucket, &error);
            None
        }
    }?;

    let bucket_arn = format!("arn:aws:{S3_ARN_NAMESPACE}:::{bucket}");
    let principals = account_principals(caller_arn);
    let denied = [GET_BUCKET_POLICY_ACTION, PUT_BUCKET_POLICY_ACTION]
        .iter()
        .any(|action| {
            let request = Request::new(action, &bucket_arn, &principals);
            resource_decision(&policy, &request) == Decision::Denied
        });
    denied.then(|| bucket.to_string())
}

/// Whether this refusal of the policy read is Amazon S3 saying the bucket carries
/// NO policy, rather than a failure to ask it at all.
fn carries_no_policy<E: ::aws_sdk_s3::error::ProvideErrorMetadata>(error: &E) -> bool {
    ::aws_sdk_s3::error::ProvideErrorMetadata::code(error) == Some(NO_SUCH_BUCKET_POLICY)
}

/// The error code Amazon S3's own documentation gives for a policy read of a
/// bucket that carries no policy.
const NO_SUCH_BUCKET_POLICY: &str = "NoSuchBucketPolicy";

/// Trace a destination this emulator could not ask its owner about, which is a
/// deployment with no such service mounted rather than a destination reported
/// unusable.
///
/// The caller's own destination name is rendered in its escaped `Debug` form, so a
/// value carrying a newline cannot forge a log line.
fn unresolved<E: ::std::error::Error + 'static>(destination: &str, error: &E) {
    ::tracing::warn!(
        destination = ?destination,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "the log destination a firewall's logging configuration names could not be resolved \
         against its owning service"
    );
}

/// An AWS SDK client for Amazon S3 over the in-process callback into this same
/// emulator: the workspace's ONE construction of that client
/// ([`service_common::s3_client::s3_client`]), so no two services configure the
/// hop differently.
///
/// The Region is read as a Region LABEL
/// (`service_common::arn::region_label`) before it reaches that client's endpoint
/// authority, so no caller-supplied text names the host this hop addresses.
fn s3(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_s3::Client> {
    let region = ::service_common::arn::region_label(region)?;
    Some(::service_common::s3_client::s3_client(
        ctx.callback(),
        region,
    ))
}
