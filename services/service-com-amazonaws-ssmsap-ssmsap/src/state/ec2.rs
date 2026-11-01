// SPDX-License-Identifier: CC0-1.0

//! The hop into Amazon EC2: the one client this crate asks the owner of an instance
//! through.
//!
//! An EC2 instance is EC2's resource, not this service's, so the only way a call here can
//! do anything with one is to ask the service that owns it: the AWS SDK driven over the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]), which
//! routes inside this emulator, so this crate names no other service's crate and nothing
//! leaves the process.
//!
//! The client lives here rather than beside the one hop that exists today
//! ([`super::ec2_shutdown`], which shuts the instances of a stopped application down), so
//! a second hop added later reaches EC2 the same way rather than building its own client.

/// The EC2 client this crate asks the owner of an instance through.
///
/// `region` is the request's own signing region, so the hop stays in the Region the
/// caller addressed. The client is given the callback's own HTTP client, so the endpoint
/// URL below is only the address that transport routes on IN PROCESS: no socket is
/// opened, no name is resolved and nothing leaves this process. The credentials are the
/// callback's, which sign this in-process hop and never appear in a response.
pub(crate) fn client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials the
        // callback hands over are used directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}
