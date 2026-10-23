// SPDX-License-Identifier: CC0-1.0

//! The lifecycle rules a delete obeys, end to end over HTTP.
//!
//! Two rules, each asserted over a SET of inputs rather than one recorded call:
//!
//! 1. An operation whose documentation states a precondition on the lifecycle state of the
//!    resource it addresses refuses every request made while the resource is not in that
//!    state, with the modelled semantic-error exception (`BadRequestException`, HTTP 400),
//!    and leaves the resource exactly as it was. DeleteFlow documents it — "Deletes a
//!    flow. Before you can delete a flow, you must stop the flow"
//!    (`smithy_query(query="doc",shape="DeleteFlow")`) — so the required state is the
//!    stopped one, `STANDBY`, and the rows below are every OTHER value of the modelled
//!    `Status` enum (`smithy_query(query="shape",shape="Status")`: STANDBY | ACTIVE |
//!    UPDATING | DELETING | STARTING | STOPPING | ERROR).
//!
//! 2. A delete whose resource's state enum carries a TERMINAL deleted value, and that the
//!    model marks `@idempotent`, is a soft delete: the record moves into the deleting
//!    transient state, the clock settles it into the terminal one, the record stays
//!    readable (and listed) in that state, and repeating the delete succeeds again.
//!    `GatewayState` (`smithy_query(query="shape",shape="GatewayState")`: CREATING | ACTIVE
//!    | UPDATING | ERROR | DELETING | DELETED) carries that value and DeleteGateway is
//!    `@idempotent` (`smithy_query(query="operation",operation="DeleteGateway")`), so the
//!    rows below vary where in its lifecycle the gateway is deleted, how often the delete
//!    is repeated, and which of two same-named gateways is addressed. A `Status` with no
//!    terminal deleted value (a flow) keeps the hard removal, which rule 1's positive rows
//!    assert.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the request path a MediaConnect client reaches.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::GatewayNetwork;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};

/// Longer than [`mc::TRANSITION_SPAN`], so a transition the clock owes is certainly due.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// Every value of the modelled `Status` enum EXCEPT the one DeleteFlow's documented
/// precondition requires (`STANDBY`): the rows of rule 1's refusal table.
///
/// Source: `smithy_query(query="shape",shape="Status")` — STANDBY | ACTIVE | UPDATING |
/// DELETING | STARTING | STOPPING | ERROR. `UPDATING`, `DELETING` and `ERROR` are statuses
/// no implemented operation moves a flow into, so their rows are set up in the store; the
/// rest are reached over the wire by the suite below as well.
const STATUSES_A_DELETE_IS_REFUSED_IN: [&str; 6] = [
    "ACTIVE",
    "STARTING",
    "STOPPING",
    "UPDATING",
    "DELETING",
    "ERROR",
];

async fn delete_flow(emulator: &Emulator, flow_arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}"), None)
        .await
}

async fn create_gateway(emulator: &Emulator, body: Value) -> (u16, Option<String>, Value) {
    emulator.send("POST", "/v1/gateways", Some(body)).await
}

async fn describe_gateway(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/gateways/{arn}"), None)
        .await
}

async fn delete_gateway(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/gateways/{arn}"), None)
        .await
}

/// A gateway request body: one network, one egress block, the name given.
fn gateway_body(name: &str) -> Value {
    json!({
        "name": name,
        "egressCidrBlocks": ["10.0.0.0/16"],
        "networks": [{ "name": "net-1", "cidrBlock": "10.0.0.0/24" }]
    })
}

/// Create one gateway over the wire and answer its ARN.
async fn created_gateway_arn(emulator: &Emulator, name: &str) -> String {
    let (status, code, created) = create_gateway(emulator, gateway_body(name)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the fixture gateway {name:?} must be created, got {status} {code:?}: {created}"
    );
    created["gateway"]["gatewayArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create answers an ARN: {created}"))
        .to_string()
}

