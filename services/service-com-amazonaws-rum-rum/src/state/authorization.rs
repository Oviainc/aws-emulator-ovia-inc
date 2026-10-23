// SPDX-License-Identifier: CC0-1.0

//! Who a request to this service speaks for, and whether IAM lets it do what it asks.
//!
//! RUM evaluates authorization on EVERY call: the data plane (`PutRumEvents`) and the
//! control plane both model `AccessDeniedException` (HTTP 403), and the decision is made
//! from the documents in force at the instant of the call — the calling principal's
//! identity-based policies and, where the operation addresses one app monitor, that
//! monitor's resource-based policy. Nothing is cached at grant time, so a policy that was
//! just put grants immediately and a policy that was just deleted stops granting
//! immediately.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_caller`]), so the
//!    principal a refusal blames is the one the caller reads back from STS for the
//!    credentials it signed with;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal (a role's inline policies and the default versions of the managed
//!    policies attached to it, a user's inline policies), read out of the IAM service's
//!    OWN store so RUM sees exactly what IAM's handlers wrote;
//! 3. **the decision** — [`::service_common::iam_authorization`], the shared evaluation of
//!    those documents against `(action, resource, principal)`, so RUM and any other
//!    service handed the same documents decide alike.
//!
//! ## The two callers that are not IAM principals
//!
//! * **The account's own published credential.** A request signed with the long-term key
//!   this emulator publishes resolves to [`sts_store::ambient_identity`], the account's
//!   own identity, which is not a principal the IAM store holds policies for — the same
//!   position AWS account root credentials are in. Such a request is allowed, while an
//!   explicit `Deny` in the resource policy still refuses it.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy is owned by AWS, not by the account, so the IAM store holds no document for it
//!   ([`iam_store::AWS_MANAGED_POLICY_ACCOUNT`]) — which is also why `AttachRolePolicy`
//!   accepts one without the account holding it. A principal with such a policy attached
//!   is allowed: RUM cannot see what that document grants, and refusing on evidence it
//!   does not have would deny access AWS permits. An explicit `Deny` in a document it CAN
//!   read still refuses. This applies ONLY to an AWS-managed ARN: an attachment naming a
//!   customer-managed policy the account does not hold grants nothing, because the account
//!   IS the authority on its own policies.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused.
//!
//! Resolving the caller goes through STS's own [`sts_store::resolve_caller`], which
//! materialises the session expiries the clock has already made due before it answers.
//! That write is owed at the same instant whether or not anybody called, which is why a
//! REPORTING operation (`GetAppMonitor`, `ListAppMonitors`) may make this decision: what a
//! later caller sees is unchanged by the read itself.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// `rum:PutRumEvents`, the data plane's action.
pub const PUT_RUM_EVENTS: &str = "rum:PutRumEvents";

/// `rum:CreateAppMonitor`.
pub const CREATE_APP_MONITOR: &str = "rum:CreateAppMonitor";

/// `rum:GetAppMonitor`.
pub const GET_APP_MONITOR: &str = "rum:GetAppMonitor";

/// `rum:UpdateAppMonitor`.
pub const UPDATE_APP_MONITOR: &str = "rum:UpdateAppMonitor";

/// `rum:DeleteAppMonitor`.
pub const DELETE_APP_MONITOR: &str = "rum:DeleteAppMonitor";

/// `rum:ListAppMonitors`.
pub const LIST_APP_MONITORS: &str = "rum:ListAppMonitors";

/// `rum:PutResourcePolicy`.
pub const PUT_RESOURCE_POLICY: &str = "rum:PutResourcePolicy";

/// `rum:GetResourcePolicy`.
pub const GET_RESOURCE_POLICY: &str = "rum:GetResourcePolicy";

/// `rum:DeleteResourcePolicy`.
pub const DELETE_RESOURCE_POLICY: &str = "rum:DeleteResourcePolicy";

/// `rum:PutRumMetricsDestination`.
pub const PUT_RUM_METRICS_DESTINATION: &str = "rum:PutRumMetricsDestination";

/// `rum:ListRumMetricsDestinations`.
pub const LIST_RUM_METRICS_DESTINATIONS: &str = "rum:ListRumMetricsDestinations";

/// `rum:DeleteRumMetricsDestination`.
pub const DELETE_RUM_METRICS_DESTINATION: &str = "rum:DeleteRumMetricsDestination";

/// `logs:PutResourcePolicy`, the CloudWatch Logs action a caller must hold for RUM to
/// turn on log delivery on its behalf: enabling `CwLogEnabled` makes RUM install a
/// resource policy on the vended log group in the caller's account, so the caller has to
/// be allowed to install one.
pub const LOGS_PUT_RESOURCE_POLICY: &str = "logs:PutResourcePolicy";

