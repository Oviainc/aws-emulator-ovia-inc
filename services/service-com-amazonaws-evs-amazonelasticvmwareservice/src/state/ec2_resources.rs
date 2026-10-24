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
//! The same store is also WRITTEN here, for the resources Amazon EVS creates in the
//! account on the caller's behalf:
//!
//! * an Amazon EVS host IS an EC2 bare metal instance (`Host` doc), so
//!   [`launch_host_instance`] puts that instance in EC2's own store through EC2's own
//!   state API, and the host reports the id EC2 minted for it;
//! * Amazon EVS "provisions the VLAN subnets" of an environment inside the VPC the
//!   request names, and "environment deletion also deletes the associated Amazon EVS
//!   VLAN subnets" (`CreateEnvironment` / `DeleteEnvironment` docs) — those subnets are
//!   EC2 subnets of the caller's account, so they are created in and removed from EC2's
//!   own store through EC2's own writers ([`provision_vlan_subnets`],
//!   [`release_vlan_subnets`]) rather than existing only inside this service's records:
//!   an id no other service holds would name nothing.
//!
//! Nothing else is written: the VPC, the service access subnet and the Elastic IP
//! addresses an environment is deployed into belong to the caller and are only read.
//!
//! Nothing in this module does arithmetic on an address or a prefix length of its own:
//! every judgement about a CIDR block a caller supplied is EC2's own published helper
//! (`canonical_subnet_ipv4_cidr`, `ipv4_cidr_block_contains`,
//! `ipv4_cidr_blocks_overlap`), so one reading of a block serves both services, and a
//! block those cannot read is skipped rather than guessed at. There is no indexing or
//! slicing of a caller-supplied string here, and no `unwrap`, `expect` or `panic!`: a
//! request cannot panic a handler through this module.
//!
//! LOCKING. Each function takes EC2's store lock for the shortest span that answers it,
//! does a bounded amount of work under it (at most one pass over the VPC's subnets, or
//! the ten subnets of one environment), and never awaits while holding it. The lock is
//! `engine::ServiceState::lock`, which recovers from poisoning rather than propagating a
//! panic, so a panicking request cannot wedge either store. The callers hold this
//! service's own lock across these calls, so the order is always EVS-then-EC2; EC2 never
//! reaches this service, so no cycle exists.

use std::time::Instant;

use crate::ops::types::ValidationExceptionField;

use ::service_com_amazonaws_ec2_amazonec2::ops::types as ec2_types;
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
        // An ESX host boots the image Amazon EVS publishes, not a kernel or RAM disk
        // image a launch names, so it carries neither; source/destination checking
        // starts at the default every EC2 instance starts at.
        kernel_id: None,
        ramdisk_id: None,
        source_dest_check: ec2::DEFAULT_SOURCE_DEST_CHECK,
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

/// The `Subnet$Type` Amazon EC2 reports for a subnet Amazon EVS provisioned:
/// "Indicates if this is a subnet used with Amazon Elastic VMware Service (EVS).
/// Possible values are `Elastic VMware Service` or no value" (`Subnet$Type` doc, EC2's
/// own model). It is what marks a subnet of the account's EC2 store as one of an
/// environment's VLAN subnets, so [`release_vlan_subnets`] removes only the subnets
/// this service provisioned and never one the caller created.
pub const EVS_SUBNET_TYPE: &str = "Elastic VMware Service";

/// One VLAN subnet of an environment, as the request described it: the `initialVlans`
/// member it came from (so a refusal names the member the caller sent) and the CIDR
/// block supplied for it.
#[derive(Debug, Clone, Copy)]
pub struct VlanSubnetRequest<'a> {
    /// The path of the `initialVlans` member this VLAN subnet came from.
    pub member: &'a str,
    /// The CIDR block the request supplied for it.
    pub cidr: &'a str,
}

