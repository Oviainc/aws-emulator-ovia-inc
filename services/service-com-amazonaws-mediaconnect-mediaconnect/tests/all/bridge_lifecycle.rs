// SPDX-License-Identifier: CC0-1.0

//! The bridge lifecycle, end to end over HTTP: CreateBridge -> DescribeBridge.
//!
//! The subject is CreateBridge: it must STORE one bridge and answer with the record it
//! stored — the ARN it minted, the placement it was given, the bridge type the request
//! named, every source and every output projected into its modelled shape, and a state
//! the clock moves — so that DescribeBridge afterwards reports the same bridge. The cases
//! below vary what that rule generalises over: the bridge type (egress / ingress), the
//! kind of each source (flow / network), how many sources and outputs one request carries
//! (from one up to the per-request bound), which protocol an entry names, and whether the
//! request carries a failover config and a recovery window.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches: routing,
//! restJson1 deserialization, constraint validation, handler, serialization.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::PROTOCOL_VALUES;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};

/// A flow ARN of the form this service mints, built through the crate's own encoding of
/// it, so it satisfies the pattern the model puts on
/// `AddBridgeFlowSourceRequest$FlowArn` (`^arn:.+:mediaconnect.+:flow:.+$`) the way a
/// real CreateFlow answer would.
fn minted_flow_arn(suffix: u64, name: &str) -> String {
    engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::flow_resource_fragment(suffix, name),
    )
}

/// A gateway ARN of the form this service mints — what a bridge's `PlacementArn` names
/// (`ListedBridge$PlacementArn`: "the ARN of the gateway associated with the bridge").
fn minted_gateway_arn(suffix: u64, name: &str) -> String {
    engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::gateway_resource_fragment(suffix, name),
    )
}

/// Every ARN CreateBridge mints must be one this service could have minted: the model
/// puts no pattern on `BridgeArn` (it carries an `@arnReference` to
/// `AWS::MediaConnect::Bridge`), so the form is the crate's own and is asserted through
/// the crate's own encoding of it — then re-asserted field by field, so a bug in that
/// function cannot hide here.
fn assert_minted_bridge_arn(case: &str, arn: &str, name: &str) {
    assert!(
        mc::is_mintable_bridge_arn(arn),
        "case {case:?}: minted BridgeArn must be one this service could mint, got {arn:?}"
    );
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        8,
        "case {case:?}: a bridge ARN has 8 colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", REGION, "bridge"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the account id is 12 digits, got {arn:?}"
    );
    assert_eq!(
        segments[7], name,
        "case {case:?}: the last segment is the bridge name, got {arn:?}"
    );
}

async fn create_bridge(emulator: &Emulator, body: Value) -> (u16, Option<String>, Value) {
    emulator.send("POST", "/v1/bridges", Some(body)).await
}

async fn describe_bridge(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/bridges/{arn}"), None)
        .await
}

