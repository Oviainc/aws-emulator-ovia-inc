// SPDX-License-Identifier: CC0-1.0

//! The flow delete and the bridge-source write paths agree about which flows the account
//! holds, end to end over HTTP.
//!
//! The rule the rows below pin is the two-sided reading of one invariant: the set of flows
//! `DeleteFlow` leaves in the store is exactly the set every path that writes a bridge flow
//! source resolves that source's flow against. A bridge's flow source names its cloud flow in
//! a member the model makes REQUIRED (`BridgeFlowSource$FlowArn`, required —
//! `smithy_query(query="shape",shape="BridgeFlowSource")` — "the ARN of the cloud flow used as
//! a source of this bridge"), so the reference can neither be dropped off the record nor left
//! naming nothing, and both sides follow from that:
//!
//! * while a stored bridge source names the flow, the delete is REFUSED (`BadRequestException`,
//!   400) and every bridge-source write path still accepts that flow ARN — so the configuration
//!   a read reports can still be stated by a request;
//! * once nothing names it the delete is SERVED, and from then on every one of those same write
//!   paths refuses that ARN as naming no flow the account holds — so no request can put the
//!   reference back.
//!
//! The rows differ along the write path the agreement is read through — the three this crate
//! has that resolve a bridge source's flow against the store: CreateBridge's `Sources`,
//! AddBridgeSources and UpdateBridgeSource. `crate::flow_deletion_bridge_source_integrity`
//! pins the refusal itself and the ways a caller clears the reference; this suite pins that
//! the WRITE paths and the delete never disagree about one flow ARN, in either direction, and
//! that the refusal counts holding BRIDGES however many of one bridge's sources name the flow.
//!
//! Every call travels the whole request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`].

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::flow_deletion_bridge_source_integrity::assert_no_dangling_references;
use crate::flow_lifecycle::Emulator;

/// The one network the gateway every bridge here is placed on declares.
const PLACEMENT_NETWORK: &str = "net-1";

/// The sentence a delete refused by the store's state carries: the flow it addressed and how
/// many bridges still take their content from it. Built here rather than read out of the crate,
/// so the wording is pinned by the test and not by the code under test.
fn expected_delete_refusal(flow_arn: &str, bridges: usize) -> String {
    format!(
        "{}Flow {flow_arn} is still the source of {bridges} bridge(s). Remove those bridge \
         sources, point them at another flow, or delete the bridges, before deleting the flow.",
        mc::BAD_REQUEST_PREFIX
    )
}

