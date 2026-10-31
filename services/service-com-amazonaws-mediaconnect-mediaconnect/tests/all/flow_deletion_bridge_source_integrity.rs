// SPDX-License-Identifier: CC0-1.0

//! What keeps a stored bridge from naming a cloud flow the store does not hold, end to end
//! over HTTP.
//!
//! The rule the cases below pin is the one half of this service's referential integrity that
//! applies to a REQUIRED reference: a resource is not taken out of the store while another
//! stored record names it in a member the model makes required and the write paths resolve
//! against the store — the removal is refused with the modelled semantic-error exception
//! (`BadRequestException`, 400, "the precise meaning depends on the API, and is documented in
//! the error message"), stating what still names it, and the caller clears the reference and
//! retries. The reference here is the cloud flow a bridge takes its content from
//! (`BridgeFlowSource$FlowArn`, required —
//! `smithy_query(query="shape",shape="BridgeFlowSource")` — "the ARN of the cloud flow used
//! as a source of this bridge"), which AddBridgeSources and UpdateBridgeSource both resolve
//! against the account's flows rather than taking on trust. Where the reference is OPTIONAL
//! instead — a router input's or router output's connection to a flow — the removal DROPS it
//! and is served, which is `crate::flow_removal_clears_router_references`' subject; a
//! required member cannot be dropped off a record, so the removal is refused instead, exactly
//! as it is for the router network interface a router record ingests on
//! (`crate::router_network_interface_association_integrity`).
//!
//! The rows differ along what the rule generalises over: how many bridges name the flow (none,
//! one, two), how many sources of one bridge do (one of two sources naming it), which kind of
//! source the other bridges hold (a flow source on another flow the account holds, a network
//! source naming no flow at all), and how the caller clears the reference (removing the bridge
//! source, pointing it at another flow, deleting the bridge).
//!
//! Every call travels the whole request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], and each case ends by asserting the store holds NO
//! bridge source naming a flow it does not hold — the state a delete must never produce.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::flow_lifecycle::Emulator;

/// The one network the gateway every bridge here is placed on declares — the set a network
/// source may name (`GatewayNetwork$Name`: "this name is used to reference the network and
/// must be unique among networks in this gateway").
const PLACEMENT_NETWORK: &str = "net-1";

/// The sentence a refused delete must carry, built here rather than read out of the crate, so
/// the wording is pinned by the test and not by the code under test.
///
/// It names the flow the request addressed and states how many bridges still take their
/// content from it, which is what the caller has to clear before retrying.
fn expected_refusal(flow_arn: &str, bridges: usize) -> String {
    format!(
        "{}Flow {flow_arn} is still the source of {bridges} bridge(s). Remove those bridge \
         sources, point them at another flow, or delete the bridges, before deleting the flow.",
        mc::BAD_REQUEST_PREFIX
    )
}

/// `DELETE /v1/flows/{FlowArn}`.
async fn delete_flow(emulator: &Emulator, flow_arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}"), None)
        .await
}

/// `DELETE /v1/bridges/{BridgeArn}`.
async fn delete_bridge(emulator: &Emulator, bridge_arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/bridges/{bridge_arn}"), None)
        .await
}

/// `DELETE /v1/bridges/{BridgeArn}/sources/{SourceName}`.
async fn remove_bridge_source(
    emulator: &Emulator,
    bridge_arn: &str,
    source_name: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "DELETE",
            &format!("/v1/bridges/{bridge_arn}/sources/{source_name}"),
            None,
        )
        .await
}

/// `PUT /v1/bridges/{BridgeArn}/sources/{SourceName}`.
async fn update_bridge_source(
    emulator: &Emulator,
    bridge_arn: &str,
    source_name: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/bridges/{bridge_arn}/sources/{source_name}"),
            Some(body),
        )
        .await
}

/// The name the bridges below address their `index`-th source by.
fn source_name(index: usize) -> String {
    format!("src-{index}")
}

/// One flow source entry, as a request configures it.
fn flow_source(index: usize, flow_arn: &str) -> Value {
    json!({ "flowSource": { "flowArn": flow_arn, "name": source_name(index) }})
}

/// One network source entry on the placement gateway's declared network — the kind of source
/// that names no flow at all.
fn network_source(index: usize, port: i64) -> Value {
    json!({ "networkSource": {
        "multicastIp": "224.0.1.1",
        "name": source_name(index),
        "networkName": PLACEMENT_NETWORK,
        "port": port,
        "protocol": "rtp",
    }})
}

