// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step an operation runs BEFORE it reads or writes anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at what the
//! request names, so a caller whose identity-based policies do not grant the action — or
//! that an explicit `Deny` covers — is told it is not authorized rather than told anything
//! about the resource, and the call performs nothing: no resource is minted, no status
//! moves, no quota is consumed. This service models that refusal as `AccessDeniedException`
//! (`@httpError(403)`), which 107 of this service's 108 operations declare
//! (`smithy_query(query="operation",operation="CreateGuardrail,DeleteResourcePolicy,CreateModelInvocationJob")`,
//! and every other one but the single exception [`UNMODELLED_REFUSAL_OPERATIONS`] names);
//! an operation whose model declares no 403 is NOT judged here, because answering a caller
//! an error its operation does not model would be a different divergence.
//!
//! The decision is therefore taken for EVERY operation of this service rather than for the
//! few whose handler asks for it: it is a property of the request's identity, the action it
//! names and the resource it addresses rather than of what the handler goes on to do, so it
//! is taken ONCE, in the layer every operation is served behind
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
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a policy `iam:PutRolePolicy` wrote takes
//!    effect on the next request, deleting it lifts it again, and this service reads an
//!    identity's permissions exactly as IAM's own authorizer does;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split every
//! other gate in this tree makes (AWS Certificate Manager, Amazon OpenSearch Service,
//! Billing and Cost Management Data Exports, Amazon EVS, CloudWatch RUM, Amazon Cognito
//! Identity), and the lookups stay in the service crate for the reason those keep theirs
//! there: hoisting them into `service-common` would make every service that depends on
//! that crate link the IAM and STS service crates.
//!
//! Amazon Bedrock's own resource policies (`PutResourcePolicy`) are attached to shapes no
//! operation gated here acts on, so the decision here is the identity-based one alone.
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
//!   boundary the store DOES hold still caps the grant. An attachment naming a
//!   CUSTOMER-managed policy the account does not hold grants nothing, because the account
//!   is the authority on its own policies.
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
/// for such a request.
pub const ANY_RESOURCE: &str = "*";

/// The wire name of the refusal this gate answers with, and the status the model gives it:
/// `AccessDeniedException`, `@error("client")` at `@httpError(403)`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
pub const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The media type this restJson1 service renders an error in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The ONE operation of this service whose model declares no `AccessDeniedException`:
/// `GetUseCaseForModelAccess`, whose errors are `ResourceNotFoundException`,
/// `ThrottlingException`, `ValidationException` and `InternalServerException`
/// (`smithy_query(query="operation",operation="GetUseCaseForModelAccess")`).
///
/// A caller of it is left unjudged rather than answered an error its operation does not
/// model: that would be a different divergence from the one this gate removes.
pub const UNMODELLED_REFUSAL_OPERATIONS: &[&str] = &["GetUseCaseForModelAccess"];

/// Whether the layer every operation is served behind judges `operation` itself.
#[must_use]
pub fn layer_judges(operation: &str) -> bool {
    !UNMODELLED_REFUSAL_OPERATIONS.contains(&operation)
}

/// Whether `value` is an ARN of THIS service's own namespace — the resource a request
/// carrying it addresses.
///
/// The parse is the crate's one ARN parse (`service_common::arn`), with the vendor field
/// compared against [`crate::state::ARN_NAMESPACE`], so no two services disagree about
/// which ARNs are theirs.
#[must_use]
pub fn names_a_resource_of_this_service(value: &str) -> bool {
    ::service_common::arn::parse_of(value, crate::state::ARN_NAMESPACE).is_some()
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at
/// its modelled 403, serialized through the SAME protocol a handler-returned error of that
/// shape goes through, so a refusal taken in the layer and one taken in a handler are
/// indistinguishable on the wire.
///
/// The hand-built fallback carries the same three things a client branches on — the status,
/// the shape in `x-amzn-errortype` and the `__type` body — for the one case the serializer
/// can fail: a value it cannot render.
///
/// CROSS-REFERENCE: `service_common::event_stream::error_response` renders a modelled error
/// the same way, for an operation that holds its error as a `dyn OperationError`. This gate
/// holds the error shape itself rather than an operation's error channel (the layer stands
/// in front of 107 of them), and its fallback keeps the refusal's own status and code where
/// that one answers a 500 — so the two are not one renderer today. Hoisting a renderer that
/// takes the shape, the schema, the code and the status is the change that would merge them,
/// and it belongs to whichever change needs it in a second service.
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

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// This service's IAM action prefix: an action is `bedrock:<OperationName>` ("Actions
/// defined by Amazon Bedrock", IAM Service Authorization Reference), which is this
/// service's `endpointPrefix` / `sigv4Name` (`smithy_query(query="service_info")`) — the
/// same string [`crate::state::ARN_NAMESPACE`] spells into every ARN this service builds.
pub const ACTION_PREFIX: &str = crate::state::ARN_NAMESPACE;

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request NAMES.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise, which each operation returns as its
/// own `AccessDeniedException`. Call it BEFORE the store is read or written, and with the
/// ARN the request carried rather than one resolved from a stored record, so the decision
/// does not depend on whether that resource exists — which is also what keeps a refused
/// call from telling an unauthorized caller whether it does.
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
    authorize_action(engine, ctx, &format!("{ACTION_PREFIX}:{operation}"), resource)
}

