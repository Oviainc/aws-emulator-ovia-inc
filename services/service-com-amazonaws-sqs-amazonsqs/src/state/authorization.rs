// SPDX-License-Identifier: CC0-1.0

//! Whether the policies in force let a request act on the queue it addresses, and what a
//! queue's `Policy` attribute has to look like to be accepted at all.
//!
//! A queue's `Policy` is a RESOURCE-BASED policy, so every call that acts on a queue is
//! decided the way AWS decides one: against the calling principal's identity-based
//! policies and the queue's own document, at the instant of the call. A policy just
//! written grants immediately and one just cleared stops granting immediately, because
//! nothing is cached — the document is read out of the queue record the write left behind.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the
//!    one the caller reads back from STS for the credentials it signed with;
//! 2. **what it may do** — the identity-based documents the IAM store holds for that
//!    principal (its inline policies and the default version of each managed policy
//!    attached to it), read through IAM's own
//!    [`principal_policies`](iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State::principal_policies)
//!    accessor so this service sees exactly what IAM's handlers wrote, plus the
//!    permissions boundary that caps them;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of policy documents against `(action, resource, principal)`, so this
//!    service and every other one handed the same documents decide alike.
//!
//! ## The rule
//!
//! In IAM's own order: an explicit `Deny` about the request — in the queue's policy or in
//! an identity-based document — refuses, whatever any grant says. Otherwise the request
//! needs a grant: an `Allow` about it in the caller's identity-based policies, or one in
//! the queue's policy that names the CALLER, and (where the principal carries one) a
//! permissions boundary that also allows it. With neither grant the request is refused —
//! IAM's implicit deny, naming the document that withheld the grant: the boundary where the
//! identity-based policies did allow the call, and those policies where nothing allowed it
//! at all — and the operation has no effect.
//!
//! A statement that names the ACCOUNT rather than the caller (the account's root ARN, or
//! the bare account id that stands for it) delegates to the account rather than granting:
//! "you must also attach an identity-based policy" to the principal. So the account
//! spellings decide a `Deny` — which covers every principal of the account — and never
//! make a grant on their own.
//!
//! ## The two callers that are not IAM principals
//!
//! * **The account's own published credential**, which is the position AWS account root
//!   credentials are in: the IAM store holds no policies for it, and the queue's owner has
//!   full control of it, so such a request is allowed — while an explicit `Deny` in the
//!   queue's policy still refuses it.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy is owned by AWS, not by the account, so the account's store holds no document
//!   for it. What it grants is unknown here and refusing on evidence this emulator does
//!   not have would deny access AWS permits, so such a principal is allowed where the
//!   readable documents neither grant nor deny; an explicit `Deny` in a document that IS
//!   held still refuses, because no grant overturns one.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused.
//!
//! Resolving the caller goes through STS's own `resolve_request_caller`, which
//! materialises the session expiries the clock has already made due before it answers.
//! That write is owed at the same instant whether or not anybody called, which is why a
//! REPORTING operation (`GetQueueAttributes`, `ListQueueTags`) may make this decision:
//! what a later caller sees is unchanged by the read itself.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `sqs:<ActionName>`, which is also this
/// service's `arnNamespace` and signing name ([`super::ARN_SERVICE`]).
pub const ACTION_PREFIX: &str = super::ARN_SERVICE;

/// Every action name an Amazon SQS policy may name: this service's own action names, which
/// is the one list the crate keeps ([`super::ACTION_NAMES`], the operations
/// `smithy_query(query="list_operations")` reports and the set `AddPermission`'s `Actions`
/// is taken from).
///
/// A statement's `Action` is compared against it, and nothing narrower: how far a grant
/// REACHES — whether a cross-account principal is served by it — is the evaluator's
/// question at call time, not a reason to refuse the document a caller stores.
pub fn policy_action_names() -> &'static [&'static str] {
    super::ACTION_NAMES
}

