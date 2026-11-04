// SPDX-License-Identifier: CC0-1.0

//! `GetAgentConfiguration` — the READ of the `Agent` resource, end to end over HTTP
//! through the same front door as the rest of this target's suites (the harness, the
//! signed `restJson1` requests and the response reader come from
//! [`crate::scheduling_loop`]).
//!
//! The RULE these suites pin: `GetAgentConfiguration` reports the configuration the
//! service HOLDS for the agent its `{agentId}` label addresses — that agent's own
//! `agentId` and the tasking document stored against it — for any registered agent
//! whatever form it registered in; being the resource's read (`readonly` —
//! `smithy_query(query="operation", operation="GetAgentConfiguration")`) it leaves the
//! store exactly as it found it, so repeated reads answer identically; and an `{agentId}`
//! no agent was registered under is refused `InvalidParameterException` (HTTP 431).
//!
//! `tests/all/agent_lifecycle.rs` asserts the lifecycle as a whole (register, report,
//! read); what is asserted HERE is the read's own rule, over a table of registration
//! forms and a table of held tasking documents, which is the only way to tell a response
//! computed from the stored agent from a constant answer that happens to match a freshly
//! registered one.

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

// The front door of the `Agent` resource is written once, in `agent_lifecycle`: this
// suite asserts the READ's own rule over those same helpers rather than its own copies.
use crate::agent_lifecycle::{
    capability_arn, get_configuration, register, update_status, uuid_from,
};
use crate::scheduling_loop::{harness_with_engine, is_uuid, ABSENT_UUID};

/// The published bounds shared by `ComponentVersionList` and by the three lists of
/// `DiscoveryData` (`IpAddressList`, `CapabilityArnList`): len 1..20 —
/// `smithy_query(query="shape", shape="AgentDetails,DiscoveryData")`. A registration form
/// is built at each bound, so the read is asserted on the smallest and the largest agent
/// the model admits.
const LIST_MIN: usize = 1;
/// See [`LIST_MIN`].
const LIST_MAX: usize = 20;

/// The published bounds of `InstanceId` (len 10..64, pattern `^[a-z0-9-]{10,64}$` —
/// `smithy_query(query="shape", shape="AgentDetails")`).
const INSTANCE_ID_MIN_LEN: usize = 10;
/// See [`INSTANCE_ID_MIN_LEN`].
const INSTANCE_ID_MAX_LEN: usize = 64;

/// Every agent this service holds, rendered so two snapshots compare field by field.
/// `Agent` derives `Debug`, so this covers every member of every stored agent — status,
/// component statuses, task, tasking document, tags and timestamps — which is what a read
/// must leave alone.
fn snapshot(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("this service's state store");
    let guard = state.lock();
    let mut agents: Vec<String> = guard
        .agents
        .values()
        .map(|agent| format!("{agent:?}"))
        .collect();
    agents.sort();
    agents
}

/// An `InstanceId` of exactly `len` characters, spelled the way the model requires.
fn instance_id(len: usize) -> String {
    let suffix = "0123456789abcdefghijklmnopqrstuvwxyz0123456789-0123456789abcdef";
    format!("i-{}", &suffix[..len.saturating_sub(2)])
}

/// `count` component versions, each a distinct `ComponentVersion`.
fn component_versions(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "componentType": format!("component_{index}"),
                    "versions": [format!("{}.0.{index}", index + 1)],
                })
            })
            .collect(),
    )
}

/// `count` distinct dotted-quad addresses in the `block`th /16.
fn ip_addresses(block: usize, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| Value::String(format!("10.{block}.{}.{}", index / 256, index % 256 + 1)))
            .collect(),
    )
}

/// `count` distinct `CapabilityArn`s, spelled by `agent_lifecycle`'s [`capability_arn`].
fn capability_arns(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| Value::String(capability_arn(index)))
            .collect(),
    )
}

