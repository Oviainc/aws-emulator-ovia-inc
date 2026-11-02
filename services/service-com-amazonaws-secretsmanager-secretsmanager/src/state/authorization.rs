// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step every operation runs BEFORE it reads or writes anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the
//! request's own input, so a caller whose identity-based policies do not grant the action
//! is told it is not authorized rather than told anything about the secret it named, and
//! the call performs nothing. Secrets Manager's model declares no error shape for that
//! decision — the Smithy model's `common_errors` are `InternalServiceError` and
//! `InvalidParameterException`, and no operation lists an access error
//! (`smithy_query(query="digest")`) — because the refusal is the AWS COMMON
//! `AccessDeniedException`, answered by the layer ahead of a service's own code. This
//! service speaks awsJson1_1, which discriminates a failure by the error type in the body
//! rather than by the transport status, so that refusal travels with this service's
//! client-error status ([`crate::scaffolded::ACCESS_DENIED_STATUS`]) and the error code
//! [`crate::scaffolded::ACCESS_DENIED_CODE`], through
//! [`crate::scaffolded::Scaffolded::AccessDenied`].
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    owns the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a policy `iam:PutUserPolicy` wrote takes
//!    effect on the next request, deleting it lifts it again, and this service reads an
//!    identity's permissions exactly as IAM's own authorizer does;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the ARN readings do. That is
//! the same split every other gate in this tree makes (AWS Certificate Manager, Amazon
//! Textract, Amazon OpenSearch Service, Amazon EVS, CloudWatch RUM, Amazon Cognito
//! Identity), and the lookups stay in the service crate for the reason those keep theirs
//! there: hoisting them into `service-common` would make every service that depends on that
//! crate link the IAM and STS service crates. WHICH IAM ENTITY a principal ARN names
//! carries no such dependency, so it is read through the shared
//! [`::service_common::iam_role`] — `assumed_role_session_role_name` for the role behind a
//! session and `user_name` for an IAM user — rather than parsed again here.
//!
//! ## What a call is judged against
//!
//! The action is `secretsmanager:<OperationName>` ([`ACTION_PREFIX`]), and the resource is
//! what the request ADDRESSES:
//!
//! * an operation carrying a `SecretId` is judged against that secret's ARN
//!   ([`secret_resource`]);
//! * `CreateSecret` against the ARN of the secret it is about to create, which it allocates
//!   before it stores anything;
//! * an operation that addresses no secret of its own — the account-wide list, the password
//!   generator, the batched read that selects by filter — against the resource IAM writes
//!   for such a request ([`ANY_RESOURCE`]), and the refusal of one states no resource
//!   clause, because there is nothing to name.
//!
//! The secretsmanager condition keys (`aws:RequestTag`, `aws:ResourceTag`,
//! `secretsmanager:VersionStage`) are NOT part of the request handed to the shared
//! evaluation here: a statement whose `Condition` this gate cannot decide never grants and
//! never stops denying, which is that evaluation's fail-closed reading of an undecided
//! clause.
//!
//! An operation whose documentation requires FURTHER permissions beyond its own action —
//! the batched read's `secretsmanager:GetSecretValue` on each secret it answers for, a
//! rotation's `lambda:InvokeFunction` on the rotation function — is judged here on its own
//! action alone; a decision about another action or another service's resource belongs to
//! the operation that needs it.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::scaffolded::AccessDeniedException;

/// The resource an action that is not resource-scoped is judged against, as IAM writes it
/// for such a request.
pub const ANY_RESOURCE: &str = "*";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// This service's IAM action prefix: an action is `secretsmanager:<OperationName>`
/// ("Actions defined by AWS Secrets Manager", IAM Service Authorization Reference), which
/// is this service's `arnNamespace` (`smithy_query(query="service_info")`) — the same
/// string this service spells into every ARN it builds.
pub const ACTION_PREFIX: &str = "secretsmanager";

