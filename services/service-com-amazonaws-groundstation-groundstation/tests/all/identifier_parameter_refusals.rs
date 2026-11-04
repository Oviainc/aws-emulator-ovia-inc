// SPDX-License-Identifier: CC0-1.0

//! What AWS Ground Station answers for an identifier that resolves to nothing, and for a
//! registration that supplies neither member of a pair the service requires one of — end
//! to end over HTTP through the same front door as the rest of this target's suites.
//!
//! The RULES under test:
//!
//! * Ground Station resolves the `{agentId}` of the agent plane and the `{satelliteId}` of
//!   the satellite reference read as PARAMETERS of the request, not as resources the
//!   request addresses: an id that resolves to nothing is refused
//!   `InvalidParameterException` (HTTP 431) carrying that family's fixed sentence, which
//!   says nothing of the id it was given, and the store is left as it was. (The other
//!   identifier families of this service — configs, contacts, mission profiles, ephemerides
//!   — answer their own `ResourceNotFoundException` texts; `tests/all/not_found_messages.rs`
//!   owns those.)
//! * where the model makes two members individually optional but the service requires ONE
//!   of the pair — `AgentDetails`' superseded `reservedCpuCores` and its current
//!   `agentCpuCores` (`smithy_query(query="shape", shape="AgentDetails")`) — a request that
//!   supplies neither is refused `InvalidParameterException` (HTTP 431) naming both member
//!   paths, and a request that supplies EITHER is accepted and reads back carrying what it
//!   supplied.
//!
//! Each suite asserts its rule over a TABLE: the agent rule over every operation the model
//! addresses at an `Agent` by that label crossed with ids that resolve to nothing, the
//! pair rule over which of the two members is supplied and at what list size, and the
//! satellite rule over ids that resolve to nothing on an account on-boarded to satellites.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::agent_lifecycle::{
    capability_arn, get_configuration, register, update_status, uuid_from,
};
// The refusal SHAPE — HTTP 431 and the `InvalidParameterException` shape — is asserted
// through the helper the sibling suite that already reads refusal texts owns, so this one
// adds rules rather than a second notion of what a bad-parameter answer is.
use crate::refusal_message_templates::bad_parameter_message;
use crate::s3_recording_buckets::harness_with_object_storage_and_engine;
use crate::satellite_catalog::{in_listing_order, onboard};
use crate::scheduling_loop::{harness_with_engine, is_uuid, send, Reply, ABSENT_UUID};

/// The sentence Ground Station answers for an `{agentId}` that resolves to no registered
/// agent, as recorded against AWS: fixed text in full, with no slot for the id.
const INVALID_AGENT_ID: &str = "Invalid AgentID.";

/// The sentence Ground Station answers for a `{satelliteId}` that resolves to no
/// onboarding of the account, as recorded against AWS: fixed text in full.
const SATELLITE_DOES_NOT_EXIST: &str = "Satellite does not exist.";

/// The sentence Ground Station answers for a registration that supplies neither member of
/// the `AgentDetails` cpu-core pair, as recorded against AWS: fixed text naming both member
/// paths.
const CPU_CORES_UNSPECIFIED: &str =
    "agentDetails.reservedCpuCores or agentDetails.agentCpuCores must be specified";

/// The published bounds of `AgentCpuCoresList` (len 0..256 — `smithy_query(query="shape",
/// shape="AgentCpuCoresList")`), so the accepted rows below span a list at each of them.
const CPU_CORES_MIN: usize = 0;
/// See [`CPU_CORES_MIN`].
const CPU_CORES_MAX: usize = 256;

/// Well-formed `Uuid`s (model pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) that resolve to no
/// resource of a fresh account: a run of zeros, a run of `f`s and a mixed one, so an
/// assertion about the refusal cannot be passing on one id's shape.
const UNRESOLVED_IDS: &[(&str, &str)] = &[
    ("the all-zero id", ABSENT_UUID),
    ("a run of f", "ffffffff-ffff-4fff-bfff-ffffffffffff"),
    ("a mixed id", "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"),
];

