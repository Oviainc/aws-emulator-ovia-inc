// SPDX-License-Identifier: CC0-1.0

//! Who a signed request to IAM speaks for, and whether that principal's own
//! identity-based policies let it do what it asks.
//!
//! IAM is itself an AWS service whose API is governed by IAM: a call to it is authorized
//! against the calling principal's identity-based policies before the operation reports
//! or changes anything, and the answer is the one IAM's documented evaluation gives — an
//! explicit `Deny` whose statement is about the request wins over every `Allow`, an
//! `Allow` with no `Deny` permits, and a principal with neither is refused (the implicit
//! deny). A refused call is answered HTTP 403 with the error code `AccessDenied`
//! ([`crate::scaffolded::Scaffolded::access_denied`]) and a message built from the three
//! things the decision was made about: the principal, the action and the resource.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the IAM user the request's SigV4 access key id belongs to, read out of
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::access_keys`], the same
//!    map `GetUser` and `CreateAccessKey` resolve a nameless caller through;
//! 2. **what it may do** — the identity-based documents in force on that principal, from
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::principal_policies`],
//!    so a policy just put grants immediately and one just deleted stops granting
//!    immediately;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of IAM policy documents against `(action, resource)`, so IAM and every
//!    other service handed the same documents decide alike.
//!
//! ## The callers that are not IAM users
//!
//! * **A credential no IAM user owns.** The emulator's own published credential, and a
//!   session credential STS minted, are not keys this account's `access_keys` map holds.
//!   They carry no identity-based policies at all, which is the position AWS account root
//!   credentials are in, so such a request is allowed rather than refused on evidence
//!   that does not exist.
//! * **A principal holding a grant this emulator cannot read.** An AWS-managed policy is
//!   owned by AWS, so the account's store holds no document for it and
//!   `principal_policies` reports it as uncarried. A principal with such an attachment is
//!   allowed where the documents that ARE held neither grant nor deny: what that document
//!   grants is unknown here, and refusing on evidence this emulator does not have would
//!   deny access AWS permits. An explicit `Deny` in a document that is held still
//!   refuses, because no grant overturns one.

use ::service_common::iam_authorization::{self as iam, Decision};

use crate::scaffolded::AccessDenied;

use super::{ComAmazonawsIamAwsidentitymanagementv20100508State as IamState, PrincipalPolicies};

/// `iam:ListGroups`.
pub const LIST_GROUPS: &str = "iam:ListGroups";

/// `iam:ListRoles`.
pub const LIST_ROLES: &str = "iam:ListRoles";

/// `iam:ListUsers`.
pub const LIST_USERS: &str = "iam:ListUsers";

/// `iam:CreateGroup`.
pub const CREATE_GROUP: &str = "iam:CreateGroup";

/// The decision value for a request the policies in force permit — one of the three
/// `PolicyEvaluationDecisionType` declares
/// (`smithy_query(query="shape",shape="PolicyEvaluationDecisionType")`).
pub const DECISION_ALLOWED: &str = "allowed";

/// The decision value for a request a statement explicitly denies.
pub const DECISION_EXPLICIT_DENY: &str = "explicitDeny";

/// The decision value for a request nothing allows and nothing explicitly denies.
pub const DECISION_IMPLICIT_DENY: &str = "implicitDeny";

/// The modelled name of the decision the two sets of documents come to, as
/// `EvaluationResult$EvalDecision` reports it.
///
/// The three values are the three outcomes IAM's evaluation has, so the mapping lives
/// beside the evaluation rather than in each operation that reports one: an explicit
/// `Deny` from either side is [`DECISION_EXPLICIT_DENY`], a grant from either side with
/// no such deny is [`DECISION_ALLOWED`], and neither is the implicit deny.
pub fn decision_name(identity: Decision, resource: Decision) -> &'static str {
    if identity == Decision::Denied || resource == Decision::Denied {
        DECISION_EXPLICIT_DENY
    } else if iam::is_allowed(identity, resource) {
        DECISION_ALLOWED
    } else {
        DECISION_IMPLICIT_DENY
    }
}

/// Whether the principal that signed this request may perform `action` on `resource`.
///
/// `Ok(())` when the call is permitted; `Err` is the refusal the operation answers
/// through its error channel (HTTP 403, error code `AccessDenied`), worded from the
/// principal, the action and the resource the decision was made about.
///
/// A READ: the store is only consulted, so authorizing a call changes nothing a later
/// caller sees.
pub fn authorize(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Result<(), AccessDenied> {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // A credential this account holds no IAM user for carries no identity-based
    // policies, so there is nothing to judge it on (see the module docs).
    let Some((principal, policies)) = signing_principal(store, &access_key_id) else {
        return Ok(());
    };

    let principals = [principal.clone()];
    let request = iam::Request::new(action, resource, &principals);
    let decided = iam::identity_decision(&policies.documents, &request);
    if iam::is_allowed(decided, Decision::NotStated) {
        return Ok(());
    }
    // An attachment whose document the account does not hold is a grant this emulator
    // cannot read, so it cannot be the basis of a refusal — but it cannot overturn one
    // either: an explicit `Deny` in a document that IS held wins over every grant,
    // including one whose content is unknown here.
    if decided != Decision::Denied && !policies.uncarried.is_empty() {
        return Ok(());
    }

    // Why the call was refused: an explicit `Deny` that is about the request, or the
    // absence of any `Allow` for it. The two are different facts about the policies in
    // force, and AWS words them differently, because they need different fixes.
    let reason = if decided == Decision::Denied {
        "with an explicit deny in an identity-based policy".to_string()
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    // Audit record for the refusal: the acting principal, what it asked for and the
    // decision. The policy documents themselves are not logged.
    ::tracing::info!(
        audit = true,
        outcome = "refused_access_denied",
        principal = %principal,
        action = %action,
        resource = %resource,
        explicit_deny = decided == Decision::Denied,
        "the caller's identity-based policies do not permit this call"
    );
    Err(AccessDenied::new(format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )))
}

/// The IAM user the access key id `access_key_id` belongs to — its ARN and the
/// identity-based documents in force on it — or `None` when the account holds no user
/// for that key.
fn signing_principal(
    store: &IamState,
    access_key_id: &str,
) -> Option<(String, PrincipalPolicies)> {
    let record = store.access_keys.get(access_key_id)?;
    let user = store.users.get(&record.user_key)?;
    let policies = store.principal_policies(&user.arn)?;
    Some((user.arn.clone(), policies))
}