/// The resource type a secret's ARN names: `arn:aws:secretsmanager:<region>:<account>:\
/// secret:<name>-<six characters>` (`::service_common::arn::SECRETS_MANAGER_SECRET`).
const SECRET_RESOURCE_TYPE: &str = "secret";

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request NAMES.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the refusal otherwise, which each operation returns through
/// [`crate::scaffolded::Scaffolded::AccessDenied`]. Call it BEFORE the store is read or
/// written, so a refused call performs nothing — and before the operation's own answer for
/// the secret it named, so a refusal tells an unauthorized caller nothing about whether
/// that secret exists.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called, which is why a REPORTING operation of this
/// service may take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, so the decision below rests on the action and the resource. (The
    // resource-based policy a secret may carry is a SECOND decision, which this gate does
    // not take: it grants access the identity-based documents do not, and the operations
    // that own the stored policy are where it is read.)
    let request = iam::Request::new(&action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => {
                format!("because no permissions boundary allows the {action} action")
            }
        };
        return Err(not_authorized(&caller.arn, &action, resource, &reason));
    }
    Ok(())
}

/// Authorize a request that addresses the secret `secret_id` names, judged against that
/// secret's ARN ([`secret_resource`]).
///
/// The one call site shape for every operation of this service that carries a `SecretId`,
/// so no two of them can judge the same request against different resources.
pub fn authorize_secret<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    secret_id: Option<&str>,
) -> Result<(), AccessDeniedException> {
    let resource = secret_resource(engine, ctx, secret_id);
    authorize(engine, ctx, operation, &resource)
}

/// The ARN the secret a `SecretId` names is judged against.
///
/// A `SecretId` is "the ARN or name of the secret", and the partial ARN (the full one
/// without its six trailing characters) names it too — while a policy statement's
/// `Resource` element matches ARNs, and the ARN of a secret this account holds is the FULL
/// one. So the id is resolved through the store's own resolution of a `SecretId`
/// ([`crate::state::ComAmazonawsSecretsmanagerSecretsmanagerState::get`]), which is the
/// same reading every operation addresses a secret by; a request naming a secret this
/// account does not hold is judged against the ARN that id spells — itself when it is an
/// ARN already, else the ARN a secret of that name would carry in the Region the caller
/// signed for.
///
/// `None`, for an operation whose `SecretId` is optional and absent, is judged against
/// [`ANY_RESOURCE`]: no secret is named, so there is none to judge against.
///
/// A READ of this service's own store, taken before the decision and under its own
/// short-lived guard: nothing is written, so what a later caller sees is unchanged.
#[must_use]
pub fn secret_resource<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    secret_id: Option<&str>,
) -> String {
    let Some(secret_id) = secret_id.filter(|id| !id.is_empty()) else {
        return ANY_RESOURCE.to_string();
    };
    let held = crate::state::service_state(engine).and_then(|state| {
        let store = state.lock();
        store.get(secret_id).map(|secret| secret.arn.clone())
    });
    if let Some(arn) = held {
        return arn;
    }
    if ::service_common::arn::is_resource_arn_of(secret_id, ACTION_PREFIX) {
        return secret_id.to_string();
    }
    secret_arn(&crate::state::request_region(ctx.parts()), secret_id)
}

/// The ARN a secret of name `name` carries in `region`, through the emulator's own ARN
/// builder — the same one `CreateSecret` allocates a secret's ARN from, so a policy scoped
/// to this account's secrets covers it.
///
/// The six random characters a created secret's ARN ends in belong to the secret rather
/// than to its name, so a name this account holds no secret for yields the ARN without
/// them: there is no stored secret to take them from, and inventing a value here would
/// name a resource nothing else addresses.
#[must_use]
pub fn secret_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ACTION_PREFIX,
        region,
        &format!("{SECRET_RESOURCE_TYPE}:{name}"),
    )
}

/// The ARN a create of a secret named `name` is judged against, which is also the ARN it
/// allocates for the secret: the one the account ALREADY holds a secret of that name under
/// — a repeat of a create addresses the resource the first one created, and answers from it
/// — else a fresh one, the name plus the six random characters a secret's ARN ends in.
///
/// Allocating it before the decision is what lets a create be judged against the secret it
/// is about to create, which is the resource the action is taken on; nothing is stored by
/// reading it.
#[must_use]
pub fn created_secret_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    name: &str,
) -> String {
    let held = crate::state::service_state(engine).and_then(|state| {
        let store = state.lock();
        store.secrets.get(name).map(|secret| secret.arn.clone())
    });
    held.unwrap_or_else(|| {
        ::engine::emulator_arn(
            ACTION_PREFIX,
            region,
            &format!(
                "{SECRET_RESOURCE_TYPE}:{name}-{}",
                crate::state::new_arn_suffix()
            ),
        )
    })
}

