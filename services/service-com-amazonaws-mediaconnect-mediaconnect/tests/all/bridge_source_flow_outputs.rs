// SPDX-License-Identifier: CC0-1.0

//! The output a cloud flow is given because a bridge takes that flow as one of its sources,
//! end to end over HTTP.
//!
//! The rule under test: a bridge's flow source "originates in MediaConnect as an existing
//! cloud flow" (`BridgeFlowSource`), and the stream that flow delivers to the bridge leaves
//! it through an output OF THE FLOW — `BridgeFlowSource$OutputArn` is "the Amazon Resource
//! Number (ARN) of the output" and `Output$BridgeArn` is "the ARN of the bridge added to
//! this output", the two ends of one record. So whenever the service stores a bridge source
//! naming a cloud flow it creates that output on the flow and records its ARN on the stored
//! source, the output exists for exactly as long as the bridge source that asked for it, and
//! no request configures or removes one: `AddBridgeFlowSourceRequest` carries a flow, a name
//! and a VPC interface attachment and nothing about an output.
//!
//! The rows vary along what the rule generalises over: WHICH call stored the bridge source
//! (CreateBridge's `Sources`, AddBridgeSources, UpdateBridgeSource pointing a source at
//! another flow), how many sources one bridge holds (one, the failover pair) and whether
//! they name one flow or two, how many bridges take one flow at once, whether the name the
//! output is given is already in use on the flow, and HOW the source stops naming the flow
//! (RemoveBridgeSource, UpdateBridgeSource repointing it, DeleteBridge).
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness [`crate::flow_lifecycle::Emulator`],
//! so what they assert is what a MediaConnect client sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::flow_lifecycle::Emulator;
use crate::flow_outputs::{add_flow_outputs, assert_minted_output_arn, described_outputs};
use crate::router_input_configuration_targets::mediaconnect_flow as flow_input_configuration;
use crate::router_input_restart::{
    create_body as input_create_body, create_router_input, get_router_input,
};

/// `POST /v1/bridges/{BridgeArn}/sources` — the AddBridgeSources binding
/// (`smithy_query(query="operation",operation="AddBridgeSources")`).
async fn add_bridge_sources(
    emulator: &Emulator,
    bridge_arn: &str,
    sources: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "POST",
            &format!("/v1/bridges/{bridge_arn}/sources"),
            Some(json!({ "sources": sources })),
        )
        .await
}

/// `PUT /v1/bridges/{BridgeArn}/sources/{SourceName}` — the UpdateBridgeSource binding
/// (`smithy_query(query="operation",operation="UpdateBridgeSource")`).
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

/// `DELETE /v1/bridges/{BridgeArn}/sources/{SourceName}` — the RemoveBridgeSource binding
/// (`smithy_query(query="operation",operation="RemoveBridgeSource")`).
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

/// `DELETE /v1/bridges/{BridgeArn}` — the DeleteBridge binding
/// (`smithy_query(query="operation",operation="DeleteBridge")`).
async fn delete_bridge(emulator: &Emulator, bridge_arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/bridges/{bridge_arn}"), None)
        .await
}

/// `DELETE /v1/flows/{FlowArn}/outputs/{OutputArn}` — the RemoveFlowOutput binding
/// (`smithy_query(query="operation",operation="RemoveFlowOutput")`).
async fn remove_flow_output(
    emulator: &Emulator,
    flow_arn: &str,
    output_arn: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/outputs/{output_arn}"),
            None,
        )
        .await
}

/// One flow source entry, as a request configures it.
fn flow_source(name: &str, flow_arn: &str) -> Value {
    json!({ "flowSource": { "flowArn": flow_arn, "name": name }})
}

