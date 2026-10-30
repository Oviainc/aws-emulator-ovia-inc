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
//! carries no account to decide against; it passes too, and the handler's own ARN
//! check answers it.
//!
//! Passing a ROLE to the service ([`authorize_pass_role`]) is the same question
//! read the other way round, and it is the one case where a value that names no
//! account is refused rather than passed: there the caller must NAME a role of its
//! own account for the service to assume, so anything the account cannot be read
//! out of names no such role.
//!
//! The refusal's wording is the shared IAM sentence
//! ([`::service_common::iam_authorization::not_authorized_by_resource_policy`]), so
//! this service cannot drift from the other services that refuse an unauthorized
//! request.
//!
//! The decision is taken by the operations whose target IS a cluster — the ones
//! whose action applies to it. The TAGGING operations take any MSK resource ARN
//! instead, and the behavioural record for a foreign-account resource there is the
//! not-found refusal (`tests/all/resource_tags.rs`, its other-account row), so they
//! deliberately do not call this module: the two answers are each their own
//! operation's, not an inconsistency to even out.

/// The IAM action namespace MSK publishes its CONTROL-plane actions under, which
/// `kafka:<ApiName>` is the action for the API of that name.
const CONTROL_PLANE_ACTION_NAMESPACE: &str = "kafka";

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
    authorize_action(
        &format!("{CONTROL_PLANE_ACTION_NAMESPACE}:{operation}"),
        target_arn,
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
