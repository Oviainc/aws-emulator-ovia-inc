// SPDX-License-Identifier: CC0-1.0

//! Shutting down the Amazon EC2 instances a registered application runs on.
//!
//! An EC2 instance is EC2's resource, not this service's, so the only way a call here can
//! shut one down is to ask the service that owns it: the AWS SDK driven over the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]), which
//! routes inside this emulator, so this crate names no other service's crate and nothing
//! leaves the process. The effect is therefore observable where an instance's state
//! lives — `DescribeInstances` — rather than only in this service's own records.
//!
//! Nothing is invented when EC2 cannot be asked (no EC2 mounted, or a refusal): the hop
//! is logged at `warn` and the application's own shutdown stands, because this service
//! holds no instance state of its own to move and must not report an instance state it
//! did not observe.

/// The EC2 client this service shuts an instance down through.
///
/// `region` is the request's own signing region, so the hop stays in the Region the
/// caller addressed. The client is given the callback's own HTTP client, so the endpoint
/// URL below is only the address that transport routes on IN PROCESS: no socket is
/// opened, no name is resolved and nothing leaves this process. The credentials are the
/// callback's, which sign this in-process hop and never appear in a response.
fn ec2(
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

/// Ask EC2 to stop each of `instance_ids`.
///
/// Each instance is asked for ON ITS OWN rather than as one batch: EC2 refuses a whole
/// batch when any instance in it cannot be stopped, and the instances of one application
/// are independent of each other — one that EC2 no longer holds must not keep the rest
/// of the application's instances running.
///
/// The walk is as long as the list it is handed, which is the instances ONE registration
/// named (its single `Instances` entry and the instance of each component it declared, at
/// most [`crate::state::MAX_COMPONENTS_PER_APPLICATION`] of them) — so no request can
/// make this hop more work than that.
pub async fn stop_instances(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_ids: &[String],
) {
    if instance_ids.is_empty() {
        return;
    }
    let client = ec2(ctx, region);
    for instance_id in instance_ids {
        let answer = client
            .stop_instances()
            .instance_ids(instance_id)
            .send()
            .await;
        if let Err(error) = answer {
            // Visible at default verbosity: an application that stopped while its
            // instance kept running is exactly what this reports, so the gap is never
            // silent.
            ::tracing::warn!(
                %region,
                %instance_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the Amazon EC2 instance of a stopped SAP application could not be shut down"
            );
        }
    }
}