/// Create one cloud-to-ground (egress) bridge carrying `sources` and answer its ARN.
///
/// A flow source is the kind an egress bridge ingests, and a bridge holding a PAIR of them
/// also carries the failover config that lets it ("if the state is active, [it] can have one
/// or two sources", `FailoverConfig$State`). The create is `bridge_lifecycle`'s subject; here
/// it is only the fixture every row addresses.
async fn egress_bridge(emulator: &Emulator, name: &str, sources: Vec<Value>) -> String {
    let placement_arn = gateway_with_networks(emulator, &format!("gw-{name}"), &["net-1"]).await;
    let mut body = json!({
        "name": name,
        "placementArn": placement_arn,
        "egressGatewayBridge": { "maxBitrate": 20_000_000 },
        "sources": sources,
    });
    if sources.len() > 1 {
        body["sourceFailoverConfig"] = json!({ "state": mc::STATE_ENABLED });
    }
    let (status, code, created) = create_bridge(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "fixture bridge {name:?}: create expected 201, got {status} {code:?}: {created}"
    );
    created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture bridge {name:?}: the create answers an ARN: {created}"))
        .to_string()
}

/// The `OutputArn` the bridge `bridge_arn` reports on the source it addresses by
/// `source_name`, or `None` when the source carries none.
async fn reported_output_arn(
    emulator: &Emulator,
    case: &str,
    bridge_arn: &str,
    source_name: &str,
) -> Option<String> {
    let (status, code, described) = describe_bridge(emulator, bridge_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeBridge must answer 200, got {status} {code:?}: {described}"
    );
    let sources = described["bridge"]["sources"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let source = sources
        .iter()
        .find(|source| source["flowSource"]["name"].as_str() == Some(source_name))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the bridge holds a flow source {source_name:?}, got {sources:?}")
        });
    source["flowSource"]["outputArn"]
        .as_str()
        .map(str::to_string)
}

/// The outputs of `flow_arn` that carry `bridge_arn` — the ones the service created for that
/// bridge's sources, as DescribeFlow reports them.
async fn outputs_for_bridge(
    emulator: &Emulator,
    case: &str,
    flow_arn: &str,
    bridge_arn: &str,
) -> Vec<Value> {
    described_outputs(emulator, case, flow_arn)
        .await
        .into_iter()
        .filter(|output| output["bridgeArn"].as_str() == Some(bridge_arn))
        .collect()
}

/// Assert the whole rule for one stored bridge flow source: the source carries an
/// `OutputArn`, the flow it names holds exactly that output, and that output is the one the
/// service creates for a bridge — named `expected_output_name`, carrying the bridge's ARN,
/// enabled, and addressed by an ARN of the form this service mints. Answers the ARN.
async fn assert_output_for_source(
    emulator: &Emulator,
    case: &str,
    bridge_arn: &str,
    flow_arn: &str,
    source_name: &str,
    expected_output_name: &str,
) -> String {
    let output_arn = reported_output_arn(emulator, case, bridge_arn, source_name)
        .await
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the flow source {source_name:?} carries the ARN of the output \
                 its cloud flow delivers through"
            )
        });
    assert_minted_output_arn(case, &output_arn, expected_output_name);

    let outputs = described_outputs(emulator, case, flow_arn).await;
    let output = outputs
        .iter()
        .find(|output| output["outputArn"].as_str() == Some(output_arn.as_str()))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the flow holds the output {output_arn:?} its bridge source \
                 names, got {outputs:?}"
            )
        });
    assert_eq!(
        (
            output["name"].as_str(),
            output["bridgeArn"].as_str(),
            output["outputStatus"].as_str(),
        ),
        (
            Some(expected_output_name),
            Some(bridge_arn),
            Some(mc::STATUS_ENABLED),
        ),
        "case {case:?}: the output is named after the bridge source, names the bridge it was \
         added for and is transmitting, got {output}"
    );
    output_arn
}

/// Which call stored the bridge flow source. The three paths that write one onto a bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StorePath {
    /// CreateBridge's `Sources` — "the sources that you want to add to this bridge".
    CreateBridge,
    /// AddBridgeSources, appending the source to a bridge that already holds one.
    AddBridgeSources,
    /// UpdateBridgeSource, pointing a source the bridge already holds at the flow.
    UpdateBridgeSource,
}

/// One row of the materialisation table: which call stored the source, under which name, and
/// how many sources of that bridge name the flow.
struct MaterialiseCase {
    case: &'static str,
    path: StorePath,
    source_names: &'static [&'static str],
}

