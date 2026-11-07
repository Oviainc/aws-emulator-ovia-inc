// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step every operation runs BEFORE it reads or writes anything, and
//! before it resolves the cluster or the secret the request names.
//!
//! `AccessDeniedException` (HTTP 403, "You don't have sufficient access to perform this
//! action") is a SERVICE-COMMON error of this service's model, so it is the answer every
//! operation here gives a caller whose policies do not grant the action. AWS decides
//! authorization at the IAM layer, ahead of the service looking at the request's own
//! resources, so a caller with no grant is told it is not authorized rather than told
//! anything about the cluster, the secret or the statement — and nothing runs.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    holds the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives, so a document `iam:PutUserPolicy` wrote
//!    takes effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request
//!    falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the readings of a principal
//! ARN do. That is the split every other gate in this tree makes, and the lookups stay in
//! the service crate for the reason those keep theirs there: hoisting them into
//! `service-common` would make every service depending on that crate link the IAM and STS
//! service crates.
//!
//! The RDS Data API publishes no resource-based policy of its own — the model declares no
//! policy operation and no policy member — so the decision here is the identity-based one
//! alone, taken against the CLUSTER the request addressed.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither
//!   STS nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no identity can
//!   exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `rds-data:<OperationName>` ("Actions
/// defined by RDS Data API", IAM Service Authorization Reference), which is this
/// service's own `arnNamespace` (`smithy_query(query="service_info")`).
pub const ACTION_PREFIX: &str = "rds-data";

/// What a refusal names as the principal of a request whose session credential has
/// already expired: that credential speaks for nobody, and the key it presented is not
/// quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// Decide `operation` on the cluster `resource` for the identity this request signed as.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); otherwise the sentence AWS refuses an unauthorized request with,
/// which each operation returns as its own `AccessDeniedException`.
///
/// `resource` is the ARN the request CARRIED rather than one resolved from a stored
/// record, so the decision does not depend on whether that cluster exists — which is also
/// what keeps a refused call from telling an unauthorized caller whether it does.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), String> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, so the decision below rests on the action and the resource.
    let request = iam::Request::new(&action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // Taken FIRST, so an explicit `Deny` in a document that CAN be read refuses the
    // request whatever else the principal carries — including a grant this emulator
    // cannot read, which only lifts the DEFAULT deny (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            &action,
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
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => {
                format!("because no permissions boundary allows the {action} action")
            }
        };
        return Err(not_authorized(&caller.arn, &action, resource, &reason));
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action,
/// on which resource, and which of IAM's rules refused it. Every value in it comes from
/// the request and from the stored policies — the principal from the credential the
/// request presented, the action from the operation it named, the resource from the ARN it
/// addressed.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}")
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the ARN STS reports, and the ARN of the IAM role
    /// behind an assumed-role session.
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
        // key id it presented is NOT quoted back.
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
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
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

    /// RULE: the action this gate asks about is `rds-data:<OperationName>`, so a policy
    /// scoped to this service's actions covers every operation of it and nothing outside
    /// it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"rds-data:*","Resource":"*"}]}"#;
        for operation in [
            "ExecuteStatement",
            "BatchExecuteStatement",
            "BeginTransaction",
            "CommitTransaction",
            "RollbackTransaction",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = iam::identity_decision(
                &[document.to_string()],
                &iam::Request::new(&action, "*", &[]),
            );
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected rds-data:* to allow {action}, got {reached:?}"
            );
        }
        let outside = iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new("rds:DescribeDBClusters", "*", &[]),
        );
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected rds-data:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: the refusal names the principal, the action and the cluster the request
    /// addressed, and closes with which of IAM's rules refused it.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_cluster() {
        let principal = "arn:aws:iam::000000000000:user/no-policies";
        let cluster = "arn:aws:rds:us-east-1:000000000000:cluster:demo";
        let action = format!("{ACTION_PREFIX}:ExecuteStatement");
        let reason = format!("because no identity-based policy allows the {action} action");
        let refusal = not_authorized(principal, &action, cluster, &reason);
        assert_eq!(
            refusal,
            format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {cluster} {reason}"
            ),
            "case a default deny: expected IAM's wording, got {refusal:?}"
        );
    }
}
