// SPDX-License-Identifier: CC0-1.0

//! What an AWS Ground Station Agent endpoint reports once an agent carries it, end to end
//! over HTTP through the same front door as the rest of this target's suites (the harness,
//! signed `restJson1` requests and the response reader come from
//! [`crate::scheduling_loop`]).
//!
//! An agent endpoint's stream is carried by an agent the customer registers with
//! `RegisterAgent` for the capabilities it manages (`smithy_query(query="shape",
//! shape="DiscoveryData")`), so a read of the group reports THAT agent: its `agentStatus`
//! is the status the agent last reported of itself through `UpdateAgentStatus`, and the
//! endpoint's health and audit result are what that status implies. Each suite asserts
//! that RULE over a table of inputs rather than one recorded call:
//!
//! * every `AgentStatus` an agent can report, and the agent that has reported none yet,
//!   against each of the three agent members `EndpointDetails` declares, with the group's
//!   ARN at a different position of a different-sized `capabilityArns` list on each row;
//! * a group no registered agent named keeps waiting for one, whatever other capabilities
//!   agents of the account registered for, and a read of either group leaves what the next
//!   read sees unchanged;
//! * a capability two agents name is carried by the LATEST registration, and the endpoint
//!   follows that agent's reports afterwards;
//! * the endpoint a `dataflowEndpointConfig` names by `dataflowEndpointName` reports the
//!   same thing the group read does, both reads going through one reader.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    service_state, AGENT_ACTIVE, AGENT_FAILED, AGENT_INACTIVE, AGENT_SUCCESS, AUDIT_HEALTHY,
    AUDIT_UNHEALTHY, HEALTH_HEALTHY, HEALTH_UNHEALTHY, REASON_DATAPLANE_FAILURE, REASON_HEALTHY,
    REASON_INITIALIZING_DATAPLANE, REASON_NO_REGISTERED_AGENT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::agent_lifecycle::{register, update_status, uuid_from};
use crate::dataflow_endpoint_groups::{create_group, details, endpoint_kinds, expected_arn};
use crate::scheduling_loop::{harness_with_engine, Reply, REGION};

/// One row of the carried-endpoint table: what the agent reports of itself, and what the
/// endpoint it carries must then report.
struct Carried {
    /// Names the row in every assertion.
    label: &'static str,
    /// The `AgentStatus` the agent reports through `UpdateAgentStatus`; `None` for an agent
    /// that has registered and reported nothing yet.
    reported: Option<&'static str>,
    /// The `agentStatus` the endpoint must report.
    agent_status: &'static str,
    /// The `healthStatus` and the one `healthReasons` entry the endpoint must report.
    health: (&'static str, &'static str),
    /// The `auditResults` the endpoint must report, `None` when it must report none.
    audit: Option<&'static str>,
}

/// The carried-endpoint table: every value `AgentStatus` declares
/// (`smithy_query(query="shape", shape="AgentStatus")` — `SUCCESS`, `FAILED`, `ACTIVE`,
/// `INACTIVE`) as the status an agent reports of itself, plus the agent that has reported
/// nothing yet.
fn carried() -> Vec<Carried> {
    vec![
        Carried {
            label: "an agent that has registered and reported nothing",
            reported: None,
            agent_status: AGENT_INACTIVE,
            health: (HEALTH_UNHEALTHY, REASON_INITIALIZING_DATAPLANE),
            audit: None,
        },
        Carried {
            label: "an agent reporting ACTIVE",
            reported: Some(AGENT_ACTIVE),
            agent_status: AGENT_ACTIVE,
            health: (HEALTH_HEALTHY, REASON_HEALTHY),
            audit: Some(AUDIT_HEALTHY),
        },
        Carried {
            label: "an agent reporting SUCCESS",
            reported: Some(AGENT_SUCCESS),
            agent_status: AGENT_SUCCESS,
            health: (HEALTH_HEALTHY, REASON_HEALTHY),
            audit: Some(AUDIT_HEALTHY),
        },
        Carried {
            label: "an agent reporting FAILED",
            reported: Some(AGENT_FAILED),
            agent_status: AGENT_FAILED,
            health: (HEALTH_UNHEALTHY, REASON_DATAPLANE_FAILURE),
            audit: Some(AUDIT_UNHEALTHY),
        },
        Carried {
            label: "an agent reporting INACTIVE",
            reported: Some(AGENT_INACTIVE),
            agent_status: AGENT_INACTIVE,
            health: (HEALTH_UNHEALTHY, REASON_INITIALIZING_DATAPLANE),
            audit: None,
        },
    ]
}