/// One row of the CreateBridge table: a request body, and what the rule says the stored
/// bridge must look like for it.
struct CreateCase {
    case: &'static str,
    name: &'static str,
    body: Value,
    /// The bridge-type member the answer must carry, and its projected value. The other
    /// one must be absent: a bridge is egress OR ingress.
    bridge_type: (&'static str, Value),
    /// `Bridge$Sources`, projected. `Value::Null` means the member must be absent.
    sources: Value,
    /// `Bridge$Outputs`, projected. `Value::Null` means the member must be absent.
    outputs: Value,
    /// `Bridge$SourceFailoverConfig`, projected. `Value::Null` means absent.
    source_failover_config: Value,
}

/// The CreateBridge table. Every row differs from the others along what the rule
/// generalises over, never only in a name.
fn create_cases() -> Vec<CreateCase> {
    let placement = minted_gateway_arn(1, "gw-placement");
    let flow_a = minted_flow_arn(2, "flow-a");
    let flow_b = minted_flow_arn(3, "flow-b");

    // The bound row: one request carrying the most sources and the most outputs a create
    // may carry (`mc::MAX_BRIDGE_SOURCES` / `mc::MAX_BRIDGE_OUTPUTS`). One past each bound
    // is in the refusal table below.
    let full_sources: Vec<Value> = (0..mc::MAX_BRIDGE_SOURCES)
        .map(|index| {
            json!({ "networkSource": {
                "multicastIp": format!("224.0.1.{index}"),
                "name": format!("src-{index}"),
                "networkName": "net-1",
                "port": 5000 + index as i64,
                "protocol": "rtp",
            }})
        })
        .collect();
    let full_outputs: Vec<Value> = (0..mc::MAX_BRIDGE_OUTPUTS)
        .map(|index| {
            json!({ "networkOutput": {
                "ipAddress": format!("10.0.1.{index}"),
                "name": format!("out-{index}"),
                "networkName": "net-1",
                "port": 6000 + index as i64,
                "protocol": "udp",
                "ttl": 32,
            }})
        })
        .collect();
    let full_projected_sources: Vec<Value> = full_sources
        .iter()
        .map(|source| json!({ "networkSource": source["networkSource"].clone() }))
        .collect();
    let full_projected_outputs: Vec<Value> = full_outputs
        .iter()
        .map(|output| json!({ "networkOutput": output["networkOutput"].clone() }))
        .collect();

    vec![
        CreateCase {
            case: "egress bridge, one flow source, no outputs, no failover",
            name: "bridge-egress-min",
            body: json!({
                "name": "bridge-egress-min",
                "placementArn": placement,
                "egressGatewayBridge": { "maxBitrate": 80_000_000 },
                "sources": [{ "flowSource": { "flowArn": flow_a, "name": "flow-src" } }],
            }),
            bridge_type: ("egressGatewayBridge", json!({ "maxBitrate": 80_000_000 })),
            sources: json!([{ "flowSource": { "flowArn": flow_a, "name": "flow-src" } }]),
            outputs: Value::Null,
            source_failover_config: Value::Null,
        },
        CreateCase {
            case: "ingress bridge, one network source, one network output",
            name: "bridge-ingress-1",
            body: json!({
                "name": "bridge-ingress-1",
                "placementArn": placement,
                "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 4 },
                "sources": [{ "networkSource": {
                    "multicastIp": "224.0.0.1",
                    "name": "net-src",
                    "networkName": "net-1",
                    "port": 5000,
                    "protocol": "rtp-fec",
                }}],
                "outputs": [{ "networkOutput": {
                    "ipAddress": "10.0.0.5",
                    "name": "net-out",
                    "networkName": "net-1",
                    "port": 5001,
                    "protocol": "rtp",
                    "ttl": 64,
                }}],
            }),
            bridge_type: (
                "ingressGatewayBridge",
                json!({ "maxBitrate": 20_000_000, "maxOutputs": 4 }),
            ),
            sources: json!([{ "networkSource": {
                "multicastIp": "224.0.0.1",
                "name": "net-src",
                "networkName": "net-1",
                "port": 5000,
                "protocol": "rtp-fec",
            }}]),
            outputs: json!([{ "networkOutput": {
                "ipAddress": "10.0.0.5",
                "name": "net-out",
                "networkName": "net-1",
                "port": 5001,
                "protocol": "rtp",
                "ttl": 64,
            }}]),
            source_failover_config: Value::Null,
        },
        CreateCase {
            case: "egress bridge, two flow sources (one with a VPC attachment), failover \
                   without a recovery window, two outputs on different protocols",
            name: "bridge-egress-failover",
            body: json!({
                "name": "bridge-egress-failover",
                "placementArn": placement,
                "egressGatewayBridge": { "maxBitrate": 120_000_000 },
                "sources": [
                    { "flowSource": {
                        "flowArn": flow_a,
                        "name": "primary",
                        "flowVpcInterfaceAttachment": { "vpcInterfaceName": "vpc-1" },
                    }},
                    { "flowSource": { "flowArn": flow_b, "name": "secondary" } },
                ],
                "sourceFailoverConfig": {
                    "failoverMode": "FAILOVER",
                    "state": "ENABLED",
                    "sourcePriority": { "primarySource": "primary" },
                },
                "outputs": [
                    { "networkOutput": {
                        "ipAddress": "10.0.0.6",
                        "name": "out-udp",
                        "networkName": "net-1",
                        "port": 5002,
                        "protocol": "udp",
                        "ttl": 16,
                    }},
                    { "networkOutput": {
                        "ipAddress": "10.0.0.7",
                        "name": "out-srt",
                        "networkName": "net-2",
                        "port": 5003,
                        "protocol": "srt-listener",
                        "ttl": 255,
                    }},
                ],
            }),
            bridge_type: ("egressGatewayBridge", json!({ "maxBitrate": 120_000_000 })),
            sources: json!([
                { "flowSource": {
                    "flowArn": flow_a,
                    "name": "primary",
                    "flowVpcInterfaceAttachment": { "vpcInterfaceName": "vpc-1" },
                }},
                { "flowSource": { "flowArn": flow_b, "name": "secondary" } },
            ]),
            outputs: json!([
                { "networkOutput": {
                    "ipAddress": "10.0.0.6",
                    "name": "out-udp",
                    "networkName": "net-1",
                    "port": 5002,
                    "protocol": "udp",
                    "ttl": 16,
                }},
                { "networkOutput": {
                    "ipAddress": "10.0.0.7",
                    "name": "out-srt",
                    "networkName": "net-2",
                    "port": 5003,
                    "protocol": "srt-listener",
                    "ttl": 255,
                }},
            ]),
            // The window the request named none for: the service's own assignment
            // (`mc::ASSIGNED_RECOVERY_WINDOW_MS`), the same one a flow's failover config
            // reports.
            source_failover_config: json!({
                "failoverMode": "FAILOVER",
                "state": "ENABLED",
                "sourcePriority": { "primarySource": "primary" },
                "recoveryWindow": mc::ASSIGNED_RECOVERY_WINDOW_MS,
            }),
        },
        CreateCase {
            case: "ingress bridge, network source with multicast source settings, failover \
                   with an explicit recovery window",
            name: "bridge-ingress-multicast",
            body: json!({
                "name": "bridge-ingress-multicast",
                "placementArn": placement,
                "ingressGatewayBridge": { "maxBitrate": 5_000_000, "maxOutputs": 1 },
                "sources": [{ "networkSource": {
                    "multicastIp": "239.0.0.9",
                    "multicastSourceSettings": { "multicastSourceIp": "10.9.9.9" },
                    "name": "mcast-src",
                    "networkName": "net-7",
                    "port": 9000,
                    "protocol": "st2110-jpegxs",
                }}],
                "sourceFailoverConfig": { "state": "DISABLED", "recoveryWindow": 3_000 },
            }),
            bridge_type: (
                "ingressGatewayBridge",
                json!({ "maxBitrate": 5_000_000, "maxOutputs": 1 }),
            ),
            sources: json!([{ "networkSource": {
                "multicastIp": "239.0.0.9",
                "multicastSourceSettings": { "multicastSourceIp": "10.9.9.9" },
                "name": "mcast-src",
                "networkName": "net-7",
                "port": 9000,
                "protocol": "st2110-jpegxs",
            }}]),
            outputs: Value::Null,
            // An explicit window is KEPT, whatever the service would have assigned.
            source_failover_config: json!({ "state": "DISABLED", "recoveryWindow": 3_000 }),
        },
        CreateCase {
            case: "ingress bridge at the per-request bound: the most sources and the most \
                   outputs one create may carry",
            name: "bridge-full",
            body: json!({
                "name": "bridge-full",
                "placementArn": placement,
                "ingressGatewayBridge": { "maxBitrate": 1, "maxOutputs": 0 },
                "sources": full_sources,
                "outputs": full_outputs,
            }),
            bridge_type: (
                "ingressGatewayBridge",
                json!({ "maxBitrate": 1, "maxOutputs": 0 }),
            ),
            sources: json!(full_projected_sources),
            outputs: json!(full_projected_outputs),
            source_failover_config: Value::Null,
        },
    ]
}

#[tokio::test]
async fn create_bridge_stores_the_bridge_describe_bridge_reports() {
    let emulator = Emulator::new();

    for case in create_cases() {
        let CreateCase {
            case: name_of_case,
            name,
            body,
            bridge_type,
            sources,
            outputs,
            source_failover_config,
        } = case;

        let (status, code, created) = create_bridge(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {name_of_case:?}: create expected 201, got {status} {code:?}: {created}"
        );

        let bridge = &created["bridge"];
        let bridge_arn = bridge["bridgeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name_of_case:?}: the create answers an ARN: {created}"))
            .to_string();
        assert_minted_bridge_arn(name_of_case, &bridge_arn, name);
        assert_eq!(
            bridge["name"].as_str(),
            Some(name),
            "case {name_of_case:?}: the stored bridge keeps its name, got {bridge}"
        );
        assert_eq!(
            bridge["bridgeState"].as_str(),
            Some(mc::BRIDGE_STATE_CREATING),
            "case {name_of_case:?}: a new bridge is reported in its transient state, got \
             {bridge}"
        );

        // The bridge type the request named, and ONLY that one.
        let (type_member, expected_type) = bridge_type;
        assert_eq!(
            bridge[type_member], expected_type,
            "case {name_of_case:?}: the {type_member} the request named, got {bridge}"
        );
        let other_member = match type_member {
            "egressGatewayBridge" => "ingressGatewayBridge",
            _ => "egressGatewayBridge",
        };
        assert_eq!(
            bridge[other_member],
            Value::Null,
            "case {name_of_case:?}: a bridge of one type reports no {other_member}, got {bridge}"
        );

        for (member, expected) in [
            ("sources", &sources),
            ("outputs", &outputs),
            ("sourceFailoverConfig", &source_failover_config),
        ] {
            assert_eq!(
                &bridge[member], expected,
                "case {name_of_case:?}: expected {member} {expected}, got {:?}",
                bridge[member]
            );
        }

        // The round trip: DescribeBridge reports the record the create stored. The state is
        // the one member the CLOCK may have moved on in between (see the transition test
        // below), so it is asserted to be one of the two states creation reaches rather
        // than compared byte for byte.
        let (status, code, described) = describe_bridge(&emulator, &bridge_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {name_of_case:?}: describe expected 200, got {status} {code:?}: {described}"
        );
        for member in [
            "bridgeArn",
            "name",
            "placementArn",
            "sources",
            "outputs",
            "sourceFailoverConfig",
            "egressGatewayBridge",
            "ingressGatewayBridge",
        ] {
            assert_eq!(
                described["bridge"][member], bridge[member],
                "case {name_of_case:?}: DescribeBridge must report the created {member}, got {:?}",
                described["bridge"][member]
            );
        }
        let described_state = described["bridge"]["bridgeState"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            [mc::BRIDGE_STATE_CREATING, mc::BRIDGE_STATE_STANDBY]
                .contains(&described_state.as_str()),
            "case {name_of_case:?}: a described bridge is CREATING or STANDBY, got \
             {described_state:?}"
        );
    }
}

