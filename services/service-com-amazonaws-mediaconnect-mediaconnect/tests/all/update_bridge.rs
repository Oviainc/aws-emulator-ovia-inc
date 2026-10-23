// SPDX-License-Identifier: CC0-1.0

//! UpdateBridge, end to end over HTTP: CreateBridge -> UpdateBridge -> DescribeBridge.
//!
//! The rule under test: UpdateBridge is a PARTIAL update of the STORED bridge — every
//! setting the request carries replaces that setting on the bridge, member by member
//! through the request-only wrapper shapes, every setting it omits is left exactly as it
//! was, nothing else about the bridge moves, and the answer is the stored record, so
//! DescribeBridge afterwards reports the same bridge.
//!
//! The rows below vary what that rule generalises over: which of the three updatable
//! settings one request carries (one, two, all of them, none at all), the bridge type the
//! request addresses (egress / ingress), which members of a wrapper the request names
//! (`MaxBitrate` alone, `MaxOutputs` alone, both), and whether the setting being merged
//! into already exists on the bridge or not.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::minted_gateway_arn;
use crate::flow_lifecycle::{Emulator, REGION};

/// The members of a bridge UpdateBridge may write, and the ones it may not: the answer and
/// the stored record are compared member by member over both lists.
const UPDATABLE_MEMBERS: [&str; 3] = [
    "egressGatewayBridge",
    "ingressGatewayBridge",
    "sourceFailoverConfig",
];
/// See [`UPDATABLE_MEMBERS`]. `bridgeState` is left out because the CLOCK may move it
/// between two reads (see `mc::TRANSITION_SPAN`); the test below that pins the state does
/// it after the transition has settled.
const UNTOUCHED_MEMBERS: [&str; 5] = [
    "bridgeArn",
    "name",
    "placementArn",
    "sources",
    "outputs",
];

async fn create_bridge(emulator: &Emulator, body: Value) -> (u16, Option<String>, Value) {
    emulator.send("POST", "/v1/bridges", Some(body)).await
}

async fn update_bridge(emulator: &Emulator, arn: &str, body: Value) -> (u16, Option<String>, Value) {
    emulator
        .send("PUT", &format!("/v1/bridges/{arn}"), Some(body))
        .await
}

async fn describe_bridge(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/bridges/{arn}"), None)
        .await
}

/// One network source, so every create below carries the one source CreateBridge requires.
fn network_source(name: &str, port: i64) -> Value {
    json!({ "networkSource": {
        "multicastIp": "224.0.0.1",
        "name": name,
        "networkName": "net-1",
        "port": port,
        "protocol": "rtp",
    }})
}

/// One row of the UpdateBridge table: the bridge to create, the update to apply, and what
/// the rule says each updatable setting must be afterwards.
struct UpdateCase {
    case: &'static str,
    create: Value,
    update: Value,
    /// `Bridge$EgressGatewayBridge` after the update; `Value::Null` means absent.
    egress: Value,
    /// `Bridge$IngressGatewayBridge` after the update; `Value::Null` means absent.
    ingress: Value,
    /// `Bridge$SourceFailoverConfig` after the update; `Value::Null` means absent.
    failover: Value,
}