/// `iam:PassRole`, the IAM action a principal has to hold to hand a service a role that
/// service will then take on ("Granting a user permissions to pass a role to an AWS
/// service", IAM User Guide).
///
/// It is an action of IAM rather than of this service, so it is not spelled with
/// [`ACTION_PREFIX`]: what IAM evaluates is `iam:PassRole` on the ROLE being passed.
pub const IAM_PASS_ROLE: &str = "iam:PassRole";

/// Whether the IAM boundary in force on this caller WITHHOLDS the `iam:PassRole` of
/// `role_arn` — the account's pass-role boundary over the role a request asks this service
/// to take on.
///
/// Handing a service a role is a permission of the CALLER rather than a property of the
/// role: the principal making the call has to be allowed `iam:PassRole` for that very role,
/// and the account bounds which of its roles each of its principals may hand out, so two
/// accounts can answer the same well-formed, same-account, correctly-trusted destination
/// differently. The decision is the identity-based one every action of this service is
/// judged by ([`authorize`]) asked for the IAM action on the role's own ARN, so a policy
/// `iam:PutRolePolicy` wrote takes effect on the next request and removing it lifts the
/// refusal again.
///
/// `false` — withhold nothing — for every caller this gate does not bound (the module
/// docs: the account's own published credential, a principal holding a grant this emulator
/// cannot read, an engine with no IAM store), because a refusal has to be grounded in a
/// document that was READ.
#[must_use]
pub fn pass_role_is_withheld<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    role_arn: &str,
) -> bool {
    authorize_action(engine, ctx, IAM_PASS_ROLE, role_arn).is_err()
}

/// The identity-based decision [`authorize`] and [`pass_role_is_withheld`] share, taken for
/// the IAM `action` spelled whole against the `resource` it names.
fn authorize_action<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, so the decision below rests on the action and the resource.
    let request = iam::Request::new(action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            action,
            resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    let granted = iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            action,
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
        return Err(not_authorized(&caller.arn, action, resource, &reason));
    }
    Ok(())
}

/// The ARN of the resource of `resource_type` that `resource_id` names, in the Region the
/// caller signed for — built through the same builder every resource ARN this service
/// mints comes from, so what IAM evaluates is spelled exactly as the ARNs of the resources
/// the action reaches are, and a policy scoped to them covers it.
fn family_arn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    resource_type: &str,
    resource_id: &str,
) -> String {
    ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        &crate::state::request_region(ctx.parts()),
        &format!("{resource_type}/{resource_id}"),
    )
}

/// The ARN a request that names no single resource of its own is judged against: every
/// resource of `resource_type` in the Region the caller signed for.
///
/// A create and a list name no one resource — the id a create would mint does not exist
/// while the create is still being authorized — so what IAM evaluates is the collection
/// the action reaches, written with IAM's own resource wildcard.
#[must_use]
pub fn collection_arn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    resource_type: &str,
) -> String {
    family_arn(ctx, resource_type, ANY_RESOURCE)
}

