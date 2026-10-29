// SPDX-License-Identifier: CC0-1.0

//! The references a CREATED bridge carries, end to end over HTTP: gateways and flows are
//! created, then CreateBridge is called, then DescribeBridge reports what was stored.
//!
//! The rule under test: a bridge CreateBridge stores names other resources of this account,
//! and every one of those references is RESOLVED against the store before the bridge is
//! written — the gateway the bridge is placed on (`Bridge$PlacementArn` /
//! `ListedBridge$PlacementArn`, "the ARN of the gateway associated with the bridge") must be
//! a gateway the account holds, a gateway network name (`BridgeNetworkSource$NetworkName`,
//! "the network source's gateway network name"; `BridgeNetworkOutput$NetworkName`) must be
//! one that placement gateway declares (`GatewayNetwork$Name`: "this name is used to
//! reference the network and must be unique among networks in this gateway"), and a flow ARN
//! (`BridgeFlowSource$FlowArn`, "the ARN of the cloud flow used as a source of this bridge")
//! must name a flow the account holds. A reference that resolves to nothing is refused with
//! the modelled `BadRequestException` (HTTP 400) in this service's validation envelope and
//! NOTHING is stored — so a create admits exactly what the add and update paths admit
//! (`crate::bridge_add_references`, `crate::bridge_update_references`), and the bridge the
//! store holds never points at a resource the account has not got.
//!
//! The rows vary what the rule generalises over: which MEMBER carries the reference (the
//! placement ARN, a source's flow ARN, a source's network name, an output's network name),
//! which KIND of value it is (an ARN, a name), the DIRECTION of the bridge the create
//! describes (cloud-to-ground, ground-to-cloud), WHERE in a collection the unresolvable
//! entry sits (the only entry, the second of a pair), and HOW the value misses — a gateway
//! ARN of the minted form the account never created, the ARN of a gateway that was created
//! and then deleted, the ARN of a resource of another type the account does hold, a held
//! gateway's ARN differing only in case, a flow ARN never minted, the ARN of a deleted flow,
//! a network name no gateway declares, a name another gateway declares, a declared name
//! differing only in case or in surrounding space, and two declared names run together. The
//! accepting rows span every network the placement gateway declares, every flow the account
//! holds and a second gateway of the same account, so the rule is asserted as SET membership
//! rather than against one allowed value.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::flow_lifecycle::{Emulator, REGION};

/// The networks the placement gateway declares. Four of them, which is every network one
/// gateway may carry (`mc::MAX_GATEWAY_NETWORKS`, asserted below), so the accepting rows
/// cover the whole declared set rather than one name of it.
const PLACEMENT_NETWORKS: [&str; 4] = ["net-1", "net-2", "net-3", "net-4"];

/// The network a SECOND gateway of the same account declares: a name that exists in the
/// store, but not on the gateway the bridges under test are placed on.
const OTHER_GATEWAY_NETWORK: &str = "elsewhere-net";

/// A gateway ARN of the form this service mints, for a gateway it never minted — the ARN the
/// issue this suite pins was filed about. It satisfies the modelled `GatewayArn` pattern
/// (`^arn:.+:mediaconnect.+:gateway:.+$`), so the request reaches the handler.
fn unheld_gateway_arn() -> String {
    format!("arn:aws:mediaconnect:{REGION}:111122223333:gateway:1-0000beef:never-created")
}

/// A flow ARN of the form this service mints, for a flow it never minted: well formed enough
/// to satisfy the `^arn:.+:mediaconnect.+:flow:.+$` pattern the model puts on a flow ARN.
fn unheld_flow_arn() -> String {
    format!("arn:aws:mediaconnect:{REGION}:111122223333:flow:1-0000dead:never-minted")
}

/// One network source entry, as a request configures it.
fn network_source(name: &str, network_name: &str, port: i64) -> Value {
    json!({ "networkSource": {
        "multicastIp": "224.0.1.1",
        "name": name,
        "networkName": network_name,
        "port": port,
        "protocol": "rtp",
    }})
}

/// One flow source entry, as a request configures it.
fn flow_source(name: &str, flow_arn: &str) -> Value {
    json!({ "flowSource": { "flowArn": flow_arn, "name": name }})
}

