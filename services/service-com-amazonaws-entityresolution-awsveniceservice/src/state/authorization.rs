// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the principal a request speaks for do what the request asks.
//!
//! Every operation of this service is a signed call (`sigv4`,
//! `smithy_query(query="service_info")`, with no `@optionalAuth` operation anywhere in the
//! model), and AWS decides such a call against the CALLER's identity-based policies before
//! the operation reads or writes anything — so a caller holding no grant is told it is not
//! authorized rather than told whether the resource it named exists, and a refused write
//! changes nothing. The operations whose model declares `AccessDeniedException` (HTTP 403,
//! "You do not have sufficient access to perform this action") are the ones that can carry
//! that refusal, and each of them asks [`refusal`] first.
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
//!    collection rule itself lives: the principal's inline policies, the default version of
//!    each managed policy attached to it and, for a user, the policies in force on the
//!    groups it belongs to — so this service sees exactly what IAM's own authorizer sees;
//! 3. **the decision** — [`::service_common::iam_authorization`], the shared evaluation of
//!    those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does. That is
//! the same split Amazon Cognito Identity's, Secrets Manager's and Amazon ECR Public's own
//! gates make, and the lookups stay in this service crate for the reason those keep theirs
//! there: hoisting them into `service-common` would make every service depending on that
//! crate link the IAM and STS service crates.
//!
//! ## Why the RESOURCE-based policies this service stores are not a second decision
//!
//! A statement this API attaches to a resource names an account or a service as its
//! principal (`StatementPrincipal`'s pattern `^((\d{12})|([a-z0-9\.]+))$` — twelve digits,
//! or a dotted service name — `smithy_query(query="shape",shape="StatementPrincipalList")`),
//! never an individual user or role; and a resource statement that ALLOWS an account
//! delegates the decision to that account's own identity-based policies
//! (`service_common::iam_authorization::account_principals`). So within one account such a
//! policy adds no grant the identity-based decision below would miss, which is why this
//! gate reads the caller's policies and the resource's incarnation is left to the policy
//! operations that own it.
//!
//! ## The two callers the decision cannot refuse
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to [`sts_store::ambient_identity`], the account's
//!   own identity, which is not a principal the IAM store holds policies for — the same
//!   position AWS account root credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   is owned by AWS, not by the account, so the IAM store holds no document for it
//!   ([`iam_store::AWS_MANAGED_POLICY_ACCOUNT`]). A principal with such a policy attached is
//!   allowed: this service cannot see what that document grants, and refusing on evidence it
//!   does not have would deny access AWS permits. An attachment naming a CUSTOMER-managed
//!   policy the account does not hold grants nothing, because the account is the authority on
//!   its own policies.
//!
//! An expired session credential speaks for nobody: it carries no document and no principal
//! spelling, so every action it asks for is refused.
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which materialises the
//! session expiries the clock has already made due before it answers: that write is owed at
//! the same instant whether or not anybody called, which is why a REPORTING operation of
//! this service may make this decision.
//!
//! ## What one decision costs, and what it reads
//!
//! The work is bounded by what IAM holds for ONE principal and does not grow with the
//! request: the inline policies embedded in that principal, the managed policies attached to
//! it, and — for a user — the groups it belongs to, each document collected once by IAM's own
//! accessor and read once by the shared evaluator, whose matcher is single-pass and never
//! recurses. The decision is taken with no store guard of this service held, and it writes
//! nothing to either store it reads.
//!
//! Every value it reads comes from the REQUEST (the access key id in the credential scope,
//! and the Region the request was signed for) and from those two in-process stores: nothing
//! here reads the host's environment or filesystem, opens a socket, or spawns anything. Every
//! lookup answers an `Option` and every ARN is read through the tree's shared readings, so a
//! value no store holds — or one that is no ARN at all — leaves the caller unauthorized
//! instead of panicking, and no refusal quotes a value back except the principal's own ARN,
//! the action and the resource the request addressed.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `entityresolution:<OperationName>`
/// ("Actions defined by AWS Entity Resolution", IAM Service Authorization Reference), which
/// is also this service's `arnNamespace` (`smithy_query(query="service_info")`:
/// `arnNamespace: entityresolution`) and so the service segment of every ARN it mints.
pub const ACTION_PREFIX: &str = "entityresolution";

