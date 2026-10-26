// SPDX-License-Identifier: CC0-1.0

//! The ONE resolution of an identity a resource-based policy GRANTS TO against this
//! account's IAM, for every service that resolves a policy document's principals before it
//! stores the document.
//!
//! A resource-based policy states WHO a permission is granted to, and a grant to an
//! identity that does not exist is a grant to nobody — so the services whose API takes such
//! a document resolve each identity it names rather than taking the text on trust. Which
//! IAM entity a value names is [`crate::iam_role`], the question is put on the wire by
//! [`crate::iam_client`], and whether the document is a policy at all is
//! [`crate::iam_policy`]: this module is only the one place those three are joined, so two
//! services asked about the same principal cannot answer differently.
//!
//! What a service then DOES with the answer — which error it words, and which of its
//! operations resolve a principal at all — stays the service's, because each one models its
//! own refusal.
//!
//! NOTHING here opens a socket or leaves the host: the question travels over the operation
//! callback's in-process transport, which is the same path every other service's lookup of a
//! caller-supplied IAM entity takes, and the number of lookups a document can cause is
//! bounded by the modelled length of the member that carries it.

/// Whether IAM ANSWERED that `principal` names an identity this account does not hold.
///
/// `principal` is one value of a statement's `Principal` element — the element a
/// resource-based policy states its GRANTEES in — in whatever form the policy language
/// writes it: the IAM entity ARNs are the two such a policy grants to by name, a role and a
/// user, and each is resolved under its own entity type
/// ([`crate::iam_role::role_name`], [`crate::iam_role::user_name`]).
///
/// `false` — do not refuse — for everything that is NOT such an answer, because a refusal
/// has to be grounded in what IAM said:
///
/// * a value that names no IAM role or user of this account: the `*` wildcard, an account id
///   or an account's root, a service principal, a federated or canonical-user principal, and
///   an entity of ANOTHER account, whose IAM this emulator is not the one to ask;
/// * an answer this emulator could not read, and any failure other than IAM reporting that
///   the entity is absent — including an emulator with no IAM mounted at all.
pub async fn identity_is_absent(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    principal: &str,
) -> bool {
    if let Some(role_name) = crate::iam_role::role_name(principal) {
        let answer = crate::iam_client::iam_client(ctx, region)
            .get_role()
            .role_name(role_name)
            .send()
            .await;
        return answer.err().is_some_and(|error| {
            error.as_service_error().is_some_and(
                ::aws_sdk_iam::operation::get_role::GetRoleError::is_no_such_entity_exception,
            )
        });
    }
    if let Some(user_name) = crate::iam_role::user_name(principal) {
        let answer = crate::iam_client::iam_client(ctx, region)
            .get_user()
            .user_name(user_name)
            .send()
            .await;
        return answer.err().is_some_and(|error| {
            error.as_service_error().is_some_and(
                ::aws_sdk_iam::operation::get_user::GetUserError::is_no_such_entity_exception,
            )
        });
    }
    false
}