/// Create one bridge over the wire carrying `sources`, and answer its ARN.
///
/// A bridge's DIRECTION decides the kind of source it ingests, so a bridge whose sources are
/// flow sources is a cloud-to-ground (egress) bridge and one whose sources are network sources
/// is a ground-to-cloud (ingress) bridge. A bridge holds more than one source only while its
/// failover state is active ("if the state is active, [it] can have one or two sources",
/// `FailoverConfig$State`), so the create states that configuration exactly when the row needs
/// it — and never a source priority, which would make a source unremovable.
async fn created_bridge(
    emulator: &Emulator,
    case: &str,
    name: &str,
    placement_arn: &str,
    sources: Vec<Value>,
) -> String {
    let egress = sources
        .first()
        .is_some_and(|source| source.get("flowSource").is_some());
    let mut body = json!({
        "name": name,
        "placementArn": placement_arn,
        "sources": sources,
    });
    if egress {
        body["egressGatewayBridge"] = json!({ "maxBitrate": 20_000_000i64 });
    } else {
        body["ingressGatewayBridge"] = json!({ "maxBitrate": 20_000_000i64, "maxOutputs": 2 });
    }
    if body["sources"].as_array().map(Vec::len).unwrap_or_default() > 1 {
        body["sourceFailoverConfig"] = json!({ "state": mc::STATE_ENABLED });
    }

    let (status, code, created) = create_bridge(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the create of bridge {name:?} expected 201, got {status} {code:?}: \
         {created}"
    );
    created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers a bridge ARN: {created}"))
        .to_string()
}

/// Every stored bridge source naming a flow the store does NOT hold, as
/// `(bridge ARN, flow ARN)`.
///
/// The reference is read through the crate's own reading of that relation
/// ([`mc::ComAmazonawsMediaconnectMediaconnectState::holds_flow`]), the same one every write
/// path resolves a flow reference through, so this sees exactly what the service resolves.
fn dangling_bridge_flow_references(emulator: &Emulator) -> Vec<(String, String)> {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    let mut dangling = Vec::new();
    for (bridge_arn, bridge) in store.bridges.iter() {
        for source in &bridge.sources {
            if let Some(flow_source) = source.flow_source.as_ref() {
                if !store.holds_flow(&flow_source.flow_arn) {
                    dangling.push((bridge_arn.clone(), flow_source.flow_arn.clone()));
                }
            }
        }
    }
    dangling
}

/// Assert the invariant this whole suite is about: no stored bridge names a flow the store
/// does not hold.
///
/// `pub(crate)` because the write-path suite
/// (`crate::flow_delete_and_bridge_source_writes_agree`) asserts the same invariant, so there
/// is one reading of it for the whole test crate rather than one per suite.
pub(crate) fn assert_no_dangling_references(emulator: &Emulator, case: &str) {
    let dangling = dangling_bridge_flow_references(emulator);
    assert!(
        dangling.is_empty(),
        "case {case:?}: no stored bridge source may name a flow the store does not hold, got \
         {dangling:?}"
    );
}

/// Whether DescribeFlow still answers for `flow_arn`.
async fn flow_is_held(emulator: &Emulator, case: &str, flow_arn: &str) -> bool {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert!(
        status == 200 || (status, code.as_deref()) == (404, Some("NotFoundException")),
        "case {case:?}: DescribeFlow answers either the flow or a not-found, got {status} \
         {code:?}: {described}"
    );
    status == 200
}

/// One row of the refusal table: the shape of the store a delete is attempted against.
struct Row {
    case: &'static str,
    /// How many bridges hold a flow source naming the flow under test.
    naming_bridges: usize,
    /// How many of those hold a SECOND source, on another flow the account holds — so the
    /// refusal counts bridges rather than sources.
    with_second_source: usize,
    /// How many bridges hold a flow source on another flow the account holds and nothing else.
    other_flow_bridges: usize,
    /// How many bridges hold a network source, which names no flow at all.
    network_bridges: usize,
}

