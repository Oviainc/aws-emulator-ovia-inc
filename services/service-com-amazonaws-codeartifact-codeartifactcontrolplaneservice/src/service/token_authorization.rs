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
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with);
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal, read through the IAM store's own records, so a policy
//!    `iam:PutRolePolicy` wrote takes effect on the next request and deleting it lifts it
//!    again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split every
//! other gate in this tree makes (AWS Certificate Manager, AWS B2B Data Interchange,
//! Amazon OpenSearch Service, CloudWatch RUM), and the lookups stay in the service crate
//! for the reason those keep theirs there: hoisting them into `service-common` would make
//! every service that depends on that crate link the IAM and STS service crates.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential**, and any long-term key: STS holds no
//!   session for it, so there is no role whose identity policies could be read, which is
//!   the position AWS account root credentials are in. Such a request is allowed. (The
//!   identity-policy gate for every OTHER operation of this service, and for a USER
//!   principal, is a wider change with its own issue; this module decides the one operation
//!   whose documentation names a second, cross-service permission.)
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.

use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// This service's IAM action prefix: an action is `codeartifact:<OperationName>` ("Actions
/// defined by AWS CodeArtifact", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` (`smithy_query(query="service_info")`) — the same string
/// [`crate::state::SERVICE_SIGNING_NAME`] spells into every ARN this service builds.
pub const ACTION_PREFIX: &str = crate::state::SERVICE_SIGNING_NAME;

/// The CROSS-SERVICE action a token mint also requires, as the operation's documentation
/// names it: it belongs to STS's action namespace, not to this service's.
pub const SERVICE_BEARER_TOKEN_ACTION: &str = "sts:GetServiceBearerToken";

/// The resource an action that is not resource-scoped is judged against, as IAM writes it
/// for such a request. `sts:GetServiceBearerToken` names no resource of its own.
pub const ANY_RESOURCE: &str = "*";

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
    // action against the resource it has none of.
    let required = [
        (format!("{ACTION_PREFIX}:{operation}"), domain_arn),
        (SERVICE_BEARER_TOKEN_ACTION.to_string(), ANY_RESOURCE),
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
            Decision::NotStated if caller.unreadable_attachment => {}
            Decision::NotStated => {
                return Err(not_authorized(&caller.arn, &action, resource, false));
            }
        }
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action and
/// — for an action scoped to one — on which resource. Every value in it comes from the
/// request and from the stored policies: the principal from the credential the request
/// presented, the action from what the mint requires, the resource from the ARN it
/// addressed.
///
/// An action that addresses no resource of its own is judged against [`ANY_RESOURCE`], and
/// the sentence states no resource clause for it: there is nothing to name. `explicit_deny`
/// states the clause IAM appends when a readable document DENIES the action rather than
/// merely not allowing it.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    explicit_deny: bool,
) -> AccessDeniedException {
    let on_resource = if resource == ANY_RESOURCE {
        String::new()
    } else {
        format!(" on resource: {resource}")
    };
    let because = if explicit_deny {
        " with an explicit deny in an identity-based policy".to_string()
    } else {
        format!(" because no identity-based policy allows the {action} action")
    };
    AccessDeniedException {
        message: format!(
            "User: {principal} is not authorized to perform: {action}{on_resource}{because}"
        ),
    }
}

/// The principal this request speaks for, as `sts:GetCallerIdentity` reports it for the
/// credentials it was signed with.
///
/// The same resolution the authorization decision uses, exposed because the credential a
/// mint hands out carries its principal: a repository endpoint the token is later presented
/// to has to decide what THAT principal may do there. A request this emulator holds no
/// session for speaks for the account's own published identity, which is what STS answers
/// for a long-term key.
pub fn caller_principal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

    let Some(sts) = sts_store::service_state(engine) else {
        return sts_store::ambient_identity().arn;
    };
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let resolved = sts_store::resolve_caller(
        &mut sts.lock(),
        &access_key_id,
        ::std::time::SystemTime::now(),
        ctx.credential_store(),
    )
    .map(|identity| identity.arn)
    .unwrap_or_else(|_| sts_store::ambient_identity().arn);
    resolved
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

