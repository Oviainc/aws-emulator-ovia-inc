// SPDX-License-Identifier: CC0-1.0

//! Who a signed request to IAM speaks for, and whether that principal's own
//! identity-based policies — within the boundary that caps them — let it do what it asks.
//!
//! IAM is itself an AWS service whose API is governed by IAM: a call to it is authorized
//! against the calling principal's identity-based policies before the operation reports
//! or changes anything, and the answer is the one IAM's documented evaluation gives — an
//! explicit `Deny` whose statement is about the request wins over every `Allow`, an
//! `Allow` with no `Deny` permits, and a principal with neither is refused (the implicit
//! deny). A refused call is answered HTTP 403 with the error code `AccessDenied`
//! ([`crate::scaffolded::Scaffolded::access_denied`]) and a message built from the three
//! things the decision was made about — the principal, the action and the resource —
//! closed by why the decision went that way. A refusal that READ the documents in force
//! and found none allowing the call reports an evaluation whose details can be read, so
//! that sentence continues with where to read them and under which authorization id
//! ([`not_authorized_with_details`]); a refusal that read no document, and one explained
//! by an explicit `Deny`, end at the reason ([`not_authorized`]).
//!
//! The four pieces, each held in exactly one place:
//!
//! 1. **who** — the IAM user the request's SigV4 access key id belongs to, read out of
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::access_keys`], the same
//!    map `GetUser` and `CreateAccessKey` resolve a nameless caller through;
//! 2. **what it may do** — the identity-based documents in force on that principal, from
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::principal_policies`],
//!    which for a user is the UNION of the documents embedded in and attached to the user
//!    itself and those in force on every group it belongs to, so a policy just put grants
//!    immediately and one just deleted stops granting immediately;
//! 3. **the ceiling on it** — the permissions boundary in force on that principal, from
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::permissions_boundary_document`],
//!    which sets the maximum permissions those documents can grant and grants nothing
//!    itself: a call the boundary does not allow is refused even where they allow it —
//!    and the refusal names the boundary as the document that withheld it — while a call
//!    only the boundary allows is refused as if the boundary had allowed nothing;
//! 4. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of IAM policy documents against `(action, resource)`, so IAM and every
//!    other service handed the same documents decide alike.
//!
//! ## What the decision knows about the request
//!
//! A statement may be written to depend on WHO is calling, or on WHICH resource is
//! addressed, rather than only on what is asked: through a `Condition` clause over a key
//! of the signing principal or of the target, or through a policy VARIABLE in its
//! `Resource` element, which the shared evaluation substitutes from the request's
//! condition context. Since the signing principal is resolved here before anything is
//! decided, and the target is named by the call, that context is built from both
//! ([`request_context`]) and passed with the request, so one document attached to many
//! users scopes itself to each of them, a clause over a principal tag is decided from the
//! tags the user holds at the instant of the call, and a clause over a tag of the
//! addressed resource is decided from the tags that resource holds then.
//!
//! ## The callers that are not IAM users
//!
//! * **A credential no IAM user owns.** The emulator's own published credential, and a
//!   session credential STS minted, are not keys this account's `access_keys` map holds.
//!   They carry no identity-based policies at all, which is the position AWS account root
//!   credentials are in, so such a request is allowed rather than refused on evidence
//!   that does not exist.
//! * **A principal holding a grant this emulator cannot read.** An AWS-managed policy is
//!   owned by AWS, so the account's store holds no document for it and
//!   `principal_policies` reports it as uncarried. A principal with such an attachment is
//!   allowed where the documents that ARE held neither grant nor deny: what that document
//!   grants is unknown here, and refusing on evidence this emulator does not have would
//!   deny access AWS permits. An explicit `Deny` in a document that is held still
//!   refuses, because no grant overturns one. A boundary whose document is not held is
//!   read the same way: it is a ceiling this emulator cannot measure a call against, so
//!   it caps nothing rather than refusing what AWS would permit.

use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::{DelegationRequest, User};
use crate::scaffolded::AccessDenied;

use super::{ComAmazonawsIamAwsidentitymanagementv20100508State as IamState, PrincipalPolicies};

/// `iam:ListGroups`.
pub const LIST_GROUPS: &str = "iam:ListGroups";

/// `iam:ListRoles`.
pub const LIST_ROLES: &str = "iam:ListRoles";

/// `iam:ListUsers`.
pub const LIST_USERS: &str = "iam:ListUsers";

/// `iam:ListPolicies`.
pub const LIST_POLICIES: &str = "iam:ListPolicies";

/// `iam:ListInstanceProfiles`.
pub const LIST_INSTANCE_PROFILES: &str = "iam:ListInstanceProfiles";

/// `iam:ListPoliciesGrantingServiceAccess`.
pub const LIST_POLICIES_GRANTING_SERVICE_ACCESS: &str = "iam:ListPoliciesGrantingServiceAccess";

/// `iam:GenerateServiceLastAccessedDetails`.
pub const GENERATE_SERVICE_LAST_ACCESSED_DETAILS: &str = "iam:GenerateServiceLastAccessedDetails";

/// `iam:GetServiceLastAccessedDetails`.
pub const GET_SERVICE_LAST_ACCESSED_DETAILS: &str = "iam:GetServiceLastAccessedDetails";

/// `iam:GetServiceLastAccessedDetailsWithEntities`.
pub const GET_SERVICE_LAST_ACCESSED_DETAILS_WITH_ENTITIES: &str =
    "iam:GetServiceLastAccessedDetailsWithEntities";

/// `iam:ListOpenIDConnectProviders`.
pub const LIST_OPEN_ID_CONNECT_PROVIDERS: &str = "iam:ListOpenIDConnectProviders";

/// `iam:ChangePassword`.
pub const CHANGE_PASSWORD: &str = "iam:ChangePassword";

/// `iam:CreateGroup`.
pub const CREATE_GROUP: &str = "iam:CreateGroup";

/// `iam:DeleteGroup`.
pub const DELETE_GROUP: &str = "iam:DeleteGroup";

/// `iam:GetGroup`.
pub const GET_GROUP: &str = "iam:GetGroup";

/// `iam:UpdateGroup`.
pub const UPDATE_GROUP: &str = "iam:UpdateGroup";

/// `iam:GetUser`.
pub const GET_USER: &str = "iam:GetUser";

/// `iam:UpdateUser`.
pub const UPDATE_USER: &str = "iam:UpdateUser";

/// `iam:GetUserPolicy`.
pub const GET_USER_POLICY: &str = "iam:GetUserPolicy";

/// `iam:ListGroupsForUser`.
pub const LIST_GROUPS_FOR_USER: &str = "iam:ListGroupsForUser";

/// `iam:ListUserTags`.
pub const LIST_USER_TAGS: &str = "iam:ListUserTags";

/// `iam:ListUserPolicies`.
pub const LIST_USER_POLICIES: &str = "iam:ListUserPolicies";

/// `iam:ListAccessKeys`.
pub const LIST_ACCESS_KEYS: &str = "iam:ListAccessKeys";

/// `iam:ListRoleTags`.
pub const LIST_ROLE_TAGS: &str = "iam:ListRoleTags";

/// `iam:ListPolicyTags`.
pub const LIST_POLICY_TAGS: &str = "iam:ListPolicyTags";

/// `iam:ListInstanceProfileTags`.
pub const LIST_INSTANCE_PROFILE_TAGS: &str = "iam:ListInstanceProfileTags";

/// `iam:TagPolicy`.
pub const TAG_POLICY: &str = "iam:TagPolicy";

/// `iam:UntagPolicy`.
pub const UNTAG_POLICY: &str = "iam:UntagPolicy";

/// `iam:CreatePolicyVersion`.
pub const CREATE_POLICY_VERSION: &str = "iam:CreatePolicyVersion";

/// `iam:DeletePolicyVersion`.
pub const DELETE_POLICY_VERSION: &str = "iam:DeletePolicyVersion";

/// `iam:SetDefaultPolicyVersion`.
pub const SET_DEFAULT_POLICY_VERSION: &str = "iam:SetDefaultPolicyVersion";

/// `iam:GetAccountAuthorizationDetails`.
pub const GET_ACCOUNT_AUTHORIZATION_DETAILS: &str = "iam:GetAccountAuthorizationDetails";

/// `iam:GetDelegationRequest`.
pub const GET_DELEGATION_REQUEST: &str = "iam:GetDelegationRequest";

/// `iam:CreateDelegationRequest`.
pub const CREATE_DELEGATION_REQUEST: &str = "iam:CreateDelegationRequest";

/// `iam:GenerateOrganizationsAccessReport`.
pub const GENERATE_ORGANIZATIONS_ACCESS_REPORT: &str = "iam:GenerateOrganizationsAccessReport";

/// `iam:ListDelegationRequests`.
pub const LIST_DELEGATION_REQUESTS: &str = "iam:ListDelegationRequests";

/// The resource an account-WIDE call is decided against.
///
/// An action that reports the account's whole configuration is not taken on one
/// entity, and the Service Authorization Reference lists no resource type for such an
/// IAM action, so a statement permits it only by covering every resource — which is
/// what this value is matched by.
pub const ACCOUNT_WIDE_RESOURCE: &str = "*";

/// The decision value for a request the policies in force permit — one of the three
/// `PolicyEvaluationDecisionType` declares
/// (`smithy_query(query="shape",shape="PolicyEvaluationDecisionType")`).
pub const DECISION_ALLOWED: &str = "allowed";

/// The decision value for a request a statement explicitly denies.
pub const DECISION_EXPLICIT_DENY: &str = "explicitDeny";

/// The decision value for a request nothing allows and nothing explicitly denies.
pub const DECISION_IMPLICIT_DENY: &str = "implicitDeny";

/// The permission-check result for a caller that covers every ask of a delegation
/// request: "ALLOWED: The caller has sufficient permissions cover all the requested
/// permissions" (`GetDelegationRequestResponse$PermissionCheckResult`). One of the three
/// values `permissionCheckResultType` declares, which
/// `crate::ops::types::PERMISSION_CHECK_RESULT_TYPE_VALUES` records.
pub const PERMISSION_CHECK_ALLOWED: &str = "ALLOWED";

/// The permission-check result for a caller that does not: "DENIED: The caller does not
/// have sufficient permissions to cover all the requested permissions". See
/// [`PERMISSION_CHECK_ALLOWED`].
pub const PERMISSION_CHECK_DENIED: &str = "DENIED";

/// The permission-check result for a check that reaches neither answer: "UNSURE: It is
/// not possible to determine whether the caller has all the permissions needed". See
/// [`PERMISSION_CHECK_ALLOWED`].
pub const PERMISSION_CHECK_UNSURE: &str = "UNSURE";

/// The modelled name of the decision the two sets of documents come to, as
/// `EvaluationResult$EvalDecision` reports it.
///
/// The three values are the three outcomes IAM's evaluation has, so the mapping lives
/// beside the evaluation rather than in each operation that reports one: an explicit
/// `Deny` from either side is [`DECISION_EXPLICIT_DENY`], a grant from either side with
/// no such deny is [`DECISION_ALLOWED`], and neither is the implicit deny.
pub fn decision_name(identity: Decision, resource: Decision) -> &'static str {
    if identity == Decision::Denied || resource == Decision::Denied {
        DECISION_EXPLICIT_DENY
    } else if iam::is_allowed(identity, resource) {
        DECISION_ALLOWED
    } else {
        DECISION_IMPLICIT_DENY
    }
}

/// The condition key whose value is the signing IAM user's friendly name: "`aws:username`
/// — the friendly name of the current user", which for an IAM user is its name without
/// the path it lives under.
const USER_NAME_KEY: &str = "aws:username";

/// The condition key whose value is the signing principal's unique id: "`aws:userid` — the
/// unique identifier of the current user", which for an IAM user is the id the account
/// minted it under.
const USER_ID_KEY: &str = "aws:userid";

/// The prefix the condition key of ONE tag of the signing principal is spelled with:
/// "`aws:PrincipalTag/tag-key` — the tag attached to the principal making the request",
/// read from the principal's CURRENT tags, since nothing in the request carries them.
const PRINCIPAL_TAG_PREFIX: &str = "aws:PrincipalTag/";

/// The prefix the condition key of ONE tag of the resource the request ADDRESSES is
/// spelled with in IAM's own key namespace: "`iam:ResourceTag/tag-key` — works with the
/// tag attached to the IAM resource", the spelling an identity policy about an IAM
/// resource uses.
const IAM_RESOURCE_TAG_PREFIX: &str = "iam:ResourceTag/";

/// The GLOBAL spelling of the same key: "`aws:ResourceTag/tag-key` — compares the tag key-
/// value pair that you specify in the policy with the key-value pair attached to the
/// resource". Both name the tags of the resource the request is taken on, so both are
/// answered from the same tag set.
const AWS_RESOURCE_TAG_PREFIX: &str = "aws:ResourceTag/";

/// The condition keys a request to IAM MODELS about its SIGNING PRINCIPAL, whatever
/// values it then carries for them
/// (`::service_common::iam_authorization::Request::published`).
///
/// The signing principal is resolved before any decision is made, so for each of these
/// this gate can answer whether the request carries a value — including answering that it
/// carries none. [`PRINCIPAL_TAG_PREFIX`] is published as the whole key FAMILY it is,
/// because the principal's tags are known in full: a key of that family the context does
/// not carry is a tag the principal does not hold, not a key this emulator cannot read.
const PUBLISHED_PRINCIPAL_KEYS: &[&str] = &[USER_NAME_KEY, USER_ID_KEY, PRINCIPAL_TAG_PREFIX];

/// The two resource-tag key families, published — like [`PRINCIPAL_TAG_PREFIX`] — only
/// where the addressed resource's tag set is known in full, which is what lets a clause
/// over one of its tags be decided at all.
const PUBLISHED_RESOURCE_TAG_KEYS: &[&str] = &[IAM_RESOURCE_TAG_PREFIX, AWS_RESOURCE_TAG_PREFIX];

/// The condition context a request signed by `user` carries: what IAM knows about the
/// SIGNING principal at the instant of the call, which is where a policy variable's value
/// and an `aws:PrincipalTag/` clause's value both come from.
///
/// The tags are the ones the principal holds NOW — nothing in the request carries them — so
/// retagging the user changes what the same unchanged request decides, and untagging it
/// takes the value away again.
fn principal_context(user: &User) -> Vec<(String, String)> {
    let mut context = vec![
        (USER_NAME_KEY.to_string(), user.user_name.clone()),
        (USER_ID_KEY.to_string(), user.user_id.clone()),
    ];
    for tag in user.tags.iter().flatten() {
        context.push((
            format!("{PRINCIPAL_TAG_PREFIX}{}", tag.key),
            tag.value.clone(),
        ));
    }
    context
}

/// The condition context ONE request carries, and the keys it declares it models: what
/// IAM knows about the signing principal ([`principal_context`]) together with what it
/// knows about the RESOURCE the request is taken on.
///
/// A statement may be written to depend on the target rather than on the caller — an
/// attribute-based grant comparing a tag of the resource to a tag of the principal is the
/// canonical case — so the tags the addressed resource holds at the instant of the call
/// are part of the request the decision is made about, under both spellings of the
/// resource-tag key family. They are read from the store
/// ([`IamState::resource_tags`](super::ComAmazonawsIamAwsidentitymanagementv20100508State::resource_tags)),
/// so retagging either side changes what the same unchanged request decides.
///
/// The two families are DECLARED only for a resource whose tag set the account knows in
/// full; for an ARN naming nothing it holds, a clause over one of its tags stays
/// undecided, which is the fail-closed reading the shared evaluation gives every key this
/// emulator cannot answer for.
fn request_context(
    store: &IamState,
    principal: &[(String, String)],
    resource: &str,
) -> (Vec<(String, String)>, Vec<&'static str>) {
    let mut context = principal.to_vec();
    let mut published = PUBLISHED_PRINCIPAL_KEYS.to_vec();
    if let Some(tags) = store.resource_tags(resource) {
        for tag in tags {
            for prefix in PUBLISHED_RESOURCE_TAG_KEYS {
                context.push((format!("{prefix}{}", tag.key), tag.value.clone()));
            }
        }
        published.extend_from_slice(PUBLISHED_RESOURCE_TAG_KEYS);
    }
    (context, published)
}

/// Whether the principal that signed this request may perform `action` on `resource`.
///
/// `Ok(())` when the call is permitted; `Err` is the refusal the operation answers
/// through its error channel (HTTP 403, error code `AccessDenied`), worded from the
/// principal, the action and the resource the decision was made about — which this
/// spelling names by its ARN. An operation whose request addresses ONE entity by name
/// words it as that entity instead, through [`authorize_named`].
///
/// A READ: the store is only consulted, so authorizing a call changes nothing a later
/// caller sees.
pub fn authorize(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Result<(), AccessDenied> {
    authorize_named(store, ctx, action, resource, resource)
}

/// [`authorize`], with the resource the refusal NAMES (`named_as`) stated separately from
/// the ARN the decision is made against (`resource`).
///
/// A denial reports "the resource the request was made on" ("Troubleshoot access denied
/// error messages", IAM User Guide), and for a request that addresses a single entity by
/// name that is the entity, not the collection it lives in — so such an operation words
/// it as the entity's TYPE followed by the name the request carried (`user Bob`), with the
/// type that disambiguates which entity a name belongs to. The ARN stays what the DECISION
/// is made against, because a policy statement's `Resource` matches ARNs.
pub fn authorize_named(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
    named_as: &str,
) -> Result<(), AccessDenied> {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // A credential this account holds no IAM user for carries no identity-based
    // policies, so there is nothing to judge it on (see the module docs).
    let Some(signer) = signing_principal(store, &access_key_id) else {
        return Ok(());
    };
    let SigningPrincipal {
        arn: principal,
        policies,
        context,
    } = signer;

    let principals = [principal.clone()];
    // What the policies in force are decided against BESIDES the action and the resource:
    // the keys that describe the signing principal and the resource the call is taken on,
    // which a statement reads either through a `Condition` clause or through a policy
    // variable in its `Resource` element.
    let (context, published) = request_context(store, &context, resource);
    let request = iam::Request::new(action, resource, &principals)
        .with_context(&context)
        .with_published_keys(&published);
    let decided = iam::identity_decision(&policies.documents, &request);
    // Whether the documents in force grant the call at all. An attachment whose
    // document the account does not hold is a grant this emulator cannot read, so it
    // cannot be the basis of a refusal — but it cannot overturn one either: an explicit
    // `Deny` in a document that IS held wins over every grant, including one whose
    // content is unknown here.
    let granted = iam::is_allowed(decided, Decision::NotStated)
        || (decided != Decision::Denied && !policies.uncarried.is_empty());
    // What the permissions boundary in force on the principal says about the call. "A
    // permissions boundary … sets the maximum permissions that an identity-based policy
    // can grant to an entity": the boundary CAPS what those policies grant and grants
    // nothing itself, so a call it does not allow is refused even where they allow it,
    // and a call only it allows is refused as if it had allowed nothing. A principal
    // carrying no boundary — or one whose document is not held, a ceiling this emulator
    // cannot read — is capped by nothing, which is the same position as a boundary that
    // allows the call. A call an identity-based policy explicitly DENIES is already
    // refused on that deny, so no ceiling is read for it.
    let capped = if decided == Decision::Denied {
        Decision::Allowed
    } else {
        match store.permissions_boundary_document(&principal) {
            Some(document) => iam::identity_decision(::std::slice::from_ref(&document), &request),
            None => Decision::Allowed,
        }
    };
    if granted && capped == Decision::Allowed {
        return Ok(());
    }

    // Why the call was refused: an explicit `Deny` that is about the request — stated in
    // an identity-based policy or in the boundary, and either wins over every `Allow` —
    // or the absence of an `Allow`, which names the DOCUMENT that withheld it: the
    // boundary where the identity-based policies did allow the call, and the
    // identity-based policies where nothing allowed it at all. They are different facts
    // about the policies in force, and AWS words each of them differently, because they
    // need different fixes.
    let reason = if decided == Decision::Denied {
        "with an explicit deny in an identity-based policy".to_string()
    } else if capped == Decision::Denied {
        "with an explicit deny in a permissions boundary".to_string()
    } else if granted {
        format!("because no permissions boundary allows the {action} action")
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    // Audit record for the refusal: the acting principal, what it asked for and the
    // decision, so the refusal is reviewable after the fact. The policy documents
    // themselves are not logged.
    ::tracing::info!(
        audit = true,
        outcome = "refused_access_denied",
        principal = %principal,
        action = %action,
        resource = %resource,
        explicit_deny = decided == Decision::Denied || capped == Decision::Denied,
        "the policies in force on the caller do not permit this call"
    );
    // A refusal that WITHHELD a grant — the documents in force were read and none of them
    // allowed the call — reports an evaluation there are details to read, so it closes
    // with the pointer at those details. A refusal that read no document at all has no
    // evaluation to point at, and one explained by an explicit `Deny` is explained by that
    // statement, so each of those ends at the reason.
    if decided == Decision::Denied || capped == Decision::Denied || policies.documents.is_empty() {
        return Err(not_authorized(&principal, action, named_as, &reason));
    }
    Err(not_authorized_with_details(
        &principal,
        action,
        named_as,
        &reason,
        &authorization_id(ctx, &principal, action, resource),
    ))
}

/// Where the authorization details of ONE refused request are read: the console page for
/// an authorization id.
///
/// The host and the Region parameter are IAM's own endpoint Region whichever Region the
/// caller addressed, because IAM is a global service: there is one place its authorization
/// details are served from, so a refusal answered to a caller elsewhere still points
/// there.
///
/// LIMITATION, stated so the pointer is not mistaken for more than it is: this gate names
/// the id of the refusal it is answering, and nothing here records the evaluation behind
/// it, so a reader that follows the id into the authorization-details reader is told the
/// details are not available. Recording them is the work of the operation that serves
/// them, not of the refusal that names one.
const AUTHORIZATION_DETAILS_PAGE: &str =
    "https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/";

/// [`not_authorized`], closed by the pointer to the authorization details of the
/// evaluation that withheld the call: the page those details are read on, and the
/// authorization id a reader of them takes.
fn not_authorized_with_details(
    principal: &str,
    action: &str,
    named_as: &str,
    reason: &str,
    authorization_id: &str,
) -> AccessDenied {
    AccessDenied::new(format!(
        "User: {principal} is not authorized to perform: {action} on resource: {named_as} \
         {reason}. Go to {AUTHORIZATION_DETAILS_PAGE}{authorization_id} for complete \
         details, or call the GetRequestAuthorizationDetails API with the following \
         authorization id: {authorization_id}"
    ))
}

/// The authorization id of THIS request: the handle its refusal points at.
///
/// Derived from what the request itself carries — the instant it was signed, the digest of
/// its content, and what the decision was made about — rather than drawn at random, so two
/// refusals over different content, at different instants or about different subjects are
/// pointed at under different ids, and nothing has to be remembered for an id to stay the
/// id of that refusal. It is a FUNCTION of those facts and nothing else: two refusals that
/// agree on every one of them — the same caller refused the same call on the same content
/// within the same second — are pointed at under one id, which is the determinism the
/// wording this gate answers with is asserted on. The credential and signature a request is
/// signed with are deliberately NOT part of it: the id travels back to the caller, so
/// nothing derived from signing material goes into it.
///
/// Rendered by the tree's ONE derivation of an id from a seed
/// ([`::service_common::caller_principal::aws_unique_id`]), in the lower case an
/// authorization id is spelled in; it carries no type prefix, because an authorization id
/// names a request rather than an entity.
fn authorization_id(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    principal: &str,
    action: &str,
    resource: &str,
) -> String {
    /// The header fields that say WHEN a request was signed and WHAT it carried, neither
    /// of which is credential material.
    const REQUEST_HEADERS: &[&str] = &["x-amz-date", "x-amz-content-sha256"];
    /// The separator between the seed's parts, so two different splittings of the same
    /// text cannot seed the same id.
    const SEPARATOR: char = '\n';

    let headers = &ctx.parts().headers;
    let mut seed = String::new();
    for header in REQUEST_HEADERS {
        if let Some(value) = headers.get(*header).and_then(|value| value.to_str().ok()) {
            seed.push_str(value);
        }
        seed.push(SEPARATOR);
    }
    for part in [principal, action, resource] {
        seed.push_str(part);
        seed.push(SEPARATOR);
    }
    ::service_common::caller_principal::aws_unique_id("", &seed).to_lowercase()
}

/// The `AccessDenied` text AWS words a call it will not perform for its caller with:
/// what the refusal is about — the principal that signed the request, the action it
/// asked for and the resource it asked for it on — closed by `reason`, which says why,
/// and nothing further.
///
/// ONE wording for the code, so a refusal an operation reaches for a reason of its own
/// is worded exactly as the policy decision above is, differing only in that reason.
/// Every part of it is a fact of the request and of the account's state, so the same
/// refused call is worded the same on every run and in every Region.
/// [`not_authorized_with_details`] is this sentence continued by the pointer at the
/// authorization details of an evaluation that withheld a grant.
pub fn not_authorized(principal: &str, action: &str, named_as: &str, reason: &str) -> AccessDenied {
    AccessDenied::new(format!(
        "User: {principal} is not authorized to perform: {action} on resource: {named_as} \
         {reason}"
    ))
}

/// Whether the caller has sufficient permissions to cover the asks of the delegation
/// request `delegated` — the permission check `GetDelegationRequest` performs when its
/// request asks for one — as one of the three values `permissionCheckResultType`
/// declares (`super::super::ops::types::PERMISSION_CHECK_RESULT_TYPE_VALUES`).
///
/// The asks are the `(action, resource)` pairs the request's permission policy names
/// ([`super::delegated_asks`]), and the question asked about each of them is the one
/// [`authorize_named`] asks about the call itself, decided against the SAME documents by
/// the same evaluation: the caller covers an ask when its identity-based policies allow
/// that action on that resource.
///
/// * [`PERMISSION_CHECK_DENIED`] as soon as a statement explicitly denies an ask, since
///   no grant overturns an explicit deny, and for a caller whose documents simply do not
///   cover every ask.
/// * [`PERMISSION_CHECK_ALLOWED`] when every ask is covered.
/// * [`PERMISSION_CHECK_UNSURE`] — "it is not possible to determine whether the caller
///   has all the permissions needed" — where the evidence is incomplete rather than
///   short: asks that cannot be enumerated from the request, a caller holding a grant
///   whose document the account does not hold (see the module docs), and a caller whose
///   grants are conditioned, which the documented case for this value ("most likely for
///   cases when the caller has permissions with conditions") names.
///
/// A READ, like every other decision here: the answer comes from the documents in force
/// at the instant of the call and nothing is written, so asking twice answers twice the
/// same.
pub fn delegation_permission_check(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    delegated: &DelegationRequest,
) -> &'static str {
    let Some(asks) = delegated
        .permission_policy
        .as_deref()
        .and_then(super::delegated_asks)
    else {
        return PERMISSION_CHECK_UNSURE;
    };

    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // A credential this account holds no IAM user for carries no identity-based policies
    // at all — the position account root credentials are in (see the module docs) — so
    // there is nothing its permissions can fall short of.
    let Some(signer) = signing_principal(store, &access_key_id) else {
        return PERMISSION_CHECK_ALLOWED;
    };
    let SigningPrincipal {
        arn: principal,
        policies,
        context,
    } = signer;

    let principals = [principal];
    let documents = iam::prepare_documents(&policies.documents);
    let mut short_of_an_ask = false;
    for (action, resource) in &asks {
        // The same question [`authorize_named`] asks, so it is asked with the same
        // knowledge of the caller and of the resource each ask names
        // ([`request_context`]).
        let (ask_context, published) = request_context(store, &context, resource);
        let request = iam::Request::new(action, resource, &principals)
            .with_context(&ask_context)
            .with_published_keys(&published);
        let decided = iam::identity_decision_prepared(&documents, &request);
        if decided == Decision::Denied {
            return PERMISSION_CHECK_DENIED;
        }
        if !iam::is_allowed(decided, Decision::NotStated) {
            short_of_an_ask = true;
        }
    }
    if !short_of_an_ask {
        return PERMISSION_CHECK_ALLOWED;
    }

    // The caller is short of an ask on the evidence that could be read. Where part of
    // that evidence could NOT be — a document the account does not hold, or a grant
    // whose condition is not decided here — what is known is that the answer is not
    // known, which is the third value rather than a refusal.
    let conditioned = !super::context_keys_in_policies(
        policies.documents.iter().map(::std::string::String::as_str),
    )
    .is_empty();
    if conditioned || !policies.uncarried.is_empty() {
        PERMISSION_CHECK_UNSURE
    } else {
        PERMISSION_CHECK_DENIED
    }
}

/// What a signed request is judged on, resolved once from the access key it carries:
/// the principal it speaks for, the identity-based documents in force on that principal,
/// and the condition context the request carries about it.
#[derive(Debug)]
struct SigningPrincipal {
    /// The ARN of the IAM user the signing access key belongs to — what a refusal names
    /// as the caller, and the principal a resource-based statement would name.
    arn: String,
    /// The identity-based documents in force on it, and the attachments whose documents
    /// the account does not hold.
    policies: PrincipalPolicies,
    /// The condition keys the request carries about that principal
    /// ([`principal_context`]).
    context: Vec<(String, String)>,
}

/// The IAM user the access key id `access_key_id` belongs to, or `None` when the account
/// holds no user for that key.
fn signing_principal(store: &IamState, access_key_id: &str) -> Option<SigningPrincipal> {
    let record = store.access_keys.get(access_key_id)?;
    let user = store.users.get(&record.user_key)?;
    let policies = store.principal_policies(&user.arn)?;
    Some(SigningPrincipal {
        arn: user.arn.clone(),
        policies,
        context: principal_context(user),
    })
}
