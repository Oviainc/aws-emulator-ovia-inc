// SPDX-License-Identifier: CC0-1.0

//! The AWS Ground Station Agent lifecycle, end to end over HTTP through the same front
//! door as the rest of this target's suites (the harness, signed `restJson1` requests and
//! the response reader come from [`crate::scheduling_loop`]).
//!
//! The model makes `Agent` a resource of this service with the lifecycle `RegisterAgent`
//! (create), `GetAgentConfiguration` (read) and `UpdateAgentStatus` (update), identified
//! by `agentId` and ARN-addressed as `agent/{agentId}` (`smithy_query(query="shape",
//! shape="Agent")`). Each suite asserts a RULE over a table of inputs rather than one
//! recorded call:
//!
//! * a registration mints a distinct `Agent` and reads back under it, and the agent starts
//!   with no status of its own — the status of an agent is what the AGENT reports;
//! * a status report is recorded against the agent the `{agentId}` label addresses and
//!   replaces the report before it, for every `AgentStatus` value, at each published bound
//!   of `ComponentStatusList`, with and without the optional counters and signature map;
//! * a report or a read for an id no agent was registered under is refused
//!   `InvalidParameterException` (HTTP 431) and changes nothing;
//! * a discovery address that is not an IPv4 address is refused
//!   `InvalidParameterException` (HTTP 431) naming the member, and nothing is stored,
//!   while each published octet bound is accepted;
//! * a component status list past the bound the model declares for it is refused by the
//!   model's own rules.

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    service_state, ResourceRef, AGENT_INACTIVE, BAD_REQUEST_ERROR_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{harness_with_engine, is_uuid, send, Reply, ABSENT_UUID, REGION};

/// The values `AgentStatus` accepts (`smithy_query(query="shape", shape="AgentStatus")`).
/// Every one of them is a status an agent may report, so the report table spans all four.
const AGENT_STATUSES: &[&str] = &["SUCCESS", "FAILED", "ACTIVE", "INACTIVE"];

/// The published bounds of `ComponentStatusList` (len 0..20 —
/// `smithy_query(query="shape", shape="ComponentStatusList")`).
const COMPONENT_STATUSES_MIN: usize = 0;
/// See [`COMPONENT_STATUSES_MIN`].
const COMPONENT_STATUSES_MAX: usize = 20;

/// Register an agent from `body`.
pub(crate) async fn register(router: &MultiServiceRouter, body: Value) -> Reply {
    send(router, "POST", "/agent", Some(body)).await
}

/// Report `body` as `agent_id`'s status.
pub(crate) async fn update_status(
    router: &MultiServiceRouter,
    agent_id: &str,
    body: Value,
) -> Reply {
    send(router, "PUT", &format!("/agent/{agent_id}"), Some(body)).await
}

/// Read `agent_id`'s configuration back.
pub(crate) async fn get_configuration(router: &MultiServiceRouter, agent_id: &str) -> Reply {
    send(
        router,
        "GET",
        &format!("/agent/{agent_id}/configuration"),
        None,
    )
    .await
}

/// A `Uuid` the test derives from `index`, spelled the way the model requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), so a row's task and
/// dataflow ids are distinct and well formed without being drawn from any recording.
pub(crate) fn uuid_from(prefix: u32, index: usize) -> String {
    format!("{prefix:08x}-0000-4000-8000-{index:012x}")
}

/// The ARN this service mints for an agent: `agent/{agentId}` under the request's region
/// and the emulator's account, which is the ARN template the model gives the `Agent`
/// resource.
fn expected_arn(agent_id: &str) -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:agent/{agent_id}",
        engine::emulator_account_id()
    )
}

/// A `CapabilityArn` for the `index`th component. `CapabilityArn` is an `arnReference` to
/// a capability of the Ground Station PROVIDER (`smithy_query(query="shape",
/// shape="CapabilityArn")`) — a resource no operation of this model creates — so the value
/// is only required to be a string naming one.
pub(crate) fn capability_arn(index: usize) -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:capability/{}-antenna",
        engine::emulator_account_id(),
        index + 1
    )
}

/// What the service must hold after a component status was reported.
struct ExpectedComponent {
    component_type: String,
    capability_arn: String,
    status: String,
    dataflow_id: String,
    /// `bytesSent`, `bytesReceived` and `packetsDropped`, which the agent may leave out.
    counters: Option<(i64, i64, i64)>,
}

