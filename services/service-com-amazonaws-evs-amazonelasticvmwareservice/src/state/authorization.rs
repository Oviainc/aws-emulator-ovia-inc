// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step every implemented operation of this service runs before it looks
//! at the resource the request names.
//!
//! AWS evaluates authorization at the IAM layer, BEFORE the service evaluates the
//! resource, so a caller whose identity policy denies an action is told it is not
//! authorized rather than told whether the resource exists. Amazon EVS declares no
//! `AccessDeniedException` of its own — the refusal is the AWS-common one — so it is
//! carried by [`crate::scaffolded::Scaffolded::access_denied`], which renders the
//! common error code at this service's wire status.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from (`sts_store::resolve_caller`), so the
//!    principal a refusal blames is the one the caller reads back from STS for the
//!    credentials it signed with;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal (a role's inline policies and the default versions of the managed
//!    policies attached to it, a user's inline policies), read out of the IAM service's
//!    OWN store so this service sees exactly what IAM's handlers wrote;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split
//! CloudWatch RUM's and AWS Transfer Family's gates make, and the lookups stay in the
//! service crate for the reason those two keep theirs there: hoisting them into
//! `service-common` would make every service that depends on that crate link the IAM and
//! STS service crates.
//!
//! ## What is refused, and what is not
//!
//! An EXPLICIT `Deny` about the action is the one IAM decision this emulator can make
//! soundly, and it is final whatever else the account holds, so it is what refuses here.
//! An ABSENT `Allow` is NOT evidence that the caller lacks the permission: the rest of a
//! principal's permission set (group policies, permissions boundaries, service control
//! policies, session policies) is not modelled by the IAM emulator, so refusing on its
//! absence would deny calls AWS permits. This is the reading AWS Transfer Family's gate
//! takes of the same store.
//!
//! A request signed with a credential the IAM and STS stores know nothing about — the
//! emulator's own published credential, which is the position AWS account root
//! credentials are in — carries no identity policy and so is never refused here.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `evs:<OperationName>` ("Actions
/// defined by Amazon Elastic VMware Service", IAM Service Authorization Reference),
/// which is also this service's `arnNamespace`
/// (`smithy_query(query="service_info")`).
pub const ACTION_PREFIX: &str = "evs";

/// The `Resource` a request that addresses NO nameable ARN is decided against.
///
/// A listing, and a create whose identifier this service mints, name no resource for a
/// statement's `Resource` element to be compared against. The empty value is what
/// expresses that in the shared evaluator's terms: IAM's wildcard matching lets only a
/// pattern covering every resource (`"*"`) match it, so a statement scoped to one
/// environment cannot decide such a request, while one that states its resources as
/// `NotResource` still covers it.
const NO_RESOURCE: &str = "";

/// The principal a request speaks for, and the identity-based documents in force on it.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal
    /// names. Empty when the request presents a credential no store knows.
    pub arn: String,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
}

/// The refusal text IAM states an explicit-deny decision with: who was refused, which
/// action, on which resource, and that the refusal came from an explicit deny in an
/// identity-based policy — the documented wording of that decision. Every value in it
/// comes from the request and the stored policy.
fn not_authorized(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         with an explicit deny in an identity-based policy"
    )
}

/// `Some(message)` when the calling principal's identity policies explicitly deny
/// `evs:<operation>` on `resource` — the message the refusal carries — and `None` when
/// no readable policy denies it.
///
/// `resource` is the ARN the request addresses, or `None` for a request that addresses
/// no nameable resource (see [`NO_RESOURCE`]). Pass the ARN the request NAMES, computed
/// before the resource is looked up, so the decision does not depend on whether the
/// resource exists.
pub fn explicit_deny<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: Option<&str>,
) -> Option<String> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    if caller.documents.is_empty() {
        return None;
    }
    let resource = resource.unwrap_or(NO_RESOURCE);
    let request = iam::Request::new(&action, resource, &[]);
    (iam::identity_decision(&caller.documents, &request) == Decision::Denied)
        .then(|| not_authorized(&caller.arn, &action, resource))
}

/// The refusal an operation returns for a request IAM explicitly denies, or `None` when
/// it is not denied: the one call an implemented handler makes.
///
/// It is generic over the operation's own modelled error enum because the refusal is
/// NOT one of them — Amazon EVS models no `AccessDeniedException` — so it is returned
/// through the error channel's own variant.
pub fn denied<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: Option<&str>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    explicit_deny(engine, ctx, operation, resource)
        .map(crate::scaffolded::Scaffolded::access_denied)
}