/// Whichever call stores a bridge source naming a cloud flow, that flow then holds the
/// output the service created for the source, and the source carries its ARN.
#[tokio::test]
async fn every_path_that_takes_a_cloud_flow_as_a_bridge_source_gives_the_flow_its_output() {
    let emulator = Emulator::new();

    let rows = [
        MaterialiseCase {
            case: "CreateBridge's Sources, one flow source",
            path: StorePath::CreateBridge,
            source_names: &["from-flow"],
        },
        MaterialiseCase {
            case: "CreateBridge's Sources, the failover pair both naming one flow",
            path: StorePath::CreateBridge,
            source_names: &["primary", "secondary"],
        },
        MaterialiseCase {
            case: "AddBridgeSources, appended to a bridge that already holds a source",
            path: StorePath::AddBridgeSources,
            source_names: &["added-from-flow"],
        },
        MaterialiseCase {
            case: "UpdateBridgeSource, pointing a stored source at the flow",
            path: StorePath::UpdateBridgeSource,
            source_names: &["repointed"],
        },
    ];

    for (index, MaterialiseCase { case, path, source_names }) in rows.iter().enumerate() {
        // One flow and one bridge per row, so each row's flow reports only the outputs its
        // own bridge sources asked for.
        let flow_arn = cloud_flow(&emulator, &format!("flow-materialise-{index}")).await;
        let bridge_arn = match path {
            StorePath::CreateBridge => {
                let sources: Vec<Value> = source_names
                    .iter()
                    .map(|name| flow_source(name, &flow_arn))
                    .collect();
                egress_bridge(&emulator, &format!("br-materialise-{index}"), sources).await
            }
            StorePath::AddBridgeSources => {
                // A bridge holding a PAIR of sources needs failover active, so the fixture
                // bridge is created with the pair's first source already on it.
                let held = cloud_flow(&emulator, &format!("flow-held-{index}")).await;
                let bridge_arn = egress_bridge(
                    &emulator,
                    &format!("br-materialise-{index}"),
                    vec![flow_source("held", &held), flow_source("spare", &held)],
                )
                .await;
                // The spare makes room for the add: the bound counts the sources the bridge
                // already holds plus the ones the request adds.
                let (status, code, removed) =
                    remove_bridge_source(&emulator, &bridge_arn, "spare").await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: the spare source must be removed, got {status} {code:?}: \
                     {removed}"
                );
                let sources: Vec<Value> = source_names
                    .iter()
                    .map(|name| flow_source(name, &flow_arn))
                    .collect();
                let (status, code, added) =
                    add_bridge_sources(&emulator, &bridge_arn, json!(sources)).await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: AddBridgeSources must append the source, got {status} \
                     {code:?}: {added}"
                );
                // The answer is the stored source, so it already carries the ARN the
                // reconciliation wrote.
                for name in *source_names {
                    let answered = added["sources"]
                        .as_array()
                        .and_then(|sources| {
                            sources
                                .iter()
                                .find(|source| source["flowSource"]["name"].as_str() == Some(name))
                                .cloned()
                        })
                        .unwrap_or_default();
                    assert_eq!(
                        answered["flowSource"]["outputArn"].as_str(),
                        reported_output_arn(&emulator, case, &bridge_arn, name)
                            .await
                            .as_deref(),
                        "case {case:?}: the add answers the source the bridge now holds, got \
                         {added}"
                    );
                }
                bridge_arn
            }
            StorePath::UpdateBridgeSource => {
                let other = cloud_flow(&emulator, &format!("flow-other-{index}")).await;
                let name = source_names[0];
                let bridge_arn = egress_bridge(
                    &emulator,
                    &format!("br-materialise-{index}"),
                    vec![flow_source(name, &other)],
                )
                .await;
                let (status, code, updated) = update_bridge_source(
                    &emulator,
                    &bridge_arn,
                    name,
                    json!({ "flowSource": { "flowArn": flow_arn } }),
                )
                .await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: the source must be repointed, got {status} {code:?}: \
                     {updated}"
                );
                // The output follows the reference: the flow the source NOW names holds it,
                // and the one it no longer names holds none for this bridge.
                let left_behind = outputs_for_bridge(&emulator, case, &other, &bridge_arn).await;
                assert!(
                    left_behind.is_empty(),
                    "case {case:?}: the flow the source no longer names holds no output for \
                     the bridge, got {left_behind:?}"
                );
                bridge_arn
            }
        };

        let mut arns = Vec::new();
        for name in *source_names {
            // Each source is named by the request, so the output it asks for takes that
            // name: no two sources of one bridge share one.
            arns.push(
                assert_output_for_source(&emulator, case, &bridge_arn, &flow_arn, name, name).await,
            );
        }
        let for_bridge = outputs_for_bridge(&emulator, case, &flow_arn, &bridge_arn).await;
        assert_eq!(
            for_bridge.len(),
            source_names.len(),
            "case {case:?}: the flow holds one output per bridge source that names it, got \
             {for_bridge:?}"
        );
        assert_eq!(
            arns.len(),
            arns.iter().collect::<std::collections::BTreeSet<_>>().len(),
            "case {case:?}: each source carries the ARN of its OWN output, got {arns:?}"
        );
    }
}