/// The `rum:alias` condition key, which `PutRumEventsRequest$Alias` documents: "This alias
/// will be compared to the `rum:alias` context key in the resource-based policy."
pub const ALIAS_CONDITION_KEY: &str = "rum:alias";

/// The principal a request speaks for, with everything the decision is made from.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// Every spelling of the principal a resource policy may name it by: the session ARN
    /// STS reports, and the ARN of the IAM role behind an assumed-role session.
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
    /// Whether the identity-based side of the decision cannot refuse this caller: the
    /// account's own credential, or a principal carrying a grant this emulator cannot
    /// read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }
}

/// An authorization refusal: who asked for what, and which side withheld the grant.
#[derive(Debug, Clone)]
pub struct Denied {
    /// The principal the request speaks for.
    pub principal: String,
    /// The IAM action it asked for.
    pub action: String,
    /// The resource it asked to act on.
    pub resource: String,
    /// Set when a statement explicitly denied the action, rather than no statement
    /// allowing it.
    pub explicit: bool,
}

impl Denied {
    /// The refusal in AWS's own wording, built from the three things the decision was made
    /// about: the principal, the action and the resource.
    pub fn message(&self) -> String {
        let reason = if self.explicit {
            format!("with an explicit deny in a policy for the {} action", self.action)
        } else {
            format!("because no policy allows the {} action", self.action)
        };
        format!(
            "User: {} is not authorized to perform: {} on resource: {} {reason}",
            self.principal, self.action, self.resource
        )
    }
}

/// Whether IAM lets this request perform `action` on `resource`.
///
/// `resource_policy` is the resource-based policy attached to the resource, when it has
/// one; `context` the condition keys this call publishes (the `rum:alias` the request
/// named, say). `Ok(())` when the request is allowed; `Err` carries the refusal every
/// operation renders as its own modelled `AccessDeniedException`.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
    resource_policy: Option<&str>,
    context: &[(String, String)],
) -> Result<(), Denied> {
    let caller = caller(engine, ctx);
    let request =
        iam::Request::new(action, resource, &caller.principals).with_context(context);
    let resource_said = resource_policy
        .map(|document| iam::resource_decision(document, &request))
        .unwrap_or(Decision::NotStated);
    let refusal = |explicit: bool| Denied {
        principal: caller.arn.clone(),
        action: action.to_string(),
        resource: resource.to_string(),
        explicit,
    };
    // An explicit deny wins over every grant, including the account's own credential.
    if resource_said == Decision::Denied {
        return Err(refusal(true));
    }
    if caller.identity_is_unbounded() {
        return Ok(());
    }
    let identity_said = iam::identity_decision(&caller.documents, &request);
    if iam::is_allowed(identity_said, resource_said) {
        return Ok(());
    }
    Err(refusal(identity_said == Decision::Denied))
}

/// Whether IAM lets this request perform `action`, judged on the caller's identity-based
/// policies alone.
///
/// The form a CROSS-SERVICE precondition takes: RUM asks whether the caller would be
/// allowed to do something in another service (install the log-group resource policy that
/// backs `CwLogEnabled`), and the resource there has no RUM resource policy to consult.
pub fn authorize_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Result<(), Denied> {
    authorize(engine, ctx, action, resource, None, &[])
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
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        let role_key = iam_store::name_key(role_name);
        if let Some(held) = store.roles.get(&role_key) {
            caller.principals.push(held.arn.clone());
        }
        caller.documents.extend(inline_documents(
            &store.role_inline_policies,
            &role_key,
        ));
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
                // the module docs); an attachment naming a customer-managed policy the
                // account does not hold grants nothing.
                None if is_aws_managed_policy(attached) => {
                    caller.unreadable_attachment = true;
                }
                None => {}
            }
        }
    } else if let Some(user_name) = iam_user_name(&caller.arn) {
        caller.documents.extend(inline_documents(
            &store.user_inline_policies,
            &iam_store::name_key(user_name),
        ));
    }
    caller
}

/// Whether `arn` names an AWS-MANAGED policy: one whose account segment is IAM's
/// `aws`, so it belongs to AWS rather than to this account and the IAM store holds no
/// document for it. Read through IAM's own ARN parser, so RUM and IAM agree on what an
/// AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam_store::parse_policy_arn(arn)
        .is_some_and(|policy| policy.account == iam_store::AWS_MANAGED_POLICY_ACCOUNT)
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as `arn:<partition>:sts::<account>:assumed-role/<role
/// name>/<session name>` (`sts_store::sts_arn`), so the role is the first segment after
/// `assumed-role/`. `None` for any other ARN form.
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
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, and the resource
/// may itself carry colons, so only the first five fields are split off.
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

/// The inline policy documents one entity holds, from the IAM store's `<entity> -> <policy
/// name> -> policy` map.
fn inline_documents(
    policies: &::std::collections::BTreeMap<
        String,
        ::std::collections::BTreeMap<String, iam_store::InlinePolicy>,
    >,
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