/// The batch operations and the action each of them is authorized under.
///
/// "Specifying `SendMessage`, `DeleteMessage`, or `ChangeMessageVisibility` for
/// `ActionName.n` also grants permissions for the corresponding batch versions of those
/// actions: `SendMessageBatch`, `DeleteMessageBatch`, and `ChangeMessageVisibilityBatch`"
/// (`smithy_query(query="doc",shape="AddPermissionRequest",member="Actions")`), so a batch
/// call is decided against the action its single-message sibling carries.
const BATCH_ACTIONS: &[(&str, &str)] = &[
    ("SendMessageBatch", "SendMessage"),
    ("DeleteMessageBatch", "DeleteMessage"),
    (
        "ChangeMessageVisibilityBatch",
        "ChangeMessageVisibility",
    ),
];

/// The IAM action an operation of this service is authorized under: `sqs:<ActionName>`,
/// with a batch operation decided under its single-message sibling's action (see
/// [`BATCH_ACTIONS`]).
pub fn action_for(operation: &str) -> String {
    let name = BATCH_ACTIONS
        .iter()
        .find(|(batch, _)| *batch == operation)
        .map_or(operation, |(_, single)| *single);
    format!("{ACTION_PREFIX}:{name}")
}

/// The condition key family that carries one tag of the calling principal, which is the
/// only `aws:` key this emulator can answer for a direct caller ("aws:PrincipalTag/tag-key",
/// IAM's global condition keys).
const PRINCIPAL_TAG_PREFIX: &str = "aws:PrincipalTag/";

/// The global condition keys that name the calling PRINCIPAL itself, which this gate knows
/// exactly: "aws:PrincipalArn — the ARN of the principal that made the request" and
/// "aws:PrincipalAccount — the account the principal belongs to".
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";
const PRINCIPAL_ACCOUNT_KEY: &str = "aws:PrincipalAccount";

/// "aws:SecureTransport — whether the request was sent using SSL": the emulator's data
/// plane is served over TLS, so every request that reaches a handler came that way.
const SECURE_TRANSPORT_KEY: &str = "aws:SecureTransport";

/// The keys that name the resource a call was made ON BEHALF OF: "aws:SourceArn — used to
/// compare the Amazon Resource Name (ARN) of the resource making a service-to-service
/// request" and the account that resource belongs to. A request that reaches this gate is a
/// caller's OWN, not one another service makes for it, so it carries NEITHER — which is a
/// value this gate knows rather than one it cannot answer, and is why both are published
/// with no value.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The value that key carries, as IAM's boolean literals spell it.
const SECURE_TRANSPORT_VALUE: &str = "true";

/// The condition keys this service MODELS, which is what makes a statement conditioned on
/// one of them decidable even where the request carries no value for it.
///
/// The families and keys are the ones the gate can answer exactly: the calling principal's
/// tags (the gate reads every tag the principal carries, so a key of that family the context
/// does not carry is a tag the principal really does not have — which is what IAM's `Null`
/// operator asks), the principal's own ARN and account, whether the request came over TLS,
/// and the two `aws:Source…` keys a caller's own request never carries. No other `aws:` key
/// is published: a key this emulator does not model leaves the statement that names it
/// undecided, which is the fail-closed reading the shared evaluator takes.
const PUBLISHED_CONDITION_KEYS: &[&str] = &[
    PRINCIPAL_TAG_PREFIX,
    PRINCIPAL_ARN_KEY,
    PRINCIPAL_ACCOUNT_KEY,
    SECURE_TRANSPORT_KEY,
    SOURCE_ARN_KEY,
    SOURCE_ACCOUNT_KEY,
];

/// The ARN of an account's root user, the spelling that stands for the whole account in a
/// resource policy's `Principal` element — and the form a bare account id is stored as
/// ([`normalised_policy`]).
pub fn account_root_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:root")
}