/// One VLAN subnet as Amazon EC2 provisioned it: the id EC2 minted for it, the block it
/// stored, and the Availability Zone the subnet is in.
#[derive(Debug, Clone)]
pub struct ProvisionedVlanSubnet {
    /// The `SubnetId` EC2's own id minter gave the subnet.
    pub subnet_id: String,
    /// The IPv4 CIDR block EC2 stored for the subnet — the requested block in the
    /// canonical form EC2 holds one in, so the VLAN and the subnet report ONE block.
    pub cidr_block: String,
    /// The zone name of the subnet (`us-west-2a`), as EC2 holds it.
    pub availability_zone: String,
}

/// What came of asking Amazon EC2 to provision an environment's VLAN subnets.
#[derive(Debug)]
pub enum VlanSubnetProvisioning {
    /// EC2 created one subnet per requested VLAN, in the order they were asked for.
    Provisioned(Vec<ProvisionedVlanSubnet>),
    /// There is no readable EC2 store on this engine, so nothing was created and
    /// nothing is known: the caller falls back to ids of its own, exactly as the
    /// read-side functions above treat an unreadable store as evidence of nothing.
    NoEc2Store,
    /// No Availability Zone could be named for the environment's subnets, so NOTHING
    /// was created: a subnet of no zone is a record EC2 itself never produces, and the
    /// caller answers the server error EC2's own `CreateSubnet` answers for a Region
    /// with no zone rather than storing one.
    ZoneUnknown,
    /// The account's EC2 store cannot hold that many more subnets, so NOTHING was
    /// created: the environment's VLAN subnets are provisioned together or not at all.
    SubnetLimitExceeded {
        /// How many subnets the store already holds.
        held: usize,
        /// The ceiling it holds them under (EC2's own `MAX_SUBNETS`).
        limit: usize,
    },
}

/// The IPv4 CIDR blocks associated with one VPC: its primary block and every block a
/// later association added, which together are the address space a subnet of that VPC
/// has to come out of.
fn vpc_ipv4_blocks(vpc: &ec2_types::Vpc) -> Vec<String> {
    let mut blocks: Vec<String> = vpc.cidr_block.clone().into_iter().collect();
    for association in vpc.cidr_block_association_set.iter().flatten() {
        if let Some(block) = association.cidr_block.clone() {
            if !blocks.contains(&block) {
                blocks.push(block);
            }
        }
    }
    blocks
}

/// Every failure of the preconditions an environment's VLAN subnet blocks must meet
/// AGAINST THE VPC they are provisioned inside: each block is a block of the VPC's own
/// address space, and overlaps no subnet that VPC already holds ("Amazon EVS VLAN
/// subnet CIDR blocks must not overlap with other subnets in the VPC", `InitialVlans`
/// doc).
///
/// This is the same overlap rule `CreateEnvironment` applies among an environment's own
/// ten blocks, at the scope the doc states it: the whole VPC. It has to hold, because
/// each block becomes a real EC2 subnet of that VPC, and EC2 holds no two overlapping
/// subnets in one VPC.
///
/// An unknown VPC leaves nothing to judge the blocks against, so nothing is reported —
/// [`invalid_vpc_clause`] is the clause for that — and so does an unreadable EC2 store.
pub fn vlan_placement_failures<Eng: Send + Sync + 'static>(
    engine: &Eng,
    vpc_id: &str,
    planned: &[VlanSubnetRequest<'_>],
) -> Vec<ValidationExceptionField> {
    let Some(state) = ec2_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    let Some(vpc) = store.vpcs.get(vpc_id) else {
        return Vec::new();
    };
    let vpc_blocks = vpc_ipv4_blocks(vpc);
    let mut failures = Vec::new();
    for vlan in planned {
        // A block the subnet rules cannot read or size is already refused by the
        // operation's own CIDR clauses, so it is not judged a second time here.
        let Ok(canonical) = ec2::canonical_subnet_ipv4_cidr(vlan.cidr) else {
            continue;
        };
        if !vpc_blocks
            .iter()
            .any(|block| ec2::ipv4_cidr_block_contains(block, &canonical))
        {
            failures.push(super::field_failure(
                vlan.member,
                &format!(
                    "must be a block of the CIDR of VPC {vpc_id} ({blocks}), because \
                     Amazon EVS provisions the VLAN subnet inside that VPC",
                    blocks = vpc_blocks.join(", "),
                ),
            ));
            continue;
        }
        let conflicting = store
            .subnets
            .iter()
            .filter(|(_, subnet)| subnet.vpc_id.as_deref() == Some(vpc_id))
            .find(|(_, subnet)| {
                subnet
                    .cidr_block
                    .as_deref()
                    .is_some_and(|held| ec2::ipv4_cidr_blocks_overlap(held, &canonical))
            })
            .map(|(subnet_id, _)| subnet_id.clone());
        if let Some(conflicting) = conflicting {
            failures.push(super::field_failure(
                vlan.member,
                &format!("overlaps the CIDR block of subnet {conflicting} in VPC {vpc_id}"),
            ));
        }
    }
    failures
}