/// Two bridges may take one flow at once, and a bridge's two sources may name two flows: the
/// store holds one output per (bridge source, flow) pair and each source carries its own.
#[tokio::test]
async fn one_flow_feeds_every_bridge_source_that_names_it_through_an_output_of_its_own() {
    let emulator = Emulator::new();
    let case = "one flow taken by two bridges, and a bridge taking two flows";
    let shared = cloud_flow(&emulator, "flow-shared").await;
    let second = cloud_flow(&emulator, "flow-second").await;

    // Two bridges whose sources share a NAME as well as the flow: an output's name is
    // unique within its flow, so the second takes the next name the flow does not hold.
    let first_bridge = egress_bridge(
        &emulator,
        "br-shared-a",
        vec![flow_source("feed", &shared)],
    )
    .await;
    let second_bridge = egress_bridge(
        &emulator,
        "br-shared-b",
        vec![flow_source("feed", &shared), flow_source("other", &second)],
    )
    .await;

    let first_arn =
        assert_output_for_source(&emulator, case, &first_bridge, &shared, "feed", "feed").await;
    let second_arn =
        assert_output_for_source(&emulator, case, &second_bridge, &shared, "feed", "feed-2").await;
    assert_ne!(
        first_arn, second_arn,
        "case {case:?}: each bridge source gets an output of its own, got {first_arn:?} and \
         {second_arn:?}"
    );
    // The same bridge's other source names ANOTHER flow, so its output is on that flow.
    assert_output_for_source(&emulator, case, &second_bridge, &second, "other", "other").await;
    let on_shared = outputs_for_bridge(&emulator, case, &shared, &second_bridge).await;
    assert_eq!(
        on_shared.len(),
        1,
        "case {case:?}: only the source that names this flow has an output on it, got \
         {on_shared:?}"
    );
}

/// The materialised output is named after the bridge source, against the names the flow
/// already addresses its outputs by — including names a request put there.
#[tokio::test]
async fn the_output_is_named_against_the_names_the_flow_already_holds() {
    let emulator = Emulator::new();
    let case = "an output a request already added under the source's name";
    let flow_arn = cloud_flow(&emulator, "flow-naming").await;
    let (status, code, added) = add_flow_outputs(
        &emulator,
        &flow_arn,
        json!([{ "name": "taken", "protocol": "rtp", "destination": "10.0.0.9", "port": 5100 }]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the fixture output must be added, got {status} {code:?}: {added}"
    );

    // Each row names its source `taken`, so each materialised output takes the next free
    // name: the request's output holds `taken`, then `taken-2`, then `taken-3`.
    for (index, expected_name) in ["taken-2", "taken-3"].into_iter().enumerate() {
        let bridge_arn = egress_bridge(
            &emulator,
            &format!("br-naming-{index}"),
            vec![flow_source("taken", &flow_arn)],
        )
        .await;
        assert_output_for_source(
            &emulator,
            case,
            &bridge_arn,
            &flow_arn,
            "taken",
            expected_name,
        )
        .await;
    }
    // The output the request configured is untouched by any of it, and carries no bridge.
    let outputs = described_outputs(&emulator, case, &flow_arn).await;
    let configured = outputs
        .iter()
        .find(|output| output["name"].as_str() == Some("taken"))
        .unwrap_or_else(|| panic!("case {case:?}: the flow keeps the output it was given"));
    assert_eq!(
        (
            configured["bridgeArn"].as_str(),
            configured["destination"].as_str(),
        ),
        (None, Some("10.0.0.9")),
        "case {case:?}: an output a request configured names no bridge and keeps its \
         settings, got {configured}"
    );
}

/// How a bridge source stops naming the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClearPath {
    /// RemoveBridgeSource takes the source out of the bridge.
    RemoveBridgeSource,
    /// UpdateBridgeSource points the same source at ANOTHER flow.
    RepointToAnotherFlow,
    /// DeleteBridge takes the whole bridge, sources and all.
    DeleteBridge,
}