/// The registration forms the read is asserted over: the rows differ in the SIZE of every
/// bounded list the registration carries, in the length of the instance id, and in which
/// optional members are present — the dimensions a registration can vary along — so a
/// read that reported anything but the addressed agent's own record would disagree with at
/// least one row.
fn registration_forms() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the smallest agent the model admits: every list at its published minimum, \
             the instance id at its minimum length, the agent cpu cores empty",
            json!({
                "agentDetails": {
                    "agentVersion": "1.0.0",
                    "instanceId": instance_id(INSTANCE_ID_MIN_LEN),
                    "instanceType": "m5.large",
                    "componentVersions": component_versions(LIST_MIN),
                    "agentCpuCores": Vec::<i64>::new(),
                },
                "discoveryData": {
                    "publicIpAddresses": ip_addresses(0, LIST_MIN),
                    "privateIpAddresses": ip_addresses(1, LIST_MIN),
                    "capabilityArns": capability_arns(LIST_MIN),
                },
            }),
        ),
        (
            "the largest agent the model admits: every list at its published maximum, the \
             instance id at its maximum length, both cpu-core lists and tags",
            json!({
                "agentDetails": {
                    "agentVersion": "2.11.0",
                    "instanceId": instance_id(INSTANCE_ID_MAX_LEN),
                    "instanceType": "c6in.32xlarge",
                    "componentVersions": component_versions(LIST_MAX),
                    "agentCpuCores": (0..8).collect::<Vec<i64>>(),
                    "reservedCpuCores": vec![8_i64, 9],
                },
                "discoveryData": {
                    "publicIpAddresses": ip_addresses(2, LIST_MAX),
                    "privateIpAddresses": ip_addresses(3, LIST_MAX),
                    "capabilityArns": capability_arns(LIST_MAX),
                },
                "tags": { "mission": "pathfinder", "band": "sband" },
            }),
        ),
        (
            "an agent between the bounds: three component versions, two addresses per \
             list, the agent cpu cores only",
            json!({
                "agentDetails": {
                    "agentVersion": "3",
                    "instanceId": instance_id(24),
                    "instanceType": "m6i.8xlarge",
                    "componentVersions": component_versions(3),
                    "agentCpuCores": vec![0_i64, 1, 2],
                },
                "discoveryData": {
                    "publicIpAddresses": ip_addresses(4, 2),
                    "privateIpAddresses": ip_addresses(5, 2),
                    "capabilityArns": capability_arns(2),
                },
            }),
        ),
    ]
}