/// A `CapabilityArn` of this account that is NOT a dataflow endpoint group of it, so no
/// group's endpoints are carried by an agent that names only these.
///
/// `CapabilityArn` is an `arnReference` to a capability of the Ground Station PROVIDER
/// (`smithy_query(query="shape", shape="CapabilityArn")`), a resource no operation of this
/// model creates, so the value is only required to be a string naming one.
fn other_capability(index: usize) -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:capability/{index}-antenna",
        engine::emulator_account_id()
    )
}

/// The `RegisterAgent` body of the `row`th agent of a suite, registered for
/// `capability_arns`, with the cpu-core member the service requires one of.
fn registration(row: usize, capability_arns: Vec<String>) -> Value {
    json!({
        "agentDetails": {
            "agentVersion": "1.4.2",
            "instanceId": format!("i-{row:010x}"),
            "instanceType": "m5.large",
            "agentCpuCores": [0, 1],
            "componentVersions": [{"componentType": "Agent", "versions": ["1.4.2"]}],
        },
        "discoveryData": {
            "publicIpAddresses": ["52.0.0.1"],
            "privateIpAddresses": ["10.0.0.1"],
            "capabilityArns": capability_arns,
        },
    })
}

/// Register an agent for `capability_arns` and answer its minted `agentId`.
async fn register_for(
    router: &MultiServiceRouter,
    row: usize,
    capability_arns: Vec<String>,
    case: &str,
) -> String {
    let reply = register(router, registration(row, capability_arns)).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected RegisterAgent 200, got {} {}",
        reply.status, reply.body
    );
    reply.string("agentId")
}

/// Report `status` as `agent_id`'s own status.
async fn report(router: &MultiServiceRouter, row: usize, agent_id: &str, status: &str, case: &str) {
    let reply = update_status(
        router,
        agent_id,
        json!({
            "taskId": uuid_from(0x7a5c_0001, row),
            "aggregateStatus": {"status": status},
            "componentStatuses": [],
        }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected UpdateAgentStatus 200, got {} {}",
        reply.status, reply.body
    );
}

/// Create a group holding one endpoint per `(member, body)` and answer its minted id.
async fn group_with(router: &MultiServiceRouter, endpoints: Vec<Value>, case: &str) -> String {
    let reply = create_group(router, json!({"endpointDetails": endpoints})).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected CreateDataflowEndpointGroup 200, got {} {}",
        reply.status, reply.body
    );
    reply.string("dataflowEndpointGroupId")
}

/// `GET /dataflowEndpointGroup/{id}`, as a reply and as the endpoint list it reports.
async fn read_group(router: &MultiServiceRouter, id: &str, case: &str) -> (Reply, Vec<Value>) {
    let reply = crate::dataflow_endpoint_groups::get_group(router, id).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected GetDataflowEndpointGroup 200, got {} {}",
        reply.status, reply.body
    );
    let endpoints = reply
        .field("endpointsDetails")
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: the read reports no endpointsDetails: {}", reply.body)
        })
        .clone();
    (reply, endpoints)
}

/// Assert the endpoint `member` describes reports `agent_status`, `health` and `audit`.
fn assert_reports(
    endpoint: &Value,
    member: &str,
    agent_status: &str,
    health: (&str, &str),
    audit: Option<&str>,
    case: &str,
) {
    assert_eq!(
        endpoint
            .pointer(&format!("/{member}/agentStatus"))
            .and_then(Value::as_str),
        Some(agent_status),
        "case {case}: expected agentStatus {agent_status}, got {endpoint}"
    );
    assert_eq!(
        endpoint
            .pointer(&format!("/{member}/auditResults"))
            .and_then(Value::as_str),
        audit,
        "case {case}: expected auditResults {audit:?}, got {endpoint}"
    );
    assert_eq!(
        endpoint.get("healthStatus").and_then(Value::as_str),
        Some(health.0),
        "case {case}: expected healthStatus {}, got {endpoint}",
        health.0
    );
    assert_eq!(
        endpoint.get("healthReasons"),
        Some(&json!([health.1])),
        "case {case}: expected healthReasons [{}], got {endpoint}",
        health.1
    );
}

