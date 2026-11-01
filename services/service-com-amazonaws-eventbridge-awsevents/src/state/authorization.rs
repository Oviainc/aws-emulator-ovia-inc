// SPDX-License-Identifier: CC0-1.0

//! WHO a publication speaks for, and whether the policies in force let that principal
//! put events onto the bus its entry names.
//!
//! Publishing an event is the action [`PUT_EVENTS_ACTION`] on the event bus the entry
//! names, so it is authorized the way AWS authorizes any call on a resource: against the
//! calling principal's identity-based policies and the bus's own resource policy
//! together, at the instant of the call. Nothing is cached — a grant just written grants
//! immediately, and one just revoked stops granting immediately.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the IAM
//!    store of the same shared engine, which holds both halves of that question: the
//!    registry `iam:CreateAccessKey` wrote for a long-term key, and the role-session
//!    record the minting operation left for a temporary one. So the principal a refusal
//!    blames is the one the account really holds for the credential the request signed
//!    with.
//! 2. **what the caller may do at all** — the identity-based documents the IAM store holds
//!    for that principal, read through IAM's own `principal_policies` accessor so this
//!    service sees exactly what `PutUserPolicy`/`AttachRolePolicy` wrote.
//! 3. **what the BUS's own document says** — the policy `PutPermission` installs and
//!    `DescribeEventBus` reports ([`BusPolicy::document`]).
//!
//! The decision itself is [`::service_common::iam_authorization`], the tree's ONE
//! evaluation of policy documents against `(action, resource, principal)`, so this service
//! and every other one handed the same documents decide alike. The statements are evaluated
//! with the publication's own condition context
//! ([`publication_condition_context`]), so a statement scoped to one event's `Source`
//! decides that event and no other.
//!
//! ## The rule
//!
//! In IAM's own order: an explicit `Deny` about the publication — in the bus's policy or in
//! an identity-based document — refuses it, whatever any grant says. Otherwise the
//! publication needs a grant: an `Allow` about it in the caller's identity-based policies,
//! or one in the bus's policy that names the CALLER. With neither the publication is
//! refused, which is IAM's implicit deny, and nothing of the request is published.
//!
//! A statement that names the ACCOUNT rather than the caller (the account's root ARN, or
//! the bare account id that stands for it) delegates to the account rather than granting:
//! "you must also attach an identity-based policy" to the principal. So the account
//! spellings decide a `Deny` — which covers every principal of the account — and never make
//! a grant on their own.
//!
//! ## The two callers that are not IAM principals
//!
//! * **The account's own published credential**, which is the position AWS account root
//!   credentials are in: the IAM store holds no policies for it and the account has full
//!   control of its own buses, so such a publication is allowed — while an explicit `Deny`
//!   in the bus's policy still refuses it. An emulated AWS service publishing its own
//!   events through the in-process callback signs with that credential, so a service's own
//!   events are never refused for want of a grant.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   is owned by AWS, not by the account, so the account's store holds no document for it;
//!   refusing on evidence this emulator does not have would deny access AWS permits, so
//!   such a principal is allowed where the readable documents neither grant nor deny.

use ::service_common::iam_authorization::{self as iam, Decision};

use super::*;

/// The principal a request speaks for, and everything the decision is made from.
///
/// Resolved once per request, BEFORE this service's own store is locked, so the STS and
/// IAM stores are never reached while this service's state guard is held.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// Every spelling of the principal a policy may name IT by: the ARN STS reports, and
    /// the ARN of the IAM role behind an assumed-role session. These decide a GRANT.
    pub principals: Vec<String>,
    /// The spellings that name the ACCOUNT the principal belongs to rather than the
    /// principal: they decide a deny and never grant (see the module docs).
    pub account_principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    pub account_credential: bool,
    /// Set when the principal carries a policy whose document the IAM store does not hold
    /// — an AWS-managed policy (see the module docs).
    pub unreadable_attachment: bool,
}

impl Caller {
    /// Every spelling a DENY is decided against: the principal's own and its account's,
    /// because a statement denying the account denies each of its principals.
    fn denied_spellings(&self) -> Vec<String> {
        let mut named = self.principals.clone();
        named.extend(self.account_principals.iter().cloned());
        named
    }

    /// Whether the policies in force need to be consulted at all for this caller: the
    /// account's own credential carries no identity-based documents, so with no resource
    /// policy on the bus there is no document that could state anything about the
    /// publication.
    #[must_use]
    pub fn decided_by_no_document(&self) -> bool {
        self.account_credential
    }
}