/// Create one flow over the wire and answer its ARN. `extra` adds members to the request,
/// so a row can vary what the flow HOLDS while the rule under test is about its status.
async fn created_flow_arn(emulator: &Emulator, name: &str, extra: Value) -> String {
    let mut body = json!({
        "name": name,
        "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    let (status, code, created) = emulator.create_flow(body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the fixture flow {name:?} must be created, got {status} {code:?}: {created}"
    );
    created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create answers an ARN: {created}"))
        .to_string()
}

/// Put one flow into the store in `status`, the way CreateFlow stores one, and answer its
/// ARN.
///
/// The statuses an implemented operation never moves a flow into (`UPDATING`, `DELETING`,
/// `ERROR`) are still values of the modelled `Status` enum a stored flow can carry, so the
/// refusal table covers them from the store rather than leaving a third of the enum
/// unasserted.
fn seed_flow(emulator: &Emulator, name: &str, status: &str) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let suffix = store.next_suffix();
    let flow_arn = engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::flow_resource_fragment(suffix, name),
    );
    store.flows.insert(
        flow_arn.clone(),
        mc::StoredFlow {
            flow_arn: flow_arn.clone(),
            name: name.to_string(),
            description: mc::derived_flow_description(name, "src", None),
            availability_zone: format!("{REGION}a"),
            status: status.to_string(),
            pending: None,
            egress_ip: Some(mc::assigned_egress_ip(suffix)),
            flow_size: None,
            maintenance: None,
            source_failover_config: None,
            source_monitoring_config: None,
            ndi_config: None,
            encoding_config: None,
            sources: vec![mc::StoredSource {
                name: "src".to_string(),
                ingest_ip: Some(mc::assigned_ingest_ip(suffix)),
                ingest_port: Some(mc::assigned_ingest_port(suffix)),
                ..mc::StoredSource::default()
            }],
            outputs: Vec::new(),
            entitlements: Vec::new(),
            media_streams: Vec::new(),
            vpc_interfaces: Vec::new(),
            tags: BTreeMap::new(),
            created_at: SystemTime::now(),
        },
    );
    flow_arn
}