/// `id` with its first hex digit cycled: the closest miss of a resolvable id there is, so
/// a refusal cannot be a prefix match.
fn near_miss(id: &str) -> String {
    let mut digits: Vec<char> = id.chars().collect();
    digits[0] = match digits[0] {
        'f' => '0',
        other => char::from_digit(other.to_digit(16).expect("a Uuid starts with a hex digit") + 1, 16)
            .expect("a hex digit"),
    };
    digits.into_iter().collect()
}

/// A `RegisterAgent` body whose `agentDetails` carries `cpu_cores` — the members of the
/// cpu-core pair this row supplies — and the required members of `AgentDetails` and
/// `DiscoveryData` beside them, with an instance, an address and a capability of its own so
/// no two rows register the same agent.
fn registration(row: usize, cpu_cores: &Value) -> Value {
    let mut details = json!({
        "agentVersion": "1.7.0",
        "instanceId": format!("i-{row:010x}"),
        "instanceType": "m5.large",
        "componentVersions": [{"componentType": "Agent", "versions": ["1.7.0"]}],
    });
    for (member, value) in cpu_cores
        .as_object()
        .expect("a row's cpu-core members are an object")
    {
        details[member.as_str()] = value.clone();
    }
    json!({
        "agentDetails": details,
        "discoveryData": {
            "publicIpAddresses": [format!("52.0.{}.{}", row / 256, row % 256)],
            "privateIpAddresses": [format!("10.0.{}.{}", row / 256, row % 256)],
            "capabilityArns": [capability_arn(row)],
        },
    })
}

/// A `taskId` for the `index`th task of a suite.
fn task_id(index: usize) -> String {
    uuid_from(0x7a5c_0000, index)
}

/// A status report an agent could legitimately send: the three required members of
/// `UpdateAgentStatus` (`taskId`, `aggregateStatus`, `componentStatuses`) with one
/// component status carrying its own four required members.
fn status_report(index: usize) -> Value {
    json!({
        "taskId": task_id(index),
        "aggregateStatus": {"status": "ACTIVE"},
        "componentStatuses": [{
            "componentType": "Agent",
            "capabilityArn": capability_arn(index),
            "status": "ACTIVE",
            "dataflowId": uuid_from(0x0d0d_0d0d, index),
        }],
    })
}

/// `GET /agentResponseUrl/{agentId}/{taskId}`.
async fn response_url(router: &MultiServiceRouter, agent_id: &str, task_id: &str) -> Reply {
    send(
        router,
        "GET",
        &format!("/agentResponseUrl/{agent_id}/{task_id}"),
        None,
    )
    .await
}

/// `GET /satellite/{satelliteId}`.
async fn get_satellite(router: &MultiServiceRouter, satellite_id: &str) -> Reply {
    send(router, "GET", &format!("/satellite/{satellite_id}"), None).await
}

