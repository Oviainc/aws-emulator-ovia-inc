// SPDX-License-Identifier: CC0-1.0

//! ONE reading of a VLAN subnet CIDR block, over awsJson1_0.
//!
//! The rule under test: a VLAN subnet CIDR block is read ONCE — as the address it names
//! with the host bits below its prefix length cleared — and every judgement this service
//! makes about that block is made on that single reading:
//!
//! * its size, which must be between /24 and /28 ("The CIDR block size must be between
//!   /24 and /28", `InitialVlans` member docs);
//! * whether it overlaps another VLAN subnet OF THE SAME environment ("Amazon EVS VLAN
//!   subnet CIDR blocks must not overlap with other subnets", `InitialVlans` doc);
//! * whether it is a block of the VPC the environment is deployed into, and whether it
//!   overlaps a subnet that VPC already holds (the same doc sentence, at the VPC's
//!   scope) — judged against the account's own Amazon EC2 store;
//! * which of its addresses an ESX host may be given, which is the block minus the four
//!   addresses AWS reserves at its start and the one at its end (Amazon VPC User Guide,
//!   subnet sizing).
//!
//! Those judgements used to be reached through two separate CIDR engines in this crate —
//! one for an environment's own ten blocks and one for the VPC's address space — so this
//! suite drives them through the operations each of them served: `CreateEnvironment`, for
//! the size and the two overlap scopes, and `CreateEnvironmentHost` plus
//! `ListEnvironmentVlans`, for the addresses the one reading leaves usable and the block
//! the environment reports. Each row spells its blocks differently — canonical and named
//! by an address inside the block, at the published minimum and maximum size and one past
//! each, at bases that are not `.0` — so what is asserted is the reading and not one
//! request.

use serde_json::{json, Value};

use crate::create_environment_validation::{assert_aggregated_refusal, environment_count};
use crate::environment_lifecycle::{call, error_type, settle, vlans_with_prefix, Emulator};

/// The ten VLAN subnet blocks of a valid create, with `overrides` replacing the block of
/// the `initialVlans` members it names.
fn vlans_with(overrides: &[(&str, &str)]) -> Value {
    let mut vlans = vlans_with_prefix(28);
    for (member, cidr) in overrides {
        vlans[*member] = json!({ "cidr": cidr });
    }
    vlans
}

/// One case: how the request spells its VLAN subnet blocks, and what the one reading of
/// those blocks makes of them.
struct Row {
    /// What the case is, named in every assertion of it.
    case: &'static str,
    /// The `initialVlans` the create carries.
    vlans: Value,
    /// The input members the refusal must name — empty for a row that is served.
    refused_members: Vec<&'static str>,
    /// For a served row, the block `ListEnvironmentVlans` must report for
    /// `vmkManagement` and the management address its first ESX host must take.
    served: Option<(&'static str, &'static str)>,
}

