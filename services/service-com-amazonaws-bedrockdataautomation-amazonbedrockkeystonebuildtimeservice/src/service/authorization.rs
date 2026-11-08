// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step an operation runs BEFORE it reads or writes anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at what the
//! request names, so a caller whose identity-based policies do not grant the action — or
//! that an explicit `Deny` covers — is told it is not authorized rather than told anything
//! about the resource, and the call performs nothing: no resource is minted, no tag is
//! written, no quota is consumed. This service models that refusal as
//! `AccessDeniedException` (`@httpError(403)`), which is a SERVICE-COMMON error of its
//! model (`smithy_query(query="digest")` lists it under `common_errors`), so it is the
//! answer every one of its 27 operations gives such a caller.
//!
//! The decision is therefore taken for EVERY operation rather than for the few whose
//! handler asks for it: it is a property of the request's identity, the action it names and
//! the resource it addresses rather than of what the handler goes on to do, so it is taken
//! ONCE, in the layer every operation is served behind
//! (`crate::service::EmulatedService::override_operation`), and nowhere else.
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
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`), so a
//!    policy `iam:PutRolePolicy` wrote takes effect on the next request and deleting it
//!    lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default
//!    deny.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split every
//! other gate in this tree makes, and the lookups stay in the service crate for the reason
//! those keep theirs there: hoisting them into `service-common` would make every service
//! that depends on that crate link the IAM and STS service crates.
//!
//! Bedrock Data Automation publishes no resource-based policy of its own — its model
//! declares no policy operation and no policy member — so the decision here is the
//! identity-based one alone.
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

use ::aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::ServerHttpProtocol;

use crate::ops::types::AccessDeniedException;

/// The resource an action that is not resource-scoped is judged against, as IAM writes it
/// for such a request — and the wildcard a COLLECTION ARN carries in place of one
/// resource's id.
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

/// This service's IAM action prefix: an action is `bedrock:<OperationName>` ("Actions
/// defined by Amazon Bedrock", IAM Service Authorization Reference, which is where this
/// service's actions are published), which is this service's `arnNamespace` /
/// `sigv4Name` (`smithy_query(query="service_info")`) — the same string
/// [`crate::state::ARN_SERVICE`] spells into every ARN this service builds.
pub const ACTION_PREFIX: &str = crate::state::ARN_SERVICE;

/// Which resource each of this model's URI spaces addresses, as `(first path segment, ARN
/// resource type)`.
///
/// Source: `smithy_query(query="digest")` on this model — the first path segment of every
/// operation's `@http` URI, paired with the ARN resource type the resources that space
/// serves carry (the types the modelled `BlueprintArn`, `DataAutomationProjectArn`,
/// `DataAutomationLibraryArn` and `BlueprintOptimizationInvocationArn` spell).
const RESOURCE_FAMILIES: &[(&str, &str)] = &[
    ("blueprints", "blueprint"),
    ("data-automation-projects", "data-automation-project"),
    ("data-automation-libraries", "data-automation-library"),
    (
        "invokeBlueprintOptimizationAsync",
        "blueprint-optimization-invocation",
    ),
    (
        "getBlueprintOptimizationStatus",
        "blueprint-optimization-invocation",
    ),
];

/// The URI spaces that address no family of their own: the TARGET is whichever resource the
/// ARN in the request names, which for these three may be a resource of any taggable
/// family (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`
/// binds each to its own path with the target in `resourceARN`).
const ARN_ADDRESSED_SPACES: &[&str] = &["tagResource", "untagResource", "listTagsForResource"];

/// Whether `value` is an ARN of THIS service's own namespace — the resource a request
/// carrying it addresses.
///
/// The parse is the crate's one ARN parse (`service_common::arn`), with the vendor field
/// compared against [`crate::state::ARN_SERVICE`], so no two services disagree about which
/// ARNs are theirs.
#[must_use]
pub fn names_a_resource_of_this_service(value: &str) -> bool {
    ::service_common::arn::parse_of(value, crate::state::ARN_SERVICE).is_some()
}

/// The ARN resource TYPE `arn` spells, when it is an ARN of this service's namespace.
fn resource_type_of(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse_of(arn, crate::state::ARN_SERVICE)?;
    parsed
        .resource
        .split_once('/')
        .map(|(resource_type, _id)| resource_type)
}

/// The ARN a request that names no single resource of its own is judged against: every
/// resource of `resource_type` in the Region the caller signed for.
///
/// A create and a list name no one resource — the id a create would mint does not exist
/// while the create is still being authorized — so what IAM evaluates is the collection the
/// action reaches, written with IAM's own resource wildcard.
#[must_use]
pub fn collection_arn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    resource_type: &str,
) -> String {
    ::engine::emulator_arn(
        crate::state::ARN_SERVICE,
        &crate::state::request_region(ctx),
        &format!("{resource_type}/{ANY_RESOURCE}"),
    )
}

