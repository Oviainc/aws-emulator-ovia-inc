// SPDX-License-Identifier: CC0-1.0

//! The EC2 resources a `CreateEnvironment` request may NAME, judged against the VPC the
//! environment is deployed into.
//!
//! The rule under test: every member of the request that names a resource of the caller's
//! existing network has to resolve in the account's own Amazon EC2 store AND belong to the
//! VPC the request names — so
//!
//! * each `initialVlans` CIDR block is a block of that VPC's own address space (its
//!   primary block or one associated with it later), because Amazon EVS provisions the
//!   VLAN subnet inside that VPC, and overlaps no subnet the VPC already holds: "Amazon
//!   EVS VLAN subnet CIDR blocks must not overlap with other subnets in the VPC"
//!   (`InitialVlans` / `InitialVlanInfo$cidr` docs), the service access subnet of the very
//!   same request included; and
//! * each `serviceAccessSecurityGroups.securityGroups` entry names a security group the
//!   account holds in that VPC: those groups "allow traffic between the Amazon EVS control
//!   plane and your VPC for Amazon EVS service access" (`ServiceAccessSecurityGroups`
//!   doc), Amazon EVS creates none of them, and a security group belongs to one VPC
//!   (`SecurityGroup$VpcId`).
//!
//! A request that breaks it is refused with the operation's `ValidationException`, whose
//! `fieldList` names the offending member, and creates nothing. The rule is VPC-SCOPED on
//! both halves, which is why every table carries a row whose block or group belongs to a
//! DIFFERENT VPC of the same account and is therefore no conflict at all.
//!
//! Each row writes the network it names into Amazon EC2's own store through EC2's own
//! writers (`insert_vpc`, `insert_subnet`, `insert_security_group`), so the VPCs, subnets
//! and groups under test are the records that service would hold, and the rows differ
//! along what the rule generalises over: the VPC's block, how many blocks it has, where
//! the VLAN block sits relative to them, which subnet it collides with, which VPC the
//! colliding subnet or the named group belongs to, and how many members offend at once.

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;

use crate::create_environment_validation::{assert_aggregated_refusal, environment_count};
use crate::environment_lifecycle::{
    call, emulator, error_type, vlans_with_prefix, PAST_THE_TRANSITION, REGION, ZONE,
};

/// The prefix length every VLAN block in these tables carries unless a row overrides the
/// member outright: the published minimum, which leaves the most room inside a VPC.
const VLAN_PREFIX_LEN: u8 = 28;

/// Write a VPC into the account's own Amazon EC2 store and answer its `VpcId`.
///
/// `secondary_cidrs` are blocks ASSOCIATED with the VPC after creation, which are part of
/// the address space a subnet of it may come out of ("Information about the IPv4 CIDR
/// blocks associated with the VPC", `Vpc$CidrBlockAssociationSet`).
fn register_vpc(
    engine: &engine::InMemoryEngine,
    primary_cidr: &str,
    secondary_cidrs: &[&str],
) -> String {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = ec2_state.lock();
    let vpc_id = store.insert_vpc(
        primary_cidr,
        false,
        ec2::state::DEFAULT_INSTANCE_TENANCY,
        ec2::state::VPC_STATE_AVAILABLE,
        REGION,
        Vec::new(),
    );
    for cidr in secondary_cidrs {
        let association_id = store.next_id("vpc-cidr-assoc");
        if let Some(associations) = store
            .vpcs
            .get_mut(&vpc_id)
            .and_then(|vpc| vpc.cidr_block_association_set.as_mut())
        {
            associations.push(ec2::ops::types::VpcCidrBlockAssociation {
                association_id: Some(association_id),
                cidr_block: Some((*cidr).to_string()),
                cidr_block_state: Some(ec2::ops::types::VpcCidrBlockState {
                    state: Some(ec2::state::CIDR_BLOCK_STATE_ASSOCIATED.to_string()),
                    status_message: None,
                }),
            });
        }
    }
    vpc_id
}

