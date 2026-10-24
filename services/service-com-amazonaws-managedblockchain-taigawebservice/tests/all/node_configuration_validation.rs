// SPDX-License-Identifier: CC0-1.0

//! `CreateNode` refuses a `NodeConfiguration` that names no node the service could
//! provision, and creates nothing when it does.
//!
//! Two rules are asserted over tables, not over one recorded request:
//!
//! * `NodeConfiguration$InstanceType` is "The Amazon Managed Blockchain instance type for
//!   the node", so the type must be one Managed Blockchain offers for the network's
//!   framework and edition — the `bc.*` catalog below, narrowed to the two smallest
//!   burstable sizes on a Hyperledger Fabric Starter Edition network.
//! * `NodeConfiguration$AvailabilityZone` is "The Availability Zone in which the node
//!   exists. Required for Ethereum nodes.", so a node on a framework that requires one
//!   must name it, while a framework that does not may omit it.
//!
//! Every refusal is checked to be inert: the node count the network reports does not move,
//! and the `ClientRequestToken` the refused call carried is still free to mint a node.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    available_network, call, create_network, create_network_body, is_resource_id, new_router,
    PAST_TRANSITION, REGION,
};
use crate::network_and_node_lifecycle::Answer;

/// The instance types Amazon Managed Blockchain offers.
///
/// Catalog source: `NodeConfiguration$InstanceType` ("The Amazon Managed Blockchain
/// instance type for the node") is typed as the unconstrained `InstanceTypeString`
/// (smithy_query: shape InstanceTypeString = string, no constraints), so the catalog is
/// the one Managed Blockchain publishes: its own `bc.t3`, `bc.m5` and `bc.c5` families.
/// Written out here rather than read from the crate, so the test is evidence about the
/// catalog and not a restatement of it.
const CATALOG: &[&str] = &[
    "bc.t3.small",
    "bc.t3.medium",
    "bc.t3.large",
    "bc.t3.xlarge",
    "bc.m5.large",
    "bc.m5.xlarge",
    "bc.m5.2xlarge",
    "bc.m5.4xlarge",
    "bc.c5.large",
    "bc.c5.xlarge",
    "bc.c5.2xlarge",
    "bc.c5.4xlarge",
];

/// The subset a Hyperledger Fabric **Starter Edition** network runs (Managed Blockchain
/// Pricing, which `NetworkFabricConfiguration$Edition` points at).
const STARTER_CATALOG: &[&str] = &["bc.t3.small", "bc.t3.medium"];

/// Instance types Managed Blockchain does not offer: an EC2 type without the `bc.`
/// prefix, a `bc.*` family that does not exist, sizes the published families stop short
/// of, a differently cased catalog entry, and the empty string.
const NOT_OFFERED: &[&str] = &[
    "m5.large",
    "t3.small",
    "bc.nonexistent",
    "bc.t3.2xlarge",
    "bc.c5.9xlarge",
    "bc.m5.24xlarge",
    "BC.T3.SMALL",
    "bc.t3.small ",
    "",
];

