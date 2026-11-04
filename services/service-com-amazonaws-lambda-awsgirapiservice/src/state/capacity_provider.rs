// SPDX-License-Identifier: CC0-1.0

//! How a capacity provider is ADDRESSED: the ARN this service mints for one, the ARN
//! resource type that ARN carries, the reading of a client-sent `CapacityProviderName` in
//! either documented form, and the refusals a value that names no provider — or an operator
//! role this service cannot manage its compute with — is answered with.
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

/// The sentence a capacity-provider operation refuses a well-formed `CapacityProviderName`
/// that names no provider with, where that sentence's variable part is the value the REQUEST
/// named — either documented `CapacityProviderName` form, echoed exactly as the caller
/// supplied it.
///
/// WHICH form the variable part takes is RECORDED PER OPERATION, because AWS states it per
/// operation and no model records it: the two sentences are otherwise identical, and both
/// are the same two fixed clauses.
///
/// * `DeleteCapacityProvider` and `ListFunctionVersionsByCapacityProvider` echo the
///   identifier, which is this function (corpus `simparity-wire-aws-v1`, fixture
///   `lambda-capacity-provider-negative-paths` steps `task#4` and `task#5`: a request
///   naming a bare provider name is answered with that bare name);
/// * `GetCapacityProvider` and `UpdateCapacityProvider` state the capacity-provider ARN the
///   identifier names in the Region the caller signed for, which is what
///   [`super::resolve_capacity_provider`] builds for every caller of it (same corpus,
///   fixture `lambda-capacity-provider-lifecycle` steps `task#3` and `task#4`: a request
///   naming a bare provider name is answered with the full ARN).
///
/// Nothing in either form is read out of the store: the provider the request named does not
/// exist, so there is no record to report anything from.
#[must_use]
pub fn capacity_provider_not_found_message(capacity_provider_name: &str) -> String {
    format!(
        "Capacity provider not found: {capacity_provider_name}. Check that the ARN is \
         correct and the capacity provider exists in this Region."
    )
}

/// Why a request naming an operator role this service cannot manage compute with is
/// refused.
///
/// Wholly fixed text: it names neither the role the request carried nor which half of the
/// question failed, because a caller that may not use the role is not told whether it
/// exists.
///
/// The question it answers is [`operator_role_is_unassumable`], which is the reading this
/// surface takes; only this sentence is the capacity-provider surface's own.
pub const OPERATOR_ROLE_INVALID_MESSAGE: &str =
    "The operator role is invalid or doesn't have sufficient permissions. Verify the role \
     and permissions and try again.";

/// Whether IAM ANSWERED that `operator_role_arn` is a role this service cannot manage a
/// provider's compute as: this account's IAM holds no role under the name the ARN carries,
/// or the trust policy of the role it does hold does not let this service assume it — the
/// create is then refused with [`OPERATOR_ROLE_INVALID_MESSAGE`] and nothing is stored.
///
/// It is the reading of the ONE shared question (`service_common::iam_role`) that the
/// cross-service suite `core/router/tests/all/lambda_capacity_provider_operator_role.rs`
/// pins over its table of operator roles: the role a provider manages compute as is part of
/// what the client provisions in this account, so "this account's IAM holds no role under
/// that name" IS an answer about the role the request named. That is what separates it from
/// the weaker reading a function's EXECUTION role takes
/// ([`super::execution_role_is_unassumable`]), where an absent record is evidence about this
/// emulator's store because an execution role is routinely provisioned out of band. Neither
/// reading restates which role of this account an ARN names, how IAM is asked, or how a
/// trust document is read.
///
/// `false` — do not refuse — wherever nothing is KNOWN: a value that names no role of THIS
/// account (another account's IAM is not this emulator's to ask), an emulator with no IAM
/// mounted to ask at all, and an answer this emulator could not read. The principal that has
/// to be admitted is the one a function's execution role admits
/// ([`super::EXECUTION_ROLE_PRINCIPAL`]): this service is what assumes both.
pub async fn operator_role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operator_role_arn: &str,
) -> bool {
    ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        operator_role_arn,
        super::EXECUTION_ROLE_PRINCIPAL,
    )
    .await
}