/// Write one subnet of `vpc_id` at `cidr` into that same store and answer its `SubnetId`.
fn register_subnet(engine: &engine::InMemoryEngine, vpc_id: &str, cidr: &str) -> String {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = ec2_state.lock();
    store.insert_subnet(ec2::state::SubnetSpec {
        vpc_id: vpc_id.to_string(),
        cidr_block: cidr.to_string(),
        availability_zone: ZONE.to_string(),
        availability_zone_id: ec2::state::zone_id_for_zone_name(ZONE).unwrap_or_default(),
        region: REGION.to_string(),
        state: ec2::state::SUBNET_STATE_AVAILABLE.to_string(),
        default_for_az: false,
        map_public_ip_on_launch: false,
        tags: Vec::new(),
    })
}

/// Write one security group of `vpc_id` into that same store and answer its `GroupId`.
fn register_security_group(
    engine: &engine::InMemoryEngine,
    vpc_id: &str,
    group_name: &str,
) -> String {
    let ec2_state = ec2::state::service_state(engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = ec2_state.lock();
    store.insert_security_group(ec2::state::SecurityGroupSpec {
        vpc_id: vpc_id.to_string(),
        group_name: group_name.to_string(),
        description: "service access".to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: Vec::new(),
        ip_permissions_egress: Vec::new(),
    })
}

/// The ten VLAN blocks of a request: the table's default spread, with `overrides`
/// replacing the block of the members it names.
fn vlans_overriding(overrides: &[(&str, &str)]) -> Value {
    let mut vlans = vlans_with_prefix(VLAN_PREFIX_LEN);
    for (member, cidr) in overrides {
        vlans[*member] = json!({ "cidr": cidr });
    }
    vlans
}

/// One row of the VLAN-placement table.
struct VlanRow {
    /// The case, named in every assertion.
    case: &'static str,
    /// The primary IPv4 CIDR block of the VPC the request names.
    vpc_cidr: &'static str,
    /// Further blocks associated with that VPC.
    secondary_cidrs: &'static [&'static str],
    /// The block of the service access subnet the request names, inside that VPC.
    service_access_cidr: &'static str,
    /// Blocks of further subnets that VPC already holds.
    other_subnets: &'static [&'static str],
    /// Blocks of subnets of a DIFFERENT VPC of the same account.
    foreign_subnets: &'static [&'static str],
    /// The `initialVlans` members whose block this row replaces.
    overrides: &'static [(&'static str, &'static str)],
    /// The `fieldList` members the refusal must name — empty for an accepted row.
    expected: &'static [&'static str],
}

