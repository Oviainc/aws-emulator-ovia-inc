// SPDX-License-Identifier: CC0-1.0

//! The resource-scoped authorization decision a cluster-scoped operation takes
//! BEFORE it resolves the cluster its target ARN names.
//!
//! Every operation of this service that acts on ONE cluster names that cluster by
//! ARN — in the URI (`ClusterArn` is an `@httpLabel` on all of them) or, for
//! `CreateVpcConnection` and `GetCompatibleKafkaVersions`, in the request. The
//! action it asks for therefore applies to that ARN, and whether the caller may
//! perform it is decided from the REQUEST alone: the action, the calling principal
//! and the ARN as submitted. So the decision comes first — before the handler
//! resolves the cluster, and before it validates the rest of the body — which is
//! why it lives here, next to the state the resolution then reads, rather than
//! inside any one handler.
//!
//! A resource in another ACCOUNT is the case this emulator can decide. The
//! emulator serves exactly one account ([`caller_account`]), whose single principal
//! is [`caller_principal`]; it therefore holds no cluster of any other account, and
//! no resource-based policy of one either. A target ARN whose account segment is
//! not the caller's is consequently allowed by nothing, and the answer is the
//! authorization refusal rather than the absence the registry would otherwise
//! report — "you may not touch this" and "it is not there" are different answers,
//! and a client acts on them differently.
//!
//! An ARN whose account segment IS the caller's is the owning account's own
//! request, which this emulator has no identity-based policies to refuse it by, so
//! it passes the gate and is resolved as usual. A value that is not an ARN at all
//! carries no account to decide against, so the account test passes it; whether it
//! is then refused depends on where the request carried it. An `@httpLabel` ARN
//! names a path segment the caller can only have mistyped, and the handler's own
//! ARN check answers it as the bad parameter it is. The TARGET of
//! `CreateVpcConnection` is decided instead by [`authorize_target_cluster`]: the
//! action applies to a cluster, so a value naming none leaves nothing for a policy
//! to grant it on, and the decision — taken over [`ANY_RESOURCE`] — refuses the
//! call before any member of the body is read.
//!
//! A cluster also carries a resource-based policy OF ITS OWN
//! ([`authorize_cluster_policy`]), which the operations MSK's cluster policy
//! governs ([`CLUSTER_POLICY_ACTIONS`]) take into account once the cluster is
//! resolved: an explicit deny in it refuses the call whatever else allows it, and
//! before the operation looks at anything else about the cluster or the request.
//!
//! Passing a ROLE to the service ([`authorize_pass_role`]) is the same question
//! read the other way round, and it is the one case where a value that names no
//! account is refused rather than passed: there the caller must NAME a role of its
//! own account for the service to assume, so anything the account cannot be read
//! out of names no such role.
//!
//! A refusal that rests on a resource-based policy is worded in the shared IAM
//! sentence ([`::service_common::iam_authorization::not_authorized_by_resource_policy`]),
//! so this service cannot drift from the other services that refuse an
//! unauthorized request; the two sentences AWS words the other decisions in — a
//! request nothing grants, and one an attached document explicitly denied — are
//! built here from the same parts.
//!
//! What the CALLER's own identity-based policies say about the action is the other
//! half of the same question, and it is decided in
//! [`crate::state::identity_authorization`]: that decision reads the IAM and STS
//! stores for the principal the request's credential speaks for, where every
//! decision here is taken from the request alone, which is why the two sit in
//! separate modules. Both word their refusal from the parts below, so one service
//! says one thing about an unauthorized request however it was refused.
//!
//! The decision is taken by the operations whose target IS a cluster — the ones
//! whose action applies to it. The TAGGING operations take any MSK resource ARN
//! instead, and the behavioural record for a foreign-account resource there is the
//! not-found refusal (`tests/all/resource_tags.rs`, its other-account row), so they
//! deliberately do not call this module: the two answers are each their own
//! operation's, not an inconsistency to even out.

/// The IAM action namespace MSK publishes its CONTROL-plane actions under, which
/// `kafka:<ApiName>` is the action for the API of that name.
pub const CONTROL_PLANE_ACTION_NAMESPACE: &str = "kafka";

/// The IAM action a control-plane call to `operation` asks for: `kafka:<ApiName>`.
///
/// One spelling, taken by every decision this service makes about such a call —
/// the resource-scoped ones here and the identity-based one in
/// [`crate::state::identity_authorization`] — so the action a refusal names cannot
/// drift between them. The caller passes its own `Operation::NAME`, so the action
/// can never drift from the operation taking it either.
pub fn control_plane_action(operation: &str) -> String {
    format!("{CONTROL_PLANE_ACTION_NAMESPACE}:{operation}")
}

