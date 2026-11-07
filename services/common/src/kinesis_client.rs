// SPDX-License-Identifier: CC0-1.0

//! The ONE construction of the in-process Kinesis client an emulated service reaches a data
//! stream through, so no two services configure that hop differently.
//!
//! A service that DELIVERS to a stream — the AWS IoT rules engine's `kinesis` topic-rule
//! action today — or that RESOLVES one a caller named against the streams Kinesis itself
//! holds — Aurora DSQL's CDC stream target today — reaches Kinesis the way any emulated
//! service reaches another: the AWS SDK driven over the emulator's own in-process transport,
//! through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]). The transport is what makes the
//! hop in-process: no socket is opened, no name is resolved, and nothing outside this
//! process is reachable from it. The endpoint URL below is addressing for that transport, not
//! a host this process connects to, and it is built from the Region alone — no part of a
//! caller's request is a destination.
//!
//! The copies that predate this module — the `callback_client!` macro in
//! `service-com-amazonaws-eventbridge-awsevents`'s `ops::put_events`, which builds this and
//! four sibling clients the same way, and the clients in
//! `service-com-amazonaws-lambda-awsgirapiservice`'s `state::event_source_poller` and
//! `service-com-amazonaws-cloudwatchlogs-logs-20140328`'s `state::subscription_delivery` —
//! are to be MIGRATED onto it, each in the change that owns its service. Nothing new should
//! build the client by hand.

/// The Kinesis client a service reaches a data stream through, addressed to `region`.
///
/// The transport is in-process, so there is nothing to retry, time out or watch for a stalled
/// stream — each of those would need a sleep implementation this client is not given — and
/// the credentials the callback hands over are used directly rather than cached. Those
/// credentials are the emulator's own: the caller's `Authorization` header is never forwarded.
#[must_use]
pub fn kinesis_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_kinesis::Client {
    let config = ::aws_sdk_kinesis::Config::builder()
        .behavior_version(::aws_sdk_kinesis::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_kinesis::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kinesis.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_kinesis::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kinesis::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kinesis::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_kinesis::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_kinesis::Client::from_conf(config)
}