/// One network output entry, as a request configures it.
fn network_output(name: &str, network_name: &str, port: i64) -> Value {
    json!({ "networkOutput": {
        "ipAddress": "10.0.0.5",
        "name": name,
        "networkName": network_name,
        "port": port,
        "protocol": "rtp",
        "ttl": 64,
    }})
}

/// A ground-to-cloud (ingress) create: the kind of bridge whose content "originates at your
/// premises", so its sources are network sources. Source failover is ENABLED whenever the
/// row carries a pair, because that is the state under which a bridge holds two sources.
fn ingress_create(name: &str, placement_arn: &str, sources: Vec<Value>) -> Value {
    let mut body = json!({
        "name": name,
        "placementArn": placement_arn,
        "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 4 },
        "sources": sources,
    });
    if body["sources"].as_array().map(Vec::len).unwrap_or_default() > 1 {
        body["sourceFailoverConfig"] = json!({ "state": mc::STATE_ENABLED });
    }
    body
}

/// A cloud-to-ground (egress) create: the kind of bridge whose content "comes from an
/// existing MediaConnect flow and is delivered to your premises", so its sources are flow
/// sources and it is the kind a request configures network outputs on.
fn egress_create(
    name: &str,
    placement_arn: &str,
    sources: Vec<Value>,
    outputs: Vec<Value>,
) -> Value {
    let mut body = json!({
        "name": name,
        "placementArn": placement_arn,
        "egressGatewayBridge": { "maxBitrate": 20_000_000 },
        "sources": sources,
    });
    if body["sources"].as_array().map(Vec::len).unwrap_or_default() > 1 {
        body["sourceFailoverConfig"] = json!({ "state": mc::STATE_ENABLED });
    }
    if !outputs.is_empty() {
        body["outputs"] = json!(outputs);
    }
    body
}

/// What the account holds when the rows below run: two gateways, two flows, and one gateway
/// and one flow whose deletes have been accepted, so their ARNs resolve nothing.
struct Fixture {
    /// The gateway declaring [`PLACEMENT_NETWORKS`].
    placement_arn: String,
    /// A second gateway of the same account, declaring [`OTHER_GATEWAY_NETWORK`] — so a
    /// placement is asserted against "a gateway of the account" rather than against one.
    second_placement_arn: String,
    /// A gateway that was created and then DELETED.
    deleted_gateway_arn: String,
    held_flow: String,
    other_held_flow: String,
    /// A flow that was minted and then DELETED.
    deleted_flow: String,
}