#[tokio::test]
async fn every_agent_addressed_operation_refuses_an_unresolved_agent_id_as_a_bad_parameter() {
    // S3 beside Ground Station, so the operation that signs an upload URL can serve the
    // agent that IS registered — the control that makes each refusal below about the id.
    let (router, engine) = harness_with_object_storage_and_engine();

    let registered = register(&router, registration(0, &json!({"agentCpuCores": [0, 1]}))).await;
    assert_eq!(
        registered.status, 200,
        "the control registration must be accepted, got {} {}",
        registered.status, registered.body
    );
    let known = registered.string("agentId");

    // The ids under test: ones that resolve to nothing on any account, and the closest
    // miss of the id this account's own registration minted.
    let mut unresolved: Vec<(String, String)> = UNRESOLVED_IDS
        .iter()
        .map(|(case, id)| ((*case).to_string(), (*id).to_string()))
        .collect();
    unresolved.push((
        "a near miss of the registered agent".to_string(),
        near_miss(&known),
    ));

    for (index, (case, agent_id)) in unresolved.iter().enumerate() {
        assert!(
            is_uuid(agent_id) && agent_id != &known,
            "case {case}: the id under test must be a well-formed Uuid naming no agent, got \
             {agent_id:?}"
        );

        // Every operation the model addresses at an `Agent` by its `{agentId}` label: the
        // read, the upload-URL request and the status report.
        let read = get_configuration(&router, agent_id).await;
        let message = bad_parameter_message(&format!("{case} / GetAgentConfiguration"), &read);
        assert_eq!(
            message, INVALID_AGENT_ID,
            "case {case}: GetAgentConfiguration answers the agent-id sentence, got {message:?}"
        );

        let url = response_url(&router, agent_id, &task_id(index)).await;
        let message = bad_parameter_message(&format!("{case} / GetAgentTaskResponseUrl"), &url);
        assert_eq!(
            message, INVALID_AGENT_ID,
            "case {case}: GetAgentTaskResponseUrl answers the agent-id sentence, got {message:?}"
        );

        let reported = update_status(&router, agent_id, status_report(index)).await;
        let message = bad_parameter_message(&format!("{case} / UpdateAgentStatus"), &reported);
        assert_eq!(
            message, INVALID_AGENT_ID,
            "case {case}: UpdateAgentStatus answers the agent-id sentence, got {message:?}"
        );

        for (operation, refusal) in [
            ("GetAgentConfiguration", &read),
            ("GetAgentTaskResponseUrl", &url),
            ("UpdateAgentStatus", &reported),
        ] {
            assert!(
                !refusal.body.to_string().contains(agent_id.as_str()),
                "case {case}: {operation}'s refusal must not echo the id it was given, got {}",
                refusal.body
            );
        }
    }

    // Nothing the refusals touched: the one registered agent is still there, still holding
    // the status a registration leaves it with, so the refused report wrote nothing.
    {
        let state = service_state(&*engine).expect("this service's state store");
        let guard = state.lock();
        assert_eq!(
            guard.agents.len(),
            1,
            "a refused call registers no agent, got {:?}",
            guard.agents.keys().collect::<Vec<_>>()
        );
        let agent = guard
            .agents
            .get(&known)
            .expect("the registered agent must still be stored");
        assert!(
            agent.task_id.is_none() && agent.component_statuses.is_empty(),
            "a refused report must write nothing, got task {:?} and components {:?}",
            agent.task_id,
            agent.component_statuses
        );
    }

    // And the agent that IS registered is served by all three operations, so the rule
    // above is about the id and not about the operations.
    let read = get_configuration(&router, &known).await;
    assert_eq!(
        read.status, 200,
        "case the-registered-agent: expected GetAgentConfiguration 200, got {} {}",
        read.status, read.body
    );
    let url = response_url(&router, &known, &task_id(0)).await;
    assert_eq!(
        url.status, 200,
        "case the-registered-agent: expected GetAgentTaskResponseUrl 200, got {} {}",
        url.status, url.body
    );
    let reported = update_status(&router, &known, status_report(0)).await;
    assert_eq!(
        reported.status, 200,
        "case the-registered-agent: expected UpdateAgentStatus 200, got {} {}",
        reported.status, reported.body
    );
}

