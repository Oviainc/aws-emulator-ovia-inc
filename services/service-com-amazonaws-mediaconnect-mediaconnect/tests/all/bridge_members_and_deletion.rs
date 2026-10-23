// SPDX-License-Identifier: CC0-1.0

//! A bridge's members and its removal, end to end over HTTP: CreateBridge ->
//! AddBridgeSources / AddBridgeOutputs -> RemoveBridgeSource / RemoveBridgeOutput ->
//! ListBridges -> DeleteBridge.
//!
//! The rules asserted here are:
//!
//! * an add APPENDS what the request configures to the bridge the ARN addresses and
//!   answers the entries it stored, so DescribeBridge afterwards reports the bridge's
//!   previous members followed by the new ones;
//! * a remove takes the member the bridge addresses by that NAME out of the bridge and
//!   answers what it removed, so DescribeBridge afterwards reports the bridge without it
//!   and with every other member untouched;
//! * a refused add or remove changes nothing: the bridge DescribeBridge reports after the
//!   refusal is byte for byte the one it reported before;
//! * ListBridges reports every stored bridge exactly once, in ARN order, as the summary of
//!   the stored record, optionally restricted to one placement;
//! * DeleteBridge removes the bridge — DescribeBridge stops answering for it — and refuses
//!   a bridge that has not been stopped.
//!
//! Each table's rows differ along what the rule generalises over: the KIND of member
//! (flow source / network source / network output), how MANY one request carries (one, a
//! batch, and one past the per-bridge bound), WHICH member of several is addressed (first,
//! middle, last, the only one), and — for the delete precondition — every value of the
//! modelled `BridgeState` enum.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{
    create_bridge, describe_bridge, minted_flow_arn, minted_gateway_arn,
};
use crate::flow_lifecycle::{Emulator, REGION};

/// A bridge ARN of the form this service mints, built through the crate's own encoding of
/// it — what an `@httpLabel BridgeArn` must look like (`^arn:.+:mediaconnect.+:bridge:.+$`)
/// for a request to reach the handler at all.
fn minted_bridge_arn(suffix: u64, name: &str) -> String {
    engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::bridge_resource_fragment(suffix, name),
    )
}

async fn add_sources(emulator: &Emulator, arn: &str, body: Value) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", &format!("/v1/bridges/{arn}/sources"), Some(body))
        .await
}

async fn add_outputs(emulator: &Emulator, arn: &str, body: Value) -> (u16, Option<String>, Value) {
    emulator
        .send("POST", &format!("/v1/bridges/{arn}/outputs"), Some(body))
        .await
}

async fn remove_source(emulator: &Emulator, arn: &str, name: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/bridges/{arn}/sources/{name}"), None)
        .await
}

async fn remove_output(emulator: &Emulator, arn: &str, name: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/bridges/{arn}/outputs/{name}"), None)
        .await
}

async fn delete_bridge(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/bridges/{arn}"), None)
        .await
}

async fn list_bridges(emulator: &Emulator, query: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/bridges{query}"), None)
        .await
}

/// One network source entry, as a request configures it and as a bridge reports it.
fn network_source(name: &str, port: i64, protocol: &str) -> Value {
    json!({ "networkSource": {
        "multicastIp": "224.0.1.1",
        "name": name,
        "networkName": "net-1",
        "port": port,
        "protocol": protocol,
    }})
}

/// One network output entry, as a request configures it and as a bridge reports it.
fn network_output(name: &str, port: i64, protocol: &str, ttl: i64) -> Value {
    json!({ "networkOutput": {
        "ipAddress": "10.0.1.1",
        "name": name,
        "networkName": "net-1",
        "port": port,
        "protocol": protocol,
        "ttl": ttl,
    }})
}