/// The `index`th component status of a report: what the request carries, and what the
/// service must hold for it afterwards.
fn component_status(index: usize, status: &str, counters: bool) -> (Value, ExpectedComponent) {
    let component_type = format!("Component_{index}");
    let capability = capability_arn(index);
    let dataflow_id = uuid_from(0x0d0d_0d0d, index);
    let mut body = Map::new();
    body.insert("componentType".to_string(), json!(component_type));
    body.insert("capabilityArn".to_string(), json!(capability));
    body.insert("status".to_string(), json!(status));
    body.insert("dataflowId".to_string(), json!(dataflow_id));
    let counter_values = counters.then(|| {
        let sent = 1_024 * (index as i64 + 1);
        let received = 2_048 * (index as i64 + 1);
        let dropped = index as i64;
        body.insert("bytesSent".to_string(), json!(sent));
        body.insert("bytesReceived".to_string(), json!(received));
        body.insert("packetsDropped".to_string(), json!(dropped));
        (sent, received, dropped)
    });
    (
        Value::Object(body),
        ExpectedComponent {
            component_type,
            capability_arn: capability,
            status: status.to_string(),
            dataflow_id,
            counters: counter_values,
        },
    )
}

/// One row of the status-report table.
struct Report {
    label: String,
    body: Value,
    aggregate_status: String,
    signature_map: Option<Vec<(String, bool)>>,
    components: Vec<ExpectedComponent>,
    task_id: String,
}

/// The status-report table: every `AgentStatus` value, each at a different
/// `ComponentStatusList` size (both published bounds included), with the optional
/// `signatureMap` and the optional per-component counters present on some rows and absent
/// on others — the dimensions a report varies along.
fn reports() -> Vec<Report> {
    let sizes = [COMPONENT_STATUSES_MIN, 1, 3, COMPONENT_STATUSES_MAX];
    AGENT_STATUSES
        .iter()
        .enumerate()
        .map(|(row, status)| {
            let size = sizes[row % sizes.len()];
            let counters = row % 2 == 0;
            let signature_map = (row % 2 == 1).then(|| {
                vec![
                    ("gs-agent".to_string(), true),
                    (format!("component-{row}"), false),
                ]
            });
            let (bodies, components): (Vec<Value>, Vec<ExpectedComponent>) = (0..size)
                .map(|index| component_status(index, status, counters))
                .unzip();
            let task_id = uuid_from(0x7a5c_0000 + row as u32, row);
            let mut aggregate = Map::new();
            aggregate.insert("status".to_string(), json!(status));
            if let Some(entries) = signature_map.as_ref() {
                aggregate.insert(
                    "signatureMap".to_string(),
                    Value::Object(
                        entries
                            .iter()
                            .map(|(key, value)| (key.clone(), json!(value)))
                            .collect(),
                    ),
                );
            }
            Report {
                label: format!(
                    "{status} with {size} component statuses, counters {}, signatureMap {}",
                    if counters { "set" } else { "absent" },
                    if signature_map.is_some() {
                        "set"
                    } else {
                        "absent"
                    }
                ),
                body: json!({
                    "taskId": task_id,
                    "aggregateStatus": Value::Object(aggregate),
                    "componentStatuses": bodies,
                }),
                aggregate_status: (*status).to_string(),
                signature_map,
                components,
                task_id,
            }
        })
        .collect()
}

