// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the caller HAND THIS SERVICE A ROLE — the `iam:PassRole`
//! authorization step a control-plane operation takes before it writes anything.
//!
//! A state machine's `roleArn` is "the Amazon Resource Name (ARN) of the IAM role to
//! use for this state machine" (`CreateStateMachine$roleArn`): the role every
//! execution of it acts as. Naming it in a request therefore HANDS the service a role,
//! which AWS authorizes as its own action — a caller may only pass a role its
//! identity-based policies allow it to pass — and refuses with `AccessDeniedException`
//! (HTTP 400) when they do not. Without that step a caller that holds no `iam:PassRole`
//! at all can give a state machine any role in the account and then make it act as
//! that role, which is a privilege escalation; it is also why a client's
//! least-privilege test passes here and fails in production.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the
//!    one the caller reads back for the credentials it signed with), else the IAM user
//!    that owns the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`,
//!    `permissions_boundary_document`), so a policy `iam:PutRolePolicy` wrote takes
//!    effect on the next request and deleting it lifts the refusal again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of policy documents against a request, so this service and every
//!    other one handed the same documents decide alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request
//!    falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the ARN readings do —
//! the same split the gates of this tree make (`service/authorization.rs` in Amazon
//! Textract is the one this module follows), and the lookups stay in the service crate
//! because hoisting them into `service-common` would make every service depending on
//! that crate link the IAM and STS service crates.
//!
//! ## The callers this gate does not bound
//!
//! Nothing is refused on evidence this emulator does not have:
//!
//! * **the account's own published credential**, which no IAM record holds policies
//!   for — the position AWS account root credentials are in — may pass any role;
//! * **a principal carrying an attachment whose document this account does not hold**
//!   (an AWS-managed policy belongs to AWS, so the store holds no document for it) is
//!   not refused on a grant that cannot be read, though an explicit `Deny` in a
//!   document that CAN be read still refuses it and a permissions boundary the store
//!   DOES hold still caps it;
//! * **an engine carrying no STS or IAM store**, where no session and no policy can
//!   exist.
//!
//! Whether the role the request names EXISTS is deliberately not part of this: the
//! control plane does not resolve a state machine's role, which is the deploy-time /
//! run-time split this service exposes (`CreateStateMachine` takes a role the account
//! does not hold, and the data plane is where assuming it fails —
//! [`crate::state::task_authorization`]). `iam:PassRole` is about what the CALLER may
//! hand over, so it is decided from the caller's documents and the ARN the request
//! carried, which is also what keeps a refused call from telling an unauthorized
//! caller whether a role exists.

use ::std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The IAM action a request that hands this service a role performs: "To pass a role
/// to a service, a user must have the `iam:PassRole` permission".
pub const PASS_ROLE_ACTION: &str = "iam:PassRole";

/// `iam:PassedToService` — IAM's condition key for the SERVICE a role is being passed
/// to, which AWS guidance tells a caller to scope every `iam:PassRole` grant with, and
/// which IAM populates from the call itself rather than from the request.
///
/// The service this request would hand the role to is this one, so the value is this
/// service's own principal — read from the one place this crate spells it
/// ([`crate::state::task_authorization::SERVICE_PRINCIPAL`]), the principal a role this
/// service acts as has to trust — and a grant conditioned on another service's
/// principal does not cover the pass.
const PASSED_TO_SERVICE_CONDITION_KEY: &str = "iam:PassedToService";