/// The three agent members of `EndpointDetails`, each with `name` set to `name`.
///
/// Source: `smithy_query(query="shape", shape="EndpointDetails")` —
/// `awsGroundStationAgentEndpoint`, `uplinkAwsGroundStationAgentEndpoint` and
/// `downlinkAwsGroundStationAgentEndpoint` are the members an AGENT carries, taken from
/// this target's one endpoint-kind table.
fn agent_members(name: &str) -> Vec<(&'static str, Value)> {
    endpoint_kinds()
        .into_iter()
        .filter(|kind| kind.member != "endpoint")
        .map(|kind| {
            let mut body = kind.body.clone();
            body["name"] = json!(format!("{name}-{}", kind.member));
            (kind.member, body)
        })
        .collect()
}

#[tokio::test]
async fn an_agent_endpoint_reports_the_status_of_the_agent_registered_for_its_group() {
    let (router, _engine) = harness_with_engine();

    for (row, case) in carried().into_iter().enumerate() {
        let Carried {
            label,
            reported,
            agent_status,
            health,
            audit,
        } = case;

        // One group holding ALL THREE agent members at once, so the rule is asserted for
        // every member of a group a single agent carries rather than one member per group.
        let members = agent_members(&format!("row-{row}"));
        let group_id = group_with(
            &router,
            members
                .iter()
                .map(|(member, body)| details(member, body, false))
                .collect(),
            label,
        )
        .await;

        // Before any agent registers for this group's ARN, the endpoints are waiting for
        // one — the state the create wrote down.
        let (_, endpoints) = read_group(&router, &group_id, label).await;
        for ((member, _), endpoint) in members.iter().zip(endpoints.iter()) {
            assert_reports(
                endpoint,
                member,
                AGENT_INACTIVE,
                (HEALTH_UNHEALTHY, REASON_NO_REGISTERED_AGENT),
                None,
                &format!("{label}, before any agent registered, {member}"),
            );
        }

        // The group's ARN sits at a different position of a different-sized capability list
        // on every row: what carries the endpoints is that the registration NAMED it, not
        // where in the list it was named.
        let mut capability_arns: Vec<String> = (0..row).map(other_capability).collect();
        capability_arns.insert(row / 2, expected_arn(&group_id));
        let agent_id = register_for(&router, row, capability_arns, label).await;
        if let Some(status) = reported {
            report(&router, row, &agent_id, status, label).await;
        }

        let (_, endpoints) = read_group(&router, &group_id, label).await;
        for ((member, _), endpoint) in members.iter().zip(endpoints.iter()) {
            assert_reports(
                endpoint,
                member,
                agent_status,
                health,
                audit,
                &format!("{label}, {member}"),
            );
        }
    }
}

#[tokio::test]
async fn a_group_no_registered_agent_named_keeps_waiting_and_a_read_changes_nothing() {
    let (router, _engine) = harness_with_engine();
    let members = agent_members("waiting");
    let (carried_member, carried_body) = members[0].clone();
    let (waiting_member, waiting_body) = members[1].clone();

    let carried_group = group_with(
        &router,
        vec![details(carried_member, &carried_body, false)],
        "the group an agent registered for",
    )
    .await;
    let waiting_group = group_with(
        &router,
        vec![details(waiting_member, &waiting_body, false)],
        "the group no agent registered for",
    )
    .await;

    // One agent carries the first group, among other capabilities of the account; another
    // carries nothing of this service at all.
    let agent_id = register_for(
        &router,
        0,
        vec![
            other_capability(0),
            expected_arn(&carried_group),
            other_capability(1),
        ],
        "the carried group",
    )
    .await;
    report(&router, 0, &agent_id, AGENT_ACTIVE, "the carried group").await;
    register_for(
        &router,
        1,
        vec![other_capability(2), other_capability(3)],
        "an agent of no group",
    )
    .await;

    let (first_read, endpoints) = read_group(&router, &carried_group, "the carried group").await;
    assert_reports(
        &endpoints[0],
        carried_member,
        AGENT_ACTIVE,
        (HEALTH_HEALTHY, REASON_HEALTHY),
        Some(AUDIT_HEALTHY),
        "the carried group",
    );

    let (_, endpoints) = read_group(&router, &waiting_group, "the waiting group").await;
    assert_reports(
        &endpoints[0],
        waiting_member,
        AGENT_INACTIVE,
        (HEALTH_UNHEALTHY, REASON_NO_REGISTERED_AGENT),
        None,
        "the waiting group",
    );

    // Reporting is not mutating: a read repeated reports exactly the same thing.
    let (second_read, _) =
        read_group(&router, &carried_group, "the carried group read twice").await;
    assert_eq!(
        second_read.field("endpointsDetails"),
        first_read.field("endpointsDetails"),
        "case the carried group read twice: a read must not change what the next one sees, \
         got {} then {}",
        first_read.body,
        second_read.body
    );
}