/// One row of the registration table: the `RegisterAgent` body, and what it says about
/// the agent.
struct Registration {
    label: &'static str,
    body: Value,
    instance_id: &'static str,
    capability_arns: Vec<String>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The registration table: the required members alone, then each optional member and each
/// repeatable list carrying more than one entry — the dimensions a registration varies
/// along (`smithy_query(query="shape", shape="AgentDetails,DiscoveryData")`).
fn registrations() -> Vec<Registration> {
    vec![
        Registration {
            label: "the required members, with the agent cpu cores at the published \
                    minimum of AgentCpuCoresList",
            body: json!({
                "agentDetails": {
                    "agentVersion": "1.0.0",
                    "instanceId": "i-0123456789",
                    "instanceType": "m5.large",
                    "agentCpuCores": Vec::<i64>::new(),
                    "componentVersions": [{"componentType": "Agent", "versions": ["1.0.0"]}],
                },
                "discoveryData": {
                    "publicIpAddresses": ["52.0.0.1"],
                    "privateIpAddresses": ["10.0.0.1"],
                    "capabilityArns": [capability_arn(0)],
                },
            }),
            instance_id: "i-0123456789",
            capability_arns: vec![capability_arn(0)],
            tags: Vec::new(),
        },
        Registration {
            label: "cpu cores, several component versions and tags",
            body: json!({
                "agentDetails": {
                    "agentVersion": "2.11.3",
                    "instanceId": "i-abcdef0123456789",
                    "instanceType": "c5.24xlarge",
                    "agentCpuCores": [0, 1, 2, 3],
                    "reservedCpuCores": [4, 5],
                    "componentVersions": [
                        {"componentType": "Agent", "versions": ["2.11.3", "2.11.2"]},
                        {"componentType": "Dataplane", "versions": ["0.9.1"]},
                    ],
                },
                "discoveryData": {
                    "publicIpAddresses": ["52.0.0.2", "52.0.0.3"],
                    "privateIpAddresses": ["10.0.0.2", "172.31.255.254"],
                    "capabilityArns": [capability_arn(1), capability_arn(2)],
                },
                "tags": {"mission": "aqua", "stage": "registered"},
            }),
            instance_id: "i-abcdef0123456789",
            capability_arns: vec![capability_arn(1), capability_arn(2)],
            tags: vec![("mission", "aqua"), ("stage", "registered")],
        },
        Registration {
            label: "the superseded cpu-core member alone, and addresses at both published \
                    octet bounds",
            body: json!({
                "agentDetails": {
                    "agentVersion": "3",
                    "instanceId": "i-9876543210",
                    "instanceType": "m5.4xlarge",
                    "reservedCpuCores": [0, 1],
                    "componentVersions": [{"componentType": "Agent", "versions": ["3"]}],
                },
                "discoveryData": {
                    "publicIpAddresses": ["255.255.255.255"],
                    "privateIpAddresses": ["0.0.0.0"],
                    "capabilityArns": [capability_arn(3)],
                },
            }),
            instance_id: "i-9876543210",
            capability_arns: vec![capability_arn(3)],
            tags: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn every_registration_mints_an_agent_that_reads_back_with_no_status_of_its_own() {
    let (router, engine) = harness_with_engine();
    let mut minted: Vec<String> = Vec::new();

    for registration in registrations() {
        let Registration {
            label,
            body,
            instance_id,
            capability_arns,
            tags,
        } = registration;

        let registered = register(&router, body).await;
        assert_eq!(
            registered.status, 200,
            "case {label}: expected RegisterAgent 200, got {} {}",
            registered.status, registered.body
        );
        let agent_id = registered.string("agentId");
        assert!(
            is_uuid(&agent_id),
            "case {label}: the minted agentId must be a lowercase uuid, got {agent_id:?}"
        );
        assert!(
            !minted.contains(&agent_id),
            "case {label}: every registration mints its own agent, got the already \
             minted {agent_id:?}"
        );
        minted.push(agent_id.clone());

        // The read half of the resource's lifecycle: the agent reads back under the id it
        // was registered with, and carries no tasking document, nothing having tasked it.
        let read = get_configuration(&router, &agent_id).await;
        assert_eq!(
            read.status, 200,
            "case {label}: expected GetAgentConfiguration 200, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("agentId"),
            agent_id,
            "case {label}: the agent must read back under the id it was registered with, \
             got {}",
            read.body
        );
        assert_eq!(
            read.field("taskingDocument"),
            &Value::Null,
            "case {label}: an agent nothing has tasked carries no taskingDocument, got {}",
            read.body
        );

        let state = service_state(&*engine).expect("this service's state store");
        let guard = state.lock();
        let agent = guard
            .agents
            .get(&agent_id)
            .unwrap_or_else(|| panic!("case {label}: the registration must persist the agent"));
        assert_eq!(
            agent.agent_arn,
            expected_arn(&agent_id),
            "case {label}: expected the agent/{{agentId}} ARN {}, got {:?}",
            expected_arn(&agent_id),
            agent.agent_arn
        );
        // The minted ARN resolves through the service's one ARN index, the way every
        // other ARN this service mints does.
        assert_eq!(
            guard.arn_index.get(&agent.agent_arn),
            Some(&ResourceRef::Agent {
                agent_id: agent_id.clone()
            }),
            "case {label}: the minted ARN must resolve to this agent, got {:?}",
            guard.arn_index.get(&agent.agent_arn)
        );
        assert_eq!(
            agent.agent_details.instance_id, instance_id,
            "case {label}: expected the registered instanceId {instance_id}, got {:?}",
            agent.agent_details.instance_id
        );
        assert_eq!(
            agent.discovery_data.capability_arns, capability_arns,
            "case {label}: expected the registered capabilityArns {capability_arns:?}, got {:?}",
            agent.discovery_data.capability_arns
        );
        for (key, value) in &tags {
            assert_eq!(
                agent.tags.get(*key).map(String::as_str),
                Some(*value),
                "case {label}: expected tag {key}={value}, got {:?}",
                agent.tags
            );
        }
        assert_eq!(
            agent.tags.len(),
            tags.len(),
            "case {label}: expected {} tags, got {:?}",
            tags.len(),
            agent.tags
        );

        // The status of an agent is what the AGENT reports: a registration asserts none.
        assert_eq!(
            agent.aggregate_status.status, AGENT_INACTIVE,
            "case {label}: a registered agent that has reported nothing is \
             {AGENT_INACTIVE}, got {:?}",
            agent.aggregate_status.status
        );
        assert!(
            agent.component_statuses.is_empty(),
            "case {label}: a registered agent that has reported nothing holds no component \
             status, got {:?}",
            agent.component_statuses
        );
        assert!(
            agent.task_id.is_none(),
            "case {label}: a registered agent that has reported nothing holds no task, got {:?}",
            agent.task_id
        );
        assert_eq!(
            agent.last_updated.as_secs_f64(),
            agent.registered.as_secs_f64(),
            "case {label}: an agent that has reported nothing was last changed when it \
             registered, got {:?}",
            agent.last_updated
        );
    }
}

#[tokio::test]
async fn a_report_replaces_the_status_the_service_holds_for_the_agent_it_addresses() {
    let (router, engine) = harness_with_engine();

    let reported = register(&router, registrations().remove(0).body).await;
    assert_eq!(
        reported.status, 200,
        "the agent under test must register: got {} {}",
        reported.status, reported.body
    );
    let agent_id = reported.string("agentId");

    // A second agent, registered and never reported on: a report must land on the record
    // its `{agentId}` label addresses and on no other.
    let untouched = register(&router, registrations().remove(1).body).await;
    assert_eq!(
        untouched.status, 200,
        "the bystander agent must register: got {} {}",
        untouched.status, untouched.body
    );
    let bystander_id = untouched.string("agentId");

    for report in reports() {
        let Report {
            label,
            body,
            aggregate_status,
            signature_map,
            components,
            task_id,
        } = report;

        let answer = update_status(&router, &agent_id, body).await;
        assert_eq!(
            answer.status, 200,
            "case {label}: expected UpdateAgentStatus 200, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.string("agentId"),
            agent_id,
            "case {label}: the answer names the agent that was updated, got {}",
            answer.body
        );

        let state = service_state(&*engine).expect("this service's state store");
        let guard = state.lock();
        let agent = guard
            .agents
            .get(&agent_id)
            .unwrap_or_else(|| panic!("case {label}: the reported agent must still be stored"));
        assert_eq!(
            agent.aggregate_status.status, aggregate_status,
            "case {label}: expected the reported aggregate status {aggregate_status}, got {:?}",
            agent.aggregate_status.status
        );
        match &signature_map {
            Some(entries) => {
                let held = agent
                    .aggregate_status
                    .signature_map
                    .as_ref()
                    .unwrap_or_else(|| panic!("case {label}: expected a reported signatureMap"));
                assert_eq!(
                    held.len(),
                    entries.len(),
                    "case {label}: expected {} signatureMap entries, got {held:?}",
                    entries.len()
                );
                for (key, value) in entries {
                    assert_eq!(
                        held.get(key),
                        Some(value),
                        "case {label}: expected signatureMap {key}={value}, got {held:?}"
                    );
                }
            }
            None => assert!(
                agent.aggregate_status.signature_map.is_none(),
                "case {label}: a report carrying no signatureMap leaves none, got {:?}",
                agent.aggregate_status.signature_map
            ),
        }
        assert_eq!(
            agent.task_id.as_deref(),
            Some(task_id.as_str()),
            "case {label}: expected the reported taskId {task_id}, got {:?}",
            agent.task_id
        );
        assert!(
            agent.last_updated.as_secs_f64() >= agent.registered.as_secs_f64(),
            "case {label}: a report moves the agent's last-changed instant no earlier than \
             its registration, got {:?} against {:?}",
            agent.last_updated,
            agent.registered
        );

        // The report is a snapshot: what the service holds is this report's component
        // statuses, not those of the report before it (the rows differ in length, so a
        // merge rather than a replacement fails here).
        assert_eq!(
            agent.component_statuses.len(),
            components.len(),
            "case {label}: expected {} component statuses, got {:?}",
            components.len(),
            agent.component_statuses
        );
        for (index, expected) in components.iter().enumerate() {
            let held = &agent.component_statuses[index];
            assert_eq!(
                held.component_type, expected.component_type,
                "case {label}: component {index}: expected componentType {}, got {:?}",
                expected.component_type, held.component_type
            );
            assert_eq!(
                held.capability_arn, expected.capability_arn,
                "case {label}: component {index}: expected capabilityArn {}, got {:?}",
                expected.capability_arn, held.capability_arn
            );
            assert_eq!(
                held.status, expected.status,
                "case {label}: component {index}: expected status {}, got {:?}",
                expected.status, held.status
            );
            assert_eq!(
                held.dataflow_id, expected.dataflow_id,
                "case {label}: component {index}: expected dataflowId {}, got {:?}",
                expected.dataflow_id, held.dataflow_id
            );
            let held_counters = (held.bytes_sent, held.bytes_received, held.packets_dropped);
            match expected.counters {
                Some((sent, received, dropped)) => assert_eq!(
                    held_counters,
                    (Some(sent), Some(received), Some(dropped)),
                    "case {label}: component {index}: expected the reported counters \
                     ({sent}, {received}, {dropped}), got {held_counters:?}"
                ),
                None => assert_eq!(
                    held_counters,
                    (None, None, None),
                    "case {label}: component {index}: a report carrying no counters leaves \
                     none, got {held_counters:?}"
                ),
            }
        }

        let bystander = guard
            .agents
            .get(&bystander_id)
            .unwrap_or_else(|| panic!("case {label}: the bystander agent must still be stored"));
        assert_eq!(
            bystander.aggregate_status.status, AGENT_INACTIVE,
            "case {label}: a report updates only the agent it addresses, but the \
             bystander became {:?}",
            bystander.aggregate_status.status
        );
        assert!(
            bystander.component_statuses.is_empty(),
            "case {label}: a report updates only the agent it addresses, but the bystander \
             holds {:?}",
            bystander.component_statuses
        );
    }
}

#[tokio::test]
async fn an_agent_that_was_never_registered_is_refused_and_nothing_changes() {
    let (router, engine) = harness_with_engine();

    let registered = register(&router, registrations().remove(0).body).await;
    let agent_id = registered.string("agentId");
    let first = reports().remove(0);
    let accepted = update_status(&router, &agent_id, first.body).await;
    assert_eq!(
        accepted.status, 200,
        "the registered agent must accept a report: got {} {}",
        accepted.status, accepted.body
    );

    // Well-formed `Uuid`s no agent was ever registered under: the one the suites share,
    // and two more so the refusal is not a property of a single value.
    let absent = [
        ABSENT_UUID.to_string(),
        uuid_from(0xdead_beef, 7),
        "ffffffff-ffff-4fff-8fff-ffffffffffff".to_string(),
    ];
    for id in &absent {
        let report = reports().remove(1);
        let refused = update_status(&router, id, report.body).await;
        assert_eq!(
            refused.status, 431,
            "case {id}: expected UpdateAgentStatus 431 for an unregistered agent, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {id}: expected InvalidParameterException, got {}",
            refused.body
        );
        assert_eq!(
            refused.field("message").as_str(),
            Some("Invalid AgentID."),
            "case {id}: the refusal is about the id as a parameter and names none, got {}",
            refused.body
        );

        let read = get_configuration(&router, id).await;
        assert_eq!(
            read.status, 431,
            "case {id}: expected GetAgentConfiguration 431 for an unregistered agent, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "InvalidParameterException",
            "case {id}: expected InvalidParameterException, got {}",
            read.body
        );
    }

    let state = service_state(&*engine).expect("this service's state store");
    let guard = state.lock();
    assert_eq!(
        guard.agents.len(),
        1,
        "a report for an agent that is not there registers none, got {:?}",
        guard.agents.keys().collect::<Vec<_>>()
    );
    let agent = guard
        .agents
        .get(&agent_id)
        .expect("the registered agent must still be stored");
    assert_eq!(
        agent.aggregate_status.status, first.aggregate_status,
        "a refused report leaves the registered agent's status alone, got {:?}",
        agent.aggregate_status.status
    );
}

#[tokio::test]
async fn a_discovery_address_that_is_not_an_ipv4_address_is_refused() {
    let (router, engine) = harness_with_engine();

    // Each row satisfies `IpV4Address`'s pattern (four dot-separated runs of up to three
    // digits — `smithy_query(query="shape", shape="IpAddressList")`) and is still not an
    // address: the octet one past the published 0..255 bound, in each list.
    let rows: &[(&str, &str, &str)] = &[
        (
            "a public address one past the top octet",
            "publicIpAddresses",
            "256.0.0.1",
        ),
        (
            "a private address one past the top octet",
            "privateIpAddresses",
            "10.0.0.256",
        ),
        (
            "a public address with a three-digit octet past the bound",
            "publicIpAddresses",
            "52.0.0.999",
        ),
    ];

    for (label, member, address) in rows {
        let mut discovery = json!({
            "publicIpAddresses": ["52.0.0.1"],
            "privateIpAddresses": ["10.0.0.1"],
            "capabilityArns": [capability_arn(0)],
        });
        discovery[*member] = json!([address]);
        let body = json!({
            "agentDetails": {
                "agentVersion": "1.0.0",
                "instanceId": "i-0123456789",
                "instanceType": "m5.large",
                "agentCpuCores": [0, 1],
                "componentVersions": [{"componentType": "Agent", "versions": ["1.0.0"]}],
            },
            "discoveryData": discovery,
        });

        let refused = register(&router, body).await;
        assert_eq!(
            refused.status, 431,
            "case {label}: expected RegisterAgent 431, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {label}: expected InvalidParameterException, got {}",
            refused.body
        );
        assert_eq!(
            refused.field("parameterName").as_str(),
            Some(format!("discoveryData.{member}").as_str()),
            "case {label}: the refusal names the member that is wrong, got {}",
            refused.body
        );
    }

    let state = service_state(&*engine).expect("this service's state store");
    assert!(
        state.lock().agents.is_empty(),
        "a refused registration stores no agent, got {:?}",
        state.lock().agents.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_component_status_list_past_its_modelled_bound_is_refused() {
    let (router, engine) = harness_with_engine();

    let registered = register(&router, registrations().remove(0).body).await;
    let agent_id = registered.string("agentId");

    let (bodies, _): (Vec<Value>, Vec<ExpectedComponent>) = (0..=COMPONENT_STATUSES_MAX)
        .map(|index| component_status(index, "ACTIVE", true))
        .unzip();
    let refused = update_status(
        &router,
        &agent_id,
        json!({
            "taskId": uuid_from(0x7a5c_0000, 99),
            "aggregateStatus": {"status": "ACTIVE"},
            "componentStatuses": bodies,
        }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "a report of {} component statuses is one past the modelled bound: expected 400, \
         got {} {}",
        COMPONENT_STATUSES_MAX + 1,
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.error_code(),
        BAD_REQUEST_ERROR_CODE,
        "expected {BAD_REQUEST_ERROR_CODE} for a report past the modelled bound, got {}",
        refused.body
    );

    let state = service_state(&*engine).expect("this service's state store");
    let guard = state.lock();
    let agent = guard
        .agents
        .get(&agent_id)
        .expect("the registered agent must still be stored");
    assert!(
        agent.component_statuses.is_empty(),
        "a refused report records nothing, got {:?}",
        agent.component_statuses
    );
    assert_eq!(
        agent.aggregate_status.status, AGENT_INACTIVE,
        "a refused report leaves the agent's status alone, got {:?}",
        agent.aggregate_status.status
    );
}