/// The IAM action a TOPIC Management call is authorized under.
///
/// The topic operations act on a cluster's Apache Kafka data plane rather than on
/// the control plane's cluster resource, and MSK publishes that plane's actions in
/// its own `kafka-cluster` namespace; reaching a cluster's topics at all is the
/// `Connect` action on the cluster, so that is the action those calls are refused
/// for whatever they then do to a topic.
pub const CLUSTER_CONNECT_ACTION: &str = "kafka-cluster:Connect";

/// The wire error code an authorization refusal carries.
///
/// This service's model declares no shape for it — it lists `ForbiddenException`
/// (403) and `UnauthorizedException` (401) — so the refusal leaves through
/// [`crate::scaffolded::Scaffolded::unmodeled`]: a client branches on the code, so
/// the code the real service answers is the code that goes on the wire.
pub const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The HTTP status that refusal carries, which is the status this service's own
/// 403 error models.
pub const ACCESS_DENIED_STATUS: u16 = 403;

/// What MSK answers a `CreateReplicator` whose service-execution role belongs to
/// another account: fixed text end to end, carrying nothing from the request.
pub const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// The account every request to this emulator is served for, and therefore the
/// account a resource has to belong to for the caller to hold any policy over it.
pub fn caller_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The IAM principal every request to this emulator speaks for — the one the
/// account holds and `sts:GetCallerIdentity` reports — which is the principal an
/// authorization refusal names.
///
/// Read from the shared definition, so the principal this service refuses in the
/// name of is the same one every other service of this emulator names.
pub fn caller_principal() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// The authorization decision for `action` (a fully qualified IAM action) on the
/// resource `target_arn` names, as submitted.
///
/// `Ok(())` when nothing about the request refuses it; otherwise the refusal, ready
/// to be returned from any operation's error channel.
pub fn authorize_action<E>(
    action: &str,
    target_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let Some(parsed) = ::service_common::arn::parse(target_arn) else {
        return Ok(());
    };
    if parsed.account.is_empty() || parsed.account == caller_account() {
        return Ok(());
    }
    Err(crate::scaffolded::Scaffolded::unmodeled(
        ACCESS_DENIED_CODE,
        ACCESS_DENIED_STATUS,
        ::service_common::iam_authorization::not_authorized_by_resource_policy(
            &caller_principal(),
            action,
            target_arn,
        ),
    ))
}

/// The same decision for the control-plane action of `operation` — the API name,
/// which the caller passes as its own `Operation::NAME` so the action named can
/// never drift from the operation taking it.
pub fn authorize_operation<E>(
    operation: &str,
    target_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    authorize_action(&control_plane_action(operation), target_arn)
}

/// The resource an authorization decision is taken over when the request named
/// none this service can act on: IAM's wildcard, which is what an action with no
/// resource to scope it is decided against, and what the refusal then names.
pub const ANY_RESOURCE: &str = "*";

/// The decision for the control-plane action of `operation` on the CLUSTER
/// `target_arn` is meant to name, taken before anything resolves or parses that
/// target.
///
/// `kafka:<ApiName>` applies to a cluster resource, so the decision needs one: a
/// value that names a cluster of another account is refused as
/// [`authorize_action`] refuses it, and a value that names no cluster of this
/// account AT ALL leaves the action with no resource to be granted on — the
/// decision is then taken over [`ANY_RESOURCE`], which nothing in this account
/// grants, and the refusal names what it was decided over. Either way the answer is
/// the authorization refusal rather than a complaint about the value's shape: the
/// decision comes first, so the request never reaches the parameter validation
/// that would describe the target, and nothing of what this registry holds is
/// disclosed.
pub fn authorize_target_cluster<E>(
    operation: &str,
    target_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    authorize_operation(operation, target_arn)?;
    if crate::state::is_cluster_arn(target_arn) {
        return Ok(());
    }
    let action = control_plane_action(operation);
    // A value this account's ARN reading can make nothing of names no resource;
    // one it can read names that resource, whatever kind it is, so the decision
    // reports the resource it was actually taken over.
    let resource = if ::service_common::arn::parse(target_arn).is_some() {
        target_arn
    } else {
        ANY_RESOURCE
    };
    Err(access_denied(not_authorized(&action, resource)))
}

/// The IAM actions an MSK CLUSTER POLICY governs — the resource-based policy
/// `PutClusterPolicy` attaches to a cluster, which Amazon MSK documents as granting
/// another account's principals multi-VPC private connectivity to it: creating a
/// VPC connection, and the two cluster readers plus the bootstrap-broker reader a
/// client needs to use one.
///
/// Spelled as API names, which is what the operations taking the decision pass as
/// their own `Operation::NAME`; `kafka:` prefixed, they are the actions a stored
/// document is evaluated for.
pub const CLUSTER_POLICY_ACTIONS: [&str; 4] = [
    "CreateVpcConnection",
    "GetBootstrapBrokers",
    "DescribeCluster",
    "DescribeClusterV2",
];

