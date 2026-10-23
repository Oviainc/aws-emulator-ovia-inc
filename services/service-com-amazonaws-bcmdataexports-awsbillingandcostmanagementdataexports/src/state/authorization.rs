// SPDX-License-Identifier: CC0-1.0

//! Who a request to this service speaks for, and whether IAM lets it do what it asks.
//!
//! The operations whose model declares `AccessDeniedException` (HTTP 403, "You don't have
//! sufficient access to perform this action" — smithy_query shape `AccessDeniedException`)
//! authorize the caller before they read or write anything: `CreateExport`, `TagResource`,
//! `UntagResource` and `ListTagsForResource` (smithy_query operation `…`, errors). The
//! decision is made from the documents in force at the instant of the call, so a policy
//! that was just put grants immediately and one that was just deleted stops granting
//! immediately. An operation whose model declares no 403 is NOT judged here: answering a
//! caller with an error its operation does not model would be a different divergence.
//!
//! The three pieces, each held in exactly one place — the same shape CloudWatch RUM's and
//! Transfer's own `state/authorization.rs` take, because this is one rule and not three:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for ([`sts_store::resolve_caller`], the same place
//!    `sts:GetCallerIdentity` answers from), else the IAM user that owns the long-term key
//!    (IAM's own `access_keys` map, which its `GetUser` resolves a caller through), else
//!    the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal (a role's inline policies and the default versions of the managed
//!    policies attached to it, a user's inline policies), read out of the IAM service's
//!    OWN store so this service sees exactly what IAM's handlers wrote;
//! 3. **the decision** — [`::service_common::iam_authorization`], the shared evaluation of
//!    those documents against `(action, resource, principal)`, so this service and any
//!    other handed the same documents decide alike.
//!
//! Data exports has no resource-based policy of its own — the model declares no
//! `PutResourcePolicy` and the service publishes no policy member — so the decision here
//! is the identity-based one alone.
//!
//! ## The two callers that are not IAM principals
//!
//! * **The account's own published credential.** A request whose access key id neither
//!   STS nor IAM holds a record for resolves to [`sts_store::ambient_identity`], the
//!   account's own identity, which is not a principal the IAM store holds policies for —
//!   the same position AWS account root credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy is owned by AWS, not by the account, so the IAM store holds no document for it
//!   ([`iam_store::AWS_MANAGED_POLICY_ACCOUNT`]). A principal with such a policy attached
//!   is allowed: this service cannot see what that document grants, and refusing on
//!   evidence it does not have would deny access AWS permits. An attachment naming a
//!   CUSTOMER-managed policy the account does not hold grants nothing, because the account
//!   is the authority on its own policies.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// `bcm-data-exports:CreateExport`.
pub const CREATE_EXPORT: &str = "bcm-data-exports:CreateExport";

/// `bcm-data-exports:TagResource`.
pub const TAG_RESOURCE: &str = "bcm-data-exports:TagResource";

/// `bcm-data-exports:UntagResource`.
pub const UNTAG_RESOURCE: &str = "bcm-data-exports:UntagResource";

/// `bcm-data-exports:ListTagsForResource`.
pub const LIST_TAGS_FOR_RESOURCE: &str = "bcm-data-exports:ListTagsForResource";

/// The principal a request speaks for, with everything the decision is made from.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the
    /// IAM role behind an assumed-role session.
    pub principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    pub account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    pub unreadable_attachment: bool,
}

impl Caller {
    /// Whether the identity-based decision cannot refuse this caller: the account's own
    /// credential, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }
}

