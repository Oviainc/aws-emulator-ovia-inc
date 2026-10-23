// SPDX-License-Identifier: CC0-1.0

//! How a capacity provider is ADDRESSED: the ARN this service mints for one, the ARN
//! resource type that ARN carries, the reading of a client-sent `CapacityProviderName`
//! in either documented form, and the refusal a value that names no provider is answered
//! with.
//!
//! It lives in its own module rather than in the already long [`crate::state`], which
//! re-exports every item here, so a handler still names them as `crate::state::…` and the
//! module split is invisible to it. The record itself, its clock-owed `Pending` ->
//! `Active` transition and the store that holds it stay in [`crate::state`] beside the
//! function slice, because the tagging surface and `materialize_all` reach them there.
//!
//! Minting and reading sit together ON PURPOSE: [`capacity_provider_arn`] writes the ARN
//! and [`parse_capacity_provider_name`] reads one back, both over
//! [`CAPACITY_PROVIDER_RESOURCE_TYPE`], so the two cannot drift.
//!
//! Nothing here can panic: the value a caller sends is UNTRUSTED text, and it is only ever
//! split on `:` and compared — nothing indexes, slices or does arithmetic on it.

use super::ARN_NAMESPACE;

/// The ARN resource type of a capacity provider, as the model's `CapacityProviderArn`
/// pattern writes it (`…:lambda:<region>:<account>:capacity-provider:<name>`) — stated
/// once, for the ARN this module mints and the ARN it reads back.
pub const CAPACITY_PROVIDER_RESOURCE_TYPE: &str = "capacity-provider";

/// The ARN of a capacity provider:
/// `arn:aws:lambda:<region>:<account>:capacity-provider:<name>`, as the model's
/// `CapacityProviderArn` pattern requires.
#[must_use]
pub fn capacity_provider_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{CAPACITY_PROVIDER_RESOURCE_TYPE}:{name}"),
    )
}

/// The bare provider name a `CapacityProviderName` names.
///
/// The model's `CapacityProviderName` pattern admits two forms — a capacity-provider ARN
/// and a bare name — and both name the same record, so an ARN resolves here to the name
/// it carries. `None` when the value is an `arn:`-prefixed string that is not a
/// capacity-provider ARN of this service, which names no provider at all.
///
/// The value is UNTRUSTED request text, so the ARN form goes through the ONE shared parser
/// (`service_common::arn`, whose `parse` walks the six fields with `splitn(6, ':')` and `?`
/// on each `next()`) rather than a second ad-hoc reader, and the bare form is taken as it
/// stands: nothing here indexes, slices or does arithmetic on the value, so no input can
/// make it panic.
#[must_use]
pub fn parse_capacity_provider_name(value: &str) -> Option<String> {
    if !value.starts_with("arn:") {
        return (!value.is_empty()).then(|| value.to_string());
    }
    // arn:<partition>:lambda:<region>:<account>:capacity-provider:<name>. The resource
    // type is required, so an ARN of another Lambda resource is not read as a provider
    // named after its resource type, and an ARN of another vendor names no provider here.
    ::service_common::arn::parse_of(value, ARN_NAMESPACE)?;
    ::service_common::arn::resource_id_of_type(value, CAPACITY_PROVIDER_RESOURCE_TYPE)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// The message a `CapacityProviderName` that names no provider is refused with: a
/// validation message built from the request, naming the member and the constraint it
/// failed.
#[must_use]
pub fn capacity_provider_name_message(value: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at 'capacityProviderName' failed to \
         satisfy constraint: Member must satisfy one of the documented name formats (name \
         or capacity provider ARN)"
    )
}
