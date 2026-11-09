// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the decision
//! taken BEFORE any operation of it reads or writes anything.
//!
//! `AccessDeniedException` (HTTP 403) is a SERVICE-COMMON error of this model
//! (`smithy_query(query="service_info")` lists it under `common_errors`), so it is the
//! answer every operation here gives a caller whose permissions do not cover the action it
//! named. AWS takes that decision at the IAM layer, in front of the service: the call then
//! performs nothing — no definition is minted, no run is started, no quota is consumed — so
//! a refused create leaves no resource behind for a later read to find.
//!
//! Because the decision is a property of the request's identity, the action it names and the
//! resource it addresses rather than of what a handler goes on to do, it is taken ONCE, in
//! the layer every operation of this service is served behind
//! (`crate::service::EmulatedService::override_operation`), and nowhere else.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same record
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one the
//!    caller reads back for the credentials it signed with), else the IAM user that owns the
//!    long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), so a document
//!    `iam:PutRolePolicy` wrote takes effect on the next request and deleting it lifts it
//!    again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the split every other
//! gate in this tree makes, and the lookups stay in this crate for the reason those keep
//! theirs there: hoisting them into `service-common` would make every service that depends
//! on that crate link the IAM and STS service crates.
//!
//! ## The action and the resource
//!
//! The action is `nova-act:<OperationName>` — this service's `arnNamespace`, which is also
//! its `sigv4Name` (`smithy_query(query="service_info")`), so the prefix is read off the
//! request's own resolved identity rather than restated here and one statement scoped to
//! this service's actions covers every operation of it and nothing outside it.
//!
//! The resource is the one the REQUEST addresses, written as this service's own ARN
//! ([`judged_resource`]): the resource chain the URI names, and for a create the resource it
//! would create. Taking it from the request rather than from a stored record is what keeps
//! the decision independent of whether that resource exists — which is also what keeps a
//! refused call from telling an unauthorized caller whether it does.
//!
//! This service publishes no resource-based policy of its own — the model declares no policy
//! operation and no policy member — so the decision here is the identity-based one alone.
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
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no principal
//! spelling, so every action it asks for is refused. (A session the authenticating layer has
//! already refused never reaches a handler at all.) A FEDERATED-user session is in the same
//! position for the opposite reason: its permissions are the ones the mint passed rather than
//! an identity's, and the IAM store holds no documents under its ARN, so every action it asks
//! for falls to the default deny.

use std::fmt::Write as _;
use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::ServerHttpProtocol;

use crate::ops::types::AccessDeniedException;

/// The resource IAM writes for a request that addresses no resource of this service, and the
/// wildcard that stands in for one resource's id in a COLLECTION's ARN.
pub const ANY_RESOURCE: &str = "*";

/// The wire name of the refusal this gate answers with, and the status the model gives it:
/// `AccessDeniedException`, `@error("client")` at `@httpError(403)`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The media type this restJson1 service renders an error in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The operation whose target resource is named in the BODY rather than in the URI: the
/// create of a workflow definition, which addresses the definition it would create
/// (`smithy_query(query="operation",operation="CreateWorkflowDefinition")` binds `name` in
/// the body of `PUT /workflow-definitions`).
const CREATE_WORKFLOW_DEFINITION: &str = "CreateWorkflowDefinition";

/// The body member that create names the definition it addresses in.
const NAME_MEMBER: &str = "name";

/// This model's resource chain, as `(the URI segment a collection is addressed by, the ARN
/// resource type its members carry)` in NESTING order.
///
/// Source: `smithy_query(query="digest")` on this model — every operation's `@http` URI is
/// built from these collection segments in this order — paired with the ARN segments
/// [`crate::state::act_path`] and the shapes above it spell for the same chain, so the ARN
/// this gate judges against and the ARN an operation reports for the same resource agree.
const RESOURCE_CHAIN: &[(&str, &str)] = &[
    ("workflow-definitions", "workflow-definition"),
    ("workflow-runs", "workflow-run"),
    ("sessions", "session"),
    ("acts", "act"),
];

/// This service's IAM action prefix, which is its `arnNamespace`
/// (`smithy_query(query="service_info")`) and also the name every request to it is signed
/// under — so the prefix is read off the request's own resolved identity rather than restated
/// here, and the ARNs this gate builds and the actions it asks about cannot name two
/// different services.
#[must_use]
pub fn action_prefix<'ctx>(ctx: &'ctx ::smithy_server_runtime::OperationContext<'_>) -> &'ctx str {
    ctx.identity().signing_name()
}