/// Register every form of [`registration_forms`] and return each row's label with the id
/// it minted.
async fn register_every_form(router: &MultiServiceRouter) -> Vec<(&'static str, String)> {
    let mut registered: Vec<(&'static str, String)> = Vec::new();
    for (label, body) in registration_forms() {
        let reply = register(router, body).await;
        assert_eq!(
            reply.status, 200,
            "case {label}: the registration under test must succeed, got {} {}",
            reply.status, reply.body
        );
        let agent_id = reply.string("agentId");
        assert!(
            is_uuid(&agent_id),
            "case {label}: the minted agentId must be a lowercase uuid, got {agent_id:?}"
        );
        registered.push((label, agent_id));
    }
    registered
}

#[tokio::test]
async fn the_read_reports_the_agent_its_label_addresses_and_changes_nothing() {
    let (router, engine) = harness_with_engine();
    let registered = register_every_form(&router).await;

    let before = snapshot(&engine);
    assert_eq!(
        before.len(),
        registration_forms().len(),
        "every form must have been stored, got {} agents",
        before.len()
    );

    // Read every agent twice, interleaved across the agents: a read reports the record its
    // label addresses — never a neighbour's, never the one registered last — and answers
    // the same on the second call, nothing about the agent having been consumed.
    for attempt in 0..2 {
        for (label, agent_id) in &registered {
            let read = get_configuration(&router, agent_id).await;
            assert_eq!(
                read.status, 200,
                "case {label} (read {attempt}): expected 200, got {} {}",
                read.status, read.body
            );
            assert_eq!(
                read.string("agentId"),
                *agent_id,
                "case {label} (read {attempt}): the read must report the agent its label \
                 addresses, got {}",
                read.body
            );
            assert_eq!(
                read.field("taskingDocument"),
                &Value::Null,
                "case {label} (read {attempt}): an agent nothing has tasked carries no \
                 taskingDocument, got {}",
                read.body
            );
        }
    }

    assert_eq!(
        snapshot(&engine),
        before,
        "reading an agent's configuration must leave every stored agent as it was"
    );
}

#[tokio::test]
async fn the_read_reports_the_tasking_document_the_service_holds_for_that_agent() {
    let (router, engine) = harness_with_engine();
    let registered = register_every_form(&router).await;

    // No operation of this model tasks an agent (`smithy_query(query="digest")` carries
    // none), so the documents are placed in the store directly — the seam the SERVICE
    // would task an agent through. The rows differ in the content and the size of the
    // document, including an agent left untasked, so what the read reports has to come
    // from the addressed agent's own record.
    let long_document = "downlink-segment\n".repeat(512);
    let documents: Vec<(&str, Option<String>)> = vec![
        (
            "a structured document",
            Some(r#"{"version":1,"tasks":[{"capability":"antenna","mode":"downlink"}]}"#.into()),
        ),
        ("a document of non-ascii text", Some("télémétrie ≤ 2 GHz".into())),
        ("no document at all", None),
        ("a long document", Some(long_document)),
        ("an empty document", Some(String::new())),
    ];

    for (document_label, document) in &documents {
        for (form_label, agent_id) in &registered {
            let case = format!("{form_label} / {document_label}");
            {
                let state = service_state(&*engine).expect("this service's state store");
                let mut guard = state.lock();
                let agent = guard
                    .agents
                    .get_mut(agent_id)
                    .unwrap_or_else(|| panic!("case {case}: the registered agent must be stored"));
                agent.tasking_document = document.clone();
            }

            let expected = match document {
                Some(text) => Value::String(text.clone()),
                None => Value::Null,
            };
            // Twice: a tasking document is reported, not handed over, so the second read
            // of the same agent reports the same document.
            for attempt in 0..2 {
                let read = get_configuration(&router, agent_id).await;
                assert_eq!(
                    read.status, 200,
                    "case {case} (read {attempt}): expected 200, got {} {}",
                    read.status, read.body
                );
                assert_eq!(
                    read.field("taskingDocument"),
                    &expected,
                    "case {case} (read {attempt}): the read must report the tasking document \
                     the service holds for the agent, got {}",
                    read.field("taskingDocument")
                );
                assert_eq!(
                    read.string("agentId"),
                    *agent_id,
                    "case {case} (read {attempt}): the read must report the agent its label \
                     addresses, got {}",
                    read.body
                );
            }

            let state = service_state(&*engine).expect("this service's state store");
            let guard = state.lock();
            let held = guard
                .agents
                .get(agent_id)
                .unwrap_or_else(|| panic!("case {case}: the registered agent must be stored"))
                .tasking_document
                .clone();
            assert_eq!(
                held, *document,
                "case {case}: reading the configuration must not consume the document the \
                 service holds, got {held:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_agent_id_no_agent_was_registered_under_is_refused() {
    let (router, engine) = harness_with_engine();
    let registered = register_every_form(&router).await;
    let before = snapshot(&engine);

    // Well-formed `Uuid`s that address no agent: the all-zero id, ids from a different
    // namespace than the ones this store minted, and one id mutated from a registered one
    // (its first hex digit cycled), which is the closest miss there is.
    let registered_id = registered[0].1.clone();
    let mut near_miss: Vec<char> = registered_id.chars().collect();
    near_miss[0] = match near_miss[0] {
        'f' => '0',
        other => char::from_digit(other.to_digit(16).expect("a hex digit") + 1, 16)
            .expect("a hex digit"),
    };
    let unregistered: Vec<(&str, String)> = vec![
        ("the all-zero id", ABSENT_UUID.to_string()),
        ("an id from another namespace", uuid_from(0x0bad_cafe, 7)),
        ("an id one digit off a registered one", near_miss.iter().collect()),
    ];

    for (label, agent_id) in &unregistered {
        assert!(
            is_uuid(agent_id),
            "case {label}: the id under test must satisfy the modelled Uuid, got {agent_id:?}"
        );
        let refused = get_configuration(&router, agent_id).await;
        assert_eq!(
            refused.status, 431,
            "case {label}: expected HTTP 431, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {label}: expected InvalidParameterException, got {}",
            refused.body
        );
        assert_eq!(
            refused.field("message").as_str(),
            Some("Invalid AgentID."),
            "case {label}: the refusal is about the id as a parameter and names none, got {}",
            refused.body
        );
    }

    assert_eq!(
        snapshot(&engine),
        before,
        "a refused read must leave every stored agent as it was"
    );

    // And the registered agents still read back: a refusal is about the id, not the store.
    for (label, agent_id) in &registered {
        let read = get_configuration(&router, agent_id).await;
        assert_eq!(
            read.status, 200,
            "case {label}: the registered agent must still read back, got {} {}",
            read.status, read.body
        );
    }
}

#[tokio::test]
async fn the_read_is_unaffected_by_what_the_agent_reports_about_itself() {
    let (router, engine) = harness_with_engine();
    let registered = register_every_form(&router).await;

    // `UpdateAgentStatus` is the resource's update, and what it writes is the agent's
    // STATUS: the configuration read keeps reporting the agent's id and the service's
    // tasking document across every reported status.
    for (index, (label, agent_id)) in registered.iter().enumerate() {
        for status in ["SUCCESS", "FAILED", "ACTIVE", "INACTIVE"] {
            let reported = update_status(
                &router,
                agent_id,
                json!({
                    "taskId": uuid_from(0x0000_7a5c, index),
                    "aggregateStatus": { "status": status },
                    "componentStatuses": Value::Array(Vec::new()),
                }),
            )
            .await;
            assert_eq!(
                reported.status, 200,
                "case {label} / {status}: the report must be accepted, got {} {}",
                reported.status, reported.body
            );

            let read = get_configuration(&router, agent_id).await;
            assert_eq!(
                read.status, 200,
                "case {label} / {status}: expected 200, got {} {}",
                read.status, read.body
            );
            assert_eq!(
                read.string("agentId"),
                *agent_id,
                "case {label} / {status}: the read must report the agent its label \
                 addresses, got {}",
                read.body
            );
            assert_eq!(
                read.field("taskingDocument"),
                &Value::Null,
                "case {label} / {status}: a status report does not task the agent, got {}",
                read.body
            );
            // The read is not what moves the agent's status: the store holds exactly what
            // the agent reported.
            let state = service_state(&*engine).expect("this service's state store");
            let guard = state.lock();
            let agent = guard
                .agents
                .get(agent_id)
                .unwrap_or_else(|| panic!("case {label}: the registered agent must be stored"));
            assert_eq!(
                agent.aggregate_status.status, status,
                "case {label} / {status}: reading the configuration must not move the \
                 reported status, got {:?}",
                agent.aggregate_status.status
            );
        }
    }

    // The read of an UNTASKED agent carries `agentId` alone: `taskingDocument` is the
    // only other member the operation outputs (`smithy_query(query="operation",
    // operation="GetAgentConfiguration")`) and it is omitted while the service holds no
    // document, so the response carries no member beyond the one modelled value it has.
    let read = get_configuration(&router, &registered[0].1).await;
    let body: &Map<String, Value> = read
        .body
        .as_object()
        .unwrap_or_else(|| panic!("the read must answer a JSON object, got {}", read.body));
    let mut members: Vec<&str> = body.keys().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(
        members,
        vec!["agentId"],
        "the read of an untasked agent carries agentId alone, got {}",
        read.body
    );
}
