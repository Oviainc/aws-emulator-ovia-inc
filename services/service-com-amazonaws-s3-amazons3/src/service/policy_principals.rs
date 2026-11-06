// SPDX-License-Identifier: CC0-1.0

//! Whether every identity a bucket policy GRANTS TO is one this account's IAM can resolve —
//! the question `PutBucketPolicy` answers before it stores the document.
//!
//! A resource-based policy states WHO a permission is granted to, so S3 resolves the
//! identities a document names rather than taking the text on trust: "provide a valid
//! principal" is what it requires of the element, and a document naming one it cannot resolve
//! is refused `MalformedPolicy` with `Invalid principal in policy`, leaving the policy the
//! bucket already carried untouched. A grant to an identity that does not exist would
//! otherwise be stored as a grant to nobody, and the deployment that wrote it would look like
//! it had succeeded.
//!
//! Two kinds of value fail, and nothing else does:
//!
//! * an IAM USER or ROLE of this account that the account does not hold — the two IAM
//!   entities a resource policy grants to by name, each read under its own entity type
//!   ([`::service_common::iam_role::user_name`], [`::service_common::iam_role::role_name`]);
//! * an IAM GROUP, whatever the account holds under that name: "you cannot identify a user
//!   group as a `Principal` in a policy (such as a resource-based policy) because groups
//!   relate to permissions, not authentication" (IAM User Guide, *IAM identities*), so a group
//!   is never a principal to resolve — the grant has to name the group's members, or the
//!   permission has to be attached to the group as an identity-based policy instead.
//!
//! Everything else resolves: the `*` wildcard, an account id or an account's root, a service
//! principal, a federated or canonical-user principal, an assumed-role session, and an entity
//! of ANOTHER account, whose IAM this emulator is not the one to ask. So is every value when
//! the engine carries no IAM store at all: a refusal has to be grounded in what IAM holds.
//!
//! WHICH values a document names is the document reading
//! ([`crate::state::policy_principals`]); this module is only where those values meet the
//! account's IAM records.

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

/// The first principal of `policy` this account's IAM cannot resolve as a grantee, or `None`
/// when every value it names resolves (see the module docs).
///
/// The number of lookups one document can cause is bounded by the published size of a bucket
/// policy ([`crate::state::MAX_BUCKET_POLICY_BYTES`]), which the operation enforces before it
/// asks, and each is a read of the IAM store in this same engine — nothing opens a socket or
/// leaves the host.
#[must_use]
pub fn unresolvable_principal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    policy: &str,
) -> Option<String> {
    let named = crate::state::policy_principals(policy);
    if named.is_empty() {
        return None;
    }
    let iam = sts_store::iam_state(engine)?;
    let store = iam.lock();
    named.into_iter().find(|principal| {
        if ::service_common::iam_role::group_name(principal).is_some() {
            return true;
        }
        if ::service_common::iam_role::user_name(principal).is_some() {
            return !store.users.values().any(|user| &user.arn == principal);
        }
        if ::service_common::iam_role::role_name(principal).is_some() {
            return !store.roles.values().any(|role| &role.arn == principal);
        }
        false
    })
}