/// The sentence a write path refused for an unresolvable reference carries: the member that
/// named the flow and the ARN it carried. A refusal about the request's CONTENT carries the
/// validation envelope, not the state one above.
fn expected_write_refusal(flow_arn: &str) -> String {
    format!(
        "{}\nFlowSource FlowArn {flow_arn:?} does not name a flow this account holds.",
        mc::VALIDATION_FAILED_PREFIX
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

/// `POST /v1/bridges/{BridgeArn}/sources`.
async fn add_bridge_sources(
    emulator: &Emulator,
    bridge_arn: &str,
    sources: Vec<Value>,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "POST",
            &format!("/v1/bridges/{bridge_arn}/sources"),
            Some(json!({ "sources": sources })),
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

/// One flow source entry, as a request configures it.
fn flow_source(name: &str, flow_arn: &str) -> Value {
    json!({ "flowSource": { "flowArn": flow_arn, "name": name }})
}

/// Create one cloud-to-ground (egress) bridge holding `sources`, with source failover ACTIVE
/// so a second source fits inside the bound its failover state sets ("if the state is inactive,
/// [it] can have only one source. If the state is active, [it] can have one or two sources",
/// `FailoverConfig$State`), and answer its ARN.
async fn egress_bridge(
    emulator: &Emulator,
    case: &str,
    name: &str,
    placement_arn: &str,
    sources: Vec<Value>,
) -> String {
    let (status, code, created) = create_bridge(
        emulator,
        json!({
            "name": name,
            "placementArn": placement_arn,
            "egressGatewayBridge": { "maxBitrate": 20_000_000i64 },
            "sources": sources,
            "sourceFailoverConfig": { "state": mc::STATE_ENABLED },
        }),
    )
    .await;
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

/// The flow ARN the bridge `bridge_arn` reports on the source named `source_name`, read back
/// through DescribeBridge.
async fn reported_source_flow(
    emulator: &Emulator,
    case: &str,
    bridge_arn: &str,
    source_name: &str,
) -> Option<String> {
    let (status, code, described) = describe_bridge(emulator, bridge_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the bridge reads back, got {status} {code:?}: {described}"
    );
    described["bridge"]["sources"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the bridge reports its sources: {described}"))
        .iter()
        .find(|source| source["flowSource"]["name"].as_str() == Some(source_name))
        .and_then(|source| source["flowSource"]["flowArn"].as_str())
        .map(str::to_string)
}

/// Which path states a bridge source's flow — the three this crate resolves
/// `BridgeFlowSource$FlowArn` against the store from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WritePath {
    /// CreateBridge's `Sources`: a NEW bridge whose flow source names the flow.
    CreateBridge,
    /// AddBridgeSources: one more flow source on a bridge the store already holds.
    AddSource,
    /// UpdateBridgeSource: a stored source re-pointed at the flow.
    UpdateSource,
}

/// The name the bridges here address their sources by.
const HELD_SOURCE: &str = "src-0";
const ADDED_SOURCE: &str = "src-1";

/// Both directions of the agreement, read through each write path in turn: while a bridge
/// source names the flow the delete is refused and the write path is served, and once no bridge
/// source names it the delete is served and that same write path refuses the ARN.
#[tokio::test]
async fn the_flow_delete_and_the_bridge_source_write_paths_agree() {
    for write_path in [
        WritePath::CreateBridge,
        WritePath::AddSource,
        WritePath::UpdateSource,
    ] {
        let case = format!("through {write_path:?}");
        let case = case.as_str();
        let emulator = Emulator::new();
        let placement_arn =
            gateway_with_networks(&emulator, "gw-write-agreement", &[PLACEMENT_NETWORK]).await;
        let flow_arn = cloud_flow(&emulator, "flow-under-test").await;
        // A flow nothing deletes, so the bridge the post-delete write path addresses survives
        // the delete of the flow under test.
        let keeper_flow = cloud_flow(&emulator, "flow-keeper").await;

        let holder = egress_bridge(
            &emulator,
            case,
            "bridge-holder",
            &placement_arn,
            vec![flow_source(HELD_SOURCE, &flow_arn)],
        )
        .await;
        let keeper = egress_bridge(
            &emulator,
            case,
            "bridge-keeper",
            &placement_arn,
            vec![flow_source(HELD_SOURCE, &keeper_flow)],
        )
        .await;

        // One bridge names the flow, so the delete is refused and nothing is taken away.
        let (status, code, refused) = delete_flow(&emulator, &flow_arn).await;
        assert_eq!(
            (status, code.as_deref(), refused["message"].as_str()),
            (
                400,
                Some("BadRequestException"),
                Some(expected_delete_refusal(&flow_arn, 1).as_str())
            ),
            "case {case:?}: with one bridge naming it the delete is refused, got {status} \
             {code:?}: {refused}"
        );
        assert_eq!(
            reported_source_flow(&emulator, case, &holder, HELD_SOURCE)
                .await
                .as_deref(),
            Some(flow_arn.as_str()),
            "case {case:?}: the refused delete left the bridge naming the flow"
        );

        // The reference the refusal kept resolvable is one the write path still accepts — and
        // the holder it is written onto is this row's own, so the refusal that follows counts
        // the bridges that name the flow rather than the sources.
        let (naming_bridges, written) = match write_path {
            WritePath::CreateBridge => {
                let second = egress_bridge(
                    &emulator,
                    case,
                    "bridge-second",
                    &placement_arn,
                    vec![flow_source(HELD_SOURCE, &flow_arn)],
                )
                .await;
                (2, vec![holder.clone(), second])
            }
            WritePath::AddSource => {
                let (status, code, added) = add_bridge_sources(
                    &emulator,
                    &holder,
                    vec![flow_source(ADDED_SOURCE, &flow_arn)],
                )
                .await;
                assert_eq!(
                    (
                        status,
                        code.as_deref(),
                        added["sources"][0]["flowSource"]["flowArn"].as_str()
                    ),
                    (202, None, Some(flow_arn.as_str())),
                    "case {case:?}: adding a source on the flow the store still holds expected \
                     202 naming that flow, got {status} {code:?}: {added}"
                );
                assert_eq!(
                    reported_source_flow(&emulator, case, &holder, ADDED_SOURCE)
                        .await
                        .as_deref(),
                    Some(flow_arn.as_str()),
                    "case {case:?}: the added source reads back naming the flow"
                );
                (1, vec![holder.clone()])
            }
            WritePath::UpdateSource => {
                let (status, code, updated) = update_bridge_source(
                    &emulator,
                    &keeper,
                    HELD_SOURCE,
                    json!({ "flowSource": { "flowArn": flow_arn } }),
                )
                .await;
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: re-pointing a stored source at the flow the store still \
                     holds expected 202, got {status} {code:?}: {updated}"
                );
                assert_eq!(
                    reported_source_flow(&emulator, case, &keeper, HELD_SOURCE)
                        .await
                        .as_deref(),
                    Some(flow_arn.as_str()),
                    "case {case:?}: the re-pointed source reads back naming the flow"
                );
                (2, vec![holder.clone(), keeper.clone()])
            }
        };

        let (status, code, refused) = delete_flow(&emulator, &flow_arn).await;
        assert_eq!(
            (status, code.as_deref(), refused["message"].as_str()),
            (
                400,
                Some("BadRequestException"),
                Some(expected_delete_refusal(&flow_arn, naming_bridges).as_str())
            ),
            "case {case:?}: the refusal counts the {naming_bridges} bridge(s) naming the flow, \
             got {status} {code:?}: {refused}"
        );
        assert_no_dangling_references(&emulator, case);

        // Nothing names the flow any more, so the delete is served.
        for bridge_arn in &written {
            let (status, code, answered) = delete_bridge(&emulator, bridge_arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: clearing bridge {bridge_arn} expected 200, got {status} \
                 {code:?}: {answered}"
            );
        }
        let (status, code, served) = delete_flow(&emulator, &flow_arn).await;
        assert_eq!(
            (status, code.as_deref(), served["flowArn"].as_str()),
            (202, None, Some(flow_arn.as_str())),
            "case {case:?}: with nothing naming it the delete is served, got {status} {code:?}: \
             {served}"
        );

        // A bridge that survived the delete, to write the gone ARN through: the keeper, unless
        // this row's write path left it naming the flow under test and it had to be cleared.
        let survivor = if written.contains(&keeper) {
            egress_bridge(
                &emulator,
                case,
                "bridge-survivor",
                &placement_arn,
                vec![flow_source(HELD_SOURCE, &keeper_flow)],
            )
            .await
        } else {
            keeper.clone()
        };

        // The other direction: the ARN the delete took out of the store is one the SAME write
        // path now refuses, so the state the delete left is one no request can take back.
        let (status, code, answered) = match write_path {
            WritePath::CreateBridge => {
                create_bridge(
                    &emulator,
                    json!({
                        "name": "bridge-after-delete",
                        "placementArn": placement_arn,
                        "egressGatewayBridge": { "maxBitrate": 20_000_000i64 },
                        "sources": [flow_source(HELD_SOURCE, &flow_arn)],
                    }),
                )
                .await
            }
            WritePath::AddSource => {
                add_bridge_sources(
                    &emulator,
                    &survivor,
                    vec![flow_source(ADDED_SOURCE, &flow_arn)],
                )
                .await
            }
            WritePath::UpdateSource => {
                update_bridge_source(
                    &emulator,
                    &survivor,
                    HELD_SOURCE,
                    json!({ "flowSource": { "flowArn": flow_arn } }),
                )
                .await
            }
        };
        assert_eq!(
            (status, code.as_deref(), answered["message"].as_str()),
            (
                400,
                Some("BadRequestException"),
                Some(expected_write_refusal(&flow_arn).as_str())
            ),
            "case {case:?}: stating the deleted flow through the write path is refused, got \
             {status} {code:?}: {answered}"
        );

        // The refused write stored nothing: the surviving bridge still names the flow it did,
        // and no bridge the store holds names a flow it does not.
        assert_eq!(
            reported_source_flow(&emulator, case, &survivor, HELD_SOURCE)
                .await
                .as_deref(),
            Some(keeper_flow.as_str()),
            "case {case:?}: the refused write left the surviving bridge as it was"
        );
        assert_eq!(
            reported_source_flow(&emulator, case, &survivor, ADDED_SOURCE).await,
            None,
            "case {case:?}: the refused add appended no source"
        );
        assert_no_dangling_references(&emulator, case);
    }
}