/// Every resource of a request that names NO single one, spelled with IAM's own wildcard: a
/// listing addresses the account's resources of that type rather than one record, so a
/// statement scoped to one resource cannot allow it while one covering the account's
/// resources can.
pub const ANY_RESOURCE: &str = "*";

/// The IAM action `operation` is performed under: `entityresolution:<OperationName>`.
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The ARN of the resource `resource_type`/`name` as this service mints it for a request
/// signed for `region` — the resource an authorization decision is made about.
///
/// The resource types are the service's own
/// ([`super::SCHEMA_MAPPING_RESOURCE_TYPE`] and its siblings) and the ARN is built by the
/// same builder every create mints with, so the value a statement has to match is the ARN
/// the resource itself carries. `name` is [`ANY_RESOURCE`] for a request that names no
/// single resource.
pub fn resource_arn(region: &str, resource_type: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ACTION_PREFIX, region, &format!("{resource_type}/{name}"))
}

/// The ARN of the ID namespace a request's `EntityNameOrIdNamespaceArn` label addresses.
///
/// That label admits either the namespace's name or the ARN the create minted
/// (`smithy_query(query="shape",shape="EntityNameOrIdNamespaceArn")`), so the value is
/// reduced to the name the store keys the namespace under ([`super::id_namespace_key`]) and
/// the decision is made about the ARN that name carries. A value that names nothing here is
/// decided as the resource it spells, so the refusal still comes before the operation
/// resolves anything.
pub fn id_namespace_resource(region: &str, name_or_arn: &str) -> String {
    let named = super::id_namespace_key(name_or_arn, region).unwrap_or(name_or_arn);
    resource_arn(region, super::ID_NAMESPACE_RESOURCE_TYPE, named)
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    pub boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    pub account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    pub unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's OWN
    /// accessors: the documents in force on it — its inline policies, the default version of
    /// each managed policy attached to it and, for a user, the policies in force on the
    /// groups it belongs to — whether it carries an attachment whose document this emulator
    /// cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's own
            // catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The refusal AWS words an authorization failure with: who was refused, which action, on
/// which resource, and which rule withheld the grant. Every value in it comes from the
/// request and the stored policies.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// `Some(message)` when IAM does not allow `entityresolution:<operation>` on `resource` for
/// the calling principal — the message the refusal carries — and `None` when it does.
///
/// `resource` is the ARN the request addresses ([`resource_arn`]), computed before anything
/// is looked up so the decision does not depend on whether that resource exists: a caller
/// holding no grant must not learn from the refusal what the account holds.
///
/// IAM's order: an explicit `Deny` in a document that CAN be read refuses the request
/// whatever else the principal carries; otherwise a grant is needed, which a caller this gate
/// does not bound has by definition (see the module docs); and a grant is capped by the
/// permissions boundary, since "a permissions boundary sets the maximum permissions that an
/// identity-based policy can grant" (IAM, permissions boundaries).
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
    // An identity-based document states no `Principal` element, so the decision rests on the
    // action and the resource; the principal spellings a resource-based statement would be
    // matched against are not part of it (see the module docs on why this service's own
    // resource policies add no grant here).
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
        // An explicit deny is a fact about the boundary document, so the refusal names it; a
        // boundary that merely states nothing withheld the grant the policies gave.
        Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
        Decision::NotStated => {
            format!("because no permissions boundary allows the {action} action")
        }
    };
    Some(not_authorized(&caller.arn, &action, resource, &reason))
}