async fn fixture(emulator: &Emulator) -> Fixture {
    let placement_arn = gateway_with_networks(emulator, "gw-placement", &PLACEMENT_NETWORKS).await;
    let second_placement_arn =
        gateway_with_networks(emulator, "gw-elsewhere", &[OTHER_GATEWAY_NETWORK]).await;

    // A gateway nothing is placed on, so its delete is accepted: "before you can delete a
    // gateway, you must deregister its instances and delete its bridges".
    let deleted_gateway_arn = gateway_with_networks(emulator, "gw-gone", &["net-1"]).await;
    let (status, code, deleted) = emulator
        .send("DELETE", &format!("/v1/gateways/{deleted_gateway_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "fixture: the gateway to be deleted must delete, got {status} {code:?}: {deleted}"
    );

    let held_flow = cloud_flow(emulator, "flow-held").await;
    let other_held_flow = cloud_flow(emulator, "flow-other").await;
    let deleted_flow = cloud_flow(emulator, "flow-deleted").await;
    let (status, code, deleted) = emulator
        .send("DELETE", &format!("/v1/flows/{deleted_flow}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "fixture: the flow to be deleted must delete, got {status} {code:?}: {deleted}"
    );

    Fixture {
        placement_arn,
        second_placement_arn,
        deleted_gateway_arn,
        held_flow,
        other_held_flow,
        deleted_flow,
    }
}

/// How many bridges the store holds — what a refused create must leave unchanged.
fn stored_bridges(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>()
        .lock()
        .bridges
        .len()
}

/// One row of the refusal table: the create a client sends, and the value in it that names
/// nothing.
struct RefusalRow {
    case: String,
    body: Value,
    /// The reference the refusal must name, so a row cannot pass on a refusal some other
    /// rule (a direction mismatch, a name already in use, a bound) produced.
    value: String,
}

/// Every way a reference a create would store can resolve to nothing, one row each.
fn refusal_rows(fixture: &Fixture) -> Vec<RefusalRow> {
    let held = PLACEMENT_NETWORKS[0];
    let placement = fixture.placement_arn.as_str();
    let mut rows: Vec<RefusalRow> = Vec::new();

    // The PLACEMENT: a gateway of the account, whatever direction the bridge carries.
    for (how, arn) in [
        ("a gateway ARN the account never created", unheld_gateway_arn()),
        (
            "the ARN of a gateway that was deleted",
            fixture.deleted_gateway_arn.clone(),
        ),
        (
            "the ARN of a flow the account holds, which is no gateway",
            fixture.held_flow.clone(),
        ),
        (
            "a held gateway's ARN differing only in case",
            fixture.placement_arn.to_uppercase(),
        ),
    ] {
        rows.push(RefusalRow {
            case: format!("ground-to-cloud bridge placed on {how}"),
            body: ingress_create("b-placement-ingress", &arn, vec![network_source(
                "src", held, 5_000,
            )]),
            value: arn.clone(),
        });
        rows.push(RefusalRow {
            case: format!("cloud-to-ground bridge placed on {how}"),
            body: egress_create(
                "b-placement-egress",
                &arn,
                vec![flow_source("src", &fixture.held_flow)],
                Vec::new(),
            ),
            value: arn,
        });
    }

    // A FLOW SOURCE's flow: a flow of the account, in either position of the pair a bridge
    // with failover ENABLED can ingest.
    for (how, arn) in [
        ("a flow ARN the account never minted", unheld_flow_arn()),
        (
            "the ARN of a flow that was deleted",
            fixture.deleted_flow.clone(),
        ),
    ] {
        rows.push(RefusalRow {
            case: format!("cloud-to-ground bridge whose only source names {how}"),
            body: egress_create(
                "b-flow-only",
                placement,
                vec![flow_source("src", &arn)],
                Vec::new(),
            ),
            value: arn.clone(),
        });
        rows.push(RefusalRow {
            case: format!("cloud-to-ground bridge whose SECOND source names {how}"),
            body: egress_create(
                "b-flow-second",
                placement,
                vec![
                    flow_source("first", &fixture.held_flow),
                    flow_source("second", &arn),
                ],
                Vec::new(),
            ),
            value: arn,
        });
    }

    // A NETWORK NAME: one of the networks the placement gateway declares, on a source and on
    // an output alike.
    for (how, name) in [
        ("a name no gateway declares", "no-such-network".to_string()),
        (
            "a name the account's OTHER gateway declares",
            OTHER_GATEWAY_NETWORK.to_string(),
        ),
        (
            "a declared name differing only in case",
            held.to_uppercase(),
        ),
        (
            "a declared name with surrounding space",
            format!(" {held} "),
        ),
        (
            "two declared names run together",
            format!("{held}{}", PLACEMENT_NETWORKS[1]),
        ),
        ("the empty name", String::new()),
    ] {
        rows.push(RefusalRow {
            case: format!("ground-to-cloud bridge whose only source ingests on {how}"),
            body: ingress_create(
                "b-network-only",
                placement,
                vec![network_source("src", &name, 5_001)],
            ),
            value: name.clone(),
        });
        rows.push(RefusalRow {
            case: format!("ground-to-cloud bridge whose SECOND source ingests on {how}"),
            body: ingress_create(
                "b-network-second",
                placement,
                vec![
                    network_source("first", held, 5_002),
                    network_source("second", &name, 5_003),
                ],
            ),
            value: name.clone(),
        });
        rows.push(RefusalRow {
            case: format!("cloud-to-ground bridge whose only output is delivered over {how}"),
            body: egress_create(
                "b-output-only",
                placement,
                vec![flow_source("src", &fixture.held_flow)],
                vec![network_output("out", &name, 6_000)],
            ),
            value: name.clone(),
        });
        rows.push(RefusalRow {
            case: format!("cloud-to-ground bridge whose SECOND output is delivered over {how}"),
            body: egress_create(
                "b-output-second",
                placement,
                vec![flow_source("src", &fixture.held_flow)],
                vec![
                    network_output("first-out", held, 6_001),
                    network_output("second-out", &name, 6_002),
                ],
            ),
            value: name,
        });
    }

    rows
}

/// A create carrying a reference the store cannot resolve is refused with the modelled
/// `BadRequestException`, the refusal names the value that resolved to nothing, and the
/// bridge store is left exactly as it was.
#[tokio::test]
async fn a_create_naming_a_resource_the_account_does_not_hold_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    assert_eq!(
        PLACEMENT_NETWORKS.len(),
        mc::MAX_GATEWAY_NETWORKS,
        "the placement gateway declares every network one gateway may carry: expected {}, \
         got {}",
        mc::MAX_GATEWAY_NETWORKS,
        PLACEMENT_NETWORKS.len()
    );

    for row in refusal_rows(&fixture) {
        let RefusalRow { case, body, value } = row;
        let before = stored_bridges(&emulator);
        let (status, code, answered) = create_bridge(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {answered}"
        );
        let message = answered["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: a refusal about the request's content is worded in the {:?} \
             envelope, got {message:?}",
            mc::VALIDATION_FAILED_PREFIX
        );
        assert!(
            message.contains(&format!("{value:?}")),
            "case {case:?}: the refusal names the reference that did not resolve ({value:?}), \
             got {message:?}"
        );
        let after = stored_bridges(&emulator);
        assert_eq!(
            after, before,
            "case {case:?}: a refused create stores no bridge, got {after} bridges where the \
             store held {before}"
        );
    }
}

/// One row of the accepting table: the create, and the placement and member values the stored
/// bridge must report back.
struct AcceptedRow {
    case: String,
    body: Value,
    placement_arn: String,
    /// `(the JSON pointer into the bridge, the value it must carry)`, so each row asserts
    /// the reference it is about rather than the whole record.
    reference: (&'static str, String),
}

/// The accepting rows: every network the placement gateway declares, every flow the account
/// holds, and a bridge placed on each gateway of the account.
fn accepted_rows(fixture: &Fixture) -> Vec<AcceptedRow> {
    let mut rows: Vec<AcceptedRow> = Vec::new();

    // Each declared network of the placement gateway, on a source and on an output.
    for (index, network_name) in PLACEMENT_NETWORKS.into_iter().enumerate() {
        rows.push(AcceptedRow {
            case: format!("ground-to-cloud bridge ingesting on the declared {network_name:?}"),
            body: ingress_create(
                &format!("b-ok-source-{index}"),
                &fixture.placement_arn,
                vec![network_source("src", network_name, 5_100 + index as i64)],
            ),
            placement_arn: fixture.placement_arn.clone(),
            reference: (
                "/sources/0/networkSource/networkName",
                network_name.to_string(),
            ),
        });
        rows.push(AcceptedRow {
            case: format!("cloud-to-ground bridge delivering over the declared {network_name:?}"),
            body: egress_create(
                &format!("b-ok-output-{index}"),
                &fixture.placement_arn,
                vec![flow_source("src", &fixture.held_flow)],
                vec![network_output("out", network_name, 6_100 + index as i64)],
            ),
            placement_arn: fixture.placement_arn.clone(),
            reference: (
                "/outputs/0/networkOutput/networkName",
                network_name.to_string(),
            ),
        });
    }

    // Each flow the account holds, as the source of a cloud-to-ground bridge.
    for (index, flow_arn) in [&fixture.held_flow, &fixture.other_held_flow]
        .into_iter()
        .enumerate()
    {
        rows.push(AcceptedRow {
            case: format!("cloud-to-ground bridge fed by held flow {index}"),
            body: egress_create(
                &format!("b-ok-flow-{index}"),
                &fixture.placement_arn,
                vec![flow_source("src", flow_arn)],
                Vec::new(),
            ),
            placement_arn: fixture.placement_arn.clone(),
            reference: ("/sources/0/flowSource/flowArn", flow_arn.clone()),
        });
    }

    // The failover pair, each source on a flow of its own: the rule holds for every entry of
    // a collection, not only the first.
    rows.push(AcceptedRow {
        case: "cloud-to-ground bridge whose failover pair names two held flows".to_string(),
        body: egress_create(
            "b-ok-pair",
            &fixture.placement_arn,
            vec![
                flow_source("first", &fixture.held_flow),
                flow_source("second", &fixture.other_held_flow),
            ],
            Vec::new(),
        ),
        placement_arn: fixture.placement_arn.clone(),
        reference: (
            "/sources/1/flowSource/flowArn",
            fixture.other_held_flow.clone(),
        ),
    });

    // The SECOND gateway of the account, and its own declared network: the placement is
    // resolved against the gateways the account holds, not against one particular gateway.
    rows.push(AcceptedRow {
        case: "ground-to-cloud bridge placed on the account's second gateway".to_string(),
        body: ingress_create(
            "b-ok-second-gateway",
            &fixture.second_placement_arn,
            vec![network_source("src", OTHER_GATEWAY_NETWORK, 5_200)],
        ),
        placement_arn: fixture.second_placement_arn.clone(),
        reference: (
            "/sources/0/networkSource/networkName",
            OTHER_GATEWAY_NETWORK.to_string(),
        ),
    });

    rows
}

/// A create whose every reference resolves is stored and reported back: the placement is the
/// gateway the request named and DescribeBridge afterwards repeats the record.
#[tokio::test]
async fn a_create_whose_references_resolve_is_stored_and_described() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for row in accepted_rows(&fixture) {
        let AcceptedRow {
            case,
            body,
            placement_arn,
            reference,
        } = row;

        let (status, code, created) = create_bridge(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: create expected 201, got {status} {code:?}: {created}"
        );
        let bridge = created["bridge"].clone();
        assert_eq!(
            bridge["placementArn"].as_str(),
            Some(placement_arn.as_str()),
            "case {case:?}: the bridge is placed on the gateway the request named, got {bridge}"
        );
        let (path, expected) = reference;
        let answered = bridge.pointer(path).unwrap_or(&Value::Null);
        assert_eq!(
            answered.as_str(),
            Some(expected.as_str()),
            "case {case:?}: the stored {path} is the reference the request named, got {bridge}"
        );

        // The round trip: the bridge DescribeBridge reports is the one the create stored.
        let bridge_arn = bridge["bridgeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {bridge}"))
            .to_string();
        let (status, code, described) = describe_bridge(&emulator, &bridge_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
        );
        for member in ["placementArn", "sources", "outputs"] {
            assert_eq!(
                described["bridge"][member], bridge[member],
                "case {case:?}: DescribeBridge must report the created {member}, got {:?}",
                described["bridge"][member]
            );
        }
    }
}