/// How many characters an AWS account id has, which is what makes a bare `Principal`
/// value one ("a 12-digit account ID").
const ACCOUNT_ID_LENGTH: usize = 12;

/// Whether `value` is a bare AWS account id: twelve digits and nothing else.
fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_LENGTH && value.chars().all(|character| character.is_ascii_digit())
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// The account the principal belongs to.
    pub account: String,
    /// Every spelling of the principal a resource policy may name IT by: the ARN STS
    /// reports, and the ARN of the IAM role behind an assumed-role session.
    pub principals: Vec<String>,
    /// The spellings that name the ACCOUNT the principal belongs to rather than the
    /// principal: they decide a deny and never grant (see the module docs).
    pub account_principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// The permissions boundary document in force on the principal, when it carries one
    /// this emulator can read, as the IAM store resolves it
    /// ([`iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State::permissions_boundary_document`]).
    pub boundary: Option<String>,
    /// The condition keys this call can answer for: the `aws:PrincipalTag/<key>` of every
    /// tag the calling principal carries, and the keys that name the principal itself (see
    /// [`condition_context`]).
    pub context: Vec<(String, String)>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    pub account_credential: bool,
    /// Set when the principal carries a policy whose document the IAM store does not hold
    /// — an AWS-managed policy (see the module docs).
    pub unreadable_attachment: bool,
}

/// Why a request was refused, worded as IAM words that decision: who was refused, which
/// action, on which resource, and which document withheld the grant.
///
/// The reasons are the different facts about the policies in force, which AWS states
/// differently because they need different fixes ("Troubleshoot access denied error
/// messages", IAM User Guide): an explicit deny names the KIND of document it is in, and an
/// implicit one names the action no policy allows. Every value in the message comes from
/// the request, the caller's credential and the stored documents; the action is named in
/// the lower case Amazon SQS words a refusal in ([`reported_action`]).
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}")
}

/// The action as a refusal of this service NAMES it: lower case throughout.
///
/// IAM compares a policy's action names case-insensitively, so the case a refusal reports
/// is a wording choice rather than part of the decision — and the wording is the service's.
fn reported_action(action: &str) -> String {
    action.to_lowercase()
}

/// The reason an IMPLICIT deny states: within one account the permission a principal is
/// missing is its own, so what the refusal names is the action no policy allows it.
fn implicit_deny(named_action: &str) -> String {
    format!("because no identity-based policy allows the {named_action} action")
}

/// The reason an implicit deny states when the identity-based policies DID allow the
/// call and the permissions boundary withheld it: the document that refused it is the
/// boundary, so that is the one the refusal names.
fn boundary_implicit_deny(named_action: &str) -> String {
    format!("because no permissions boundary allows the {named_action} action")
}

