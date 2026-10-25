// SPDX-License-Identifier: CC0-1.0

//! Which sources a bridge ingests, and how many, end to end over HTTP: CreateBridge /
//! AddBridgeSources / UpdateBridge -> DescribeBridge.
//!
//! Two rules, both read off the stored bridge:
//!
//! * the bridge's DIRECTION decides the KIND of source it ingests. A cloud-to-ground
//!   (egress) bridge's "content comes from an existing MediaConnect flow"
//!   (`AddEgressGatewayBridgeRequest`), which is the flow source — "a flow source
//!   originates in MediaConnect as an existing cloud flow" (`BridgeFlowSource`) — while a
//!   ground-to-cloud (ingress) bridge's "content originates at your premises"
//!   (`AddIngressGatewayBridgeRequest`), which is the network source — "a network source
//!   originates at your premises" (`BridgeNetworkSource`). So a bridge never mixes the two
//!   kinds; and
//! * its own source failover state decides HOW MANY it ingests: "if the state is inactive,
//!   [it] can have only one source. If the state is active, [it] can have one or two
//!   sources" (`FailoverConfig$State`, the shape `CreateBridgeRequest$SourceFailoverConfig`
//!   and `UpdateBridgeRequest$SourceFailoverConfig` target).
//!
//! Every request that would leave a bridge outside either rule is refused with the modelled
//! `BadRequestException` (HTTP 400) in this service's validation envelope, and the bridge is
//! left exactly as it was — which DescribeBridge is asked afterwards.
//!
//! The tables below span what the rules generalise over: the bridge TYPE addressed, the
//! KIND of each source a request carries, how many sources the bridge already holds, how
//! many one request adds, and every form a failover config can take (absent, a state the
//! model declares, a config that names no state at all).
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case here drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::FailoverConfig;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{create_bridge, describe_bridge, minted_flow_arn, minted_gateway_arn};
use crate::flow_lifecycle::Emulator;

/// `POST /v1/bridges/{BridgeArn}/sources`.
async fn add_sources(emulator: &Emulator, arn: &str, sources: Vec<Value>) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "POST",
            &format!("/v1/bridges/{arn}/sources"),
            Some(json!({ "sources": sources })),
        )
        .await
}

/// `PUT /v1/bridges/{BridgeArn}`.
async fn update_bridge(emulator: &Emulator, arn: &str, body: Value) -> (u16, Option<String>, Value) {
    emulator
        .send("PUT", &format!("/v1/bridges/{arn}"), Some(body))
        .await
}

/// One flow source: the kind a cloud-to-ground bridge ingests.
fn flow_source(name: &str, suffix: u64) -> Value {
    json!({ "flowSource": {
        "flowArn": minted_flow_arn(suffix, &format!("flow-{suffix}")),
        "name": name,
    }})
}

/// One network source: the kind a ground-to-cloud bridge ingests.
fn network_source(name: &str, port: i64) -> Value {
    json!({ "networkSource": {
        "multicastIp": "224.0.1.1",
        "name": name,
        "networkName": "net-1",
        "port": port,
        "protocol": "rtp",
    }})
}

/// The two kinds of source a `BridgeSource` reports, and the bridge type each one belongs
/// on. `source(index, name)` builds one entry, so a row can ask for several of one kind
/// without ever repeating a name.
struct Kind {
    /// What this kind is called in a case name.
    label: &'static str,
    /// The bridge-type member a bridge that ingests this kind carries.
    admitted_by: &'static str,
    /// One source entry of this kind, named `name`.
    source: fn(&str, u64) -> Value,
}

const FLOW: Kind = Kind {
    label: "flow source",
    admitted_by: "egressGatewayBridge",
    source: flow_source,
};
const NETWORK: Kind = Kind {
    label: "network source",
    admitted_by: "ingressGatewayBridge",
    source: |name, index| network_source(name, 5_000 + index as i64),
};

