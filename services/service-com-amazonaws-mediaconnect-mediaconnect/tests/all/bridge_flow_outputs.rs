// SPDX-License-Identifier: CC0-1.0

//! The flow output a bridge is given because a cloud flow takes it as a source, end to end
//! over HTTP.
//!
//! The rule under test: a flow source's `GatewayBridgeSource` is "the source configuration
//! for cloud flows receiving a stream from a bridge", and the stream a bridge delivers into
//! the cloud is an output OF THE BRIDGE — a `BridgeFlowOutput`, "the output of the bridge. A
//! flow output is delivered to the Amazon Web Services cloud", whose three required members
//! are the cloud flow (`FlowArn`), the flow source (`FlowSourceArn`) and the name the bridge
//! addresses the output by (`Name`). So the service holds one such output per flow source
//! that names the bridge, for exactly as long as that flow source names it, and no request
//! configures or removes one: `AddBridgeOutputRequest` carries a `NetworkOutput` only.
//!
//! The rows vary along what the rule generalises over: WHICH call pointed the flow source at
//! the bridge (CreateFlow's singular `Source`, CreateFlow's `Sources` list, AddFlowSources,
//! UpdateFlowSource), whether the source was named by the request or by the service, how many
//! flow sources name one bridge at once (including two that share a name), and HOW the flow
//! source stops naming it (RemoveFlowSource, UpdateFlowSource pointing it at another bridge,
//! DeleteFlow).
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness [`crate::flow_lifecycle::Emulator`],
//! so what they assert is what a MediaConnect client sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{
    bridge_feeding_a_flow, cloud_flow, create_bridge, describe_bridge, gateway_with_networks,
};
use crate::bridge_members_and_deletion::{add_outputs, network_output};
use crate::flow_lifecycle::Emulator;
use crate::flow_sources::{
    add_flow_sources, create_flow_from, described_sources, failover_active, remove_flow_source,
};
use crate::update_flow_source::update_flow_source;

/// `PUT /v1/bridges/{BridgeArn}/outputs/{OutputName}` — the UpdateBridgeOutput binding
/// (`smithy_query(query="operation",operation="UpdateBridgeOutput")`).
async fn update_bridge_output(
    emulator: &Emulator,
    bridge_arn: &str,
    output_name: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/bridges/{bridge_arn}/outputs/{output_name}"),
            Some(body),
        )
        .await
}

/// `DELETE /v1/bridges/{BridgeArn}/outputs/{OutputName}` — the RemoveBridgeOutput binding
/// (`smithy_query(query="operation",operation="RemoveBridgeOutput")`).
async fn remove_bridge_output(
    emulator: &Emulator,
    bridge_arn: &str,
    output_name: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "DELETE",
            &format!("/v1/bridges/{bridge_arn}/outputs/{output_name}"),
            None,
        )
        .await
}

/// `DELETE /v1/flows/{FlowArn}` — the DeleteFlow binding
/// (`smithy_query(query="operation",operation="DeleteFlow")`).
async fn delete_flow(emulator: &Emulator, flow_arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}"), None)
        .await
}