/// The principal this request speaks for, with the identity-based policies in force on
/// it.
///
/// The caller is resolved through STS's own `resolve_caller`, which materialises the
/// session expiries the clock has already made due before it answers: that write is owed
/// at the same instant whether or not anybody called, which is why a REPORTING operation
/// of this service may make this decision.
pub fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which holds no identity policy.
        None => Ok(sts_store::ambient_identity()),
    };
    // An expired session credential speaks for nobody: it carries no documents, so
    // nothing here refuses it (the framework's own authentication is what rejects it).
    let Ok(identity) = identity else {
        return Caller::default();
    };
    let mut caller = Caller {
        arn: identity.arn,
        documents: Vec::new(),
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        let role_key = iam_store::name_key(role_name);
        caller
            .documents
            .extend(inline_documents(&store.role_inline_policies, &role_key));
        for attached in store
            .role_attached_policies
            .get(&role_key)
            .into_iter()
            .flatten()
        {
            // A policy the account does not hold a document for — an AWS-managed one,
            // which AWS owns — carries no statement this gate can read. It is evidence
            // of nothing, and since only an explicit deny refuses here, nothing follows
            // from it either way.
            if let Some(document) = store
                .policy_by_arn(attached)
                .and_then(default_version_document)
            {
                caller.documents.push(document);
            }
        }
    } else if let Some(user_name) = iam_user_name(&caller.arn) {
        caller.documents.extend(inline_documents(
            &store.user_inline_policies,
            &iam_store::name_key(user_name),
        ));
    }
    caller
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role
/// is the first segment after `assumed-role/`. `None` for any other ARN form.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "sts")?;
    let tail = resource.strip_prefix("assumed-role/")?;
    let name = tail.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// The user name an IAM user ARN names: the LAST segment of `user/<path…>/<name>`,
/// since the path is optional and only the last segment is the name.
fn iam_user_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "iam")?;
    let name = resource.strip_prefix("user/")?.split('/').next_back()?;
    (!name.is_empty()).then_some(name)
}

/// The resource part of an ARN whose service segment is `service`.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, and the resource
/// may itself carry colons, so only the first five fields are split off.
fn arn_resource<'a>(arn: &'a str, service: &str) -> Option<&'a str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(named), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    (named == service).then_some(resource)
}

/// The inline policy documents one entity holds, from the IAM store's `<entity> ->
/// <policy name> -> policy` map.
fn inline_documents(
    policies: &::std::collections::BTreeMap<
        String,
        ::std::collections::BTreeMap<String, iam_store::InlinePolicy>,
    >,
    entity_key: &str,
) -> Vec<String> {
    policies
        .get(entity_key)
        .map(|held| {
            held.values()
                .map(|policy| policy.policy_document.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The document of a managed policy's DEFAULT version — the one in force on every entity
/// it is attached to. `None` when the store holds no document for it.
fn default_version_document(managed: &iam_store::ManagedPolicy) -> Option<String> {
    let default = managed.policy.default_version_id.as_deref();
    managed
        .versions
        .values()
        .find(|version| default.is_some() && version.version_id.as_deref() == default)
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

    /// Whether one stored document explicitly denies `action` on `resource`, through the
    /// shared evaluator this gate consults.
    fn denies(document: &str, action: &str, resource: &str) -> bool {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        ) == Decision::Denied
    }

    const DENY_EVS: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Deny", "Action": "evs:*", "Resource": "*"}]
    }"#;

    #[test]
    fn a_wildcard_deny_of_the_service_covers_every_action_of_it() {
        for operation in ["GetEnvironment", "CreateEnvironment", "ListEnvironments"] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            assert!(
                denies(DENY_EVS, &action, NO_RESOURCE),
                "case {operation:?}: expected evs:* to deny {action}, got no deny"
            );
        }
        assert!(
            !denies(DENY_EVS, "ec2:DescribeVpcs", NO_RESOURCE),
            "case \"another service\": expected evs:* to deny nothing outside evs"
        );
    }

    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let message = not_authorized(
            "arn:aws:sts::000000000000:assumed-role/denied/session",
            "evs:GetEnvironment",
            "arn:aws:evs:us-west-2:000000000000:environment/env-0123456789",
        );
        for part in [
            "assumed-role/denied/session",
            "evs:GetEnvironment",
            "environment/env-0123456789",
            "with an explicit deny in an identity-based policy",
        ] {
            assert!(
                message.contains(part),
                "case {part:?}: expected the refusal to carry it, got {message:?}"
            );
        }
    }

    #[test]
    fn a_session_arn_names_its_role_and_a_user_arn_its_user() {
        // (case, ARN, the role it names, the user it names)
        let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/evs-role/session-1",
                Some("evs-role"),
                None,
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/evs-user",
                None,
                Some("evs-user"),
            ),
            (
                "a path'd IAM user",
                "arn:aws:iam::000000000000:user/team/evs-user",
                None,
                Some("evs-user"),
            ),
            (
                "the account's own identity",
                "arn:aws:iam::000000000000:root",
                None,
                None,
            ),
            ("a value that is no ARN", "AKIDTESTONLY", None, None),
        ];
        for (case, arn, role, user) in rows {
            assert_eq!(
                assumed_role_name(arn),
                *role,
                "case {case:?}: expected role {role:?}, got {:?}",
                assumed_role_name(arn)
            );
            assert_eq!(
                iam_user_name(arn),
                *user,
                "case {case:?}: expected user {user:?}, got {:?}",
                iam_user_name(arn)
            );
        }
    }
}
