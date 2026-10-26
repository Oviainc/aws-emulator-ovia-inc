// SPDX-License-Identifier: CC0-1.0

//! Who a request to this service speaks for, and whether IAM lets it do what it asks.
//!
//! Every operation of this service that the model requires a SigV4 signature for is a
//! developer-credentials call ("You must use Amazon Web Services developer credentials to
//! call this operation", this service's own documentation), and such a call is authorized
//! against the calling principal's identity-based policies BEFORE the operation reads or
//! writes anything — so a caller holding no grant is told it is not authorized rather than
//! told whether the pool it named exists, and the pool survives the refusal. The
//! operations the model marks `@optionalAuth` (`GetId`, `GetCredentialsForIdentity`,
//! `GetOpenIdToken`, `UnlinkIdentity` — `smithy_query(query="digest")`) are the
//! unauthenticated identity flow: a request that carries no credential at all speaks for
//! no principal, so they are not judged here.
//!
//! This service's model declares no `AccessDeniedException` of its own — the refusal is
//! the AWS-common one — so it travels through
//! [`crate::scaffolded::Scaffolded::access_denied`], which renders the common error code
//! at the status this refusal carries.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_caller`]) for a
//!    session credential, and from IAM's own `access_keys` map (the map `iam:GetUser`
//!    resolves a caller through) for a long-term key, so the principal a refusal blames is
//!    the one the caller reads back for the credentials it signed with;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal: its inline policies and the default versions of the managed policies
//!    attached to it, read out of the IAM service's OWN store so this service sees exactly
//!    what IAM's handlers wrote;
//! 3. **the decision** — [`::service_common::iam_authorization`], the shared evaluation of
//!    those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split
//! CloudWatch RUM's, AWS Transfer Family's and Data Exports' own gates make, and the
//! lookups stay in this service crate for the reason those keep theirs there: hoisting
//! them into `service-common` would make every service depending on that crate link the
//! IAM and STS service crates.
//!
//! ## The two callers the identity decision cannot refuse
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to [`sts_store::ambient_identity`], the account's
//!   own identity, which is not a principal the IAM store holds policies for — the same
//!   position AWS account root credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy is owned by AWS, not by the account, so the IAM store holds no document for it
//!   ([`iam_store::AWS_MANAGED_POLICY_ACCOUNT`]). A principal with such a policy attached
//!   is allowed: this service cannot see what that document grants, and refusing on
//!   evidence it does not have would deny access AWS permits. An attachment naming a
//!   CUSTOMER-managed policy the account does not hold grants nothing, because the account
//!   is the authority on its own policies.
//!
//! An expired session credential speaks for nobody: it carries no document and no
//! principal spelling, so every action it asks for is refused.
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which materialises the
//! session expiries the clock has already made due before it answers: that write is owed
//! at the same instant whether or not anybody called, which is why a REPORTING operation
//! of this service may make this decision.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `cognito-identity:<OperationName>`
/// ("Actions defined by Amazon Cognito Identity", IAM Service Authorization Reference),
/// which is also this service's `arnNamespace`
/// (`smithy_query(query="service_info")`: `arnNamespace: cognito-identity`) — the constant
/// this crate already holds as [`super::ARN_NAMESPACE`].
pub const ACTION_PREFIX: &str = super::ARN_NAMESPACE;

/// The IAM action `operation` is performed under: `cognito-identity:<OperationName>`.
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The resource an operation that names NO single identity pool is decided against and
/// refused about: every identity pool of this account in `region`, spelled with IAM's own
/// resource wildcard. A listing of the account's pools, and a create whose id this service
/// has not minted yet, address no one pool — so a statement scoped to one pool cannot
/// allow them, while one covering the account's pools can.
pub fn any_identity_pool_arn(region: &str) -> String {
    ::engine::emulator_arn(
        super::ARN_NAMESPACE,
        region,
        &format!("{}/*", super::IDENTITY_POOL_RESOURCE_TYPE),
    )
}

/// The principal a request speaks for, and the identity-based documents in force on it.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    pub account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    pub unreadable_attachment: bool,
}

impl Caller {
    /// Whether the identity-based decision cannot refuse this caller: the account's own
    /// credential, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }
}

