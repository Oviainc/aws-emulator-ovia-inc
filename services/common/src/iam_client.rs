// SPDX-License-Identifier: CC0-1.0

//! The ONE construction of the IAM client an emulated service resolves a caller-supplied
//! IAM entity through, so no two services configure that in-process hop differently.
//!
//! Several services have to ask IAM about a value a request carried before they write
//! anything — Firehose about a delivery `RoleARN`, EventBridge Scheduler about a target's
//! role, EMR about a cluster's `JobFlowRole`, which is an EC2 instance profile. IAM is
//! reached the way any emulated service reaches another: the AWS SDK driven over the
//! emulator's OWN in-process transport, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so the asking service names no
//! other service crate.
//!
//! The reading of what the ARN NAMES is [`crate::iam_role`]; this module is only how the
//! question is put on the wire. Services with their own copy of this builder — `iam_client`
//! in `service-com-amazonaws-firehose-firehose-20150804` and in
//! `service-com-amazonaws-cloudformation-cloudformation`, and the inline blocks in
//! `service-com-amazonaws-scheduler-awschronosservice`,
//! `service-com-amazonaws-eks-awswesleyfrontend` and
//! `service-com-amazonaws-codepipeline-codepipeline-20150709` — are to be MIGRATED onto it,
//! each in the change that owns its service. Nothing new should build the client by hand.

/// The IAM client a service asks about a caller-supplied IAM entity, addressed to `region`.
///
/// The transport is in-process, so there is nothing to retry, time out or watch for a
/// stalled stream — and each of those would need a sleep implementation this client is not
/// given. The same goes for caching the identity: the credentials the callback hands over
/// are used directly.
#[must_use]
pub fn iam_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_iam::Client {
    iam_client_through(ctx.callback(), region)
}

/// [`iam_client`] from the emulator callback itself rather than from a request's own
/// context.
///
/// The question is the same and so is the transport; what differs is who is asking. A
/// service that resolves an IAM entity OUTSIDE the request that named it — on a task it
/// spawned, which holds a clone of the callback and no `OperationContext` — reaches IAM
/// through this, so there is still one construction of this hop.
#[must_use]
pub fn iam_client_through(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_iam::Client {
    let config = ::aws_sdk_iam::Config::builder()
        .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_iam::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://iam.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_iam::Client::from_conf(config)
}