/// Who this request speaks for, resolved from the access key id it signed with.
///
/// The question "which principal does this access key id speak for" is one about the
/// account's IDENTITIES, and the IAM store of the same shared engine answers both halves
/// of it: a long-term key through the registry `iam:CreateAccessKey` wrote, and a
/// TEMPORARY credential of a role session through the record the minting operation left
/// there ([`::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::RoleSessionRecord`],
/// which exists so a service deciding an authorization can reach the role a session is
/// of). So no other service's store is consulted here.
///
/// A key the account holds no live record for is the account's OWN published credential,
/// which is the same reading every service of this emulator takes of one: whether a
/// credential verifies at all is the emulator's authentication layer's decision, taken
/// from the credential store before any handler runs, so a credential that has been
/// withdrawn never reaches this function.
///
/// Call it before locking this service's store (see [`Caller`]).
pub fn request_caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let Some(iam) = iam_state(engine) else {
        // No IAM store on this engine: the account holds no identity for the key to
        // speak for, so the request is the account's own.
        return account_caller();
    };
    let store = iam.lock();
    let now = ::aws_smithy_types::DateTime::from(SystemTime::now());
    // A temporary credential of a role session: the principal is the SESSION, and the
    // policies in force are the ROLE's ("the temporary security credentials … have the
    // permissions of the role", `AssumeRole`). The role's own ARN is also a spelling a
    // policy names the caller by, so a statement about the role decides the session.
    if let Some(session) = store.role_session(&access_key_id, now) {
        let mut caller = named_caller(&session.session_arn);
        if !caller.principals.contains(&session.role_arn) {
            caller.principals.push(session.role_arn.clone());
        }
        let role_arn = session.role_arn.clone();
        read_documents(&store, &role_arn, &mut caller);
        return caller;
    }
    // A long-term access key speaks for the IAM user that holds it, which is the identity
    // `sts:GetCallerIdentity` reports for it.
    let user_arn = store
        .access_keys
        .get(&access_key_id)
        .and_then(|record| store.users.get(&record.user_key))
        .map(|user| user.arn.clone());
    match user_arn {
        Some(user_arn) => {
            let mut caller = named_caller(&user_arn);
            read_documents(&store, &user_arn, &mut caller);
            caller
        }
        None => account_caller(),
    }
}

/// The caller a request presenting the account's own published credential speaks for: the
/// principal `sts:GetCallerIdentity` reports for it, which the IAM store holds no
/// policies for (see the module docs).
fn account_caller() -> Caller {
    Caller {
        account_credential: true,
        ..named_caller(&caller_principal())
    }
}

/// The caller `arn` names, with the spellings that name its ACCOUNT beside it and no
/// documents read yet.
fn named_caller(arn: &str) -> Caller {
    let mut account_principals = iam::account_principals(arn);
    if account_principals.is_empty() {
        account_principals = vec![account_root_arn(::engine::emulator_account_id())];
    }
    Caller {
        arn: arn.to_string(),
        principals: vec![arn.to_string()],
        account_principals,
        ..Caller::default()
    }
}

/// Read the identity-based documents in force on the IAM identity `identity_arn` onto
/// `caller`, through IAM's own accessor — so this service sees exactly what
/// `PutUserPolicy`/`AttachRolePolicy` wrote, and a document it cannot read is recorded as
/// such rather than counted as absent.
fn read_documents(
    store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::ComAmazonawsIamAwsidentitymanagementv20100508State,
    identity_arn: &str,
    caller: &mut Caller,
) {
    if let Some(policies) = store.principal_policies(identity_arn) {
        caller.documents = policies.documents;
        caller.unreadable_attachment = !policies.uncarried.is_empty();
    }
}

/// The ARN of an account's root user, the spelling that stands for the whole account in a
/// policy's `Principal` element.
fn account_root_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:root")
}

/// The `AccessDeniedException` message a publication onto the bus `bus_arn` names earns
/// when the policies in force do not permit it, or `None` when they do.
///
/// `policy` is the bus's own resource policy document, when it carries one, and `context`
/// the publication's condition context ([`publication_condition_context`]). The decision is
/// the module docs' rule: an explicit deny in either half refuses, otherwise one of the two
/// halves has to grant.
#[must_use]
pub fn publication_denial(
    caller: &Caller,
    bus_arn: &str,
    policy: Option<&str>,
    context: &[(String, String)],
) -> Option<String> {
    let denied = caller.denied_spellings();
    // Every spelling of the caller, the account ones included: this is the comparison a
    // DENY is decided under, because a statement denying the account denies each of its
    // principals.
    let denying = iam::Request::new(PUT_EVENTS_ACTION, bus_arn, &denied)
        .with_context(context)
        .with_published_keys(&PUBLICATION_CONDITION_KEYS);
    // The caller's own spellings only: this is the comparison a GRANT is decided under,
    // because naming the account delegates rather than grants.
    let granting = iam::Request::new(PUT_EVENTS_ACTION, bus_arn, &caller.principals)
        .with_context(context)
        .with_published_keys(&PUBLICATION_CONDITION_KEYS);

    // An explicit deny wins over every grant, including the account's own credential.
    if policy.is_some_and(|document| iam::resource_decision(document, &denying) == Decision::Denied)
    {
        return Some(explicit_resource_policy_deny(
            &caller.arn,
            PUT_EVENTS_ACTION,
            bus_arn,
        ));
    }
    // The account's own published credential holds no IAM policies, and the account owns
    // its own buses (see the module docs).
    if caller.account_credential {
        return None;
    }
    match iam::identity_decision(&caller.documents, &granting) {
        Decision::Denied => {
            return Some(super::not_authorized(
                &caller.arn,
                PUT_EVENTS_ACTION,
                bus_arn,
                "with an explicit deny in an identity-based policy",
            ))
        }
        Decision::Allowed => return None,
        Decision::NotStated => {}
    }
    // A grant in the BUS's own policy names this principal directly, which is a permission
    // of its own: "the resource-based policy and the identity-based policy are evaluated
    // together, and access is granted if either allows it".
    if policy
        .is_some_and(|document| iam::resource_decision(document, &granting) == Decision::Allowed)
    {
        return None;
    }
    // A grant this emulator cannot read is not evidence of a refusal (see the module
    // docs), so a principal carrying one is allowed where the readable documents neither
    // grant nor deny.
    if caller.unreadable_attachment {
        return None;
    }
    Some(super::not_authorized(
        &caller.arn,
        PUT_EVENTS_ACTION,
        bus_arn,
        &format!("because no identity-based policy allows the {PUT_EVENTS_ACTION} action"),
    ))
}
