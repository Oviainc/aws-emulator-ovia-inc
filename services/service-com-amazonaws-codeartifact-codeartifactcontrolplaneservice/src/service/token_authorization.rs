// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the caller MINT a repository bearer credential — the authorization
//! step `GetAuthorizationToken` takes before anything is minted.
//!
//! Minting one of these credentials needs TWO permissions in the CALLER'S OWN identity
//! policies, not one: the operation's own action and a cross-service action belonging to
//! another service, both named by the operation's documentation ("This API requires the
//! codeartifact:GetAuthorizationToken and sts:GetServiceBearerToken permissions",
//! `smithy_query(query="doc",shape="GetAuthorizationToken")`). A principal holding one and
//! not the other is refused, with the operation's modelled `AccessDeniedException` (HTTP
//! 403).
//!
//! The cross-service action is an IDENTITY permission, and only an identity permission: a
//! grant of it in a resource policy this service holds (a domain's or a repository's)
//! belongs to a different document class and to a different service's action namespace, so
//! nothing here reads one. That is what this module gets right by construction — it is
//! handed the identity documents alone.
//!
//! WHO the caller is, WHAT its identity policies say and HOW the decision is made are the
//! service's one authorization gate ([`super::authorization`]); this module adds only what
//! is particular to a token mint — the SECOND, cross-service permission it requires — and
//! the callers that gate does not bound are the ones it documents.

use ::service_common::iam_authorization::{self as iam, Decision};

use super::authorization::{caller, not_authorized, Caller, ACTION_PREFIX};
use crate::ops::types::AccessDeniedException;

/// The CROSS-SERVICE action a token mint also requires, as the operation's documentation
/// names it: it belongs to STS's action namespace, not to this service's.
pub const SERVICE_BEARER_TOKEN_ACTION: &str = "sts:GetServiceBearerToken";

/// The resource a cross-service action that names no resource TYPE of its own is judged
/// against: the CALLER'S own identity, because what such an action asks to act on is the
/// credential presenting it (`sts:GetServiceBearerToken` mints a bearer credential for the
/// calling session, and the IAM Service Authorization Reference lists no resource type for
/// it). A policy that grants it for every resource still grants it, since `*` matches any
/// ARN.
fn judged_resource(caller: &Caller) -> &str {
    &caller.arn
}