/// The ids of the nodes a network reports, through `ListNodes`.
async fn listed_nodes(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: Option<&str>,
) -> Vec<String> {
    let path = match member_id {
        Some(member_id) => format!("/networks/{network_id}/nodes?memberId={member_id}"),
        None => format!("/networks/{network_id}/nodes"),
    };
    let answer = call(router, "GET", &path, None).await;
    assert_eq!(
        answer.status, 200,
        "case ListNodes on {network_id}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["Nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| node["Id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Send one `CreateNode` and return what the emulator answered.
async fn create_node(
    router: &MultiServiceRouter,
    network_id: &str,
    token: &str,
    member_id: Option<&str>,
    configuration: Value,
) -> Answer {
    let mut body = json!({
        "ClientRequestToken": token,
        "NodeConfiguration": configuration,
    });
    if let Some(member_id) = member_id {
        body["MemberId"] = Value::String(member_id.to_string());
    }
    call(
        router,
        "POST",
        &format!("/networks/{network_id}/nodes"),
        Some(body),
    )
    .await
}

/// Create one Hyperledger Fabric network of `edition` that has already settled to
/// `AVAILABLE`, and return `(NetworkId, MemberId)`.
async fn available_network_of_edition(
    router: &MultiServiceRouter,
    token: &str,
    name: &str,
    edition: &str,
) -> (String, String) {
    let mut body = create_network_body(token, name);
    body["FrameworkConfiguration"] = json!({ "Fabric": { "Edition": edition } });
    let ids = create_network(router, body).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    ids
}

/// Every catalog entry is accepted on a Standard Edition network and persisted, every
/// entry outside the catalog is refused with a 400 `InvalidRequestException`, and a
/// Starter Edition network accepts only the two sizes that edition runs.
#[tokio::test]
async fn create_node_accepts_only_the_instance_types_the_edition_offers() {
    let router = new_router();
    let (standard_id, standard_member) =
        available_network_of_edition(&router, "edition-standard", "standard-net", "STANDARD").await;
    let (starter_id, starter_member) =
        available_network_of_edition(&router, "edition-starter", "starter-net", "STARTER").await;

    // Rows: every published instance type, plus every type the service does not offer.
    // `accepted_on_starter` is what the edition narrows the catalog to.
    let rows: Vec<(&str, bool, bool)> = CATALOG
        .iter()
        .map(|instance_type| {
            (
                *instance_type,
                true,
                STARTER_CATALOG.contains(instance_type),
            )
        })
        .chain(
            NOT_OFFERED
                .iter()
                .map(|instance_type| (*instance_type, false, false)),
        )
        .collect();

    let mut standard_expected: Vec<String> = Vec::new();
    let mut starter_expected: Vec<String> = Vec::new();
    for (case, (instance_type, on_standard, on_starter)) in rows.iter().enumerate() {
        for (edition, network_id, member_id, accepted, expected) in [
            (
                "STANDARD",
                &standard_id,
                &standard_member,
                *on_standard,
                &mut standard_expected,
            ),
            (
                "STARTER",
                &starter_id,
                &starter_member,
                *on_starter,
                &mut starter_expected,
            ),
        ] {
            let answer = create_node(
                &router,
                network_id,
                &format!("instance-{case}-{edition}"),
                Some(member_id),
                json!({
                    "InstanceType": instance_type,
                    "AvailabilityZone": format!("{REGION}a"),
                }),
            )
            .await;
            if accepted {
                assert_eq!(
                    answer.status, 200,
                    "case {instance_type:?} on {edition}: expected CreateNode 200, got {} {:?}",
                    answer.status, answer.body
                );
                let node_id = answer.text(&["NodeId"]);
                assert!(
                    is_resource_id(&node_id, "nd"),
                    "case {instance_type:?} on {edition}: expected a minted node id, got \
                     {node_id:?}"
                );
                expected.push(node_id.clone());

                // The accepted type round-trips through the stored record.
                let answer = call(
                    &router,
                    "GET",
                    &format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
                    None,
                )
                .await;
                assert_eq!(
                    answer.text(&["Node", "InstanceType"]),
                    *instance_type,
                    "case {instance_type:?} on {edition}: expected GetNode to report the stored \
                     instance type, got {:?}",
                    answer.body
                );
            } else {
                assert_eq!(
                    (answer.status, answer.error_type.as_str()),
                    (400, "InvalidRequestException"),
                    "case {instance_type:?} on {edition}: expected 400 InvalidRequestException, \
                     got {} {:?}",
                    answer.status,
                    answer.body
                );
            }
        }
    }

    // Nothing a refusal touched exists: each network holds exactly the nodes its edition
    // accepted.
    for (edition, network_id, member_id, mut expected) in [
        ("STANDARD", &standard_id, &standard_member, standard_expected),
        ("STARTER", &starter_id, &starter_member, starter_expected),
    ] {
        expected.sort();
        assert_eq!(
            listed_nodes(&router, network_id, Some(member_id)).await,
            expected,
            "case {edition}: expected only the accepted nodes to exist"
        );
    }
}

/// One row of the Availability Zone table: what the request names for the zone, and
/// whether the node's framework lets the node exist without it.
#[derive(Debug)]
struct ZoneCase {
    zone: Option<&'static str>,
    accepted_on_ethereum: bool,
}

/// `AvailabilityZone` is required on an Ethereum network and optional on a Hyperledger
/// Fabric one, and an accepted zone is the one every later read reports.
#[tokio::test]
async fn an_ethereum_node_requires_an_availability_zone() {
    let router = new_router();
    let (fabric_id, fabric_member) = available_network(&router, "zone-fabric", "zone-net").await;

    // Rows: the zone omitted, named empty, named blank, and two different real zone
    // names — so acceptance turns on the zone being named, not on which zone it is.
    let cases: &[ZoneCase] = &[
        ZoneCase {
            zone: None,
            accepted_on_ethereum: false,
        },
        ZoneCase {
            zone: Some(""),
            accepted_on_ethereum: false,
        },
        ZoneCase {
            zone: Some("   "),
            accepted_on_ethereum: false,
        },
        ZoneCase {
            zone: Some("eu-west-1a"),
            accepted_on_ethereum: true,
        },
        ZoneCase {
            zone: Some("eu-west-1c"),
            accepted_on_ethereum: true,
        },
    ];

    // The AWS-owned public Ethereum networks (`NetworkId`s `CreateNode` documents), both
    // of which an Ethereum node is placed on the same way.
    for network_id in ["n-ethereum-mainnet", "n-ethereum-goerli"] {
        let mut expected: Vec<String> = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            let mut configuration = json!({ "InstanceType": "bc.m5.xlarge" });
            if let Some(zone) = case.zone {
                configuration["AvailabilityZone"] = Value::String(zone.to_string());
            }
            let token = format!("zone-{network_id}-{index}");
            let answer = create_node(&router, network_id, &token, None, configuration).await;
            if case.accepted_on_ethereum {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} on {network_id}: expected CreateNode 200, got {} {:?}",
                    answer.status, answer.body
                );
                let node_id = answer.text(&["NodeId"]);
                expected.push(node_id.clone());
                let answer = call(
                    &router,
                    "GET",
                    &format!("/networks/{network_id}/nodes/{node_id}"),
                    None,
                )
                .await;
                assert_eq!(
                    answer.body["Node"]["AvailabilityZone"].as_str(),
                    case.zone,
                    "case {case:?} on {network_id}: expected GetNode to report the stored zone, \
                     got {:?}",
                    answer.body
                );
            } else {
                assert_eq!(
                    (answer.status, answer.error_type.as_str()),
                    (400, "InvalidRequestException"),
                    "case {case:?} on {network_id}: expected 400 InvalidRequestException, got \
                     {} {:?}",
                    answer.status,
                    answer.body
                );

                // A refused create minted nothing, and left its token free: the same
                // token now names a node, which it could not if the refusal had consumed
                // it.
                let answer = create_node(
                    &router,
                    network_id,
                    &token,
                    None,
                    json!({
                        "InstanceType": "bc.m5.xlarge",
                        "AvailabilityZone": format!("{REGION}b"),
                    }),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} on {network_id}: expected the token of a refused create to \
                     still mint a node, got {} {:?}",
                    answer.status, answer.body
                );
                expected.push(answer.text(&["NodeId"]));
            }
        }
        expected.sort();
        assert_eq!(
            listed_nodes(&router, network_id, None).await,
            expected,
            "case {network_id}: expected exactly the accepted nodes to exist"
        );
    }

    // On Hyperledger Fabric the member is optional — the model marks it optional and only
    // Ethereum nodes require it — so the same omission is accepted and read back as absent.
    let answer = create_node(
        &router,
        &fabric_id,
        "zone-fabric-node",
        Some(&fabric_member),
        json!({ "InstanceType": "bc.t3.small" }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case a Fabric node with no AvailabilityZone: expected CreateNode 200, got {} {:?}",
        answer.status, answer.body
    );
    let node_id = answer.text(&["NodeId"]);
    let answer = call(
        &router,
        "GET",
        &format!("/networks/{fabric_id}/nodes/{node_id}?memberId={fabric_member}"),
        None,
    )
    .await;
    assert!(
        answer.body["Node"]["AvailabilityZone"].is_null(),
        "case a Fabric node with no AvailabilityZone: expected no zone reported, got {:?}",
        answer.body
    );
}