/// Assert one refused delete: the modelled status and code, the service's state-refusal
/// envelope, and that the flow is INTACT afterwards — same status, still describable.
async fn assert_delete_is_refused(emulator: &Emulator, case: &str, flow_arn: &str, status: &str) {
    let (http_status, code, refused) = delete_flow(emulator, flow_arn).await;
    assert_eq!(
        (http_status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: a flow in {status} must be refused with 400 BadRequestException, got \
         {http_status} {code:?}: {refused}"
    );
    let message = refused["message"].as_str().unwrap_or_default();
    let Some(detail) = message.strip_prefix(mc::BAD_REQUEST_PREFIX) else {
        panic!(
            "case {case:?}: the refusal opens with {:?}, got {message:?}",
            mc::BAD_REQUEST_PREFIX
        );
    };
    assert!(
        detail.contains(mc::STATUS_STANDBY),
        "case {case:?}: the refusal names the state the precondition requires, got {detail:?}"
    );

    let (http_status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (
            http_status,
            code.as_deref(),
            described["flow"]["status"].as_str()
        ),
        (200, None, Some(status)),
        "case {case:?}: a refused delete leaves the flow intact, got {http_status} {code:?}: \
         {described}"
    );
    let (_, _, list) = emulator.list_flows("?maxResults=1000").await;
    assert!(
        list["flows"]
            .as_array()
            .is_some_and(|flows| flows.iter().any(|flow| flow["flowArn"].as_str()
                == Some(flow_arn))),
        "case {case:?}: a refused delete leaves the flow listed, got {list}"
    );
}

/// Rule 1, the refusals: DeleteFlow refuses a flow in EVERY modelled `Status` other than
/// `STANDBY` with `BadRequestException` (400) and leaves it intact. Each row reaches its
/// status the way a client would where an implemented operation moves a flow into it, and
/// from the store for the statuses no implemented operation reaches.
#[tokio::test]
async fn delete_flow_refuses_every_status_but_standby() {
    let emulator = Emulator::new();

    // The statuses a client drives a flow through, each observed where the rule applies:
    // STARTING (the transient StartFlow enters), ACTIVE (what the clock settles it into),
    // STOPPING (the transient StopFlow enters).
    let wire_flow = created_flow_arn(&emulator, "gate-wire", json!({})).await;
    let (status, _, started) = emulator
        .send("POST", &format!("/v1/flows/start/{wire_flow}"), None)
        .await;
    assert_eq!(status, 202, "start expected 202, got {status}: {started}");
    assert_delete_is_refused(&emulator, "straight after StartFlow", &wire_flow, "STARTING").await;

    tokio::time::sleep(PAST_THE_TRANSITION).await;
    assert_delete_is_refused(&emulator, "a running flow", &wire_flow, "ACTIVE").await;

    let (status, _, stopped) = emulator
        .send("POST", &format!("/v1/flows/stop/{wire_flow}"), None)
        .await;
    assert_eq!(status, 202, "stop expected 202, got {status}: {stopped}");
    assert_delete_is_refused(&emulator, "straight after StopFlow", &wire_flow, "STOPPING").await;

    // Every modelled status but STANDBY, including the ones only the store reaches, and on
    // flows that differ in what they hold — the gate is about the status, not the content.
    for status in STATUSES_A_DELETE_IS_REFUSED_IN {
        let flow_arn = seed_flow(&emulator, &format!("gate-seeded-{status}"), status);
        assert_delete_is_refused(&emulator, status, &flow_arn, status).await;
    }
}

/// Rule 1, the positive rows: a flow in `STANDBY` is deleted — 202 with `DELETING`, and
/// afterwards the record is gone (`Status` carries no terminal `DELETED` value, so a
/// deleted flow is one the store no longer holds). The rows differ in how the flow reached
/// `STANDBY` and in what it holds.
#[tokio::test]
async fn delete_flow_accepts_a_stopped_flow() {
    let emulator = Emulator::new();

    // A flow that has never been started, one stopped again after a start, and one
    // carrying outputs, entitlements and tags: the delete is about the status alone.
    let fresh = created_flow_arn(&emulator, "standby-fresh", json!({})).await;
    let loaded = created_flow_arn(
        &emulator,
        "standby-loaded",
        json!({
            "outputs": [
                { "name": "out-1", "protocol": "rtp", "destination": "203.0.113.5", "port": 5000 }
            ],
            "entitlements": [{ "name": "ent-1", "subscribers": ["444455556666"] }],
            "flowTags": { "team": "video" }
        }),
    )
    .await;
    let stopped = created_flow_arn(&emulator, "standby-stopped", json!({})).await;
    for method in ["start", "stop"] {
        let (status, _, body) = emulator
            .send("POST", &format!("/v1/flows/{method}/{stopped}"), None)
            .await;
        assert_eq!(status, 202, "{method} expected 202, got {status}: {body}");
        tokio::time::sleep(PAST_THE_TRANSITION).await;
    }
    // The stop settled while nobody was reading: the gate must see the status the CLOCK
    // already owed, not the transient one the store still carried.
    let seeded = seed_flow(&emulator, "standby-seeded", mc::STATUS_STANDBY);

    for (case, flow_arn) in [
        ("never started", fresh),
        ("outputs, entitlements and tags", loaded),
        ("stopped after a start", stopped),
        ("stored in STANDBY", seeded),
    ] {
        let (status, code, deleted) = delete_flow(&emulator, &flow_arn).await;
        assert_eq!(
            (
                status,
                code.as_deref(),
                deleted["flowArn"].as_str(),
                deleted["status"].as_str()
            ),
            (202, None, Some(flow_arn.as_str()), Some("DELETING")),
            "case {case:?}: deleting a STANDBY flow expected 202 DELETING, got {status} \
             {code:?}: {deleted}"
        );

        let (status, code, body) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: a deleted flow is gone, got {status} {code:?}: {body}"
        );
        let (_, _, list) = emulator.list_flows("?maxResults=1000").await;
        assert!(
            list["flows"]
                .as_array()
                .is_some_and(|flows| flows.iter().all(|flow| flow["flowArn"].as_str()
                    != Some(flow_arn.as_str()))),
            "case {case:?}: a deleted flow is not listed, got {list}"
        );
    }
}

