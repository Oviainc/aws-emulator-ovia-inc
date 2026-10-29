// SPDX-License-Identifier: CC0-1.0

//! What keeps a stored flow source from naming a bridge the store does not hold, end to end
//! over HTTP.
//!
//! The rule the cases below pin is this service's referential integrity for a REQUIRED
//! reference: a resource is not taken out of the store while another stored record names it
//! in a member the model makes required and the write paths resolve against the store — the
//! removal is refused with the modelled semantic-error exception (`BadRequestException`, 400,
//! "the precise meaning depends on the API, and is documented in the error message"), stating
//! what still names it, and the caller clears the reference and retries. The reference here is
//! the bridge a flow's source takes its content from (`GatewayBridgeSource$BridgeArn`,
//! required — `smithy_query(query="shape",shape="GatewayBridgeSource")` — "the ARN of the
//! bridge feeding this flow"), which CreateFlow's `Source` and `Sources`, AddFlowSources and
//! UpdateFlowSource all resolve against the account's bridges rather than taking on trust.
//! Where the reference is OPTIONAL instead — a router input's or router output's connection to
//! a flow — the removal DROPS it and is served, which is
//! `crate::flow_removal_clears_router_references`' subject; a required member cannot be dropped
//! off a record, so the removal is refused instead, exactly as it is for the cloud flow a
//! bridge source names (`crate::flow_deletion_bridge_source_integrity`) and for the router
//! network interface a router record ingests on
//! (`crate::router_network_interface_association_integrity`).
//!
//! The rows differ along what the rule generalises over: how many flows name the bridge (none,
//! one, two), how many sources of one flow do (one of two sources naming it), what the other
//! flows hold instead (a source fed by another bridge the account holds, a plain source naming
//! no bridge at all), and how the caller clears the reference (removing the flow source,
//! pointing it at another bridge, deleting the flow).
//!
//! Every call travels the whole request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], and each case ends by asserting the store holds NO
//! flow source naming a bridge it does not hold — the state a delete must never produce.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{bridge_feeding_a_flow, describe_bridge};
use crate::flow_lifecycle::Emulator;
// The source fed by no bridge at all — the kind that references nothing of the account — is
// the test crate's one encoding of a plain source.
use crate::flow_ndi_sizing::plain_source;
use crate::flow_sources::{
    create_flow_from, described_sources, failover_active, remove_flow_source,
};
use crate::update_flow_source::update_flow_source;

/// The sentence a refused delete must carry, built here rather than read out of the crate, so
/// the wording is pinned by the test and not by the code under test.
///
/// It names the bridge the request addressed and states how many flows still take their
/// content from it, which is what the caller has to clear before retrying.
fn expected_refusal(bridge_arn: &str, flows: usize) -> String {
    format!(
        "{}Bridge {bridge_arn} is still the source of {flows} flow(s). Remove those flow \
         sources, point them at another bridge, or delete the flows, before deleting the \
         bridge.",
        mc::BAD_REQUEST_PREFIX
    )
}

/// `DELETE /v1/bridges/{BridgeArn}`.
async fn delete_bridge(emulator: &Emulator, bridge_arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/bridges/{bridge_arn}"), None)
        .await
}

/// `DELETE /v1/flows/{FlowArn}`.
async fn delete_flow(emulator: &Emulator, flow_arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}"), None)
        .await
}

/// A source fed by `bridge_arn`, as a CreateFlow request configures one.
fn bridge_source(name: &str, bridge_arn: &str) -> Value {
    json!({ "name": name, "gatewayBridgeSource": { "bridgeArn": bridge_arn } })
}

/// Create one flow over the wire carrying `sources`, and answer its ARN.
///
/// A flow holds more than one source only while its source failover is active ("if the state
/// is active, the flow can have one or two sources", `FailoverConfig$State`), so the create
/// states that configuration exactly when the row needs it.
async fn created_flow(
    emulator: &Emulator,
    case: &str,
    name: &str,
    sources: Vec<Value>,
) -> String {
    let mut body = json!({ "name": name, "sources": sources });
    if body["sources"].as_array().map(Vec::len).unwrap_or_default() > 1 {
        body["sourceFailoverConfig"] = failover_active();
    }
    create_flow_from(emulator, case, body).await
}

