// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER's own identity-based IAM policies let a request to this service
//! perform the action it asks for — the decision an operation takes before it resolves
//! anything the request names.
//!
//! AWS decides a signed call against the calling principal's identity-based policies
//! before the service evaluates the resource, so a caller holding no grant is told it is
//! not authorized rather than told whether the resource it named exists, and a refused
//! write changes nothing. The operations whose model declares `AccessDeniedException`
//! ("The requester doesn't have permissions to perform the requested operation") are the
//! ones that can carry that refusal, and each of them asks [`refusal`] first.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_caller`]) for a
//!    session credential, and from IAM's own `access_keys` map (the map `iam:GetUser`
//!    resolves a caller through) for a long-term key, so the principal a refusal blames is
//!    the one the caller reads back for the credentials it signed with;
//! 2. **what it may do** — the identity-based policy documents in force on that principal
//!    and the permissions boundary that caps them, read through the IAM service's OWN
//!    accessors (`principal_policies`, `permissions_boundary_document`), where the
//!    collection rule itself lives, so this service sees exactly what IAM's own authorizer
//!    sees;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does. The
//! lookups stay in this service crate because the IAM service crate depends on
//! `service-common`, so the shared crate cannot read IAM's store without a dependency
//! cycle. WHICH IAM entity a principal ARN names carries no such dependency, so it is read
//! through the shared [`::service_common::iam_role`] rather than parsed again here.
//!
//! ## The two callers the decision cannot refuse
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to [`sts_store::ambient_identity`], the account's
//!   own identity, which is not a principal the IAM store holds policies for — the same
//!   position AWS account root credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   is owned by AWS, not by the account, so the IAM store holds no document for it. A
//!   principal with such a policy attached is allowed: this service cannot see what that
//!   document grants, and refusing on evidence it does not have would deny access AWS
//!   permits.
//!
//! An expired session credential speaks for nobody: it carries no document, so every action it
//! asks for is refused, and the refusal names the credential that was presented rather than a
//! blank principal.
//!
//! ## What one decision costs, and what it reads
//!
//! The work is bounded by what IAM holds for ONE principal and does not grow with the
//! request: that principal's inline policies, the managed policies attached to it and —
//! for a user — the groups it belongs to, each collected once by IAM's own accessor and
//! read once by the shared evaluator. The decision is taken with no guard of this service's
//! own store held, and it writes nothing to either store it reads. Every value it reads
//! comes from the request (the access key id in the credential scope) and from those two
//! in-process stores; every lookup answers an `Option`, so a value no store holds leaves the
//! caller unauthorized instead of panicking, and no refusal quotes a value back except the
//! principal's own ARN, the action and the resource the request addressed.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// Every resource of a request that names NO single one, spelled with IAM's own wildcard:
/// such a call addresses the account's resources rather than one record, so a statement
/// scoped to one resource cannot allow it while one covering the account's resources can.
pub const ANY_RESOURCE: &str = "*";

/// The IAM action `operation` is performed under: `ssm:<OperationName>` ("Actions defined
/// by AWS Systems Manager", IAM Service Authorization Reference), whose prefix is this
/// service's own ARN namespace ([`super::ARN_SERVICE`]).
pub fn action_for(operation: &str) -> String {
    format!("{}:{operation}", super::ARN_SERVICE)
}

