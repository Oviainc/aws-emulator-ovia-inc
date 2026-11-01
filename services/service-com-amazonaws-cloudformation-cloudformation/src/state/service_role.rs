// SPDX-License-Identifier: CC0-1.0

//! The PERMISSIONS a stack operation runs under: the identity policies of the stack's
//! service role, and the refusal a call those policies deny is answered with.
//!
//! ## The rule this module implements
//!
//! `CreateStackInput$RoleARN`: "The Amazon Resource Name (ARN) of an IAM role that
//! CloudFormation assumes to create the stack. CloudFormation uses the role's credentials
//! to make calls on your behalf." So the calls a deployment makes in the services that own
//! a stack's resources ([`super::provisioning`]) are not made with the caller's own
//! permissions but with that role's: a change whose API call the role is denied cannot be
//! carried out, and CloudFormation reports the resource it could not act on as failed with
//! that refusal — which is what parks a stack in `UPDATE_ROLLBACK_FAILED` when a rollback's
//! own calls are the denied ones, and what `ContinueUpdateRollback`'s `ResourcesToSkip` then
//! steps over.
//!
//! [`super::role_trust_refusal`] is the other half of the same reading, answered when the
//! request arrives: a role whose TRUST policy does not let CloudFormation assume it at all
//! refuses the operation outright. This module is about a role that CAN be assumed and what
//! its PERMISSIONS policies say about one call.
//!
//! ## What is refused, and what is not
//!
//! The decision is [`::service_common::iam_authorization`]'s — the tree's ONE evaluation of
//! IAM policy documents, so two services handed the same documents cannot answer
//! differently — and this gate acts on an EXPLICIT `Deny` alone, the one answer that is
//! evidence against the call whatever else is in force ("an explicit deny in any policy
//! overrides any allows"). A document that merely says nothing about the call withholds no
//! call here, for a reason particular to an emulator: the calls a deployment makes are this
//! crate's ([`super::provisioner`]), not the resource provider AWS ships for the type, so
//! the set of actions a deployment of one resource touches is not the set the real provider
//! needs a grant for — refusing for want of a grant would refuse deployments the real
//! service performs. Nothing else is judged either: a stack with no `RoleARN` runs with the
//! caller's own permissions ("CloudFormation uses a temporary session that's generated from
//! your user credentials"), an ARN the IAM store holds no role for may belong to another
//! account, and a document this emulator cannot read as a policy is evidence of nothing.
//!
//! ## How a call reaches the decision
//!
//! The role is resolved by the OPERATION that drives a deployment, from the IAM store and
//! with no store of this service locked, and put in scope for the whole of that deployment
//! ([`acting_as`]); the provisioning walk puts the RESOURCE each call is about in scope
//! around that resource's own calls ([`on_resource`]). The in-process call helpers then ask
//! [`refusal`] before they send anything, so the one place a call is authorized is the one
//! place it is made, and a call made outside a deployment (a readiness read a client's poll
//! drives, say) is judged by nothing here.
//!
//! Both scopes are TASK-local: a deployment's calls run in the task serving that request, so
//! two requests deploying two stacks under two roles never see each other's.
//!
//! ## Bounds
//!
//! One role's documents per deployment, at most [`MAX_ROLE_DOCUMENTS`] of them — a role
//! carrying more is one this gate cannot read in full, so it decides nothing — cloned out of
//! the IAM store once per deployment and shared by reference for every call of it.

use std::sync::Arc;

use ::service_common::iam_authorization::{self as iam, Decision};

/// The error code the refusal of a denied provisioning call carries.
///
/// The refusal is CloudFormation's own — the call was never made, so no owning service
/// answered a code of its own, and the spellings those services use differ
/// (`AccessDeniedException`, `AuthorizationError`). What a client acts on is the CLASS the
/// refusal falls in, and this is the code that class is read off
/// ([`super::provisioning`]'s `HANDLER_ERROR_CLASSES`), which is what says whether the
/// failed resource is worth retrying.
pub const ACCESS_DENIED: &str = "AccessDenied";

