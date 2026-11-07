// SPDX-License-Identifier: CC0-1.0

//! The ONE construction of the in-process EventBridge client an emulated service publishes
//! an event through, so no two services configure that hop differently — the sibling of
//! [`crate::kinesis_client`] for the other service emulated services deliver to.
//!
//! A service that PUBLISHES onto a bus — a state-change event of its own, or the
//! management-event fan-out of a call it served ([`crate::api_call_events`]) — reaches
//! EventBridge the way any emulated service reaches another: the AWS SDK driven over the
//! emulator's own in-process transport, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]). The transport is what makes the
//! publication in-process: no socket is opened, no name is resolved, and nothing outside
//! this process is reachable from it. The endpoint URL below is addressing for that
//! transport, not a host this process connects to, and it is built from the Region alone —
//! no part of a caller's request is a destination.
//!
//! The copies that predate this module — the ones in
//! `service-com-amazonaws-batch-awsbatchv20160810`'s `state`,
//! `service-com-amazonaws-codepipeline-codepipeline-20150709`'s `state::events`,
//! `service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice`'s
//! `state::package_version_events` and their siblings — are to be MIGRATED onto it, each in
//! the change that owns its service. Nothing new should build the client by hand.
//!
//! The callback is taken as the `EmbeddedEmulator` rather than as an `OperationContext`,
//! because a publisher is not always inside one: a clock pump outlives the request that
//! started it and publishes over the same callback.

use smithy_server_runtime::EmbeddedEmulator;

/// The EventBridge client a service publishes an event through, addressed to `region`.
///
/// The transport is in-process, so there is nothing to retry, time out or watch for a
/// stalled stream — each of those would need a sleep implementation this client is not
/// given — and the credentials the callback hands over are used directly rather than
/// cached. Those credentials are the emulator's own: the caller's `Authorization` header is
/// never forwarded.
#[must_use]
pub fn eventbridge_client(
    callback: &EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_eventbridge::Client {
    let config = ::aws_sdk_eventbridge::Config::builder()
        .behavior_version(::aws_sdk_eventbridge::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_eventbridge::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://events.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_eventbridge::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_eventbridge::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_eventbridge::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_eventbridge::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_eventbridge::Client::from_conf(config)
}
