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
//! things the decision was made about: the principal, the action and the resource —
//! closed by the sentence naming where the details of that one denial are readable, by
//! the authorization id the refusal mints ([`authorization_details_sentence`]).
//!
//! The four pieces, each held in exactly one place:
//!
//! 1. **who** — the IAM user the request's SigV4 access key id belongs to, read out of
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::access_keys`], the same
//!    map `GetUser` and `CreateAccessKey` resolve a nameless caller through;
//! 2. **what it may do** — the identity-based documents in force on that principal, from
//!    [`super::ComAmazonawsIamAwsidentitymanagementv20100508State::principal_policies`],
//!    so a policy just put grants immediately and one just deleted stops granting
//!    immediately;
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

use ::std::sync::atomic::{AtomicU64, Ordering};

use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::DelegationRequest;
use crate::scaffolded::AccessDenied;

use super::{ComAmazonawsIamAwsidentitymanagementv20100508State as IamState, PrincipalPolicies};

/// The Region the console view of a denial lives in.
///
/// IAM is a GLOBAL service: its endpoint `iam.amazonaws.com` is in `us-east-1` ("AWS
/// Identity and Access Management endpoints and quotas", AWS General Reference), and so
/// is the console that serves a refused request's authorization details. A request signed
/// for another Region is still a request to that one global service, so the page its
/// denial points at is the same one — the Region a request named in its SigV4 credential
/// scope is not where its authorization details are read.
const IAM_GLOBAL_REGION: &str = "us-east-1";

/// The alphabet an authorization id is spelled in.
///
/// It is the one the reader of these ids accepts: `GetRequestAuthorizationDetails`
/// takes `authorizationId` constrained to the pattern
/// `^[0123456789abcdefghijklmnopqrstuvwxyz]+$` (`smithy_query(query="operation",
/// operation="GetRequestAuthorizationDetails", model_path=
/// "services/service-com-amazonaws-iamtoolbox-authrequestservice/smithy-model/com-amazonaws-iamtoolbox-authrequestservice.json")`),
/// so an id a denial hands a caller is one that reader takes.
const AUTHORIZATION_ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// How many characters a minted authorization id carries. The reader's member declares
/// no length bound, so the length is this emulator's own: long enough that an id is
/// opaque rather than a readable counter.
const AUTHORIZATION_ID_CHARACTERS: usize = 32;

/// How many characters of [`AUTHORIZATION_ID_ALPHABET`] spell out a whole 64-bit word
/// (36^13 > 2^64), so the first that many characters of an id carry every bit of the
/// word they were drawn from.
const AUTHORIZATION_ID_CHARACTERS_PER_WORD: usize = 13;

/// How many requests this process has refused, which is what each refusal's
/// authorization id is minted from.
///
/// Deliberately NOT a field of the service's state: reaching a decision is a READ (see
/// [`authorize_named`]), so a refused call must leave nothing behind for a later caller
/// to observe. All an id has to be is distinct per denial, which a fetch-and-add is
/// without touching the store.
static REFUSED_REQUESTS: AtomicU64 = AtomicU64::new(0);

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

/// `iam:ListOpenIDConnectProviders`.
pub const LIST_OPEN_ID_CONNECT_PROVIDERS: &str = "iam:ListOpenIDConnectProviders";

/// `iam:CreateGroup`.
pub const CREATE_GROUP: &str = "iam:CreateGroup";

/// `iam:DeleteGroup`.
pub const DELETE_GROUP: &str = "iam:DeleteGroup";

/// `iam:GetGroup`.
pub const GET_GROUP: &str = "iam:GetGroup";

/// `iam:UpdateGroup`.
pub const UPDATE_GROUP: &str = "iam:UpdateGroup";

/// `iam:ListGroupsForUser`.
pub const LIST_GROUPS_FOR_USER: &str = "iam:ListGroupsForUser";

/// `iam:ListUserTags`.
pub const LIST_USER_TAGS: &str = "iam:ListUserTags";

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
    let Some((principal, policies)) = signing_principal(store, &access_key_id) else {
        return Ok(());
    };

    let principals = [principal.clone()];
    let request = iam::Request::new(action, resource, &principals);
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
    // The handle this one denial is readable by, minted per refused request.
    let authorization_id = next_authorization_id();
    // Audit record for the refusal: the acting principal, what it asked for, the
    // decision and the id the caller was handed, so a log entry and the caller's own
    // message name the same denial. The policy documents themselves are not logged.
    ::tracing::info!(
        audit = true,
        outcome = "refused_access_denied",
        principal = %principal,
        action = %action,
        resource = %resource,
        explicit_deny = decided == Decision::Denied || capped == Decision::Denied,
        authorization_id = %authorization_id,
        "the policies in force on the caller do not permit this call"
    );
    // What the decision was about, then why it went that way, then where the caller
    // reads the details of it: the three parts of AWS's `AccessDenied` text, with the
    // closing sentence carried by every refusal this function words rather than by the
    // operations that happen to have been measured.
    Err(AccessDenied::new(format!(
        "User: {principal} is not authorized to perform: {action} on resource: {named_as} \
         {reason}{}",
        authorization_details_sentence(IAM_GLOBAL_REGION, &authorization_id)
    )))
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
    let Some((principal, policies)) = signing_principal(store, &access_key_id) else {
        return PERMISSION_CHECK_ALLOWED;
    };

    let principals = [principal];
    let documents = iam::prepare_documents(&policies.documents);
    let mut short_of_an_ask = false;
    for (action, resource) in &asks {
        let request = iam::Request::new(action, resource, &principals);
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

/// A fresh authorization id for one denied request: the handle the denial hands the
/// caller so it can read that request's authorization details back.
///
/// Distinct for every denial. The first [`AUTHORIZATION_ID_CHARACTERS_PER_WORD`]
/// characters spell out all 64 bits of a bijection of [`REFUSED_REQUESTS`], so two
/// denials never mint the same id; the characters after them come from further words
/// keyed to the same count, purely so the id is as long and as opaque as a service-minted
/// handle rather than a readable counter.
fn next_authorization_id() -> String {
    let alphabet = AUTHORIZATION_ID_ALPHABET.len() as u64;
    let refusal = REFUSED_REQUESTS.fetch_add(1, Ordering::Relaxed);
    let mut id = String::with_capacity(AUTHORIZATION_ID_CHARACTERS);
    let mut word = super::splitmix64(refusal);
    for position in 0..AUTHORIZATION_ID_CHARACTERS {
        if position > 0 && position % AUTHORIZATION_ID_CHARACTERS_PER_WORD == 0 {
            word = super::splitmix64(refusal ^ (position as u64).rotate_left(32));
        }
        id.push(char::from(AUTHORIZATION_ID_ALPHABET[(word % alphabet) as usize]));
        word /= alphabet;
    }
    id
}

/// The sentence that closes an `AccessDenied` message: where the caller reads the
/// authorization details of the request that was refused.
///
/// Both halves name the SAME `authorization_id` — the console view of that one denial, in
/// the Region IAM's console lives in ([`IAM_GLOBAL_REGION`]), and the API that serves the
/// same record
/// (`GetRequestAuthorizationDetails`, served by this emulator's IAM Toolbox service).
/// It is fixed text around two values taken from the request and from the id this
/// denial minted, so it carries nothing of any other request.
fn authorization_details_sentence(region: &str, authorization_id: &str) -> String {
    format!(
        ". Go to https://{region}.console.aws.amazon.com/iam/home?region={region}\
         #/authorization-details/{authorization_id} for complete details, or call the \
         GetRequestAuthorizationDetails API with the following authorization id: \
         {authorization_id}"
    )
}

/// The IAM user the access key id `access_key_id` belongs to — its ARN and the
/// identity-based documents in force on it — or `None` when the account holds no user
/// for that key.
fn signing_principal(
    store: &IamState,
    access_key_id: &str,
) -> Option<(String, PrincipalPolicies)> {
    let record = store.access_keys.get(access_key_id)?;
    let user = store.users.get(&record.user_key)?;
    let policies = store.principal_policies(&user.arn)?;
    Some((user.arn.clone(), policies))
}
