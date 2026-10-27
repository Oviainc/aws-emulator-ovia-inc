// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateEventSourceMapping`: "Updates an event source mapping. You
//! can change the function that Lambda invokes, or pause invocation and resume later from
//! the same location."
//!
//! The rule under test, from the operation's own input: an update addresses the ONE mapping
//! its `UUID` names and states a SUBSET of that mapping's members, so every member the
//! request states replaces the stored value, every member it does not state keeps the value
//! the store already holds, and no other mapping is touched at all — the mapping is read
//! out of the store rather than rebuilt from the request, which is why the whole reported
//! configuration is compared member by member here and not only the members the update
//! named.
//!
//! What [`crate::event_source_mapping_update`] pins is the members an update DOES change
//! and the window in which a mapping is unavailable to take one; this suite pins the
//! complement: that the update leaves everything else exactly as it was, in the update's own
//! answer, in `GetEventSourceMapping`, in `ListEventSourceMappings` and in the mapping the
//! update did not address. The lifecycle members are excluded from that comparison because
//! the operation is defined to move them: `State` becomes `Updating` (the change is accepted
//! rather than applied — the operation answers 202), `StateTransitionReason` records who
//! asked, and `LastModified` is "The date that the event source mapping was last updated or
//! that its state changed".
//!
//! Every row drives real HTTP requests through the router over the ONE mapping harness this
//! test binary has, [`crate::event_source_mappings`]. The rows differ along what the rule
//! generalises over: HOW MANY members the update states (none, one, two, five), of which
//! TYPE (integer, boolean, list, structure, a function name that re-targets the mapping),
//! which event source the mapping polls, and what its create stated.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::EVENT_SOURCE_MAPPING_SETTLE_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_function, create_mapping, create_request, get_mapping, list_mappings, router_on,
    Answer, DYNAMODB_SOURCE, KINESIS_SOURCE, MAPPINGS_URI, QUEUE_SOURCE, USER_INITIATED,
};

/// The `State` a mapping an update was accepted for carries, and the two it settles into,
/// from `EventSourceMappingConfiguration$State`: "It can be one of the following: Creating,
/// Enabling, Enabled, Disabling, Disabled, Updating, or Deleting."
const STATE_UPDATING: &str = "Updating";
/// See [`STATE_UPDATING`].
const STATE_ENABLED: &str = "Enabled";
/// See [`STATE_UPDATING`].
const STATE_DISABLED: &str = "Disabled";

/// The members `UpdateEventSourceMapping` is defined to move on every call, which is why
/// they are not part of the "unchanged" comparison: see the module docs.
const LIFECYCLE_MEMBERS: [&str; 3] = ["State", "StateTransitionReason", "LastModified"];

/// The function a row re-targets its mapping onto, created alongside the one the mapping is
/// created on.
const RETARGET_FUNCTION: &str = "esm-omitted-retarget";

/// The region and account every ARN this emulator mints carries.
const REGION: &str = "us-east-1";
/// See [`REGION`].
const ACCOUNT: &str = "000000000000";

/// A well-formed `OnFailure` destination, satisfying the model's `DestinationArn` pattern.
const FAILURE_DESTINATION: &str = "arn:aws:sqs:us-east-1:000000000000:order-dlq";

/// How long past a due instant this suite waits before calling. The transitions are owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf: it waits past the instant the
/// call made due.
const SLACK: Duration = Duration::from_millis(100);