/// Whether the policies in force let this request perform its operation on `resource`.
///
/// `resource_policy` is the queue's own `Policy` attribute, when it carries one.
/// `Ok(())` when the request is allowed; `Err` is the message the operation answers with
/// through its error channel (HTTP 403, error code `AccessDenied`).
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
    resource_policy: Option<&str>,
) -> Result<(), String> {
    let action = action_for(operation);
    let named_action = reported_action(&action);
    let caller = caller(engine, ctx);
    let refusal = |reason: &str| not_authorized(&caller.arn, &named_action, resource, reason);

    // Every spelling of the caller, the account ones included: this is the comparison a
    // DENY is decided under, because a statement denying the account denies each of its
    // principals.
    let mut named = caller.principals.clone();
    named.extend(caller.account_principals.iter().cloned());
    let denying = iam::Request::new(&action, resource, &named)
        .with_context(&caller.context)
        .with_published_keys(PUBLISHED_CONDITION_KEYS);
    // The caller's own spellings only: this is the comparison a GRANT is decided under,
    // because naming the account delegates rather than grants.
    let granting = iam::Request::new(&action, resource, &caller.principals)
        .with_context(&caller.context)
        .with_published_keys(PUBLISHED_CONDITION_KEYS);

    let denied_by_resource = resource_policy
        .map(|document| iam::resource_decision(document, &denying))
        .unwrap_or(Decision::NotStated);
    // An explicit deny wins over every grant, including the account's own credential.
    if denied_by_resource == Decision::Denied {
        return Err(refusal("with an explicit deny in a resource-based policy"));
    }
    if caller.account_credential {
        return Ok(());
    }

    let identity_said = iam::identity_decision(&caller.documents, &granting);
    if identity_said == Decision::Denied {
        return Err(refusal("with an explicit deny in an identity-based policy"));
    }
    let resource_said = resource_policy
        .map(|document| iam::resource_decision(document, &granting))
        .unwrap_or(Decision::NotStated);
    // A grant in the QUEUE's own policy names this principal directly, and "a permissions
    // boundary does not limit the permissions granted to the entity by a resource-based
    // policy that names it as a principal": such a grant stands whatever the boundary says.
    if resource_said == Decision::Allowed {
        return Ok(());
    }
    let granted_by_identity = identity_said == Decision::Allowed
        // A grant this emulator cannot read is not evidence of a refusal (see the module
        // docs), so a principal carrying one is allowed where the readable documents
        // neither grant nor deny.
        || caller.unreadable_attachment;
    if !granted_by_identity {
        return Err(refusal(&implicit_deny(&named_action)));
    }
    // "A permissions boundary … sets the maximum permissions that an identity-based policy
    // can grant": a grant the boundary does not also allow is not a permission the
    // principal has.
    if let Some(boundary) = &caller.boundary {
        match iam::identity_decision(::std::slice::from_ref(boundary), &granting) {
            Decision::Allowed => {}
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave, so the refusal names the boundary as the
            // document that allows nothing here.
            Decision::Denied => {
                return Err(refusal("with an explicit deny in a permissions boundary"))
            }
            Decision::NotStated => return Err(refusal(&boundary_implicit_deny(&named_action))),
        }
    }
    Ok(())
}

/// The refusal an operation returns for a request the policies in force do not permit, or
/// `None` when they do: the one call an implemented handler makes.
///
/// Generic over the operation's own modelled error enum because the refusal is NOT one of
/// them — Amazon SQS's model declares no `AccessDenied` shape — so it is carried by the
/// error channel's own variant.
pub fn denied<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
    resource_policy: Option<&str>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    authorize(engine, ctx, operation, resource, resource_policy)
        .err()
        .map(crate::scaffolded::Scaffolded::access_denied)
}

