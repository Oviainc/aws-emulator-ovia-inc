// SPDX-License-Identifier: CC0-1.0

//! The references a source or an output ADDED to a stored bridge carries, end to end over
//! HTTP: a gateway, flows and a bridge are created, then AddBridgeSources /
//! AddBridgeOutputs is called, then DescribeBridge reports what was stored.
//!
//! The rule under test: a bridge member written onto a bridge the store already holds names
//! other resources, and every such reference is RESOLVED against the store before the
//! member is written — a gateway network name (`BridgeNetworkSource$NetworkName`, "the
//! network source's gateway network name"; `BridgeNetworkOutput$NetworkName`) must be one
//! the gateway the bridge is placed on declares (`GatewayNetwork$Name`: "this name is used
//! to reference the network and must be unique among networks in this gateway"), and a flow
//! ARN (`BridgeFlowSource$FlowArn`, "the ARN of the cloud flow used as a source of this
//! bridge") must name a flow the account holds. A reference that resolves to nothing is
//! refused with the modelled `BadRequestException` (HTTP 400) in this service's validation
//! envelope and nothing is appended — so the add path admits exactly what the update path
//! admits, which the last test below asserts request for request.
//!
//! The rows vary what the rule generalises over: which OPERATION writes the member
//! (AddBridgeSources, AddBridgeOutputs), which MEMBER carries the reference (a source's
//! network name, a source's flow ARN, an output's network name), which kind of value it is
//! (a name, an ARN), WHERE in the batch the unresolvable entry sits, and HOW the value
//! misses — a name no gateway declares, a name another gateway declares, a name differing
//! from a declared one only in case or in surrounding space, the empty name, two declared
//! names run together, an ARN of a flow this account never minted, the ARN of a flow that
//! was minted and then deleted, and a held flow's ARN with a segment appended. The
//! accepting rows span every network the placement gateway declares and every flow the
//! account holds, so the rule is asserted as SET membership rather than as one allowed
//! value.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{cloud_flow, create_bridge, describe_bridge, gateway_with_networks};
use crate::flow_lifecycle::{Emulator, REGION};

/// The networks the placement gateway declares — `mc::MAX_GATEWAY_NETWORKS` of them, which
/// is every network one gateway may carry, so the accepting rows below cover the whole
/// declared set of this bridge's gateway.
const PLACEMENT_NETWORKS: [&str; 4] = ["net-1", "net-2", "net-3", "net-4"];

/// A network declared on ANOTHER gateway: a name that exists in the store, but not on the
/// gateway the bridges under test are placed on.
const OTHER_GATEWAY_NETWORK: &str = "elsewhere-net";

