// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the principal a request speaks for perform the action it asks for —
//! the step every operation of this service takes BEFORE it reads or writes anything.
//!
//! Every operation of this API is a signed call (`sigv4`, and no `@optionalAuth` anywhere
//! in the model — `smithy_query(query="service_info")`), and AWS decides such a call at the
//! IAM layer, in front of the service: a caller whose identity-based policies do not grant
//! the action is told it is not authorized rather than told anything about the resource it
//! named or the input it sent, and the call changes nothing. So the decision is taken
//! first, ahead of this service's own request rules, and answered with
//! `AccessDeniedException` (HTTP 403) — the refusal nine of this model's ten operations
//! declare, and which `GetAccountAlias` takes through `extra_errors`.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one the
//!    caller reads back for the credentials it signed with), else the IAM user that owns the
//!    long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), where the
//!    collection rule itself lives — so a policy `iam:PutRolePolicy` wrote takes effect on
//!    the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the ARN readings do. The
//! lookups stay in this service crate for the reason every other service's gate keeps
//! theirs there: hoisting them into `service-common` would make every service that depends
//! on that crate link the IAM and STS service crates.
//!
//! ## The resource a Support App action is judged against
//!
//! This API publishes no resource types of its own — the model declares no ARN member and
//! no resource shape, and every operation is a POST to its own path under `/control`
//! (`smithy_query(query="digest")`) — so the resource an action is performed on is that
//! path, in this service's own ARN namespace: `arn:aws:supportapp:<region>:<account>:<uri>`,
//! built by [`operation_resource_arn`] from the operation's modelled URI and the Region the
//! call was signed for. It is derived from the operation, not from the request's own path,
//! so no caller-supplied text reaches the refusal.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses, and a permissions boundary the
//!   store DOES hold still caps the grant.
//!
//! An expired session credential speaks for nobody: it carries no documents and no principal
//! spelling, so every action it asks for is refused. (A session the authenticating layer has
//! already refused never reaches a handler at all.)
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which first materialises the
//! session expiries the clock has already made due; that write is owed at the same instant
//! whether or not anybody called, which is why a REPORTING operation of this service may take
//! this decision too.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// This service's IAM action prefix: an action is `supportapp:<OperationName>` ("Actions
/// defined by AWS Support App in Slack", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` and its `sigv4Name`
/// (`smithy_query(query="service_info")`) — the same string the resource ARN below spells.
pub const ACTION_PREFIX: &str = "supportapp";

/// The URI space every operation of this API is bound under: each one is a POST to
/// `/control/<its-name-in-kebab-case>` (`smithy_query(query="digest")`).
const CONTROL_PATH_PREFIX: &str = "/control/";

/// The Region a resource ARN carries when the request names none — the Region an unsigned
/// request is served in, which is also the emulator's default.
const DEFAULT_REGION: &str = "us-east-1";

/// The AWS Region a request was signed for, read from its SigV4 credential scope.
pub fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The IAM action `operation` is performed under: `supportapp:<OperationName>`.
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The modelled URI `operation` is bound to: `/control/` plus the operation name with each
/// word lowercased and hyphen-separated, which is how this service's model spells every one
/// of its ten `@http` URIs (`smithy_query(query="digest")`).
fn control_path(operation: &str) -> String {
    let mut path = String::from(CONTROL_PATH_PREFIX);
    for (index, character) in operation.char_indices() {
        if character.is_ascii_uppercase() && index != 0 {
            path.push('-');
        }
        path.extend(character.to_lowercase());
    }
    path
}

/// The ARN of the resource `operation` acts on, for a request signed for `region`: this
/// service's ARN namespace, the account the emulator presents, and the operation's own
/// modelled URI as the resource part (see the module docs).
pub fn operation_resource_arn(region: &str, operation: &str) -> String {
    ::emulator_server::emulator_arn(ACTION_PREFIX, region, &control_path(operation))
}