/// `UpdateEventSourceMapping` on the mapping `uuid` names
/// (`PUT /2015-03-31/event-source-mappings/{UUID}`).
async fn update_mapping(router: &MultiServiceRouter, uuid: &str, body: &Value) -> Answer {
    call(router, "PUT", &format!("{MAPPINGS_URI}/{uuid}"), body).await
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Create a second mapping, on another event source, that no row's update names: what
/// proves an update reaches exactly the one mapping its `UUID` addresses. Answers its UUID.
async fn create_bystander(router: &MultiServiceRouter, function: &str, source: &str) -> String {
    // "Required for Amazon Kinesis and Amazon DynamoDB Stream event sources", so a shard
    // source states one and a queue states none.
    let extras = if source == QUEUE_SOURCE {
        json!({})
    } else {
        json!({ "StartingPosition": "LATEST" })
    };
    let created = create_mapping(router, &create_request(function, Some(source), &extras)).await;
    assert_eq!(
        created.status,
        202,
        "the bystander mapping on {source} must be created, got {} {}",
        created.status,
        created.text()
    );
    created
        .string("UUID")
        .unwrap_or_else(|| panic!("the bystander create answers a UUID, got {}", created.text()))
}

/// One update row: what the mapping was created with, what the update states, and which
/// reported members that update is allowed to have changed.
struct Row {
    name: &'static str,
    /// The function the mapping under test is created on.
    function: &'static str,
    event_source: &'static str,
    create_extras: Value,
    update: Value,
    /// The members the update must have changed, each as `(reported member, value)`.
    /// EVERY other member of the configuration must come back unchanged.
    changed: Vec<(&'static str, Value)>,
    /// The state the mapping settles into once the clock makes the move due.
    settles_into: &'static str,
}

/// The rows. The integer values are each member's published maximum, from
/// `smithy_query(query="operation", operation="UpdateEventSourceMapping")`: `BatchSize`
/// `@range(min: 1, max: 10000)`, `MaximumBatchingWindowInSeconds` `@range(min: 0, max: 300)`,
/// `ParallelizationFactor` `@range(min: 1, max: 10)`, `TumblingWindowInSeconds`
/// `@range(min: 0, max: 900)`, `MaximumRecordAgeInSeconds` `@range(min: -1, max: 604800)` and
/// `MaximumRetryAttempts` `@range(min: -1, max: 10000)`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "one integer member, at the published BatchSize maximum",
            function: "esm-omitted-batch",
            event_source: QUEUE_SOURCE,
            create_extras: json!({ "BatchSize": 5, "ScalingConfig": { "MaximumConcurrency": 2 } }),
            update: json!({ "BatchSize": 10_000 }),
            changed: vec![("BatchSize", json!(10_000))],
            settles_into: STATE_ENABLED,
        },
        Row {
            name: "one boolean member, which moves no reported member but the state it \
                   settles into",
            function: "esm-omitted-pause",
            event_source: KINESIS_SOURCE,
            create_extras: json!({ "StartingPosition": "LATEST", "BatchSize": 50 }),
            update: json!({ "Enabled": false }),
            changed: vec![],
            settles_into: STATE_DISABLED,
        },
        Row {
            name: "five integer members at once, each at its published maximum",
            function: "esm-omitted-bounds",
            event_source: DYNAMODB_SOURCE,
            create_extras: json!({ "StartingPosition": "TRIM_HORIZON" }),
            update: json!({
                "MaximumBatchingWindowInSeconds": 300,
                "ParallelizationFactor": 10,
                "TumblingWindowInSeconds": 900,
                "MaximumRecordAgeInSeconds": 604_800,
                "MaximumRetryAttempts": 10_000,
            }),
            changed: vec![
                ("MaximumBatchingWindowInSeconds", json!(300)),
                ("ParallelizationFactor", json!(10)),
                ("TumblingWindowInSeconds", json!(900)),
                ("MaximumRecordAgeInSeconds", json!(604_800)),
                ("MaximumRetryAttempts", json!(10_000)),
            ],
            settles_into: STATE_ENABLED,
        },
        Row {
            name: "two structure members, on a mapping created with neither",
            function: "esm-omitted-structures",
            event_source: QUEUE_SOURCE,
            create_extras: json!({ "BatchSize": 4 }),
            update: json!({
                "DestinationConfig": { "OnFailure": { "Destination": FAILURE_DESTINATION } },
                "FilterCriteria": { "Filters": [{ "Pattern": "{\"body\":{\"kind\":[\"order\"]}}" }] },
            }),
            changed: vec![
                (
                    "DestinationConfig",
                    json!({ "OnFailure": { "Destination": FAILURE_DESTINATION } }),
                ),
                (
                    "FilterCriteria",
                    json!({ "Filters": [{ "Pattern": "{\"body\":{\"kind\":[\"order\"]}}" }] }),
                ),
            ],
            settles_into: STATE_ENABLED,
        },
        Row {
            name: "a list member and a metrics configuration, on a mapping created with a \
                   scaling one",
            function: "esm-omitted-lists",
            event_source: QUEUE_SOURCE,
            create_extras: json!({ "ScalingConfig": { "MaximumConcurrency": 3 } }),
            update: json!({
                "FunctionResponseTypes": ["ReportBatchItemFailures"],
                "MetricsConfig": { "Metrics": ["EventCount"] },
            }),
            changed: vec![
                ("FunctionResponseTypes", json!(["ReportBatchItemFailures"])),
                ("MetricsConfig", json!({ "Metrics": ["EventCount"] })),
            ],
            settles_into: STATE_ENABLED,
        },
        Row {
            name: "a re-target onto another function, which moves the function arn alone",
            function: "esm-omitted-retargeted",
            event_source: QUEUE_SOURCE,
            create_extras: json!({ "BatchSize": 6 }),
            update: json!({ "FunctionName": RETARGET_FUNCTION }),
            changed: vec![("FunctionArn", json!(function_arn(RETARGET_FUNCTION)))],
            settles_into: STATE_ENABLED,
        },
        Row {
            name: "no updatable member at all, which may move nothing but the lifecycle",
            function: "esm-omitted-nothing",
            event_source: KINESIS_SOURCE,
            create_extras: json!({ "StartingPosition": "TRIM_HORIZON", "BatchSize": 7 }),
            update: json!({}),
            changed: vec![],
            settles_into: STATE_ENABLED,
        },
    ]
}