/// Create one ingress bridge carrying `sources` and `outputs`, and answer its ARN.
///
/// The create is the subject of `bridge_lifecycle`; here it is only the fixture every
/// member operation below addresses.
async fn bridge_with(
    emulator: &Emulator,
    name: &str,
    placement_arn: &str,
    sources: Vec<Value>,
    outputs: Vec<Value>,
) -> String {
    let mut body = json!({
        "name": name,
        "placementArn": placement_arn,
        "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 8 },
        "sources": sources,
    });
    if !outputs.is_empty() {
        body["outputs"] = json!(outputs);
    }
    let (status, code, created) = create_bridge(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "fixture {name:?}: create expected 201, got {status} {code:?}: {created}"
    );
    created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {name:?}: the create answers an ARN: {created}"))
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

/// One add request: what it carries, and the entries the bridge must hold afterwards.
struct AddCase {
    case: &'static str,
    /// The `Sources` the add request carries, and their projection in the bridge.
    sources: Vec<Value>,
    /// The `Outputs` a second add request carries, and their projection in the bridge.
    outputs: Vec<Value>,
}

/// The add table. Every row differs in the KIND of the entries, how many one request
/// carries, and which protocol each one names.
fn add_cases() -> Vec<AddCase> {
    let flow_a = minted_flow_arn(101, "flow-a");
    let flow_b = minted_flow_arn(102, "flow-b");
    vec![
        AddCase {
            case: "one network source, one network output",
            sources: vec![network_source("added-net", 5100, "rtp")],
            outputs: vec![network_output("added-out", 6100, "udp", 32)],
        },
        AddCase {
            case: "a batch of two sources — one flow, one network — and two outputs on \
                   different protocols",
            sources: vec![
                json!({ "flowSource": { "flowArn": flow_a, "name": "added-flow" } }),
                network_source("added-net-2", 5200, "rtp-fec"),
            ],
            outputs: vec![
                network_output("added-out-udp", 6200, "udp", 16),
                network_output("added-out-srt", 6201, "srt-listener", 255),
            ],
        },
        AddCase {
            case: "a flow source with a VPC interface attachment, and three outputs",
            sources: vec![json!({ "flowSource": {
                "flowArn": flow_b,
                "name": "added-flow-vpc",
                "flowVpcInterfaceAttachment": { "vpcInterfaceName": "vpc-1" },
            }})],
            outputs: vec![
                network_output("added-out-a", 6300, "rtp", 1),
                network_output("added-out-b", 6301, "rtp-fec", 64),
                network_output("added-out-c", 6302, "st2110-jpegxs", 128),
            ],
        },
    ]
}

/// An add appends what the request configures to the stored bridge and answers the
/// entries it stored: DescribeBridge afterwards reports the bridge's own members followed
/// by the added ones.
#[tokio::test]
async fn adding_sources_and_outputs_appends_them_to_the_stored_bridge() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(1, "gw-members");

    for (index, case) in add_cases().into_iter().enumerate() {
        let AddCase {
            case: name_of_case,
            sources,
            outputs,
        } = case;
        let held_source = network_source("base-src", 5000, "rtp");
        let held_output = network_output("base-out", 6000, "udp", 8);
        let arn = bridge_with(
            &emulator,
            &format!("bridge-add-{index}"),
            &placement,
            vec![held_source.clone()],
            vec![held_output.clone()],
        )
        .await;

        let (status, code, added) =
            add_sources(&emulator, &arn, json!({ "sources": sources })).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {name_of_case:?}: add sources expected 202, got {status} {code:?}: {added}"
        );
        assert_eq!(
            added["bridgeArn"].as_str(),
            Some(arn.as_str()),
            "case {name_of_case:?}: the answer names the bridge added to, got {added}"
        );
        assert_eq!(
            added["sources"],
            json!(sources),
            "case {name_of_case:?}: the answer carries only the sources added, got {added}"
        );

        let (status, code, added) =
            add_outputs(&emulator, &arn, json!({ "outputs": outputs })).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {name_of_case:?}: add outputs expected 202, got {status} {code:?}: {added}"
        );
        assert_eq!(
            added["bridgeArn"].as_str(),
            Some(arn.as_str()),
            "case {name_of_case:?}: the answer names the bridge added to, got {added}"
        );
        assert_eq!(
            added["outputs"],
            json!(outputs),
            "case {name_of_case:?}: the answer carries only the outputs added, got {added}"
        );

        // The round trip: the bridge holds what it held plus what each add appended, in
        // that order.
        let bridge = bridge_of(&emulator, name_of_case, &arn).await;
        let mut expected_sources = vec![held_source];
        expected_sources.extend(sources);
        let mut expected_outputs = vec![held_output];
        expected_outputs.extend(outputs);
        assert_eq!(
            bridge["sources"],
            json!(expected_sources),
            "case {name_of_case:?}: expected the held source then the added ones, got {bridge}"
        );
        assert_eq!(
            bridge["outputs"],
            json!(expected_outputs),
            "case {name_of_case:?}: expected the held output then the added ones, got {bridge}"
        );
    }
}

