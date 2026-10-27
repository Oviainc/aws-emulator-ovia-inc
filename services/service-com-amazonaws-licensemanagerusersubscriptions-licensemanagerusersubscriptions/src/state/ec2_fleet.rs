// SPDX-License-Identifier: CC0-1.0

//! Resolving the EC2 instance a request names against the instances the account has.
//!
//! An EC2 instance is EC2's resource, not this service's: `InstanceId` is a plain string
//! in this service's model and the model declares no set of instance ids
//! (`smithy_query(query="operation",operation="AssociateUser")`), so the only way a call
//! here can establish that the account has the instance a request names is to ask the
//! service that OWNS instances. That is the AWS SDK driven over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which routes inside this
//! emulator, so this crate names no other service's crate and nothing leaves the process —
//! the same seam every other service reaches another one through. An instance a client
//! launched with `ec2:RunInstances` is therefore a target this service's user-association
//! plane accepts, exactly as it is in the real service.
//!
//! The fleet this service holds of its own
//! ([`crate::state::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::account_instances`])
//! is consulted FIRST: it costs no hop, and it is what answers in a process that mounts
//! this service with no EC2 beside it. Neither source is authoritative over the other —
//! an instance either of them reports is one the account has — so the resolution fails
//! closed only when NEITHER holds the id.

/// The EC2 client this service resolves a caller-named instance through.
///
/// `region` is the request's own signing region, so the question is asked in the Region the
/// caller addressed. The client is given the callback's own HTTP client, so the endpoint URL
/// below is only the address that transport routes on IN PROCESS: no socket is opened, no
/// name is resolved and nothing leaves this process. The credentials are the callback's,
/// which sign this in-process hop and never appear in a response.
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

/// Whether the account has the EC2 instance `instance_id`, as the service that owns
/// instances reports it.
///
/// ONE instance is asked about, by id, so the work is independent of how many instances the
/// account has. `false` whenever EC2 cannot be asked or answers that it holds no such
/// instance: nothing about an instance is invented here, and an instance that cannot be
/// shown to be the caller's is not one this service acts on.
///
/// An instance EC2 reports in the terminal `terminated` state is NOT one the account has:
/// it is reported for a while after it is gone, and it runs nothing a user-based
/// subscription could be served from. Every other state is an instance the account has.
async fn ec2_holds_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_id: &str,
) -> bool {
    let answer = ec2(ctx, region)
        .describe_instances()
        .instance_ids(instance_id)
        .send()
        .await;
    match answer {
        Ok(output) => output
            .reservations()
            .iter()
            .flat_map(::aws_sdk_ec2::types::Reservation::instances)
            .any(|instance| {
                instance.instance_id() == Some(instance_id)
                    && instance.state().and_then(::aws_sdk_ec2::types::InstanceState::name)
                        != Some(&::aws_sdk_ec2::types::InstanceStateName::Terminated)
            }),
        Err(error) => {
            // At `debug`: an instance EC2 does not hold is the ordinary answer to this
            // question (it is how an unheld id is refused), so a failed hop is not a
            // service fault — but it IS why a request was refused, so it is traceable.
            ::tracing::debug!(
                %region,
                %instance_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the EC2 instance a user association names could not be resolved"
            );
            false
        }
    }
}

/// Whether the caller may be shown to have the EC2 instance `instance_id`: true exactly
/// when the account has it, in either of the two places an instance of this account can be
/// reported from — this service's own fleet seam, or EC2 itself.
///
/// The gate every operation that acts on a caller-named instance runs BEFORE it looks
/// anything else up, so a refusal discloses nothing about this service's own resources.
/// Asked in one place, so no two operations can resolve the same instance differently.
pub async fn authorizes_instance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    instance_id: &str,
) -> bool {
    // The lock is taken and released here, before the hop below: the store is never held
    // across an await.
    let held_locally = crate::state::service_state(engine)
        .is_some_and(|state| state.lock().authorizes_instance(instance_id));
    if held_locally {
        return true;
    }
    let region = crate::state::request_region(&ctx.parts().headers);
    ec2_holds_instance(ctx, &region, instance_id).await
}