/// The bridge-type member `member` carries, with its settings. `IngressGatewayBridge`
/// declares `MaxOutputs` as well, which this suite never exercises (that is
/// `crate::bridge_output_admission`'s subject), so the value is only a valid one.
fn bridge_type_settings(member: &str) -> Value {
    if member == "egressGatewayBridge" {
        json!({ "maxBitrate": 20_000_000 })
    } else {
        json!({ "maxBitrate": 20_000_000, "maxOutputs": 2 })
    }
}

/// A CreateBridge body for a bridge of `member` type carrying `sources`, with `failover`
/// when the row names one.
fn create_body(name: &str, member: &str, sources: Vec<Value>, failover: Option<&Value>) -> Value {
    let mut body = json!({
        "name": name,
        "placementArn": minted_gateway_arn(1, "gw-source-admission"),
        "sources": sources,
    });
    body[member] = bridge_type_settings(member);
    if let Some(failover) = failover {
        body["sourceFailoverConfig"] = failover.clone();
    }
    body
}

/// Assert one refusal: the modelled error, this service's validation envelope, and a
/// message that states what was unacceptable.
fn assert_refused(case: &str, status: u16, code: Option<&str>, answered: &Value) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {answered}"
    );
    let message = answered["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX),
        "case {case:?}: the refusal states what was wrong, got {message:?}"
    );
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

/// The source failover forms a request can carry, paired with whether each one leaves
/// failover ACTIVE. `FailoverConfig$State` targets the `State` enum
/// (`smithy_query(query="shape",shape="State")`: ENABLED | DISABLED) and the member is
/// optional, so a config may also leave it unset — which is not the active state either.
fn failover_forms() -> Vec<(&'static str, Option<Value>, bool)> {
    vec![
        ("no sourceFailoverConfig at all", None, false),
        (
            "failover state DISABLED",
            Some(json!({ "state": mc::STATE_DISABLED })),
            false,
        ),
        (
            "a failover config naming a mode but no state",
            Some(json!({ "failoverMode": "MERGE" })),
            false,
        ),
        (
            "failover state ENABLED",
            Some(json!({ "state": mc::STATE_ENABLED })),
            true,
        ),
        (
            "failover state ENABLED with a mode and a recovery window",
            Some(json!({
                "state": mc::STATE_ENABLED,
                "failoverMode": "FAILOVER",
                "recoveryWindow": 3_000,
            })),
            true,
        ),
    ]
}

/// The bound a bridge whose failover is `active` holds its sources under — the crate's two
/// published counts, which the test below pins against the ones `FailoverConfig$State`
/// documents.
fn bound(active: bool) -> usize {
    if active {
        mc::MAX_BRIDGE_SOURCES
    } else {
        mc::MAX_BRIDGE_SOURCES_FAILOVER_INACTIVE
    }
}

/// The counts the crate publishes are the counts the model documents: a single source while
/// source failover is inactive, a pair while it is active.
#[test]
fn the_published_bridge_source_bound_is_the_documented_one() {
    assert_eq!(
        (
            mc::MAX_BRIDGE_SOURCES,
            mc::MAX_BRIDGE_SOURCES_FAILOVER_INACTIVE
        ),
        (2, 1),
        "case \"published bound\": a bridge holds one source while failover is inactive and \
         two while it is active, got {} / {}",
        mc::MAX_BRIDGE_SOURCES,
        mc::MAX_BRIDGE_SOURCES_FAILOVER_INACTIVE
    );
}

/// The bound one function resolves, over every failover form and every count around each
/// form's bound — the rule the create path, the add path and UpdateBridge all read.
#[test]
fn a_bridge_s_source_count_is_bounded_by_its_own_failover_state() {
    for (form, failover, active) in failover_forms() {
        let limit = bound(active);
        let config = failover.as_ref().map(stored_failover_config);
        for (held, adding) in [(0, 1), (0, 2), (0, 3), (1, 1), (1, 2), (2, 1), (2, 0)] {
            let case = format!("{form}, {held} held, {adding} added");
            let refused_expected = held + adding > limit;
            let rejection = mc::bridge_source_bound_rejection(config.as_ref(), held, adding);
            assert_eq!(
                rejection.is_some(),
                refused_expected,
                "case {case:?}: expected refused={refused_expected} under a bound of {limit}, \
                 got {rejection:?}"
            );
        }
    }
}

