// SPDX-License-Identifier: CC0-1.0

//! The platform tagging rules as `RegisterAgent` enforces them, end to end over HTTP
//! through the same front door as the rest of this target's suites (the harness, signed
//! `restJson1` requests and the response reader come from [`crate::scheduling_loop`]).
//!
//! `RegisterAgent` takes a `tags` map (`smithy_query(query="operation",
//! operation="RegisterAgent")`) and is a create, so it is held to the one rule every
//! operation of this service that accepts a tag map is held to: a request whose `tags`
//! map breaks the AWS tagging rules is refused with the modelled
//! `InvalidParameterException` (HTTP 431) naming the `tags` member and registers NOTHING,
//! while a map inside every bound is registered and reads back verbatim through the
//! resource's tag read.
//!
//! The rule is a property of the TAG MAP, not of the resource, so the rows are the very
//! tables [`crate::tag_rules`] asserts over the other creates of this service — a
//! reserved `aws:` key in each spelling, keys and values at and past their published
//! bounds, counted in characters rather than bytes, and tag counts at and past the
//! per-resource ceiling — rather than a second copy of them. The bounds themselves are
//! the constants in `state`, each citing its source.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{service_state, TAGS_PARAMETER};

use crate::agent_lifecycle::{capability_arn, get_configuration, register};
use crate::resource_tagging::{agent_arn, list_tags, reported_tags};
use crate::scheduling_loop::{harness_with_engine, is_uuid};
use crate::tag_rules::{illegal_tag_maps, legal_tag_maps};

/// A `RegisterAgent` request carrying `tags` when the row names a map, with the required
/// members of `AgentDetails` and `DiscoveryData` and the cpu-core member the service
/// requires one of (`smithy_query(query="shape", shape="AgentDetails,DiscoveryData")`).
/// `row` distinguishes one registration's EC2 instance from the next, so no two rows of a
/// table register the same instance.
fn registration(row: usize, tags: Option<&Value>) -> Value {
    let mut body = json!({
        "agentDetails": {
            "agentVersion": "2.0.1",
            "instanceId": format!("i-{row:016x}"),
            "instanceType": "c5.xlarge",
            "agentCpuCores": [0, 1],
            "componentVersions": [{"componentType": "Agent", "versions": ["2.0.1"]}],
        },
        "discoveryData": {
            "publicIpAddresses": [format!("52.0.{}.{}", row / 256, row % 256)],
            "privateIpAddresses": [format!("10.0.{}.{}", row / 256, row % 256)],
            "capabilityArns": [capability_arn(row)],
        },
    });
    if let Some(tags) = tags {
        body["tags"] = tags.clone();
    }
    body
}

/// How many agents the store holds.
///
/// Read from the store rather than through an operation, because what has to be
/// observable for a refused registration is that it wrote NOTHING — and this model
/// declares no listing of agents at all.
fn stored_agents(engine: &engine::InMemoryEngine) -> usize {
    let state = service_state(engine).expect("this service's state store");
    let guard = state.lock();
    guard.agents.len()
}

/// A request's tag map as a comparable map, so what a tag read reports is compared to
/// what the registration carried whatever order either serialized in.
fn expected_tags(case: &str, tags: &Value) -> BTreeMap<String, String> {
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case}: the row's tag map must be a map, got {tags}"))
        .iter()
        .map(|(key, value)| {
            let text = value
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: tag {key} must be a string, got {value}"))
                .to_string();
            (key.clone(), text)
        })
        .collect()
}

#[tokio::test]
async fn an_illegal_tag_map_is_refused_by_register_agent_and_registers_nothing() {
    let (router, engine) = harness_with_engine();

    for (row, (label, tags)) in illegal_tag_maps().into_iter().enumerate() {
        let case = format!("RegisterAgent / {label}");
        let before = stored_agents(&engine);
        let reply = register(&router, registration(row, Some(&tags))).await;

        assert_eq!(
            reply.status, 431,
            "case {case:?}: expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {}",
            reply.body
        );
        assert_eq!(
            reply.field("parameterName"),
            &json!(TAGS_PARAMETER),
            "case {case:?}: the refusal must name the tags member, got {}",
            reply.body
        );
        assert!(
            reply
                .body
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal must carry a message, got {}",
            reply.body
        );
        assert!(
            reply.body.get("agentId").is_none(),
            "case {case:?}: a refused registration must mint no agentId, got {}",
            reply.body
        );
        assert_eq!(
            stored_agents(&engine),
            before,
            "case {case:?}: a refused registration must register no agent"
        );
    }
}

#[tokio::test]
async fn a_legal_tag_map_is_registered_and_reads_back_verbatim() {
    let (router, engine) = harness_with_engine();

    for (row, (label, tags)) in legal_tag_maps().into_iter().enumerate() {
        let case = format!("RegisterAgent / {label}");
        let before = stored_agents(&engine);
        let registered = register(&router, registration(row, tags.as_ref())).await;

        assert_eq!(
            registered.status, 200,
            "case {case:?}: expected 200, got {} {}",
            registered.status, registered.body
        );
        let agent_id = registered.string("agentId");
        assert!(
            is_uuid(&agent_id),
            "case {case:?}: the minted agentId must be a lowercase uuid, got {agent_id:?}"
        );
        assert_eq!(
            stored_agents(&engine),
            before + 1,
            "case {case:?}: an accepted registration must register the agent"
        );

        // The round trip through the `Agent` resource's own read: the registration is
        // only a fix if the agent the id names is there afterwards.
        let read = get_configuration(&router, &agent_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetAgentConfiguration 200, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("agentId"),
            agent_id,
            "case {case:?}: the read must answer the agent the registration minted, got {}",
            read.body
        );

        // A resource carrying no tag reads back as the empty map, never an omitted
        // member (`state::response_tags`), so an omitted `tags` and an empty one read
        // back alike.
        let tag_read = list_tags(&router, &agent_arn(&agent_id)).await;
        assert_eq!(
            tag_read.status, 200,
            "case {case:?}: expected ListTagsForResource 200, got {} {}",
            tag_read.status, tag_read.body
        );
        let expected = expected_tags(&case, &tags.unwrap_or_else(|| Value::Object(Map::new())));
        let reported = reported_tags(&case, &tag_read);
        assert_eq!(
            reported, expected,
            "case {case:?}: the agent must carry the tags it was registered with, \
             got {reported:?}"
        );
    }
}
