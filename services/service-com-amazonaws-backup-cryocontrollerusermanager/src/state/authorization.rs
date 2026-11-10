// SPDX-License-Identifier: CC0-1.0

//! The authorization decision a call that addresses a RECOVERY POINT by its ARN
//! takes before it resolves that point.
//!
//! # The rule this module implements
//!
//! AWS Backup authorizes such a call on the recovery point the request NAMES,
//! and it authorizes before it resolves: the action is this service's action
//! namespace plus the API name (`backup:DeleteRecoveryPoint`), and the resource
//! is the `RecoveryPointArn` the request carried. A recovery point belongs to the
//! account its ARN's account field names, and nothing a principal of another
//! account holds can grant access to it — only that account's own resource-based
//! policy could, and this emulator serves exactly one account
//! ([`engine::emulator_account_id`]). So an ARN whose account field is not that
//! account is refused for lack of privileges, with the sentence AWS words such a
//! refusal ([`::service_common::iam_authorization::not_authorized_by_resource_policy`]).
//!
//! Refusing it as ABSENT would be wrong twice over: a caller would learn from the
//! refusal what another account does or does not hold, and a client that branches
//! on the authorization refusal — to re-assume a role, to surface a permissions
//! error, to stop retrying — would instead read a delete the service refused as a
//! point that is already gone.
//!
//! The decision is taken from the REQUEST alone, so it needs no store and runs
//! ahead of every lookup. It is stated once here, for every call that addresses a
//! recovery point by ARN, so the delete, the read and the two jobs that take one
//! as input cannot drift into different answers for one condition.

/// How much of the caller-supplied ARN the refusal echoes.
///
/// The ARN is a request value and the message is a response value, so only the
/// leading characters are echoed: a request can never be amplified into an
/// arbitrarily long refusal. The model types `RecoveryPointArn` as `ARN` with no
/// length constraint, and the ARNs this service mints are far shorter than this,
/// so no ARN that names a recovery point is ever shortened.
const MAX_ECHOED_ARN: usize = 256;

/// The authorization decision for `operation` on the recovery point
/// `recovery_point_arn` names: `Ok(())` when the ARN names a recovery point of
/// the account this emulator serves, and the refusal otherwise.
///
/// `operation` is the API name (`Self::NAME` at the call site), which with this
/// service's namespace is the IAM action the decision is taken on.
pub fn authorized_recovery_point(
    operation: &str,
    recovery_point_arn: &str,
) -> Result<(), crate::scaffolded::AccessDeniedException> {
    let own_account = ::service_common::arn::parse(recovery_point_arn)
        .is_some_and(|arn| arn.account == ::engine::emulator_account_id());
    if own_account {
        return Ok(());
    }
    let shown: String = recovery_point_arn.chars().take(MAX_ECHOED_ARN).collect();
    Err(crate::scaffolded::AccessDeniedException::new(
        ::service_common::iam_authorization::not_authorized_by_resource_policy(
            &::service_common::caller_principal::ambient_user_arn(),
            &format!("{}:{operation}", super::ARN_NAMESPACE),
            &shown,
        ),
    ))
}
