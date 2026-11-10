// SPDX-License-Identifier: CC0-1.0

//! Whether EventBridge may ASSUME the role a rule's target names.
//!
//! `Target$RoleArn` is "the Amazon Resource Name (ARN) of the IAM role to be
//! used for this target when the rule is triggered", and EventBridge makes that
//! target's call ACTING AS the role — so assuming the role is the first step of
//! every such invocation, taken before the call itself is made. The step is
//! about the ROLE alone: which API call would have followed does not enter into
//! it, and a role whose trust policy does not admit [`SERVICE_PRINCIPAL`] fails
//! the delivery whatever the target was going to be.
//!
//! That is why it is stated here on its own rather than only inside the whole
//! delivery decision ([`super::delivery_refusal`], which goes on to ask what the
//! policies in force on the role allow): a handover to a target this emulator
//! makes no API call for still reaches the assume-role step, so that step is
//! what decides it, and the event is dead-lettered exactly as one whose target
//! call was refused.

use super::*;

/// Why EventBridge cannot assume `role_arn` to deliver to the target that names
/// it, as the condition and sentence a dead-lettered event carries — or `None`
/// when nothing of the kind can be shown.
///
/// The decision is taken against the account's OWN IAM records (the ones
/// `CreateRole`/`UpdateAssumeRolePolicy` wrote) and through the shared reading of
/// a trust document ([`::service_common::iam_trust_policy`]), so no two services
/// handed the same document answer differently.
///
/// The emulator refuses only what it can SHOW, so each of these is evidence of
/// nothing and lets the delivery go ahead: a target naming no role at all, a
/// `RoleArn` the store holds no role for (it may belong to another account), a
/// role carrying no trust document, and a document that cannot be read as a
/// policy.
pub fn assume_role_refusal(role_arn: Option<&str>, iam: &IamState) -> Option<(String, String)> {
    let role_arn = role_arn?;
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let admitted = role
        .assume_role_policy_document
        .as_deref()
        .and_then(|document| {
            ::service_common::iam_trust_policy::admits_service_principal(
                document,
                SERVICE_PRINCIPAL,
            )
        });
    if admitted != Some(false) {
        return None;
    }
    Some((
        FAILED_TO_ASSUME_ROLE.to_string(),
        format!(
            "Event bus failed to assume role {role_arn}: its trust policy does not let \
             {SERVICE_PRINCIPAL} perform {}.",
            ::service_common::iam_trust_policy::ASSUME_ROLE_ACTION
        ),
    ))
}