/// The modelled `FailoverConfig` a request body's `sourceFailoverConfig` describes, so the
/// unit test above reads the bound through exactly the stored shape the handlers pass it.
fn stored_failover_config(body: &Value) -> FailoverConfig {
    FailoverConfig {
        failover_mode: body["failoverMode"].as_str().map(ToString::to_string),
        recovery_window: body["recoveryWindow"]
            .as_i64()
            .and_then(|window| i32::try_from(window).ok()),
        source_priority: None,
        state: body["state"].as_str().map(ToString::to_string),
    }
}

/// CreateBridge stores a bridge whose sources are of the kind its direction admits and
/// whose count is inside the bound its own failover state sets, and refuses anything else
/// with `BadRequestException` (400), storing no bridge.
///
/// The rows cross both kinds of source with both bridge types — so each kind is asserted
/// both on the bridge that admits it and on the one that does not — over the counts around
/// each failover form's bound.
#[tokio::test]
async fn create_bridge_admits_the_sources_the_bridge_s_direction_and_failover_state_allow() {
    let emulator = Emulator::new();

    for (kind_index, kind) in [FLOW, NETWORK].into_iter().enumerate() {
        for member in ["egressGatewayBridge", "ingressGatewayBridge"] {
            let admitted = member == kind.admitted_by;
            for (form_index, (form, failover, active)) in failover_forms().into_iter().enumerate() {
                let limit = bound(active);
                for count in 1..=3usize {
                    let case = format!(
                        "{count} {}(s) on a bridge whose type is {member}, {form}",
                        kind.label
                    );
                    let name =
                        format!("bridge-create-{kind_index}-{member}-{form_index}-{count}");
                    let sources: Vec<Value> = (0..count)
                        .map(|index| {
                            (kind.source)(&format!("src-{index}"), (index + 1) as u64)
                        })
                        .collect();
                    let (status, code, answered) = create_bridge(
                        &emulator,
                        create_body(&name, member, sources.clone(), failover.as_ref()),
                    )
                    .await;

                    if admitted && count <= limit {
                        assert_eq!(
                            (status, code.as_deref()),
                            (201, None),
                            "case {case:?}: a bridge inside both rules is created, got \
                             {status} {code:?}: {answered}"
                        );
                        let arn = answered["bridge"]["bridgeArn"]
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!("case {case:?}: the create answers an ARN: {answered}")
                            })
                            .to_string();
                        let bridge = bridge_of(&emulator, &case, &arn).await;
                        assert_eq!(
                            bridge["sources"],
                            json!(sources),
                            "case {case:?}: the stored bridge holds the sources the create \
                             named, got {bridge}"
                        );
                    } else {
                        assert_refused(&case, status, code.as_deref(), &answered);
                        // A refused create stores nothing: no bridge answers under that name.
                        let (status, _, listed) =
                            emulator.send("GET", "/v1/bridges?maxResults=100", None).await;
                        assert_eq!(status, 200, "case {case:?}: ListBridges must answer 200");
                        let named = listed["bridges"]
                            .as_array()
                            .map(|bridges| {
                                bridges
                                    .iter()
                                    .any(|bridge| bridge["name"].as_str() == Some(name.as_str()))
                            })
                            .unwrap_or_default();
                        assert!(
                            !named,
                            "case {case:?}: a refused create stores no bridge, yet {name:?} is \
                             listed: {listed}"
                        );
                    }
                }
            }
        }
    }
}