/// The refusal AWS words an identity-based decision with: who was refused, which action,
/// on which resource, and why the grant was withheld — an explicit `Deny` in a policy in
/// force, or no policy allowing the action at all. Every value in it comes from the request
/// and the stored policies.
fn not_authorized(principal: &str, action: &str, resource: &str, explicit: bool) -> String {
    let reason = if explicit {
        "with an explicit deny in an identity-based policy".to_string()
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// `Some(message)` when the calling principal's identity-based policies do not allow
/// `cognito-identity:<operation>` on `resource` — the message the refusal carries — and
/// `None` when they do.
///
/// `resource` is the ARN the request addresses, computed before anything is looked up so
/// the decision does not depend on whether that resource exists, or `None` for a request
/// that addresses no single pool (see [`any_identity_pool_arn`]).
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: Option<&str>,
) -> Option<String> {
    let action = action_for(operation);
    let caller = caller(engine, ctx);
    let fallback = resource
        .is_none()
        .then(|| any_identity_pool_arn(&super::request_region(ctx.parts())));
    let resource = resource.or(fallback.as_deref()).unwrap_or_default();
    if caller.identity_is_unbounded() {
        return None;
    }
    let request = iam::Request::new(&action, resource, &[]);
    let said = iam::identity_decision(&caller.documents, &request);
    if iam::is_allowed(said, Decision::NotStated) {
        return None;
    }
    Some(not_authorized(
        &caller.arn,
        &action,
        resource,
        said == Decision::Denied,
    ))
}

/// The refusal an operation returns for a request IAM does not allow, or `None` when it is
/// allowed: the one call an implemented handler makes, placed before it reads or writes
/// this service's state.
///
/// Generic over the operation's own modelled error enum because the refusal is NOT one of
/// them — this service's model declares no `AccessDeniedException` — so it travels through
/// the error channel's own variant.
pub fn denied<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: Option<&str>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    refusal(engine, ctx, operation, resource).map(crate::scaffolded::Scaffolded::access_denied)
}

/// The principal this request speaks for, with the identity-based policies in force on it.
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
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody.
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                documents: Vec::new(),
                account_credential: false,
                unreadable_attachment: false,
            }
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        arn: identity.arn,
        documents: Vec::new(),
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map `iam:GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the
    // user's policies rather than served as the account. A key the IAM store does not hold
    // either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
        }
    }
    // The documents in force on the principal: its inline policies and the default
    // versions of the managed policies attached to it, whichever kind of principal it is.
    let (inline, attached) = if let Some(role_name) = assumed_role_name(&caller.arn) {
        let role_key = iam_store::name_key(role_name);
        (
            inline_documents(&store.role_inline_policies, &role_key),
            store.role_attached_policies.get(&role_key),
        )
    } else if let Some(user_name) = iam_user_name(&caller.arn) {
        let user_key = iam_store::name_key(user_name);
        (
            inline_documents(&store.user_inline_policies, &user_key),
            store.user_attached_policies.get(&user_key),
        )
    } else {
        (Vec::new(), None)
    };
    caller.documents.extend(inline);
    for attachment in attached.into_iter().flatten() {
        match store
            .policy_by_arn(attachment)
            .and_then(default_version_document)
        {
            Some(document) => caller.documents.push(document),
            // Only an AWS-MANAGED policy is a grant this emulator cannot read (see the
            // module docs).
            None if is_aws_managed_policy(attachment) => caller.unreadable_attachment = true,
            None => {}
        }
    }
    caller
}

/// Whether `arn` names an AWS-MANAGED policy: one whose account segment is IAM's own
/// `aws`, so it belongs to AWS rather than to this account and the IAM store holds no
/// document for it. Read through IAM's own ARN parser and constant, so this service and
/// IAM agree on what an AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam_store::parse_policy_arn(arn)
        .is_some_and(|policy| policy.account == iam_store::AWS_MANAGED_POLICY_ACCOUNT)
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role is
/// the first segment after `assumed-role/`. `None` for any other ARN form.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "sts")?;
    let tail = resource.strip_prefix("assumed-role/")?;
    let name = tail.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// The user name an IAM user ARN names: the LAST segment of `user/<path…>/<name>`, since
/// the path is optional and only the last segment is the name.
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

/// The inline policy documents one entity holds, from the IAM store's `<entity> -> <policy
/// name> -> policy` map.
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