/// Whether IAM lets this request perform `action` on `resource`, judged on the caller's
/// identity-based policies.
///
/// `Ok(())` when the request is allowed; `Err` is the modelled `AccessDeniedException`
/// each operation returns as its own, built from the three things the decision was made
/// about — the principal, the action and the resource — in AWS's own wording.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let caller = caller(engine, ctx);
    let request = iam::Request::new(action, resource, &caller.principals);
    if caller.identity_is_unbounded() {
        return Ok(());
    }
    let identity_said = iam::identity_decision(&caller.documents, &request);
    if iam::is_allowed(identity_said, Decision::NotStated) {
        return Ok(());
    }
    let reason = if identity_said == Decision::Denied {
        format!("with an explicit deny in a policy for the {action} action")
    } else {
        format!("because no policy allows the {action} action")
    };
    Err(AccessDeniedException {
        message: format!(
            "User: {} is not authorized to perform: {action} on resource: {resource} {reason}",
            caller.arn
        ),
    })
}

/// The principal this request speaks for, and the identity-based policies in force on it.
pub fn caller<Eng: Send + Sync + 'static>(
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
        // The session credential has expired, so it speaks for nobody.
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                principals: Vec::new(),
                documents: Vec::new(),
                account_credential: false,
                unreadable_attachment: false,
            }
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
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
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        let role_key = iam_store::name_key(role_name);
        if let Some(held) = store.roles.get(&role_key) {
            caller.principals.push(held.arn.clone());
        }
        caller
            .documents
            .extend(inline_documents(&store.role_inline_policies, &role_key));
        for attached in store
            .role_attached_policies
            .get(&role_key)
            .into_iter()
            .flatten()
        {
            match store
                .policy_by_arn(attached)
                .and_then(default_version_document)
            {
                Some(document) => caller.documents.push(document),
                // Only an AWS-MANAGED policy is a grant this emulator cannot read (see
                // the module docs).
                None if is_aws_managed_policy(attached) => {
                    caller.unreadable_attachment = true;
                }
                None => {}
            }
        }
    } else if let Some(user_name) = iam_user_name(&caller.arn) {
        // A user's INLINE policies are what the IAM store holds for it; `AttachUserPolicy`
        // is not implemented there, so there is no attached-policy map to read for a user
        // (unlike a role's `role_attached_policies` above).
        caller.documents.extend(inline_documents(
            &store.user_inline_policies,
            &iam_store::name_key(user_name),
        ));
    }
    caller
}

/// Whether `arn` names an AWS-MANAGED policy: one whose account segment is IAM's `aws`,
/// so it belongs to AWS rather than to this account and the IAM store holds no document
/// for it. Read through IAM's own ARN parser, so this service and IAM agree on what an
/// AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam_store::parse_policy_arn(arn)
        .is_some_and(|policy| policy.account == iam_store::AWS_MANAGED_POLICY_ACCOUNT)
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`
/// (`sts_store::sts_arn`), so the role is the first segment after `assumed-role/`.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "sts")?;
    let tail = resource.strip_prefix("assumed-role/")?;
    let name = tail.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// The user name an IAM user ARN names: the LAST segment of `user/<path…>/<name>`, since
/// the path is optional and only the last segment is the name.
fn iam_user_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "iam")?;
    let name = resource.strip_prefix("user/")?.split('/').next_back()?;
    (!name.is_empty()).then_some(name)
}

/// The resource part of an ARN whose service segment is `service`.
fn arn_resource<'a>(arn: &'a str, service: &str) -> Option<&'a str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(named), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    (named == service).then_some(resource)
}

/// The inline policy documents one entity holds, from the IAM store's
/// `<entity> -> <policy name> -> policy` map.
fn inline_documents(
    policies: &BTreeMap<String, BTreeMap<String, iam_store::InlinePolicy>>,
    entity_key: &str,
) -> Vec<String> {
    policies
        .get(entity_key)
        .map(|held| {
            held.values()
                .map(|policy| policy.policy_document.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The document of a managed policy's DEFAULT version — the one in force on every entity
/// it is attached to. `None` when the store holds no document for it.
fn default_version_document(managed: &iam_store::ManagedPolicy) -> Option<String> {
    let default = managed.policy.default_version_id.as_deref();
    managed
        .versions
        .values()
        .find(|version| default.is_some() && version.version_id.as_deref() == default)
        .or_else(|| {
            managed
                .versions
                .values()
                .find(|version| version.is_default_version)
        })
        .and_then(|version| version.document.clone())
}