/// The single Availability Zone every subnet of the environment's VPC is in — its zone
/// name and that zone's id.
///
/// "Amazon EVS requires that all VPC subnets exist in a single Availability Zone in a
/// Region where the service is available" (`vpcId` doc), and the service access subnet
/// is one of those subnets, so ITS zone is the environment's zone and the VLAN subnets
/// are provisioned there. A service access subnet the store holds without a zone leaves
/// the Region's first zone, which is the zone `CreateSubnet` itself picks for a request
/// that names none; `None` when neither names one, which is no zone to put a subnet in.
fn environment_zone(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    region: &str,
    service_access_subnet_id: &str,
) -> Option<(String, String)> {
    let held = store
        .subnets
        .get(service_access_subnet_id)
        .and_then(|subnet| {
            let zone_name = subnet.availability_zone.clone()?;
            let zone_id = subnet
                .availability_zone_id
                .clone()
                .or_else(|| ec2::zone_id_for_zone_name(&zone_name))
                .unwrap_or_default();
            Some((zone_name, zone_id))
        });
    held.or_else(|| {
        ec2::availability_zones_in_region(region)
            .into_iter()
            .next()
            .map(|zone| (zone.zone_name, zone.zone_id))
    })
    .filter(|(zone_name, _)| !zone_name.is_empty())
}

/// How many subnets the account's EC2 store holds and the ceiling it holds them under,
/// when it cannot take `count` more — `None` when there is room (or no readable store,
/// which refuses nothing).
///
/// A READ, so a create can refuse on the EC2 subnet ceiling BEFORE it reserves anything
/// of its own: [`provision_vlan_subnets`] enforces the same ceiling, and writes nothing
/// when it is reached, but by then the create has already taken an environment slot.
pub fn subnet_room_shortfall<Eng: Send + Sync + 'static>(
    engine: &Eng,
    count: usize,
) -> Option<(usize, usize)> {
    let state = ec2_state(engine)?;
    let held = state.lock().subnets.len();
    (held + count > ec2::MAX_SUBNETS).then_some((held, ec2::MAX_SUBNETS))
}