/// `Some(message)` when IAM does not allow `ssm:<operation>` on `resource` for the calling
/// principal — the sentence the refusal carries — and `None` when it does.
///
/// `resource` is the ARN the request addresses, computed before anything is looked up so
/// the decision does not depend on whether that resource exists: a caller holding no grant
/// must not learn from the refusal what the account holds. [`ANY_RESOURCE`] stands for a
/// call that names no single resource.
///
/// IAM's order: an explicit `Deny` in a document that CAN be read refuses the request
/// whatever else the principal carries; otherwise a grant is needed, which a caller this
/// gate does not bound has by definition (see the module docs); and a grant is capped by
/// the permissions boundary, since "a permissions boundary sets the maximum permissions
/// that an identity-based policy can grant" (IAM, permissions boundaries).
///
/// Asked with NO store guard held, and before the operation reads or writes anything.
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Option<String> {
    let action = action_for(operation);
    let caller = caller(engine, ctx);
    // An identity-based document states no `Principal` element, so the decision rests on
    // the action and the resource the request addressed.
    let request = iam::Request::new(&action, resource, &[]);
    let said = iam::identity_decision(&caller.documents, &request);
    if said == Decision::Denied {
        return Some(not_authorized(
            &caller.arn,
            &action,
            resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    if !(iam::is_allowed(said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Some(not_authorized(
            &caller.arn,
            &action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        ));
    }
    let boundary = caller.boundary.as_ref()?;
    let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
        Decision::Allowed => return None,
        // An explicit deny is a fact about the boundary document, so the refusal names it;
        // a boundary that merely states nothing withheld the grant the policies gave.
        Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
        Decision::NotStated => {
            format!("because no permissions boundary allows the {action} action")
        }
    };
    Some(not_authorized(&caller.arn, &action, resource, &reason))
}

/// The sentence IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which rule withheld the grant. Every value in it comes from the
/// request and the stored policies.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// The ARN of the principal this request speaks for, as `sts:GetCallerIdentity` reports it —
/// what a write of this service records as the caller that made it, and the same reading a
/// refusal blames.
///
/// A credential neither store holds a record for speaks for the account itself, which is the
/// identity this emulator publishes its own credential as
/// ([`::service_common::caller_principal::ambient_user_arn`]), and so is the fallback for a
/// credential nothing resolves a spelling for at all. A credential that speaks for nobody —
/// an expired session — carries a spelling of its own but never reaches a write: [`refusal`]
/// has already refused it, because it carries no document that allows anything.
pub fn caller_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let arn = caller(engine, ctx).arn;
    if arn.is_empty() {
        ::service_common::caller_principal::ambient_user_arn()
    } else {
        arn
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
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

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's OWN
    /// accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
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
///
/// Resolving the caller goes through STS's own `resolve_caller`, which materialises the
/// session expiries the clock has already made due before it answers: that write is owed at
/// the same instant whether or not anybody called, which is why a REPORTING operation of
/// this service may take this decision too.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::emulator_server::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which holds no identity policy.
        None => Ok(sts_store::ambient_identity()),
    };
    // An expired session credential speaks for nobody: it carries no document, so every
    // action it asks for is refused — and the refusal still names what was presented rather
    // than a blank principal.
    let identity = match identity {
        Ok(identity) => identity,
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                ..Caller::default()
            }
        }
    };
    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        arn: identity.arn,
        ..Caller::default()
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (the map `iam:GetUser` resolves a caller through), so a request
    // signed with one speaks for THAT user and is judged on the user's policies. A key the
    // IAM store does not hold either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
        }
    }
    // Which IAM identity the principal's policies are held under is read through the tree's
    // shared readings of a principal ARN.
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
            caller.read_policies_of(&store, &role_arn);
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

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// The ARN of the access request `id` names in `region`, the resource the just-in-time
    /// node access decisions below are made about.
    fn access_request(region: &str, id: &str) -> String {
        super::super::just_in_time_node_access::access_request_arn(region, id)
    }

    /// RULE: the action this gate asks about is `ssm:<OperationName>`, so a document scoped
    /// to this service's actions decides every operation of it and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Deny","Action":"ssm:*","Resource":"*"}]}"#;
        for operation in ["GetAccessToken", "StartAccessRequest", "GetParameter"] {
            let action = action_for(operation);
            let reached = decision(document, &action, ANY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Denied,
                "case {operation:?}: expected ssm:* to deny {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "ec2:DescribeInstances", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected ssm:* to state nothing outside this service, \
             got {outside:?}"
        );
    }

    /// RULE: a grant is read for the action AND the resource it names, so a statement that
    /// names neither this action nor this resource allows nothing.
    #[test]
    fn a_grant_is_read_for_the_action_and_the_resource_it_names() {
        let granted = access_request("us-west-2", "oi-0123456789ab");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"ssm:GetAccessToken","Resource":"{granted}"}}]}}"#
        );
        // (case, operation, resource, allowed)
        let rows: &[(&str, &str, String, bool)] = &[
            (
                "the granted action on the granted access request",
                "GetAccessToken",
                granted.clone(),
                true,
            ),
            (
                "another action on the same access request",
                "StartAccessRequest",
                granted.clone(),
                false,
            ),
            (
                "the granted action on another access request",
                "GetAccessToken",
                access_request("us-west-2", "oi-ba9876543210"),
                false,
            ),
            (
                "the granted action on the same id in another Region",
                "GetAccessToken",
                access_request("eu-central-1", "oi-0123456789ab"),
                false,
            ),
            (
                "the granted action naming no single resource",
                "GetAccessToken",
                ANY_RESOURCE.to_string(),
                false,
            ),
        ];
        for (case, operation, resource, expected) in rows {
            let said = decision(&document, &action_for(operation), resource);
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected allowed == {expected}, got {actual} ({said:?})"
            );
        }
    }

    /// RULE: an explicit `Deny` is final, whatever else the principal carries.
    #[test]
    fn an_explicit_deny_wins_over_a_wildcard_grant() {
        let document = r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"ssm:*","Resource":"*"},
            {"Effect":"Deny","Action":"ssm:GetAccessToken","Resource":"*"}]}"#;
        let resource = access_request("us-west-2", "oi-0123456789ab");
        let denied = decision(document, &action_for("GetAccessToken"), &resource);
        assert_eq!(
            denied,
            Decision::Denied,
            "case \"the denied action\": expected Denied, got {denied:?}"
        );
        let allowed = decision(document, &action_for("StartAccessRequest"), &resource);
        assert_eq!(
            allowed,
            Decision::Allowed,
            "case \"an action the deny does not name\": expected Allowed, got {allowed:?}"
        );
    }

    /// RULE: the refusal names the principal the credential speaks for, the action asked
    /// for, the resource the request addressed, and the rule that withheld the grant.
    #[test]
    fn the_refusal_names_the_principal_the_action_the_resource_and_the_reason() {
        let principal = format!(
            "arn:aws:sts::{}:assumed-role/ssm-noperm/session-1",
            ::engine::emulator_account_id()
        );
        let resource = access_request("us-west-2", "oi-0123456789ab");
        let action = action_for("GetAccessToken");
        // (case, the clause the message must carry)
        let rows: Vec<(&str, String)> = vec![
            (
                "no policy allows it",
                format!("because no identity-based policy allows the {action} action"),
            ),
            (
                "a policy denies it",
                "with an explicit deny in an identity-based policy".to_string(),
            ),
            (
                "the permissions boundary withholds it",
                format!("because no permissions boundary allows the {action} action"),
            ),
            (
                "the permissions boundary denies it",
                "with an explicit deny in a permissions boundary".to_string(),
            ),
        ];
        for (case, reason) in rows {
            let message = not_authorized(&principal, &action, &resource, &reason);
            let expected = format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {resource} {reason}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