/// A flow's bridge output lives exactly as long as the bridge source that asked for it: when
/// that source stops naming the flow, the flow stops reporting the output — whichever call
/// cleared it — while the outputs a request configured stay.
#[tokio::test]
async fn the_output_goes_when_the_bridge_source_stops_naming_the_flow() {
    let emulator = Emulator::new();

    let rows = [
        (
            "RemoveBridgeSource takes the source out of the bridge",
            ClearPath::RemoveBridgeSource,
        ),
        (
            "UpdateBridgeSource points the source at another flow",
            ClearPath::RepointToAnotherFlow,
        ),
        (
            "DeleteBridge takes the bridge and its sources",
            ClearPath::DeleteBridge,
        ),
    ];

    for (index, (case, path)) in rows.into_iter().enumerate() {
        let flow_arn = cloud_flow(&emulator, &format!("flow-cleared-{index}")).await;
        let (status, code, added) = add_flow_outputs(
            &emulator,
            &flow_arn,
            json!([{ "name": "kept", "protocol": "rtp", "destination": "10.0.0.8", "port": 5200 }]),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the fixture output must be added, got {status} {code:?}: {added}"
        );
        // A bridge whose source is REMOVED keeps a second source, so every row works on the
        // same failover pair and the rows stay comparable.
        let spare = cloud_flow(&emulator, &format!("flow-spare-{index}")).await;
        let bridge_arn = egress_bridge(
            &emulator,
            &format!("br-cleared-{index}"),
            vec![
                flow_source("cleared", &flow_arn),
                flow_source("spare", &spare),
            ],
        )
        .await;
        let output_arn = assert_output_for_source(
            &emulator,
            case,
            &bridge_arn,
            &flow_arn,
            "cleared",
            "cleared",
        )
        .await;

        match path {
            ClearPath::RemoveBridgeSource => {
                let (status, code, removed) =
                    remove_bridge_source(&emulator, &bridge_arn, "cleared").await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: the source must be removed, got {status} {code:?}: {removed}"
                );
            }
            ClearPath::RepointToAnotherFlow => {
                let other = cloud_flow(&emulator, &format!("flow-repointed-{index}")).await;
                let (status, code, updated) = update_bridge_source(
                    &emulator,
                    &bridge_arn,
                    "cleared",
                    json!({ "flowSource": { "flowArn": other } }),
                )
                .await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: the source must be repointed, got {status} {code:?}: \
                     {updated}"
                );
                // The output follows the reference onto the flow the source now names.
                assert_output_for_source(
                    &emulator,
                    case,
                    &bridge_arn,
                    &other,
                    "cleared",
                    "cleared",
                )
                .await;
            }
            ClearPath::DeleteBridge => {
                let (status, code, deleted) = delete_bridge(&emulator, &bridge_arn).await;
                assert_eq!(
                    (status, code.as_deref()),
                    (200, None),
                    "case {case:?}: the bridge must be deleted, got {status} {code:?}: {deleted}"
                );
            }
        }

        let after = described_outputs(&emulator, case, &flow_arn).await;
        assert!(
            after
                .iter()
                .all(|output| output["outputArn"].as_str() != Some(output_arn.as_str())),
            "case {case:?}: the flow holds no output once no bridge source asks for it, got \
             {after:?}"
        );
        assert_eq!(
            after.len(),
            1,
            "case {case:?}: the output the request configured stays, got {after:?}"
        );
        assert_eq!(
            after[0]["name"].as_str(),
            Some("kept"),
            "case {case:?}: the output left is the one a request added, got {after:?}"
        );
    }
}