/// AddBridgeSources counts and admits against the STORED bridge: a batch is accepted
/// exactly while every entry is of the kind the bridge's direction ingests and the sources
/// it already holds plus the ones the call adds stay inside the bound its failover state
/// sets. Anything else is refused with `BadRequestException` (400) and appends nothing.
#[tokio::test]
async fn add_bridge_sources_admits_the_sources_the_stored_bridge_s_direction_and_bound_allow() {
    let emulator = Emulator::new();

    for (kind_index, kind) in [FLOW, NETWORK].into_iter().enumerate() {
        for member in ["egressGatewayBridge", "ingressGatewayBridge"] {
            let admitted = member == kind.admitted_by;
            // The bridge under test is created with the ONE source its own direction
            // admits, so what the add carries is the only thing each row varies.
            let held_kind = if member == FLOW.admitted_by { FLOW } else { NETWORK };
            for (form_index, (form, failover, active)) in failover_forms().into_iter().enumerate() {
                let limit = bound(active);
                for adding in 1..=2usize {
                    let case = format!(
                        "{adding} {}(s) added to a bridge whose type is {member}, {form}",
                        kind.label
                    );
                    let name = format!("bridge-add-{kind_index}-{member}-{form_index}-{adding}");
                    let (status, code, created) = create_bridge(
                        &emulator,
                        create_body(
                            &name,
                            member,
                            vec![(held_kind.source)("held", 50)],
                            failover.as_ref(),
                        ),
                    )
                    .await;
                    assert_eq!(
                        (status, code.as_deref()),
                        (201, None),
                        "case {case:?}: the bridge under test must be created, got {status} \
                         {code:?}: {created}"
                    );
                    let arn = created["bridge"]["bridgeArn"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: the create answers an ARN: {created}")
                        })
                        .to_string();
                    let before = bridge_of(&emulator, &case, &arn).await;
                    let held = before["sources"].as_array().map(Vec::len).unwrap_or_default();

                    let added: Vec<Value> = (0..adding)
                        .map(|index| {
                            (kind.source)(&format!("added-{index}"), (index + 10) as u64)
                        })
                        .collect();
                    let (status, code, answered) =
                        add_sources(&emulator, &arn, added.clone()).await;
                    let after = bridge_of(&emulator, &case, &arn).await;

                    if admitted && held + adding <= limit {
                        assert_eq!(
                            (status, code.as_deref()),
                            (202, None),
                            "case {case:?}: an add inside both rules is accepted, got {status} \
                             {code:?}: {answered}"
                        );
                        assert_eq!(
                            answered["sources"],
                            json!(added),
                            "case {case:?}: the answer carries the sources added, got {answered}"
                        );
                        let mut expected: Vec<Value> = before["sources"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        expected.extend(added.clone());
                        assert_eq!(
                            after["sources"],
                            json!(expected),
                            "case {case:?}: the bridge holds what it held then what was added, \
                             got {after}"
                        );
                    } else {
                        assert_refused(&case, status, code.as_deref(), &answered);
                        assert_eq!(
                            after["sources"], before["sources"],
                            "case {case:?}: a refused add appends nothing, got {after}"
                        );
                    }
                }
            }
        }
    }
}

/// A batch whose entries are of MIXED kinds is refused whichever bridge it addresses,
/// because one of the two kinds always contradicts the bridge's direction — and the
/// position of the offending entry in the batch does not matter.
#[tokio::test]
async fn a_batch_mixing_the_two_kinds_of_source_is_refused_by_either_bridge() {
    let emulator = Emulator::new();

    for member in ["egressGatewayBridge", "ingressGatewayBridge"] {
        let held_kind = if member == FLOW.admitted_by { FLOW } else { NETWORK };
        for (position, batch) in [
            ("the flow source first", vec![flow_source("a", 1), network_source("b", 5_001)]),
            ("the network source first", vec![network_source("a", 5_002), flow_source("b", 2)]),
        ] {
            let case = format!("a mixed batch on a bridge whose type is {member}, {position}");
            let name = format!("bridge-mixed-{member}-{}", batch.len() + position.len());
            let (status, code, created) = create_bridge(
                &emulator,
                create_body(
                    &name,
                    member,
                    vec![(held_kind.source)("held", 60)],
                    // Failover ACTIVE, so the batch is never refused for the COUNT: what
                    // this row asserts is the kind rule alone.
                    Some(&json!({ "state": mc::STATE_ENABLED })),
                ),
            )
            .await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: the bridge under test must be created, got {status} {code:?}: \
                 {created}"
            );
            let arn = created["bridge"]["bridgeArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
                .to_string();
            let before = bridge_of(&emulator, &case, &arn).await;

            let (status, code, answered) = add_sources(&emulator, &arn, batch).await;
            assert_refused(&case, status, code.as_deref(), &answered);
            let after = bridge_of(&emulator, &case, &arn).await;
            assert_eq!(
                after["sources"], before["sources"],
                "case {case:?}: a refused add appends nothing, got {after}"
            );
        }
    }
}

