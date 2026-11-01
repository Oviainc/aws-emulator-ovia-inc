// SPDX-License-Identifier: CC0-1.0

//! Whether a `Task` state's EXECUTION ROLE lets the state make the integration call
//! its `Resource` names.
//!
//! An execution's `Task` states do not act as whoever started the execution: Step
//! Functions makes each integration call ACTING AS the state machine's `roleArn` —
//! "the Amazon Resource Name (ARN) of the IAM role to use for this state machine"
//! (`CreateStateMachine$roleArn`) — so a call the policies in force on that role do
//! not allow is a task that FAILS rather than a call that happens. That is the
//! difference between a least-privilege deployment that works and one that only
//! appears to, and it is why the call is decided here BEFORE the request goes
//! anywhere: a child workflow the role was denied must not run.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the `roleArn` of the configuration the execution RAN, read at the
//!    moment of each call, so an execution started before an update is judged by the
//!    role it carries;
//! 2. **what the call asks for** — the `(action, resource)` pair the call is subject
//!    to, formed from the call itself ([`super::integration::access_request`]): the
//!    IAM action is the invoked service's action namespace and the operation's own
//!    name, and the resource is the ARN of what that integration's request names —
//!    the ARN itself where the operation takes one, and otherwise the ARN of the
//!    resource of that NAME in the Region the call is made in;
//! 3. **what the role may do** — the identity-based documents the IAM store holds for
//!    it (its inline policies and the default version of each managed policy attached
//!    to it), read through IAM's own
//!    [`principal_policies`](IamState::principal_policies) accessor so this service
//!    sees exactly what `CreateRole`/`PutRolePolicy` wrote, plus the permissions
//!    boundary that caps them;
//! 4. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of policy documents against a request, so this service and every
//!    other one handed the same documents decide alike. How the documents in force
//!    combine — an attachment whose document this account does not hold grants
//!    something unreadable, so it withholds a refusal without overturning an explicit
//!    `Deny`, and the boundary caps rather than grants — is read exactly as IAM's own
//!    authorizer reads it, which is the authority on its own store.
//!
//! The refusal is worded here, because only the invoked service can say what its own
//! refusal says: the principal is the role the call is made as — the emulator assumes
//! no session for a `Task` state's call, so there is no session identity to report it
//! under — and the metadata tail names the service, the status and the error code the
//! answer came back with, and nothing it did not carry.
//!
//! Nothing is refused on evidence this emulator does not have. Each of these shows
//! nothing about the role and lets the call be made: an engine carrying no IAM store,
//! a `roleArn` the store holds no role for (it may belong to another account), a role
//! carrying an attachment whose document this account does not hold — an AWS-managed
//! policy might be the very one that carries the grant — and a call no
//! [`super::integration::TARGET_RESOURCES`] entry names the target resource of, since
//! an access request cannot be formed without the resource it is about. What the
//! documents DO state is decided in IAM's own order: an explicit `Deny` refuses, a
//! grant allows, and policies that state nothing about the call withhold it, because
//! the documents in force on a role are the authority on what it may do.

use ::std::sync::Arc;

use ::service_common::iam_authorization::{self as iam, Decision};

use super::integration::{access_request, Call, Failed, RoleGate};


/// The IAM store a state machine's `roleArn` is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine
/// this service's store lives in, so a task reads the very records IAM's handlers
/// wrote rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The error code an AWS service refuses an unauthorized call with, which the cause
/// of a failed task names as the code the call came back under.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The status that refusal carries.
const ACCESS_DENIED_STATUS: u16 = 400;