/// The output a bridge source asked for is the service's own: no request removes it, while
/// an output a request DID configure on the same flow stays removable.
#[tokio::test]
async fn no_request_removes_the_output_a_bridge_source_asked_for() {
    let emulator = Emulator::new();
    let case = "the output a cloud-to-ground bridge's flow source was given";
    let flow_arn = cloud_flow(&emulator, "flow-refusal").await;
    let (status, code, added) = add_flow_outputs(
        &emulator,
        &flow_arn,
        json!([{ "name": "configured", "protocol": "rtp", "destination": "10.0.0.7", "port": 5300 }]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the fixture output must be added, got {status} {code:?}: {added}"
    );
    let configured_arn = added["outputs"][0]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let bridge_arn = egress_bridge(
        &emulator,
        "br-refusal",
        vec![flow_source("fed-bridge", &flow_arn)],
    )
    .await;
    let output_arn = assert_output_for_source(
        &emulator,
        case,
        &bridge_arn,
        &flow_arn,
        "fed-bridge",
        "fed-bridge",
    )
    .await;

    let (status, code, refused) = remove_flow_output(&emulator, &flow_arn, &output_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: the output a bridge source asked for is not one a request removes, \
         got {status} {code:?}: {refused}"
    );
    let message = refused["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::BAD_REQUEST_PREFIX),
        "case {case:?}: a well-formed request the flow does not admit keeps this service's \
         bad-request envelope, got {refused}"
    );
    assert!(
        message.contains(&bridge_arn) && message.contains("fed-bridge"),
        "case {case:?}: the refusal names the bridge and the source the output belongs to, \
         got {refused}"
    );
    // The refused removal changed nothing: the source still carries the same ARN.
    assert_eq!(
        reported_output_arn(&emulator, case, &bridge_arn, "fed-bridge").await,
        Some(output_arn.clone()),
        "case {case:?}: a refused removal leaves the record alone"
    );

    // The refusal is keyed to the KIND of output, not to the operation: the output a
    // request configured on the same flow is still removed by the request that addresses it.
    let (status, code, removed) = remove_flow_output(&emulator, &flow_arn, &configured_arn).await;
    assert_eq!(
        (status, code.as_deref(), removed["outputArn"].as_str()),
        (202, None, Some(configured_arn.as_str())),
        "case {case:?}: an output a request configured is removed by the request that \
         addresses it, got {status} {code:?}: {removed}"
    );
    let after = described_outputs(&emulator, case, &flow_arn).await;
    assert_eq!(
        after.len(),
        1,
        "case {case:?}: the flow keeps only the output its bridge source asked for, got \
         {after:?}"
    );
}

/// A later write to the bridge's sources leaves an output already materialised exactly as it
/// was: the reconciliation neither remints nor renames one, so an `OutputArn` a caller read
/// stays the ARN of the same output.
#[tokio::test]
async fn a_later_bridge_source_write_leaves_the_materialised_output_alone() {
    let emulator = Emulator::new();
    let case = "a second source added beside one already materialised";
    let flow_arn = cloud_flow(&emulator, "flow-stable").await;
    // The bridge is created with the failover pair it may hold, so there is room to add a
    // second source back after it is removed ("if the state is active, the bridge can have
    // one or two sources", `FailoverConfig$State`).
    let bridge_arn = egress_bridge(
        &emulator,
        "br-stable",
        vec![
            flow_source("first", &flow_arn),
            flow_source("second", &flow_arn),
        ],
    )
    .await;
    let before =
        assert_output_for_source(&emulator, case, &bridge_arn, &flow_arn, "first", "first").await;

    // Every later write that runs the reconciliation: the second source removed, added
    // again, repointed at another flow, and removed once more.
    let other = cloud_flow(&emulator, "flow-stable-other").await;
    let (status, code, removed) = remove_bridge_source(&emulator, &bridge_arn, "second").await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the second source must be removed, got {status} {code:?}: {removed}"
    );
    let (status, code, added) = add_bridge_sources(
        &emulator,
        &bridge_arn,
        json!([flow_source("second", &flow_arn)]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the second source must be added, got {status} {code:?}: {added}"
    );
    let (status, code, updated) = update_bridge_source(
        &emulator,
        &bridge_arn,
        "second",
        json!({ "flowSource": { "flowArn": other } }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the second source must be repointed, got {status} {code:?}: {updated}"
    );
    let (status, code, removed) = remove_bridge_source(&emulator, &bridge_arn, "second").await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the second source must be removed, got {status} {code:?}: {removed}"
    );

    let after =
        assert_output_for_source(&emulator, case, &bridge_arn, &flow_arn, "first", "first").await;
    assert_eq!(
        after, before,
        "case {case:?}: the output the first source asked for keeps its ARN across every \
         later write, got {after:?}"
    );
    let for_bridge = outputs_for_bridge(&emulator, case, &flow_arn, &bridge_arn).await;
    assert_eq!(
        for_bridge.len(),
        1,
        "case {case:?}: the reconciliation neither duplicates nor leaves behind an output, \
         got {for_bridge:?}"
    );
}

/// A router input may be connected to any output of a flow, including the one a bridge source
/// asked for — so when that source stops asking, the reference goes with the output and the
/// router input reports the DISCONNECTED configuration the model defines ("to disconnect it …
/// omit both fields while keeping this configuration object",
/// `smithy_query(query="shape",shape="MediaConnectFlowRouterInputConfiguration")`), rather
/// than an `OutputArn` nothing resolves.
#[tokio::test]
async fn a_router_input_connected_to_the_output_is_disconnected_when_the_source_goes() {
    let emulator = Emulator::new();

    let rows = [
        (
            "RemoveBridgeSource takes the source out of the bridge",
            ClearPath::RemoveBridgeSource,
        ),
        (
            "UpdateBridgeSource points the source at another flow",
            ClearPath::RepointToAnotherFlow,
        ),
        (
            "DeleteBridge takes the bridge and its sources",
            ClearPath::DeleteBridge,
        ),
    ];

    for (index, (case, path)) in rows.into_iter().enumerate() {
        let flow_arn = cloud_flow(&emulator, &format!("flow-routed-{index}")).await;
        let spare = cloud_flow(&emulator, &format!("flow-routed-spare-{index}")).await;
        let bridge_arn = egress_bridge(
            &emulator,
            &format!("br-routed-{index}"),
            vec![
                flow_source("routed", &flow_arn),
                flow_source("spare", &spare),
            ],
        )
        .await;
        let output_arn =
            assert_output_for_source(&emulator, case, &bridge_arn, &flow_arn, "routed", "routed")
                .await;

        // A router input takes its content FROM a flow output, so the connection it is
        // created with names this output.
        let (status, code, created) = create_router_input(
            &emulator,
            input_create_body(
                &format!("ri-routed-{index}"),
                flow_input_configuration(Some(&flow_arn), Some(&output_arn)),
            ),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the router input must be created connected to the output, got \
             {status} {code:?}: {created}"
        );
        let router_input_arn = created["routerInput"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        match path {
            ClearPath::RemoveBridgeSource => {
                let (status, code, removed) =
                    remove_bridge_source(&emulator, &bridge_arn, "routed").await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: the source must be removed, got {status} {code:?}: {removed}"
                );
            }
            ClearPath::RepointToAnotherFlow => {
                let other = cloud_flow(&emulator, &format!("flow-routed-other-{index}")).await;
                let (status, code, updated) = update_bridge_source(
                    &emulator,
                    &bridge_arn,
                    "routed",
                    json!({ "flowSource": { "flowArn": other } }),
                )
                .await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: the source must be repointed, got {status} {code:?}: \
                     {updated}"
                );
            }
            ClearPath::DeleteBridge => {
                let (status, code, deleted) = delete_bridge(&emulator, &bridge_arn).await;
                assert_eq!(
                    (status, code.as_deref()),
                    (200, None),
                    "case {case:?}: the bridge must be deleted, got {status} {code:?}: {deleted}"
                );
            }
        }

        let (status, code, read) = get_router_input(&emulator, &router_input_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the router input must still be readable, got {status} {code:?}: \
             {read}"
        );
        let configuration = &read["routerInput"]["configuration"]["MediaConnectFlow"];
        assert_eq!(
            (
                configuration["flowOutputArn"].as_str(),
                configuration["flowArn"].as_str(),
            ),
            (None, Some(flow_arn.as_str())),
            "case {case:?}: the connection to the output goes with the output, while the flow \
             the input still names stays, got {read}"
        );
    }
}