/// Rule 2: DeleteGateway is a SOFT delete. Each row deletes a gateway at a different point
/// in its lifecycle and repeats the delete a different number of times; every row must
/// answer 200, leave the record readable in `DELETING` and then in the terminal `DELETED`,
/// and never answer `NotFoundException`.
#[tokio::test]
async fn delete_gateway_retires_the_gateway_and_is_idempotent() {
    struct Case {
        case: &'static str,
        /// Whether to wait out the create transition before deleting, so the row deletes a
        /// gateway in `CREATING` or in `ACTIVE`.
        settle_before_delete: bool,
        /// How many times DeleteGateway is called — one call, or a retry an SDK would make
        /// of an `@idempotent` operation.
        deletes: usize,
    }

    let cases = [
        Case {
            case: "delete a gateway that is still CREATING",
            settle_before_delete: false,
            deletes: 1,
        },
        Case {
            case: "delete a settled ACTIVE gateway",
            settle_before_delete: true,
            deletes: 1,
        },
        Case {
            case: "delete twice, the second while the first is still DELETING",
            settle_before_delete: true,
            deletes: 2,
        },
        Case {
            case: "delete four times",
            settle_before_delete: false,
            deletes: 4,
        },
    ];

    let emulator = Emulator::new();
    for (index, case) in cases.iter().enumerate() {
        let name = format!("gw-retire-{index}");
        let gateway_arn = created_gateway_arn(&emulator, &name).await;
        if case.settle_before_delete {
            tokio::time::sleep(PAST_THE_TRANSITION).await;
        }

        for attempt in 0..case.deletes {
            let (status, code, deleted) = delete_gateway(&emulator, &gateway_arn).await;
            assert_eq!(
                (status, code.as_deref(), deleted["gatewayArn"].as_str()),
                (200, None, Some(gateway_arn.as_str())),
                "case {:?} attempt {attempt}: expected 200 with the gateway's ARN, got {status} \
                 {code:?}: {deleted}",
                case.case
            );

            // Readable straight after every delete, never a 404, and never back out of the
            // terminal state once it is reached.
            let (status, code, described) = describe_gateway(&emulator, &gateway_arn).await;
            let state = described["gateway"]["gatewayState"].as_str();
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {:?} attempt {attempt}: a retired gateway stays readable, got {status} \
                 {code:?}: {described}",
                case.case
            );
            assert!(
                state == Some(mc::GATEWAY_STATE_DELETING)
                    || state == Some(mc::GATEWAY_STATE_DELETED),
                "case {:?} attempt {attempt}: a retired gateway reports DELETING or DELETED, \
                 got {state:?}",
                case.case
            );
            // The whole record is projected, not a husk: the delete retires the gateway,
            // it does not empty it.
            assert_eq!(
                (
                    described["gateway"]["name"].as_str(),
                    &described["gateway"]["networks"],
                    &described["gateway"]["egressCidrBlocks"]
                ),
                (
                    Some(name.as_str()),
                    &gateway_body(&name)["networks"],
                    &gateway_body(&name)["egressCidrBlocks"]
                ),
                "case {:?} attempt {attempt}: the retired record is reported whole, got \
                 {described}",
                case.case
            );
        }

        // The clock settles the deletion into the TERMINAL state, and it stays there.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        for read in 0..2 {
            let (status, code, described) = describe_gateway(&emulator, &gateway_arn).await;
            assert_eq!(
                (
                    status,
                    code.as_deref(),
                    described["gateway"]["gatewayState"].as_str()
                ),
                (200, None, Some(mc::GATEWAY_STATE_DELETED)),
                "case {:?} read {read}: the clock settles the gateway into DELETED and it stays \
                 there, got {status} {code:?}: {described}",
                case.case
            );
        }

        // And a delete after the terminal state is still a success, with the same ARN.
        let (status, code, deleted) = delete_gateway(&emulator, &gateway_arn).await;
        assert_eq!(
            (status, code.as_deref(), deleted["gatewayArn"].as_str()),
            (200, None, Some(gateway_arn.as_str())),
            "case {:?}: deleting an already DELETED gateway succeeds, got {status} {code:?}: \
             {deleted}",
            case.case
        );
    }
}

/// Rule 2, the records a delete must NOT touch: deleting one gateway leaves every other
/// stored gateway alone — including one that shares its name, which only the gateway id in
/// the ARN tells apart — and a delete that resolves no gateway at all is still a
/// `NotFoundException` (404), because idempotence is about repeating a delete of a resource
/// that EXISTS.
#[tokio::test]
async fn deleting_one_gateway_leaves_its_siblings_untouched() {
    let emulator = Emulator::new();
    let shared_name = "gw-sibling";
    let first = created_gateway_arn(&emulator, shared_name).await;
    let second = created_gateway_arn(&emulator, shared_name).await;
    let other = created_gateway_arn(&emulator, "gw-unrelated").await;
    assert_ne!(
        first, second,
        "two gateways sharing a name get their own ARNs, got {first} and {second}"
    );
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let (status, code, deleted) = delete_gateway(&emulator, &first).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "deleting the first of two same-named gateways expected 200, got {status} {code:?}: \
         {deleted}"
    );
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for (case, arn) in [("same name", &second), ("unrelated", &other)] {
        let (status, code, described) = describe_gateway(&emulator, arn).await;
        assert_eq!(
            (
                status,
                code.as_deref(),
                described["gateway"]["gatewayArn"].as_str(),
                described["gateway"]["gatewayState"].as_str()
            ),
            (200, None, Some(arn.as_str()), Some(mc::GATEWAY_STATE_ACTIVE)),
            "case {case:?}: a gateway another delete named must be untouched, got {status} \
             {code:?}: {described}"
        );
    }
    let (status, code, described) = describe_gateway(&emulator, &first).await;
    assert_eq!(
        (
            status,
            code.as_deref(),
            described["gateway"]["gatewayState"].as_str()
        ),
        (200, None, Some(mc::GATEWAY_STATE_DELETED)),
        "the deleted gateway is the one that reports DELETED, got {status} {code:?}: {described}"
    );

    // A delete that resolves nothing: an ARN of the form this service mints, naming no
    // gateway, and an ARN naming a gateway whose name differs from a stored one.
    for (case, arn) in [
        (
            "an ARN this service never minted",
            format!("arn:aws:mediaconnect:{REGION}:000000000000:gateway:1-0000dead:never-made"),
        ),
        (
            "a minted ARN with another gateway's name",
            first.replace(shared_name, "gw-not-stored"),
        ),
    ] {
        let (status, code, refused) = delete_gateway(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: a delete that resolves no gateway is a 404, got {status} {code:?}: \
             {refused}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(mc::not_found_message(&arn).as_str()),
            "case {case:?}: the not-found names the identifier the request carried, got {refused}"
        );
    }
}

