// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step an index operation runs BEFORE it reads or writes the index the
//! request names.
//!
//! AWS evaluates authorization at the IAM layer, before the service looks at the
//! resource, so a caller whose identity policies do not grant the action is told it is
//! not authorized rather than told anything about the resource, and the call performs
//! nothing. Each of this service's index operations models `AccessDeniedException`
//! (HTTP 403), which is the refusal that decision is returned as.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from (`sts_store::resolve_caller`), so the
//!    principal a refusal blames is the one the caller reads back from STS for the
//!    credentials it signed with;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal: the role's inline policies and the default version of each managed
//!    policy attached to it, read out of the IAM service's OWN store, so a policy written
//!    by `iam:PutRolePolicy` takes effect on the next request and deleting it lifts it
//!    again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request
//!    falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split the
//! gates of the other services that enforce authorization make (AWS B2B Data Interchange,
//! Amazon EVS, CloudWatch RUM, AWS Transfer Family), and the lookups stay in the service
//! crate for the reason those keep theirs there: hoisting them into `service-common` would
//! make every service that depends on that crate link the IAM and STS service crates.
//!
//! ## Which callers this bounds
//!
//! A request is evaluated when its credentials name an assumed-role session, because the
//! role is the principal whose identity policies this emulator can read. These are served
//! rather than evaluated, since no identity policy here bounds them:
//!
//! * a request whose access key id names no session — the long-term key this emulator
//!   publishes at start, which is the account's own administrator
//!   (`sts_store::ambient_identity`), the position AWS account root credentials are in;
//! * a session that is not an assumed role (an `sts:GetSessionToken` or
//!   `sts:GetFederationToken` session): what bounds such a caller is the IAM user's own
//!   identity policies or the session policy its mint supplied, and this gate reads
//!   neither;
//! * an engine that carries no STS or IAM store, where no session and no role can exist;
//! * an expired session, which the layer that authenticates a request refuses, not this
//!   one;
//! * a role carrying a managed policy no document can be read for — an AWS-managed ARN
//!   outside the catalog the IAM emulator publishes — whose grant is evidence this
//!   emulator does not have: refusing on it would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// This service's IAM action prefix: an action is `es:<OperationName>` ("Actions defined
/// by Amazon OpenSearch Service", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` (`smithy_query(query="service_info")`) — the same string
/// [`super::ARN_NAMESPACE`] spells into every ARN it builds.
pub const ACTION_PREFIX: &str = super::ARN_NAMESPACE;

/// Authorize this request, as [`ACTION_PREFIX`] and `operation` name the action and
/// `resource` names the ARN the request addresses.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise. Call it BEFORE the store is read or
/// written, with the ARN the request NAMES rather than one resolved from a stored record,
/// so the decision does not depend on whether that resource exists — which is also what
/// keeps a refused call from telling an unauthorized caller whether it does.
///
/// The caller is resolved through STS's own `resolve_caller`, which first materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called, which is why a REPORTING operation of this service may
/// take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let Some(caller) = caller(engine, ctx) else {
        return Ok(());
    };
    let action = format!("{ACTION_PREFIX}:{operation}");
    // Identity-based documents name no principal, so the principal spellings a
    // resource-based policy would be matched against are not part of this decision.
    let request = iam::Request::new(&action, resource, &[]);
    match iam::identity_decision(&caller.documents, &request) {
        Decision::Allowed => Ok(()),
        // An explicit `Deny` in a document this emulator CAN read refuses the request
        // whatever else the role carries, which is why it is answered before the
        // unreadable attachment below.
        Decision::Denied => Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            "with an explicit deny in an identity-based policy",
        )),
        // Nothing the store holds grants the action — but a grant it cannot READ may.
        Decision::NotStated if caller.unreadable_attachment => Ok(()),
        Decision::NotStated => Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        )),
    }
}

/// The refusal IAM states an authorization failure with: who was refused, which action,
/// on which resource, and which of IAM's two rules refused it. Every value in it comes
/// from the request and from the stored policies.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(format!(
            "User: {principal} is not authorized to perform: {action} on resource: \
             {resource} {reason}"
        )),
    }
}

/// The principal a request speaks for, and the identity-based documents in force on it.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// Set when the principal carries a managed policy whose document cannot be read
    /// here, so what it grants is unknown. See the module docs.
    unreadable_attachment: bool,
}

/// The principal this request speaks for with the documents in force on it, or `None` for
/// a caller this gate does not bound (see the module docs).
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<Caller> {
    let sts = sts_store::service_state(engine)?;
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = sts_store::resolve_caller(
        &mut sts.lock(),
        &access_key_id,
        SystemTime::now(),
        ctx.credential_store(),
    )
    .ok()?;
    let role_name = assumed_role_name(&identity.arn)?.to_string();
    let iam = iam_store::service_state(engine)?;
    let mut caller = role_identity_policies(&iam.lock(), &role_name);
    caller.arn = identity.arn;
    Some(caller)
}

