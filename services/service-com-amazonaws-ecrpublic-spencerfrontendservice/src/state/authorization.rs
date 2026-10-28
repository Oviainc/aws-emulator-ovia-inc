// SPDX-License-Identifier: CC0-1.0

//! Who a request to this service speaks for, and whether IAM lets it do what it asks.
//!
//! Every operation of this service is signed (`smithy_query(query="service_info")`:
//! `authSchemes: [sigv4]`, and the model marks none of the 23 `@optionalAuth`), so every
//! call is made by a principal — and a principal may only do what a policy in force on it
//! ALLOWS. There is no `AccessDeniedException` in this model for the same reason: the
//! decision is taken BEFORE the operation runs, so it is not one of the operation's own
//! outcomes, and the refusal is the AWS-common error (see
//! [`crate::scaffolded::Scaffolded::access_denied`]). Nothing is cached: a policy that was
//! just put grants immediately, and one that was just deleted stops granting immediately.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_caller`]) for a
//!    session credential and from IAM's own `access_keys` map (the map `iam:GetUser`
//!    resolves a caller through) for a long-term key, so the principal a refusal blames is
//!    the one the caller reads back for the credentials it signed with;
//! 2. **what it may do** — the identity-based documents the IAM store holds for that
//!    principal, collected by IAM's own
//!    [`principal_policies`](iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State::principal_policies)
//!    (its inline policies, the operative version of each managed policy attached to it,
//!    and for a user the policies of the groups it belongs to), so this service sees
//!    exactly what IAM's handlers wrote and reads an attachment the way IAM does;
//! 3. **the decision** — [`::service_common::iam_authorization`], the shared evaluation of
//!    those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split Cognito
//! Identity's, CloudWatch RUM's and Data Exports' own gates make, and the lookups stay in
//! this service crate for the reason those keep theirs there: hoisting them into
//! `service-common` would make every service depending on that crate link the IAM and STS
//! service crates.
//!
//! ## What a decision is made ABOUT
//!
//! An IAM action is taken on the resource its own action takes: the operations that address
//! ONE repository are decided against that repository's ARN
//! ([`repository_resource`]), the one that may name SEVERAL is decided against each of them
//! ([`denied_on_repositories`]), and the registry-wide actions — which take no resource
//! type at all — are decided against [`NO_RESOURCE`], so only a statement covering every
//! resource can allow one. The ARN is built from the request and the account contract
//! before anything is looked up, so the decision never depends on whether the resource
//! exists and a refused call leaves no trace.
//!
//! ## The two callers the identity decision cannot refuse
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to [`sts_store::ambient_identity`], the account's
//!   own identity, which is not a principal the IAM store holds policies for — the same
//!   position AWS account root credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   is owned by AWS, not by the account, so the IAM store holds no document for every entry
//!   of AWS's catalog (`PrincipalPolicies::uncarried`). A principal with such a policy
//!   attached is allowed: this service cannot see what that document grants, and refusing on
//!   evidence it does not have would deny access AWS permits. An attachment naming a
//!   CUSTOMER-managed policy the account does not hold grants nothing, because the account
//!   is the authority on its own policies.
//!
//! An expired session credential speaks for nobody: it carries no document and no principal
//! spelling, so every action it asks for is refused.
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which materialises the
//! session expiries the clock has already made due before it answers: that write is owed at
//! the same instant whether or not anybody called, which is why a REPORTING operation of
//! this service may make this decision — what a later caller sees is unchanged by the read.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `ecr-public:<OperationName>` ("Actions
/// defined by Amazon Elastic Container Registry Public", IAM Service Authorization
/// Reference), which is also this service's `arnNamespace`
/// (`smithy_query(query="service_info")`: `arnNamespace: ecr-public`) — the constant this
/// crate already holds as [`super::ARN_NAMESPACE`].
pub const ACTION_PREFIX: &str = super::ARN_NAMESPACE;

/// The resource an action that takes NO resource type is decided against, spelled with
/// IAM's own resource wildcard.
///
/// The registry-wide actions of this service — the authorization token, the registry
/// listing and both legs of the registry catalog — address no repository, and the Service
/// Authorization Reference lists no resource type for them, so a statement allows one only
/// by covering every resource. This is also the resource a refusal of such a call names, as
/// recorded for `GetAuthorizationToken`.
pub const NO_RESOURCE: &str = "*";

