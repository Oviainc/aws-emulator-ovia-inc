// SPDX-License-Identifier: CC0-1.0

//! The Amazon VPC networking an `awsvpc` placement NAMES, resolved against the task
//! definition it places.
//!
//! A task definition in the `awsvpc` network mode "is allocated its own elastic network
//! interface", and that interface is placed in the subnets the request names
//! (`AwsVpcConfiguration$subnets`: "The IDs of the subnets associated with the task or
//! service") with the security groups it names — so a placement of such a revision has to
//! carry a `networkConfiguration` that says where the interface goes, and a placement of a
//! revision in any other mode has no interface of its own to configure.
//!
//! Every operation that places a task from a stored revision — `RunTask`, `StartTask`,
//! `CreateService`, `UpdateService`, `CreateTaskSet` — is held to the one rule here, so none
//! of them can answer differently about the same pair.

use super::*;

/// `InvalidParameterException$message` for an `awsvpcConfiguration` whose `subnets` list is
/// empty. `subnets` is "The IDs of the subnets associated with the task or service", and the
/// interface an `awsvpc` task is given is created IN one of them, so a list naming none
/// configures no interface at all. The whole sentence is fixed text, naming the member and
/// nothing of the request.
pub const AWSVPC_SUBNETS_EMPTY_MESSAGE: &str = "subnets can not be empty.";

/// The refusal a launch earns when its `networkConfiguration` does not agree with the
/// network mode of the task definition it runs, or `None` when the two agree.
///
/// The rule, which the model cannot express because `networkConfiguration` is optional on
/// every launch: a task definition in the `awsvpc` network mode gets its own elastic
/// network interface, so the launch has to say which subnets and security groups that
/// interface takes ([`AWSVPC_NETWORK_CONFIGURATION_REQUIRED_MESSAGE`]) — and a task
/// definition in any other mode has no interface of its own to configure, so a network
/// configuration is not a thing that launch can carry
/// ([`NETWORK_CONFIGURATION_NOT_VALID_MESSAGE`]). Saying which subnets the interface takes
/// means naming at least one of them: an empty list names none, so there is nowhere for the
/// interface to be created ([`AWSVPC_SUBNETS_EMPTY_MESSAGE`]).
///
/// Shared by every operation that launches a task from a stored revision, so `RunTask`
/// and `CreateService` cannot answer differently about the same pair.
pub fn network_configuration_violation(
    network_mode: &str,
    network_configuration: Option<&NetworkConfiguration>,
) -> Option<String> {
    // `NetworkConfiguration` carries exactly one member, `awsvpcConfiguration`, so a
    // configuration without it configures no interface and is not one.
    let awsvpc = network_configuration
        .and_then(|configuration| configuration.awsvpc_configuration.as_ref());
    match (network_mode == NETWORK_MODE_AWSVPC, awsvpc.is_some()) {
        (true, false) => return Some(AWSVPC_NETWORK_CONFIGURATION_REQUIRED_MESSAGE.to_string()),
        (false, true) => return Some(NETWORK_CONFIGURATION_NOT_VALID_MESSAGE.to_string()),
        _ => {}
    }
    awsvpc
        .is_some_and(|awsvpc| awsvpc.subnets.is_empty())
        .then(|| AWSVPC_SUBNETS_EMPTY_MESSAGE.to_string())
}

/// The Amazon EC2 error code for a subnet id no subnet of the account carries, as
/// `DescribeSubnets` answers it.
pub const SUBNET_NOT_FOUND_ERROR_CODE: &str = "InvalidSubnetID.NotFound";

/// `InvalidParameterException$message` for a placement whose `awsvpcConfiguration` names a
/// subnet Amazon VPC holds none of.
///
/// Amazon ECS states the lookup it made and the answer Amazon VPC gave it: the subnets it
/// asked about, then Amazon EC2's own sentence for the first id that resolved to nothing
/// ("The subnet ID '<id>' does not exist") and the error code that sentence came with. Every
/// variable part is taken from the request — the ids it named, in the order it named them —
/// and from the lookup's answer.
pub fn subnet_lookup_message(requested: &[String], missing: &str) -> String {
    format!(
        "Error retrieving subnet information for [{}]: The subnet ID '{missing}' does not \
         exist (ErrorCode: {SUBNET_NOT_FOUND_ERROR_CODE})",
        requested.join(", ")
    )
}

/// The refusal a placement earns when a subnet its `awsvpcConfiguration` names is one
/// Amazon VPC holds no subnet for in `region`, or `None` when every named subnet resolves.
///
/// The interface an `awsvpc` task is allocated is created IN one of the subnets the
/// placement named, so a placement naming a subnet the account does not hold cannot be made:
/// Amazon ECS looks the subnets up in Amazon VPC and answers the lookup failure it got back
/// ([`subnet_lookup_message`]). The subnets of the account are Amazon EC2's records, so they
/// are read out of the Amazon EC2 service's own store through the one shared engine.
///
/// The lookup is made in the Region the request was signed for: a subnet is in one
/// Availability Zone, and so in one Region (`Subnet$availabilityZone`), so a placement in
/// one Region cannot use a subnet of another, and an id that names a subnet of another
/// Region names none of this one.
///
/// LOCK ORDER: this takes the Amazon EC2 store's lock, holds no other while it does, and
/// hands back an owned message — so it is called with NO guard of this service's store held,
/// which is what keeps a lock cycle between the two stores unreachable. The same contract
/// [`evaluate_image_pulls`] has.
pub fn unresolved_subnet_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    configuration: Option<&NetworkConfiguration>,
) -> Option<String> {
    let requested = configuration
        .and_then(|configuration| configuration.awsvpc_configuration.as_ref())
        .map(|awsvpc| awsvpc.subnets.clone())
        .unwrap_or_default();
    if requested.is_empty() {
        return None;
    }
    let ec2 = ::service_com_amazonaws_ec2_amazonec2::state::service_state(engine)?;
    let missing = {
        let store = ec2.lock();
        requested
            .iter()
            .find(|subnet| {
                store
                    .subnets
                    .get(subnet.as_str())
                    .and_then(|subnet| subnet.availability_zone.as_deref())
                    .is_none_or(|zone| !zone.starts_with(region))
            })
            .cloned()
    };
    missing.map(|missing| subnet_lookup_message(&requested, &missing))
}