/// The principal this request speaks for, with the documents in force on it.
pub fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let iam = iam_store::service_state(engine);
    // A long-term access key speaks for the IAM user that holds it, which is the identity
    // STS itself reports for it; a key no store holds is the account's own published
    // credential.
    let long_term = match &iam {
        Some(iam) => sts_store::long_term_identity(Some(&iam.lock()), &access_key_id),
        None => sts_store::ambient_identity(),
    };
    let resolved = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_request_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
            long_term.clone(),
        )
        .map(|caller| {
            (
                caller.identity.arn,
                caller.identity.account,
                caller.role_arn,
                caller.tags,
            )
        }),
        // No STS store on this engine: the credential names no session, so it speaks for
        // the long-term identity it was resolved to.
        None => Ok((
            long_term.arn.clone(),
            long_term.account.clone(),
            None,
            Vec::new(),
        )),
    };
    // The session credential has expired, so it speaks for nobody: it carries no documents
    // and no principal spelling. The refusal still names what was presented, so a caller
    // reading it can tell an expired credential from an unauthorized one.
    let Ok((arn, account, role_arn, tags)) = resolved else {
        return Caller {
            arn: format!("an expired session credential ({access_key_id})"),
            ..Caller::default()
        };
    };

    let mut caller = Caller {
        account_credential: arn == sts_store::ambient_identity().arn,
        principals: vec![arn.clone()],
        account_principals: vec![account_root_arn(&account), account.clone()],
        account: account.clone(),
        arn,
        ..Caller::default()
    };
    let session_tags = tags;
    // An assumed-role session is also named by the ARN of the role it is a session of,
    // which is the spelling a resource policy uses to name the role itself.
    if let Some(role_arn) = role_arn {
        if !caller.principals.contains(&role_arn) {
            caller.principals.push(role_arn.clone());
        }
    }
    let Some(iam) = iam else {
        caller.context = condition_context(&caller.arn, &account, &[], &session_tags);
        return caller;
    };
    let store = iam.lock();
    // The identity whose policies are in force: an assumed-role session's are the ROLE's,
    // every other principal's are its own.
    let Some(identity_arn) = caller
        .principals
        .iter()
        .find(|named| iam_store::parse_entity_arn(named).is_some())
        .cloned()
        .or_else(|| role_arn_of_session(&store, &caller.arn))
    else {
        caller.context = condition_context(&caller.arn, &account, &[], &session_tags);
        return caller;
    };
    if let Some(policies) = store.principal_policies(&identity_arn) {
        caller.documents = policies.documents;
        caller.unreadable_attachment = !policies.uncarried.is_empty();
    }
    if !caller.principals.contains(&identity_arn) {
        caller.principals.push(identity_arn.clone());
    }
    caller.context = condition_context(
        &caller.arn,
        &account,
        &entity_tags(&store, &identity_arn),
        &session_tags,
    );
    caller.boundary = store.permissions_boundary_document(&identity_arn);
    caller
}

/// The condition keys the decision is made under: the `aws:PrincipalTag/<key>` of every tag
/// the calling principal carries — "the tags attached to the principal making the request",
/// which for an assumed-role session are the role's tags with the SESSION's over them, since
/// a session tag with the same key as one of the identity's "takes precedence" over it — plus
/// the keys that name the principal itself and the transport it came over.
///
/// Nothing else is published: a key this emulator cannot answer for leaves the statement
/// that names it undecided, which is the fail-closed reading the shared evaluator takes.
fn condition_context(
    arn: &str,
    account: &str,
    identity_tags: &[(String, String)],
    session_tags: &[(String, String)],
) -> Vec<(String, String)> {
    // Session tags come last, so a key both carry keeps the SESSION's value.
    let named: ::std::collections::BTreeMap<String, String> = identity_tags
        .iter()
        .chain(session_tags.iter())
        .map(|(key, value)| (format!("{PRINCIPAL_TAG_PREFIX}{key}"), value.clone()))
        .collect();
    named
        .into_iter()
        .chain([
            (PRINCIPAL_ARN_KEY.to_string(), arn.to_string()),
            (PRINCIPAL_ACCOUNT_KEY.to_string(), account.to_string()),
            (
                SECURE_TRANSPORT_KEY.to_string(),
                SECURE_TRANSPORT_VALUE.to_string(),
            ),
        ])
        .collect()
}

/// The tags the IAM identity `arn` names carries, as key/value pairs.
fn entity_tags(
    store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
    arn: &str,
) -> Vec<(String, String)> {
    let Some(entity) = iam_store::parse_entity_arn(arn) else {
        return Vec::new();
    };
    let key = iam_store::name_key(&entity.name);
    let held = match entity.kind {
        iam_store::EntityKind::Role => store.roles.get(&key).and_then(|role| role.tags.clone()),
        iam_store::EntityKind::User => store.users.get(&key).and_then(|user| user.tags.clone()),
        iam_store::EntityKind::Group => None,
    };
    held.unwrap_or_default()
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect()
}

/// The ARN of the IAM role an STS assumed-role session ARN names, as the IAM store holds
/// it.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role
/// is the first segment after `assumed-role/`; the role's own ARN is read from IAM rather
/// than rebuilt here, because only IAM knows the path the role was created under.
fn role_arn_of_session(
    store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
    session_arn: &str,
) -> Option<String> {
    let mut fields = session_arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some("sts"), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    store
        .roles
        .get(&iam_store::name_key(name))
        .map(|role| role.arn.clone())
}