#[tokio::test]
async fn the_latest_registration_for_a_capability_carries_its_endpoints() {
    let (router, _engine) = harness_with_engine();
    let members = agent_members("takeover");
    let (member, body) = members[2].clone();
    let group_id = group_with(&router, vec![details(member, &body, false)], "takeover").await;
    let capability = expected_arn(&group_id);

    let first = register_for(&router, 0, vec![capability.clone()], "the first agent").await;
    report(&router, 0, &first, AGENT_ACTIVE, "the first agent").await;
    let (_, endpoints) = read_group(&router, &group_id, "the first agent").await;
    assert_reports(
        &endpoints[0],
        member,
        AGENT_ACTIVE,
        (HEALTH_HEALTHY, REASON_HEALTHY),
        Some(AUDIT_HEALTHY),
        "the first agent",
    );

    // A second agent registers for the same capability — the customer moved the agent onto
    // another instance — and has reported nothing yet, so the endpoint reports IT.
    let second = register_for(&router, 1, vec![capability], "the second agent").await;
    assert_ne!(
        first, second,
        "the two registrations must mint distinct agents, got {first} twice"
    );
    let (_, endpoints) = read_group(&router, &group_id, "the second agent").await;
    assert_reports(
        &endpoints[0],
        member,
        AGENT_INACTIVE,
        (HEALTH_UNHEALTHY, REASON_INITIALIZING_DATAPLANE),
        None,
        "the second agent, before it has reported",
    );

    // And it follows that agent's reports afterwards, not the one it took over from.
    report(&router, 1, &second, AGENT_FAILED, "the second agent").await;
    let (_, endpoints) = read_group(&router, &group_id, "the second agent reporting FAILED").await;
    assert_reports(
        &endpoints[0],
        member,
        AGENT_FAILED,
        (HEALTH_UNHEALTHY, REASON_DATAPLANE_FAILURE),
        Some(AUDIT_UNHEALTHY),
        "the second agent reporting FAILED",
    );
}

#[tokio::test]
async fn the_endpoint_a_config_names_reports_what_the_group_read_reports() {
    let (router, engine) = harness_with_engine();
    let members = agent_members("by-name");
    let (member, body) = members[0].clone();
    let endpoint_name = body["name"]
        .as_str()
        .expect("the endpoint's name")
        .to_string();
    let group_id = group_with(&router, vec![details(member, &body, false)], "by name").await;

    let agent_id = register_for(&router, 0, vec![expected_arn(&group_id)], "by name").await;
    report(&router, 0, &agent_id, AGENT_ACTIVE, "by name").await;

    let (_, endpoints) = read_group(&router, &group_id, "by name").await;
    assert_reports(
        &endpoints[0],
        member,
        AGENT_ACTIVE,
        (HEALTH_HEALTHY, REASON_HEALTHY),
        Some(AUDIT_HEALTHY),
        "by name",
    );

    // A `dataflowEndpointConfig` names an endpoint of this account by
    // `dataflowEndpointName`, and that lookup is the second read path onto a stored
    // endpoint: it must describe the endpoint the same way the group read just did.
    let state = service_state(&*engine).expect("this service's state store");
    let guard = state.lock();
    let by_name = guard.dataflow_endpoint(&endpoint_name).unwrap_or_else(|| {
        panic!("case by name: no endpoint of this account is named {endpoint_name}")
    });
    assert_eq!(
        by_name.health_reasons.as_deref(),
        Some([REASON_HEALTHY.to_string()].as_slice()),
        "case by name: expected healthReasons [{REASON_HEALTHY}], got {:?}",
        by_name.health_reasons
    );
    assert_eq!(
        by_name
            .aws_ground_station_agent_endpoint
            .as_ref()
            .and_then(|endpoint| endpoint.agent_status.as_deref()),
        Some(AGENT_ACTIVE),
        "case by name: expected agentStatus {AGENT_ACTIVE}, got {:?}",
        by_name.aws_ground_station_agent_endpoint
    );
}
