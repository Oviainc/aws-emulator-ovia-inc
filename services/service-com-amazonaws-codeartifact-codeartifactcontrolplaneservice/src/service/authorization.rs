// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the caller perform a CodeArtifact action — the decision every
//! control-plane operation that addresses a REPOSITORY takes before it answers.
//!
//! A CodeArtifact API call is authorized, not merely routed: a principal may perform
//! `codeartifact:<OperationName>` on a repository only when some policy in force allows it —
//! the CALLER'S OWN identity-based policies, or that repository's own permissions policy
//! naming the caller as `Principal` — and an explicit `Deny` in either refuses whatever the
//! other says. A principal allowed by NEITHER is refused with the operation's modelled
//! `AccessDeniedException` (HTTP 403, `smithy_query(query="operation",
//! operation="GetRepositoryEndpoint")`), stated in IAM's own words. That is one rule for the
//! whole service, so it lives here once: two operations handed the same documents must not
//! answer differently.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS minted
//!    that key for ([`caller_principal`], the same place `sts:GetCallerIdentity` answers
//!    from, so the principal a refusal blames is the one the caller reads back for the
//!    credentials it signed with);
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal, read through the IAM store's own records, so a policy `iam:PutRolePolicy`
//!    wrote takes effect on the next request and deleting it lifts it again; and the
//!    repository's own permissions policy, read out of THIS service's store, so a document
//!    `PutRepositoryPermissionsPolicy` wrote is an authorization input rather than an inert
//!    record;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource, principal)`, so this service and every
//!    other one handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split every
//! other gate in this tree makes (AWS Key Management Service, AWS Certificate Manager,
//! Amazon OpenSearch Service), and the lookups stay in the service crate for the reason
//! those keep theirs there: hoisting them into `service-common` would make every service
//! that depends on that crate link the IAM and STS service crates.
//!
//! ## What a statement has to name to grant
//!
//! An assumed-role session's permissions are the ROLE's, so a repository policy naming the
//! role grants its sessions: the spellings a statement may name the caller ITSELF by are the
//! session ARN STS reports and the ARN of the role behind it ([`Caller::principals`]).
//! Naming the whole ACCOUNT is not naming the principal — it delegates to that account's own
//! identity policies — so those spellings decide a DENY only, which is the split the shared
//! evaluator publishes ([`iam::account_principals`]).
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential**, and any long-term key: STS holds no session
//!   for it, so there is no role whose identity policies could be read, which is the position
//!   AWS account root credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.

use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// This service's IAM action prefix: an action is `codeartifact:<OperationName>` ("Actions
/// defined by AWS CodeArtifact", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` (`smithy_query(query="service_info")`) — the same string
/// [`crate::state::SERVICE_SIGNING_NAME`] spells into every ARN this service builds.
pub const ACTION_PREFIX: &str = crate::state::SERVICE_SIGNING_NAME;

/// Authorize `operation` on the repository the request addressed.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see the
/// module docs); the modelled refusal otherwise. Call it at the TOP of a handler, before this
/// service's store is locked and before the addressed resources are resolved, and with the
/// names the REQUEST carries rather than ones read back from a stored record: an
/// authorization decision does not depend on whether the repository exists, which is also
/// what keeps a refused call from telling an unauthorized caller whether it does.
pub fn authorize_repository_action<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    domain: &str,
    domain_owner: Option<&str>,
    repository: &str,
) -> Result<(), AccessDeniedException> {
    let Some(caller) = caller(engine, ctx) else {
        return Ok(());
    };
    let region = crate::state::request_region(ctx.parts());
    let resource = crate::state::repository_arn(&region, domain, repository);
    let action = format!("{ACTION_PREFIX}:{operation}");
    let policy = repository_policy(engine, domain, domain_owner, repository);
    decide(&caller, &action, &resource, policy.as_deref())
}

/// The decision itself, over documents already collected: the identity-based policies in
/// force on the caller and the addressed repository's own permissions policy (`None` for a
/// repository carrying none, or one this service does not hold).
fn decide(
    caller: &Caller,
    action: &str,
    resource: &str,
    policy: Option<&str>,
) -> Result<(), AccessDeniedException> {
    // Identity-based documents state no `Principal` element, so the principal spellings a
    // resource-based policy is matched against are not part of that half of the decision.
    let identity_request = iam::Request::new(action, resource, &[]);
    let identity = iam::identity_decision(&caller.documents, &identity_request);
    if identity == Decision::Denied {
        return Err(not_authorized(&caller.arn, action, resource, true));
    }

    let resource_said = match policy {
        None => Decision::NotStated,
        Some(document) => {
            // A `Deny` is decided against the caller's own spellings and its account's
            // together, because denying an account denies each of its principals; a GRANT is
            // decided against the caller's own alone (see the module docs).
            let mut denying = caller.principals.clone();
            denying.extend(iam::account_principals(&caller.arn));
            let denied = iam::resource_decision(
                document,
                &iam::Request::new(action, resource, &denying),
            ) == Decision::Denied;
            if denied {
                return Err(denied_by_resource_policy(&caller.arn, action, resource));
            }
            iam::resource_decision(
                document,
                &iam::Request::new(action, resource, &caller.principals),
            )
        }
    };

    if iam::is_allowed(identity, resource_said) {
        return Ok(());
    }
    // Nothing the stores hold grants the action — but a grant this emulator cannot READ may:
    // see the module docs.
    if caller.unreadable_attachment {
        return Ok(());
    }
    Err(not_authorized(&caller.arn, action, resource, false))
}