/// Why a `Policy` attribute value cannot be stored, or `None` when it is one Amazon SQS
/// accepts.
///
/// A queue policy is validated when it is WRITTEN, not only when it is evaluated: a
/// statement this service can never act on cannot ever grant anything, so the document is
/// refused and the attribute is left as it was. A statement is one it can act on when
///
/// * its `Effect` is `Allow` or `Deny` — the only two effects the grammar has;
/// * it states an `Action` or a `NotAction`, each of whose values names an action of this
///   service ([`policy_action_names`]) — a statement about no action of this service is
///   about nothing this queue does;
/// * every principal its `Principal`/`NotPrincipal` names in the `AWS` form exists, where
///   this account is the authority on that ([`names_a_principal`]).
///
/// Everything a document can state that this service cannot judge — the `Resource` element,
/// a condition key, a principal of another account — is accepted, because a resource policy
/// may name resources and principals that do not exist yet. So is a statement that is
/// INEFFECTIVE: one stating no effect, or no action, grants nothing and refuses nothing. A
/// document that states no statement at all is accepted too, for the same reason.
pub fn policy_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    document: &str,
) -> Option<super::InvalidAttributeValue> {
    let parsed: ::serde_json::Value = ::serde_json::from_str(document).ok()?;
    // A policy document is a JSON OBJECT — `{"Version": …, "Statement": …}`; a value that
    // is some other JSON is not a document at all, so there is nothing for this service to
    // act on in it.
    if !parsed.is_object() {
        return Some(super::invalid_attribute_value(super::POLICY));
    }
    let acceptable = statements(&parsed)
        .into_iter()
        .all(|statement| statement_is_acceptable(engine, statement));
    // The refusal names the attribute whose value was not accepted, which is the one
    // wording this service refuses every unacceptable attribute value with.
    (!acceptable).then(|| super::invalid_attribute_value(super::POLICY))
}

/// Whether ONE statement is one this service can act on. See [`policy_refusal`].
fn statement_is_acceptable<Eng: Send + Sync + 'static>(
    engine: &Eng,
    statement: &::serde_json::Value,
) -> bool {
    if !statement.is_object() {
        return false;
    }
    // Only the elements this service is the authority on are judged. A statement that
    // states no effect, no action or no resource is INEFFECTIVE rather than unacceptable —
    // it grants nothing and refuses nothing — which is a document a caller may store.
    for element in ["Action", "NotAction"] {
        let Some(named) = statement.get(element) else {
            continue;
        };
        // An element that IS stated has to name an action: one naming none — an empty list,
        // or a value the grammar does not admit — is about nothing this queue does.
        let actions = string_or_list(Some(named));
        if actions.is_empty() || !actions.iter().all(|named| names_an_action(named)) {
            return false;
        }
    }
    ["Principal", "NotPrincipal"]
        .into_iter()
        .flat_map(|element| principals(statement.get(element)))
        .all(|named| names_a_principal(engine, &named))
}

/// The statements of a policy document, which "can be a single statement or an array of
/// statements" — and nothing at all for a document that states none, which is a document
/// this service accepts and that grants nothing.
fn statements(document: &::serde_json::Value) -> Vec<&::serde_json::Value> {
    match document.get("Statement") {
        Some(::serde_json::Value::Array(statements)) => statements.iter().collect(),
        Some(statement) => vec![statement],
        None => Vec::new(),
    }
}

