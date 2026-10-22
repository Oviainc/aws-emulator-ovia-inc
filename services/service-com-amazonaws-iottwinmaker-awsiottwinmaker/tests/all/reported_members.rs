// SPDX-License-Identifier: CC0-1.0

//! What a read REPORTS for a stored resource, member by member.
//!
//! Two rules, over tables of inputs rather than one recorded case:
//!
//! * every `Status` this service reports carries BOTH of its modelled members —
//!   a `state` from the model's `State` enum, and an `error` structure that is
//!   present (and empty) while nothing has failed, never an absent member;
//! * a resource's `description` is a member it always carries: a read reports
//!   the string the create named, or the empty string when the create named
//!   none.
//!
//! Every read here also asserts the state the create SETTLED the entity in, so
//! a status is checked whole rather than one member at a time.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::STATE_VALUES;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::STATE_ACTIVE;

use crate::harness::{encode_label, workspace_arn, Emulator, Resp};

/// How many entities one `ListEntities` call in this suite asks for: more than
/// any table here creates, so a lookup in the page is a lookup in the listing.
const MANY_ENTITIES: u32 = 50;

/// The `description` a read must report for a create that carried `supplied`.
fn expected_description(supplied: Option<&str>) -> Value {
    json!(supplied.unwrap_or(""))
}

/// Assert `status` is a whole `Status`: a modelled `state`, and an `error`
/// structure present but empty because nothing has failed.
fn assert_whole_status(case: &str, where_: &str, status: &Value) {
    let state = status["state"].as_str();
    assert!(
        state.is_some_and(|state| STATE_VALUES.contains(&state)),
        "case {case:?} ({where_}): status.state must be one of the modelled State values \
         {STATE_VALUES:?}, got {status:?}"
    );
    assert_eq!(
        state,
        Some(STATE_ACTIVE),
        "case {case:?} ({where_}): a stored resource reports the state it settled in, got \
         {status:?}"
    );
    assert_eq!(
        status.get("error"),
        Some(&json!({})),
        "case {case:?} ({where_}): status.error must be a present, empty ErrorDetails, got \
         {status:?}"
    );
}

/// Every `Status` in a `GetEntity` body: the entity's own and each component's.
fn statuses_of(read: &Resp) -> Vec<(String, Value)> {
    let mut statuses = vec![("entity".to_string(), read.member("status").clone())];
    if let Some(components) = read.member("components").as_object() {
        for (name, component) in components {
            statuses.push((format!("component {name}"), component["status"].clone()));
        }
    }
    statuses
}

/// RULE: every status this service reports carries both modelled members — a
/// `State` value and a present (empty, while nothing failed) `error` structure.
///
/// The rows differ along how many components the entity carries, because each
/// component reports a status of its own beside the entity's.
#[tokio::test]
async fn every_reported_status_carries_a_state_and_an_error_member() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("ws").await.status, 200);

    let rows: Vec<(&str, usize)> = vec![
        ("noComponents", 0),
        ("oneComponent", 1),
        ("threeComponents", 3),
    ];

    for (case, components) in &rows {
        let mut request = json!({"entityId": case, "entityName": case});
        if *components > 0 {
            let mut map = serde_json::Map::new();
            for index in 0..*components {
                map.insert(
                    format!("c{index}"),
                    json!({"componentTypeId": format!("com.example.c{index}")}),
                );
            }
            request["components"] = Value::Object(map);
        }
        let created = emulator.create_entity("ws", request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateEntity, got {created:?}"
        );

        let read = emulator.get_entity("ws", case).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetEntity, got {read:?}"
        );
        for (where_, status) in statuses_of(&read) {
            assert_whole_status(case, &where_, &status);
        }
        let listed = emulator
            .list_entities("ws", json!({"maxResults": MANY_ENTITIES}))
            .await;
        for summary in listed
            .member("entitySummaries")
            .as_array()
            .unwrap_or(&Vec::new())
        {
            assert_whole_status(case, "ListEntities summary", &summary["status"]);
        }
    }
}

/// RULE: an entity's `description` is reported by every read of it — the string
/// the create named, or the empty string when the create named none — and never
/// as an absent member.
///
/// The rows span the `Description` member's modelled `@length 0..2048`
/// (`smithy_query(query="shape", shape="Description")`): no description at all,
/// both published bounds, one past the upper bound, and a multi-byte value whose
/// length is counted in characters rather than bytes.
#[tokio::test]
async fn an_entity_description_is_reported_present_even_when_empty() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("ws").await.status, 200);

    let at_max = "d".repeat(2048);
    let past_max = "d".repeat(2049);
    let rows: Vec<(&str, Option<&str>)> = vec![
        ("absent", None),
        ("empty", Some("")),
        ("oneChar", Some("x")),
        ("plain", Some("a pump on line 3")),
        ("multiByte", Some("Kühlpumpe — 冷却ポンプ")),
        ("atMaxLength", Some(at_max.as_str())),
    ];

    for (case, description) in &rows {
        let mut request = json!({"entityId": case, "entityName": case});
        if let Some(description) = description {
            request["description"] = json!(description);
        }
        let created = emulator.create_entity("ws", request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateEntity, got {created:?}"
        );

        let read = emulator.get_entity("ws", case).await;
        assert_eq!(
            read.body.get("description").cloned(),
            Some(expected_description(*description)),
            "case {case:?}: GetEntity must report the description, got {:?}",
            read.body.get("description")
        );

        let listed = emulator
            .list_entities("ws", json!({"maxResults": MANY_ENTITIES}))
            .await;
        let summary = listed
            .member("entitySummaries")
            .as_array()
            .and_then(|summaries| {
                summaries
                    .iter()
                    .find(|summary| summary["entityId"].as_str() == Some(*case))
                    .cloned()
            });
        let summary = summary.unwrap_or_else(|| panic!("case {case:?}: not listed: {listed:?}"));
        assert_eq!(
            summary.get("description").cloned(),
            Some(expected_description(*description)),
            "case {case:?}: the ListEntities summary must report the description, got {summary:?}"
        );
    }

    // One past the published upper bound is refused by the modelled constraint,
    // so nothing is stored for it.
    let refused = emulator
        .create_entity(
            "ws",
            json!({"entityId": "pastMax", "entityName": "pastMax", "description": past_max}),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "a description of 2049 characters is past the modelled maximum of 2048 and must be \
         refused, got {refused:?}"
    );
}

/// RULE: a workspace's `description` is reported the same way an entity's is —
/// present on every read, the empty string when the create named none — whether
/// the workspace is read by id or by the ARN the create minted.
#[tokio::test]
async fn a_workspace_description_is_reported_present_even_when_empty() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, Option<&str>)> = vec![
        ("bare", None),
        ("emptyDescription", Some("")),
        ("described", Some("the plant floor")),
    ];

    for (workspace_id, description) in &rows {
        let mut request = json!({});
        if let Some(description) = description {
            request["description"] = json!(description);
        }
        let created = emulator
            .call("POST", &format!("/workspaces/{workspace_id}"), Some(request))
            .await;
        assert_eq!(
            created.status, 200,
            "case {workspace_id:?}: expected 200 from CreateWorkspace, got {created:?}"
        );

        for lookup in [
            workspace_id.to_string(),
            encode_label(&workspace_arn(workspace_id)),
        ] {
            let read = emulator.get_workspace(&lookup).await;
            assert_eq!(
                read.body.get("description").cloned(),
                Some(expected_description(*description)),
                "case {workspace_id:?} read by {lookup:?}: GetWorkspace must report the \
                 description, got {read:?}"
            );
        }
    }
}
