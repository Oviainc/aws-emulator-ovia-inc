// SPDX-License-Identifier: CC0-1.0

//! What the account's own Amazon EC2 store says about the resources an Amazon EVS
//! request names.
//!
//! An Amazon EVS environment is deployed INTO the caller's existing network: `vpcId` is
//! "the VPC associated with the environment" and `serviceAccessSubnetId` "the subnet
//! that is used to establish connectivity between the Amazon EVS control plane and
//! VPC", so both name resources the account already holds. Whether it holds them is not
//! something this service can answer from its own records — it is a fact about EC2 — so
//! the question is put to EC2's OWN store accessor in the shared engine, exactly as
//! CloudWatch RUM's authorization gate reads the IAM store rather than keeping a second
//! copy of what IAM knows. Nothing is cached: a VPC created a moment ago is usable at
//! once, and one that has been deleted stops being usable at once.
//!
//! A store this emulator cannot read at all — an engine that is not the in-memory one —
//! is EVIDENCE OF NOTHING and so refuses nothing: every function here answers `None`
//! for that case, and the callers treat `None` as "no failure to report".

use crate::ops::types::ValidationExceptionField;

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

/// The account's EC2 store in the shared engine, through EC2's own accessor.
fn ec2_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ec2::ComAmazonawsEc2Amazonec2State>> {
    ec2::service_state(engine)
}

/// The clause `CreateEnvironment` states for a `vpcId` the account holds no VPC for.
///
/// The sentence names the id the request gave and nothing from the store, and it is the
/// wording Amazon EVS states this refusal with: a request-derived validation clause that
/// names the member's value and what to check about it.
pub fn invalid_vpc_clause(vpc_id: &str) -> ValidationExceptionField {
    super::field_failure_stated(
        "vpcId",
        format!(
            "The VPC id {vpc_id} is not valid or does not exist. Please verify the VPC id \
             exists and is valid."
        ),
    )
}

/// The clause `CreateEnvironment` states for a `serviceAccessSubnetId` that is not a
/// subnet of the VPC the request named — because the account holds no such subnet, or
/// because the subnet it holds belongs to another VPC.
///
/// Stated in the same form as [`invalid_vpc_clause`], because it is the same kind of
/// failure: a member naming a network resource the environment cannot be deployed into.
pub fn invalid_subnet_clause(subnet_id: &str, vpc_id: &str) -> ValidationExceptionField {
    super::field_failure_stated(
        "serviceAccessSubnetId",
        format!(
            "The service access subnet id {subnet_id} is not valid or does not exist in VPC \
             {vpc_id}. Please verify the subnet id exists in the VPC and is valid."
        ),
    )
}

/// Every failure of the preconditions that the network a `CreateEnvironment` request
/// names exists: the VPC, and the service access subnet INSIDE that VPC.
///
/// The subnet is only judged once the VPC resolves — a subnet is identified within the
/// VPC that holds it, so an unknown VPC leaves nothing to judge it against, and the
/// refusal then names only the VPC. An unreadable EC2 store reports no failure at all.
pub fn network_failures<Eng: Send + Sync + 'static>(
    engine: &Eng,
    vpc_id: &str,
    service_access_subnet_id: &str,
) -> Vec<ValidationExceptionField> {
    let Some(state) = ec2_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    if !store.vpcs.contains_key(vpc_id) {
        return vec![invalid_vpc_clause(vpc_id)];
    }
    let subnet_in_vpc = store
        .subnets
        .get(service_access_subnet_id)
        .is_some_and(|subnet| subnet.vpc_id.as_deref() == Some(vpc_id));
    if subnet_in_vpc {
        Vec::new()
    } else {
        vec![invalid_subnet_clause(service_access_subnet_id, vpc_id)]
    }
}

/// The Elastic IP address the allocation `allocation_id` names, as EC2 holds it.
///
/// `None` when the account holds no such allocation, when the allocation carries no
/// public address, or when the EC2 store cannot be read: the association is then
/// reported without an `ipAddress` rather than with an address nothing allocated.
pub fn elastic_ip_address<Eng: Send + Sync + 'static>(
    engine: &Eng,
    allocation_id: &str,
) -> Option<String> {
    let state = ec2_state(engine)?;
    let store = state.lock();
    store
        .addresses
        .get(allocation_id)
        .and_then(|address| address.public_ip.clone())
}