/// Rule 2 and the store's own bound: a retired gateway no longer occupies one of the
/// account's gateway slots, and the retired records the store keeps are bounded, so a
/// create-then-delete loop neither exhausts the ceiling nor grows the map without limit.
#[tokio::test]
async fn retired_gateways_free_their_slot_and_stay_bounded() {
    let emulator = Emulator::new();
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();

    // More creates than a single slot: each one is deleted before the next, so the live
    // count returns to zero every round while the retired records accumulate.
    let rounds = 4;
    let mut retired: Vec<String> = Vec::new();
    for round in 0..rounds {
        let gateway_arn = created_gateway_arn(&emulator, &format!("gw-loop-{round}")).await;
        let (status, _, deleted) = delete_gateway(&emulator, &gateway_arn).await;
        assert_eq!(
            status, 200,
            "round {round}: delete expected 200, got {status}: {deleted}"
        );
        retired.push(gateway_arn);

        let live = state.lock().live_gateway_count();
        assert_eq!(
            live, 0,
            "round {round}: a retired gateway holds no live slot, got {live} live gateways"
        );
    }

    // Every retired record is still readable: the bound is far above these rounds.
    for (round, gateway_arn) in retired.iter().enumerate() {
        let (status, code, described) = describe_gateway(&emulator, gateway_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "round {round}: a retired gateway is still readable, got {status} {code:?}: \
             {described}"
        );
    }

    // Past the retention bound, the OLDEST retired records are dropped and the newest are
    // kept — the map cannot grow without limit. The records are set up directly, because
    // the bound is about what is STORED; the delete that crosses it goes over the wire.
    {
        let mut store = state.lock();
        for index in 0..mc::MAX_RETIRED_GATEWAYS {
            let suffix = store.next_suffix();
            let name = format!("gw-retired-{suffix}");
            let gateway_arn = engine::emulator_arn(
                mc::SERVICE_NAMESPACE,
                REGION,
                &mc::gateway_resource_fragment(suffix, &name),
            );
            store.gateways.insert(
                gateway_arn.clone(),
                mc::StoredGateway {
                    gateway_arn,
                    name,
                    egress_cidr_blocks: vec!["10.0.0.0/16".to_string()],
                    networks: vec![GatewayNetwork {
                        name: "net-1".to_string(),
                        cidr_block: "10.0.0.0/24".to_string(),
                    }],
                    gateway_state: mc::GATEWAY_STATE_DELETED.to_string(),
                    pending: None,
                    // Retired further and further in the past, so the eviction order is
                    // determined rather than a tie.
                    deleted_at: Some(
                        SystemTime::now() - Duration::from_secs(3_600 + index as u64),
                    ),
                },
            );
        }
    }
    let newest = created_gateway_arn(&emulator, "gw-newest").await;
    let (status, _, deleted) = delete_gateway(&emulator, &newest).await;
    assert_eq!(status, 200, "delete expected 200, got {status}: {deleted}");

    let (stored, newest_is_held) = {
        let store = state.lock();
        (store.gateways.len(), store.gateways.contains_key(&newest))
    };
    assert!(
        stored <= mc::MAX_RETIRED_GATEWAYS,
        "the retired records are bounded by {}, got {stored} stored gateways",
        mc::MAX_RETIRED_GATEWAYS
    );
    assert!(
        newest_is_held,
        "the gateway retired last is the one kept, got {stored} stored gateways"
    );
    let (status, code, described) = describe_gateway(&emulator, &newest).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "a client polling the delete it just made finds its record, got {status} {code:?}: \
         {described}"
    );
}