/// The rows span the class: a store with no bridge, with one and with two bridges naming the
/// flow, with a bridge naming it among its two sources, and with bridges that name another
/// flow or no flow at all — the last being the rows the delete must still be served for.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no bridge at all",
            naming_bridges: 0,
            with_second_source: 0,
            other_flow_bridges: 0,
            network_bridges: 0,
        },
        Row {
            case: "a bridge naming another flow, and one with a network source",
            naming_bridges: 0,
            with_second_source: 0,
            other_flow_bridges: 1,
            network_bridges: 1,
        },
        Row {
            case: "one bridge naming the flow",
            naming_bridges: 1,
            with_second_source: 0,
            other_flow_bridges: 0,
            network_bridges: 0,
        },
        Row {
            case: "two bridges naming the flow",
            naming_bridges: 2,
            with_second_source: 0,
            other_flow_bridges: 0,
            network_bridges: 0,
        },
        Row {
            case: "one bridge naming the flow in one of its two sources",
            naming_bridges: 1,
            with_second_source: 1,
            other_flow_bridges: 0,
            network_bridges: 0,
        },
        Row {
            case: "one bridge naming the flow beside bridges that do not",
            naming_bridges: 1,
            with_second_source: 0,
            other_flow_bridges: 1,
            network_bridges: 1,
        },
    ]
}

/// The delete of a flow a stored bridge takes its content from is refused, however many
/// bridges name it and whatever else is in the store — and the refused call changes nothing:
/// the flow is still there, and the reference that was protected still resolves, so the
/// bridge source can still be re-stated through the update path.
#[tokio::test]
async fn a_flow_a_bridge_source_names_is_not_deleted() {
    for Row {
        case,
        naming_bridges,
        with_second_source,
        other_flow_bridges,
        network_bridges,
    } in rows()
    {
        let emulator = Emulator::new();
        let placement_arn =
            gateway_with_networks(&emulator, "gw-flow-delete", &[PLACEMENT_NETWORK]).await;
        let flow_arn = cloud_flow(&emulator, "flow-under-test").await;
        // A second flow nothing deletes: what the bystander bridges name, and what a
        // protected bridge's second source names.
        let other_flow = cloud_flow(&emulator, "flow-bystander").await;

        let mut naming: Vec<String> = Vec::new();
        for index in 0..naming_bridges {
            let mut sources = vec![flow_source(0, &flow_arn)];
            if index < with_second_source {
                sources.push(flow_source(1, &other_flow));
            }
            naming.push(
                created_bridge(
                    &emulator,
                    case,
                    &format!("bridge-naming-{index}"),
                    &placement_arn,
                    sources,
                )
                .await,
            );
        }
        for index in 0..other_flow_bridges {
            created_bridge(
                &emulator,
                case,
                &format!("bridge-other-flow-{index}"),
                &placement_arn,
                vec![flow_source(0, &other_flow)],
            )
            .await;
        }
        for index in 0..network_bridges {
            created_bridge(
                &emulator,
                case,
                &format!("bridge-network-{index}"),
                &placement_arn,
                vec![network_source(0, 5_000 + index as i64)],
            )
            .await;
        }

        let bridges_before: Vec<Value> = {
            let mut described = Vec::new();
            for bridge_arn in &naming {
                let (status, code, read) = describe_bridge(&emulator, bridge_arn).await;
                assert_eq!(
                    (status, code.as_deref()),
                    (200, None),
                    "case {case:?}: the bridge reads back before the delete, got {status} \
                     {code:?}: {read}"
                );
                described.push(read["bridge"].clone());
            }
            described
        };

        let (status, code, answered) = delete_flow(&emulator, &flow_arn).await;
        if naming_bridges == 0 {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: a flow no bridge source names is deleted, got {status} \
                 {code:?}: {answered}"
            );
            assert!(
                !flow_is_held(&emulator, case, &flow_arn).await,
                "case {case:?}: the deleted flow stopped existing"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: deleting a flow a bridge source names is refused, got {status} \
                 {code:?}: {answered}"
            );
            assert_eq!(
                answered["message"].as_str(),
                Some(expected_refusal(&flow_arn, naming_bridges).as_str()),
                "case {case:?}: expected {:?}, got {answered}",
                expected_refusal(&flow_arn, naming_bridges)
            );
            assert!(
                flow_is_held(&emulator, case, &flow_arn).await,
                "case {case:?}: the refused delete left the flow in the store"
            );
        }

        // The bystander flow is untouched either way: the delete addressed another flow.
        assert!(
            flow_is_held(&emulator, case, &other_flow).await,
            "case {case:?}: the flow the delete did not address is still held"
        );

        // Every protected bridge is byte for byte the one it was, and the reference the
        // refusal kept resolvable can still be re-stated through the update path — which is
        // what a caller does with the configuration a read reports.
        for (bridge_arn, before) in naming.iter().zip(bridges_before) {
            let (status, code, read) = describe_bridge(&emulator, bridge_arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: the protected bridge reads back, got {status} {code:?}: {read}"
            );
            assert_eq!(
                read["bridge"], before,
                "case {case:?}: the refused delete left the bridge exactly as it was, got {read}"
            );

            let (status, code, restated) = update_bridge_source(
                &emulator,
                bridge_arn,
                &source_name(0),
                json!({ "flowSource": { "flowArn": flow_arn } }),
            )
            .await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: re-stating the flow the protected source names expected 202, \
                 got {status} {code:?}: {restated}"
            );
        }

        assert_no_dangling_references(&emulator, case);
    }
}