/// The `SourceArn` of the flow's source that names `bridge_arn`.
async fn source_fed_by(emulator: &Emulator, case: &str, flow_arn: &str, bridge_arn: &str) -> String {
    let sources = described_sources(emulator, case, flow_arn).await;
    sources
        .iter()
        .find(|source| source["gatewayBridgeSource"]["bridgeArn"].as_str() == Some(bridge_arn))
        .unwrap_or_else(|| {
            panic!("case {case:?}: a source of the flow names the bridge, got {sources:?}")
        })["sourceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every stored flow source naming a bridge the store does NOT hold, as
/// `(flow ARN, bridge ARN)`.
///
/// The reference is read through the crate's own reading of that relation
/// ([`mc::ComAmazonawsMediaconnectMediaconnectState::holds_bridge`]), the same one every write
/// path resolves a bridge reference through, so this sees exactly what the service resolves.
fn dangling_flow_bridge_references(emulator: &Emulator) -> Vec<(String, String)> {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    let mut dangling = Vec::new();
    for (flow_arn, flow) in store.flows.iter() {
        for source in &flow.sources {
            if let Some(gateway_bridge_source) = source.gateway_bridge_source.as_ref() {
                if !store.holds_bridge(&gateway_bridge_source.bridge_arn) {
                    dangling.push((
                        flow_arn.clone(),
                        gateway_bridge_source.bridge_arn.clone(),
                    ));
                }
            }
        }
    }
    dangling
}

/// Assert the invariant this whole suite is about: no stored flow source names a bridge the
/// store does not hold.
fn assert_no_dangling_references(emulator: &Emulator, case: &str) {
    let dangling = dangling_flow_bridge_references(emulator);
    assert!(
        dangling.is_empty(),
        "case {case:?}: no stored flow source may name a bridge the store does not hold, got \
         {dangling:?}"
    );
}

/// Whether DescribeBridge still answers for `bridge_arn`.
async fn bridge_is_held(emulator: &Emulator, case: &str, bridge_arn: &str) -> bool {
    let (status, code, described) = describe_bridge(emulator, bridge_arn).await;
    assert!(
        status == 200 || (status, code.as_deref()) == (404, Some("NotFoundException")),
        "case {case:?}: DescribeBridge answers either the bridge or a not-found, got {status} \
         {code:?}: {described}"
    );
    status == 200
}

/// The bridge as DescribeBridge reports it, without the one member the CLOCK moves on its own
/// (`Bridge$BridgeState`: a created bridge settles into `STANDBY` a transition span later), so
/// comparing a record before and against after a refused call compares what the CALL wrote.
async fn bridge_record_without_state(emulator: &Emulator, case: &str, bridge_arn: &str) -> Value {
    let (status, code, described) = describe_bridge(emulator, bridge_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the bridge reads back, got {status} {code:?}: {described}"
    );
    let mut bridge = described["bridge"].clone();
    if let Some(members) = bridge.as_object_mut() {
        members.remove("bridgeState");
    }
    bridge
}

/// One row of the refusal table: the shape of the store a delete is attempted against.
struct Row {
    case: &'static str,
    /// How many flows hold a source fed by the bridge under test.
    naming_flows: usize,
    /// How many of those hold a SECOND, plain source — so the refusal counts flows rather
    /// than sources.
    with_second_source: usize,
    /// How many flows hold a source fed by ANOTHER bridge the account holds, and nothing else.
    other_bridge_flows: usize,
    /// How many flows hold a plain source, which names no bridge at all.
    plain_flows: usize,
}

/// The rows span the class: a store with no flow, with one and with two flows naming the
/// bridge, with a flow naming it among its two sources, and with flows that name another
/// bridge or no bridge at all — the last being the rows the delete must still be served for.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no flow at all",
            naming_flows: 0,
            with_second_source: 0,
            other_bridge_flows: 0,
            plain_flows: 0,
        },
        Row {
            case: "a flow naming another bridge, and one with a plain source",
            naming_flows: 0,
            with_second_source: 0,
            other_bridge_flows: 1,
            plain_flows: 1,
        },
        Row {
            case: "one flow naming the bridge",
            naming_flows: 1,
            with_second_source: 0,
            other_bridge_flows: 0,
            plain_flows: 0,
        },
        Row {
            case: "two flows naming the bridge",
            naming_flows: 2,
            with_second_source: 0,
            other_bridge_flows: 0,
            plain_flows: 0,
        },
        Row {
            case: "one flow naming the bridge in one of its two sources",
            naming_flows: 1,
            with_second_source: 1,
            other_bridge_flows: 0,
            plain_flows: 0,
        },
        Row {
            case: "one flow naming the bridge beside flows that do not",
            naming_flows: 1,
            with_second_source: 0,
            other_bridge_flows: 1,
            plain_flows: 1,
        },
    ]
}