/// The resource this request is judged against: this service's ARN for the resource the
/// request ADDRESSES, in the Region the request was signed for.
///
/// A request of this service addresses a resource in exactly two ways:
///
/// * its URI NAMES one, by walking this model's resource chain
///   ([`RESOURCE_CHAIN`]) — a definition, a run under it, a session under that run, an act
///   under that session — and a request that stops at a COLLECTION (a create of a child, a
///   listing) addresses every member of that collection, which IAM writes with its own
///   resource wildcard;
/// * it CREATES one, so the id does not exist while the create is being authorized and the
///   resource it addresses is the one it would create: the definition the body's `name`
///   spells.
///
/// Anything else — a request to an operation that addresses no resource of this service at
/// all — is judged against [`ANY_RESOURCE`], as IAM writes it for such a request.
#[must_use]
pub fn judged_resource(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    req: &RtRequest,
) -> String {
    let region = crate::state::request_region(ctx.parts());
    if operation == CREATE_WORKFLOW_DEFINITION {
        if let Some(name) = created_definition_name(req) {
            return crate::state::workflow_definition_arn(&region, &name);
        }
    }
    match addressed_resource_path(::service_common::uri_path(req.uri())) {
        Some(path) => crate::state::resource_arn(&region, &path),
        None => ANY_RESOURCE.to_string(),
    }
}

/// The name a `CreateWorkflowDefinition` request addresses, read off the request's own body,
/// or `None` when the body is not a readable JSON object carrying [`NAME_MEMBER`] as a
/// string.
///
/// `None` is the honest answer for a malformed or absent member: such a request is judged
/// against the definition COLLECTION — never against a resource the caller did not name —
/// and the operation refuses the absent member itself through the modelled constraint walk.
fn created_definition_name(req: &RtRequest) -> Option<String> {
    let body = req.body().bytes()?;
    let document: ::serde_json::Value = ::serde_json::from_slice(body).ok()?;
    document
        .get(NAME_MEMBER)
        .and_then(::serde_json::Value::as_str)
        .map(str::to_string)
}

