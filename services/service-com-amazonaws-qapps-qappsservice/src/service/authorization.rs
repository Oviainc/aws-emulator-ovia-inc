// SPDX-License-Identifier: CC0-1.0

//! The OTHER front door: whether plain IAM lets a request to the resource-tagging
//! surface act on the ARN it names.
//!
//! # The rule
//!
//! `ListTagsForResource`, `TagResource` and `UntagResource` are the AWS-standard
//! tagging operations, and the only three of this service whose model declares no
//! `UnauthorizedException` (`smithy_query(query="digest")`): they are not gated on an
//! IAM Identity Center identity ([`super::identity_center`]) but authorized through
//! the caller's own IAM policy against the ARN the request addresses, and refused
//! `AccessDeniedException` (HTTP 403, `com.amazonaws.qapps#AccessDeniedException`,
//! required `message`) when it does not permit the action. The decision is taken on
//! the ARN the REQUEST carried, before the store is read, so a refused call says
//! nothing about whether that resource exists.
//!
//! Two halves, in IAM's own order:
//!
//! 1. **the account boundary.** An identity-based policy grants access to resources of
//!    the account that holds the policy, and reaching ACROSS accounts additionally
//!    needs the resource's own policy to allow it — which this service publishes none
//!    of: its model declares no policy operation and no policy member. So an ARN
//!    outside the caller's account is refused whatever the caller carries, and so is a
//!    value that names no Q Apps resource at all ([`crate::state::tagged_resource`]):
//!    there is no resource for a statement to be about.
//! 2. **what the caller is allowed to do**, judged by
//!    [`::service_common::iam_authorization`] — the tree's ONE evaluation of policy
//!    documents against `(action, resource, principal)`, so this service and every
//!    other one handed the same documents decide alike: an explicit `Deny` about the
//!    request refuses it whatever any grant says, otherwise it needs an `Allow` about
//!    it, and with neither it falls to IAM's default deny.
//!
//! The action is `qapps:<OperationName>` ("Actions defined by Amazon Q Apps", IAM
//! Service Authorization Reference), whose prefix is this service's own
//! [`crate::state::SIGNING_NAME`].
//!
//! # Where the pieces come from
//!
//! * **who** — the principal the request's access key id speaks for, read from the
//!   SAME place `sts:GetCallerIdentity` answers from, so the principal a refusal
//!   blames is the one the caller reads back for the credentials it signed with;
//! * **what it may do** — the identity-based documents the IAM store holds for that
//!   principal, through IAM's own `principal_policies` accessor, where the collection
//!   rule lives: a policy `iam:PutUserPolicy` wrote is in force on the next request
//!   and deleting it lifts it again.
//!
//! Only those LOOKUPS live here; the decision does not. That is the split every other
//! gate in this tree makes (Amazon DynamoDB, AWS Certificate Manager, Amazon
//! OpenSearch Service and the rest), and the lookups stay in the service crate for the
//! reason those keep theirs there: hoisting them into `service-common` would make
//! every service that depends on that crate link the IAM and STS service crates.
//!
//! # The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither
//!   STS nor IAM holds a record for speaks for the account's own identity, which is not
//!   a principal the IAM store holds policies for — the position AWS account root
//!   credentials are in — and the account owns its resources, so such a request is
//!   allowed inside its own account.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the account's store holds no
//!   document for it; refusing on a grant this emulator cannot see would deny access
//!   AWS permits. An explicit `Deny` in a document that CAN be read still refuses.
//! * **An engine carrying no STS or IAM store**, where no session and no policy can
//!   exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for falls to the default deny.

use std::time::SystemTime;

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;
use crate::state::{tagged_resource, TaggedResource, SIGNING_NAME};

/// What a refusal names as the principal of a request whose session credential has
/// already expired: that credential speaks for nobody, and the key it presented is
/// not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// Authorize a tagging request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource_arn` the ARN the request
/// NAMES.
///
/// `Ok(resource)` — the Q Apps resource that ARN addresses — for a request IAM
/// permits; the modelled refusal otherwise, which each operation returns as its own
/// `AccessDeniedException`. Call it BEFORE the store is read or written: that is what
/// keeps a refused call from telling an unauthorized caller whether the resource
/// exists.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first
/// materialises the session expiries the clock has already made due; that write is
/// owed at the same instant whether or not anybody called, which is why the REPORTING
/// operation of this surface may take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource_arn: &str,
) -> Result<TaggedResource, AccessDeniedException> {
    let action = format!("{SIGNING_NAME}:{operation}");
    let caller = caller(engine, ctx);
    // The account boundary (see the module docs). A value that names no Q Apps
    // resource, and one naming another account's, are one answer: not a resource any
    // identity-based policy of the caller's account grants.
    let addressed = tagged_resource(resource_arn)
        .filter(|_| arn_account(resource_arn) == Some(caller.account.as_str()));
    let Some(addressed) = addressed else {
        return Err(not_authorized(&caller.arn));
    };

    let request = iam::Request::new(&action, resource_arn, &caller.principals);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be
    // read refuses the request whatever else the principal carries — including a grant
    // this emulator cannot read, which only lifts the DEFAULT deny.
    match iam::identity_decision(&caller.documents, &request) {
        Decision::Denied => Err(not_authorized(&caller.arn)),
        Decision::Allowed => Ok(addressed),
        Decision::NotStated if caller.is_unbounded() => Ok(addressed),
        Decision::NotStated => Err(not_authorized(&caller.arn)),
    }
}

/// The account field of the ARN `value`, or `None` when `value` is not a well-formed
/// resource ARN or names no account.
fn arn_account(value: &str) -> Option<&str> {
    ::service_common::arn::parse(value)
        .map(|arn| arn.account)
        .filter(|account| !account.is_empty())
}