/// The delete of a bridge a stored flow source takes its content from is refused, however many
/// flows name it and whatever else is in the store — and the refused call changes nothing: the
/// bridge is still there, and the reference that was protected still resolves, so the flow
/// source can still be re-stated through the update path.
#[tokio::test]
async fn a_bridge_a_flow_source_names_is_not_deleted() {
    for Row {
        case,
        naming_flows,
        with_second_source,
        other_bridge_flows,
        plain_flows,
    } in rows()
    {
        let emulator = Emulator::new();
        let bridge_arn = bridge_feeding_a_flow(&emulator, "br-under-test").await;
        // A second bridge nothing deletes: what the bystander flows name.
        let other_bridge = bridge_feeding_a_flow(&emulator, "br-bystander").await;

        let mut naming: Vec<String> = Vec::new();
        for index in 0..naming_flows {
            let mut sources = vec![bridge_source("from-bridge", &bridge_arn)];
            if index < with_second_source {
                sources.push(plain_source("plain"));
            }
            naming.push(
                created_flow(
                    &emulator,
                    case,
                    &format!("flow-naming-{index}"),
                    sources,
                )
                .await,
            );
        }
        for index in 0..other_bridge_flows {
            created_flow(
                &emulator,
                case,
                &format!("flow-other-bridge-{index}"),
                vec![bridge_source("from-bridge", &other_bridge)],
            )
            .await;
        }
        for index in 0..plain_flows {
            created_flow(
                &emulator,
                case,
                &format!("flow-plain-{index}"),
                vec![plain_source("plain")],
            )
            .await;
        }

        let before = bridge_record_without_state(&emulator, case, &bridge_arn).await;

        let (status, code, answered) = delete_bridge(&emulator, &bridge_arn).await;
        if naming_flows == 0 {
            assert_eq!(
                (status, code.as_deref(), answered["bridgeArn"].as_str()),
                (200, None, Some(bridge_arn.as_str())),
                "case {case:?}: a bridge no flow source names is deleted and answers its own \
                 ARN, got {status} {code:?}: {answered}"
            );
            assert!(
                !bridge_is_held(&emulator, case, &bridge_arn).await,
                "case {case:?}: the deleted bridge stopped existing"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: deleting a bridge a flow source names is refused, got {status} \
                 {code:?}: {answered}"
            );
            assert_eq!(
                answered["message"].as_str(),
                Some(expected_refusal(&bridge_arn, naming_flows).as_str()),
                "case {case:?}: expected {:?}, got {answered}",
                expected_refusal(&bridge_arn, naming_flows)
            );
            // The refused call wrote nothing on the bridge, and the reference it kept
            // resolvable can still be re-stated through the update path — which is what a
            // caller does with the configuration a read reports.
            let after = bridge_record_without_state(&emulator, case, &bridge_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: the refused delete left the bridge exactly as it was, got \
                 {after}"
            );
            for flow_arn in &naming {
                let source_arn = source_fed_by(&emulator, case, flow_arn, &bridge_arn).await;
                let (status, code, restated) = update_flow_source(
                    &emulator,
                    flow_arn,
                    &source_arn,
                    json!({ "gatewayBridgeSource": { "bridgeArn": bridge_arn } }),
                )
                .await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: re-stating the bridge the protected source names expected \
                     202, got {status} {code:?}: {restated}"
                );
            }
        }

        // The bystander bridge is untouched either way: the delete addressed another bridge.
        assert!(
            bridge_is_held(&emulator, case, &other_bridge).await,
            "case {case:?}: the bridge the delete did not address is still held"
        );

        assert_no_dangling_references(&emulator, case);
    }
}