#[tokio::test]
async fn a_registration_must_supply_one_of_the_two_cpu_core_members_and_either_will_do() {
    let (router, engine) = harness_with_engine();

    // The rows differ along which of the pair is supplied and at what list size — the
    // dimensions the rule generalises over — with the published bounds of
    // `AgentCpuCoresList` among them. Only the row that supplies NEITHER member is refused.
    let all_cores: Vec<i64> = (0..CPU_CORES_MAX as i64).collect();
    let rows: Vec<(&str, Value, bool)> = vec![
        ("neither member", json!({}), false),
        (
            "agentCpuCores at the published minimum",
            json!({"agentCpuCores": vec![0_i64; CPU_CORES_MIN]}),
            true,
        ),
        (
            "agentCpuCores with one core",
            json!({"agentCpuCores": [3_i64]}),
            true,
        ),
        (
            "agentCpuCores at the published maximum",
            json!({"agentCpuCores": all_cores}),
            true,
        ),
        (
            "the superseded reservedCpuCores alone",
            json!({"reservedCpuCores": [0_i64, 1]}),
            true,
        ),
        (
            "reservedCpuCores at the published minimum",
            json!({"reservedCpuCores": vec![0_i64; CPU_CORES_MIN]}),
            true,
        ),
        (
            "both members",
            json!({"agentCpuCores": [0_i64, 1], "reservedCpuCores": [2_i64, 3]}),
            true,
        ),
    ];

    let mut minted = 0_usize;
    for (row, (case, cpu_cores, accepted)) in rows.iter().enumerate() {
        // `row + 1`: row 0 of the table is the refused one, and the instance, address and
        // capability a row registers under are its own either way.
        let reply = register(&router, registration(row + 1, cpu_cores)).await;
        if !*accepted {
            let message = bad_parameter_message(case, &reply);
            assert_eq!(
                message, CPU_CORES_UNSPECIFIED,
                "case {case}: the refusal names both members of the pair, got {message:?}"
            );
            let state = service_state(&*engine).expect("this service's state store");
            assert_eq!(
                state.lock().agents.len(),
                minted,
                "case {case}: a refused registration stores no agent, got {:?}",
                state.lock().agents.keys().collect::<Vec<_>>()
            );
            continue;
        }

        assert_eq!(
            reply.status, 200,
            "case {case}: supplying one member of the pair is enough, got {} {}",
            reply.status, reply.body
        );
        let agent_id = reply.string("agentId");
        assert!(
            is_uuid(&agent_id),
            "case {case}: the minted agentId must be a Uuid, got {agent_id:?}"
        );
        minted += 1;

        // The round trip: the agent reads back under the id the registration minted…
        let read = get_configuration(&router, &agent_id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the registered agent must read back, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("agentId"),
            agent_id,
            "case {case}: the read must answer the registered agent's own id, got {}",
            read.body
        );

        // …carrying exactly the cpu-core members the registration supplied, each as sent:
        // whichever member of the pair the row named, the other stays unsupplied.
        let state = service_state(&*engine).expect("this service's state store");
        let guard = state.lock();
        let stored = guard
            .agents
            .get(&agent_id)
            .unwrap_or_else(|| panic!("case {case}: the registered agent must be stored"));
        for member in ["agentCpuCores", "reservedCpuCores"] {
            let supplied: Option<Vec<i64>> = cpu_cores
                .get(member)
                .map(|value| serde_json::from_value(value.clone()).expect("a list of cores"));
            let held = match member {
                "agentCpuCores" => stored.agent_details.agent_cpu_cores.clone(),
                _ => stored.agent_details.reserved_cpu_cores.clone(),
            }
            .map(|cores| cores.into_iter().map(i64::from).collect::<Vec<i64>>());
            assert_eq!(
                held, supplied,
                "case {case}: expected {member} {supplied:?} as supplied, got {held:?}"
            );
        }
    }
}

#[tokio::test]
async fn get_satellite_refuses_a_satellite_id_that_resolves_to_nothing_as_a_bad_parameter() {
    let (router, engine) = harness_with_engine();
    // An account on-boarded to satellites differing in NORAD catalogue number and in how
    // many ground stations they are on-boarded to, so a refusal below is about the id the
    // request carried rather than about an account holding nothing.
    let catalog = onboard(&engine, "0,25994=Ohio 1+Oregon 1,339999");
    let onboarded = in_listing_order(&catalog);
    let held = onboarded
        .first()
        .map(|satellite| satellite.satellite_id.clone())
        .expect("the catalogue onboards a satellite");

    let mut unresolved: Vec<(String, String)> = UNRESOLVED_IDS
        .iter()
        .map(|(case, id)| ((*case).to_string(), (*id).to_string()))
        .collect();
    unresolved.push((
        "a near miss of an on-boarded satellite".to_string(),
        near_miss(&held),
    ));

    for (case, satellite_id) in &unresolved {
        let reply = get_satellite(&router, satellite_id).await;
        let message = bad_parameter_message(case, &reply);
        assert_eq!(
            message, SATELLITE_DOES_NOT_EXIST,
            "case {case}: GetSatellite answers the satellite sentence, got {message:?}"
        );
        assert!(
            !reply.body.to_string().contains(satellite_id.as_str()),
            "case {case}: the refusal must not echo the id it was given, got {}",
            reply.body
        );
        for satellite in &onboarded {
            assert!(
                !message.contains(&satellite.satellite_id),
                "case {case}: the sentence discloses no satellite this account holds, got \
                 {message:?}"
            );
        }
    }

    // The refusals disturbed nothing: every on-boarded satellite still reads back.
    for satellite in &onboarded {
        let case = format!("after-refusals norad-{}", satellite.norad_satellite_id);
        let reply = get_satellite(&router, &satellite.satellite_id).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: an on-boarded satellite must still read back, got {} {}",
            reply.status, reply.body
        );
    }
}