/// `POST /v1/bridges/{BridgeArn}/sources`.
async fn add_sources(
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

/// `PUT /v1/bridges/{BridgeArn}/sources/{SourceName}` — the update path the add path must
/// agree with.
async fn update_source(
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

/// A flow ARN of the form this service mints, for a flow it never minted: well formed
/// enough to satisfy the `^arn:.+:mediaconnect.+:flow:.+$` pattern the model puts on a flow
/// ARN, so the request reaches the handler.
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

/// The gateway every bridge here is placed on, and the flows a flow source may name.
struct Fixture {
    /// The gateway declaring [`PLACEMENT_NETWORKS`].
    placement_arn: String,
    /// Two flows the account holds, and one it minted and then DELETED, so its ARN resolves
    /// nothing.
    held_flow: String,
    other_held_flow: String,
    deleted_flow: String,
}

async fn fixture(emulator: &Emulator, label: &str) -> Fixture {
    let placement_arn =
        gateway_with_networks(emulator, &format!("gw-{label}"), &PLACEMENT_NETWORKS).await;
    // A second gateway, so a name that IS declared somewhere in the store but not on these
    // bridges' gateway can be asserted as a miss.
    let _ = gateway_with_networks(
        emulator,
        &format!("gw-elsewhere-{label}"),
        &[OTHER_GATEWAY_NETWORK],
    )
    .await;

    let held_flow = cloud_flow(emulator, &format!("flow-held-{label}")).await;
    let other_held_flow = cloud_flow(emulator, &format!("flow-other-{label}")).await;
    let deleted_flow = cloud_flow(emulator, &format!("flow-deleted-{label}")).await;
    let (status, code, deleted) = emulator
        .send("DELETE", &format!("/v1/flows/{deleted_flow}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "fixture {label:?}: the flow to be deleted must delete, got {status} {code:?}: \
         {deleted}"
    );

    Fixture {
        placement_arn,
        held_flow,
        other_held_flow,
        deleted_flow,
    }
}

/// A ground-to-cloud (ingress) bridge holding one network source on the placement gateway's
/// first network, with source failover ACTIVE so one more source fits inside the bound its
/// failover state sets ("if the state is active, [it] can have one or two sources").
async fn ingress_bridge(emulator: &Emulator, name: &str, fixture: &Fixture) -> String {
    created_bridge(
        emulator,
        name,
        json!({
            "name": name,
            "placementArn": fixture.placement_arn,
            "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 8 },
            "sources": [network_source("held", PLACEMENT_NETWORKS[0], 5_000)],
            "sourceFailoverConfig": { "state": mc::STATE_ENABLED },
        }),
    )
    .await
}

/// A cloud-to-ground (egress) bridge holding one flow source on a flow the account holds —
/// the only kind of bridge a request configures network outputs on — with source failover
/// ACTIVE for the same reason.
async fn egress_bridge(emulator: &Emulator, name: &str, fixture: &Fixture) -> String {
    created_bridge(
        emulator,
        name,
        json!({
            "name": name,
            "placementArn": fixture.placement_arn,
            "egressGatewayBridge": { "maxBitrate": 20_000_000 },
            "sources": [flow_source("held", &fixture.held_flow)],
            "outputs": [network_output("held-out", PLACEMENT_NETWORKS[0], 6_000)],
            "sourceFailoverConfig": { "state": mc::STATE_ENABLED },
        }),
    )
    .await
}

async fn created_bridge(emulator: &Emulator, name: &str, body: Value) -> String {
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

/// The `Bridge` DescribeBridge reports for `arn`, asserted to be reported at all.
async fn bridge_of(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let (status, code, described) = describe_bridge(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
    );
    described["bridge"].clone()
}

/// Assert one refusal: the modelled error, this service's validation envelope, and a message
/// that states WHICH value did not resolve — so a row cannot pass on a refusal that some
/// other rule (a name already in use, the per-bridge bound) produced.
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
        "case {case:?}: the refusal names the reference that did not resolve ({value:?}), got \
         {message:?}"
    );
}

/// How a gateway network name can miss the set the placement gateway declares. Every row is
/// a value no gateway network of this bridge's placement declares, each missing in a
/// different way.
fn unresolvable_network_names() -> Vec<(&'static str, String)> {
    vec![
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
            format!("{}{}", PLACEMENT_NETWORKS[0], PLACEMENT_NETWORKS[1]),
        ),
    ]
}

/// A network source may be added on every network the placement gateway declares, and
/// DescribeBridge reports the bridge holding what it held followed by what was added.
#[tokio::test]
async fn a_network_source_is_added_on_every_network_the_placement_gateway_declares() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "add-net-src-ok").await;

    for (index, network_name) in PLACEMENT_NETWORKS.into_iter().enumerate() {
        let case = format!("a source added on {network_name:?}");
        // One bridge per row: a bridge whose failover is active holds a pair of sources, so
        // each row is asserted on a bridge with room for the source it adds.
        let arn = ingress_bridge(&emulator, &format!("bridge-net-src-{index}"), &fixture).await;
        let before = bridge_of(&emulator, &case, &arn).await;
        let added = vec![network_source("added", network_name, 5_100 + index as i64)];

        let (status, code, answered) = add_sources(&emulator, &arn, added.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: a declared network is accepted, got {status} {code:?}: {answered}"
        );
        assert_eq!(
            answered["sources"],
            json!(added),
            "case {case:?}: the answer carries the source added, got {answered}"
        );
        let after = bridge_of(&emulator, &case, &arn).await;
        let mut expected: Vec<Value> = before["sources"].as_array().cloned().unwrap_or_default();
        expected.extend(added);
        assert_eq!(
            after["sources"],
            json!(expected),
            "case {case:?}: the bridge holds what it held then what was added, got {after}"
        );
    }
}