/// What decides whether `role_arn` may make the calls of one walk, or `None` when
/// there is nothing to decide them against — an engine with no IAM store, or an
/// execution carrying no role at all.
///
/// `region` is the Region the walk's calls are made in — the execution's own — which
/// is where a resource a request names by NAME rather than by ARN lives, so it is
/// what the ARN the access request is over is formed in.
///
/// The gate is consulted once per call the walk asks for, so a statement withdrawn
/// between two states of one execution stops granting from the next call: nothing is
/// read in advance and nothing is cached.
pub fn role_gate<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    region: &str,
    role_arn: &str,
) -> Option<RoleGate> {
    if role_arn.is_empty() {
        return None;
    }
    iam_state(&**engine)?;
    let engine = Arc::clone(engine);
    let region = region.to_string();
    let role_arn = role_arn.to_string();
    Some(Arc::new(move |call: &Call, request: &::serde_json::Value| {
        let targets = call.targets?;
        let iam = iam_state(&*engine)?;
        let (action, resource) = access_request(call, request, &region)?;
        let refused = refusal(
            &iam.lock(),
            &role_arn,
            &action,
            &resource,
            targets.refusal_service,
        )?;
        Some(Failed {
            error: targets.refusal_error.to_string(),
            cause: refused,
        })
    }))
}

/// The IAM store in the SAME shared engine, which is where the account's roles live.
/// `None` means the engine carries no such store — a deployment with no IAM to ask,
/// which is evidence of nothing.
fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The sentence the invoked service refuses the call with when the role `role_arn`
/// may not perform `action` on `resource`, or `None` when nothing of the kind can be
/// shown (see the module docs for the cases that show nothing).
///
/// It is the refusal AS THE SDK REPORTS IT to the state: the service's own
/// not-authorized sentence, followed by the metadata that answer carried — the
/// service it came from, the status and the error code. The identifier of the refused
/// request is not among them: no call was made, so there is no request to name, and
/// nothing is invented to fill the field.
fn refusal(
    iam: &IamState,
    role_arn: &str,
    action: &str,
    resource: &str,
    service: &str,
) -> Option<String> {
    iam.roles.values().find(|role| role.arn == role_arn)?;
    // The documents in force on the role. The collection is bounded by IAM's own
    // quotas on how many policies one role carries, so this walk is bounded by the
    // store rather than by a number of its own.
    let policies = iam.principal_policies(role_arn)?;
    let request = iam::Request::new(action, resource, &[]);
    let stated = iam::identity_decision(&policies.documents, &request);
    // An attachment whose document this account does not hold is a grant this
    // emulator cannot read, so it cannot be the basis of a refusal — but it cannot
    // overturn one either: an explicit `Deny` in a document that IS held wins over
    // every grant, including one whose content is unknown here.
    let granted = iam::is_allowed(stated, Decision::NotStated)
        || (stated != Decision::Denied && !policies.uncarried.is_empty());
    // "A permissions boundary sets the maximum permissions that an identity-based
    // policy can grant to an entity": the boundary CAPS what those policies grant and
    // grants nothing itself, so a call it does not allow is refused even where they
    // allow it. A role carrying none — or one whose document is not held, a ceiling
    // this emulator cannot read — is capped by nothing. A call an identity-based
    // policy explicitly DENIES is already refused on that deny, so no ceiling is read
    // for it.
    let capped = if stated == Decision::Denied {
        Decision::Allowed
    } else {
        iam.permissions_boundary_document(role_arn)
            .map_or(Decision::Allowed, |document| {
                iam::identity_decision(::std::slice::from_ref(&document), &request)
            })
    };
    if granted && capped == Decision::Allowed {
        return None;
    }
    // Why the call was refused: an explicit `Deny` that is about it — stated in an
    // identity-based policy or in the boundary, either of which wins over every
    // `Allow` — or the absence of an `Allow`, which names the DOCUMENT that withheld
    // it: the boundary where the identity-based policies did allow the call, and the
    // identity-based policies where nothing allowed it at all.
    let because = if stated == Decision::Denied {
        "with an explicit deny in an identity-based policy".to_string()
    } else if capped == Decision::Denied {
        "with an explicit deny in a permissions boundary".to_string()
    } else if granted {
        format!("because no permissions boundary allows the {action} action")
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    // The acting principal is the role itself: the emulator assumes no session for a
    // `Task` state's call, so there is no session name to report it under.
    Some(format!(
        "User: {role_arn} is not authorized to perform: {action} on resource: {resource} \
         {because} (Service: {service}; Status Code: {ACCESS_DENIED_STATUS}; Error Code: \
         {ACCESS_DENIED_CODE}; Proxy: null)"
    ))
}