/// `Bridge$Outputs` as DescribeBridge reports it — an empty list for a bridge that holds no
/// output, whose optional member is absent rather than empty.
async fn bridge_outputs(emulator: &Emulator, case: &str, bridge_arn: &str) -> Vec<Value> {
    let (status, code, described) = describe_bridge(emulator, bridge_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeBridge must answer 200, got {status} {code:?}: {described}"
    );
    described["bridge"]["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The stored source of `flow_arn` that names `bridge_arn`, as `(SourceArn, Name)`.
async fn source_fed_by(
    emulator: &Emulator,
    case: &str,
    flow_arn: &str,
    bridge_arn: &str,
) -> (String, String) {
    let sources = described_sources(emulator, case, flow_arn).await;
    let source = sources
        .iter()
        .find(|source| source["gatewayBridgeSource"]["bridgeArn"].as_str() == Some(bridge_arn))
        .unwrap_or_else(|| {
            panic!("case {case:?}: a source of the flow names the bridge, got {sources:?}")
        });
    (
        source["sourceArn"].as_str().unwrap_or_default().to_string(),
        source["name"].as_str().unwrap_or_default().to_string(),
    )
}

/// Assert the bridge `bridge_arn` reports exactly one output, and that it is the flow output
/// the rule says the flow source `(flow_arn, source_arn, source_name)` gave it.
async fn assert_only_flow_output(
    emulator: &Emulator,
    case: &str,
    bridge_arn: &str,
    flow_arn: &str,
    source_arn: &str,
    source_name: &str,
) {
    let outputs = bridge_outputs(emulator, case, bridge_arn).await;
    assert_eq!(
        outputs.len(),
        1,
        "case {case:?}: the bridge holds the one flow output its cloud flow asked for, got \
         {outputs:?}"
    );
    let flow_output = &outputs[0]["flowOutput"];
    assert_eq!(
        (
            flow_output["flowArn"].as_str(),
            flow_output["flowSourceArn"].as_str(),
            flow_output["name"].as_str(),
        ),
        (Some(flow_arn), Some(source_arn), Some(source_name)),
        "case {case:?}: the flow output names the cloud flow, the flow source and the source \
         it is the output for, got {:?}",
        outputs[0]
    );
    assert!(
        outputs[0]["networkOutput"].is_null(),
        "case {case:?}: a flow output is not a network output, got {:?}",
        outputs[0]
    );
}

/// Which call points the flow source at the bridge. The four paths that write a
/// `GatewayBridgeSource` onto a flow source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StorePath {
    /// CreateFlow's own `Source` — "the settings for the source of the flow".
    CreateFlowSource,
    /// CreateFlow's `Sources` list.
    CreateFlowSources,
    /// AddFlowSources, appending the source to a flow that already holds one.
    AddFlowSources,
    /// UpdateFlowSource, pointing a source the flow already holds at the bridge.
    UpdateFlowSource,
}

/// One row of the materialisation table: which call stored the source, and the name the
/// request gave it (`None` where the SERVICE names it, which it does for a source whose
/// request carried no name).
struct MaterialiseCase {
    case: &'static str,
    path: StorePath,
    requested_name: Option<&'static str>,
}

/// Point one flow source at `bridge_arn` through `path` and answer the flow's ARN.
async fn point_a_source_at(
    emulator: &Emulator,
    case: &str,
    flow_name: &str,
    bridge_arn: &str,
    path: StorePath,
    requested_name: Option<&str>,
) -> String {
    let mut bridge_source = json!({ "gatewayBridgeSource": { "bridgeArn": bridge_arn } });
    if let Some(name) = requested_name {
        bridge_source["name"] = json!(name);
    }
    let plain = json!({ "name": "plain", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" });

    match path {
        StorePath::CreateFlowSource => {
            create_flow_from(
                emulator,
                case,
                json!({ "name": flow_name, "source": bridge_source }),
            )
            .await
        }
        StorePath::CreateFlowSources => {
            create_flow_from(
                emulator,
                case,
                json!({ "name": flow_name, "sources": [bridge_source] }),
            )
            .await
        }
        StorePath::AddFlowSources => {
            // A flow holding a PAIR of sources needs source failover active — "if the state
            // is active, the flow can have one or two sources" (`FailoverConfig$State`).
            let flow_arn = create_flow_from(
                emulator,
                case,
                json!({
                    "name": flow_name,
                    "sourceFailoverConfig": failover_active(),
                    "source": plain,
                }),
            )
            .await;
            let (status, code, added) =
                add_flow_sources(emulator, &flow_arn, json!([bridge_source])).await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: AddFlowSources must append the source, got {status} {code:?}: \
                 {added}"
            );
            flow_arn
        }
        StorePath::UpdateFlowSource => {
            let flow_arn = create_flow_from(
                emulator,
                case,
                json!({ "name": flow_name, "source": plain }),
            )
            .await;
            let source_arn = described_sources(emulator, case, &flow_arn).await[0]["sourceArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (status, code, updated) = update_flow_source(
                emulator,
                &flow_arn,
                &source_arn,
                json!({ "gatewayBridgeSource": { "bridgeArn": bridge_arn } }),
            )
            .await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: UpdateFlowSource must store the bridge, got {status} {code:?}: \
                 {updated}"
            );
            flow_arn
        }
    }
}

/// Whichever call points a flow source at a bridge, that bridge then holds the flow output
/// the service materialises for it, reported by DescribeBridge.
#[tokio::test]
async fn every_path_that_points_a_flow_source_at_a_bridge_gives_the_bridge_its_flow_output() {
    let emulator = Emulator::new();

    let rows = [
        MaterialiseCase {
            case: "CreateFlow's own Source, named by the request",
            path: StorePath::CreateFlowSource,
            requested_name: Some("from-bridge"),
        },
        MaterialiseCase {
            case: "CreateFlow's own Source, named by the service",
            path: StorePath::CreateFlowSource,
            requested_name: None,
        },
        MaterialiseCase {
            case: "CreateFlow's Sources list",
            path: StorePath::CreateFlowSources,
            requested_name: Some("listed-source"),
        },
        MaterialiseCase {
            case: "AddFlowSources, appended to a flow that already has a source",
            path: StorePath::AddFlowSources,
            requested_name: Some("added-from-bridge"),
        },
        MaterialiseCase {
            case: "AddFlowSources, with the appended source named by the service",
            path: StorePath::AddFlowSources,
            requested_name: None,
        },
        MaterialiseCase {
            case: "UpdateFlowSource, pointing a stored source at the bridge",
            path: StorePath::UpdateFlowSource,
            requested_name: None,
        },
    ];


    for (index, MaterialiseCase { case, path, requested_name }) in rows.iter().enumerate() {
        // One bridge per row, so each row's bridge reports only the output its own flow
        // source asked for.
        let bridge_arn = bridge_feeding_a_flow(&emulator, &format!("br-materialise-{index}")).await;
        let before = bridge_outputs(&emulator, case, &bridge_arn).await;
        assert!(
            before.is_empty(),
            "case {case:?}: a bridge no flow names yet holds no output, got {before:?}"
        );

        let flow_arn = point_a_source_at(
            &emulator,
            case,
            &format!("flow-materialise-{index}"),
            &bridge_arn,
            *path,
            *requested_name,
        )
        .await;
        let (source_arn, source_name) =
            source_fed_by(&emulator, case, &flow_arn, &bridge_arn).await;
        // The output is named after the flow source it carries into the cloud, whether the
        // request named that source or the service did.
        if let Some(requested) = requested_name {
            assert_eq!(
                source_name, *requested,
                "case {case:?}: the source keeps the name the request gave it"
            );
        }
        assert_only_flow_output(
            &emulator,
            case,
            &bridge_arn,
            &flow_arn,
            &source_arn,
            &source_name,
        )
        .await;
    }
}

/// A bridge holds ONE flow output per flow source that names it, each addressable by its own
/// name — including when two of those sources, on different flows, share a name.
#[tokio::test]
async fn a_bridge_holds_one_addressable_flow_output_per_flow_source_that_names_it() {
    let emulator = Emulator::new();
    let bridge_arn = bridge_feeding_a_flow(&emulator, "br-many").await;
    let case = "three flow sources naming one bridge";

    let mut expected: Vec<(String, String, String)> = Vec::new();
    // Two sources that share a name, and one that does not: an output's name "must be
    // unique among outputs in this bridge", so the second of the two takes the next name
    // the bridge does not already address an output by.
    for (index, (flow_name, source_name, expected_output_name)) in [
        ("flow-many-a", "shared", "shared"),
        ("flow-many-b", "shared", "shared-2"),
        ("flow-many-c", "other", "other"),
    ]
    .iter()
    .enumerate()
    {
        let flow_arn = create_flow_from(
            &emulator,
            case,
            json!({
                "name": flow_name,
                "source": {
                    "name": source_name,
                    "gatewayBridgeSource": { "bridgeArn": bridge_arn },
                },
            }),
        )
        .await;
        let (source_arn, stored_name) =
            source_fed_by(&emulator, case, &flow_arn, &bridge_arn).await;
        assert_eq!(
            stored_name, *source_name,
            "case {case:?}: source {index} keeps the name the request gave it"
        );
        expected.push((
            flow_arn,
            source_arn,
            (*expected_output_name).to_string(),
        ));
    }

    let outputs = bridge_outputs(&emulator, case, &bridge_arn).await;
    let reported: Vec<(String, String, String)> = outputs
        .iter()
        .map(|output| {
            (
                output["flowOutput"]["flowArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                output["flowOutput"]["flowSourceArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                output["flowOutput"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        reported, expected,
        "case {case:?}: the bridge holds one flow output per flow source that names it, each \
         under a name of its own, got {outputs:?}"
    );

    // A name the bridge reports is a name the bridge ADDRESSES that output by: the name is
    // the `@httpLabel` RemoveBridgeOutput and UpdateBridgeOutput resolve an output with, so
    // an update carrying no setting answers exactly the stored output.
    for (_, _, output_name) in &expected {
        let (status, code, answered) =
            update_bridge_output(&emulator, &bridge_arn, output_name, json!({})).await;
        assert_eq!(
            (status, code.as_deref(), answered["output"]["flowOutput"]["name"].as_str()),
            (202, None, Some(output_name.as_str())),
            "case {case:?}: the bridge addresses its flow output by {output_name:?}, got \
             {status} {code:?}: {answered}"
        );
    }
}

/// How a flow source stops naming the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClearPath {
    /// RemoveFlowSource takes the source out of the flow.
    RemoveFlowSource,
    /// UpdateFlowSource points the same source at ANOTHER bridge.
    RepointToAnotherBridge,
    /// DeleteFlow takes the whole flow, sources and all.
    DeleteFlow,
}

/// One row of the removal table.
struct ClearCase {
    case: &'static str,
    path: ClearPath,
}

/// A bridge's flow output lives exactly as long as the flow source that asked for it: when
/// that source stops naming the bridge, the output stops being reported — whichever call
/// cleared it.
#[tokio::test]
async fn a_bridges_flow_output_goes_when_the_flow_source_stops_naming_it() {
    let emulator = Emulator::new();

    let rows = [
        ClearCase {
            case: "RemoveFlowSource takes the source out of the flow",
            path: ClearPath::RemoveFlowSource,
        },
        ClearCase {
            case: "UpdateFlowSource points the source at another bridge",
            path: ClearPath::RepointToAnotherBridge,
        },
        ClearCase {
            case: "DeleteFlow takes the flow and its sources",
            path: ClearPath::DeleteFlow,
        },
    ];


    for (index, ClearCase { case, path }) in rows.iter().enumerate() {
        let bridge_arn = bridge_feeding_a_flow(&emulator, &format!("br-cleared-{index}")).await;
        // A flow whose source is REMOVED must hold more than one source ("this request can
        // be made only if there is more than one source on the flow"), so every row works on
        // the same two-source flow and the rows stay comparable.
        let flow_arn = create_flow_from(
            &emulator,
            case,
            json!({
                "name": format!("flow-cleared-{index}"),
                "sourceFailoverConfig": failover_active(),
                "sources": [
                    { "name": "plain", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
                    { "name": "from-bridge", "gatewayBridgeSource": { "bridgeArn": bridge_arn } },
                ],
            }),
        )
        .await;
        let (source_arn, source_name) =
            source_fed_by(&emulator, case, &flow_arn, &bridge_arn).await;
        assert_only_flow_output(
            &emulator,
            case,
            &bridge_arn,
            &flow_arn,
            &source_arn,
            &source_name,
        )
        .await;

        match path {
            ClearPath::RemoveFlowSource => {
                let (status, code, removed) =
                    remove_flow_source(&emulator, &flow_arn, &source_arn).await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: the source must be removed, got {status} {code:?}: {removed}"
                );
            }
            ClearPath::RepointToAnotherBridge => {
                let other_arn =
                    bridge_feeding_a_flow(&emulator, &format!("br-repointed-{index}")).await;
                let (status, code, updated) = update_flow_source(
                    &emulator,
                    &flow_arn,
                    &source_arn,
                    json!({ "gatewayBridgeSource": { "bridgeArn": other_arn } }),
                )
                .await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: the source must be repointed, got {status} {code:?}: \
                     {updated}"
                );
                // The output follows the reference: the bridge the source NOW names holds it.
                assert_only_flow_output(
                    &emulator,
                    case,
                    &other_arn,
                    &flow_arn,
                    &source_arn,
                    &source_name,
                )
                .await;
            }
            ClearPath::DeleteFlow => {
                let (status, code, deleted) = delete_flow(&emulator, &flow_arn).await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: the flow must be deleted, got {status} {code:?}: {deleted}"
                );
            }
        }

        let after = bridge_outputs(&emulator, case, &bridge_arn).await;
        assert!(
            after.is_empty(),
            "case {case:?}: the bridge holds no flow output once no flow source names it, got \
             {after:?}"
        );
    }
}

/// A bridge's flow output is the service's own: no request configures one, updates one's
/// network settings or removes one — while a network output a request DID configure stays
/// removable.
#[tokio::test]
async fn a_bridges_flow_output_is_not_an_output_a_request_configures() {
    let emulator = Emulator::new();
    let case = "an output of the ground-to-cloud bridge a flow takes as its source";
    let bridge_arn = bridge_feeding_a_flow(&emulator, "br-kinds").await;
    let flow_arn = create_flow_from(
        &emulator,
        case,
        json!({
            "name": "flow-kinds",
            "source": {
                "name": "from-bridge",
                "gatewayBridgeSource": { "bridgeArn": bridge_arn },
            },
        }),
    )
    .await;
    let (source_arn, output_name) = source_fed_by(&emulator, case, &flow_arn, &bridge_arn).await;
    assert_only_flow_output(
        &emulator,
        case,
        &bridge_arn,
        &flow_arn,
        &source_arn,
        &output_name,
    )
    .await;

    // A ground-to-cloud bridge's outputs ARE these flow outputs, so it still takes no output
    // in a request — the refusal a client reads now describes outputs the bridge really holds.
    let (status, code, refused) = add_outputs(
        &emulator,
        &bridge_arn,
        json!({ "outputs": [network_output("net-out", 5001, "rtp", 64)] }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: a ground-to-cloud bridge takes no output in a request, got {status} \
         {code:?}: {refused}"
    );

    // `UpdateBridgeOutputRequest` carries a network wrapper only, and a flow output has no
    // network settings to update.
    let (status, code, update_refused) = update_bridge_output(
        &emulator,
        &bridge_arn,
        &output_name,
        json!({ "networkOutput": { "port": 6001 } }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: a flow output has no network settings to update, got {status} \
         {code:?}: {update_refused}"
    );
    let message = update_refused["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::BAD_REQUEST_PREFIX),
        "case {case:?}: a well-formed request the output does not admit keeps this service's \
         bad-request envelope, got {update_refused}"
    );

    // And no request removes one either: it would strand the flow source that asked for it.
    let (status, code, remove_refused) =
        remove_bridge_output(&emulator, &bridge_arn, &output_name).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: a flow output is not an output a request removes, got {status} \
         {code:?}: {remove_refused}"
    );
    let message = remove_refused["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&flow_arn) && message.contains(&source_arn),
        "case {case:?}: the refusal names the flow and the flow source the output belongs to, \
         got {remove_refused}"
    );
    assert_only_flow_output(
        &emulator,
        case,
        &bridge_arn,
        &flow_arn,
        &source_arn,
        &output_name,
    )
    .await;

    // The refusal is keyed to the KIND of output, not to the operation: the network output a
    // request configured on a cloud-to-ground bridge is still removed by the request that
    // addresses it.
    let case = "a network output a request configured";
    let placement_arn = gateway_with_networks(&emulator, "gw-kinds-egress", &["net-1"]).await;
    let egress_source = cloud_flow(&emulator, "flow-kinds-egress").await;
    let (status, code, created) = create_bridge(
        &emulator,
        json!({
            "name": "br-kinds-egress",
            "placementArn": placement_arn,
            "egressGatewayBridge": { "maxBitrate": 80_000_000 },
            "sources": [{ "flowSource": { "flowArn": egress_source, "name": "flow-src" } }],
            "outputs": [network_output("net-out", 5001, "rtp", 64)],
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the fixture bridge must be created, got {status} {code:?}: {created}"
    );
    let egress_arn = created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, code, removed) = remove_bridge_output(&emulator, &egress_arn, "net-out").await;
    assert_eq!(
        (status, code.as_deref(), removed["outputName"].as_str()),
        (202, None, Some("net-out")),
        "case {case:?}: a network output is removed by the request that addresses it, got \
         {status} {code:?}: {removed}"
    );
    let after = bridge_outputs(&emulator, case, &egress_arn).await;
    assert!(
        after.is_empty(),
        "case {case:?}: the removed network output is gone from the bridge, got {after:?}"
    );
}