/// A remove takes out the member the bridge addresses by that name and leaves every other
/// one where it was; repeating it answers `NotFoundException`, because the name no longer
/// addresses anything.
#[tokio::test]
async fn removing_a_source_or_an_output_takes_it_out_of_the_stored_bridge() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(2, "gw-remove");

    // Which of the bridge's three members is removed: the first, the middle and the last,
    // so the rule is asserted at both ends of the list and inside it.
    let removal_rows: [(&str, usize); 3] = [
        ("the first of three", 0),
        ("the middle of three", 1),
        ("the last of three", 2),
    ];

    for (name_of_case, position) in removal_rows {
        let sources: Vec<Value> = (0..3)
            .map(|index| {
                network_source(&format!("src-{index}"), 5000 + index as i64, "rtp")
            })
            .collect();
        let outputs: Vec<Value> = (0..3)
            .map(|index| {
                network_output(&format!("out-{index}"), 6000 + index as i64, "udp", 32)
            })
            .collect();
        let arn = bridge_with(
            &emulator,
            &format!("bridge-remove-{position}"),
            &placement,
            sources.clone(),
            outputs.clone(),
        )
        .await;

        let source_name = format!("src-{position}");
        let (status, code, removed) = remove_source(&emulator, &arn, &source_name).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {name_of_case:?}: remove source expected 202, got {status} {code:?}: {removed}"
        );
        assert_eq!(
            (
                removed["bridgeArn"].as_str(),
                removed["sourceName"].as_str()
            ),
            (Some(arn.as_str()), Some(source_name.as_str())),
            "case {name_of_case:?}: the answer names the bridge and the source removed, got \
             {removed}"
        );

        let output_name = format!("out-{position}");
        let (status, code, removed) = remove_output(&emulator, &arn, &output_name).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {name_of_case:?}: remove output expected 202, got {status} {code:?}: {removed}"
        );
        assert_eq!(
            (
                removed["bridgeArn"].as_str(),
                removed["outputName"].as_str()
            ),
            (Some(arn.as_str()), Some(output_name.as_str())),
            "case {name_of_case:?}: the answer names the bridge and the output removed, got \
             {removed}"
        );

        // The round trip: the member is gone and the other two are reported in their
        // original order.
        let bridge = bridge_of(&emulator, name_of_case, &arn).await;
        let mut expected_sources = sources.clone();
        expected_sources.remove(position);
        let mut expected_outputs = outputs.clone();
        expected_outputs.remove(position);
        assert_eq!(
            bridge["sources"],
            json!(expected_sources),
            "case {name_of_case:?}: the removed source is gone and the rest keep their order, \
             got {bridge}"
        );
        assert_eq!(
            bridge["outputs"],
            json!(expected_outputs),
            "case {name_of_case:?}: the removed output is gone and the rest keep their order, \
             got {bridge}"
        );

        // A name that no longer addresses a member of this bridge resolves nothing.
        let (status, code, repeated) = remove_source(&emulator, &arn, &source_name).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {name_of_case:?}: removing the source twice expected 404, got {status} \
             {code:?}: {repeated}"
        );
        let (status, code, repeated) = remove_output(&emulator, &arn, &output_name).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {name_of_case:?}: removing the output twice expected 404, got {status} \
             {code:?}: {repeated}"
        );
    }
}

/// Nothing in the model puts a lower bound on a STORED bridge's members — "the request
/// must include one source" is CreateBridge's rule about a create REQUEST — so the only
/// source of a bridge can be removed, and the bridge then reports no `Sources` member at
/// all (the member is optional, so an empty list is absent).
#[tokio::test]
async fn the_only_source_of_a_bridge_can_be_removed() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(3, "gw-last-member");
    let arn = bridge_with(
        &emulator,
        "bridge-last-member",
        &placement,
        vec![network_source("only-src", 5000, "rtp")],
        vec![network_output("only-out", 6000, "udp", 32)],
    )
    .await;

    for (case, name, member) in [
        ("the only source", "only-src", "sources"),
        ("the only output", "only-out", "outputs"),
    ] {
        let (status, code, removed) = if member == "sources" {
            remove_source(&emulator, &arn, name).await
        } else {
            remove_output(&emulator, &arn, name).await
        };
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: remove expected 202, got {status} {code:?}: {removed}"
        );
        let bridge = bridge_of(&emulator, case, &arn).await;
        assert_eq!(
            bridge[member],
            Value::Null,
            "case {case:?}: a bridge holding no {member} reports no such member, got {bridge}"
        );
    }
}