/// The ARN an operation is judged against, given the ARN of one of this service's own
/// resources that the request carries (`named`, when it carries one) and the request `uri`
/// it was addressed to.
///
/// IAM evaluates an action against the resource the request addresses, and a request
/// addresses one in exactly four ways:
///
/// * it NAMES it, by spelling the ARN of a resource of the family its URI space addresses —
///   `GetCustomModel$modelIdentifier` spelled as a `custom-model` ARN,
///   `DeleteResourcePolicy$resourceArn`. The resource TYPE has to be that family's, because
///   several operations carry an ARN of ANOTHER family beside their target —
///   `CreateModelInvocationJob$modelId` admits a `foundation-model`, `custom-model` or
///   `inference-profile` ARN, `ListModelCopyJobs$sourceModelArnEquals` a source model — and
///   the model a job runs is not the resource the job-creating action acts on;
/// * it carries that ONE resource's own ARN resource id in its URI instead of the whole ARN
///   ([`ID_ADDRESSED_FAMILIES`]) — the bare id the identifier member's modelled form admits
///   beside the ARN — so the resource it addresses is the member that id names, and its ARN
///   is built from the id by this service's own builder for the family ([`family_arn`]);
/// * it names no single member of that family — a create, a listing, a read addressed by a
///   NAME — and so addresses the family's whole collection in this account and Region
///   ([`collection_arn`]): the id a create would mint does not exist while the create is
///   still being authorized, and the id behind a name is one this service minted, so
///   neither is an ARN this gate could resolve without reading the store;
/// * its URI space addresses no family of its own and the request carries the TARGET's ARN
///   instead ([`ARN_ADDRESSED_SPACES`]) — the tagging and resource-policy operations, whose
///   target may be a resource of any taggable family.
///
/// Anything else is judged against [`ANY_RESOURCE`], as IAM writes it for a request that
/// addresses no one resource: an account-level setting, a configuration, and an ARN in a
/// request whose space addresses neither that ARN's family nor the ARN the caller named.
#[must_use]
pub fn judged_resource(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    named: Option<String>,
    uri: &str,
) -> String {
    match addressed(uri) {
        Addressed::Family(resource_types) => {
            // The ARN the request spells, only where it is one of THIS family's.
            if let Some(arn) = named {
                if resource_types
                    .iter()
                    .any(|resource_type| crate::state::names_arn_of(&arn, resource_type))
                {
                    return arn;
                }
            }
            // Else the one member the URI addresses by its own id, where it does.
            if let Some((resource_type, resource_id)) = id_addressed_member(uri) {
                return family_arn(ctx, resource_type, resource_id);
            }
            // Else the family's collection, written with the type its own resources carry
            // first.
            match resource_types.first() {
                Some(resource_type) => collection_arn(ctx, resource_type),
                None => ANY_RESOURCE.to_string(),
            }
        }
        Addressed::NamedByTheRequest => named.unwrap_or_else(|| ANY_RESOURCE.to_string()),
        Addressed::NoResource => ANY_RESOURCE.to_string(),
    }
}

/// The ARN resource type and resource id of the ONE member `uri` addresses by that
/// member's own id, or `None` for a request that addresses no single member that way.
///
/// The id sits at the segment after the space ([`ID_ADDRESSED_FAMILIES`]), and is read as
/// an id only where it is spelled as THAT family spells the resource id inside its ARNs
/// ([`IdAddressedFamily::admits_id`]) — so the percent-encoded ARN a caller may put in the
/// same label, a sub-resource segment, and a label of any other form fall through to the
/// ARN the request named or to the family's collection rather than being read as an id, and
/// the ARN this gate builds is always one a resource of that family could carry.
fn id_addressed_member(uri: &str) -> Option<(&'static str, &str)> {
    let space = path_segment(uri, 0);
    let family = ID_ADDRESSED_FAMILIES
        .iter()
        .find(|family| family.space == space)?;
    let identifier = path_segment(uri, 1);
    family
        .admits_id(identifier)
        .then_some((family.resource_type, identifier))
}

/// One URI space whose single-resource operations address that resource by its OWN ARN
/// resource id.
struct IdAddressedFamily {
    /// The URI space: the first path segment of those operations' `@http` URIs, with the
    /// id at the segment right after it.
    space: &'static str,
    /// The ARN resource type the id belongs to — the type this service's own ARN builder
    /// for that family spells.
    resource_type: &'static str,
    /// The lengths, inclusive, the model admits for that resource id.
    id_length: (usize, usize),
}

impl IdAddressedFamily {
    /// Whether `value` is spelled as this family spells the resource id inside its ARNs: a
    /// run of lowercase letters and digits of a length the model admits, so an ARN built
    /// from it is one a resource of this family could carry.
    fn admits_id(&self, value: &str) -> bool {
        let (shortest, longest) = self.id_length;
        let length = value.chars().count();
        length >= shortest
            && length <= longest
            && value
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    }
}