/// What a refusal names as the principal of a request whose session credential has
/// already expired: that credential speaks for nobody, and the key it presented is not
/// quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// Why this caller may not hand the role `role_arn` to this service, or `None` when it
/// may — including every caller this gate does not bound (see the module docs).
///
/// Call it BEFORE the store is read or written, and with the ARN the request CARRIED
/// rather than one resolved from a stored record, so the decision does not depend on
/// whether that role exists.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first
/// materialises the session expiries the clock has already made due; that write is
/// owed at the same instant whether or not anybody called.
pub fn pass_role_denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    role_arn: &str,
) -> Option<String> {
    let caller = caller(engine, ctx);
    let context = pass_context();
    let request =
        iam::Request::new(PASS_ROLE_ACTION, role_arn, &caller.principals).with_context(&context);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be
    // read refuses the pass whatever else the principal carries — including a grant
    // this emulator cannot read, which only lifts the DEFAULT deny.
    if identity_said == Decision::Denied {
        return Some(not_authorized(
            &caller.arn,
            role_arn,
            "with an explicit deny in an identity-based policy",
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Some(not_authorized(
            &caller.arn,
            role_arn,
            &format!("because no identity-based policy allows the {PASS_ROLE_ACTION} action"),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based
    // policy can grant": a grant the boundary does not also allow is not a permission
    // the principal has. The account's own credential carries none.
    let boundary = caller.boundary.as_ref()?;
    let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
        Decision::Allowed => return None,
        // An explicit deny is a fact about the boundary document, so the refusal names
        // it; a boundary that merely states nothing withheld the grant.
        Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
        Decision::NotStated => {
            format!("because no permissions boundary allows the {PASS_ROLE_ACTION} action")
        }
    };
    Some(not_authorized(&caller.arn, role_arn, &reason))
}

/// The condition context this pass carries: the SERVICE the role is being passed to
/// ([`PASSED_TO_SERVICE_CONDITION_KEY`]), which is the one value a grant of this
/// request can be scoped by and the only key the call itself fixes. Nothing else is
/// filled in: a clause over a key this request does not carry is left undecided by the
/// shared evaluator rather than guessed at, which is what keeps the decision
/// fail-closed.
fn pass_context() -> [(String, String); 1] {
    [(
        PASSED_TO_SERVICE_CONDITION_KEY.to_string(),
        crate::state::task_authorization::SERVICE_PRINCIPAL.to_string(),
    )]
}

/// The sentence IAM states an authorization failure with: who was refused, which
/// action, on which resource, and which of IAM's two rules refused it. Every value in
/// it comes from the request and from the stored policies — the principal from the
/// credential the request presented, the resource from the role ARN it carried.
fn not_authorized(principal: &str, role_arn: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {PASS_ROLE_ACTION} on resource: \
         {role_arn} {reason}"
    )
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal
    /// names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of
    /// the IAM role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which
    /// holds no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does
    /// not hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once
    /// no readable document DENIES the action: the account's own credential, which
    /// holds no IAM policies, or a principal carrying a grant this emulator cannot
    /// read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM
    /// service's own accessors: the documents in force on it, whether it carries an
    /// attachment whose document this emulator cannot read, and the permissions
    /// boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of
            // AWS's own catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on
/// it.
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
        // The session credential has expired, so it speaks for nobody: no documents,
        // no principal spelling, and every action it asks for falls to the default
        // deny. The key id it presented is NOT quoted back, and in the assembled
        // emulator the layer that authenticates a request refuses an expired
        // credential before any handler runs.
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
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it
    // never minted is the account's ambient identity — but IAM's store maps an access
    // key id to the user that owns it (`access_keys`, the map IAM's own `GetUser`
    // resolves a caller through), so a request signed with one speaks for THAT user
    // and is judged on the user's policies. A key the IAM store does not hold either
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
        // An assumed-role session's permissions are the ROLE's, and the role is
        // addressed by the ARN the account holds it under — which carries its PATH,
        // where the session ARN does not.
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

    /// RULE: the refusal names the principal, the `iam:PassRole` action, the ROLE the
    /// request named and which of IAM's rules refused it — every part taken from the
    /// request and the stored policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_role() {
        let principal = "arn:aws:iam::000000000000:user/least-privilege";
        let role = "arn:aws:iam::000000000000:role/StepFunctionsRole";
        for (case, reason) in [
            (
                "nothing allows it",
                format!("because no identity-based policy allows the {PASS_ROLE_ACTION} action"),
            ),
            (
                "an explicit deny",
                "with an explicit deny in an identity-based policy".to_string(),
            ),
        ] {
            let stated = not_authorized(principal, role, &reason);
            let expected = format!(
                "User: {principal} is not authorized to perform: {PASS_ROLE_ACTION} on resource: \
                 {role} {reason}"
            );
            assert_eq!(
                stated, expected,
                "case {case:?}: expected IAM's wording {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: the decision is the shared evaluation of the caller's documents, so a
    /// policy scoped to the roles it may pass grants exactly those and nothing else.
    #[test]
    fn a_pass_role_grant_covers_the_roles_it_names() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"iam:PassRole",
            "Resource":"arn:aws:iam::000000000000:role/sfn/*"}]}"#;
        let context = pass_context();
        // (case, the role the request named, the decision the documents reach)
        for (case, role, expected) in [
            (
                "a role the grant covers",
                "arn:aws:iam::000000000000:role/sfn/Runner",
                Decision::Allowed,
            ),
            (
                "a role outside the grant",
                "arn:aws:iam::000000000000:role/Other",
                Decision::NotStated,
            ),
            (
                "a role of another account",
                "arn:aws:iam::210987654321:role/sfn/Runner",
                Decision::NotStated,
            ),
        ] {
            let reached = iam::identity_decision(
                &[document.to_string()],
                &iam::Request::new(PASS_ROLE_ACTION, role, &[]).with_context(&context),
            );
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?} for {role}, got {reached:?}"
            );
        }
    }

    /// RULE: the pass carries the SERVICE it hands the role to, so a grant scoped with
    /// `iam:PassedToService` — the scoping AWS guidance tells a caller to write — is
    /// decided on its own terms: it covers a pass to this service and no other.
    #[test]
    fn a_grant_scoped_to_a_service_covers_a_pass_to_that_service() {
        let role = "arn:aws:iam::000000000000:role/sfn/Runner";
        let scoped_to = |service: &str| {
            format!(
                r#"{{"Statement":[{{"Effect":"Allow","Action":"iam:PassRole","Resource":"*",
                "Condition":{{"StringEquals":{{"{PASSED_TO_SERVICE_CONDITION_KEY}":"{service}"}}}}}}]}}"#
            )
        };
        for (case, document, expected) in [
            (
                "a grant scoped to this service",
                scoped_to(crate::state::task_authorization::SERVICE_PRINCIPAL),
                Decision::Allowed,
            ),
            (
                "a grant scoped to another service",
                scoped_to("ec2.amazonaws.com"),
                Decision::NotStated,
            ),
        ] {
            let context = pass_context();
            let reached = iam::identity_decision(
                &[document],
                &iam::Request::new(PASS_ROLE_ACTION, role, &[]).with_context(&context),
            );
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }
}