/// The resource part of the ARN a request to `path` addresses: the deepest resource of
/// [`RESOURCE_CHAIN`] the path names, with [`ANY_RESOURCE`] for a level the path addresses as
/// a collection. `None` for a path that names no resource of this service's chain at all.
///
/// The path labels are percent-decoded the way the codec that deserializes the request
/// decodes them, so the ARN carries the value the handler will see rather than its escaped
/// spelling.
fn addressed_resource_path(path: &str) -> Option<String> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let mut resource = String::new();
    for (collection, resource_type) in RESOURCE_CHAIN {
        if segments.next() != Some(*collection) {
            break;
        }
        if !resource.is_empty() {
            resource.push('/');
        }
        match segments.next() {
            Some(id) => {
                let id = ::service_common::percent_decode_path_label(id);
                let _ = write!(resource, "{resource_type}/{id}");
            }
            // The path stops at this collection, so the request addresses every member of
            // it.
            None => {
                let _ = write!(resource, "{resource_type}/{ANY_RESOURCE}");
                break;
            }
        }
    }
    (!resource.is_empty()).then_some(resource)
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request ADDRESSES
/// ([`judged_resource`]).
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see the
/// module docs); the modelled refusal otherwise. Taken BEFORE the operation runs, so a
/// refused request reads and writes nothing.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called, which is why a REPORTING operation of this service is
/// judged here too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{}:{operation}", action_prefix(ctx));
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no such
    // element, and this service publishes no resource-based policy (see the module docs), so
    // the decision below rests on the action and the resource.
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
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not a
    // permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the identity-based
            // policies gave.
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => {
                format!("because no permissions boundary allows the {action} action")
            }
        };
        return Err(not_authorized(&caller.arn, &action, resource, &reason));
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it. Every value in it comes from the
/// request and from the stored policies — the principal from the credential the request
/// presented, the action from the operation it named, the resource from the ARN it addressed.
///
/// A request that addresses no resource of this service is judged against [`ANY_RESOURCE`],
/// and the sentence states no resource clause for it: there is nothing to name.
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
    AccessDeniedException {
        message: format!(
            "User: {principal} is not authorized to perform: {action}{on_resource} {reason}"
        ),
    }
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at its
/// modelled 403, serialized through the SAME protocol a handler-returned error of that shape
/// goes through, so a refusal taken in the layer and one taken in a handler are
/// indistinguishable on the wire.
///
/// The hand-built fallback carries the same three things a client branches on — the status,
/// the shape in `x-amzn-errortype` and the `__type` body — for the one case the serializer can
/// fail: a value it cannot render.
#[must_use]
pub fn refusal_response(
    protocol: &dyn ServerHttpProtocol,
    denial: &AccessDeniedException,
) -> RtResponse {
    protocol
        .serialize_error(
            denial,
            &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_STATUS,
        )
        .unwrap_or_else(|_| {
            let body = ::serde_json::json!({
                "__type": ACCESS_DENIED_EXCEPTION,
                "message": denial.message.clone(),
            });
            let mut response = RtResponse::new(
                StatusCode::try_from(ACCESS_DENIED_STATUS)
                    .expect("the status is a constant of this module"),
                SdkBody::from(body.to_string()),
            );
            response
                .headers_mut()
                .insert("content-type", JSON_CONTENT_TYPE);
            response
                .headers_mut()
                .insert("x-amzn-errortype", ACCESS_DENIED_EXCEPTION);
            response
        })
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the IAM
    /// role behind an assumed-role session.
    principals: Vec<String>,
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

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's own
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
        // principal spelling, and every action it asks for falls to the default deny. The key
        // id it presented is NOT quoted back — this module echoes no value of the request —
        // and in the assembled emulator the layer that authenticates a request refuses an
        // expired credential before any handler runs.
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
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
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

    /// RULE: the resource a request is judged against is the deepest resource of this
    /// model's chain its URI names, with the collection wildcard for a level it addresses as
    /// a collection, and nothing at all for a path outside the chain.
    #[test]
    fn the_resource_is_the_one_the_request_path_addresses() {
        // (case, the request path, the resource field of the ARN it is judged against)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "the definition collection",
                "/workflow-definitions",
                Some("workflow-definition/*"),
            ),
            (
                "one definition",
                "/workflow-definitions/planner",
                Some("workflow-definition/planner"),
            ),
            (
                "the run collection of one definition",
                "/workflow-definitions/planner/workflow-runs",
                Some("workflow-definition/planner/workflow-run/*"),
            ),
            (
                "one run",
                "/workflow-definitions/planner/workflow-runs/run-1",
                Some("workflow-definition/planner/workflow-run/run-1"),
            ),
            (
                "one act, with a trailing step segment",
                "/workflow-definitions/planner/workflow-runs/run-1/sessions/s-1/acts/a-1/invoke-step/",
                Some("workflow-definition/planner/workflow-run/run-1/session/s-1/act/a-1"),
            ),
            (
                "a path that leaves the chain",
                "/workflow-definitions/planner/acts",
                Some("workflow-definition/planner"),
            ),
            ("a path outside the chain", "/models", None),
            ("the root path", "/", None),
        ];
        for (case, path, expected) in rows {
            let actual = addressed_resource_path(path);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the resource a request is judged against is the one this service's own ARNs name
    /// it by, so the walk above reproduces the resource path the state module builds for the
    /// same resource — a statement scoped to the ARN an operation REPORTS covers the request
    /// that addresses it.
    #[test]
    fn the_walk_reproduces_the_resource_path_this_service_reports() {
        let (name, run, session, act) = ("planner", "run-1", "s-1", "a-1");
        // (case, the request path, the resource path the state module spells)
        let rows: Vec<(&str, String, String)> = vec![
            (
                "a definition",
                format!("/workflow-definitions/{name}"),
                crate::state::workflow_definition_path(name),
            ),
            (
                "a run",
                format!("/workflow-definitions/{name}/workflow-runs/{run}"),
                crate::state::workflow_run_path(name, run),
            ),
            (
                "a session",
                format!("/workflow-definitions/{name}/workflow-runs/{run}/sessions/{session}"),
                crate::state::session_path(name, run, session),
            ),
            (
                "an act",
                format!(
                    "/workflow-definitions/{name}/workflow-runs/{run}/sessions/{session}/\
                     acts/{act}"
                ),
                crate::state::act_path(name, run, session, act),
            ),
        ];
        for (case, path, expected) in rows {
            let actual = addressed_resource_path(&path);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource the request addressed
    /// and which of IAM's rules refused it — every part taken from the request and the stored
    /// policies — and it names no resource for a request that addresses none.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = format!(
            "arn:aws:sts::{}:assumed-role/novaact-caller/session-1",
            ::engine::emulator_account_id()
        );
        let definition = crate::state::workflow_definition_arn("us-east-1", "planner");
        // (case, the action, the resource the request addressed, the clause the sentence
        // carries)
        let rows: &[(&str, &str, &str, String)] = &[
            (
                "a resource-scoped action",
                "nova-act:CreateWorkflowDefinition",
                &definition,
                format!(" on resource: {definition}"),
            ),
            (
                "an action that addresses no resource",
                "nova-act:ListModels",
                ANY_RESOURCE,
                String::new(),
            ),
        ];
        for (case, action, resource, clause) in rows {
            let reason = "with an explicit deny in an identity-based policy";
            let refusal = not_authorized(&principal, action, resource, reason);
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

    /// RULE: a create addresses the resource it would create, which it names in its body.
    #[test]
    fn a_create_addresses_the_definition_its_body_names() {
        // (case, the body, the name the create addresses)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a create naming its definition",
                r#"{"name":"planner","description":"d"}"#,
                Some("planner"),
            ),
            ("a body naming none", r#"{"description":"d"}"#, None),
            ("a name of another JSON type", r#"{"name":42}"#, None),
            ("a body that is not an object", "[]", None),
            ("a body no codec can read", "{", None),
        ];
        for (case, body, expected) in rows {
            let mut req = RtRequest::new(SdkBody::from(body.to_string()));
            req.set_uri("/workflow-definitions")
                .expect("the test URI is valid");
            let actual = created_definition_name(&req);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
