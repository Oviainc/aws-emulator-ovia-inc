// SPDX-License-Identifier: CC0-1.0

//! One set of gateway network names per bridge, whichever operation writes a network output
//! onto it, end to end over HTTP: a gateway, a flow and an egress bridge are created, then
//! AddBridgeOutputs and UpdateBridgeOutput are called with the SAME network name on that
//! same bridge, then DescribeBridge reports what was stored.
//!
//! The rule under test: `BridgeNetworkOutput$NetworkName` ("the network output's gateway
//! network name") is resolved against the networks the gateway the bridge is placed on
//! declares (`GatewayNetwork$Name`: "this name is used to reference the network and must be
//! unique among networks in this gateway") by EVERY operation that writes a network output
//! onto a bridge the store holds, so one bridge admits exactly one set of names however an
//! output reaches it: a declared name is stored by both paths, and a name outside that set
//! is refused by both with the modelled `BadRequestException` (HTTP 400) in this service's
//! validation envelope, worded alike and stored by neither. No name is therefore storable
//! through one path and refused by the other, and no stored output is ever delivered on a
//! network its placement gateway does not declare — which the invariant at the end of each
//! row asserts over the whole bridge rather than over the output the row wrote.
//!
//! The rows vary what the rule generalises over: which of the placement gateway's declared
//! networks the name is (every one of them, so the accepting side is asserted as SET
//! membership and not as one allowed value) and HOW an undeclared name misses the set — a
//! name no gateway declares, a name ANOTHER gateway declares, a declared name differing
//! only in case or in surrounding space, the empty name, two declared names run together,
//! and a declared name with a segment appended.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::flow_lifecycle::Emulator;

/// The networks the placement gateway declares — `mc::MAX_GATEWAY_NETWORKS` of them, which
/// is every network one gateway may carry, so the accepting rows cover the whole declared
/// set of this bridge's gateway.
const PLACEMENT_NETWORKS: [&str; 4] = ["net-1", "net-2", "net-3", "net-4"];

/// A network declared on ANOTHER gateway: a name that exists in the store, but not on the
/// gateway the bridges under test are placed on.
const OTHER_GATEWAY_NETWORK: &str = "elsewhere-net";

/// The output every bridge below is created holding, which the update path addresses.
const HELD_OUTPUT: &str = "held-out";

/// The output the add path appends.
const ADDED_OUTPUT: &str = "added-out";

/// `POST /v1/bridges/{BridgeArn}/outputs`.
async fn add_outputs(
    emulator: &Emulator,
    bridge_arn: &str,
    outputs: Vec<Value>,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "POST",
            &format!("/v1/bridges/{bridge_arn}/outputs"),
            Some(json!({ "outputs": outputs })),
        )
        .await
}

/// `PUT /v1/bridges/{BridgeArn}/outputs/{OutputName}`.
async fn update_output(
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

/// One row: a network name, and whether the placement gateway declares it.
struct Case {
    case: String,
    network_name: String,
    declared: bool,
}

/// The whole table: every network the placement gateway declares, then every way a name can
/// miss that set.
fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> = PLACEMENT_NETWORKS
        .into_iter()
        .map(|network_name| Case {
            case: format!("the declared network {network_name:?}"),
            network_name: network_name.to_string(),
            declared: true,
        })
        .collect();
    for (case, network_name) in [
        ("a name no gateway declares", "no-such-network".to_string()),
        (
            "a name ANOTHER gateway declares, not this bridge's",
            OTHER_GATEWAY_NETWORK.to_string(),
        ),
        (
            "a declared name in a different case: a network name is matched exactly",
            PLACEMENT_NETWORKS[0].to_uppercase(),
        ),
        (
            "a declared name with surrounding space",
            format!(" {} ", PLACEMENT_NETWORKS[1]),
        ),
        ("the empty name", String::new()),
        (
            "two declared names run together",
            format!("{}{}", PLACEMENT_NETWORKS[2], PLACEMENT_NETWORKS[3]),
        ),
        (
            "a declared name with a segment appended",
            format!("{}-extra", PLACEMENT_NETWORKS[3]),
        ),
    ] {
        cases.push(Case {
            case: case.to_string(),
            network_name,
            declared: false,
        });
    }
    cases
}

/// The gateway every bridge here is placed on, and the flow an egress bridge's source names.
struct Fixture {
    placement_arn: String,
    held_flow: String,
}

async fn fixture(emulator: &Emulator) -> Fixture {
    let placement_arn =
        gateway_with_networks(emulator, "gw-output-paths", &PLACEMENT_NETWORKS).await;
    // A second gateway, so a name that IS declared somewhere in the store but not on these
    // bridges' gateway can be asserted as a miss.
    let _ =
        gateway_with_networks(emulator, "gw-output-paths-elsewhere", &[OTHER_GATEWAY_NETWORK])
            .await;
    let held_flow = cloud_flow(emulator, "flow-output-paths").await;
    Fixture {
        placement_arn,
        held_flow,
    }
}