fn update_cases() -> Vec<UpdateCase> {
    let placement = minted_gateway_arn(1, "gw-updates");
    let egress = |name: &str, failover: Option<Value>| {
        let mut body = json!({
            "name": name,
            "placementArn": placement,
            "egressGatewayBridge": { "maxBitrate": 80_000_000 },
            "sources": [network_source("src", 5000)],
        });
        if let Some(config) = failover {
            body["sourceFailoverConfig"] = config;
        }
        body
    };
    let ingress = |name: &str, failover: Option<Value>| {
        let mut body = json!({
            "name": name,
            "placementArn": placement,
            "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 4 },
            "sources": [network_source("src", 5001)],
        });
        if let Some(config) = failover {
            body["sourceFailoverConfig"] = config;
        }
        body
    };
    // The failover config a row starts from, and the one the service stores for it: a
    // request that names no recovery window gets the service's assignment
    // (`mc::ASSIGNED_RECOVERY_WINDOW_MS`), the same on the create path and on this one.
    let stored_failover = json!({
        "failoverMode": "FAILOVER",
        "state": mc::STATE_ENABLED,
        "sourcePriority": { "primarySource": "src" },
        "recoveryWindow": 4_000,
    });

    vec![
        UpdateCase {
            case: "egress bridge, MaxBitrate alone: the one member the egress wrapper has",
            create: egress("bridge-egress-bitrate", None),
            update: json!({ "egressGatewayBridge": { "maxBitrate": 150_000_000 } }),
            egress: json!({ "maxBitrate": 150_000_000 }),
            ingress: Value::Null,
            failover: Value::Null,
        },
        UpdateCase {
            case: "ingress bridge, MaxBitrate alone: MaxOutputs keeps its stored value",
            create: ingress("bridge-ingress-bitrate", None),
            update: json!({ "ingressGatewayBridge": { "maxBitrate": 1_000 } }),
            egress: Value::Null,
            ingress: json!({ "maxBitrate": 1_000, "maxOutputs": 4 }),
            failover: Value::Null,
        },
        UpdateCase {
            case: "ingress bridge, MaxOutputs alone: MaxBitrate keeps its stored value",
            create: ingress("bridge-ingress-outputs", None),
            update: json!({ "ingressGatewayBridge": { "maxOutputs": 0 } }),
            egress: Value::Null,
            ingress: json!({ "maxBitrate": 20_000_000, "maxOutputs": 0 }),
            failover: Value::Null,
        },
        UpdateCase {
            case: "ingress bridge, both wrapper members at once",
            create: ingress("bridge-ingress-both", None),
            update: json!({
                "ingressGatewayBridge": { "maxBitrate": 7, "maxOutputs": 20 },
            }),
            egress: Value::Null,
            ingress: json!({ "maxBitrate": 7, "maxOutputs": 20 }),
            failover: Value::Null,
        },
        UpdateCase {
            case: "egress bridge, an empty wrapper: it names no member, so nothing moves",
            create: egress("bridge-egress-empty-wrapper", None),
            update: json!({ "egressGatewayBridge": {} }),
            egress: json!({ "maxBitrate": 80_000_000 }),
            ingress: Value::Null,
            failover: Value::Null,
        },
        UpdateCase {
            case: "egress bridge, FailoverMode alone: the rest of the stored failover \
                   config is kept",
            create: egress("bridge-egress-mode", Some(stored_failover.clone())),
            update: json!({ "sourceFailoverConfig": { "failoverMode": "MERGE" } }),
            egress: json!({ "maxBitrate": 80_000_000 }),
            ingress: Value::Null,
            failover: json!({
                "failoverMode": "MERGE",
                "state": mc::STATE_ENABLED,
                "sourcePriority": { "primarySource": "src" },
                "recoveryWindow": 4_000,
            }),
        },
        UpdateCase {
            case: "ingress bridge, RecoveryWindow alone replaces the stored window",
            create: ingress("bridge-ingress-window", Some(stored_failover.clone())),
            update: json!({ "sourceFailoverConfig": { "recoveryWindow": 9_999 } }),
            egress: Value::Null,
            ingress: json!({ "maxBitrate": 20_000_000, "maxOutputs": 4 }),
            failover: json!({
                "failoverMode": "FAILOVER",
                "state": mc::STATE_ENABLED,
                "sourcePriority": { "primarySource": "src" },
                "recoveryWindow": 9_999,
            }),
        },
        UpdateCase {
            case: "egress bridge, SourcePriority and State together, window untouched",
            create: egress("bridge-egress-priority", Some(stored_failover.clone())),
            update: json!({ "sourceFailoverConfig": {
                "state": mc::STATE_DISABLED,
                "sourcePriority": { "primarySource": "other" },
            }}),
            egress: json!({ "maxBitrate": 80_000_000 }),
            ingress: Value::Null,
            failover: json!({
                "failoverMode": "FAILOVER",
                "state": mc::STATE_DISABLED,
                "sourcePriority": { "primarySource": "other" },
                "recoveryWindow": 4_000,
            }),
        },
        UpdateCase {
            case: "ingress bridge with NO failover config: the merge starts from nothing, \
                   so the window the request omits is the service's assignment",
            create: ingress("bridge-ingress-new-failover", None),
            update: json!({ "sourceFailoverConfig": { "state": mc::STATE_ENABLED } }),
            egress: Value::Null,
            ingress: json!({ "maxBitrate": 20_000_000, "maxOutputs": 4 }),
            failover: json!({
                "state": mc::STATE_ENABLED,
                "recoveryWindow": mc::ASSIGNED_RECOVERY_WINDOW_MS,
            }),
        },
        UpdateCase {
            case: "egress bridge, every updatable setting in one request",
            create: egress("bridge-egress-all", Some(stored_failover.clone())),
            update: json!({
                "egressGatewayBridge": { "maxBitrate": 2 },
                "sourceFailoverConfig": {
                    "failoverMode": "MERGE",
                    "recoveryWindow": 100,
                    "state": mc::STATE_DISABLED,
                    "sourcePriority": { "primarySource": "src" },
                },
            }),
            egress: json!({ "maxBitrate": 2 }),
            ingress: Value::Null,
            failover: json!({
                "failoverMode": "MERGE",
                "state": mc::STATE_DISABLED,
                "sourcePriority": { "primarySource": "src" },
                "recoveryWindow": 100,
            }),
        },
        UpdateCase {
            case: "ingress bridge, a request that carries no setting at all: a partial \
                   update of nothing changes nothing",
            create: ingress("bridge-ingress-noop", Some(stored_failover.clone())),
            update: json!({}),
            egress: Value::Null,
            ingress: json!({ "maxBitrate": 20_000_000, "maxOutputs": 4 }),
            failover: stored_failover,
        },
    ]
}

