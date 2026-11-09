// SPDX-License-Identifier: CC0-1.0

//! The Amazon VPC records a suite's resources are placed in, in the account's own Amazon
//! EC2 store.
//!
//! Several services RESOLVE a network reference against those records before they place
//! anything: Amazon ECS creates the elastic network interface of an `awsvpc` task IN one of
//! the subnets the placement names, and Route 53 Resolver puts an endpoint's IP addresses in
//! subnets of the caller's VPC. A suite whose requests name a subnet LITERALLY — because its
//! own rule is about something else, and the id has to be the fixed one its requests and
//! assertions carry, which `CreateSubnet` (which mints its own) cannot give — therefore has
//! to hold that network before the first request. This is the one place these suites write
//! it, as the modelled shapes Amazon EC2 reports them as.

use service_com_amazonaws_ec2_amazonec2 as ec2;

/// Put `vpc_id` and `subnet_ids` in the account's Amazon EC2 store, every subnet in the
/// Availability Zone `availability_zone` of the VPC.
///
/// The Region of a subnet is the Region of its zone, so a suite signing for one Region
/// names a zone of that Region here; a placement in another Region resolves none of these.
pub(crate) fn register_network(
    engine: &engine::InMemoryEngine,
    vpc_id: &str,
    subnet_ids: &[&str],
    availability_zone: &str,
) {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's Amazon EC2 store");
    let mut store = ec2_state.lock();
    store.vpcs.insert(
        vpc_id.to_string(),
        ec2::ops::types::Vpc {
            vpc_id: Some(vpc_id.to_string()),
            cidr_block: Some("10.0.0.0/16".to_string()),
            state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
            ..Default::default()
        },
    );
    for (index, subnet_id) in subnet_ids.iter().enumerate() {
        store.subnets.insert(
            (*subnet_id).to_string(),
            ec2::ops::types::Subnet {
                subnet_id: Some((*subnet_id).to_string()),
                vpc_id: Some(vpc_id.to_string()),
                cidr_block: Some(format!("10.0.{index}.0/24")),
                availability_zone: Some(availability_zone.to_string()),
                state: Some(ec2::state::SUBNET_STATE_AVAILABLE.to_string()),
                ..Default::default()
            },
        );
    }
}
