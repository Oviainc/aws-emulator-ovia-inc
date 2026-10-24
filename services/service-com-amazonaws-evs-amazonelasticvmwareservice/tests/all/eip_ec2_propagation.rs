// SPDX-License-Identifier: CC0-1.0

//! An Elastic IP address `AssociateEipToVlan` associates with a public HCX VLAN is a
//! REAL Amazon EC2 association of the account's own allocation.
//!
//! The rule under test, one level above the example: when an operation of one service
//! associates a resource ANOTHER service owns, the association is recorded on that other
//! service's own record — so every service reads one association — the resource is then
//! consumed and cannot be associated a second time while it stands, and the operation
//! that ends the association gives the resource back. For Amazon EVS: `allocationId` is
//! "The Elastic IP address allocation ID" of the caller's Amazon EC2 account and
//! `associationId` is an EC2 `eipassoc-` id of "the elastic IP address association with
//! the VLAN subnet" (`AssociateEipToVlan` input and `EipAssociation` docs), and an
//! Elastic IP address carries one association at a time — EC2's own
//! `Resource.AlreadyAssociated`. So:
//!
//! * a successful `AssociateEipToVlan` leaves `ec2 DescribeAddresses` reporting the
//!   allocation as associated, with exactly the `associationId` Amazon EVS reported, and
//!   the `ipAddress` it reported is the `publicIp` EC2 holds for the allocation;
//! * EC2's own `association-id` filter finds that allocation by that id, so the
//!   association is an EC2 association and not a name of Amazon EVS's own;
//! * a second association of the same allocation is REFUSED with the modelled
//!   `ValidationException` — whichever environment, and whichever Region, asks for it —
//!   and leaves both stores exactly as they were;
//! * every path that ends the association (`DisassociateEipFromVlan`, and
//!   `DeleteEnvironment`, which deletes the VLAN subnets the address was bound to)
//!   clears it on the EC2 record while the allocation STAYS allocated to the account, so
//!   it can be associated again.
//!
//! The rows differ along what the rule generalises over: how many allocations one VLAN
//! holds, which path ends the association, and which environment (and Region) asks for
//! the allocation a VLAN already has. The recorded case — one allocation associated with
//! one environment's HCX VLAN — is one row.
//!
//! Everything here runs over the wire against a router carrying BOTH services on one
//! engine ([`crate::vlan_subnet_propagation::emulator`]): the Elastic IP addresses are
//! allocated through Amazon EC2 itself and read back through `DescribeAddresses`, so
//! what the rows assert is that the two services read and write ONE store.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{call_in_region, error_type, PAST_THE_TRANSITION};
use crate::vlan_subnet_propagation::{call_ec2, element, elements, emulator};

/// The ten `InitialVlans` members, in the order the model declares them.
const VLAN_MEMBERS: [&str; 10] = [
    "vmkManagement",
    "vmManagement",
    "vMotion",
    "vSan",
    "vTep",
    "edgeVTep",
    "nsxUplink",
    "hcx",
    "expansionVlan1",
    "expansionVlan2",
];

/// One network an environment is deployed into: the Region it is addressed in, the zone
/// its subnets sit in, and the first two octets of its address space.
#[derive(Debug, Clone, Copy)]
struct Network {
    region: &'static str,
    zone: &'static str,
    base: &'static str,
}

/// An environment under test: the network it was deployed into and the id it was given.
#[derive(Debug, Clone)]
struct Environment {
    network: Network,
    environment_id: String,
}