/// The decision the cluster's OWN resource-based policy takes about the
/// control-plane action of `operation` on `cluster`, after the cluster has been
/// resolved and before the operation does anything else.
///
/// IAM's order, through the tree's one evaluation of policy documents
/// ([`::service_common::iam_authorization`]): an explicit `Deny` about this request
/// refuses it whatever else allows it, while the owning account's own request is
/// allowed by its identity-based policies, so a document that merely states nothing
/// about the request — or no document at all — leaves the call served. The
/// principals a document may name the caller by are the calling principal itself
/// and the account it belongs to, which is what a resource policy may name in its
/// place.
///
/// Taken by the operations [`CLUSTER_POLICY_ACTIONS`] names, which are the ones a
/// cluster policy governs. The work it does is bounded by the stored document
/// rather than by the request: `PutClusterPolicy` caps what one cluster may hold
/// ([`crate::state::MAX_CLUSTER_POLICY_BYTES`]), and the shared evaluation reads no
/// document longer than its own ceiling.
pub fn authorize_cluster_policy<E>(
    operation: &str,
    cluster: &crate::state::Cluster,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let Some(policy) = cluster.policy.as_ref() else {
        return Ok(());
    };
    let action = control_plane_action(operation);
    let caller = caller_principal();
    let mut principals = vec![caller.clone()];
    principals.extend(::service_common::iam_authorization::account_principals(
        &caller,
    ));
    let stated = ::service_common::iam_authorization::resource_decision(
        &policy.document,
        &::service_common::iam_authorization::Request::new(&action, &cluster.arn, &principals),
    );
    // The resource belongs to the account this emulator serves, so that account's
    // identity-based policies are the grant the combined decision starts from; only
    // an explicit deny in the attached document overturns it.
    if ::service_common::iam_authorization::is_allowed(
        ::service_common::iam_authorization::Decision::Allowed,
        stated,
    ) {
        return Ok(());
    }
    Err(access_denied(not_authorized_by_explicit_deny(
        &action,
        &cluster.arn,
    )))
}

/// The refusal an authorization decision leaves, carrying `message`: the code and
/// status the real service answers, through the unmodeled channel this service's
/// model gives no shape for.
pub(crate) fn access_denied<E>(message: String) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(ACCESS_DENIED_CODE, ACCESS_DENIED_STATUS, message)
}

/// The sentence AWS refuses a request NOTHING grants with: fixed connectives
/// around the calling principal, the action asked for and the resource the decision
/// was taken over — and no clause about a document, because none of them refused
/// it; the request simply reaches no grant.
fn not_authorized(action: &str, resource: &str) -> String {
    not_authorized_as(&caller_principal(), action, resource)
}

/// The same sentence about a principal the REQUEST's credential named rather than
/// the ambient one — the form an identity-based decision needs, since the principal
/// it refuses is the session that signed the call.
///
/// Built here, from the same parts, so a refusal reads alike whichever decision
/// reached it.
pub(crate) fn not_authorized_as(principal: &str, action: &str, resource: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource}")
}

/// The same sentence for a request a resource-based policy explicitly DENIED,
/// closed by the clause AWS names that document with: a client reading the message
/// learns that fixing its own permissions will not help.
fn not_authorized_by_explicit_deny(action: &str, resource: &str) -> String {
    format!(
        "{} with an explicit deny in a resource-based policy",
        not_authorized(action, resource)
    )
}

/// The decision for PASSING `role_arn` to this service, as `CreateReplicator`'s
/// `ServiceExecutionRoleArn` does.
///
/// Passing a role is the caller granting the service that role's permissions, and
/// MSK accepts only a role of the caller's OWN account for it. The question is
/// answered from the ARN, through the tree's one reading of it
/// ([`::service_common::iam_role::role_name`]): the value has to name an IAM ROLE
/// of this account, and any other value — a role of another account, an ARN of some
/// other identity or service, or text no role of this account can be read out of at
/// all — names no role to pass. All of them are refused outright, with one fixed
/// sentence that mentions neither the role nor anything else of the request, before
/// anything the request asks for is created.
pub fn authorize_pass_role<E>(role_arn: &str) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    // The ONE reading of "an IAM role ARN of this account" the tree has
    // (`service_common::iam_role`), which is also what settles the forms around the
    // edges: an IAM ARN carries no Region, a role under a path is the role IAM holds
    // under that name, and an empty account field names no OTHER account.
    if ::service_common::iam_role::role_name(role_arn).is_some() {
        return Ok(());
    }
    Err(crate::scaffolded::Scaffolded::unmodeled(
        ACCESS_DENIED_CODE,
        ACCESS_DENIED_STATUS,
        CROSS_ACCOUNT_PASS_ROLE_MESSAGE,
    ))
}