/// Authorize a token mint for `domain_arn`, the domain the request named.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise. Call it BEFORE anything is minted and
/// before the store is read, and with the ARN the REQUEST names rather than one resolved
/// from a stored record, so the decision does not depend on whether that domain exists —
/// which is also what keeps a refused call from telling an unauthorized caller whether it
/// does.
pub fn authorize_token_mint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    domain_arn: &str,
) -> Result<(), AccessDeniedException> {
    let Some(caller) = caller(engine, ctx) else {
        return Ok(());
    };
    // Both permissions, each judged against the resource IAM judges it against: the
    // operation's own action against the domain the request named, and the cross-service
    // action against the caller's own identity ([`judged_resource`]).
    let required = [
        (format!("{ACTION_PREFIX}:{operation}"), domain_arn),
        (
            SERVICE_BEARER_TOKEN_ACTION.to_string(),
            judged_resource(&caller),
        ),
    ];
    for (action, resource) in required {
        // Identity-based documents state no `Principal` element, so the principal
        // spellings a resource-based policy would be matched against are not part of this
        // decision.
        let request = iam::Request::new(&action, resource, &[]);
        match iam::identity_decision(&caller.documents, &request) {
            // An explicit `Deny` in a document this emulator CAN read refuses the request
            // whatever else the principal carries, which is why it is answered before the
            // unreadable attachment below.
            Decision::Denied => {
                return Err(not_authorized(&caller.arn, &action, resource, true));
            }
            Decision::Allowed => {}
            // Nothing the store holds grants the action — but a grant it cannot READ may:
            // see the module docs.
            Decision::NotStated if caller.unreadable_grants => {}
            Decision::NotStated => {
                return Err(not_authorized(&caller.arn, &action, resource, false));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a mint requires BOTH the operation's own action and the cross-service bearer
    /// action in the caller's identity policies — a document granting one and not the other
    /// grants no mint.
    #[test]
    fn both_permissions_are_asked_about_and_one_alone_is_not_a_grant() {
        let document = |actions: &str| {
            format!(r#"{{"Statement":[{{"Effect":"Allow","Action":{actions},"Resource":"*"}}]}}"#)
        };
        // (case, the document the role carries, whether the operation's own action is
        // allowed, whether the cross-service action is)
        let rows: &[(&str, String, bool, bool)] = &[
            (
                "both actions named",
                document(r#"["codeartifact:GetAuthorizationToken","sts:GetServiceBearerToken"]"#),
                true,
                true,
            ),
            (
                "only this service's action",
                document(r#""codeartifact:GetAuthorizationToken""#),
                true,
                false,
            ),
            (
                "only the cross-service action",
                document(r#""sts:GetServiceBearerToken""#),
                false,
                true,
            ),
            ("every action of every service", document(r#""*""#), true, true),
            (
                "every action of this service only",
                document(r#""codeartifact:*""#),
                true,
                false,
            ),
        ];
        for (case, policy, own, bearer) in rows {
            let documents = [policy.clone()];
            let decide = |action: &str, resource: &str| {
                iam::identity_decision(&documents, &iam::Request::new(action, resource, &[]))
                    == Decision::Allowed
            };
            let own_reached = decide(
                &format!("{ACTION_PREFIX}:GetAuthorizationToken"),
                "arn:aws:codeartifact:us-east-1:000000000000:domain/acme",
            );
            let bearer_reached = decide(
                SERVICE_BEARER_TOKEN_ACTION,
                "arn:aws:sts::000000000000:assumed-role/ca-publisher/session-1",
            );
            assert_eq!(
                own_reached, *own,
                "case {case:?}: expected the operation's own action allowed={own}, got \
                 {own_reached}"
            );
            assert_eq!(
                bearer_reached, *bearer,
                "case {case:?}: expected the cross-service action allowed={bearer}, got \
                 {bearer_reached}"
            );
        }
    }

    /// RULE: the refusal names the principal, the action and the resource the action was
    /// judged against — for the cross-service action that names no resource type of its own,
    /// the caller's own identity — with the explicit-deny clause only for a document that
    /// denies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = "arn:aws:sts::000000000000:assumed-role/ca-publisher/session-1";
        let domain = "arn:aws:codeartifact:us-east-1:000000000000:domain/acme";
        // (case, the action, the resource it was judged against, whether a document denied,
        //  the whole sentence expected)
        let rows: &[(&str, &str, &str, bool, String)] = &[
            (
                "the operation's own action, allowed by nothing",
                "codeartifact:GetAuthorizationToken",
                domain,
                false,
                format!(
                    "User: {principal} is not authorized to perform: \
                     codeartifact:GetAuthorizationToken on resource: {domain} because no \
                     identity-based policy allows the codeartifact:GetAuthorizationToken action"
                ),
            ),
            (
                "the cross-service action, allowed by nothing",
                SERVICE_BEARER_TOKEN_ACTION,
                principal,
                false,
                format!(
                    "User: {principal} is not authorized to perform: \
                     {SERVICE_BEARER_TOKEN_ACTION} on resource: {principal} because no \
                     identity-based policy allows the {SERVICE_BEARER_TOKEN_ACTION} action"
                ),
            ),
            (
                "the cross-service action, explicitly denied",
                SERVICE_BEARER_TOKEN_ACTION,
                principal,
                true,
                format!(
                    "User: {principal} is not authorized to perform: \
                     {SERVICE_BEARER_TOKEN_ACTION} on resource: {principal} with an explicit \
                     deny in an identity-based policy"
                ),
            ),
        ];
        for (case, action, resource, explicit_deny, expected) in rows {
            let refusal = not_authorized(principal, action, resource, *explicit_deny);
            assert_eq!(
                &refusal.message, expected,
                "case {case:?}: expected IAM's wording, got {:?}",
                refusal.message
            );
        }
    }
}
