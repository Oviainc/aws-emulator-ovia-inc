// SPDX-License-Identifier: CC0-1.0

//! Resolving the SERVICE ROLE a stack operation acts as, against the account's own IAM
//! records.
//!
//! `RoleARN` is "the Amazon Resource Name (ARN) of an IAM role that CloudFormation assumes"
//! — so whether a stack operation can run at all is a question about a role in IAM, and this
//! is where [`super`] asks it: the store those roles live in ([`iam_state`]), the principal
//! the role has to trust ([`SERVICE_PRINCIPAL`]), the reading of its trust policy
//! ([`role_trust_refusal`]) and the refusal every operation taking a `RoleARN` answers with
//! ([`refuse_unassumable_role`]).
//!
//! It lives beside the state module rather than in it because that file is at the length a
//! change to this repository may touch: the concern moved out whole, under the one name its
//! callers already reach it by ([`super`] re-exports it).
//!
//! Every path below is absolute, so this module imports nothing from its parent.

/// The IAM store a stack operation resolves its `RoleARN` against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so CloudFormation reads the very records `CreateRole` wrote
/// rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The service principal CloudFormation assumes a stack's service role as.
///
/// `CreateStackInput$RoleARN`: "The Amazon Resource Name (ARN) of an IAM role that
/// CloudFormation assumes to create the stack. CloudFormation uses the role's credentials
/// to make calls on your behalf." A role it is to assume has to trust this principal for
/// `sts:AssumeRole`.
pub const SERVICE_PRINCIPAL: &str = "cloudformation.amazonaws.com";

/// The IAM store in the SAME shared engine, which is where the account's roles live.
///
/// This is the IAM service's OWN accessor, called rather than copied, so CloudFormation
/// reads the very store that service's handlers write. `None` has the same meaning as for
/// [`super::service_state`]: the engine carries no such store.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// `None` when this emulator has no reason to think CloudFormation could not assume
/// `role_arn`; otherwise the `ValidationError` message that refuses the call.
///
/// A stack operation acts as the `RoleARN` it is handed, so it resolves that role up front
/// and refuses the call when the role's trust policy does not let
/// [`SERVICE_PRINCIPAL`] assume it. The role is resolved against the account's IAM records
/// — the same records `CreateRole` wrote — by its ARN. An ARN the IAM store holds no role
/// for is NOT evidence of a missing trust (the role may belong to another account, or to a
/// client that never told this emulator about it), so the operation goes ahead; only a role
/// this emulator holds and whose trust policy withholds the grant is refused.
///
/// The message names the submitted ARN, which is the rule the real service follows: the
/// refusal is built from the request's own `RoleARN`.
pub fn role_trust_refusal(role_arn: &str, iam: &IamState) -> Option<String> {
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let document = role.assume_role_policy_document.as_deref()?;
    // The rule itself is IAM's, not this service's, so it is read through the ONE
    // evaluator every service that resolves a caller-supplied service role shares: the
    // statement must name the principal and `sts:AssumeRole`, an explicit `Deny` wins
    // over any `Allow`, and the document is percent-decoded before it is parsed. A
    // document this emulator cannot read as a policy at all is no evidence either way.
    match ::service_common::iam_trust_policy::admits_service_principal(document, SERVICE_PRINCIPAL)
    {
        None | Some(true) => None,
        Some(false) => Some(format!("Role {role_arn} is invalid or cannot be assumed")),
    }
}

/// Refuse the call when the account's IAM records show CloudFormation cannot assume the
/// service role this request named, so nothing is written for a stack that could never
/// deploy under it.
///
/// Call it with the request's OWN `RoleARN` (`None` when it named none — "If you don't
/// specify a value, CloudFormation uses the role that was previously associated with the
/// stack", which was checked when that request named it) and with the handler's
/// `&*self.engine`, holding no store lock. Generic over the operation's error enum, so
/// every operation that takes a `RoleARN` refuses it the same way: HTTP 400 with the wire
/// code `ValidationError`.
///
/// An engine carrying no IAM store carries no CloudFormation store either, so the caller's
/// own [`super::service_state`] lookup is what answers that case; here it is simply no evidence.
///
/// Called BEFORE the store is read, so a request naming a role CloudFormation cannot assume
/// can never reach a write — which also means it is answered ahead of a stack-not-found or
/// already-exists refusal when a request earns both. Both are `ValidationError` on the wire,
/// and the model fixes no precedence between them, so the earlier refusal is the one whose
/// check needs no store.
pub fn refuse_unassumable_role<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    role_arn: Option<&str>,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let Some(role_arn) = role_arn else {
        return Ok(());
    };
    let Some(iam) = iam_state(engine) else {
        return Ok(());
    };
    let refusal = role_trust_refusal(role_arn, &iam.lock());
    match refusal {
        Some(message) => Err(crate::scaffolded::Scaffolded::validation_error(message)),
        None => Ok(()),
    }
}