#[tokio::test]
async fn a_vlan_block_is_a_block_of_its_vpc_that_overlaps_no_subnet_of_it() {
    // The default spread is 10.0.0.0/28, 10.0.2.0/28 … 10.0.18.0/28 (one per
    // `InitialVlans` member, from `vlans_with_prefix`), so a VPC of 10.0.0.0/16 with its
    // service access subnet high in the range holds every one of them.
    let rows: &[VlanRow] = &[
        VlanRow {
            case: "every VLAN block inside the VPC and clear of its subnets",
            vpc_cidr: "10.0.0.0/16",
            secondary_cidrs: &[],
            service_access_cidr: "10.0.240.0/24",
            other_subnets: &[],
            foreign_subnets: &[],
            overrides: &[],
            expected: &[],
        },
        VlanRow {
            // The reported case: ten blocks out of 10.0.0.0/24 against a VPC of
            // 172.31.0.0/16 — every one of them outside the VPC the subnets would be
            // provisioned in.
            case: "ten VLAN blocks in a range the VPC does not cover",
            vpc_cidr: "172.31.0.0/16",
            secondary_cidrs: &[],
            service_access_cidr: "172.31.0.0/20",
            other_subnets: &[],
            foreign_subnets: &[],
            overrides: &[],
            expected: &[
                "initialVlans.vmkManagement",
                "initialVlans.vmManagement",
                "initialVlans.vMotion",
                "initialVlans.vSan",
                "initialVlans.vTep",
                "initialVlans.edgeVTep",
                "initialVlans.nsxUplink",
                "initialVlans.hcx",
                "initialVlans.expansionVlan1",
                "initialVlans.expansionVlan2",
            ],
        },
        VlanRow {
            case: "a VLAN block that is exactly the service access subnet of the request",
            vpc_cidr: "10.0.0.0/16",
            secondary_cidrs: &[],
            service_access_cidr: "10.0.240.0/24",
            other_subnets: &[],
            foreign_subnets: &[],
            overrides: &[("hcx", "10.0.240.0/24")],
            expected: &["initialVlans.hcx"],
        },
        VlanRow {
            case: "a VLAN block inside the service access subnet of the request",
            vpc_cidr: "10.0.0.0/16",
            secondary_cidrs: &[],
            service_access_cidr: "10.0.240.0/24",
            other_subnets: &[],
            foreign_subnets: &[],
            overrides: &[("vTep", "10.0.240.16/28")],
            expected: &["initialVlans.vTep"],
        },
        VlanRow {
            case: "a VLAN block inside another subnet the VPC already holds",
            vpc_cidr: "10.0.0.0/16",
            secondary_cidrs: &[],
            service_access_cidr: "10.0.240.0/24",
            other_subnets: &["10.0.100.0/22"],
            foreign_subnets: &[],
            overrides: &[("vMotion", "10.0.101.0/28")],
            expected: &["initialVlans.vMotion"],
        },
        VlanRow {
            // The overlap the other way round: the VLAN block is the LARGER of the two
            // and contains a subnet the VPC already holds.
            case: "a VLAN block that contains a subnet the VPC holds",
            vpc_cidr: "10.0.0.0/16",
            secondary_cidrs: &[],
            service_access_cidr: "10.0.240.0/24",
            other_subnets: &["10.0.100.16/28"],
            overrides: &[("nsxUplink", "10.0.100.0/24")],
            foreign_subnets: &[],
            expected: &["initialVlans.nsxUplink"],
        },
        VlanRow {
            // A /20 covers 10.0.0.0 – 10.0.15.255, so the last two members of the
            // default spread (10.0.16.0/28 and 10.0.18.0/28) are just past its end: the
            // bound of the VPC's own block, and one step over it.
            case: "two VLAN blocks just past the end of a /20 VPC",
            vpc_cidr: "10.0.0.0/20",
            secondary_cidrs: &[],
            service_access_cidr: "10.0.15.0/24",
            other_subnets: &[],
            foreign_subnets: &[],
            overrides: &[],
            expected: &[
                "initialVlans.expansionVlan1",
                "initialVlans.expansionVlan2",
            ],
        },
        VlanRow {
            // The same VPC, with those two blocks brought inside a SECOND block
            // associated with it: a block of the VPC's address space either way.
            case: "VLAN blocks out of a secondary CIDR block associated with the VPC",
            vpc_cidr: "10.0.0.0/20",
            secondary_cidrs: &["10.7.0.0/16"],
            service_access_cidr: "10.0.15.0/24",
            other_subnets: &[],
            foreign_subnets: &[],
            overrides: &[
                ("expansionVlan1", "10.7.1.0/28"),
                ("expansionVlan2", "10.7.2.0/28"),
            ],
            expected: &[],
        },
        VlanRow {
            // The rule is scoped to the VPC the environment is deployed into, so a
            // subnet of some OTHER VPC of the same account is no conflict.
            case: "a VLAN block overlapping a subnet of a different VPC",
            vpc_cidr: "10.0.0.0/16",
            secondary_cidrs: &[],
            service_access_cidr: "10.0.240.0/24",
            other_subnets: &[],
            foreign_subnets: &["10.0.50.0/24"],
            overrides: &[("hcx", "10.0.50.0/28")],
            expected: &[],
        },
        VlanRow {
            // Both halves of the rule broken by different members of one request, which
            // is reported as one refusal naming both.
            case: "one VLAN block outside the VPC and one overlapping its subnet",
            vpc_cidr: "10.0.0.0/16",
            secondary_cidrs: &[],
            service_access_cidr: "10.0.240.0/24",
            other_subnets: &["10.0.100.0/24"],
            foreign_subnets: &[],
            overrides: &[("vSan", "192.168.5.0/28"), ("edgeVTep", "10.0.100.32/28")],
            expected: &["initialVlans.vSan", "initialVlans.edgeVTep"],
        },
    ];

    for row in rows {
        // A fresh emulator per row, so "nothing was created" is exact.
        let router = emulator();
        let vpc_id = register_vpc(&router.engine, row.vpc_cidr, row.secondary_cidrs);
        let service_access_subnet_id = register_subnet(&router.engine, &vpc_id, row.service_access_cidr);
        for cidr in row.other_subnets {
            register_subnet(&router.engine, &vpc_id, cidr);
        }
        if !row.foreign_subnets.is_empty() {
            let other_vpc = register_vpc(&router.engine, row.vpc_cidr, &[]);
            for cidr in row.foreign_subnets {
                register_subnet(&router.engine, &other_vpc, cidr);
            }
        }

        let request = json!({
            "vpcId": vpc_id,
            "serviceAccessSubnetId": service_access_subnet_id,
            "vcfVersion": "VCF-5.2.1",
            "termsAccepted": true,
            "initialVlans": vlans_overriding(row.overrides),
        });
        let (status, body) = call(&router, "CreateEnvironment", &request).await;

        if row.expected.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body),
                case = row.case,
            );
            // The blocks the request asked for are what the environment holds, read back
            // through `ListEnvironmentVlans` once the deployment the clock owes the new
            // environment has come due: an accepted row is accepted with its OWN blocks
            // and not with substitutes.
            let id = body["environment"]["environmentId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {:?}: no environmentId in {body}", row.case))
                .to_string();
            std::thread::sleep(PAST_THE_TRANSITION);
            let (status, listed) = call(
                &router,
                "ListEnvironmentVlans",
                &json!({ "environmentId": id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: ListEnvironmentVlans expected 200, got {status} {listed}",
                row.case
            );
            let vlans = listed["environmentVlans"]
                .as_array()
                .unwrap_or_else(|| panic!("case {:?}: no environmentVlans in {listed}", row.case))
                .clone();
            let requested = vlans_overriding(row.overrides);
            for (member, block) in requested
                .as_object()
                .expect("the VLAN plan is an object")
                .iter()
            {
                let cidr = block["cidr"].as_str().unwrap_or_default();
                assert!(
                    vlans
                        .iter()
                        .any(|vlan| vlan["cidr"].as_str() == Some(cidr)),
                    "case {case:?}: expected a VLAN reporting the {member} block {cidr:?}, \
                     got {vlans:?}",
                    case = row.case,
                );
            }
            continue;
        }

        assert_aggregated_refusal(row.case, status, &body, row.expected);
        assert_eq!(
            environment_count(&router).await,
            0,
            "case {:?}: a refused create must not persist an environment",
            row.case
        );
    }
}