/// The refusal IAM states an authorization failure with: who was refused, which action,
/// on which resource, and which of IAM's two rules refused it. Every value in it comes
/// from the request and from the stored policies — the principal from the credential the
/// request presented, the action from the operation it named, the resource from the ARN it
/// addressed.
///
/// An action that addresses no resource of its own is judged against [`ANY_RESOURCE`], and
/// the sentence states no resource clause for it: there is nothing to name.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    let on_resource = if resource == ANY_RESOURCE {
        String::new()
    } else {
        format!(" on resource: {resource}")
    };
    AccessDeniedException::new(format!(
        "User: {principal} is not authorized to perform: {action}{on_resource} {reason}"
    ))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the
    /// IAM role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// own accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(
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
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The
        // key id it presented is NOT quoted back — this module echoes no value of the
        // request — and in the assembled emulator the layer that authenticates a request
        // refuses an expired credential before any handler runs.
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                principals: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
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
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            caller.read_policies_of(&store, &held.arn.clone());
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `secretsmanager:<OperationName>`, so a
    /// policy scoped to this service's actions covers every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"secretsmanager:*","Resource":"*"}]}"#;
        for operation in [
            "ListSecrets",
            "GetSecretValue",
            "CreateSecret",
            "UpdateSecretVersionStage",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action, ANY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected secretsmanager:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "s3:GetObject", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected secretsmanager:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: the principal a request speaks for is read through the SHARED readings of an
    /// identity ARN, so a session ARN names the role whose policies this gate reads and an
    /// IAM user ARN names the user whose policies it reads, while every other principal
    /// form names neither (and the account's own identity carries no policies at all).
    #[test]
    fn a_principal_arn_names_the_entity_its_policies_are_read_from() {
        let account = ::engine::emulator_account_id();
        // (case, the ARN, the role it names, the user it names)
        let rows: Vec<(&str, String, Option<&str>, Option<&str>)> = vec![
            (
                "an assumed-role session",
                format!("arn:aws:sts::{account}:assumed-role/secrets-reader/session-1"),
                Some("secrets-reader"),
                None,
            ),
            (
                "a session name carrying a slash",
                format!("arn:aws:sts::{account}:assumed-role/secrets-reader/one/two"),
                Some("secrets-reader"),
                None,
            ),
            (
                "an IAM user",
                format!("arn:aws:iam::{account}:user/secrets-user"),
                None,
                Some("secrets-user"),
            ),
            (
                "an IAM user under a path",
                format!("arn:aws:iam::{account}:user/team/secrets-user"),
                None,
                Some("secrets-user"),
            ),
            (
                "the account's own identity",
                format!("arn:aws:iam::{account}:root"),
                None,
                None,
            ),
            (
                "a value that is no ARN",
                "AKIDTESTONLY".to_string(),
                None,
                None,
            ),
        ];
        for (case, arn, role, user) in rows {
            let read_role = ::service_common::iam_role::assumed_role_session_role_name(&arn);
            let read_user = ::service_common::iam_role::user_name(&arn);
            assert_eq!(
                read_role, role,
                "case {case:?}: expected the role {role:?}, got {read_role:?}"
            );
            assert_eq!(
                read_user, user,
                "case {case:?}: expected the user {user:?}, got {read_user:?}"
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource the request
    /// addressed and which of IAM's rules refused — every part taken from the request and
    /// the stored policies — and it names no resource for an action that addresses none.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let account = ::engine::emulator_account_id();
        let principal = format!("arn:aws:iam::{account}:user/secrets-user");
        let secret = secret_arn("us-west-2", "cdn-credentials-AbCdEf");
        // (case, the operation, the resource it addressed, the clause the sentence carries)
        let rows: &[(&str, &str, &str, String)] = &[
            (
                "a secret-scoped action",
                "GetSecretValue",
                secret.as_str(),
                format!(" on resource: {secret}"),
            ),
            (
                "an action that addresses no secret",
                "ListSecrets",
                ANY_RESOURCE,
                String::new(),
            ),
        ];
        for (case, operation, resource, clause) in rows {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reason = format!("because no identity-based policy allows the {action} action");
            let refusal = not_authorized(&principal, &action, resource, &reason);
            assert_eq!(
                refusal.message,
                format!(
                    "User: {principal} is not authorized to perform: {action}{clause} {reason}"
                ),
                "case {case:?}: expected IAM's wording, got {:?}",
                refusal.message
            );
        }
    }
}
