// SPDX-License-Identifier: CC0-1.0

//! Whether the ROLE a topic-rule action names lets the rules engine make that action's API
//! call.
//!
//! A rule's action does not reach its target as whoever created the rule: each action names
//! "the ARN of the IAM role that grants access to" its target, and AWS IoT acts AS that role,
//! so an action whose role may not make the call is an action that delivers nothing. The
//! engine therefore asks this before it performs one ([`super::rules_engine`]).
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the role the action names, resolved against the IAM store in the SAME shared
//!    engine, so the record is the one `CreateRole` wrote;
//! 2. **whether this service may assume it at all** — the role's trust document, read through
//!    [`::service_common::iam_trust_policy`];
//! 3. **what the role may do** — the identity-based documents the IAM store holds for it (its
//!    inline policies and the default version of each managed policy attached to it), read
//!    through IAM's own [`principal_policies`](IamState::principal_policies) accessor, plus
//!    the permissions boundary that caps them
//!    ([`permissions_boundary_document`](IamState::permissions_boundary_document));
//! 4. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation of
//!    policy documents against a request, so this service and every other one handed the same
//!    documents decide alike. How the documents combine — an attachment whose document this
//!    account does not hold grants something unreadable, so it withholds a refusal without
//!    overturning an explicit `Deny`, and the boundary caps rather than grants — is read the
//!    way IAM's own authorizer reads it.
//!
//! This reading is a service's own because [`::service_common`] cannot hold it: the IAM
//! store's type lives in the IAM service's crate, which depends on `service_common` itself.
//! Only the DECISION is shared, which is the part two services must not answer differently.
//!
//! Nothing is barred on evidence this emulator does not have. Each of these shows nothing
//! about the role and lets the action go ahead: an engine carrying no IAM store, a role ARN
//! the store holds no role for (it may belong to another account), and a role carrying an
//! attachment whose document this account does not hold — an AWS-managed policy might be the
//! very one that carries the grant.

use ::service_common::iam_authorization::{self as iam, Decision};

/// The IAM store a rule action's `roleArn` is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so an action is judged against the very records IAM's handlers
/// wrote rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine. `None` is a deployment with no IAM to ask, which
/// is evidence of nothing.
pub fn iam_state<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// Whether the role `role_arn` BARS the rules engine from performing `action` on `resource`.
///
/// The trust document first: a role this service cannot assume never reaches its permissions.
/// Then the documents in force, capped by the permissions boundary — "a permissions boundary
/// sets the maximum permissions that an identity-based policy can grant to an entity", so the
/// boundary caps what those policies grant and grants nothing itself, and a role carrying none
/// is capped by nothing.
pub fn role_bars_action(
    iam_store: &IamState,
    role_arn: &str,
    service_principal: &str,
    action: &str,
    resource: &str,
) -> bool {
    // A value this account's IAM holds no role under says nothing about what that role may do.
    let Some(role) = iam_store.roles.values().find(|role| role.arn == role_arn) else {
        return false;
    };
    if role
        .assume_role_policy_document
        .as_deref()
        .and_then(|document| {
            ::service_common::iam_trust_policy::admits_service_principal(
                document,
                service_principal,
            )
        })
        == Some(false)
    {
        return true;
    }
    let Some(policies) = iam_store.principal_policies(role_arn) else {
        return false;
    };
    let documents = iam::prepare_documents(&policies.documents);
    let boundary = iam_store
        .permissions_boundary_document(role_arn)
        .map(|document| iam::prepare_documents(::std::slice::from_ref(&document)));
    let request = iam::Request::new(action, resource, &[]);
    let stated = iam::identity_decision_prepared(&documents, &request);
    // An attachment whose document this account does not hold is a grant this emulator cannot
    // read, so it cannot be the basis of a refusal — but it cannot overturn one either: an
    // explicit `Deny` in a document that IS held wins over every grant, including one whose
    // content is unknown here.
    let granted = stated == Decision::Allowed
        || (stated != Decision::Denied && !policies.uncarried.is_empty());
    // A call an identity-based policy explicitly DENIES is already barred on that deny, so no
    // ceiling is read for it.
    let capped = if stated == Decision::Denied {
        Decision::Allowed
    } else {
        boundary.as_ref().map_or(Decision::Allowed, |boundary| {
            iam::identity_decision_prepared(boundary, &request)
        })
    };
    !granted || capped != Decision::Allowed
}