/// Every row: the update writes what it carries, keeps what it omits, leaves the rest of
/// the bridge alone, and DescribeBridge afterwards reports the record it wrote.
#[tokio::test]
async fn update_bridge_applies_the_settings_it_carries_and_keeps_the_rest() {
    let emulator = Emulator::new();

    for case in update_cases() {
        let UpdateCase {
            case,
            create,
            update,
            egress,
            ingress,
            failover,
        } = case;

        let (status, code, created) = create_bridge(&emulator, create).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: create expected 201, got {status} {code:?}: {created}"
        );
        let bridge_arn = created["bridge"]["bridgeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();

        let (status, code, updated) = update_bridge(&emulator, &bridge_arn, update).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: update expected 202, got {status} {code:?}: {updated}"
        );

        let answered = &updated["bridge"];
        for (member, expected) in UPDATABLE_MEMBERS
            .iter()
            .zip([&egress, &ingress, &failover])
        {
            assert_eq!(
                &answered[*member], expected,
                "case {case:?}: expected {member} {expected}, got {:?}",
                answered[*member]
            );
        }
        // Nothing the request did not name moved.
        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                answered[member], created["bridge"][member],
                "case {case:?}: the update leaves {member} alone, got {:?}",
                answered[member]
            );
        }

        // The round trip: the answer is the STORED record, so the next read repeats it.
        let (status, code, described) = describe_bridge(&emulator, &bridge_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
        );
        for member in UPDATABLE_MEMBERS.iter().chain(UNTOUCHED_MEMBERS.iter()) {
            assert_eq!(
                described["bridge"][*member], answered[*member],
                "case {case:?}: DescribeBridge must report the updated {member}, got {:?}",
                described["bridge"][*member]
            );
        }
    }
}

/// `UpdateBridge` is `@idempotent`: sending one request twice leaves the same record, and
/// the second answer is byte for byte the first one (the state aside, which the clock owns
/// and which is settled before the first call here).
#[tokio::test]
async fn repeating_one_update_bridge_request_leaves_the_same_bridge() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(2, "gw-idempotent");

    let (status, _, created) = create_bridge(
        &emulator,
        json!({
            "name": "bridge-idempotent",
            "placementArn": placement,
            "ingressGatewayBridge": { "maxBitrate": 10_000, "maxOutputs": 2 },
            "sources": [network_source("src", 5002)],
        }),
    )
    .await;
    assert_eq!(status, 201, "create bridge expected 201, got {created}");
    let bridge_arn = created["bridge"]["bridgeArn"]
        .as_str()
        .expect("the create answers a bridge ARN")
        .to_string();

    // Let the clock settle the creation first, so the only thing that could differ between
    // the two answers is what the update writes.
    tokio::time::sleep(mc::TRANSITION_SPAN + Duration::from_millis(400)).await;

    let request = json!({
        "ingressGatewayBridge": { "maxBitrate": 55, "maxOutputs": 1 },
        "sourceFailoverConfig": { "state": mc::STATE_ENABLED, "failoverMode": "MERGE" },
    });
    let (first_status, _, first) = update_bridge(&emulator, &bridge_arn, request.clone()).await;
    let (second_status, _, second) = update_bridge(&emulator, &bridge_arn, request).await;
    assert_eq!(
        (first_status, second_status),
        (202, 202),
        "both updates expected 202, got {first_status} and {second_status}: {second}"
    );
    assert_eq!(
        second, first,
        "repeating one update answers the same bridge, got {second} after {first}"
    );

    // The bridge's STATE is not something this operation writes: the model gives it no
    // state member, so a settled bridge is still in the state the clock left it in.
    assert_eq!(
        second["bridge"]["bridgeState"].as_str(),
        Some(mc::BRIDGE_STATE_STANDBY),
        "UpdateBridge does not move the bridge's state, got {second}"
    );
}

