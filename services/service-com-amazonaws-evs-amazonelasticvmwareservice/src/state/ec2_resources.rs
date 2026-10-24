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
//!
//! The same store is also WRITTEN here, for the one resource Amazon EVS creates in the
//! account on the caller's behalf: an Amazon EVS host IS an EC2 bare metal instance
//! (`Host` doc), so [`launch_host_instance`] puts that instance in EC2's own store
//! through EC2's own state API, and the host reports the id EC2 minted for it. Nothing
//! else is written: the VPC, the subnets and the Elastic IP addresses an environment is
//! deployed into belong to the caller and are only read.

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

/// How many elastic network interfaces Amazon EVS attaches to a host.
///
/// "Amazon EVS provisions two identically configured ENIs in the VMkernel management
/// subnet during host creation. One ENI is active, and the other is in standby mode for
/// automatic switchover during a failure scenario" (`NetworkInterface` shape doc), which
/// is also the `@length(max: 2)` the model puts on `Host$networkInterfaces`.
pub const HOST_NETWORK_INTERFACES: usize = 2;

/// The EC2 resources Amazon EVS created for one ESX host: the bare metal instance that
/// IS the host, and the elastic network interfaces attached to it.
///
/// The two always travel together, because one launch produces both — which is why a
/// host either reports `ec2InstanceId` AND `networkInterfaces` or reports neither.
#[derive(Debug, Clone)]
pub struct HostEc2Resources {
    /// The `InstanceId` of the EC2 instance the host runs on, as EC2's own store minted
    /// it: the id `DescribeInstances` answers for.
    pub instance_id: String,
    /// The `NetworkInterfaceId` of each ENI attached to the host, in attachment order.
    pub network_interface_ids: Vec<String>,
}

/// What a host launch needs to know about the host and the environment it joins.
///
/// Every field is a fact the request or the stored environment already carries: nothing
/// here is invented, so the instance EC2 ends up holding describes the same host Amazon
/// EVS reports.
#[derive(Debug, Clone, Copy)]
pub struct HostLaunch<'a> {
    /// The Region the environment lives in — the Region the request was addressed to.
    pub region: &'a str,
    /// The EC2 bare metal instance type the request asked for.
    pub instance_type: &'a str,
    /// The SSH key pair the request named.
    pub key_name: &'a str,
    /// The host's management IP address, taken out of the `vmkManagement` VLAN subnet:
    /// the instance's primary private IPv4 address, because that address IS the host's.
    pub management_ip: &'a str,
    /// The `vmkManagement` VLAN subnet the host's ENIs live in.
    pub management_subnet_id: &'a str,
    /// The environment's service access subnet, read for the Availability Zone the
    /// environment is deployed in.
    pub service_access_subnet_id: &'a str,
}