/// A gateway stops resolving the moment its delete is accepted, so a create placed on it is
/// refused from then on — and the bridges the gateway already carries are what the delete
/// required to be gone first, which is why no stored bridge is left pointing at it.
#[tokio::test]
async fn a_gateway_stops_being_a_placement_once_its_delete_is_accepted() {
    let emulator = Emulator::new();
    let placement = gateway_with_networks(&emulator, "gw-retiring", &["net-1"]).await;

    // While the gateway is live the create is accepted.
    let (status, code, created) = create_bridge(
        &emulator,
        ingress_create(
            "bridge-before-delete",
            &placement,
            vec![network_source("src", "net-1", 5_000)],
        ),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "a create placed on a live gateway expects 201, got {status} {code:?}: {created}"
    );
    let bridge_arn = created["bridge"]["bridgeArn"]
        .as_str()
        .expect("the create answers a bridge ARN")
        .to_string();

    // The gateway's own delete is refused while that bridge is placed on it.
    let (status, code, refused) = emulator
        .send("DELETE", &format!("/v1/gateways/{placement}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "a gateway with a bridge placed on it must be refused, got {status} {code:?}: {refused}"
    );

    let (status, code, deleted) = emulator
        .send("DELETE", &format!("/v1/bridges/{bridge_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the bridge delete expects 200, got {status} {code:?}: {deleted}"
    );
    let (status, code, deleted) = emulator
        .send("DELETE", &format!("/v1/gateways/{placement}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "with its bridge deleted the gateway is deletable, got {status} {code:?}: {deleted}"
    );

    // And from now on nothing can be placed on it: the account has given it up.
    let before = stored_bridges(&emulator);
    let (status, code, answered) = create_bridge(
        &emulator,
        ingress_create(
            "bridge-after-delete",
            &placement,
            vec![network_source("src", "net-1", 5_001)],
        ),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "a create placed on a deleted gateway expects 400 BadRequestException, got {status} \
         {code:?}: {answered}"
    );
    let after = stored_bridges(&emulator);
    assert_eq!(
        after, before,
        "a refused create stores no bridge, got {after} bridges where the store held {before}"
    );
}