/// How a caller clears a flow's reference to the bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Clearing {
    /// RemoveFlowSource takes the naming source off the flow.
    RemoveSource,
    /// UpdateFlowSource points it at another bridge the account holds.
    RepointSource,
    /// DeleteFlow takes the whole flow away.
    DeleteFlow,
}

/// The delete is served as soon as nothing names the bridge, and the count the refusal states
/// walks down with the store: with two flows naming the bridge it is refused stating 2, then 1
/// once one of them is cleared, and served once the second is — whichever of the three ways of
/// clearing the reference the caller uses.
#[tokio::test]
async fn the_bridge_delete_is_served_once_no_flow_source_names_it() {
    for clearing in [
        Clearing::RemoveSource,
        Clearing::RepointSource,
        Clearing::DeleteFlow,
    ] {
        let case = format!("cleared by {clearing:?}");
        let case = case.as_str();
        let emulator = Emulator::new();
        let bridge_arn = bridge_feeding_a_flow(&emulator, "br-cleared").await;
        let other_bridge = bridge_feeding_a_flow(&emulator, "br-repointed-to").await;

        let mut naming: Vec<String> = Vec::new();
        for index in 0..2 {
            // Each flow carries a second, plain source: "this request can be made only if
            // there is more than one source on the flow" (RemoveFlowSource), so the rows stay
            // comparable across the three ways of clearing.
            naming.push(
                created_flow(
                    &emulator,
                    case,
                    &format!("flow-clear-{index}"),
                    vec![
                        bridge_source("from-bridge", &bridge_arn),
                        plain_source("plain"),
                    ],
                )
                .await,
            );
        }

        for (cleared, still_naming) in [(0usize, 2usize), (1, 1)] {
            // Before clearing this one, the delete is refused stating how many flows still
            // name the bridge.
            let (status, code, refused) = delete_bridge(&emulator, &bridge_arn).await;
            assert_eq!(
                (status, code.as_deref(), refused["message"].as_str()),
                (
                    400,
                    Some("BadRequestException"),
                    Some(expected_refusal(&bridge_arn, still_naming).as_str())
                ),
                "case {case:?}: with {still_naming} flow(s) naming it the delete is refused \
                 stating {still_naming}, got {status} {code:?}: {refused}"
            );

            let flow_arn = &naming[cleared];
            let source_arn = source_fed_by(&emulator, case, flow_arn, &bridge_arn).await;
            let (status, code, answered) = match clearing {
                Clearing::RemoveSource => {
                    remove_flow_source(&emulator, flow_arn, &source_arn).await
                }
                Clearing::RepointSource => {
                    update_flow_source(
                        &emulator,
                        flow_arn,
                        &source_arn,
                        json!({ "gatewayBridgeSource": { "bridgeArn": other_bridge } }),
                    )
                    .await
                }
                Clearing::DeleteFlow => delete_flow(&emulator, flow_arn).await,
            };
            // Each of the three calls answers with the 202 its own binding declares.
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: clearing the reference of flow {cleared} expected 202, got \
                 {status} {code:?}: {answered}"
            );
            assert_no_dangling_references(&emulator, case);
        }

        let (status, code, served) = delete_bridge(&emulator, &bridge_arn).await;
        assert_eq!(
            (status, code.as_deref(), served["bridgeArn"].as_str()),
            (200, None, Some(bridge_arn.as_str())),
            "case {case:?}: with nothing naming it the delete is served and answers the \
             bridge's own ARN, got {status} {code:?}: {served}"
        );
        assert!(
            !bridge_is_held(&emulator, case, &bridge_arn).await,
            "case {case:?}: the deleted bridge stopped existing"
        );
        assert!(
            bridge_is_held(&emulator, case, &other_bridge).await,
            "case {case:?}: the bridge the delete did not address is still held"
        );
        assert_no_dangling_references(&emulator, case);
    }
}