/// The modelled refusal for a request IAM does not permit, or `None` when it does (and for
/// every caller this gate does not bound — see the module docs).
///
/// `operation` is the caller's [`smithy_server_runtime::Operation::NAME`]. Called BEFORE the
/// store is reached and with no guard of it held, so a refused call reads and writes nothing
/// and tells an unauthorized caller nothing about what the account holds.
pub fn authorization_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
) -> Option<AccessDeniedException> {
    let action = action_for(operation);
    let resource = operation_resource_arn(&request_region(ctx), operation);
    let caller = caller(engine, ctx);
    // An identity-based document states no `Principal` element, and this API publishes no
    // resource-based policy of its own (the model declares no policy operation and no policy
    // member), so the decision rests on the action and the resource.
    let said = iam::identity_decision(&caller.documents, &iam::Request::new(&action, &resource, &[]));
    // Taken FIRST, so an explicit `Deny` in a document that CAN be read refuses the request
    // whatever else the principal carries — including a grant this emulator cannot read,
    // which only lifts the DEFAULT deny of a request no readable statement allows.
    if said == Decision::Denied {
        return Some(not_authorized(
            &caller.arn,
            &action,
            &resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    if !(iam::is_allowed(said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Some(not_authorized(
            &caller.arn,
            &action,
            &resource,
            &format!("because no identity-based policy allows the {action} action"),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not a
    // permission the principal has. The account's own credential carries none.
    let boundary = caller.boundary.as_ref()?;
    let reason = match iam::identity_decision(
        ::std::slice::from_ref(boundary),
        &iam::Request::new(&action, &resource, &[]),
    ) {
        Decision::Allowed => return None,
        // An explicit deny is a fact about the boundary document, so the refusal names it; a
        // boundary that merely states nothing withheld the grant the policies gave.
        Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
        Decision::NotStated => {
            format!("because no permissions boundary allows the {action} action")
        }
    };
    Some(not_authorized(&caller.arn, &action, &resource, &reason))
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it. Every value in it comes from the
/// request and from the stored policies — the principal from the credential the request
/// presented, the action from the operation it named, the resource from that operation's
/// modelled URI.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(format!(
            "User: {principal} is not authorized to perform: {action} on resource: {resource} \
             {reason}"
        )),
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose document
    /// this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no IAM
    /// policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's OWN
    /// accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's own
            // catalog this emulator does not carry: a grant it cannot read.
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
    let access_key_id = ::emulator_server::extract_akid(&ctx.parts().headers);
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
        // principal spelling, and every action it asks for falls to the default deny. The key
        // id it presented is NOT quoted back — this module echoes no value of the request.
        Err(_) => {
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
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
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the user's
    // policies rather than served as the account. A key the IAM store does not hold either
    // really is the account's own published credential.
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
    // Which IAM entity a principal ARN names is read through the tree's shared readings of
    // one, not parsed again here.
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

#[cfg(test)]
mod tests {
    use super::*;

    /// Every operation this service binds, with the URI the model binds it to
    /// (`smithy_query(query="digest")`) — the catalog [`control_path`] has to reproduce.
    const MODELLED_URIS: [(&str, &str); 10] = [
        (
            "CreateSlackChannelConfiguration",
            "/control/create-slack-channel-configuration",
        ),
        ("DeleteAccountAlias", "/control/delete-account-alias"),
        (
            "DeleteSlackChannelConfiguration",
            "/control/delete-slack-channel-configuration",
        ),
        (
            "DeleteSlackWorkspaceConfiguration",
            "/control/delete-slack-workspace-configuration",
        ),
        ("GetAccountAlias", "/control/get-account-alias"),
        (
            "ListSlackChannelConfigurations",
            "/control/list-slack-channel-configurations",
        ),
        (
            "ListSlackWorkspaceConfigurations",
            "/control/list-slack-workspace-configurations",
        ),
        ("PutAccountAlias", "/control/put-account-alias"),
        (
            "RegisterSlackWorkspaceForOrganization",
            "/control/register-slack-workspace-for-organization",
        ),
        (
            "UpdateSlackChannelConfiguration",
            "/control/update-slack-channel-configuration",
        ),
    ];

    /// RULE: the resource an action is judged against is the operation's own modelled URI in
    /// this service's ARN namespace — so the path this module derives has to be the one the
    /// model binds, for every operation and not only the one a recording named.
    #[test]
    fn the_resource_is_the_operations_modelled_uri() {
        for (operation, uri) in MODELLED_URIS {
            let derived = control_path(operation);
            assert_eq!(
                derived, uri,
                "case {operation:?}: expected the modelled URI {uri:?}, got {derived:?}"
            );
            let arn = operation_resource_arn("us-west-2", operation);
            assert_eq!(
                arn,
                format!(
                    "arn:aws:{ACTION_PREFIX}:us-west-2:{}:{uri}",
                    ::engine::emulator_account_id()
                ),
                "case {operation:?}: expected this service's ARN for {uri:?}, got {arn:?}"
            );
        }
    }

    /// RULE: the action asked about is `supportapp:<OperationName>`, so a policy scoped to
    /// this service's actions covers every operation of it and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"supportapp:*","Resource":"*"}]}"#;
        for (operation, _) in MODELLED_URIS {
            let action = action_for(operation);
            let reached = iam::identity_decision(
                &[document.to_string()],
                &iam::Request::new(&action, "*", &[]),
            );
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected supportapp:* to allow {action}, got {reached:?}"
            );
        }
        let outside = iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new("support:DescribeCases", "*", &[]),
        );
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected supportapp:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: the refusal names the principal, the action, the resource the request addressed
    /// and which of IAM's rules refused it — every part taken from the request and the stored
    /// policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = "arn:aws:sts::000000000000:assumed-role/noperm-probe/session-1";
        let action = action_for("GetAccountAlias");
        let resource = operation_resource_arn("us-east-1", "GetAccountAlias");
        // (case, the clause the sentence closes with)
        let rows: Vec<(&str, String)> = vec![
            (
                "no policy allows it",
                format!("because no identity-based policy allows the {action} action"),
            ),
            (
                "a policy denies it",
                "with an explicit deny in an identity-based policy".to_string(),
            ),
            (
                "the permissions boundary withholds it",
                format!("because no permissions boundary allows the {action} action"),
            ),
        ];
        for (case, reason) in rows {
            let message = not_authorized(principal, &action, &resource, &reason)
                .message
                .unwrap_or_default();
            let expected = format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {resource} {reason}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