/// Create `network`'s VPC and service access subnet through Amazon EC2 itself, then the
/// Amazon EVS environment deployed into it with a PUBLIC HCX VLAN, and answer it.
async fn create_environment(
    router: &MultiServiceRouter,
    network: Network,
    name: &str,
) -> Environment {
    let (status, xml) = call_ec2(
        router,
        network.region,
        "CreateVpc",
        &[("CidrBlock", &format!("{}.0.0/16", network.base))],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateVpc expected 200, got {status} {xml}"
    );
    let vpc_id =
        element(&xml, "vpcId").unwrap_or_else(|| panic!("case {name:?}: no vpcId in {xml}"));

    let (status, xml) = call_ec2(
        router,
        network.region,
        "CreateSubnet",
        &[
            ("VpcId", &vpc_id),
            ("CidrBlock", &format!("{}.200.0/24", network.base)),
            ("AvailabilityZone", network.zone),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateSubnet expected 200, got {status} {xml}"
    );
    let service_access_subnet_id =
        element(&xml, "subnetId").unwrap_or_else(|| panic!("case {name:?}: no subnetId in {xml}"));

    // Ten non-overlapping /28 blocks of the VPC, with the HCX VLAN public — the only
    // VLAN an Elastic IP address can be associated with, and only when the create asked
    // for it ("This operation is only allowed for public HCX VLANs at this time").
    let mut initial_vlans = json!({});
    for (index, member) in VLAN_MEMBERS.iter().enumerate() {
        initial_vlans[*member] = json!({ "cidr": format!("{}.{index}.0/28", network.base) });
    }
    initial_vlans["isHcxPublic"] = json!(true);
    // "Required when isHcxPublic is set to true" (`hcxNetworkAclId` doc).
    initial_vlans["hcxNetworkAclId"] = json!("acl-0123456789abcdef0");

    let (status, body) = call_in_region(
        router,
        network.region,
        "CreateEnvironment",
        &json!({
            "environmentName": name,
            "vpcId": vpc_id,
            "serviceAccessSubnetId": service_access_subnet_id,
            "vcfVersion": "SELF_DEPLOYED",
            "termsAccepted": true,
            "initialVlans": initial_vlans,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateEnvironment expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let environment_id = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no environmentId in {body}"))
        .to_string();
    // Wait out the transition the clock owes the new environment: it is CREATING — still
    // initializing — until then, and a list addressed to an environment in that state is
    // refused (`tests/all/list_while_initializing.rs`), while every row below reads the
    // HCX VLAN back through `ListEnvironmentVlans`.
    std::thread::sleep(PAST_THE_TRANSITION);
    Environment {
        network,
        environment_id,
    }
}

/// Allocate one Elastic IP address through Amazon EC2 itself, and answer
/// `(allocationId, publicIp)`.
async fn allocate_address(router: &MultiServiceRouter, region: &str) -> (String, String) {
    let (status, xml) = call_ec2(router, region, "AllocateAddress", &[("Domain", "vpc")]).await;
    assert_eq!(
        status, 200,
        "AllocateAddress: expected 200, got {status} {xml}"
    );
    let allocation_id =
        element(&xml, "allocationId").unwrap_or_else(|| panic!("no allocationId in {xml}"));
    let public_ip = element(&xml, "publicIp").unwrap_or_else(|| panic!("no publicIp in {xml}"));
    (allocation_id, public_ip)
}

/// What Amazon EC2 reports for one allocation through `DescribeAddresses`:
/// `(publicIp, associationId)`, the association id absent when the address carries none.
///
/// The account must still hold the allocation — an Elastic IP association is never a
/// release, so every caller here expects it to be found.
async fn ec2_address(
    router: &MultiServiceRouter,
    region: &str,
    allocation_id: &str,
) -> (String, Option<String>) {
    let (status, xml) = call_ec2(
        router,
        region,
        "DescribeAddresses",
        &[("AllocationId.1", allocation_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAddresses {allocation_id}: expected 200, got {status} {xml}"
    );
    let held = elements(&xml, "allocationId");
    assert_eq!(
        held,
        vec![allocation_id.to_string()],
        "DescribeAddresses {allocation_id}: expected the account to still hold exactly \
         that allocation, got {held:?} in {xml}"
    );
    let public_ip = element(&xml, "publicIp")
        .unwrap_or_else(|| panic!("DescribeAddresses {allocation_id}: no publicIp in {xml}"));
    (public_ip, element(&xml, "associationId"))
}

/// The allocation ids Amazon EC2 finds through its OWN `association-id` filter for
/// `association_id`.
async fn ec2_allocations_by_association(
    router: &MultiServiceRouter,
    region: &str,
    association_id: &str,
) -> Vec<String> {
    let (status, xml) = call_ec2(
        router,
        region,
        "DescribeAddresses",
        &[
            ("Filter.1.Name", "association-id"),
            ("Filter.1.Value.1", association_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAddresses by association {association_id}: expected 200, got {status} {xml}"
    );
    elements(&xml, "allocationId")
}

/// Associate `allocation_id` with `environment`'s HCX VLAN and answer the status, the
/// body, and the association the VLAN reports for that allocation.
async fn associate(
    router: &MultiServiceRouter,
    environment: &Environment,
    allocation_id: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        environment.network.region,
        "AssociateEipToVlan",
        &json!({
            "environmentId": environment.environment_id,
            "vlanName": "hcx",
            "allocationId": allocation_id,
        }),
    )
    .await
}

/// The `eipAssociations` of `environment`'s HCX VLAN, read back through
/// `ListEnvironmentVlans`.
async fn vlan_associations(router: &MultiServiceRouter, environment: &Environment) -> Vec<Value> {
    let (status, body) = call_in_region(
        router,
        environment.network.region,
        "ListEnvironmentVlans",
        &json!({ "environmentId": environment.environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListEnvironmentVlans: expected 200, got {status} {body}"
    );
    body["environmentVlans"]
        .as_array()
        .unwrap_or_else(|| panic!("ListEnvironmentVlans: no environmentVlans in {body}"))
        .iter()
        .find(|vlan| vlan["functionName"] == json!("hcx"))
        .and_then(|vlan| vlan["eipAssociations"].as_array().cloned())
        .unwrap_or_else(|| panic!("ListEnvironmentVlans: no hcx VLAN with associations in {body}"))
}

/// The association the response's VLAN carries for `allocation_id`.
fn reported_association(body: &Value, allocation_id: &str, case: &str) -> Value {
    body["vlan"]["eipAssociations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no eipAssociations in {body}"))
        .iter()
        .find(|association| association["allocationId"] == json!(allocation_id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no association for {allocation_id} in {body}"))
}

/// How a row ends the associations it made.
#[derive(Debug, Clone, Copy)]
enum Release {
    /// `DisassociateEipFromVlan`, one call per association.
    Disassociate,
    /// `DeleteEnvironment`: the VLAN subnets the addresses were bound to go away with
    /// the environment, so the associations do too.
    DeleteEnvironment,
}

#[tokio::test]
async fn an_association_consumes_the_ec2_allocation_and_every_release_path_gives_it_back() {
    // Each row: the case, how many allocations the VLAN holds, and the path that ends
    // them. The count and the release path are what the rule generalises over.
    let rows: Vec<(&str, usize, Release)> = vec![
        (
            "one allocation, released with DisassociateEipFromVlan",
            1,
            Release::Disassociate,
        ),
        (
            "three allocations on one VLAN, released with DisassociateEipFromVlan",
            3,
            Release::Disassociate,
        ),
        (
            "two allocations on one VLAN, released by deleting the environment",
            2,
            Release::DeleteEnvironment,
        ),
    ];

    for (case, count, release) in rows {
        // A fresh emulator per row, so what EC2 holds is exactly what this row made.
        let router = emulator();
        let holder = create_environment(
            &router,
            Network {
                region: "us-west-2",
                zone: "us-west-2a",
                base: "10.0",
            },
            "eip-holder",
        )
        .await;
        let other = create_environment(
            &router,
            Network {
                region: "us-west-2",
                zone: "us-west-2b",
                base: "10.1",
            },
            "eip-other",
        )
        .await;
        let region = holder.network.region;

        let mut allocations: Vec<(String, String, String)> = Vec::new();
        for index in 0..count {
            let (allocation_id, public_ip) = allocate_address(&router, region).await;
            let (_, before) = ec2_address(&router, region, &allocation_id).await;
            assert_eq!(
                before, None,
                "case {case:?}: allocation {index} must start unassociated, got {before:?}"
            );

            let (status, body) = associate(&router, &holder, &allocation_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: associate {index} expected 200, got {status} ({}) {body}",
                error_type(&body)
            );
            let association = reported_association(&body, &allocation_id, case);
            let association_id = association["associationId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no associationId in {association}"))
                .to_string();
            assert_eq!(
                association["ipAddress"],
                json!(public_ip),
                "case {case:?}: allocation {index} must report the address EC2 holds \
                 ({public_ip}), got {}",
                association["ipAddress"]
            );

            // THE POINT: Amazon EC2's own record of the allocation now carries exactly
            // the association Amazon EVS reported.
            let (held_ip, held_association) = ec2_address(&router, region, &allocation_id).await;
            assert_eq!(
                held_ip, public_ip,
                "case {case:?}: allocation {index} must keep its address, got {held_ip}"
            );
            assert_eq!(
                held_association.as_deref(),
                Some(association_id.as_str()),
                "case {case:?}: allocation {index} must be associated in EC2 with \
                 {association_id}, got {held_association:?}"
            );
            // And it is an EC2 association: EC2's own filter finds it by that id.
            let found = ec2_allocations_by_association(&router, region, &association_id).await;
            assert_eq!(
                found,
                vec![allocation_id.clone()],
                "case {case:?}: EC2's association-id filter must find {allocation_id} for \
                 {association_id}, got {found:?}"
            );
            allocations.push((allocation_id, public_ip, association_id));
        }

        // Consumed: no other environment can take an allocation this VLAN holds, and the
        // refusal leaves both stores as they were.
        let (first_allocation, first_ip, first_association) = allocations[0].clone();
        let (status, body) = associate(&router, &other, &first_allocation).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second environment taking {first_allocation} expected HTTP \
             400, got {status} {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {} in {body}",
            error_type(&body)
        );
        let (held_ip, held_association) = ec2_address(&router, region, &first_allocation).await;
        assert_eq!(
            (held_ip.as_str(), held_association.as_deref()),
            (first_ip.as_str(), Some(first_association.as_str())),
            "case {case:?}: the refused associate must leave the EC2 record as it was"
        );
        let left = vlan_associations(&router, &other).await;
        assert!(
            left.is_empty(),
            "case {case:?}: the refused associate must store nothing on the other VLAN, \
             got {left:?}"
        );

        // Released — and the allocation is still the account's, so it can be used again.
        match release {
            Release::Disassociate => {
                for (index, (_, _, association_id)) in allocations.iter().enumerate() {
                    let (status, body) = call_in_region(
                        &router,
                        region,
                        "DisassociateEipFromVlan",
                        &json!({
                            "environmentId": holder.environment_id,
                            "vlanName": "hcx",
                            "associationId": association_id,
                        }),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: disassociate {index} expected 200, got {status} \
                         ({}) {body}",
                        error_type(&body)
                    );
                }
            }
            Release::DeleteEnvironment => {
                let (status, body) = call_in_region(
                    &router,
                    region,
                    "DeleteEnvironment",
                    &json!({ "environmentId": holder.environment_id }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteEnvironment expected 200, got {status} ({}) {body}",
                    error_type(&body)
                );
            }
        }

        for (index, (allocation_id, public_ip, association_id)) in allocations.iter().enumerate() {
            let (held_ip, held_association) = ec2_address(&router, region, allocation_id).await;
            assert_eq!(
                held_ip, *public_ip,
                "case {case:?}: allocation {index} stays allocated with its address after \
                 {release:?}, got {held_ip}"
            );
            assert_eq!(
                held_association, None,
                "case {case:?}: allocation {index} must be unassociated in EC2 after \
                 {release:?}, got {held_association:?}"
            );
            let found = ec2_allocations_by_association(&router, region, association_id).await;
            assert!(
                found.is_empty(),
                "case {case:?}: no address may still answer to the released association \
                 {association_id}, got {found:?}"
            );
        }

        // The released allocation is associable again — by the environment that was
        // refused it while it was in use.
        let (status, body) = associate(&router, &other, &first_allocation).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the released {first_allocation} expected 200, got {status} \
             ({}) {body}",
            error_type(&body)
        );
        let association = reported_association(&body, &first_allocation, case);
        let association_id = association["associationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no associationId in {association}"))
            .to_string();
        assert_ne!(
            association_id, first_association,
            "case {case:?}: a new association is a new id, got {association_id}"
        );
        let (_, held_association) = ec2_address(&router, region, &first_allocation).await;
        assert_eq!(
            held_association.as_deref(),
            Some(association_id.as_str()),
            "case {case:?}: EC2 must now report the new association, got {held_association:?}"
        );
    }
}

#[tokio::test]
async fn one_elastic_ip_address_carries_one_association_whoever_asks_for_it() {
    // One allocation, and four claimants that differ along what the rule generalises
    // over: the SAME VLAN (which is the `@idempotent` repeat, not a second association),
    // another environment of the same Region, another environment of another Region, and
    // the same VLAN once more at the end — so the rows also assert that a refusal
    // changed nothing for the holder.
    let router = emulator();
    let holder = create_environment(
        &router,
        Network {
            region: "us-west-2",
            zone: "us-west-2a",
            base: "10.0",
        },
        "eip-single-use",
    )
    .await;
    let same_region = create_environment(
        &router,
        Network {
            region: "us-west-2",
            zone: "us-west-2c",
            base: "10.1",
        },
        "eip-same-region",
    )
    .await;
    let other_region = create_environment(
        &router,
        Network {
            region: "eu-west-1",
            zone: "eu-west-1a",
            base: "192.168",
        },
        "eip-other-region",
    )
    .await;

    let region = holder.network.region;
    let (allocation_id, public_ip) = allocate_address(&router, region).await;
    let (status, body) = associate(&router, &holder, &allocation_id).await;
    assert_eq!(
        status, 200,
        "the first associate expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let first_association = reported_association(&body, &allocation_id, "the first associate")
        ["associationId"]
        .as_str()
        .unwrap_or_else(|| panic!("the first associate: no associationId in {body}"))
        .to_string();

    // Each row: the case, the environment asking, and whether the call is accepted.
    let rows: Vec<(&str, &Environment, bool)> = vec![
        ("the VLAN that already holds it", &holder, true),
        (
            "the HCX VLAN of another environment in the same Region",
            &same_region,
            false,
        ),
        (
            "the HCX VLAN of an environment in another Region",
            &other_region,
            false,
        ),
        ("the holder once more, after the refusals", &holder, true),
    ];

    for (case, claimant, accepted) in rows {
        let (status, body) = associate(&router, claimant, &allocation_id).await;
        if accepted {
            // `@idempotent`: the holder is told the association it already has, and
            // nothing is added on either side.
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
            let association = reported_association(&body, &allocation_id, case);
            assert_eq!(
                association["associationId"],
                json!(first_association),
                "case {case:?}: expected the association it already had, got {}",
                association["associationId"]
            );
            let held = vlan_associations(&router, claimant).await;
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: expected exactly one stored association, got {held:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert!(
                error_type(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {} in {body}",
                error_type(&body)
            );
            let left = vlan_associations(&router, claimant).await;
            assert!(
                left.is_empty(),
                "case {case:?}: a refused associate stores nothing, got {left:?}"
            );
        }
        // Whatever the row asked, EC2 still reports ONE association for the allocation:
        // the holder's, with the address it was allocated.
        let (held_ip, held_association) = ec2_address(&router, region, &allocation_id).await;
        assert_eq!(
            (held_ip.as_str(), held_association.as_deref()),
            (public_ip.as_str(), Some(first_association.as_str())),
            "case {case:?}: expected EC2 to still report the holder's association"
        );
    }
}