/// A network source naming a network the placement gateway does not declare is refused
/// whichever way the name misses, and the bridge is left exactly as it was — including when
/// the unresolvable entry sits beside a resolvable one in the same batch.
#[tokio::test]
async fn a_network_source_naming_an_undeclared_network_is_refused_and_appends_nothing() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "add-net-src-refused").await;
    let arn = ingress_bridge(&emulator, "bridge-net-src-refused", &fixture).await;
    let before = bridge_of(&emulator, "before", &arn).await;

    for (case, network_name) in unresolvable_network_names() {
        let (status, code, answered) = add_sources(
            &emulator,
            &arn,
            vec![network_source("added", &network_name, 5_200)],
        )
        .await;
        assert_refused(case, status, code.as_deref(), &answered, &network_name);
        let after = bridge_of(&emulator, case, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused add appends nothing, got {after}"
        );
    }

    // The whole batch is refused wherever the unresolvable entry sits in it: the entry
    // beside it names a declared network and is appended just as little.
    let undeclared = "no-such-network";
    for (case, batch) in [
        (
            "the unresolvable entry last in the batch",
            vec![
                network_source("first", PLACEMENT_NETWORKS[1], 5_300),
                network_source("second", undeclared, 5_301),
            ],
        ),
        (
            "the unresolvable entry first in the batch",
            vec![
                network_source("first", undeclared, 5_302),
                network_source("second", PLACEMENT_NETWORKS[1], 5_303),
            ],
        ),
    ] {
        let (status, code, answered) = add_sources(&emulator, &arn, batch).await;
        assert_refused(case, status, code.as_deref(), &answered, undeclared);
        let after = bridge_of(&emulator, case, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused batch appends none of its entries, got {after}"
        );
    }
}

/// A flow source may be added for every flow the account holds, and for no other ARN: an ARN
/// this account never minted, the ARN of a flow it deleted, and a held ARN with a segment
/// appended are all refused and append nothing.
#[tokio::test]
async fn a_flow_source_is_added_only_for_a_flow_the_account_holds() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "add-flow-src").await;

    for (index, flow_arn) in [&fixture.held_flow, &fixture.other_held_flow]
        .into_iter()
        .enumerate()
    {
        let case = format!("a source added on the flow {flow_arn:?}");
        let arn = egress_bridge(&emulator, &format!("bridge-flow-src-{index}"), &fixture).await;
        let before = bridge_of(&emulator, &case, &arn).await;
        let added = vec![flow_source("added", flow_arn)];

        let (status, code, answered) = add_sources(&emulator, &arn, added.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: a flow the account holds is accepted, got {status} {code:?}: \
             {answered}"
        );
        let after = bridge_of(&emulator, &case, &arn).await;
        // Every flow source also carries the `OutputArn` of the output the service created
        // on its cloud flow for the bridge, which these rows cannot state (see
        // `crate::bridge_lifecycle::without_output_arns`).
        let mut expected: Vec<Value> =
            crate::bridge_lifecycle::without_output_arns(&case, &before["sources"])
                .as_array()
                .cloned()
                .unwrap_or_default();
        expected.extend(added);
        assert_eq!(
            crate::bridge_lifecycle::without_output_arns(&case, &after["sources"]),
            json!(expected),
            "case {case:?}: the bridge holds what it held then what was added, got {after}"
        );
    }

    let arn = egress_bridge(&emulator, "bridge-flow-src-refused", &fixture).await;
    let before = bridge_of(&emulator, "before", &arn).await;
    let cases: Vec<(&str, String)> = vec![
        ("an ARN this account never minted", unheld_flow_arn()),
        (
            "the ARN of a flow that was minted and then deleted",
            fixture.deleted_flow.clone(),
        ),
        (
            "a held flow's ARN with another segment appended",
            format!("{}-not-it", fixture.held_flow),
        ),
    ];

    for (case, flow_arn) in cases {
        let (status, code, answered) = add_sources(
            &emulator,
            &arn,
            vec![json!({ "flowSource": {
                "flowArn": flow_arn,
                "name": "added",
                "flowVpcInterfaceAttachment": { "vpcInterfaceName": "vpc-1" },
            }})],
        )
        .await;
        assert_refused(case, status, code.as_deref(), &answered, &flow_arn);
        let after = bridge_of(&emulator, case, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused add appends nothing, not even the VPC interface \
             attachment it carried beside the bad flow ARN, got {after}"
        );
    }
}