/// Every URI space whose single-resource operations address that resource by its OWN ARN
/// resource id, with the ARN resource type the id belongs to and the lengths the model
/// admits for it.
///
/// A space is here exactly when the identifier member its operations carry declares the
/// bare ARN resource id as an alternative beside the whole ARN, so a value that is no ARN
/// IS that resource's id and the ARN follows from it without reading the store:
///
/// * `GuardrailIdentifier` — `^(([a-z0-9]+)|(arn:…:guardrail/[a-z0-9]+))$`, whose id is a
///   `GuardrailId`: `^[a-z0-9]+$` with `@length(max: 64)`;
/// * `ModelInvocationJobIdentifier` — `^((arn:…:model-invocation-job/)?[a-z0-9]{12})$`;
/// * `AdvancedPromptOptimizationJobIdentifier` — the same shape over that family
///   (`smithy_query(query="operation",operation="GetGuardrail,GetModelInvocationJob,GetAdvancedPromptOptimizationJob")`).
///
/// Every OTHER space of [`RESOURCE_FAMILIES`] is absent for one of two stated reasons, and
/// a request there keeps the resource it had:
///
/// * its identifier admits nothing but the ARN (`EvaluationJobIdentifier`), or the label IS
///   the ARN (`automated-reasoning-policies`, `model-copy-jobs`, `prompt-routers`), so the
///   ARN the request named is already the resource it addresses;
/// * its bare alternative is a resource NAME (`ModelIdentifier` and
///   `ImportedModelIdentifier`'s `([0-9a-zA-Z][_-]?)+`, `ProvisionedModelId`,
///   `CustomModelDeploymentIdentifier`, `ModelImportJobIdentifier`'s job name), or its ARN
///   carries a second segment this gate cannot know (`custom-model/<base model>/<id>`,
///   `model-customization-job/<base model>/<id>`), or the id belongs to either of two
///   resource types (`InferenceProfileIdentifier`) — in each case the id this service
///   minted is not derivable from the label, so the action reaches the family's collection.
const ID_ADDRESSED_FAMILIES: &[IdAddressedFamily] = &[
    IdAddressedFamily {
        space: "advanced-prompt-optimization-jobs",
        resource_type: "advanced-prompt-optimization-job",
        id_length: (12, 12),
    },
    IdAddressedFamily {
        space: "guardrails",
        resource_type: "guardrail",
        id_length: (1, 64),
    },
    IdAddressedFamily {
        space: "model-invocation-job",
        resource_type: "model-invocation-job",
        id_length: (12, 12),
    },
];

/// What the URI space a request was sent to addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addressed {
    /// A family of this service's OWN resources, named by these ARN resource types
    /// ([`RESOURCE_FAMILIES`]).
    Family(&'static [&'static str]),
    /// The resource the REQUEST names, of whichever family it belongs to
    /// ([`ARN_ADDRESSED_SPACES`]).
    NamedByTheRequest,
    /// No resource of this service: a space in [`RESOURCE_FREE_SPACES`], or a target this
    /// model binds no operation to.
    NoResource,
}

/// How `uri` addresses a resource, read off the FIRST path segment of the request target
/// ([`path_segment`], which reads both the origin and the absolute form of one).
fn addressed(uri: &str) -> Addressed {
    let space = path_segment(uri, 0);
    if let Some((_, resource_types)) = RESOURCE_FAMILIES
        .iter()
        .find(|(known, _)| *known == space)
    {
        return Addressed::Family(resource_types);
    }
    if ARN_ADDRESSED_SPACES.contains(&space) {
        return Addressed::NamedByTheRequest;
    }
    // Every remaining space this service SERVES holds no resource of its own
    // ([`RESOURCE_FREE_SPACES`]); a target outside this model's own spaces reaches no
    // operation at all, so the framework answers it as a routing failure. The invariant is
    // about the MODEL rather than about any request, so no caller can reach this assertion,
    // and `tests::every_uri_space_of_this_service_is_classified` holds it in every build.
    debug_assert!(
        RESOURCE_FREE_SPACES.contains(&space) || !crate::ROUTE_PATH_SEGMENTS.contains(&space),
        "every URI space this service serves is classified"
    );
    Addressed::NoResource
}

/// The `index`-th path segment of a request target, in either of the two forms one arrives
/// in, or the empty string where the target carries no such segment.
///
/// Both readings are the shared crate's: the target is reduced to its PATH by the one
/// reader of a whole request URI (`service_common::uri_path`, which strips the scheme, the
/// authority, the query string and the fragment) and split by the one segment reader
/// (`service_common::nth_path_segment`) — so the origin form a client sends
/// (`/guardrails?maxResults=1`) and the absolute form the AWS SDK sends
/// (`https://bedrock.<region>.amazonaws.com/guardrails`) read the same, and this crate grows
/// no private copy of either.
fn path_segment(uri: &str, index: usize) -> &str {
    ::service_common::nth_path_segment(::service_common::uri_path(uri), index).unwrap_or_default()
}