/// Assert `after` carries every member of `before` unchanged, apart from the lifecycle
/// members and the ones `changed` names.
fn assert_rest_unchanged(case: &str, reader: &str, before: &Value, after: &Value, changed: &[&str]) {
    let mut members: BTreeSet<&String> = BTreeSet::new();
    for document in [before, after] {
        if let Some(object) = document.as_object() {
            members.extend(object.keys());
        }
    }
    for member in members {
        if LIFECYCLE_MEMBERS.contains(&member.as_str()) || changed.contains(&member.as_str()) {
            continue;
        }
        let was = before.get(member).cloned().unwrap_or(Value::Null);
        let now = after.get(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            was, now,
            "case {case:?}: an update that does not state {member} must leave it at {was}, \
             but {reader} reports {now}"
        );
    }
}

#[tokio::test]
async fn an_update_moves_the_members_it_states_and_leaves_every_other_one_alone() {
    for row in rows() {
        let label = row.name;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, row.function).await;
        create_function(&router, RETARGET_FUNCTION).await;

        let created = create_mapping(
            &router,
            &create_request(row.function, Some(row.event_source), &row.create_extras),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {label:?}: the mapping under test must be created, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {label:?}: the create answers a UUID"));

        // A second mapping, on another event source, that this row's update never names.
        let bystander_source = if row.event_source == QUEUE_SOURCE {
            DYNAMODB_SOURCE
        } else {
            QUEUE_SOURCE
        };
        let bystander = create_bystander(&router, row.function, bystander_source).await;

        // Both mappings are read AFTER their create has settled, so the baseline is the
        // state the clock owed rather than a window still closing under the comparison.
        tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;
        let before = get_mapping(&router, &uuid).await;
        assert_eq!(
            (before.status, before.string("State")),
            (200, Some(STATE_ENABLED.to_string())),
            "case {label:?}: the baseline mapping must read back settled, got {} {}",
            before.status,
            before.text()
        );
        let bystander_before = get_mapping(&router, &bystander).await;
        assert_eq!(
            bystander_before.status,
            200,
            "case {label:?}: the bystander mapping must read back, got {} {}",
            bystander_before.status,
            bystander_before.text()
        );

        let answer = update_mapping(&router, &uuid, &row.update).await;
        assert_eq!(
            answer.status,
            202,
            "case {label:?}: expected the modelled 202 from UpdateEventSourceMapping, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            (answer.string("State"), answer.string("StateTransitionReason")),
            (
                Some(STATE_UPDATING.to_string()),
                Some(USER_INITIATED.to_string())
            ),
            "case {label:?}: the update must answer the mapping as {STATE_UPDATING}, asked for \
             by a caller, got {}",
            answer.text()
        );

        let changed: Vec<&str> = row.changed.iter().map(|(member, _)| *member).collect();
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            reading.status,
            200,
            "case {label:?}: the updated mapping must read back, got {} {}",
            reading.status,
            reading.text()
        );
        // The update's own answer and the read-back are both the STORED record, so each has
        // to show the stated members changed and every other member untouched.
        for (reader, document) in [
            ("the update's own answer", answer.json()),
            ("GetEventSourceMapping", reading.json()),
        ] {
            for (member, value) in &row.changed {
                assert_eq!(
                    document.get(*member),
                    Some(value),
                    "case {label:?}: {reader} must report the stated {member} as {value}, got {document}"
                );
            }
            assert_rest_unchanged(label, reader, &before.json(), &document, &changed);
        }

        // "The date that the event source mapping was last updated or that its state
        // changed": both happened, so the stored instant cannot be older than the baseline's.
        let modified_before = before
            .json()
            .get("LastModified")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let modified_after = reading
            .json()
            .get("LastModified")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        assert!(
            modified_after >= modified_before,
            "case {label:?}: LastModified must not go backwards over an update: was \
             {modified_before}, got {modified_after}"
        );

        // The listing reads the same record, so it reports the same change.
        let listed = list_mappings(&router, "").await;
        assert_eq!(
            listed.status,
            200,
            "case {label:?}: the mappings must list, got {} {}",
            listed.status,
            listed.text()
        );
        let entry = listed
            .mappings()
            .into_iter()
            .find(|mapping| mapping.get("UUID").and_then(Value::as_str) == Some(uuid.as_str()))
            .unwrap_or_else(|| {
                panic!("case {label:?}: the updated mapping must be listed, got {}", listed.text())
            });
        for (member, value) in &row.changed {
            assert_eq!(
                entry.get(*member),
                Some(value),
                "case {label:?}: ListEventSourceMappings must report the stated {member} as \
                 {value}, got {entry}"
            );
        }

        // And the mapping the update did NOT address is reported exactly as it was, whole.
        let bystander_after = get_mapping(&router, &bystander).await;
        assert_eq!(
            bystander_before.json(),
            bystander_after.json(),
            "case {label:?}: an update of one mapping must leave the mapping it does not \
             address exactly as it was, got {}",
            bystander_after.text()
        );

        // Once the clock makes the move out of `Updating` due, the mapping is in the state
        // its `Enabled` setting asks for and still carries what the update stated.
        tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;
        let settled = get_mapping(&router, &uuid).await;
        assert_eq!(
            (settled.status, settled.string("State")),
            (200, Some(row.settles_into.to_string())),
            "case {label:?}: the mapping must settle into {}, got {} {}",
            row.settles_into,
            settled.status,
            settled.text()
        );
        for (member, value) in &row.changed {
            assert_eq!(
                settled.json().get(*member),
                Some(value),
                "case {label:?}: the settled mapping must still carry {member} as {value}, got {}",
                settled.text()
            );
        }
    }
}