/// The same rule on the output side: a network output is added on every network the
/// placement gateway declares and on no other name.
#[tokio::test]
async fn a_network_output_is_added_only_on_a_network_the_placement_gateway_declares() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "add-net-out").await;
    let arn = egress_bridge(&emulator, "bridge-net-out", &fixture).await;

    for (index, network_name) in PLACEMENT_NETWORKS.into_iter().enumerate() {
        let case = format!("an output added on {network_name:?}");
        let before = bridge_of(&emulator, &case, &arn).await;
        let added = vec![network_output(
            &format!("added-{index}"),
            network_name,
            6_100 + index as i64,
        )];

        let (status, code, answered) = add_outputs(&emulator, &arn, added.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: a declared network is accepted, got {status} {code:?}: {answered}"
        );
        assert_eq!(
            answered["outputs"],
            json!(added),
            "case {case:?}: the answer carries the output added, got {answered}"
        );
        let after = bridge_of(&emulator, &case, &arn).await;
        let mut expected: Vec<Value> = before["outputs"].as_array().cloned().unwrap_or_default();
        expected.extend(added);
        assert_eq!(
            after["outputs"],
            json!(expected),
            "case {case:?}: the bridge holds what it held then what was added, got {after}"
        );
    }

    let before = bridge_of(&emulator, "before", &arn).await;
    for (case, network_name) in unresolvable_network_names() {
        let (status, code, answered) = add_outputs(
            &emulator,
            &arn,
            vec![network_output("added-refused", &network_name, 6_200)],
        )
        .await;
        assert_refused(case, status, code.as_deref(), &answered, &network_name);
        let after = bridge_of(&emulator, case, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused add appends nothing, got {after}"
        );
    }
}

/// The add path and the update path answer ONE unresolvable reference alike: the same
/// status, the same modelled error and the same message, for the same value on the same
/// bridge — so no value is storable through one of the two and refused by the other.
#[tokio::test]
async fn the_add_path_and_the_update_path_refuse_one_unresolvable_reference_alike() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "add-vs-update").await;
    let ingress_arn = ingress_bridge(&emulator, "bridge-add-vs-update-in", &fixture).await;
    let egress_arn = egress_bridge(&emulator, "bridge-add-vs-update-out", &fixture).await;

    // A network name no gateway declares, written onto a network source of the same bridge
    // by each of the two paths.
    let undeclared = "no-such-network";
    let (add_status, add_code, added) = add_sources(
        &emulator,
        &ingress_arn,
        vec![network_source("added", undeclared, 5_400)],
    )
    .await;
    let (update_status, update_code, updated) = update_source(
        &emulator,
        &ingress_arn,
        "held",
        json!({ "networkSource": { "networkName": undeclared } }),
    )
    .await;
    assert_eq!(
        (add_status, add_code.as_deref(), added["message"].as_str()),
        (
            update_status,
            update_code.as_deref(),
            updated["message"].as_str()
        ),
        "case \"an undeclared network name\": the add and the update must answer alike, got \
         {add_status} {add_code:?} {added} and {update_status} {update_code:?} {updated}"
    );
    assert_refused(
        "an undeclared network name, through the add path",
        add_status,
        add_code.as_deref(),
        &added,
        undeclared,
    );

    // And the same for a flow ARN this account never minted.
    let unheld = unheld_flow_arn();
    let (add_status, add_code, added) =
        add_sources(&emulator, &egress_arn, vec![flow_source("added", &unheld)]).await;
    let (update_status, update_code, updated) = update_source(
        &emulator,
        &egress_arn,
        "held",
        json!({ "flowSource": { "flowArn": unheld } }),
    )
    .await;
    assert_eq!(
        (add_status, add_code.as_deref(), added["message"].as_str()),
        (
            update_status,
            update_code.as_deref(),
            updated["message"].as_str()
        ),
        "case \"a flow ARN the account does not hold\": the add and the update must answer \
         alike, got {add_status} {add_code:?} {added} and {update_status} {update_code:?} \
         {updated}"
    );
    assert_refused(
        "a flow ARN the account does not hold, through the add path",
        add_status,
        add_code.as_deref(),
        &added,
        &unheld,
    );
}
