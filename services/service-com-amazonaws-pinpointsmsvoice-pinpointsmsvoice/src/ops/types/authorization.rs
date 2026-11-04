// SPDX-License-Identifier: CC0-1.0

//! Whether the account this emulator serves may perform an action of this API at all —
//! the half of a request that is decided before any resource is resolved.
//!
//! RULE: this API's RESOURCE-SCOPED actions (every action on a configuration set, on an
//! event destination, or on a voice call) are authorized only for an account that holds
//! the entitlement for this legacy surface. For an account that does not, the service
//! answers HTTP 403 with the error code `AccessDeniedException` and the message
//! `Access Denied`, before it looks the named resource up and whatever else the request
//! carried — so the refusal is the same for a name that exists and one that never did,
//! and the same for every payload. The ACCOUNT-SCOPED listing names no resource, is not
//! covered by the rule, and keeps answering what is stored.
//!
//! `AccessDeniedException` is the code of no shape in this model
//! (`smithy_query(query="shape",shape="AccessDeniedException",model_path="services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/smithy-model/com-amazonaws-pinpointsmsvoice-pinpointsmsvoice.json")`
//! finds none; the model's own error vocabulary is `BadRequestException`,
//! `InternalServiceErrorException`, `TooManyRequestsException`, `AlreadyExistsException`,
//! `LimitExceededException` and `NotFoundException`), so the refusal is answered through
//! [`crate::scaffolded::Scaffolded::service_failure`], which carries the wire code and the
//! status with it.
//!
//! The entitlement itself is state, not a constant:
//! [`crate::state::ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState::legacy_surface_entitled`].
//!
//! ORDERING: this is AUTHORIZATION, which a malformed request never reaches — a parameter
//! this service cannot use is refused first, in `super::refusal_wording`'s wording, so a
//! caller can still tell "fix the request" (400) from "you may not do this" (403). Within
//! authorization the more specific refusal comes first: a request that hands this service a
//! role of another account is told so ([`pass_role_denial`]), whatever the account's own
//! entitlement.

/// The wire error code (`__type`, and the `x-amzn-errortype` header) an authorization
/// refusal carries. A client branches on it, so it is the one thing here that is fixed.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The HTTP status an authorization refusal is answered with: the request was understood
/// and is not permitted, which is permanent, so an SDK must not retry it.
const ACCESS_DENIED_STATUS: u16 = 403;

/// The whole message an authorization refusal carries. Fixed text with no variable part:
/// the refusal names neither the action nor the resource, because it is decided before
/// either is resolved.
const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// The message the refusal carries instead when what the request hands over is a role
/// ANOTHER account holds. Fixed text too — it names no ARN — but a different sentence,
/// because the two refusals are different failures: this one a caller fixes by passing a
/// role of its own account, the other it cannot fix at all.
const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// The IAM resource type a passed role ARN names, which is what makes it a role rather
/// than some other IAM entity.
const ROLE_RESOURCE_TYPE: &str = "role";

/// The ARN vendor field IAM's own resources carry (`aws.api#service$arnNamespace` of IAM).
const IAM_VENDOR: &str = "iam";

/// The refusal a resource-scoped action gets when the account this emulator serves is not
/// entitled to this legacy surface, or `None` when it is and the action may proceed.
///
/// Every handler of a resource-scoped action asks this ONE question, so the rule lives
/// here rather than once per operation and no two of them can disagree about it.
pub fn resource_scope_denial<E>(
    store: &crate::state::ComAmazonawsPinpointsmsvoicePinpointsmsvoiceState,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    (!store.legacy_surface_entitled).then(access_denied)
}

/// The refusal an action gets when the event destination it defines hands this service an
/// IAM role ARN naming an account OTHER than the one this emulator serves, or `None` when
/// it hands over no such role.
///
/// `iam:PassRole` is granted by the account that HOLDS the role, so a role of another
/// account is one no principal of this account can hand to a service, whatever else is
/// true of the request — which is why this is decided for every account, entitled or not,
/// and ahead of [`resource_scope_denial`]: it is the more specific of the two
/// authorization failures, and a caller can act on it.
///
/// A destination that passes no role at all (an SNS topic) reaches the other decision.
pub fn pass_role_denial<E>(
    definition: &super::EventDestinationDefinition,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    [
        definition
            .cloud_watch_logs_destination
            .as_ref()
            .and_then(|destination| destination.iam_role_arn.as_deref()),
        definition
            .kinesis_firehose_destination
            .as_ref()
            .and_then(|destination| destination.iam_role_arn.as_deref()),
    ]
    .into_iter()
    .flatten()
    .any(names_another_accounts_role)
    .then(cross_account_pass_role)
}

/// Whether `role_arn` names an IAM role an account OTHER than the one this emulator serves
/// holds.
///
/// The six ARN fields come from `service-common`'s one ARN parse. A value that names this
/// account, that names no account at all, that names another IAM entity, or that is not an
/// ARN is NOT a cross-account pass: either there is no other account's grant in the way, or
/// what is wrong with the value is its shape rather than whose role it is.
fn names_another_accounts_role(role_arn: &str) -> bool {
    service_common::arn::parse(role_arn).is_some_and(|arn| {
        arn.vendor == IAM_VENDOR
            && arn.resource_type() == ROLE_RESOURCE_TYPE
            && !arn.account.is_empty()
            && arn.account != ::engine::emulator_account_id()
    })
}

/// The authorization refusal itself: HTTP 403, code [`ACCESS_DENIED_CODE`], message
/// [`ACCESS_DENIED_MESSAGE`].
pub fn access_denied<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        ACCESS_DENIED_CODE,
        ACCESS_DENIED_STATUS,
        ACCESS_DENIED_MESSAGE,
    )
}

/// The same refusal, worded for a role another account holds
/// ([`CROSS_ACCOUNT_PASS_ROLE_MESSAGE`]): the same code and status, because it is the same
/// decision reached for a more specific reason.
pub fn cross_account_pass_role<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        ACCESS_DENIED_CODE,
        ACCESS_DENIED_STATUS,
        CROSS_ACCOUNT_PASS_ROLE_MESSAGE,
    )
}