/// How a caller clears a bridge's reference to the flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Clearing {
    /// RemoveBridgeSource takes the naming source off the bridge.
    RemoveSource,
    /// UpdateBridgeSource points it at another flow the account holds.
    RepointSource,
    /// DeleteBridge takes the whole bridge away.
    DeleteBridge,
}

/// The delete is served as soon as nothing names the flow, and the count the refusal states
/// walks down with the store: with two bridges naming the flow it is refused stating 2, then
/// 1 once one of them is cleared, and served once the second is — whichever of the three ways
/// of clearing the reference the caller uses.
#[tokio::test]
async fn the_flow_delete_is_served_once_no_bridge_source_names_it() {
    for clearing in [
        Clearing::RemoveSource,
        Clearing::RepointSource,
        Clearing::DeleteBridge,
    ] {
        let case = format!("cleared by {clearing:?}");
        let case = case.as_str();
        let emulator = Emulator::new();
        let placement_arn =
            gateway_with_networks(&emulator, "gw-flow-delete-clear", &[PLACEMENT_NETWORK]).await;
        let flow_arn = cloud_flow(&emulator, "flow-under-test").await;
        let other_flow = cloud_flow(&emulator, "flow-repointed-to").await;

        let mut naming: Vec<String> = Vec::new();
        for index in 0..2 {
            naming.push(
                created_bridge(
                    &emulator,
                    case,
                    &format!("bridge-clear-{index}"),
                    &placement_arn,
                    vec![flow_source(0, &flow_arn)],
                )
                .await,
            );
        }

        for (cleared, still_naming) in [(0usize, 2usize), (1, 1)] {
            // Before clearing this one, the delete is refused stating how many bridges still
            // name the flow.
            let (status, code, refused) = delete_flow(&emulator, &flow_arn).await;
            assert_eq!(
                (status, code.as_deref(), refused["message"].as_str()),
                (
                    400,
                    Some("BadRequestException"),
                    Some(expected_refusal(&flow_arn, still_naming).as_str())
                ),
                "case {case:?}: with {still_naming} bridge(s) naming it the delete is refused \
                 stating {still_naming}, got {status} {code:?}: {refused}"
            );

            let bridge_arn = &naming[cleared];
            let (status, code, answered) = match clearing {
                Clearing::RemoveSource => {
                    remove_bridge_source(&emulator, bridge_arn, &source_name(0)).await
                }
                Clearing::RepointSource => {
                    update_bridge_source(
                        &emulator,
                        bridge_arn,
                        &source_name(0),
                        json!({ "flowSource": { "flowArn": other_flow } }),
                    )
                    .await
                }
                Clearing::DeleteBridge => delete_bridge(&emulator, bridge_arn).await,
            };
            // Each of the three calls answers with its own success status: the two that write
            // on a bridge answer 202, DeleteBridge answers 200.
            let expected = match clearing {
                Clearing::DeleteBridge => 200,
                Clearing::RemoveSource | Clearing::RepointSource => 202,
            };
            assert_eq!(
                (status, code.as_deref()),
                (expected, None),
                "case {case:?}: clearing the reference of bridge {cleared} expected \
                 {expected}, got {status} {code:?}: {answered}"
            );
            assert_no_dangling_references(&emulator, case);
        }

        let (status, code, served) = delete_flow(&emulator, &flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: with nothing naming it the delete is served, got {status} {code:?}: \
             {served}"
        );
        assert_eq!(
            (
                served["flowArn"].as_str(),
                served["status"].as_str()
            ),
            (Some(flow_arn.as_str()), Some(mc::STATUS_DELETING)),
            "case {case:?}: the served delete answers the flow's own ARN and the status the \
             delete begins in, got {served}"
        );
        assert!(
            !flow_is_held(&emulator, case, &flow_arn).await,
            "case {case:?}: the deleted flow stopped existing"
        );
        assert!(
            flow_is_held(&emulator, case, &other_flow).await,
            "case {case:?}: the flow the delete did not address is still held"
        );
        assert_no_dangling_references(&emulator, case);
    }
}