/// The refusal Q Apps states an authorization failure with: WHO was refused, and
/// nothing else.
///
/// The sentence itself is the tree's ONE spelling of that denial
/// ([`iam::not_authorized_to_access_resource`]), the denial of a request decided from the
/// ARN it carries rather than from a policy evaluation: it names neither an action nor a
/// resource, because an unauthorized caller must not learn from a refusal what exists in
/// an account that is not its own. The principal is the one the request's own credential
/// speaks for.
fn not_authorized(principal: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: iam::not_authorized_to_access_resource(principal),
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal
    /// names.
    arn: String,
    /// The account the principal belongs to: the account whose resources its
    /// identity-based policies can grant.
    account: String,
    /// Every spelling of the principal a statement's `Principal` element may name it
    /// by: the ARN STS reports, and the ARN of the IAM role behind an assumed-role
    /// session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// Set when this request presents the account's own published credential, which
    /// holds no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal carries an attachment whose document the IAM store does
    /// not hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing readable can refuse this caller once no document DENIES the
    /// action: the account's own credential, or a principal carrying a grant this
    /// emulator cannot read.
    fn is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }
}

/// Who this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::service_common::request_access_key_id(ctx.parts());
    let iam = sts_store::iam_state(engine);
    // A long-term access key speaks for the IAM user that holds it, which is the
    // identity STS itself reports for it; a key no store holds is the account's own
    // published credential.
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
        // No STS store on this engine: the credential names no session, so it speaks
        // for the long-term identity it was resolved to.
        None => Ok((long_term.arn.clone(), long_term.account.clone(), None)),
    };
    let Ok((arn, account, role_arn)) = resolved else {
        // The session credential has expired, so it speaks for nobody: no documents,
        // no principal spelling, and no account whose resources it could reach.
        return Caller {
            arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
            account: String::new(),
            principals: Vec::new(),
            documents: Vec::new(),
            account_credential: false,
            unreadable_attachment: false,
        };
    };
    let mut caller = Caller {
        account_credential: arn == sts_store::ambient_identity().arn,
        principals: vec![arn.clone()],
        arn,
        account,
        documents: Vec::new(),
        unreadable_attachment: false,
    };
    // An assumed-role session is also named by the ARN of the ROLE it is a session of,
    // which is the spelling a policy uses to name the role itself and the identity
    // whose policies are in force for it.
    let identity_arn = match role_arn {
        Some(role_arn) => {
            if !caller.principals.contains(&role_arn) {
                caller.principals.push(role_arn.clone());
            }
            role_arn
        }
        None => caller.arn.clone(),
    };
    if let Some(iam) = iam {
        if let Some(policies) = iam.lock().principal_policies(&identity_arn) {
            caller.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of
            // AWS's own catalog this emulator does not carry: a grant it cannot read.
            caller.unreadable_attachment = !policies.uncarried.is_empty();
        }
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the refusal names the principal the request's own credential speaks for,
    /// and states no action and no resource — whatever principal form it is.
    ///
    /// The sentence is spelled out here rather than read back from the shared helper the
    /// handler calls, so the wording is pinned independently of it.
    #[test]
    fn the_refusal_names_the_caller_and_nothing_about_the_resource() {
        // (case, the principal)
        let rows: &[(&str, &str)] = &[
            ("an IAM user", "arn:aws:iam::000000000000:user/emulator"),
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/qapps-reader/session-1",
            ),
            ("the account's own identity", "arn:aws:iam::000000000000:root"),
            ("an expired credential", EXPIRED_CREDENTIAL_PRINCIPAL),
        ];
        for (case, principal) in rows {
            let message = not_authorized(principal).message;
            assert_eq!(
                message,
                format!("User: {principal} is not authorized to access this resource"),
                "case {case:?}: expected the Q Apps access-denied wording, got {message:?}"
            );
            assert!(
                !message.contains("arn:aws:qapps"),
                "case {case:?}: expected no resource in the refusal, got {message:?}"
            );
        }
    }

    /// RULE: the action asked about is `qapps:<OperationName>`, so a policy scoped to
    /// this service's actions covers its tagging surface and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"qapps:*","Resource":"*"}]}"#;
        let documents = [document.to_string()];
        for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
            let action = format!("{SIGNING_NAME}:{operation}");
            let reached = iam::identity_decision(
                &documents,
                &iam::Request::new(&action, "arn:aws:qapps:us-east-1:000000000000:application/i/qapp/a", &[]),
            );
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected qapps:* to allow {action}, got {reached:?}"
            );
        }
        let outside = iam::identity_decision(
            &documents,
            &iam::Request::new("s3:GetObject", "arn:aws:s3:::bucket/key", &[]),
        );
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected qapps:* to grant nothing outside this service, \
             got {outside:?}"
        );
    }

    /// RULE: the account boundary is read off the ARN's own account field, through the
    /// one ARN reader — so a value that is no ARN names no account at all.
    #[test]
    fn the_account_boundary_is_read_from_the_arn() {
        // (case, the value, the account it names)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "this account's app",
                "arn:aws:qapps:us-east-1:000000000000:application/i/qapp/a",
                Some("000000000000"),
            ),
            (
                "another account's app",
                "arn:aws:qapps:us-east-1:111122223333:application/i/qapp/a",
                Some("111122223333"),
            ),
            (
                "an account-less ARN",
                "arn:aws:qapps:us-east-1::application/i/qapp/a",
                None,
            ),
            ("a value that is no ARN", "application/i/qapp/a", None),
        ];
        for (case, value, expected) in rows {
            let actual = arn_account(value);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected the account {expected:?}, got {actual:?}"
            );
        }
    }
}