#[tokio::test]
async fn one_reading_of_a_vlan_subnet_block_serves_every_judgement_about_it() {
    let rows: Vec<Row> = vec![
        Row {
            case: "ten canonical /28 blocks are served, and the first host takes the \
                   fifth address of the vmkManagement block",
            vlans: vlans_with(&[]),
            refused_members: vec![],
            served: Some(("10.0.0.0/28", "10.0.0.4")),
        },
        Row {
            // /24 is the largest block an `InitialVlans` member may carry.
            case: "ten /24 blocks are served at the published maximum size",
            vlans: vlans_with_prefix(24),
            refused_members: vec![],
            served: Some(("10.0.0.0/24", "10.0.0.4")),
        },
        Row {
            // The same /28 range as the first row, named by an address inside it: one
            // reading means the block reported, the overlap judged and the host address
            // drawn are those of the RANGE, not of the spelling.
            case: "a block named by an address inside it is read as the range that \
                   address falls in",
            vlans: vlans_with(&[("vmkManagement", "10.0.0.9/28")]),
            refused_members: vec![],
            served: Some(("10.0.0.0/28", "10.0.0.4")),
        },
        Row {
            // A /27 whose range starts at .32, named by an address inside it: the usable
            // window is four addresses into the RANGE, so the host takes .36.
            case: "a /27 block named by an address inside it addresses its host four \
                   addresses into the range",
            vlans: vlans_with(&[("vmkManagement", "10.0.32.33/27")]),
            refused_members: vec![],
            served: Some(("10.0.32.32/27", "10.0.32.36")),
        },
        Row {
            case: "a /28 block whose base is not a multiple of 256 addresses its host \
                   four addresses into it",
            vlans: vlans_with(&[("vmkManagement", "10.0.200.16/28")]),
            refused_members: vec![],
            served: Some(("10.0.200.16/28", "10.0.200.20")),
        },
        Row {
            // The /28 falls inside the /24 of another member of the same request, and is
            // named by an address inside itself: both blocks are compared as ranges.
            case: "a block nested in another VLAN subnet of the same environment \
                   overlaps it",
            vlans: vlans_with(&[
                ("vmkManagement", "10.0.0.0/24"),
                ("vmManagement", "10.0.0.33/28"),
            ]),
            refused_members: vec!["initialVlans.vmManagement"],
            served: None,
        },
        Row {
            case: "a /23 block is one past the published /24 maximum",
            vlans: vlans_with(&[("vmkManagement", "10.0.20.0/23")]),
            refused_members: vec!["initialVlans.vmkManagement"],
            served: None,
        },
        Row {
            case: "a /29 block is one past the published /28 minimum",
            vlans: vlans_with(&[("vmkManagement", "10.0.20.0/29")]),
            refused_members: vec!["initialVlans.vmkManagement"],
            served: None,
        },
        Row {
            // The VPC the harness registers is 10.0.0.0/16, so a 10.9 block is no part
            // of the address space the VLAN subnet would be provisioned inside.
            case: "a block outside the VPC's own address space is refused",
            vlans: vlans_with(&[("vmkManagement", "10.9.0.0/28")]),
            refused_members: vec!["initialVlans.vmkManagement"],
            served: None,
        },
        Row {
            // Inside the service access subnet the harness registers (10.0.240.0/24),
            // and named by an address inside its own /28: the VPC-scoped overlap is
            // judged on the range too.
            case: "a block that overlaps a subnet the VPC already holds is refused",
            vlans: vlans_with(&[("vmkManagement", "10.0.240.20/28")]),
            refused_members: vec!["initialVlans.vmkManagement"],
            served: None,
        },
    ];

    for row in rows {
        let case = row.case;
        // A fresh emulator per row, so "nothing was created" is exact and every row
        // starts from the same registered network.
        let router = crate::environment_lifecycle::emulator();
        let mut request = router.create_request(Some("cidr-reading"));
        request["initialVlans"] = row.vlans.clone();

        let (status, body) = call(&router, "CreateEnvironment", &request).await;
        let Some((expected_block, expected_address)) = row.served else {
            assert_aggregated_refusal(case, status, &body, &row.refused_members);
            assert_eq!(
                environment_count(&router).await,
                0,
                "case {case:?}: a refused create must create nothing"
            );
            continue;
        };

        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let environment = body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
            .to_string();

        // The environment is CREATING until the deployment the clock owes it comes due,
        // and a host joins a deployed environment.
        settle(&router);

        let reported = vmk_management_block(&router, case, &environment).await;
        assert_eq!(
            reported,
            json!(expected_block),
            "case {case:?}: expected the vmkManagement VLAN to report the block its \
             spelling names ({expected_block}), got {reported}"
        );

        let (status, body) = call(
            &router,
            "CreateEnvironmentHost",
            &json!({
                "environmentId": environment,
                "host": {
                    "hostName": "esx-1",
                    "keyName": "evs-key",
                    "instanceType": "i4i.metal",
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEnvironmentHost expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        assert_eq!(
            body["host"]["ipAddress"],
            json!(expected_address),
            "case {case:?}: expected the first usable address of the vmkManagement \
             block ({expected_address}), got {}",
            body["host"]["ipAddress"]
        );

        // Read back through the list, so the address is the PERSISTED one and not just
        // what the create answered.
        let (status, listed) = call(
            &router,
            "ListEnvironmentHosts",
            &json!({ "environmentId": environment }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListEnvironmentHosts expected HTTP 200, got {status} {listed}"
        );
        let addresses: Vec<&str> = listed["environmentHosts"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentHosts in {listed}"))
            .iter()
            .filter_map(|host| host["ipAddress"].as_str())
            .collect();
        assert_eq!(
            addresses,
            vec![expected_address],
            "case {case:?}: expected the stored host to hold {expected_address}, got \
             {addresses:?}"
        );
    }
}

/// The `cidr` `ListEnvironmentVlans` reports for the environment's `vmkManagement` VLAN
/// subnet.
async fn vmk_management_block(router: &Emulator, case: &str, environment: &str) -> Value {
    let (status, listed) = call(
        router,
        "ListEnvironmentVlans",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListEnvironmentVlans expected HTTP 200, got {status} {listed}"
    );
    listed["environmentVlans"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentVlans in {listed}"))
        .iter()
        .find(|vlan| vlan["functionName"] == json!("vmkManagement"))
        .unwrap_or_else(|| panic!("case {case:?}: no vmkManagement VLAN in {listed}"))["cidr"]
        .clone()
}