/// The document of a managed policy's DEFAULT version — the one in force on every entity it
/// is attached to. `None` when the store holds no document for it.
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

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    const POOL: &str = "arn:aws:cognito-identity:us-east-1:000000000000:identitypool/us-east-1:00000000-0000-0000-0000-000000000000";

    #[test]
    fn a_principal_with_no_document_is_allowed_nothing() {
        for operation in ["DeleteIdentityPool", "DescribeIdentityPool", "TagResource"] {
            let action = action_for(operation);
            let said = iam::identity_decision(&[], &iam::Request::new(&action, POOL, &[]));
            assert_eq!(
                said,
                Decision::NotStated,
                "case {operation:?}: expected no grant from an empty document set, got {said:?}"
            );
            assert!(
                !iam::is_allowed(said, Decision::NotStated),
                "case {operation:?}: an unstated decision must not allow {action}"
            );
        }
    }

    #[test]
    fn a_grant_is_read_for_the_action_and_the_resource_it_names() {
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"cognito-identity:DeleteIdentityPool","Resource":"{POOL}"}}]}}"#
        );
        // (case, action, resource, allowed)
        let rows: &[(&str, &str, &str, bool)] = &[
            ("the granted action on the granted pool", "DeleteIdentityPool", POOL, true),
            ("another action on the same pool", "DescribeIdentityPool", POOL, false),
            (
                "the granted action on another pool",
                "DeleteIdentityPool",
                "arn:aws:cognito-identity:us-east-1:000000000000:identitypool/us-east-1:11111111-1111-1111-1111-111111111111",
                false,
            ),
            (
                "the granted action with no single pool named",
                "DeleteIdentityPool",
                &any_identity_pool_arn("us-east-1"),
                false,
            ),
        ];
        for (case, operation, resource, expected) in rows {
            let said = decision(&document, &action_for(operation), resource);
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected allowed == {expected}, got {actual} ({said:?})"
            );
        }
    }

    #[test]
    fn an_explicit_deny_wins_over_a_wildcard_grant() {
        let document = r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"cognito-identity:*","Resource":"*"},
            {"Effect":"Deny","Action":"cognito-identity:DeleteIdentityPool","Resource":"*"}]}"#;
        let denied = decision(document, &action_for("DeleteIdentityPool"), POOL);
        assert_eq!(
            denied,
            Decision::Denied,
            "case \"the denied action\": expected Denied, got {denied:?}"
        );
        let allowed = decision(document, &action_for("DescribeIdentityPool"), POOL);
        assert_eq!(
            allowed,
            Decision::Allowed,
            "case \"an action the deny does not name\": expected Allowed, got {allowed:?}"
        );
    }

    #[test]
    fn the_refusal_names_the_principal_the_action_the_resource_and_the_reason() {
        // (case, explicit deny, the clause the message must carry)
        let rows: &[(&str, bool, &str)] = &[
            (
                "no policy allows it",
                false,
                "because no identity-based policy allows the cognito-identity:DeleteIdentityPool \
                 action",
            ),
            (
                "a policy denies it",
                true,
                "with an explicit deny in an identity-based policy",
            ),
        ];
        for (case, explicit, reason) in rows {
            let message = not_authorized(
                "arn:aws:iam::000000000000:user/noperm-probe",
                &action_for("DeleteIdentityPool"),
                POOL,
                *explicit,
            );
            let expected = format!(
                "User: arn:aws:iam::000000000000:user/noperm-probe is not authorized to perform: \
                 cognito-identity:DeleteIdentityPool on resource: {POOL} {reason}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    #[test]
    fn a_session_arn_names_its_role_and_a_user_arn_its_user() {
        // (case, ARN, the role it names, the user it names)
        let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/pool-admin/session-1",
                Some("pool-admin"),
                None,
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/noperm-probe",
                None,
                Some("noperm-probe"),
            ),
            (
                "a path'd IAM user",
                "arn:aws:iam::000000000000:user/team/noperm-probe",
                None,
                Some("noperm-probe"),
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