/// One row of the security-group table: the groups the request names, written as the VPC
/// each one belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamedGroup {
    /// A group of the VPC the request is deployed into.
    OfThisVpc,
    /// A group of another VPC of the same account.
    OfAnotherVpc,
    /// A well-formed group id the account holds no security group for.
    Unknown,
}

/// One row of the security-group table.
struct GroupRow {
    /// The case, named in every assertion.
    case: &'static str,
    /// The `securityGroups` entries the request carries, at most two (the modelled
    /// maximum) — `None` for a request with no `serviceAccessSecurityGroups` member at
    /// all, which names no group to resolve.
    named: Option<&'static [NamedGroup]>,
    /// The `fieldList` members the refusal must name — empty for an accepted row.
    expected: &'static [&'static str],
}

#[tokio::test]
async fn a_service_access_security_group_is_a_group_of_the_environments_vpc() {
    let rows: &[GroupRow] = &[
        GroupRow {
            case: "no serviceAccessSecurityGroups member at all",
            named: None,
            expected: &[],
        },
        GroupRow {
            case: "an empty securityGroups list",
            named: Some(&[]),
            expected: &[],
        },
        GroupRow {
            case: "one group of the environment's own VPC",
            named: Some(&[NamedGroup::OfThisVpc]),
            expected: &[],
        },
        GroupRow {
            case: "two groups of the environment's own VPC",
            named: Some(&[NamedGroup::OfThisVpc, NamedGroup::OfThisVpc]),
            expected: &[],
        },
        GroupRow {
            case: "a group id the account holds no security group for",
            named: Some(&[NamedGroup::Unknown]),
            expected: &["serviceAccessSecurityGroups.securityGroups[0]"],
        },
        GroupRow {
            case: "a group of another VPC of the same account",
            named: Some(&[NamedGroup::OfAnotherVpc]),
            expected: &["serviceAccessSecurityGroups.securityGroups[0]"],
        },
        GroupRow {
            case: "one group of this VPC and one of another",
            named: Some(&[NamedGroup::OfThisVpc, NamedGroup::OfAnotherVpc]),
            expected: &["serviceAccessSecurityGroups.securityGroups[1]"],
        },
        GroupRow {
            case: "both entries name a group the account does not hold",
            named: Some(&[NamedGroup::Unknown, NamedGroup::Unknown]),
            expected: &[
                "serviceAccessSecurityGroups.securityGroups[0]",
                "serviceAccessSecurityGroups.securityGroups[1]",
            ],
        },
    ];

    for GroupRow {
        case,
        named,
        expected,
    } in rows
    {
        let router = emulator();
        let vpc_id = register_vpc(&router.engine, "10.0.0.0/16", &[]);
        let service_access_subnet_id = register_subnet(&router.engine, &vpc_id, "10.0.240.0/24");
        let other_vpc_id = register_vpc(&router.engine, "172.31.0.0/16", &[]);

        let mut request = json!({
            "vpcId": vpc_id,
            "serviceAccessSubnetId": service_access_subnet_id,
            "vcfVersion": "VCF-5.2.1",
            "termsAccepted": true,
            "initialVlans": vlans_with_prefix(VLAN_PREFIX_LEN),
        });
        if let Some(named) = named {
            let groups: Vec<String> = named
                .iter()
                .enumerate()
                .map(|(index, group)| match group {
                    NamedGroup::OfThisVpc => register_security_group(
                        &router.engine,
                        &vpc_id,
                        &format!("evs-access-{index}"),
                    ),
                    NamedGroup::OfAnotherVpc => register_security_group(
                        &router.engine,
                        &other_vpc_id,
                        &format!("elsewhere-{index}"),
                    ),
                    // Well formed (`sg-` and eight hex digits, as EC2's own minter
                    // produces) and held by nothing.
                    NamedGroup::Unknown => format!("sg-0000000{index}"),
                })
                .collect();
            request["serviceAccessSecurityGroups"] = json!({ "securityGroups": groups });
        }

        let (status, body) = call(&router, "CreateEnvironment", &request).await;
        if expected.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
            // Read the accepted groups back through `GetEnvironment`: what the create
            // accepted is what the stored environment reports.
            let id = body["environment"]["environmentId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
                .to_string();
            let (status, got) = call(&router, "GetEnvironment", &json!({ "environmentId": id })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: read back expected HTTP 200, got {status} {got}"
            );
            assert_eq!(
                got["environment"]["serviceAccessSecurityGroups"],
                request["serviceAccessSecurityGroups"],
                "case {case:?}: expected the stored groups to be the ones the request \
                 named ({}), got {}",
                request["serviceAccessSecurityGroups"],
                got["environment"]["serviceAccessSecurityGroups"]
            );
            continue;
        }

        assert_aggregated_refusal(case, status, &body, expected);
        assert_eq!(
            environment_count(&router).await,
            0,
            "case {case:?}: a refused create must not persist an environment"
        );
    }
}