/// The ARN segment of an STS assumed-role session, whose principal is the ROLE it is a
/// session of.
const STS_NAMESPACE: &str = "sts";

/// The resource prefix of an assumed-role session's ARN:
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`.
const ASSUMED_ROLE_PREFIX: &str = "assumed-role/";

/// The IAM action `operation` is performed under: `ecr-public:<OperationName>`.
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The resource a call addressing the repository `repository_name` is decided against: that
/// repository's ARN, minted by the one builder every reported repository ARN comes from
/// ([`super::repository_arn`]), so a statement scoped to the repository a client reads back
/// is the statement that allows the call.
pub fn repository_resource(repository_name: &str) -> String {
    super::repository_arn(repository_name)
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
    /// hold — an entry of AWS's own catalog (see the module docs).
    pub unreadable_attachment: bool,
}

impl Caller {
    /// Whether the identity-based decision cannot refuse this caller: the account's own
    /// credential, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }
}

/// The refusal AWS words an identity-based decision with: who was refused, which action, on
/// which resource, and why the grant was withheld — an explicit `Deny` in a policy in
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
/// `ecr-public:<operation>` on every resource in `resources` — the message the refusal
/// carries, naming the first resource no policy allows — and `None` when they do.
///
/// An empty `resources` is a call that addresses no resource, decided against
/// [`NO_RESOURCE`]. The documents are read once and asked about each resource, so a request
/// naming several repositories parses them once.
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resources: &[String],
) -> Option<String> {
    let action = action_for(operation);
    let caller = caller(engine, ctx);
    if caller.identity_is_unbounded() {
        return None;
    }
    let documents = iam::prepare_documents(&caller.documents);
    let wildcard = [NO_RESOURCE.to_string()];
    let asked: &[String] = if resources.is_empty() {
        &wildcard
    } else {
        resources
    };
    for resource in asked {
        let request = iam::Request::new(&action, resource, &[]);
        let said = iam::identity_decision_prepared(&documents, &request);
        if !iam::is_allowed(said, Decision::NotStated) {
            // The refusal is the security-relevant event of this call, so it is
            // attributable in the emulator's log the way this service's state-changing
            // operations log theirs: who asked, for what, and the outcome. The policy
            // documents themselves are not logged.
            ::tracing::info!(
                audit = true,
                operation,
                principal = %caller.arn,
                action = %action,
                resource = %resource,
                explicit_deny = said == Decision::Denied,
                outcome = "refused_access_denied",
                "the identity-based policies in force on the caller do not permit this call"
            );
            return Some(not_authorized(
                &caller.arn,
                &action,
                resource,
                said == Decision::Denied,
            ));
        }
    }
    None
}

/// The refusal an operation returns for a request IAM does not allow, or `None` when it is
/// allowed: the one call an implemented handler makes, placed before it reads or writes this
/// service's state.
///
/// Generic over the operation's own modelled error enum because the refusal is NOT one of
/// them — this service's model declares no `AccessDeniedException` — so it travels through
/// the error channel's own variant.
pub fn denied<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resources: &[String],
) -> Option<crate::scaffolded::Scaffolded<E>> {
    refusal(engine, ctx, operation, resources).map(crate::scaffolded::Scaffolded::access_denied)
}

/// [`denied`] for an operation that addresses ONE repository by name.
pub fn denied_on_repository<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    repository_name: &str,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    denied(engine, ctx, operation, &[repository_resource(repository_name)])
}

/// [`denied`] for an operation that may name SEVERAL repositories: the caller must be
/// allowed the action on each of them, and one that names none addresses the whole registry
/// ([`NO_RESOURCE`]).
pub fn denied_on_repositories<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    repository_names: &[String],
) -> Option<crate::scaffolded::Scaffolded<E>> {
    let resources: Vec<String> = repository_names
        .iter()
        .map(|name| repository_resource(name))
        .collect();
    denied(engine, ctx, operation, &resources)
}

/// [`denied`] for an operation addressed by a `resourceArn`: the decision is made against
/// the ARN the request carried, whether or not anything answers to it.
pub fn denied_on_resource<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource_arn: &str,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    denied(engine, ctx, operation, &[resource_arn.to_string()])
}

/// [`denied`] for a registry-wide operation, whose action takes no resource type
/// ([`NO_RESOURCE`]).
pub fn denied_on_registry<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    denied(engine, ctx, operation, &[])
}

/// The principal this request speaks for, and the identity-based policies in force on it.
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
    // The IAM identity the principal's policies are held under: an assumed-role session's
    // are the ROLE's, and a long-term key's are the user's own.
    let entity_arn = match assumed_role_name(&caller.arn) {
        Some(role_name) => store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|role| role.arn.clone()),
        None => Some(caller.arn.clone()),
    };
    if let Some(policies) = entity_arn
        .as_deref()
        .and_then(|arn| store.principal_policies(arn))
    {
        caller.unreadable_attachment = !policies.uncarried.is_empty();
        caller.documents = policies.documents;
    }
    caller
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role is
/// the first segment after the resource type. `None` for any other ARN form. Read through
/// the tree's one ARN parse (`service_common::arn`), so a session ARN is split the way every
/// other ARN is.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != STS_NAMESPACE {
        return None;
    }
    let name = parsed
        .resource
        .strip_prefix(ASSUMED_ROLE_PREFIX)?
        .split('/')
        .next()?;
    (!name.is_empty()).then_some(name)
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

    /// A repository of this registry, as every ARN this service mints spells one.
    fn repository(name: &str) -> String {
        repository_resource(name)
    }

    #[test]
    fn a_principal_with_no_document_is_allowed_nothing() {
        for operation in ["CreateRepository", "DescribeRepositories", "PutImage"] {
            let action = action_for(operation);
            let said = iam::identity_decision(
                &[],
                &iam::Request::new(&action, &repository("probe"), &[]),
            );
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
            "Action":"ecr-public:CreateRepository","Resource":"{}"}}]}}"#,
            repository("granted/app")
        );
        // (case, operation, resource, allowed)
        let rows: &[(&str, &str, String, bool)] = &[
            (
                "the granted action on the granted repository",
                "CreateRepository",
                repository("granted/app"),
                true,
            ),
            (
                "another action on the same repository",
                "DeleteRepository",
                repository("granted/app"),
                false,
            ),
            (
                "the granted action on another repository",
                "CreateRepository",
                repository("other-app"),
                false,
            ),
            (
                "the granted action with no resource named",
                "CreateRepository",
                NO_RESOURCE.to_string(),
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
            {"Effect":"Allow","Action":"ecr-public:*","Resource":"*"},
            {"Effect":"Deny","Action":"ecr-public:CreateRepository","Resource":"*"}]}"#;
        let denied = decision(
            document,
            &action_for("CreateRepository"),
            &repository("probe"),
        );
        assert_eq!(
            denied,
            Decision::Denied,
            "case \"the denied action\": expected Denied, got {denied:?}"
        );
        let allowed = decision(
            document,
            &action_for("DescribeRepositories"),
            &repository("probe"),
        );
        assert_eq!(
            allowed,
            Decision::Allowed,
            "case \"an action the deny does not name\": expected Allowed, got {allowed:?}"
        );
    }

    #[test]
    fn the_refusal_names_the_principal_the_action_the_resource_and_the_reason() {
        let principal = "arn:aws:iam::000000000000:user/noperm-probe";
        let resource = repository("probe");
        // (case, explicit deny, the clause the message must carry)
        let rows: &[(&str, bool, &str)] = &[
            (
                "no policy allows it",
                false,
                "because no identity-based policy allows the ecr-public:CreateRepository action",
            ),
            (
                "a policy denies it",
                true,
                "with an explicit deny in an identity-based policy",
            ),
        ];
        for (case, explicit, reason) in rows {
            let message = not_authorized(
                principal,
                &action_for("CreateRepository"),
                &resource,
                *explicit,
            );
            let expected = format!(
                "User: {principal} is not authorized to perform: ecr-public:CreateRepository \
                 on resource: {resource} {reason}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    #[test]
    fn a_session_arn_names_the_role_it_is_a_session_of() {
        // (case, ARN, the role it names)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/registry-admin/session-1",
                Some("registry-admin"),
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/noperm-probe",
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
            let actual = assumed_role_name(arn);
            assert_eq!(
                actual, *role,
                "case {case:?}: expected role {role:?}, got {actual:?}"
            );
        }
    }
}