/// The URI spaces whose operations address a resource of ANY family, named by the request
/// itself: the tagging operations, whose `resourceARN` is "the Amazon Resource Name (ARN) of
/// the resource" being tagged, and the resource-policy operations, whose `resourceArn` is
/// the resource the policy is attached to
/// (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource,PutResourcePolicy,GetResourcePolicy,DeleteResourcePolicy")`).
///
/// They map to no entry of [`RESOURCE_FAMILIES`] because the family is not a property of the
/// space: it is whichever family the ARN in the request belongs to, which is exactly the
/// resource IAM evaluates such an action against.
const ARN_ADDRESSED_SPACES: &[&str] = &[
    "listTagsForResource",
    "resource-policy",
    "tagResource",
    "untagResource",
];

/// Every URI space of this service that addresses a family of its OWN resources, and the
/// ARN resource types that family's members are named by — the FIRST being the one a
/// resource this service mints in that space carries, so it is the type the family's
/// collection is written with.
///
/// The spaces are the first segments of this model's `@http` URIs (`OPERATIONS.md`, the
/// same list `crate::ROUTE_PATH_SEGMENTS` claims a request by, and
/// [`tests::every_uri_space_of_this_service_is_classified`] holds this table to it), and
/// every resource type is one [`crate::state`]'s ARN builder for that family spells — so
/// the collection a request is judged against is written exactly as the ARNs of the
/// resources it reaches are, and a policy scoped to this account's resources of that type
/// covers it.
///
/// A space whose operations address no resource of this service is in
/// [`RESOURCE_FREE_SPACES`]; one whose target is the ARN the REQUEST carries is in
/// [`ARN_ADDRESSED_SPACES`].
const RESOURCE_FAMILIES: &[(&str, &[&str])] = &[
    (
        "advanced-prompt-optimization-job",
        &["advanced-prompt-optimization-job"],
    ),
    (
        "advanced-prompt-optimization-jobs",
        &["advanced-prompt-optimization-job"],
    ),
    (
        "automated-reasoning-policies",
        &["automated-reasoning-policy"],
    ),
    ("create-foundation-model-agreement", &[FOUNDATION_MODEL]),
    ("custom-models", &[crate::state::CUSTOM_MODEL_RESOURCE_TYPE]),
    ("delete-foundation-model-agreement", &[FOUNDATION_MODEL]),
    ("evaluation-job", &["evaluation-job"]),
    ("evaluation-jobs", &["evaluation-job"]),
    ("foundation-model-availability", &[FOUNDATION_MODEL]),
    ("foundation-models", &[FOUNDATION_MODEL]),
    ("guardrails", &["guardrail"]),
    ("imported-models", &["imported-model"]),
    // This space holds BOTH profile kinds: a create mints the application profile
    // (`crate::state::application_inference_profile_arn`), while the profiles AWS publishes
    // are system-defined ones (`crate::state::system_defined_inference_profile_arn`), and a
    // read or a delete may name either.
    (
        "inference-profiles",
        &["application-inference-profile", "inference-profile"],
    ),
    ("list-foundation-model-agreement-offers", &[FOUNDATION_MODEL]),
    (
        "model-customization",
        &[crate::state::CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE],
    ),
    ("model-copy-jobs", &["model-copy-job"]),
    ("model-customization-jobs", &["model-customization-job"]),
    ("model-import-jobs", &["model-import-job"]),
    ("model-invocation-job", &["model-invocation-job"]),
    ("model-invocation-jobs", &["model-invocation-job"]),
    // Every prompt-router ARN this service mints spells `default-prompt-router/<id>`, as
    // `PromptRouterArn` does (`crate::state::prompt_router_arn`); the taggable form of the
    // same resource is `prompt-router/<id>`, which a caller may name it by.
    (
        "prompt-routers",
        &["default-prompt-router", "prompt-router"],
    ),
    ("provisioned-model-throughput", &["provisioned-model"]),
    ("provisioned-model-throughputs", &["provisioned-model"]),
];

/// The URI spaces whose operations address no resource of this service, so an action of
/// theirs is judged against [`ANY_RESOURCE`], as IAM writes it for such a request: the
/// account-level settings and configurations (`/data-retention`, `/logging/modelinvocations`,
/// `/use-case-for-model-access`, `/enforcedGuardrailsConfiguration`), and
/// `/marketplace-model`, whose endpoints are named by an Amazon SageMaker ARN rather than
/// by one of this service's (`crate::state::marketplace_model_endpoint_arn`).
///
/// Stated rather than left to the fall-through, so a space this model adds later cannot
/// become an unjudged default without a case saying so
/// ([`tests::every_uri_space_of_this_service_is_classified`]).
const RESOURCE_FREE_SPACES: &[&str] = &[
    "data-retention",
    "enforcedGuardrailsConfiguration",
    "logging",
    "marketplace-model",
    "use-case-for-model-access",
];