/// The ARN an operation is judged against, given the ARN of one of this service's own
/// resources that the request carries (`named`, when it carries one) and the request `uri`
/// it was addressed to.
///
/// IAM evaluates an action against the resource the request addresses, and a request of
/// this service addresses one in exactly three ways:
///
/// * it NAMES it, by spelling the ARN of a resource of the family its URI space addresses
///   (`GetBlueprint$blueprintArn`, `DeleteDataAutomationProject$projectArn`). The resource
///   TYPE has to be that family's, because an operation may carry an ARN of ANOTHER family
///   beside its target — a blueprint-optimization submission names the blueprint it reads
///   and the profile it runs under — and neither of those is the resource the action acts
///   on;
/// * it names no single member of that family — a create, a listing — and so addresses the
///   family's whole collection in this account and Region ([`collection_arn`]): the id a
///   create would mint does not exist while the create is still being authorized;
/// * its URI space addresses no family of its own and the request carries the TARGET's ARN
///   instead ([`ARN_ADDRESSED_SPACES`]) — the three tagging operations, whose target may be
///   a resource of any taggable family.
///
/// Anything else is judged against [`ANY_RESOURCE`], as IAM writes it for a request that
/// addresses no one resource.
#[must_use]
pub fn judged_resource(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    named: Option<String>,
    uri: &str,
) -> String {
    let space = first_path_segment(uri);
    if ARN_ADDRESSED_SPACES.contains(&space) {
        return named.unwrap_or_else(|| ANY_RESOURCE.to_string());
    }
    let Some((_space, resource_type)) = RESOURCE_FAMILIES
        .iter()
        .find(|(declared, _)| *declared == space)
    else {
        return ANY_RESOURCE.to_string();
    };
    // The ARN the request spells, only where it is one of THIS family's.
    if let Some(arn) = named {
        if resource_type_of(&arn) == Some(*resource_type) {
            return arn;
        }
    }
    collection_arn(ctx, resource_type)
}

/// The first path segment of a request URI, which is the URI SPACE a request addresses.
///
/// `uri` is the request's own, so it may be absolute (`https://host/path?query`), which is
/// the form an AWS SDK sends: the path is taken through the shared reader
/// (`service_common::shared_path`) rather than parsed again here.
fn first_path_segment(uri: &str) -> &str {
    ::service_common::nth_path_segment(::service_common::uri_path(uri), 0).unwrap_or_default()
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request
/// ADDRESSES ([`judged_resource`]).
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise. Call it BEFORE the store is read or
/// written, and with the ARN the request carried rather than one resolved from a stored
/// record, so the decision does not depend on whether that resource exists — which is also
/// what keeps a refused call from telling an unauthorized caller whether it does.
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
    // such element, and this service publishes no resource-based policy (see the module
    // docs), so the decision below rests on the action and the resource.
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
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow is
    // not a permission the principal has. The account's own credential carries none.
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

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's two rules refused it. Every value in it comes from
/// the request and from the stored policies — the principal from the credential the request
/// presented, the action from the operation it named, the resource from the ARN it
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
    AccessDeniedException {
        message: Some(format!(
            "User: {principal} is not authorized to perform: {action}{on_resource} {reason}"
        )),
    }
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at
/// its modelled 403, serialized through the SAME protocol a handler-returned error of that
/// shape goes through, so a refusal taken in the layer and one taken in a handler are
/// indistinguishable on the wire.
///
/// The hand-built fallback carries the same three things a client branches on — the status,
/// the shape in `x-amzn-errortype` and the `__type` body — for the one case the serializer
/// can fail: a value it cannot render.
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
                "message": denial.message.clone().unwrap_or_default(),
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
        // by the ARN the account holds it under — which carries its PATH, where the session
        // ARN does not.
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

    /// RULE: the action this gate asks about is `bedrock:<OperationName>`, so a policy
    /// scoped to this service's actions covers every operation of it and nothing outside
    /// it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"bedrock:*","Resource":"*"}]}"#;
        for operation in [
            "CreateBlueprint",
            "GetBlueprint",
            "DeleteDataAutomationProject",
            "TagResource",
            "ListTagsForResource",
            "InvokeBlueprintOptimizationAsync",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action, ANY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected bedrock:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "s3:GetObject", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected bedrock:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: the refusal names the principal, the action, the resource the request
    /// addressed and which of IAM's rules refused — every part taken from the request and
    /// the stored policies — and it names no resource for an action that addresses none.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = "arn:aws:sts::000000000000:assumed-role/bda-reader/session-1";
        let blueprint = "arn:aws:bedrock:us-west-2:000000000000:blueprint/83f78afe19be";
        // (case, the operation, the resource it addressed, the clause the sentence carries)
        let rows: &[(&str, &str, &str, String)] = &[
            (
                "a resource-scoped action",
                "GetBlueprint",
                blueprint,
                format!(" on resource: {blueprint}"),
            ),
            (
                "an action that addresses no resource",
                "ListTagsForResource",
                ANY_RESOURCE,
                String::new(),
            ),
        ];
        for (case, operation, resource, clause) in rows {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reason = format!("because no identity-based policy allows the {action} action");
            let refusal = not_authorized(principal, &action, resource, &reason);
            let message = refusal.message.unwrap_or_default();
            assert_eq!(
                message,
                format!(
                    "User: {principal} is not authorized to perform: {action}{clause} {reason}"
                ),
                "case {case:?}: expected IAM's wording, got {message:?}"
            );
        }
    }
}