/// A cloud-to-ground (egress) bridge — the only kind a request configures network outputs on
/// — holding one flow source and one network output on the placement gateway's first
/// declared network.
async fn egress_bridge(emulator: &Emulator, name: &str, fixture: &Fixture) -> String {
    let (status, code, created) = create_bridge(
        emulator,
        json!({
            "name": name,
            "placementArn": fixture.placement_arn,
            "egressGatewayBridge": { "maxBitrate": 20_000_000 },
            "sources": [{ "flowSource": { "flowArn": fixture.held_flow, "name": "flow-src" }}],
            "outputs": [network_output(HELD_OUTPUT, PLACEMENT_NETWORKS[0], 6_000)],
        }),
    )
    .await;
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

/// The outputs DescribeBridge reports for `arn`.
async fn described_outputs(emulator: &Emulator, case: &str, arn: &str) -> Vec<Value> {
    let (status, code, described) = describe_bridge(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
    );
    described["bridge"]["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The network name the output `name` is stored with, or `None` when the bridge holds no
/// output by that name.
fn stored_network_name(outputs: &[Value], name: &str) -> Option<String> {
    outputs
        .iter()
        .find(|output| output["networkOutput"]["name"].as_str() == Some(name))
        .and_then(|output| output["networkOutput"]["networkName"].as_str())
        .map(str::to_string)
}

/// Assert one refusal: the modelled error, this service's validation envelope, and a message
/// that names the value which did not resolve — so a row cannot pass on a refusal some other
/// rule (a name already in use, the per-bridge bound) produced.
fn assert_refused(case: &str, status: u16, code: Option<&str>, answered: &Value, value: &str) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {answered}"
    );
    let message = answered["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX),
        "case {case:?}: a refusal about the request's content is worded in the {:?} envelope, \
         got {message:?}",
        mc::VALIDATION_FAILED_PREFIX
    );
    assert!(
        message.contains(&format!("{value:?}")),
        "case {case:?}: the refusal names the network name that did not resolve ({value:?}), \
         got {message:?}"
    );
}

/// Every output the bridge holds is delivered on a network its placement gateway declares —
/// the invariant neither path may break, asserted over the whole bridge.
fn assert_every_output_on_a_declared_network(case: &str, outputs: &[Value]) {
    for output in outputs {
        let Some(network_name) = output["networkOutput"]["networkName"].as_str() else {
            continue;
        };
        assert!(
            PLACEMENT_NETWORKS.contains(&network_name),
            "case {case:?}: the bridge may hold no output on a network its placement gateway \
             does not declare, got {network_name:?} in {output}"
        );
    }
}

/// The add path and the update path admit exactly the same network names on one bridge: a
/// declared name is stored by both, and an undeclared name is refused by both, worded alike
/// and stored by neither.
#[tokio::test]
async fn the_add_path_and_the_update_path_admit_one_set_of_network_names_for_an_output() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for (index, row) in cases().into_iter().enumerate() {
        let Case {
            case,
            network_name,
            declared,
        } = row;
        // One bridge per row, so what each row asserts about the stored bridge does not
        // depend on the rows before it.
        let arn =
            egress_bridge(&emulator, &format!("bridge-output-paths-{index}"), &fixture).await;

        let (add_status, add_code, added) = add_outputs(
            &emulator,
            &arn,
            vec![network_output(ADDED_OUTPUT, &network_name, 6_100)],
        )
        .await;
        let (update_status, update_code, updated) = update_output(
            &emulator,
            &arn,
            HELD_OUTPUT,
            json!({ "networkOutput": { "networkName": network_name } }),
        )
        .await;

        // The two paths write the same stored member on the same bridge, so they answer one
        // network name alike — status, modelled error and message.
        assert_eq!(
            (add_status, add_code.as_deref(), added["message"].as_str()),
            (
                update_status,
                update_code.as_deref(),
                updated["message"].as_str()
            ),
            "case {case:?}: the add and the update must answer the network name \
             {network_name:?} alike, got {add_status} {add_code:?} {added} and \
             {update_status} {update_code:?} {updated}"
        );

        let outputs = described_outputs(&emulator, &case, &arn).await;
        if declared {
            assert_eq!(
                (add_status, add_code.as_deref()),
                (202, None),
                "case {case:?}: a declared network is accepted by the add path, got \
                 {add_status} {add_code:?}: {added}"
            );
            assert_eq!(
                stored_network_name(&outputs, ADDED_OUTPUT).as_deref(),
                Some(network_name.as_str()),
                "case {case:?}: the added output is delivered on the network the request \
                 named, got {outputs:?}"
            );
            assert_eq!(
                stored_network_name(&outputs, HELD_OUTPUT).as_deref(),
                Some(network_name.as_str()),
                "case {case:?}: the updated output is delivered on the network the request \
                 named, got {outputs:?}"
            );
        } else {
            assert_refused(&case, add_status, add_code.as_deref(), &added, &network_name);
            assert_refused(
                &case,
                update_status,
                update_code.as_deref(),
                &updated,
                &network_name,
            );
            assert_eq!(
                stored_network_name(&outputs, ADDED_OUTPUT),
                None,
                "case {case:?}: a refused add appends nothing, got {outputs:?}"
            );
            assert_eq!(
                stored_network_name(&outputs, HELD_OUTPUT).as_deref(),
                Some(PLACEMENT_NETWORKS[0]),
                "case {case:?}: a refused update leaves the stored output as it was, got \
                 {outputs:?}"
            );
        }
        assert_every_output_on_a_declared_network(&case, &outputs);
    }
}