/// The ARN resource type a foundation model is named by: AWS publishes it, so its ARN
/// carries no account, and the collection a request about one is judged against is this
/// Region's.
const FOUNDATION_MODEL: &str = "foundation-model";

/// The refusal IAM states an authorization failure with: who was refused, which action,
/// on which resource, and which of IAM's rules refused it. Every value in it comes from
/// the request and from the stored policies — the principal from the credential the
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
    AccessDeniedException {
        message: Some(format!(
            "User: {principal} is not authorized to perform: {action}{on_resource} {reason}"
        )),
    }
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
    ///
    /// `principal_policies` is where the collection rule itself lives — a role's or user's
    /// inline policies, the default version of each managed policy attached to it, and for
    /// a user the policies of the groups it belongs to — so this service reads an
    /// identity's permissions exactly as IAM's own authorizer does.
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
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            caller.read_policies_of(&store, &held.arn.clone());
        }
    } else if iam_user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role
/// is the first segment after `assumed-role/`.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "sts")?;
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
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

    /// RULE: a request that names no ARN of the family it addressed is judged against that
    /// family's collection, and against every resource only where the space holds no
    /// resource of this service.
    #[test]
    fn a_uri_space_names_the_resource_family_an_action_is_judged_against() {
        // (case, the request target, what that space addresses)
        let rows: &[(&str, &str, Addressed)] = &[
            ("a create", "/guardrails", Addressed::Family(&["guardrail"])),
            (
                "a listing with a query",
                "/guardrails?maxResults=1",
                Addressed::Family(&["guardrail"]),
            ),
            (
                "a read addressed by a bare id",
                "/guardrails/abcd1234",
                Addressed::Family(&["guardrail"]),
            ),
            (
                "an absolute request target",
                "https://bedrock.us-west-2.amazonaws.com/model-invocation-job",
                Addressed::Family(&["model-invocation-job"]),
            ),
            (
                "the plural space of the same family",
                "/model-invocation-jobs",
                Addressed::Family(&["model-invocation-job"]),
            ),
            (
                "the singular space of the evaluation jobs",
                "/evaluation-job/abcd1234/stop",
                Addressed::Family(&["evaluation-job"]),
            ),
            (
                "a family whose space nests the resource",
                "/model-customization/custom-model-deployments",
                Addressed::Family(&[crate::state::CUSTOM_MODEL_DEPLOYMENT_RESOURCE_TYPE]),
            ),
            (
                "a throughput space, whose resources are named provisioned models",
                "/provisioned-model-throughput",
                Addressed::Family(&["provisioned-model"]),
            ),
            (
                "a space holding two kinds of the same resource",
                "/inference-profiles",
                Addressed::Family(&["application-inference-profile", "inference-profile"]),
            ),
            // The spaces whose target is the ARN the request carries, whatever family it
            // belongs to.
            (
                "a tagging operation",
                "/tagResource",
                Addressed::NamedByTheRequest,
            ),
            (
                "a resource-policy operation",
                "/resource-policy/arn%3Aaws%3Abedrock%3Aus-west-2%3A000000000000%3Aguardrail%2Fabc",
                Addressed::NamedByTheRequest,
            ),
            // And the spaces that address no resource of this service at all.
            (
                "an account-level setting",
                "/data-retention",
                Addressed::NoResource,
            ),
            (
                "the logging configuration",
                "/logging/modelinvocations",
                Addressed::NoResource,
            ),
            (
                "a target this model does not bind",
                "/nothing-here",
                Addressed::NoResource,
            ),
            ("the root", "/", Addressed::NoResource),
        ];
        for (case, target, expected) in rows {
            let read = addressed(target);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: every resource type this table maps a family to is one this service's OWN ARN
    /// builder spells for a resource of that family, so the collection an action is judged
    /// against is written exactly as the ARNs of the resources it reaches are — and a
    /// policy scoped to this account's resources of that type covers it.
    ///
    /// Asserted by minting an ARN through the builder each family's resources are minted by
    /// and reading its resource type back ([`crate::state::names_arn_of`]), so a family
    /// mapped to the WRONG type fails here.
    #[test]
    fn every_mapped_family_is_named_by_the_type_its_own_arns_carry() {
        let region = "us-west-2";
        let id = "0123456789ab";
        // One ARN per family, minted by `crate::state`'s own builder for it.
        let minted: &[(&str, String)] = &[
            (
                "advanced-prompt-optimization-jobs",
                crate::state::advanced_prompt_optimization_job_arn(region, id),
            ),
            (
                "automated-reasoning-policies",
                crate::state::automated_reasoning_policy_arn(region, id),
            ),
            (
                "custom-models",
                crate::state::custom_model_arn(region, "amazon.base-v1:0", id),
            ),
            (
                "evaluation-jobs",
                crate::state::evaluation_job_arn(region, id),
            ),
            ("guardrails", crate::state::guardrail_arn(region, id)),
            (
                "imported-models",
                crate::state::imported_model_arn(region, id),
            ),
            (
                "inference-profiles",
                crate::state::application_inference_profile_arn(region, id),
            ),
            (
                "inference-profiles",
                crate::state::system_defined_inference_profile_arn(region, id),
            ),
            (
                "model-customization",
                crate::state::custom_model_deployment_arn(region, id),
            ),
            (
                "model-copy-jobs",
                crate::state::model_copy_job_arn(region, id),
            ),
            (
                "model-customization-jobs",
                crate::state::model_customization_job_arn(region, "amazon.base-v1:0", id),
            ),
            (
                "model-import-jobs",
                crate::state::model_import_job_arn(region, id),
            ),
            (
                "model-invocation-job",
                crate::state::model_invocation_job_arn(region, id),
            ),
            ("prompt-routers", crate::state::prompt_router_arn(region, id)),
            (
                "provisioned-model-throughput",
                crate::state::provisioned_model_arn(region, id),
            ),
            (
                "foundation-models",
                crate::state::foundation_model_arn(region, "amazon.base-v1:0"),
            ),
        ];
        for (space, arn) in minted {
            let Addressed::Family(resource_types) = addressed(&format!("/{space}")) else {
                panic!("case {space:?}: this space must address a resource family");
            };
            assert!(
                resource_types
                    .iter()
                    .any(|resource_type| crate::state::names_arn_of(arn, resource_type)),
                "case {space:?}: {arn:?} is of none of the mapped types {resource_types:?}"
            );
        }
    }

    /// RULE: every URI space this service serves is CLASSIFIED — it addresses a family of
    /// this service's resources, or the resource the request names, or no resource at all —
    /// so a space added by a later model refresh cannot silently fall through to the
    /// resource that no scoped policy matches.
    #[test]
    fn every_uri_space_of_this_service_is_classified() {
        for space in crate::ROUTE_PATH_SEGMENTS {
            let reached = addressed(&format!("/{space}"));
            let resource_free = RESOURCE_FREE_SPACES.contains(space);
            assert_eq!(
                reached == Addressed::NoResource,
                resource_free,
                "case {space:?}: a space reads as addressing no resource exactly when it is \
                 one of the spaces stated to hold none, got {reached:?} against \
                 {resource_free}"
            );
        }
    }

    /// RULE: the action this gate asks about is `bedrock:<OperationName>`, so a policy
    /// scoped to this service's actions covers every operation of it and nothing outside
    /// it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"bedrock:*","Resource":"*"}]}"#;
        for operation in ["CreateCustomModel", "GetCustomModel", "ListCustomModels"] {
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

    /// RULE: a session ARN names the role whose policies are read, an IAM user ARN the
    /// user whose policies are; every other principal form names neither.
    #[test]
    fn a_principal_arn_names_the_entity_its_policies_are_read_from() {
        // (case, the ARN, the role it names, the user it names)
        let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/bedrock-caller/session-1",
                Some("bedrock-caller"),
                None,
            ),
            (
                "a session name carrying a slash",
                "arn:aws:sts::000000000000:assumed-role/bedrock-caller/one/two",
                Some("bedrock-caller"),
                None,
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/bedrock-user",
                None,
                Some("bedrock-user"),
            ),
            (
                "an IAM user under a path",
                "arn:aws:iam::000000000000:user/team/bedrock-user",
                None,
                Some("bedrock-user"),
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
                "case {case:?}: expected the role {role:?}, got {:?}",
                assumed_role_name(arn)
            );
            assert_eq!(
                iam_user_name(arn),
                *user,
                "case {case:?}: expected the user {user:?}, got {:?}",
                iam_user_name(arn)
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource the request
    /// addressed and which of IAM's rules refused — every part taken from the request and
    /// the stored policies — and it names no resource for an action that addresses none.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = "arn:aws:sts::000000000000:assumed-role/bedrock-caller/session-1";
        let model = "arn:aws:bedrock:us-west-2:000000000000:custom-model/imported/0123456789ab";
        // (case, the operation, the resource it addressed, the clause the sentence carries)
        let rows: &[(&str, &str, &str, String)] = &[
            (
                "a resource-scoped action",
                "GetCustomModel",
                model,
                format!(" on resource: {model}"),
            ),
            (
                "an action that addresses no resource",
                "GetModelInvocationLoggingConfiguration",
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

    /// RULE: a request that carries ONE resource's own ARN resource id in its URI is judged
    /// against that resource, so the id is read off the segment after the space — and only
    /// where it is spelled as THAT family spells a resource id, never from an ARN label, a
    /// sub-resource segment, or a label of any other form.
    #[test]
    fn a_uri_carrying_a_resource_id_addresses_that_one_resource() {
        let encoded_arn = "arn%3Aaws%3Abedrock%3Aus-west-2%3A000000000000%3Aguardrail%2Fabc123";
        /// One row: the case, the request target, and the resource type and id it
        /// addresses.
        type Row<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);
        // (case, the request target, the type and id it addresses)
        let rows: &[Row<'_>] = &[
            (
                "a read addressed by a bare guardrail id",
                "/guardrails/abc123",
                Some(("guardrail", "abc123")),
            ),
            (
                "the same id with a query string",
                "/guardrails/abc123?guardrailVersion=DRAFT",
                Some(("guardrail", "abc123")),
            ),
            (
                "an absolute request target",
                "https://bedrock.us-west-2.amazonaws.com/guardrails/abc123",
                Some(("guardrail", "abc123")),
            ),
            (
                "a batch inference job read",
                "/model-invocation-job/0123456789ab",
                Some(("model-invocation-job", "0123456789ab")),
            ),
            (
                "a sub-resource of that job",
                "/model-invocation-job/0123456789ab/stop",
                Some(("model-invocation-job", "0123456789ab")),
            ),
            (
                "an optimization job read",
                "/advanced-prompt-optimization-jobs/0123456789ab",
                Some((
                    "advanced-prompt-optimization-job",
                    "0123456789ab",
                )),
            ),
            ("a create, which names no member", "/guardrails", None),
            ("a listing, which names none either", "/guardrails?maxResults=1", None),
            (
                "a label carrying the whole ARN, which the request NAMES instead",
                &format!("/guardrails/{encoded_arn}"),
                None,
            ),
            (
                "a label of a form no resource id of the family takes",
                "/guardrails/Not-An-Id",
                None,
            ),
            (
                "a label longer than the family's id",
                &format!("/guardrails/{}", "a".repeat(65)),
                None,
            ),
            (
                "a label shorter than a fixed-length family's id",
                "/model-invocation-job/abc",
                None,
            ),
            (
                "a batch-delete, whose space addresses its targets by the request's body",
                "/advanced-prompt-optimization-job/batch-delete",
                None,
            ),
            (
                "a family whose label may be a resource NAME",
                "/imported-models/my-model",
                None,
            ),
            (
                "a space holding no resource of this service",
                "/logging/modelinvocations",
                None,
            ),
        ];
        for (case, target, expected) in rows {
            let read = id_addressed_member(target);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: every space that addresses its resources by their own id is a space of this
    /// service's own resource families, and the ARN this gate builds from a label is the
    /// very ARN that family's own builder mints for the same id — so a policy scoped to the
    /// resource the store holds covers the request that addresses it by id.
    #[test]
    fn every_id_addressed_family_builds_the_arn_its_own_resources_carry() {
        let region = "us-west-2";
        let id = "0123456789ab";
        // (the space, the ARN that family's own builder in `crate::state` mints for `id`)
        let minted: &[(&str, String)] = &[
            (
                "advanced-prompt-optimization-jobs",
                crate::state::advanced_prompt_optimization_job_arn(region, id),
            ),
            ("guardrails", crate::state::guardrail_arn(region, id)),
            (
                "model-invocation-job",
                crate::state::model_invocation_job_arn(region, id),
            ),
        ];
        assert_eq!(
            minted.len(),
            ID_ADDRESSED_FAMILIES.len(),
            "every id-addressed family needs a row here, got {} rows for {} families",
            minted.len(),
            ID_ADDRESSED_FAMILIES.len()
        );
        for (space, arn) in minted {
            let family = ID_ADDRESSED_FAMILIES
                .iter()
                .find(|family| family.space == *space)
                .expect("the row names an id-addressed family");
            let Addressed::Family(resource_types) = addressed(&format!("/{space}")) else {
                panic!("case {space:?}: this space must address a resource family");
            };
            assert!(
                resource_types.contains(&family.resource_type),
                "case {space:?}: {:?} is none of the family's types {resource_types:?}",
                family.resource_type
            );
            assert!(
                family.admits_id(id),
                "case {space:?}: {id:?} must be a resource id of this family"
            );
            let built = ::engine::emulator_arn(
                crate::state::ARN_NAMESPACE,
                region,
                &format!("{}/{id}", family.resource_type),
            );
            assert_eq!(
                &built, arn,
                "case {space:?}: expected the builder's {arn:?}, got {built:?}"
            );
        }
    }
}