/// Provision one EC2 subnet per requested VLAN in the account's own Amazon EC2 store,
/// inside `vpc_id`, and report what EC2 made of each.
///
/// "When you specify SELF_DEPLOYED for vcfVersion, Amazon EVS provisions only the VLAN
/// subnets" and provisions them in every other mode too, so these are real subnets of
/// the caller's VPC: EC2's own [`ec2::ComAmazonawsEc2Amazonec2State::insert_subnet`]
/// mints each id, ARN and available-address count, so `DescribeSubnets` reports a VLAN
/// subnet exactly as it reports one `CreateSubnet` created, and each carries the
/// [`EVS_SUBNET_TYPE`] that marks it as an Amazon EVS subnet.
///
/// Each subnet starts `pending` and the clock owes it `available` one EC2
/// `TRANSITION_DELAY` later, the same span on the same monotonic clock as a subnet
/// `CreateSubnet` created — EC2 owns its own resource's lifecycle, so the span is EC2's
/// and not this service's.
///
/// NOTHING is written when the store cannot hold them all, or when no Availability Zone
/// can be named for them: an environment's VLAN subnets are provisioned together or not
/// at all, and never as a subnet of no zone.
pub fn provision_vlan_subnets<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    vpc_id: &str,
    service_access_subnet_id: &str,
    planned: &[VlanSubnetRequest<'_>],
) -> VlanSubnetProvisioning {
    let Some(state) = ec2_state(engine) else {
        return VlanSubnetProvisioning::NoEc2Store;
    };
    let mut store = state.lock();
    let held = store.subnets.len();
    if held + planned.len() > ec2::MAX_SUBNETS {
        return VlanSubnetProvisioning::SubnetLimitExceeded {
            held,
            limit: ec2::MAX_SUBNETS,
        };
    }
    let Some((zone_name, zone_id)) = environment_zone(&store, region, service_access_subnet_id)
    else {
        return VlanSubnetProvisioning::ZoneUnknown;
    };
    let available_at = Instant::now() + ec2::TRANSITION_DELAY;
    let mut provisioned = Vec::with_capacity(planned.len());
    for vlan in planned {
        // "We modify the specified CIDR block to its canonical form"
        // (`CreateSubnetRequest$CidrBlock`): the subnet is stored the way EC2 stores
        // one. A block EC2 cannot canonicalise cannot reach here — the operation's own
        // CIDR clauses refuse it — so the request's own text is the honest fallback.
        let cidr_block = ec2::canonical_subnet_ipv4_cidr(vlan.cidr)
            .unwrap_or_else(|_| vlan.cidr.to_string());
        let subnet_id = store.insert_subnet(ec2::SubnetSpec {
            vpc_id: vpc_id.to_string(),
            cidr_block,
            availability_zone: zone_name.clone(),
            availability_zone_id: zone_id.clone(),
            region: region.to_string(),
            state: ec2::SUBNET_STATE_PENDING.to_string(),
            // Only a default VPC's own subnets are the default for their zone, and a
            // VLAN subnet does not auto-assign a public IPv4 address: a public VLAN is
            // reached through the Elastic IP addresses `AssociateEipToVlan` associates.
            default_for_az: false,
            map_public_ip_on_launch: false,
            // Tags "don't propagate to any other cluster or Amazon Web Services
            // resources" (`CreateEnvironmentRequest$tags` doc), so the VLAN subnets
            // carry none of the environment's.
            tags: Vec::new(),
        });
        if let Some(subnet) = store.subnets.get_mut(&subnet_id) {
            subnet.r#type = Some(EVS_SUBNET_TYPE.to_string());
        }
        store
            .subnet_available_at
            .insert(subnet_id.clone(), available_at);
        let cidr_block = store
            .subnets
            .get(&subnet_id)
            .and_then(|subnet| subnet.cidr_block.clone())
            .unwrap_or_else(|| vlan.cidr.to_string());
        provisioned.push(ProvisionedVlanSubnet {
            subnet_id,
            cidr_block,
            availability_zone: zone_name.clone(),
        });
    }
    VlanSubnetProvisioning::Provisioned(provisioned)
}

/// Remove the VLAN subnets `subnet_ids` names from the account's own Amazon EC2 store,
/// and report how many were removed.
///
/// "Environment deletion also deletes the associated Amazon EVS VLAN subnets"
/// (`DeleteEnvironment` doc): the subnets this service provisioned go away with the
/// environment, so the account is not left holding a subnet of an environment that no
/// longer exists — and the VPC's address space is free for the next environment.
///
/// Only a subnet EC2 reports as an Amazon EVS subnet ([`EVS_SUBNET_TYPE`]) is removed,
/// so an id that has come to name some other subnet of the account cannot delete it.
pub fn release_vlan_subnets<Eng: Send + Sync + 'static>(
    engine: &Eng,
    subnet_ids: &[String],
) -> usize {
    let Some(state) = ec2_state(engine) else {
        return 0;
    };
    let mut store = state.lock();
    let mut removed = 0;
    for subnet_id in subnet_ids {
        let is_evs_subnet = store
            .subnets
            .get(subnet_id)
            .and_then(|subnet| subnet.r#type.as_deref())
            == Some(EVS_SUBNET_TYPE);
        if is_evs_subnet && store.remove_subnet(subnet_id).is_some() {
            removed += 1;
        }
    }
    removed
}