/// The role name an assumed-role session's ARN names
/// (`arn:<partition>:sts::<account>:assumed-role/<RoleName>/<SessionName>`), or `None` for
/// every other principal form.
fn assumed_role_name(identity_arn: &str) -> Option<&str> {
    let resource = identity_arn.splitn(6, ':').nth(5)?;
    let role_name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!role_name.is_empty()).then_some(role_name)
}

/// The identity policies in force on one role: its inline policies and the document of
/// each managed policy attached to it — the two kinds of identity policy IAM evaluates
/// for a role. A role the IAM store does not hold carries none, which IAM's default deny
/// refuses.
fn role_identity_policies(
    iam: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
    role_name: &str,
) -> Caller {
    let role_key = iam_store::name_key(role_name);
    let mut caller = Caller {
        arn: String::new(),
        documents: iam
            .role_inline_policies
            .get(&role_key)
            .into_iter()
            .flat_map(|held| held.values())
            .map(|policy| policy.policy_document.clone())
            .collect(),
        unreadable_attachment: false,
    };
    for arn in iam
        .role_attached_policies
        .get(&role_key)
        .into_iter()
        .flatten()
    {
        // A managed policy is either the ACCOUNT's, whose versions the store holds, or
        // one of AWS's own, whose document the IAM emulator publishes as a catalog entry.
        let document = iam
            .policy_by_arn(arn)
            .and_then(default_version_document)
            .or_else(|| {
                iam_store::aws_managed_policy(arn).map(|managed| managed.document.to_string())
            });
        match document {
            Some(document) => caller.documents.push(document),
            // An AWS-managed ARN the catalog does not carry is a grant AWS owns and this
            // emulator cannot read; an attachment naming a CUSTOMER-managed policy the
            // account does not hold grants nothing, because the account is the authority
            // on its own policies.
            None if iam_store::is_aws_managed_policy_arn(arn) => {
                caller.unreadable_attachment = true;
            }
            None => {}
        }
    }
    caller
}

/// The document of an account-managed policy's DEFAULT version — the one in force on
/// every entity it is attached to. `None` when the store holds no document for it.
fn default_version_document(managed: &iam_store::ManagedPolicy) -> Option<String> {
    managed
        .policy
        .default_version_id
        .as_deref()
        .and_then(|version_id| managed.versions.get(version_id))
        .or_else(|| {
            managed
                .versions
                .values()
                .find(|version| version.is_default_version)
        })
        .and_then(|version| version.document.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision one stored document reaches about an action on a resource, through
    /// the shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `es:<OperationName>`, so a policy scoped
    /// to this service's actions covers every operation of it and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"es:*","Resource":"*"}]}"#;
        for operation in ["UpdateIndex", "GetIndex", "CreateIndex", "DeleteIndex"] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            assert_eq!(
                decision(document, &action, "*"),
                Decision::Allowed,
                "case {operation:?}: expected es:* to allow {action}, got {:?}",
                decision(document, &action, "*")
            );
        }
        assert_eq!(
            decision(document, "s3:GetObject", "*"),
            Decision::NotStated,
            "case \"another service\": expected es:* to grant nothing outside this service"
        );
    }

    /// RULE: a session ARN names the role whose policies are read; every other principal
    /// form names none, so its request is not evaluated here.
    #[test]
    fn a_session_arn_names_the_role_its_policies_are_read_from() {
        // (case, the ARN, the role it names)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/search-reader/session-1",
                Some("search-reader"),
            ),
            (
                "a session name carrying a slash",
                "arn:aws:sts::000000000000:assumed-role/search-reader/one/two",
                Some("search-reader"),
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/search-user",
                None,
            ),
            (
                "the account's own identity",
                "arn:aws:iam::000000000000:root",
                None,
            ),
            ("a value that is no ARN", "AKIDTESTONLY", None),
        ];
        for (case, arn, role) in rows {
            assert_eq!(
                assumed_role_name(arn),
                *role,
                "case {case:?}: expected the role {role:?}, got {:?}",
                assumed_role_name(arn)
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource and which of IAM's
    /// rules refused — every part taken from the request and the stored policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let action = format!("{ACTION_PREFIX}:UpdateIndex");
        let refusal = not_authorized(
            "arn:aws:sts::000000000000:assumed-role/search-reader/session-1",
            &action,
            "arn:aws:es:us-east-1:000000000000:domain/search-target",
            &format!("because no identity-based policy allows the {action} action"),
        );
        let message = refusal.message.unwrap_or_default();
        for part in [
            "User: arn:aws:sts::000000000000:assumed-role/search-reader/session-1",
            " is not authorized to perform: es:UpdateIndex",
            " on resource: arn:aws:es:us-east-1:000000000000:domain/search-target",
            " because no identity-based policy allows the es:UpdateIndex action",
        ] {
            assert!(
                message.contains(part),
                "case {part:?}: expected the refusal to carry it, got {message:?}"
            );
        }
    }
}