/// The role session name CloudFormation's own calls are made under, which is what an
/// authorization refusal names the caller by.
///
/// `RoleARN`: "CloudFormation uses the role's credentials to make calls on your behalf" —
/// those calls are made as a SESSION of that role, so the principal a refusal is about is
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<this name>`.
const SESSION_NAME: &str = "AWSCloudFormation";

/// The most identity-policy documents this gate reads for one role.
///
/// A role carrying more is one whose permissions this gate cannot establish, so it decides
/// nothing for it rather than deciding on part of them. The documented default quota is 10
/// inline policies and 10 attached managed policies per role, so this is that pair.
const MAX_ROLE_DOCUMENTS: usize = 20;

/// The permissions in force on one stack's service role: the principal the role's calls are
/// made as, and the identity-policy documents that decide them.
///
/// [`Self::unspecified`] is a deployment with no role to judge — a stack that named none, an
/// engine with no IAM store, an ARN that account holds no role for — and it refuses nothing.
#[derive(Debug, Clone, Default)]
pub struct ServiceRole {
    /// The session principal a refusal names ([`SESSION_NAME`]), or `None` where there is no
    /// role.
    principal: Option<String>,
    /// The identity-policy documents in force on the role: its inline policies and the
    /// documents of the policies attached to it, as IAM's own store collects them.
    documents: Vec<String>,
}

impl ServiceRole {
    /// A deployment whose calls no service role governs.
    pub fn unspecified() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The permissions the role `role_arn` names carries, read from the IAM store in the
    /// same shared engine the account's roles live in.
    ///
    /// Call it with the handler's `&*self.engine` and NO store of this service locked: it
    /// takes the IAM guard, clones what it reads and releases it, so the deployment that
    /// follows holds neither lock.
    pub fn of<Eng: Send + Sync + 'static>(engine: &Eng, role_arn: Option<&str>) -> Arc<Self> {
        let Some(role_arn) = role_arn else {
            return Self::unspecified();
        };
        let Some(iam) = super::iam_state(engine) else {
            return Self::unspecified();
        };
        let store = iam.lock();
        // An ARN this account holds no role for says nothing about any call: the role may
        // belong to another account, or to a client that never told this emulator about it.
        let Some(role) = store.roles.values().find(|role| role.arn == role_arn) else {
            return Self::unspecified();
        };
        let role_name = role.role_name.clone();
        // The documents are IAM's OWN collection of what an identity carries — its inline
        // policies plus the documents of the policies attached to it — called rather than
        // re-derived, so this gate reads exactly what IAM itself judges that role on.
        let documents = store
            .principal_policies(role_arn)
            .map(|policies| policies.documents)
            .unwrap_or_default();
        drop(store);
        if documents.len() > MAX_ROLE_DOCUMENTS {
            return Self::unspecified();
        }
        Arc::new(Self {
            principal: Some(session_principal(role_arn, &role_name)),
            documents,
        })
    }

    /// The sentence refusing `action` on `resource`, or `None` where the role's documents
    /// show nothing against that call.
    ///
    /// The wording is the one AWS answers an unauthorized call with — the principal, the
    /// action, the resource and why — every part of it read off the request and the
    /// account's own records.
    fn refusal(&self, action: &str, resource: &str) -> Option<String> {
        let principal = self.principal.as_deref()?;
        if self.documents.is_empty() {
            return None;
        }
        let request = iam::Request::new(action, resource, &[]);
        if iam::identity_decision(&self.documents, &request) != Decision::Denied {
            return None;
        }
        Some(format!(
            "User: {principal} is not authorized to perform: {action} on resource: \
             {resource} with an explicit deny in an identity-based policy"
        ))
    }
}

/// The principal a session of `role_arn` is: the partition and the account are the role's
/// own, read off its ARN through the ONE ARN parse this workspace has, so a role of another
/// account is named as that role's session rather than as this emulator's.
fn session_principal(role_arn: &str, role_name: &str) -> String {
    let parsed = ::service_common::arn::parse(role_arn);
    let partition = parsed.map_or("aws", |arn| arn.partition);
    let account = parsed.map_or("", |arn| arn.account);
    format!("arn:{partition}:sts::{account}:assumed-role/{role_name}/{SESSION_NAME}")
}

tokio::task_local! {
    /// The service role the deployment running in this task makes its calls under, set by
    /// [`acting_as`] around the whole of that deployment.
    static ROLE: Arc<ServiceRole>;
    /// The resource the calls running inside this scope are about, set by [`on_resource`]
    /// around one resource's own calls: a policy statement's `Resource` element is what it
    /// is matched against.
    static TARGET: String;
}

/// Run `deployment` with `role`'s permissions in force for every call it makes.
pub async fn acting_as<F: ::std::future::Future>(role: Arc<ServiceRole>, deployment: F) -> F::Output {
    ROLE.scope(role, deployment).await
}

/// Run `calls` — the calls that create, change or delete ONE resource — with that resource
/// as what they are authorized against.
pub async fn on_resource<F: ::std::future::Future>(resource: String, calls: F) -> F::Output {
    TARGET.scope(resource, calls).await
}

/// The refusal the service role in force answers a call of `operation` on `signing_name`
/// with, or `None` where nothing refuses it.
///
/// `signing_name` is the service the call is signed for and `operation` the API operation it
/// invokes, which together are the IAM action it asks for: an action is
/// `<service prefix>:<operation name>`, and the prefix is the name the service signs under.
/// A call made outside a deployment, or about no resource, is judged by nothing: there is no
/// role in scope to judge it on.
pub fn refusal(signing_name: &str, operation: &str) -> Option<String> {
    let role = ROLE.try_with(Arc::clone).ok()?;
    let resource = TARGET.try_with(String::clone).ok()?;
    role.refusal(&format!("{signing_name}:{operation}"), &resource)
}

/// What a policy statement's `Resource` element is matched against for the calls about
/// `resource`: the ARN the resource carries, which is what a statement names it by, and
/// otherwise the identifier its owning service knows it by.
///
/// Every resource carries an ARN: either its physical id IS one ("Ref returns the Amazon
/// Resource Name (ARN)", for the types documented that way) or one of its `…Arn` return
/// values is, and a type that publishes neither still carries the conventional one
/// [`super::template::provision`] computes for it from the stack's identity and the
/// resource's name. The physical id is the fallback for a record carrying no ARN at all,
/// which is a resource no statement scoped to an ARN can be about either.
pub fn target_of(resource: &super::StoredResource) -> String {
    if resource.physical_resource_id.starts_with("arn:") {
        return resource.physical_resource_id.clone();
    }
    resource
        .attributes
        .iter()
        .find(|(attribute, value)| attribute.ends_with("Arn") && value.starts_with("arn:"))
        .map(|(_, arn)| arn.clone())
        .unwrap_or_else(|| resource.physical_resource_id.clone())
}