/// The principal this request speaks for, with the identity-based policies in force on it.
pub fn caller<Eng: Send + Sync + 'static>(
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
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody.
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            }
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
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
    // the user that owns it (`access_keys`, the map `iam:GetUser` resolves a caller
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
        }
    }
    // Which IAM identity the principal's policies are held under is read through the tree's
    // shared readings of a principal ARN.
    if let Some(role_name) =
        ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
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

    /// A schema mapping's ARN in the region the rows below are decided in.
    fn schema_mapping(name: &str) -> String {
        resource_arn("us-east-1", super::super::SCHEMA_MAPPING_RESOURCE_TYPE, name)
    }

    #[test]
    fn a_principal_with_no_document_is_allowed_nothing() {
        for operation in ["GetSchemaMapping", "DeleteMatchingWorkflow", "ListIdNamespaces"] {
            let action = action_for(operation);
            let resource = schema_mapping("probe");
            let said = iam::identity_decision(&[], &iam::Request::new(&action, &resource, &[]));
            assert_eq!(
                said,
                Decision::NotStated,
                "case {operation:?}: expected no grant from an empty document set, got {said:?}"
            );
            assert!(
                !iam::is_allowed(said, Decision::NotStated),
                "case {operation:?}: an unstated decision must not allow {action}"
            );
        }
    }

    #[test]
    fn a_grant_is_read_for_the_action_and_the_resource_it_names() {
        let granted = schema_mapping("probe");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"entityresolution:GetSchemaMapping","Resource":"{granted}"}}]}}"#
        );
        // (case, operation, resource, allowed)
        let rows: &[(&str, &str, String, bool)] = &[
            (
                "the granted action on the granted schema mapping",
                "GetSchemaMapping",
                granted.clone(),
                true,
            ),
            (
                "another action on the same schema mapping",
                "DeleteSchemaMapping",
                granted.clone(),
                false,
            ),
            (
                "the granted action on another schema mapping",
                "GetSchemaMapping",
                schema_mapping("other"),
                false,
            ),
            (
                "the granted action with no single resource named",
                "GetSchemaMapping",
                resource_arn(
                    "us-east-1",
                    super::super::SCHEMA_MAPPING_RESOURCE_TYPE,
                    ANY_RESOURCE,
                ),
                false,
            ),
            (
                "the granted action on a resource of another type",
                "GetSchemaMapping",
                resource_arn(
                    "us-east-1",
                    super::super::MATCHING_WORKFLOW_RESOURCE_TYPE,
                    "probe",
                ),
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

    #[test]
    fn an_explicit_deny_wins_over_a_wildcard_grant() {
        let document = r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"entityresolution:*","Resource":"*"},
            {"Effect":"Deny","Action":"entityresolution:DeleteSchemaMapping","Resource":"*"}]}"#;
        let resource = schema_mapping("probe");
        let denied = decision(document, &action_for("DeleteSchemaMapping"), &resource);
        assert_eq!(
            denied,
            Decision::Denied,
            "case \"the denied action\": expected Denied, got {denied:?}"
        );
        let allowed = decision(document, &action_for("GetSchemaMapping"), &resource);
        assert_eq!(
            allowed,
            Decision::Allowed,
            "case \"an action the deny does not name\": expected Allowed, got {allowed:?}"
        );
    }

    #[test]
    fn the_refusal_names_the_principal_the_action_the_resource_and_the_reason() {
        let resource = schema_mapping("probe");
        let action = action_for("GetSchemaMapping");
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
            let message = not_authorized(
                "arn:aws:iam::000000000000:user/noperm-probe",
                &action,
                &resource,
                &reason,
            );
            let expected = format!(
                "User: arn:aws:iam::000000000000:user/noperm-probe is not authorized to perform: \
                 {action} on resource: {resource} {reason}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// The principal a request speaks for is read through the tree's SHARED readings of a
    /// principal ARN, so a session ARN names the role whose policies this gate reads and a
    /// user ARN names the user whose policies it reads, while every other form names neither.
    #[test]
    fn a_principal_arn_names_the_entity_its_policies_are_read_from() {
        // (case, ARN, the role it names, the user it names)
        let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/er-admin/session-1",
                Some("er-admin"),
                None,
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/noperm-probe",
                None,
                Some("noperm-probe"),
            ),
            (
                "a path'd IAM user",
                "arn:aws:iam::000000000000:user/team/noperm-probe",
                None,
                Some("noperm-probe"),
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
            let named_role = ::service_common::iam_role::assumed_role_session_role_name(arn);
            let named_user = ::service_common::iam_role::user_name(arn);
            assert_eq!(
                named_role, *role,
                "case {case:?}: expected role {role:?}, got {named_role:?}"
            );
            assert_eq!(
                named_user, *user,
                "case {case:?}: expected user {user:?}, got {named_user:?}"
            );
        }
    }
}