/// The repository permissions policy document in force on the addressed repository, or
/// `None` when this service holds no such repository or it carries no policy.
///
/// Read under its own short-lived guard, so no handler's state guard is held across this
/// gate's STS and IAM reads.
fn repository_policy<Eng: Send + Sync + 'static>(
    engine: &Eng,
    domain: &str,
    domain_owner: Option<&str>,
    repository: &str,
) -> Option<String> {
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    store
        .domain(domain, domain_owner)?
        .repositories
        .get(repository)?
        .policy
        .as_ref()?
        .document
        .clone()
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and why. Every value in it comes from the request and from the stored
/// policies: the principal from the credential the request presented, the action from the
/// operation being refused, the resource from the ARN the request addressed.
///
/// The resource clause is part of the sentence for EVERY refusal: an authorization decision
/// is always reached against some resource, and the clause is what tells an operator (and any
/// tooling parsing the message) which one the deny applies to. `explicit_deny` states the
/// clause IAM appends when a readable identity-based document DENIES the action rather than
/// merely not allowing it.
pub(crate) fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    explicit_deny: bool,
) -> AccessDeniedException {
    let because = if explicit_deny {
        " with an explicit deny in an identity-based policy".to_string()
    } else {
        format!(" because no identity-based policy allows the {action} action")
    };
    refusal(principal, action, resource, &because)
}

/// The refusal for an action a READABLE resource-based document denies: the same sentence,
/// naming the document class that denied it, because that is the document an operator has to
/// change.
fn denied_by_resource_policy(
    principal: &str,
    action: &str,
    resource: &str,
) -> AccessDeniedException {
    refusal(
        principal,
        action,
        resource,
        " with an explicit deny in a resource-based policy",
    )
}

/// IAM's refusal sentence: the principal, the action, the resource the action was judged
/// against, and the reason clause.
fn refusal(
    principal: &str,
    action: &str,
    resource: &str,
    because: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "User: {principal} is not authorized to perform: {action} on resource: \
             {resource}{because}"
        ),
    }
}

/// The principal this request speaks for, as `sts:GetCallerIdentity` reports it for the
/// credentials it was signed with.
///
/// The same resolution the authorization decision uses, exposed because the credential a mint
/// hands out carries its principal: a repository endpoint the token is later presented to has
/// to decide what THAT principal may do there. A request this emulator holds no session for
/// speaks for the account's own published identity, which is what STS answers for a long-term
/// key.
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
pub(crate) struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub(crate) arn: String,
    /// The spellings a resource policy statement names this principal ITSELF by: the session
    /// ARN STS reports, and the ARN of the IAM role behind that session. A statement naming
    /// one of them grants to the caller outright.
    pub(crate) principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub(crate) documents: Vec<String>,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    pub(crate) unreadable_attachment: bool,
}