/// UpdateBridge moves the bound rather than the source count: a request that would leave
/// the bridge's failover state inactive under a pair of sources is refused and writes
/// nothing, and one that turns failover ON widens the bound, so the add refused before it
/// is accepted after.
#[tokio::test]
async fn update_bridge_moves_the_bound_a_bridge_holds_its_sources_under() {
    let emulator = Emulator::new();

    // A pair of flow sources under ACTIVE failover: switching the state off would leave the
    // bridge over the inactive bound.
    let case = "failover switched off under a failover pair";
    let (status, code, created) = create_bridge(
        &emulator,
        create_body(
            "bridge-bound-pair",
            "egressGatewayBridge",
            vec![flow_source("primary", 1), flow_source("backup", 2)],
            Some(&json!({ "state": mc::STATE_ENABLED })),
        ),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the bridge under test must be created, got {status} {code:?}: {created}"
    );
    let pair_arn = created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
        .to_string();
    let stored_config = created["bridge"]["sourceFailoverConfig"].clone();

    for off in [
        json!({ "state": mc::STATE_DISABLED }),
        json!({ "state": mc::STATE_DISABLED, "failoverMode": "FAILOVER" }),
    ] {
        let (status, code, answered) = update_bridge(
            &emulator,
            &pair_arn,
            json!({ "sourceFailoverConfig": off }),
        )
        .await;
        assert_refused(case, status, code.as_deref(), &answered);
        let bridge = bridge_of(&emulator, case, &pair_arn).await;
        assert_eq!(
            bridge["sourceFailoverConfig"], stored_config,
            "case {case:?}: a refused update writes nothing, got {:?}",
            bridge["sourceFailoverConfig"]
        );
    }

    // One network source under INACTIVE failover: a second source is refused, the update
    // that turns failover on is accepted, and the same add is accepted afterwards.
    let case = "failover turned on widens the bound";
    let (status, code, created) = create_bridge(
        &emulator,
        create_body(
            "bridge-bound-single",
            "ingressGatewayBridge",
            vec![network_source("held", 5_000)],
            None,
        ),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the bridge under test must be created, got {status} {code:?}: {created}"
    );
    let arn = created["bridge"]["bridgeArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
        .to_string();
    let second = vec![network_source("second", 5_001)];

    let (status, code, answered) = add_sources(&emulator, &arn, second.clone()).await;
    assert_refused(case, status, code.as_deref(), &answered);

    let (status, code, answered) = update_bridge(
        &emulator,
        &arn,
        json!({ "sourceFailoverConfig": { "state": mc::STATE_ENABLED } }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: turning failover on under one source is accepted, got {status} \
         {code:?}: {answered}"
    );

    let (status, code, answered) = add_sources(&emulator, &arn, second).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: with failover active the pair is accepted, got {status} {code:?}: \
         {answered}"
    );
    let bridge = bridge_of(&emulator, case, &arn).await;
    assert_eq!(
        bridge["sources"].as_array().map(Vec::len),
        Some(mc::MAX_BRIDGE_SOURCES),
        "case {case:?}: the bridge holds the failover pair, got {bridge}"
    );

    // And the pair is still the ceiling: a third source is refused however active the
    // failover state is.
    let (status, code, answered) =
        add_sources(&emulator, &arn, vec![network_source("third", 5_002)]).await;
    assert_refused(case, status, code.as_deref(), &answered);
}