/// Every protocol the model declares (`PROTOCOL_VALUES`, generated from
/// `smithy_query(query="shape",shape="Protocol")`) is stored and reported as given: the
/// handler resolves no protocol of its own. The row for `fujitsu-qos` is one no other case
/// here names — "Elemental MediaConnect no longer supports the Fujitsu QoS protocol. This
/// reference is maintained for legacy purposes only" — and it is accepted like the rest,
/// because the model still declares it.
#[tokio::test]
async fn every_modelled_protocol_is_stored_on_a_bridge_source_and_output() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(1, "gw-protocols");

    for protocol in PROTOCOL_VALUES {
        let (status, code, created) = create_bridge(
            &emulator,
            json!({
                "name": format!("bridge-{}", protocol.replace('-', "")),
                "placementArn": placement,
                "ingressGatewayBridge": { "maxBitrate": 10_000, "maxOutputs": 2 },
                "sources": [{ "networkSource": {
                    "multicastIp": "224.0.0.2",
                    "name": "src",
                    "networkName": "net-1",
                    "port": 5000,
                    "protocol": protocol,
                }}],
                "outputs": [{ "networkOutput": {
                    "ipAddress": "10.0.0.8",
                    "name": "out",
                    "networkName": "net-1",
                    "port": 5001,
                    "protocol": protocol,
                    "ttl": 8,
                }}],
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {protocol:?}: create expected 201, got {status} {code:?}: {created}"
        );
        assert_eq!(
            created["bridge"]["sources"][0]["networkSource"]["protocol"].as_str(),
            Some(*protocol),
            "case {protocol:?}: the source keeps the protocol the request named, got {created}"
        );
        assert_eq!(
            created["bridge"]["outputs"][0]["networkOutput"]["protocol"].as_str(),
            Some(*protocol),
            "case {protocol:?}: the output keeps the protocol the request named, got {created}"
        );
    }

    // And a value the enum does not declare is refused at the boundary, before any of this.
    let (status, code, body) = create_bridge(
        &emulator,
        json!({
            "name": "bridge-bad-protocol",
            "placementArn": placement,
            "ingressGatewayBridge": { "maxBitrate": 10_000, "maxOutputs": 2 },
            "sources": [{ "networkSource": {
                "multicastIp": "224.0.0.2",
                "name": "src",
                "networkName": "net-1",
                "port": 5000,
                "protocol": "carrier-pigeon",
            }}],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a protocol the Protocol enum does not declare is refused with 400, got {status} \
         {code:?}: {body}"
    );
}

/// One request body per way a create request can be semantically wrong, and the refusal
/// the model names for it: `BadRequestException`, "thrown if the request contains a
/// semantic error" (HTTP 400). Nothing is stored for any of them.
fn refusal_cases() -> Vec<(&'static str, Value)> {
    let placement = minted_gateway_arn(1, "gw-refusals");
    let flow = minted_flow_arn(2, "flow-r");
    let source = json!({ "networkSource": {
        "multicastIp": "224.0.0.3",
        "name": "src",
        "networkName": "net-1",
        "port": 5000,
        "protocol": "rtp",
    }});
    let output = json!({ "networkOutput": {
        "ipAddress": "10.0.0.9",
        "name": "out",
        "networkName": "net-1",
        "port": 5001,
        "protocol": "rtp",
        "ttl": 4,
    }});
    let ingress = json!({ "maxBitrate": 10_000, "maxOutputs": 2 });
    let egress = json!({ "maxBitrate": 10_000 });
    // One source / output past each per-request bound, the row beside the bound row above.
    let too_many_sources: Vec<Value> = (0..=mc::MAX_BRIDGE_SOURCES)
        .map(|index| {
            let mut entry = source.clone();
            entry["networkSource"]["name"] = json!(format!("src-{index}"));
            entry
        })
        .collect();
    let too_many_outputs: Vec<Value> = (0..=mc::MAX_BRIDGE_OUTPUTS)
        .map(|index| {
            let mut entry = output.clone();
            entry["networkOutput"]["name"] = json!(format!("out-{index}"));
            entry
        })
        .collect();

    vec![
        (
            "no source at all: the request must include one source",
            json!({
                "name": "b", "placementArn": placement,
                "ingressGatewayBridge": ingress, "sources": [],
            }),
        ),
        (
            "one source past the per-request bound",
            json!({
                "name": "b", "placementArn": placement,
                "ingressGatewayBridge": ingress, "sources": too_many_sources,
            }),
        ),
        (
            "one output past the per-request bound",
            json!({
                "name": "b", "placementArn": placement,
                "ingressGatewayBridge": ingress, "sources": [source],
                "outputs": too_many_outputs,
            }),
        ),
        (
            "both bridge types: a bridge is egress or ingress, never both",
            json!({
                "name": "b", "placementArn": placement,
                "ingressGatewayBridge": ingress, "egressGatewayBridge": egress,
                "sources": [source],
            }),
        ),
        (
            "neither bridge type: the request describes no bridge",
            json!({ "name": "b", "placementArn": placement, "sources": [source] }),
        ),
        (
            "two sources with one name: a source name must be unique in a bridge",
            json!({
                "name": "b", "placementArn": placement,
                "ingressGatewayBridge": ingress, "sources": [source, source],
            }),
        ),
        (
            "two outputs with one name: an output name must be unique in a bridge",
            json!({
                "name": "b", "placementArn": placement,
                "ingressGatewayBridge": ingress, "sources": [source],
                "outputs": [output, output],
            }),
        ),
        (
            "a source entry naming both a flow source and a network source",
            json!({
                "name": "b", "placementArn": placement,
                "egressGatewayBridge": egress,
                "sources": [{
                    "flowSource": { "flowArn": flow, "name": "f" },
                    "networkSource": source["networkSource"],
                }],
            }),
        ),
        (
            "a source entry naming neither kind of source",
            json!({
                "name": "b", "placementArn": placement,
                "egressGatewayBridge": egress, "sources": [{}],
            }),
        ),
        (
            "an output entry naming no network output",
            json!({
                "name": "b", "placementArn": placement,
                "ingressGatewayBridge": ingress, "sources": [source], "outputs": [{}],
            }),
        ),
        (
            "an unnamed source: the name is how a bridge addresses its source",
            json!({
                "name": "b", "placementArn": placement,
                "egressGatewayBridge": egress,
                "sources": [{ "flowSource": { "flowArn": flow, "name": "" } }],
            }),
        ),
        (
            "a source name carrying the URI path separator",
            json!({
                "name": "b", "placementArn": placement,
                "egressGatewayBridge": egress,
                "sources": [{ "flowSource": { "flowArn": flow, "name": "a/b" } }],
            }),
        ),
        (
            "an empty bridge name: the name is the last segment of the minted ARN",
            json!({
                "name": "", "placementArn": placement,
                "ingressGatewayBridge": ingress, "sources": [source],
            }),
        ),
        (
            "a bridge name carrying the ARN segment separator",
            json!({
                "name": "a:b", "placementArn": placement,
                "ingressGatewayBridge": ingress, "sources": [source],
            }),
        ),
        (
            "a bridge name carrying the URI path separator",
            json!({
                "name": "a/b", "placementArn": placement,
                "ingressGatewayBridge": ingress, "sources": [source],
            }),
        ),
        (
            "no placement: a bridge is placed on a gateway",
            json!({
                "name": "b", "placementArn": "",
                "ingressGatewayBridge": ingress, "sources": [source],
            }),
        ),
    ]
}

#[tokio::test]
async fn a_semantically_invalid_create_bridge_request_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    for (case, body) in refusal_cases() {
        let (status, code, parsed) = create_bridge(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {parsed}"
        );
        let message = parsed["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: the refusal states what was wrong, got {message:?}"
        );
    }

    let stored = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>()
        .lock()
        .bridges
        .len();
    assert_eq!(
        stored, 0,
        "a refused create stores nothing: expected an empty bridge store, got {stored} bridges"
    );
}

/// A new bridge is answered in the non-terminal `CREATING` state and the CLOCK settles it
/// into `STANDBY` — a created bridge is not running until its state is asked to change.
/// DescribeBridge reports the state the store holds, and nothing else moves.
#[tokio::test]
async fn the_clock_settles_a_new_bridge() {
    let emulator = Emulator::new();

    // The placement and the flow source are real records of this emulator, minted by the
    // operations that own them, so this case is the one a client actually drives.
    let (status, _, gateway) = emulator
        .send(
            "POST",
            "/v1/gateways",
            Some(json!({
                "name": "gw-for-bridge",
                "egressCidrBlocks": ["10.0.0.0/16"],
                "networks": [{ "name": "net-1", "cidrBlock": "10.0.0.0/24" }],
            })),
        )
        .await;
    assert_eq!(status, 201, "create gateway expected 201, got {gateway}");
    let placement_arn = gateway["gateway"]["gatewayArn"]
        .as_str()
        .expect("the create answers a gateway ARN")
        .to_string();

    let (status, _, flow) = emulator
        .create_flow(json!({
            "name": "flow-for-bridge",
            "source": { "name": "src1", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
        }))
        .await;
    assert_eq!(status, 201, "create flow expected 201, got {flow}");
    let flow_arn = flow["flow"]["flowArn"]
        .as_str()
        .expect("the create answers a flow ARN")
        .to_string();

    let (status, _, created) = create_bridge(
        &emulator,
        json!({
            "name": "bridge-lifecycle",
            "placementArn": placement_arn,
            "egressGatewayBridge": { "maxBitrate": 60_000_000 },
            "sources": [{ "flowSource": { "flowArn": flow_arn, "name": "from-flow" } }],
        }),
    )
    .await;
    assert_eq!(status, 201, "create bridge expected 201, got {created}");
    let bridge_arn = created["bridge"]["bridgeArn"]
        .as_str()
        .expect("the create answers a bridge ARN")
        .to_string();
    assert_eq!(
        created["bridge"]["placementArn"].as_str(),
        Some(placement_arn.as_str()),
        "the bridge is placed on the gateway the request named, got {created}"
    );

    // A caller acting straight after the create still sees the transient state.
    let (_, _, described) = describe_bridge(&emulator, &bridge_arn).await;
    assert_eq!(
        described["bridge"]["bridgeState"].as_str(),
        Some(mc::BRIDGE_STATE_CREATING),
        "a bridge read straight after the create is still CREATING, got {described}"
    );

    // Wait out the transition the clock owes, then the settled state is reported.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let (_, _, described) = describe_bridge(&emulator, &bridge_arn).await;
    assert_eq!(
        described["bridge"]["bridgeState"].as_str(),
        Some(mc::BRIDGE_STATE_STANDBY),
        "the clock settles the bridge into STANDBY, got {described}"
    );
    assert_eq!(
        described["bridge"]["sources"],
        created["bridge"]["sources"],
        "settling the state changes nothing else about the bridge, got {described}"
    );
}

/// Two bridges created with ONE name are two resources: the bridge id in the ARN keeps
/// them apart, and each read answers its own record. A bridge ARN no store holds is
/// `NotFoundException` (HTTP 404), the error the model declares for DescribeBridge.
#[tokio::test]
async fn same_named_bridges_are_distinct_and_an_unknown_bridge_is_not_found() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(1, "gw-twins");
    let body = |source_name: &str| {
        json!({
            "name": "twin",
            "placementArn": placement,
            "ingressGatewayBridge": { "maxBitrate": 1_000, "maxOutputs": 1 },
            "sources": [{ "networkSource": {
                "multicastIp": "224.0.0.4",
                "name": source_name,
                "networkName": "net-1",
                "port": 5000,
                "protocol": "rtp",
            }}],
        })
    };

    let (_, _, first) = create_bridge(&emulator, body("first-src")).await;
    let (_, _, second) = create_bridge(&emulator, body("second-src")).await;
    let first_arn = first["bridge"]["bridgeArn"]
        .as_str()
        .expect("an ARN")
        .to_string();
    let second_arn = second["bridge"]["bridgeArn"]
        .as_str()
        .expect("an ARN")
        .to_string();
    assert_ne!(
        first_arn, second_arn,
        "two bridges sharing a name are distinct resources with distinct ARNs"
    );

    for (case, arn, expected_source) in [
        ("first", &first_arn, "first-src"),
        ("second", &second_arn, "second-src"),
    ] {
        let (status, _, described) = describe_bridge(&emulator, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status}: {described}"
        );
        assert_eq!(
            described["bridge"]["sources"][0]["networkSource"]["name"].as_str(),
            Some(expected_source),
            "case {case:?}: each bridge answers its own record, got {described}"
        );
    }

    let unknown = minted_gateway_arn(99, "never-created").replace(":gateway:", ":bridge:");
    let (status, code, parsed) = describe_bridge(&emulator, &unknown).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "a bridge ARN no store holds is not found, got {status} {code:?}: {parsed}"
    );
}