/// The principal this request speaks for, with the identity-based policies in force on it,
/// or `None` for a caller this gate does not bound (see the module docs).
pub(crate) fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<Caller> {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

    let sts = sts_store::service_state(engine)?;
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // Resolving the caller goes through STS's own `resolve_caller`, which first materialises
    // the session expiries the clock has already made due; that write is owed at the same
    // instant whether or not anybody called.
    let identity = sts_store::resolve_caller(
        &mut sts.lock(),
        &access_key_id,
        ::std::time::SystemTime::now(),
        ctx.credential_store(),
    )
    // An expired session credential is refused by the layer that verifies the request, not
    // here.
    .ok()?;
    // An assumed-role session's permissions are the ROLE's. Every other principal form — the
    // account's own published credential, a long-term key — is a caller this gate does not
    // bound.
    let role_name = assumed_role_name(&identity.arn)?;
    let iam = iam_store::service_state(engine)?;
    let store = iam.lock();
    // The role is addressed by the ARN the account holds it under — which carries its PATH,
    // where the session ARN does not — and a role the account does not hold is a caller this
    // gate does not bound.
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
        principals: vec![identity.arn.clone(), role_arn],
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
pub(crate) fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn.splitn(6, ':').nth(5)?;
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A caller carrying `documents`, named by its session and by the role behind it.
    fn session(role: &str, documents: &[&str], unreadable_attachment: bool) -> Caller {
        let account = "000000000000";
        Caller {
            arn: format!("arn:aws:sts::{account}:assumed-role/{role}/session-1"),
            principals: vec![
                format!("arn:aws:sts::{account}:assumed-role/{role}/session-1"),
                format!("arn:aws:iam::{account}:role/{role}"),
            ],
            documents: documents.iter().map(|text| (*text).to_string()).collect(),
            unreadable_attachment,
        }
    }

    /// RULE: either grant suffices and a single deny refuses — the decision is made from the
    /// caller's identity policies and the repository's own policy together.
    #[test]
    fn either_policy_may_grant_and_either_may_deny() {
        let action = "codeartifact:GetRepositoryEndpoint";
        let resource = "arn:aws:codeartifact:us-east-1:000000000000:repository/acme/releases";
        let identity_allow =
            r#"{"Statement":[{"Effect":"Allow","Action":"codeartifact:*","Resource":"*"}]}"#;
        let identity_deny = r#"{"Statement":[{"Effect":"Deny","Action":"codeartifact:GetRepositoryEndpoint","Resource":"*"}]}"#;
        let names_the_role = r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:role/granted"},"Action":["codeartifact:GetRepositoryEndpoint"],"Resource":"*"}]}"#;
        let denies_the_role = r#"{"Statement":[{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::000000000000:role/granted"},"Action":"codeartifact:*","Resource":"*"}]}"#;

        // (case, role, identity documents, the repository's policy, allowed)
        type Row<'a> = (&'a str, &'a str, &'a [&'a str], Option<&'a str>, bool);
        let rows: &[Row<'_>] = &[
            ("no policy anywhere", "other", &[], None, false),
            (
                "the identity policy alone allows it",
                "other",
                &[identity_allow],
                None,
                true,
            ),
            (
                "the repository policy alone names the role",
                "granted",
                &[],
                Some(names_the_role),
                true,
            ),
            (
                "the repository policy names another role",
                "other",
                &[],
                Some(names_the_role),
                false,
            ),
            (
                "the repository policy denies a role its identity policy allows",
                "granted",
                &[identity_allow],
                Some(denies_the_role),
                false,
            ),
            (
                "the identity policy denies a role the repository policy allows",
                "granted",
                &[identity_deny],
                Some(names_the_role),
                false,
            ),
        ];
        for (case, role, documents, policy, allowed) in rows {
            let decided = decide(
                &session(role, documents, false),
                action,
                resource,
                *policy,
            );
            assert_eq!(
                decided.is_ok(),
                *allowed,
                "case {case:?}: expected allowed={allowed}, got {decided:?}"
            );
        }
    }

    /// RULE: a principal carrying an attachment this emulator holds no document for is not
    /// refused on evidence it does not have — but a readable deny still refuses it.
    #[test]
    fn an_unreadable_attachment_is_not_refused_but_a_readable_deny_still_refuses() {
        let action = "codeartifact:DescribeRepository";
        let resource = "arn:aws:codeartifact:us-west-2:000000000000:repository/acme/releases";
        let allowed = decide(&session("other", &[], true), action, resource, None);
        assert!(
            allowed.is_ok(),
            "case \"an unreadable attachment\": expected the call to be served, got {allowed:?}"
        );
        let denied = decide(
            &session("other", &[], true),
            action,
            resource,
            Some(r#"{"Statement":[{"Effect":"Deny","Principal":"*","Action":"codeartifact:*","Resource":"*"}]}"#),
        );
        assert!(
            denied.is_err(),
            "case \"a resource policy that denies everyone\": expected a refusal, got {denied:?}"
        );
    }

    /// RULE: a session ARN names the role whose policies are read; every other principal form
    /// names none, and is a caller this gate does not bound.
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

    /// RULE: the refusal names the principal, the action and the resource the action was
    /// judged against, with the clause naming WHY — nothing allowed it, or a readable document
    /// of that class denied it.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = "arn:aws:sts::000000000000:assumed-role/ca-publisher/session-1";
        let action = "codeartifact:GetRepositoryEndpoint";
        let repository = "arn:aws:codeartifact:us-east-1:000000000000:repository/acme/releases";
        // (case, the refusal, the whole sentence expected)
        let rows: &[(&str, AccessDeniedException, String)] = &[
            (
                "allowed by nothing",
                not_authorized(principal, action, repository, false),
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {repository} because no identity-based policy allows the {action} action"
                ),
            ),
            (
                "denied by an identity-based policy",
                not_authorized(principal, action, repository, true),
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {repository} with an explicit deny in an identity-based policy"
                ),
            ),
            (
                "denied by the repository's own policy",
                denied_by_resource_policy(principal, action, repository),
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {repository} with an explicit deny in a resource-based policy"
                ),
            ),
        ];
        for (case, refusal, expected) in rows {
            assert_eq!(
                &refusal.message, expected,
                "case {case:?}: expected IAM's wording, got {:?}",
                refusal.message
            );
        }
    }
}
