// SPDX-License-Identifier: CC0-1.0

//! The account's resource groups: a fact this service RESOLVES rather than holds.
//!
//! An application is created OVER a resource group, and a resource group is not a
//! resource of this service — it belongs to Resource Groups. So the group a create
//! names is resolved against Resource Groups, reached the way any emulated service
//! reaches another: through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]).
//!
//! The answer is only ever one of three things, and a refusal is never assumed from
//! silence: an emulator with no Resource Groups mounted knows nothing about the group,
//! and nothing is what it refuses on ([`GroupLookup::Unknown`]). Only Resource Groups
//! ANSWERING that the account holds no such group ([`GroupLookup::Absent`]) is grounds
//! for refusing the create.

/// What Resource Groups says about the group a request named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupLookup {
    /// Resource Groups reports the group.
    Present,
    /// Resource Groups reports that the account holds no group of that name in this
    /// region.
    Absent,
    /// Nothing is known: there is no Resource Groups to ask, or it answered something
    /// else.
    Unknown,
}

/// The Resource Groups client this service resolves a named group through.
///
/// The transport is in-process, so there is nothing to retry, time out or watch for a
/// stalled stream — and each of those would need a sleep implementation this client is
/// not given. The same goes for caching the identity: the credentials the callback hands
/// over are used directly. `region` is the request's own signing region, because a
/// resource group lives in the region whose endpoint created it.
fn resource_groups(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_resourcegroups::Client {
    let config = ::aws_sdk_resourcegroups::Config::builder()
        .behavior_version(::aws_sdk_resourcegroups::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_resourcegroups::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://resource-groups.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_resourcegroups::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_resourcegroups::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_resourcegroups::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_resourcegroups::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_resourcegroups::Client::from_conf(config)
}

/// What Resource Groups says about the group `name` names in `region`.
///
/// Resource Groups answers its own not-found error for a name the account holds no
/// group for, which is the one answer that says the group does not exist. Any other
/// failure is this emulator failing to ask rather than Resource Groups reporting a
/// missing group, so it says nothing ([`GroupLookup::Unknown`]).
pub async fn resolve_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> GroupLookup {
    let answer = resource_groups(ctx, region)
        .get_group()
        .group(name)
        .send()
        .await;
    let error = match answer {
        Ok(_) => return GroupLookup::Present,
        Err(error) => error,
    };
    if error
        .as_service_error()
        .is_some_and(::aws_sdk_resourcegroups::operation::get_group::GetGroupError::is_not_found_exception)
    {
        return GroupLookup::Absent;
    }
    ::tracing::debug!(
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "the resource group a request named could not be resolved against Resource Groups"
    );
    GroupLookup::Unknown
}