/// One refused add or remove: the request, and the refusal the service must answer.
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    /// The URI, relative to the bridge the fixture created (`{arn}` is substituted).
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
}

/// The refusal table. Every row is a request the service must refuse, and after each one
/// the bridge must be exactly what it was.
fn refusal_cases(bridge_arn: &str) -> Vec<RefusalCase> {
    let missing = minted_bridge_arn(9_999, "no-such-bridge");
    // One entry past the per-bridge bound, counting the one source and one output the
    // fixture bridge already holds (`mc::MAX_BRIDGE_SOURCES` / `mc::MAX_BRIDGE_OUTPUTS`).
    let past_sources_bound: Vec<Value> = (0..mc::MAX_BRIDGE_SOURCES)
        .map(|index| network_source(&format!("over-src-{index}"), 7000 + index as i64, "rtp"))
        .collect();
    let past_outputs_bound: Vec<Value> = (0..mc::MAX_BRIDGE_OUTPUTS)
        .map(|index| {
            network_output(&format!("over-out-{index}"), 8000 + index as i64, "udp", 32)
        })
        .collect();

    vec![
        RefusalCase {
            case: "an add that carries no source at all",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/sources"),
            body: Some(json!({ "sources": [] })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a source entry naming both a flow source and a network source",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/sources"),
            body: Some(json!({ "sources": [{
                "flowSource": {
                    "flowArn": minted_flow_arn(201, "flow-both"),
                    "name": "both",
                },
                "networkSource": {
                    "multicastIp": "224.0.1.2",
                    "name": "both",
                    "networkName": "net-1",
                    "port": 5300,
                    "protocol": "rtp",
                },
            }] })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a source entry naming neither a flow source nor a network source",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/sources"),
            body: Some(json!({ "sources": [{}] })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a source whose name the bridge already holds",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/sources"),
            body: Some(json!({ "sources": [network_source("base-src", 5400, "rtp")] })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "two sources of one request sharing a name",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/sources"),
            body: Some(json!({ "sources": [
                network_source("twice", 5500, "rtp"),
                network_source("twice", 5501, "udp"),
            ] })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an add that would take the bridge one source past the bound",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/sources"),
            body: Some(json!({ "sources": past_sources_bound })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an add of sources to a bridge the store does not hold",
            method: "POST",
            uri: format!("/v1/bridges/{missing}/sources"),
            body: Some(json!({ "sources": [network_source("orphan", 5600, "rtp")] })),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "an add that carries no output at all",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/outputs"),
            body: Some(json!({ "outputs": [] })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an output entry naming no network output",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/outputs"),
            body: Some(json!({ "outputs": [{}] })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an output whose name the bridge already holds",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/outputs"),
            body: Some(json!({ "outputs": [network_output("base-out", 6400, "udp", 32)] })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "two outputs of one request sharing a name",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/outputs"),
            body: Some(json!({ "outputs": [
                network_output("twice", 6500, "udp", 32),
                network_output("twice", 6501, "rtp", 32),
            ] })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an add that would take the bridge one output past the bound",
            method: "POST",
            uri: format!("/v1/bridges/{bridge_arn}/outputs"),
            body: Some(json!({ "outputs": past_outputs_bound })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an add of outputs to a bridge the store does not hold",
            method: "POST",
            uri: format!("/v1/bridges/{missing}/outputs"),
            body: Some(json!({ "outputs": [network_output("orphan", 6600, "udp", 32)] })),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "the removal of a source name the bridge does not hold",
            method: "DELETE",
            uri: format!("/v1/bridges/{bridge_arn}/sources/no-such-source"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "the removal of an output name the bridge does not hold",
            method: "DELETE",
            uri: format!("/v1/bridges/{bridge_arn}/outputs/no-such-output"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "the removal of a source from a bridge the store does not hold",
            method: "DELETE",
            uri: format!("/v1/bridges/{missing}/sources/base-src"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "the removal of an output from a bridge the store does not hold",
            method: "DELETE",
            uri: format!("/v1/bridges/{missing}/outputs/base-out"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "the delete of a bridge the store does not hold",
            method: "DELETE",
            uri: format!("/v1/bridges/{missing}"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "a listing resumed from a token this service never issued",
            method: "GET",
            uri: "/v1/bridges?nextToken=not-a-token".to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
    ]
}

/// A refused add, remove, delete or listing answers the modelled error and changes
/// nothing: the bridge is byte for byte the one it was before the request.
#[tokio::test]
async fn a_refused_request_names_the_modelled_error_and_leaves_the_bridge_untouched() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(4, "gw-refusals");
    let arn = bridge_with(
        &emulator,
        "bridge-refusals",
        &placement,
        vec![network_source("base-src", 5000, "rtp")],
        vec![network_output("base-out", 6000, "udp", 32)],
    )
    .await;

    for case in refusal_cases(&arn) {
        let before = bridge_of(&emulator, case.case, &arn).await;
        let (status, code, answered) = emulator.send(case.method, &case.uri, case.body).await;
        assert_eq!(
            (status, code.as_deref()),
            (case.status, Some(case.code)),
            "case {:?}: expected {} {}, got {status} {code:?}: {answered}",
            case.case,
            case.status,
            case.code
        );
        let after = bridge_of(&emulator, case.case, &arn).await;
        for member in ["sources", "outputs", "name", "placementArn", "bridgeArn"] {
            assert_eq!(
                after[member], before[member],
                "case {:?}: a refusal must leave {member} as it was, got {:?}",
                case.case, after[member]
            );
        }
    }
}

/// ListBridges reports every stored bridge exactly once, in ARN order, as the summary of
/// the stored record; `maxResults` bounds a page and `nextToken` resumes the walk;
/// `filterArn` keeps only the bridges placed on the gateway it names.
#[tokio::test]
async fn list_bridges_pages_every_stored_bridge_and_filters_by_placement() {
    let emulator = Emulator::new();
    let placement_a = minted_gateway_arn(5, "gw-a");
    let placement_b = minted_gateway_arn(6, "gw-b");

    // Three bridges on one gateway and two on another, so the filter has something to
    // exclude and the paging more than one page to walk.
    let mut on_a = Vec::new();
    for index in 0..3 {
        on_a.push(
            bridge_with(
                &emulator,
                &format!("bridge-a-{index}"),
                &placement_a,
                vec![network_source("src", 5000 + index, "rtp")],
                Vec::new(),
            )
            .await,
        );
    }
    let mut on_b = Vec::new();
    for index in 0..2 {
        // An egress bridge: `BridgeType` is read off the bridge-type member the create
        // named, so the two placements also differ in the type they report.
        let (status, code, created) = create_bridge(
            &emulator,
            json!({
                "name": format!("bridge-b-{index}"),
                "placementArn": placement_b,
                "egressGatewayBridge": { "maxBitrate": 80_000_000 },
                "sources": [network_source("src", 5100 + index, "rtp")],
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "fixture egress bridge {index}: create expected 201, got {status} {code:?}: {created}"
        );
        on_b.push(
            created["bridge"]["bridgeArn"]
                .as_str()
                .expect("the create answers an ARN")
                .to_string(),
        );
    }
    let mut every_arn: Vec<String> = on_a.iter().chain(on_b.iter()).cloned().collect();
    every_arn.sort();

    // One page holds every bridge: the default page size is 10 and the store holds five.
    let (status, code, listed) = list_bridges(&emulator, "").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "a listing expects 200, got {status} {code:?}: {listed}"
    );
    assert_eq!(
        listed["nextToken"],
        Value::Null,
        "a listing that reported every bridge carries no token, got {listed}"
    );
    let summaries = listed["bridges"]
        .as_array()
        .expect("a listing answers a bridges array")
        .clone();
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary["bridgeArn"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<String>>(),
        every_arn,
        "every stored bridge is listed exactly once, in ARN order, got {listed}"
    );

    // Each summary is the stored record's own: what DescribeBridge reports for that ARN.
    for summary in &summaries {
        let arn = summary["bridgeArn"].as_str().expect("a summary names an ARN");
        let bridge = bridge_of(&emulator, "listed summary", arn).await;
        for member in ["name", "placementArn", "bridgeState"] {
            assert_eq!(
                summary[member], bridge[member],
                "case {arn:?}: the summary's {member} is the stored bridge's, got {summary}"
            );
        }
        let expected_type = if bridge["egressGatewayBridge"] == Value::Null {
            mc::BRIDGE_TYPE_INGRESS
        } else {
            mc::BRIDGE_TYPE_EGRESS
        };
        assert_eq!(
            summary["bridgeType"].as_str(),
            Some(expected_type),
            "case {arn:?}: the summary's type is the kind of the bridge, got {summary}"
        );
    }

    // Paging: two at a time, following the token until it stops being issued, reports the
    // same bridges in the same order.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..10 {
        let query = match &token {
            Some(token) => format!("?maxResults=2&nextToken={token}"),
            None => "?maxResults=2".to_string(),
        };
        let (status, code, page) = list_bridges(&emulator, &query).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "page {query:?} expects 200, got {status} {code:?}: {page}"
        );
        let bridges = page["bridges"].as_array().cloned().unwrap_or_default();
        assert!(
            bridges.len() <= 2,
            "page {query:?}: a page holds at most maxResults bridges, got {page}"
        );
        walked.extend(
            bridges
                .iter()
                .map(|summary| summary["bridgeArn"].as_str().unwrap_or_default().to_string()),
        );
        token = page["nextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, every_arn,
        "paging two at a time reports every bridge exactly once, in ARN order"
    );

    // The filter keeps the bridges placed on the gateway it names, and nothing else.
    for (case, filter_arn, expected) in [
        ("the first gateway", placement_a.clone(), {
            let mut expected = on_a.clone();
            expected.sort();
            expected
        }),
        ("the second gateway", placement_b.clone(), {
            let mut expected = on_b.clone();
            expected.sort();
            expected
        }),
        (
            "a gateway no bridge is placed on",
            minted_gateway_arn(7, "gw-empty"),
            Vec::new(),
        ),
    ] {
        let (status, code, filtered) =
            list_bridges(&emulator, &format!("?filterArn={filter_arn}")).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: a filtered listing expects 200, got {status} {code:?}: {filtered}"
        );
        assert_eq!(
            filtered["bridges"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|summary| summary["bridgeArn"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>(),
            expected,
            "case {case:?}: only the bridges placed on {filter_arn} are listed, got {filtered}"
        );
    }
}

/// DeleteBridge removes the bridge: its ARN is answered, DescribeBridge stops reporting
/// it, ListBridges stops listing it, and a second delete of the same ARN resolves nothing.
#[tokio::test]
async fn delete_bridge_removes_the_bridge_from_the_store() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(8, "gw-delete");
    let arn = bridge_with(
        &emulator,
        "bridge-delete",
        &placement,
        vec![network_source("src", 5000, "rtp")],
        vec![network_output("out", 6000, "udp", 32)],
    )
    .await;

    let (status, code, deleted) = delete_bridge(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "delete expected 200, got {status} {code:?}: {deleted}"
    );
    assert_eq!(
        deleted["bridgeArn"].as_str(),
        Some(arn.as_str()),
        "the delete answers the ARN of the bridge it deleted, got {deleted}"
    );

    let (status, code, described) = describe_bridge(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "a deleted bridge is no longer described, got {status} {code:?}: {described}"
    );

    let (status, _, listed) = list_bridges(&emulator, "").await;
    assert_eq!(status, 200, "a listing expects 200, got {listed}");
    assert_eq!(
        listed["bridges"],
        json!([]),
        "a deleted bridge is no longer listed, got {listed}"
    );

    let (status, code, repeated) = delete_bridge(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "deleting the same bridge twice expected 404, got {status} {code:?}: {repeated}"
    );
}

/// Put the stored bridge `arn` into `bridge_state`, the way the operation that moves a
/// bridge into it would.
///
/// The states no implemented operation reaches yet (UpdateBridgeState is still scaffolded)
/// are still values of the modelled `BridgeState` enum a stored bridge can carry, so the
/// table below covers the whole enum from the store rather than leaving most of it
/// unasserted.
fn force_bridge_state(emulator: &Emulator, arn: &str, bridge_state: &str) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let bridge = store
        .bridges
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the fixture bridge {arn} is stored"));
    bridge.bridge_state = bridge_state.to_string();
    // The state is forced, so the transition the create left pending is not owed any more.
    bridge.pending = None;
}

/// "Before you can delete a bridge, you must stop the bridge": every value of the
/// modelled `BridgeState` enum (`smithy_query(query="shape",shape="BridgeState")`), and
/// whether a bridge carrying it has been stopped. The states a bridge reaches by being
/// STARTED — or while moving to or from being started — are the ones a delete refuses;
/// `DELETING` and `DELETED` are named as literals because no handler of this crate
/// produces them.
const EVERY_BRIDGE_STATE: [(&str, bool); 12] = [
    (mc::BRIDGE_STATE_CREATING, true),
    (mc::BRIDGE_STATE_STANDBY, true),
    (mc::BRIDGE_STATE_START_FAILED, true),
    ("DELETING", true),
    ("DELETED", true),
    (mc::BRIDGE_STATE_STARTING, false),
    (mc::BRIDGE_STATE_START_PENDING, false),
    (mc::BRIDGE_STATE_DEPLOYING, false),
    (mc::BRIDGE_STATE_ACTIVE, false),
    (mc::BRIDGE_STATE_STOPPING, false),
    (mc::BRIDGE_STATE_STOP_FAILED, false),
    (mc::BRIDGE_STATE_UPDATING, false),
];

/// A delete is accepted exactly while the bridge is stopped, and a refused delete leaves
/// the bridge describable in the state it was in.
#[tokio::test]
async fn deleting_a_bridge_that_was_not_stopped_is_refused() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(9, "gw-states");

    for (index, (bridge_state, deletable)) in EVERY_BRIDGE_STATE.into_iter().enumerate() {
        let arn = bridge_with(
            &emulator,
            &format!("bridge-state-{index}"),
            &placement,
            vec![network_source("src", 5000, "rtp")],
            Vec::new(),
        )
        .await;
        force_bridge_state(&emulator, &arn, bridge_state);

        let (status, code, answered) = delete_bridge(&emulator, &arn).await;
        if deletable {
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {bridge_state:?}: a stopped bridge is deletable, got {status} {code:?}: \
                 {answered}"
            );
            let (status, code, described) = describe_bridge(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (404, Some("NotFoundException")),
                "case {bridge_state:?}: the deleted bridge is gone, got {status} {code:?}: \
                 {described}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {bridge_state:?}: a bridge that was not stopped must be refused, got \
                 {status} {code:?}: {answered}"
            );
            let bridge = bridge_of(&emulator, bridge_state, &arn).await;
            assert_eq!(
                bridge["bridgeState"].as_str(),
                Some(bridge_state),
                "case {bridge_state:?}: a refused delete leaves the bridge as it was, got \
                 {bridge}"
            );
        }
    }
}

/// "Before you can delete a gateway, you must deregister its instances and delete its
/// bridges": a gateway a bridge is placed on is refused until the bridge is deleted —
/// which is what DeleteBridge is for — and accepted afterwards.
#[tokio::test]
async fn a_gateway_a_bridge_is_placed_on_is_deleted_only_after_the_bridge() {
    let emulator = Emulator::new();

    let (status, code, created) = emulator
        .send(
            "POST",
            "/v1/gateways",
            Some(json!({
                "name": "gw-with-bridge",
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": [{ "name": "net-1", "cidrBlock": "10.0.0.0/24" }],
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the gateway fixture expects 201, got {status} {code:?}: {created}"
    );
    let gateway_arn = created["gateway"]["gatewayArn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    let bridge_arn = bridge_with(
        &emulator,
        "bridge-on-gateway",
        &gateway_arn,
        vec![network_source("src", 5000, "rtp")],
        Vec::new(),
    )
    .await;

    let (status, code, refused) = emulator
        .send("DELETE", &format!("/v1/gateways/{gateway_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "a gateway with a bridge placed on it must be refused, got {status} {code:?}: {refused}"
    );

    let (status, code, deleted) = delete_bridge(&emulator, &bridge_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the bridge delete expects 200, got {status} {code:?}: {deleted}"
    );

    let (status, code, deleted) = emulator
        .send("DELETE", &format!("/v1/gateways/{gateway_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "with its bridge deleted the gateway is deletable, got {status} {code:?}: {deleted}"
    );
}
