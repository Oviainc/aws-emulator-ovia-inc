// SPDX-License-Identifier: CC0-1.0

//! WHO a signed request speaks for, and what the account's IAM store holds about it.
//!
//! The lookups every authorization question of this service is decided from, and the
//! sentence IAM states a refusal with — held here once, so the questions
//! [`super::authorization`] answers (may this caller hand the service a role, and may it
//! perform the action it asked for) read one principal the same way:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for ([`sts_store::resolve_caller`], the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    owns the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a document `iam:PutRolePolicy` wrote
//!    takes effect on the next request and deleting it lifts the refusal again.
//!
//! Only the store LOOKUPS live here; the decision does not — it is
//! [`::service_common::iam_authorization`], asked by [`super::authorization`] — and the
//! lookups stay in this service crate because hoisting them into `service-common` would
//! make every service depending on that crate link the IAM and STS service crates.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for falls to IAM's default deny. In the
//! assembled emulator the layer that authenticates a request refuses such a credential
//! before any handler runs.

use ::std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

/// What a refusal names as the principal of a request whose session credential has
/// already expired: that credential speaks for nobody, and the key it presented is not
/// quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The sentence IAM states an authorization failure with: who was refused, which action,
/// on which resource, and which of IAM's two rules refused it.
///
/// Every value in it comes from the request and from the stored policies — the principal
/// from the credential the request presented, the action from the operation it named, the
/// resource from the ARN it carried — so nothing has to be looked up to state it, which is
/// why it can be stated before the resource is resolved.
pub(crate) fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// The principal a request speaks for, and everything a decision about it is made from.
#[derive(Debug, Clone)]
pub(crate) struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub(crate) arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the
    /// IAM role behind an assumed-role session.
    pub(crate) principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub(crate) documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    pub(crate) boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see [`super::authorization`]).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see [`super::authorization`]).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    pub(crate) fn identity_is_unbounded(&self) -> bool {
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

/// The principal this request speaks for, with the identity-based policies in force on it.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called.
pub(crate) fn caller<Eng: Send + Sync + 'static>(
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
        // The session credential has expired, so it speaks for nobody (see the module
        // docs). The key id it presented is NOT quoted back.
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
