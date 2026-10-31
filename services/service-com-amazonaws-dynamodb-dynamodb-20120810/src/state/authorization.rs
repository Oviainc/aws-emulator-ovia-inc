// SPDX-License-Identifier: CC0-1.0

//! WHO a data-plane request speaks for, and whether the policies in force let it act on
//! the table it addresses.
//!
//! A table's resource-based policy is evaluated on every call that reads or writes an item
//! of that table, at the instant of the call — nothing is cached, so a policy just put
//! refuses immediately and one just deleted stops refusing immediately.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the
//!    one the caller reads back from STS for the credentials it signed with. Resolving it
//!    materialises the session expiries the clock has already made due, a write owed at
//!    that instant whether or not anybody called.
//! 2. **what the TABLE's own document says** — [`super::AttachedPolicy`], the document
//!    `PutResourcePolicy` parsed and this store holds, evaluated by the matcher that lives
//!    with it: a `Deny` about the request refuses it whatever any grant says, and an
//!    `Allow` that names the caller itself grants it.
//! 3. **what the CALLER is allowed to do at all** — the identity-based documents the IAM
//!    store holds for that principal, judged by [`::service_common::iam_authorization`],
//!    the tree's one evaluation of policy documents against `(action, resource,
//!    principal)`, so this service and every other one handed the same documents decide
//!    alike. These documents are IAM's, not this service's, which is why they are not read
//!    with the matcher above.
//!
//! ## The rule
//!
//! In IAM's own order: an explicit `Deny` about the request — in the table's policy or in
//! an identity-based document — refuses, whatever any grant says. Otherwise the request
//! needs a grant: an `Allow` about it in the caller's identity-based policies, or one in
//! the table's policy that names the CALLER. With neither the request is refused, which is
//! IAM's implicit deny, and the operation has no effect.
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
//!   credentials are in: the IAM store holds no policies for it and the table's owner has
//!   full control of its tables, so such a request is allowed — while an explicit `Deny`
//!   in the table's policy still refuses it.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy is owned by AWS, not by the account, so the account's store holds no document
//!   for it; refusing on evidence this emulator does not have would deny access AWS
//!   permits, so such a principal is allowed where the readable documents neither grant
//!   nor deny.
//!
//! An expired session credential speaks for nobody: no statement names it, so every action
//! it asks for is refused.

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
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
}

/// Who this request speaks for, resolved from the access key id it signed with.
///
/// Call it before locking this service's store (see [`Caller`]).
pub fn request_caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let iam = sts_store::iam_state(engine);
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
        .map(|caller| (caller.identity.arn, caller.identity.account, caller.role_arn)),
        // No STS store on this engine: the credential names no session, so it speaks for
        // the long-term identity it was resolved to.
        None => Ok((
            long_term.arn.clone(),
            long_term.account.clone(),
            None,
        )),
    };
    // An expired session credential speaks for nobody: it carries no documents and no
    // principal spelling. The refusal still names what was presented, so a caller reading
    // it can tell an expired credential from an unauthorized one.
    let Ok((arn, account, role_arn)) = resolved else {
        return Caller {
            arn: format!("an expired session credential ({access_key_id})"),
            ..Caller::default()
        };
    };
    let mut caller = Caller {
        account_credential: arn == sts_store::ambient_identity().arn,
        principals: vec![arn.clone()],
        account_principals: iam::account_principals(&arn),
        arn,
        ..Caller::default()
    };
    // An assumed-role session is also named by the ARN of the role it is a session of,
    // which is the spelling a policy uses to name the role itself, and the identity whose
    // policies are in force for it.
    if let Some(role_arn) = &role_arn {
        if !caller.principals.contains(role_arn) {
            caller.principals.push(role_arn.clone());
        }
    }
    if caller.account_principals.is_empty() {
        caller.account_principals = vec![account_root_arn(&account), account];
    }
    let identity_arn = role_arn.unwrap_or_else(|| caller.arn.clone());
    if let Some(iam) = iam {
        if let Some(policies) = iam.lock().principal_policies(&identity_arn) {
            caller.documents = policies.documents;
            caller.unreadable_attachment = !policies.uncarried.is_empty();
        }
    }
    caller
}

/// The ARN of an account's root user, the spelling that stands for the whole account in a
/// policy's `Principal` element.
fn account_root_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:root")
}

/// The `AccessDeniedException` message a data-plane request earns when the policies in
/// force do not permit it, or `None` when they do.
///
/// `action` is the IAM action the operation performs (`dynamodb:GetItem`). The decision is
/// the module docs' rule, over the table's own policy and the caller's identity-based
/// documents; a table with no policy attached — or one whose application is not due yet —
/// states nothing, so such a request is decided by the identity-based half alone.
pub fn denial(caller: &Caller, table: &Table, action: &str, now: SystemTime) -> Option<String> {
    let refusal = |reason: &str| {
        Some(format!(
            "User: {} is not authorized to perform: {action} on resource: {} {reason}",
            caller.arn, table.arn
        ))
    };
    // "PutResourcePolicy is an asynchronous operation" and "the policy application is
    // eventually consistent" (the `PutResourcePolicy` doc): a policy whose application is
    // not due yet states nothing about this request.
    let policy = table
        .resource_policy
        .as_ref()
        .filter(|policy| now >= policy.effective_at);

    // An explicit deny wins over every grant, including the account's own credential.
    if let Some(policy) = policy {
        if let Some(conditioned) = policy.denies_any(&caller.denied_spellings(), action, &table.arn)
        {
            let mut message = refusal("with an explicit deny in a resource-based policy")?;
            if conditioned {
                // Say what was not evaluated rather than imply the condition matched.
                message.push_str(
                    " (the statement carries a Condition, which this emulator does not \
                     evaluate: it refuses rather than serves what the policy may deny)",
                );
            }
            return Some(message);
        }
    }
    // The account's own published credential holds no IAM policies, and the account owns
    // its tables (see the module docs).
    if caller.account_credential {
        return None;
    }

    let granting = iam::Request::new(action, &table.arn, &caller.principals);
    match iam::identity_decision(&caller.documents, &granting) {
        Decision::Denied => return refusal("with an explicit deny in an identity-based policy"),
        // A grant in the TABLE's own policy names this principal directly, which is a
        // permission of its own: "the resource-based policy and the identity-based policy
        // are evaluated together, and access is granted if either allows it".
        Decision::Allowed => return None,
        Decision::NotStated => {}
    }
    if policy.is_some_and(|policy| policy.grants_any(&caller.principals, action, &table.arn)) {
        return None;
    }
    // A grant this emulator cannot read is not evidence of a refusal (see the module
    // docs), so a principal carrying one is allowed where the readable documents neither
    // grant nor deny.
    if caller.unreadable_attachment {
        return None;
    }
    refusal(&format!(
        "because no identity-based policy allows the {action} action"
    ))
}