/// Launch the EC2 bare metal instance an ESX host IS, and attach its ENIs.
///
/// "An ESX host that runs on an Amazon EC2 bare metal instance" (`Host` doc): the host
/// is not an Amazon EVS record that merely mentions an instance, it is that instance, so
/// adding a host to an environment has to put a real instance in the ACCOUNT's own EC2
/// store — the one `DescribeInstances` answers from — and report its id. The instance is
/// written through EC2's own state API ([`ec2::ComAmazonawsEc2Amazonec2State::insert_instance`]),
/// so it is indistinguishable from one `RunInstances` launched: `pending` with the move
/// to `running` the clock owes it, its primary private address the host's own management
/// address, and its subnet the VLAN subnet that address came out of.
///
/// `None` — no instance launched, so the host reports neither `ec2InstanceId` nor
/// `networkInterfaces` rather than naming resources that do not exist — when the EC2
/// store cannot be read at all (the same "evidence of nothing" answer every function in
/// this module gives for that case), or when that store is at a ceiling of its own.
///
/// The caller holds this service's own store while this runs. That nesting is safe in
/// one direction only, and this is it: Amazon EC2's handlers never read Amazon EVS
/// state, so EVS -> EC2 is the only order either lock is ever taken in.
pub fn launch_host_instance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch: HostLaunch<'_>,
) -> Option<HostEc2Resources> {
    let state = ec2_state(engine)?;
    let mut store = state.lock();

    // The Availability Zone the environment is deployed in: the zone of the service
    // access subnet the caller gave it, which is a fact about that EC2 subnet rather
    // than a choice. An account whose store no longer holds that subnet falls back to
    // the Region's first zone, the way EC2's own placement does — and a Region with no
    // zone at all leaves nothing to place an instance in, so nothing is launched: a
    // Region name is not a zone name, and storing one would report a zone no
    // `DescribeAvailabilityZones` lists.
    let availability_zone = store
        .subnets
        .get(launch.service_access_subnet_id)
        .and_then(|subnet| subnet.availability_zone.clone())
        .or_else(|| {
            ec2::availability_zones_in_region(launch.region)
                .first()
                .map(|zone| zone.zone_name.clone())
        })?;

    // The EC2 store's own ceilings, asked BEFORE anything is minted: this writer is
    // bound by them exactly as EC2's own launch paths are.
    let payload_bytes = launch.instance_type.len()
        + launch.key_name.len()
        + launch.management_subnet_id.len()
        + availability_zone.len();
    if store.instances.len() >= ec2::MAX_STORED_INSTANCES
        || store.would_exceed_stored_bytes(payload_bytes)
    {
        return None;
    }

    // One host is one instance, so it is its own reservation, the way a Spot Instance
    // launch is.
    let reservation_id = store.mint_reservation_id();
    store.insert_reservation(ec2::ReservationRecord {
        reservation_id: reservation_id.clone(),
        owner_id: ::engine::emulator_account_id().to_string(),
        requester_id: None,
        instance_ids: Vec::new(),
    });
    let instance_id = store.mint_instance_id();
    store.insert_instance(ec2::InstanceRecord {
        instance_id: instance_id.clone(),
        reservation_id,
        // The ESX image Amazon EVS boots a host from is published by Amazon EVS, and
        // this emulator's AMI catalogue holds no such image: an `ami-` id invented here
        // would name an image `DescribeImages` cannot report, so none is claimed.
        image_id: None,
        instance_type: launch.instance_type.to_string(),
        key_name: Some(launch.key_name.to_string()),
        // The host's ENIs live in the VMkernel management subnet (`NetworkInterface`
        // doc), which is the subnet its management address came out of.
        subnet_id: Some(launch.management_subnet_id.to_string()),
        // The environment's service access security groups guard the Amazon EVS control
        // plane's access to the VPC, not the host, so the launch names no group.
        security_groups: Vec::new(),
        private_dns_name: ec2::private_dns_name(launch.management_ip, launch.region),
        private_ip: launch.management_ip.to_string(),
        // A host is reached over the VLAN subnets, never over a public address.
        public_ip: None,
        // Just launched: `pending`, with the move to `running` the clock owes it on
        // EC2's own span — the host's own CREATING -> CREATED runs on Amazon EVS's.
        state: ec2::InstanceStateName::Pending,
        pending_transition: Some(ec2::PendingTransition::starting_now(
            ec2::InstanceStateName::Running,
        )),
        state_transition_reason: None,
        state_reason: None,
        launch_time: ::std::time::SystemTime::now(),
        ami_launch_index: 0,
        // "EC2 instances created through Amazon EVS do not support associating an IAM
        // instance profile" (`instanceType` doc) and nothing passes user data to an ESX
        // host either.
        user_data: None,
        placement_az: availability_zone,
        monitoring_state: ec2::MONITORING_STATE_DISABLED.to_string(),
        architecture: ec2::architecture_for(launch.instance_type),
        ebs_optimized: false,
        hibernation_configured: false,
        disable_api_termination: false,
        // The environment's tags tag the ENVIRONMENT; nothing tags the instance.
        tags: ::std::collections::BTreeMap::new(),
        client_token: None,
        // Not a Spot Instance: an ESX host is launched on demand.
        spot_instance_request_id: None,
    });
    // The ENIs. EC2 holds no network interface store yet (every `…NetworkInterface`
    // operation is still a scaffold stub), so the ids are minted from EC2's OWN id
    // counter — `eni-` + 17 hex digits, the form `ec2::network_interface_id_is_well_formed`
    // reads — and are therefore unique among the account's EC2 ids, ready to be inserted
    // into that store on the day it lands.
    let network_interface_ids = (0..HOST_NETWORK_INTERFACES)
        .map(|_| store.next_id("eni"))
        .collect();
    Some(HostEc2Resources {
        instance_id,
        network_interface_ids,
    })
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