/// One policy element's values: each of `Action`/`Resource`/`Principal` is "either one
/// string or a list of them".
fn string_or_list(element: Option<&::serde_json::Value>) -> Vec<String> {
    match element {
        Some(::serde_json::Value::String(value)) => vec![value.clone()],
        Some(::serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// The `AWS` principals a `Principal` element names — the only kind whose existence this
/// account is the authority on.
///
/// The element is either the whole-element wildcard `"*"`, which names every principal, or
/// a map keyed by principal TYPE (`AWS`, `Service`, `Federated`, `CanonicalUser`); a
/// `Service` or `Federated` principal is owned by AWS or by an external provider, so
/// nothing here judges one.
fn principals(element: Option<&::serde_json::Value>) -> Vec<String> {
    match element {
        Some(::serde_json::Value::Object(map)) => map
            .get("AWS")
            .map(|aws| string_or_list(Some(aws)))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Whether `named` names an action of this service: the whole-element wildcard, or
/// `sqs:<name>` where the name — wildcards included, which IAM compares
/// case-insensitively — covers at least one of [`policy_action_names`].
fn names_an_action(named: &str) -> bool {
    if named == "*" {
        return true;
    }
    let Some((prefix, name)) = named.split_once(':') else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case(ACTION_PREFIX) {
        return false;
    }
    policy_action_names()
        .iter()
        .any(|action| iam::wildcard_match(name, action, false))
}

/// Whether `named` names a principal that exists — or one this account cannot be the
/// authority on, which is accepted.
///
/// This account IS the authority on its own identities, so an ARN naming an identity of
/// this account that the IAM store does not hold names a principal that does not exist.
/// Everything else — the wildcard, another account, an account's root user — is accepted:
/// a resource policy may name a principal this emulator holds no record of.
fn names_a_principal<Eng: Send + Sync + 'static>(engine: &Eng, named: &str) -> bool {
    if named == "*" || is_account_id(named) {
        return true;
    }
    if !named.starts_with("arn:") {
        return false;
    }
    let account = ::engine::emulator_account_id();
    if named == account_root_arn(account) {
        return true;
    }
    // An identity of THIS account: the store is the authority, so the identity has to be
    // one it holds.
    if let Some(entity) = iam_store::parse_entity_arn(named) {
        if entity.account != account {
            return true;
        }
        let Some(store) = iam_store::service_state(engine) else {
            return true;
        };
        let store = store.lock();
        let key = iam_store::name_key(&entity.name);
        return match entity.kind {
            iam_store::EntityKind::Role => store.roles.contains_key(&key),
            iam_store::EntityKind::User => store.users.contains_key(&key),
            iam_store::EntityKind::Group => store.groups.contains_key(&key),
        };
    }
    // An assumed-role session of this account names the role it is a session of.
    if let Some(role) = assumed_role_of(named) {
        let Some(store) = iam_store::service_state(engine) else {
            return true;
        };
        let store = store.lock();
        if role.0 == account {
            return store.roles.contains_key(&iam_store::name_key(role.1));
        }
    }
    // Every other ARN form — another service's resource, another account's root — is one
    // this account cannot judge.
    true
}

/// The `(account, role name)` an STS assumed-role session ARN names, or `None` for any
/// other ARN.
fn assumed_role_of(arn: &str) -> Option<(&str, &str)> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some("sts"), Some(_region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!name.is_empty()).then_some((account, name))
}

/// The policy document as this service STORES it: a `Principal` naming a bare account id
/// is held as that account's root user ARN, which is the spelling AWS normalizes it to and
/// therefore the one every later read reports.
///
/// A document that needs no such rewrite is stored exactly as it was supplied, so a client
/// that wrote a canonical document reads its own bytes back.
pub fn normalised_policy(document: &str) -> String {
    let Ok(mut parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return document.to_string();
    };
    if !normalise_statements(&mut parsed) {
        return document.to_string();
    }
    ::serde_json::to_string(&parsed).unwrap_or_else(|_| document.to_string())
}

/// Rewrite every bare-account-id principal of `document` into that account's root ARN,
/// answering whether anything changed.
fn normalise_statements(document: &mut ::serde_json::Value) -> bool {
    let Some(element) = document.get_mut("Statement") else {
        return false;
    };
    match element {
        ::serde_json::Value::Array(statements) => {
            let mut rewritten = false;
            for statement in statements.iter_mut() {
                rewritten |= normalise_statement(statement);
            }
            rewritten
        }
        statement => normalise_statement(statement),
    }
}

/// Rewrite the bare-account-id principals of ONE statement.
fn normalise_statement(statement: &mut ::serde_json::Value) -> bool {
    let mut rewritten = false;
    for element in ["Principal", "NotPrincipal"] {
        if let Some(::serde_json::Value::Object(map)) = statement.get_mut(element) {
            if let Some(aws) = map.get_mut("AWS") {
                rewritten |= normalise_account_ids(aws);
            }
        }
    }
    rewritten
}

/// Rewrite the bare account ids in one `AWS` principal value — a string or a list of them.
fn normalise_account_ids(value: &mut ::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(named) if is_account_id(named) => {
            *named = account_root_arn(named);
            true
        }
        ::serde_json::Value::Array(values) => {
            let mut rewritten = false;
            for value in values.iter_mut() {
                rewritten |= normalise_account_ids(value);
            }
            rewritten
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_call_is_authorized_under_its_single_message_action() {
        // (the operation, the action it is decided under)
        let rows: &[(&str, &str)] = &[
            ("SendMessage", "sqs:SendMessage"),
            ("SendMessageBatch", "sqs:SendMessage"),
            ("DeleteMessageBatch", "sqs:DeleteMessage"),
            ("ChangeMessageVisibilityBatch", "sqs:ChangeMessageVisibility"),
            ("DeleteQueue", "sqs:DeleteQueue"),
        ];
        for (operation, expected) in rows {
            let actual = action_for(operation);
            assert_eq!(
                actual, *expected,
                "case {operation:?}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn an_action_element_names_an_action_of_this_service_or_nothing() {
        // (the Action value, whether it names an action of this service)
        let rows: &[(&str, bool)] = &[
            ("*", true),
            ("sqs:*", true),
            ("sqs:SendMessage", true),
            // IAM compares action names case-insensitively.
            ("sqs:sendmessage", true),
            ("sqs:Get*", true),
            ("sqs:ListQueueTags", true),
            // Every action name of this service is one a policy may name: how far a grant
            // reaches is decided at call time, not when the document is stored.
            ("sqs:CreateQueue", true),
            ("sqs:SendMessageBatch", true),
            ("sqs:NotARealAction", false),
            ("sqs:", false),
            ("s3:GetObject", false),
            ("SendMessage", false),
        ];
        for (named, expected) in rows {
            let actual = names_an_action(named);
            assert_eq!(
                actual, *expected,
                "case {named:?}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn a_bare_account_id_principal_is_stored_as_that_accounts_root_arn() {
        let root = account_root_arn("210987654321");
        // (case, the document, the document as it is stored)
        let rows: &[(&str, String, String)] = &[
            (
                "a bare account id",
                r#"{"Statement":[{"Principal":{"AWS":"210987654321"}}]}"#.to_string(),
                format!(r#"{{"Statement":[{{"Principal":{{"AWS":"{root}"}}}}]}}"#),
            ),
            (
                "a root ARN, already canonical",
                format!(r#"{{"Statement":[{{"Principal":{{"AWS":"{root}"}}}}]}}"#),
                format!(r#"{{"Statement":[{{"Principal":{{"AWS":"{root}"}}}}]}}"#),
            ),
            (
                "a document with no principal at all",
                r#"{"Pad":"pp"}"#.to_string(),
                r#"{"Pad":"pp"}"#.to_string(),
            ),
            (
                "a value that is not a policy",
                "not a policy".to_string(),
                "not a policy".to_string(),
            ),
        ];
        for (case, document, expected) in rows {
            let actual = normalised_policy(document);
            assert_eq!(
                &actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }
}
