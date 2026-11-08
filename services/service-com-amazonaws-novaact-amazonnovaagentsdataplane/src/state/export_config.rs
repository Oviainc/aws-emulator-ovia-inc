// SPDX-License-Identifier: CC0-1.0

//! Resolving a `WorkflowExportConfig`'s Amazon S3 bucket before a definition is stored.
//!
//! `WorkflowExportConfig$s3BucketName` is "The name of your Amazon S3 bucket, that Nova
//! Act uses to export your workflow data"
//! (`smithy_query(query="shape",shape="com.amazonaws.novaact#WorkflowExportConfig",model_path="services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/smithy-model/com-amazonaws-novaact-amazonnovaagentsdataplane.json")`),
//! so a definition carrying one names a destination this service would have to write to.
//! A destination the account does not hold is therefore an input fault answered at the
//! call that named it — the modelled `ValidationException` (HTTP 400), keyed by the member
//! the caller has to fix — rather than a definition that looks healthy and exports
//! nowhere.
//!
//! The bucket is asked OF Amazon S3 rather than re-decided here: `HeadBucket` is the
//! operation S3 documents for it ("You can use this operation to determine if a bucket
//! exists"), put through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]) in the emulator's own name. So
//! the bucket resolved is the one a client's own `CreateBucket` created, in the one account
//! this emulator serves, and every per-bucket rule stays the object store's own.
//!
//! Two things about the destination are established, in the order the caller has to fix
//! them in: the NAME has to be one Amazon S3 holds a bucket under at all (the workspace's
//! one reading of the published naming rules, [`::service_common::s3_bucket_name`]), which
//! is decided from the request alone; and the BUCKET has to be one this account holds,
//! which is asked of Amazon S3.
//!
//! An answer that is not S3 reporting the bucket absent refuses nothing: an assembly with
//! no object store to ask, or any other failure to ask it, establishes nothing about the
//! bucket, and a refusal has to be grounded in what S3 answered rather than assumed.
//!
//! Nothing here WRITES: a create that is accepted leaves the destination exactly as the
//! caller left it, and the export objects themselves are not this slice's.

use crate::ops::types::{ValidationException, WorkflowExportConfig};

/// The wire path of the export bucket member a refusal about it is keyed by
/// (`WorkflowExportConfig$s3BucketName` inside `exportConfig`).
pub const EXPORT_BUCKET_MEMBER: &str = "exportConfig.s3BucketName";

/// The refusal text for an export bucket this account's object store does not hold: what
/// the caller has to put right, with the bucket it named and the account the call was made
/// for in it.
///
/// Request-derived (the `s3BucketName` the caller sent) plus the account the emulator
/// serves, because those are what the caller fixes the call by changing — by naming a
/// bucket that exists in this account, or by creating that one.
pub fn export_bucket_message(bucket: &str) -> String {
    format!(
        "Failed to validate export bucket '{bucket}'. Ensure the bucket exists, is \
         accessible, and is owned by the calling account ({}).",
        ::engine::emulator_account_id()
    )
}

/// `None` when nothing establishes that this service could not export to `export_config`'s
/// bucket; otherwise the refusal that answers the create.
///
/// A request carrying no `exportConfig` names no destination and has nothing to resolve.
///
/// LOCK ORDER: the question is put to Amazon S3 over the in-process callback with NO guard
/// of this service's store held — the object store's front door must not wait on this
/// service's lock — so this is called before the store is locked.
pub async fn unusable_export_bucket(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    export_config: Option<&WorkflowExportConfig>,
) -> Option<ValidationException> {
    let bucket = &export_config?.s3_bucket_name;
    let refusal = || {
        super::paging::field_validation_failed(EXPORT_BUCKET_MEMBER, export_bucket_message(bucket))
    };
    // A value outside the published Amazon S3 bucket naming rules names no bucket in any
    // account, so no provisioning could make such a destination usable: it is settled from
    // the request alone, through the workspace's one reading of those rules, before S3 is
    // asked about it. The modelled `@length`/`@pattern` on this member already refuses most
    // of that domain at the boundary; what is left (a reserved prefix or suffix, adjacent
    // periods, an IP-address form) is refused here.
    if !::service_common::s3_bucket_name::is_well_formed(bucket) {
        return Some(refusal());
    }
    let answer = s3_client(ctx, region)
        .head_bucket()
        .bucket(bucket)
        .send()
        .await;
    match answer {
        // S3 holds the bucket, so the destination is one this service can export to as far
        // as this emulator can establish.
        Ok(_) => None,
        // S3 says this account holds no such bucket: the refusal is grounded in its answer.
        Err(error) if bucket_reported_absent(&error) => Some(refusal()),
        // Any other answer is this emulator failing to ask rather than S3 reporting a
        // missing bucket, so it refuses nothing. The bucket's own value is rendered in its
        // escaped `Debug` form, so a name carrying a newline cannot forge a log line.
        Err(error) => {
            ::tracing::debug!(
                bucket = ?bucket,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the export configuration's bucket could not be resolved against Amazon S3, \
                 so the create is not refused for it"
            );
            None
        }
    }
}

/// The Amazon S3 client this service resolves an export destination through: the AWS SDK
/// driven over the emulator's own in-process transport, in the emulator's own name.
fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
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
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Whether an S3 `HeadBucket` failure is S3 reporting that the bucket does not exist
/// (`NotFound`, the only error that operation models), as opposed to the call not reaching
/// S3 at all.
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