/// One request per way an update request cannot be satisfied, the refusal the model names
/// for it, and the envelope this service words it in. Nothing is written for any of them.
#[tokio::test]
async fn an_update_bridge_request_the_bridge_cannot_satisfy_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(3, "gw-refusals");

    let (_, _, egress_created) = create_bridge(
        &emulator,
        json!({
            "name": "bridge-egress-refusals",
            "placementArn": placement,
            "egressGatewayBridge": { "maxBitrate": 40_000_000 },
            "sources": [network_source("src", 5003)],
        }),
    )
    .await;
    let egress_arn = egress_created["bridge"]["bridgeArn"]
        .as_str()
        .expect("an ARN")
        .to_string();
    let (_, _, ingress_created) = create_bridge(
        &emulator,
        json!({
            "name": "bridge-ingress-refusals",
            "placementArn": placement,
            "ingressGatewayBridge": { "maxBitrate": 30_000, "maxOutputs": 3 },
            "sources": [network_source("src", 5004)],
        }),
    )
    .await;
    let ingress_arn = ingress_created["bridge"]["bridgeArn"]
        .as_str()
        .expect("an ARN")
        .to_string();
    // A bridge ARN of the form this service mints, for a bridge nothing ever created.
    let unknown_arn = engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::bridge_resource_fragment(99, "never-created"),
    );

    let cases: Vec<(&str, &str, Value, u16, &str, &str)> = vec![
        (
            "the ingress wrapper on a cloud-to-ground (egress) bridge",
            &egress_arn,
            json!({ "ingressGatewayBridge": { "maxBitrate": 1 } }),
            400,
            "BadRequestException",
            mc::BAD_REQUEST_PREFIX,
        ),
        (
            "the egress wrapper on a ground-to-cloud (ingress) bridge",
            &ingress_arn,
            json!({ "egressGatewayBridge": { "maxBitrate": 1 } }),
            400,
            "BadRequestException",
            mc::BAD_REQUEST_PREFIX,
        ),
        (
            "both wrappers at once: a bridge is one type or the other",
            &egress_arn,
            json!({
                "egressGatewayBridge": { "maxBitrate": 1 },
                "ingressGatewayBridge": { "maxBitrate": 1 },
            }),
            400,
            "BadRequestException",
            mc::VALIDATION_FAILED_PREFIX,
        ),
        (
            "a bridge ARN no store holds",
            &unknown_arn,
            json!({ "sourceFailoverConfig": { "state": mc::STATE_ENABLED } }),
            404,
            "NotFoundException",
            mc::NOT_FOUND_PREFIX,
        ),
    ];

    for (case, arn, body, expected_status, expected_code, expected_prefix) in cases {
        let (status, code, parsed) = update_bridge(&emulator, arn, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} \
             {code:?}: {parsed}"
        );
        let message = parsed["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(expected_prefix),
            "case {case:?}: the refusal is worded in this service's {expected_prefix:?} \
             envelope, got {message:?}"
        );
    }

    // A refused update writes nothing: both bridges are exactly as they were created.
    for (case, arn, created) in [
        ("egress", &egress_arn, &egress_created),
        ("ingress", &ingress_arn, &ingress_created),
    ] {
        let (status, _, described) = describe_bridge(&emulator, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status}: {described}"
        );
        for member in UPDATABLE_MEMBERS.iter().chain(UNTOUCHED_MEMBERS.iter()) {
            assert_eq!(
                described["bridge"][*member], created["bridge"][*member],
                "case {case:?}: a refused update leaves {member} alone, got {:?}",
                described["bridge"][*member]
            );
        }
    }
}