/// The principal this request speaks for, with the identity-based policies in force on it,
/// or `None` for a caller this gate does not bound (see the module docs).
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<Caller> {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

    let sts = sts_store::service_state(engine)?;
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // Resolving the caller goes through STS's own `resolve_caller`, which first
    // materialises the session expiries the clock has already made due; that write is owed
    // at the same instant whether or not anybody called.
    let identity = sts_store::resolve_caller(
        &mut sts.lock(),
        &access_key_id,
        ::std::time::SystemTime::now(),
        ctx.credential_store(),
    )
    // An expired session credential is refused by the layer that verifies the request, not
    // here.
    .ok()?;
    // An assumed-role session's permissions are the ROLE's. Every other principal form —
    // the account's own published credential, a long-term key — is a caller this gate does
    // not bound.
    let role_name = assumed_role_name(&identity.arn)?;
    let iam = iam_store::service_state(engine)?;
    let store = iam.lock();
    // The role is addressed by the ARN the account holds it under — which carries its
    // PATH, where the session ARN does not — and a role the account does not hold is a
    // caller this gate does not bound.
    let role_arn = store
        .roles
        .get(&iam_store::name_key(role_name))?
        .arn
        .clone();
    // `principal_policies` is where the collection rule itself lives — the role's inline
    // policies and the default version of each managed policy attached to it — so this
    // service reads an identity's permissions exactly as IAM's own authorizer does.
    let policies = store.principal_policies(&role_arn)?;
    Some(Caller {
        arn: identity.arn,
        documents: policies.documents,
        // An attachment the collection resolved no document for is an entry of AWS's own
        // catalog this emulator does not carry: a grant it cannot read.
        unreadable_attachment: !policies.uncarried.is_empty(),
    })
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role is
/// the first segment after `assumed-role/`.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn.splitn(6, ':').nth(5)?;
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!name.is_empty()).then_some(name)
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
            let bearer_reached = decide(SERVICE_BEARER_TOKEN_ACTION, ANY_RESOURCE);
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

    /// RULE: a session ARN names the role whose policies are read; every other principal
    /// form names none, and is a caller this gate does not bound.
    #[test]
    fn a_principal_arn_names_the_role_its_policies_are_read_from() {
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/ca-publisher/session-1",
                Some("ca-publisher"),
            ),
            (
                "a session name carrying a slash",
                "arn:aws:sts::000000000000:assumed-role/ca-publisher/one/two",
                Some("ca-publisher"),
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/ca-user",
                None,
            ),
            ("the account's own identity", "arn:aws:iam::000000000000:root", None),
            ("a value that is no ARN", "AKIDTESTONLY", None),
        ];
        for (case, arn, expected) in rows {
            let named = assumed_role_name(arn);
            assert_eq!(
                named, *expected,
                "case {case:?}: expected the role {expected:?}, got {named:?}"
            );
        }
    }

    /// RULE: the refusal names the principal and the action, with the resource clause only
    /// for an action scoped to a resource and the explicit-deny clause only for a document
    /// that denies.
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
                ANY_RESOURCE,
                false,
                format!(
                    "User: {principal} is not authorized to perform: \
                     {SERVICE_BEARER_TOKEN_ACTION} because no identity-based policy allows the \
                     {SERVICE_BEARER_TOKEN_ACTION} action"
                ),
            ),
            (
                "the cross-service action, explicitly denied",
                SERVICE_BEARER_TOKEN_ACTION,
                ANY_RESOURCE,
                true,
                format!(
                    "User: {principal} is not authorized to perform: \
                     {SERVICE_BEARER_TOKEN_ACTION} with an explicit deny in an identity-based \
                     policy"
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
