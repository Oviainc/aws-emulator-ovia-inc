// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateEventSourceMapping`: "Updates an event source mapping. You
//! can change the function that Lambda invokes, or pause invocation and resume later from
//! the same location."
//!
//! The rule under test: an update of a mapping this account holds replaces exactly the
//! members the request states, leaves every member it omits at the value the store holds,
//! and is ACCEPTED rather than applied — the operation answers 202 with the mapping in
//! `Updating`, and the move to the state its `Enabled` setting asks for is owed by the
//! clock. While a mapping is still settling (out of its create or out of an earlier
//! update), or while its deletion is still owed, it is not available to take another
//! change and a write to it is refused with the modelled `ResourceInUseException` — so two
//! concurrent updates of one mapping have exactly one winner. A UUID no mapping is held
//! under, and a `FunctionName` that names no function, are refused with the modelled
//! `ResourceNotFoundException` without changing anything.
//!
//! Every row drives real HTTP requests through the router — over the ONE mapping harness
//! this test binary has, [`crate::event_source_mappings`] — and reads the mapping back
//! through `GetEventSourceMapping`, so an update that answered 202 without persisting the
//! change fails here. The rows differ along what the rule generalises over: WHICH members
//! the update states (a batch size, a pause, a filter, a destination, a re-target onto
//! another function, and nothing at all), which event source the mapping polls, and the
//! settings its create stated.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ComAmazonawsLambdaAwsgirapiserviceState, EVENT_SOURCE_MAPPING_SETTLE_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_function, create_mapping, create_request, created_state_for, get_mapping,
    router_on, Answer, DYNAMODB_SOURCE, KINESIS_SOURCE, MAPPINGS_URI, QUEUE_SOURCE,
    STATE_CREATING,
};

/// The `State` a mapping an update was accepted for carries, from
/// `EventSourceMappingConfiguration$State`: "It can be one of the following: Creating,
/// Enabling, Enabled, Disabling, Disabled, Updating, or Deleting."
const STATE_UPDATING: &str = "Updating";

/// See [`STATE_UPDATING`].
const STATE_ENABLED: &str = "Enabled";

/// See [`STATE_UPDATING`].
const STATE_DISABLED: &str = "Disabled";

/// A UUID of the length `UUIDString` requires (`@length(min: 36, max: 36)`) that no mapping
/// is held under.
const UNHELD_UUID: &str = "11111111-2222-3333-4444-555555555555";

/// How long past a due instant this suite waits before calling. The transitions are owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf: it waits past the instant the
/// call made due.
const SLACK: Duration = Duration::from_millis(100);

/// `UpdateEventSourceMapping` on the mapping `uuid` names.
async fn update_mapping(router: &MultiServiceRouter, uuid: &str, body: &Value) -> Answer {
    call(router, "PUT", &format!("{MAPPINGS_URI}/{uuid}"), body).await
}

/// `DeleteEventSourceMapping` on the mapping `uuid` names.
async fn delete_mapping(router: &MultiServiceRouter, uuid: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &Value::Null,
    )
    .await
}

/// The stored `(batch size, function arn)` of the mapping `uuid` names.
///
/// The STATE is deliberately not among them: it is owed by the clock, so a read of the
/// raw record reports whatever the last API call materialised, which is not what an
/// unchanged mapping means. The state is asserted through the API, which materialises
/// first.
fn stored(engine: &Arc<engine::InMemoryEngine>, uuid: &str) -> Option<(i32, String)> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .event_source_mappings
        .get(uuid)
        .map(|mapping| (mapping.batch_size, mapping.function_arn.clone()))
}

/// One update row: what the mapping was created with, what the update states, and which
/// members must have changed afterwards.
struct UpdateCase {
    name: &'static str,
    function: &'static str,
    event_source: &'static str,
    create_extras: Value,
    update: Value,
    /// The members the mapping must report afterwards, each as `(member, value)`.
    expected: Vec<(&'static str, Value)>,
    /// The state the mapping settles into once the clock makes the move due.
    settles_into: &'static str,
}

/// The rows. Each states a different subset of the updatable members over a different
/// starting mapping, on a different event source.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            name: "the batch size alone, on a queue mapping",
            function: "esm-up-batch",
            event_source: QUEUE_SOURCE,
            create_extras: json!({ "BatchSize": 5 }),
            update: json!({ "BatchSize": 9 }),
            expected: vec![("BatchSize", json!(9))],
            settles_into: STATE_ENABLED,
        },
        UpdateCase {
            name: "a pause, which the mapping settles into",
            function: "esm-up-pause",
            event_source: QUEUE_SOURCE,
            create_extras: json!({ "BatchSize": 5 }),
            update: json!({ "Enabled": false }),
            expected: vec![("BatchSize", json!(5))],
            settles_into: STATE_DISABLED,
        },
        UpdateCase {
            name: "a resume of a mapping created paused",
            function: "esm-up-resume",
            event_source: QUEUE_SOURCE,
            create_extras: json!({ "Enabled": false, "BatchSize": 3 }),
            update: json!({ "Enabled": true }),
            expected: vec![("BatchSize", json!(3))],
            settles_into: STATE_ENABLED,
        },
        UpdateCase {
            name: "filter criteria and a batching window, on a kinesis mapping",
            function: "esm-up-filter",
            event_source: KINESIS_SOURCE,
            create_extras: json!({ "StartingPosition": "LATEST", "BatchSize": 100 }),
            update: json!({
                "FilterCriteria": { "Filters": [{ "Pattern": "{\"eventName\":[\"INSERT\"]}" }] },
                "MaximumBatchingWindowInSeconds": 30,
            }),
            expected: vec![
                ("MaximumBatchingWindowInSeconds", json!(30)),
                (
                    "FilterCriteria",
                    json!({ "Filters": [{ "Pattern": "{\"eventName\":[\"INSERT\"]}" }] }),
                ),
            ],
            settles_into: STATE_ENABLED,
        },
        UpdateCase {
            name: "the stream error-handling settings, on a dynamodb mapping",
            function: "esm-up-stream",
            event_source: DYNAMODB_SOURCE,
            create_extras: json!({ "StartingPosition": "TRIM_HORIZON" }),
            update: json!({
                "MaximumRetryAttempts": 4,
                "MaximumRecordAgeInSeconds": 3600,
                "BisectBatchOnFunctionError": true,
                "ParallelizationFactor": 3,
            }),
            expected: vec![
                ("MaximumRetryAttempts", json!(4)),
                ("MaximumRecordAgeInSeconds", json!(3600)),
                ("BisectBatchOnFunctionError", json!(true)),
                ("ParallelizationFactor", json!(3)),
            ],
            settles_into: STATE_ENABLED,
        },
        UpdateCase {
            name: "no updatable member at all, which only re-accepts the mapping",
            function: "esm-up-nothing",
            event_source: QUEUE_SOURCE,
            create_extras: json!({ "BatchSize": 7 }),
            update: json!({}),
            expected: vec![("BatchSize", json!(7))],
            settles_into: STATE_ENABLED,
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_members_it_states_and_settles_by_the_clock() {
    for case in update_cases() {
        let label = case.name;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, case.function).await;

        let created = create_mapping(
            &router,
            &create_request(case.function, Some(case.event_source), &case.create_extras),
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
        let arn = created
            .string("EventSourceMappingArn")
            .unwrap_or_else(|| panic!("case {label:?}: the create answers an ARN"));

        // The mapping is not available while it is still being created, which is the
        // model's own example of `ResourceInUseException`.
        // A mapping created with polling PAUSED was never in that window — it is created
        // in the terminal state its setting asks for — so there is no create window to be
        // refused inside for such a row.
        if created_state_for(&case.create_extras) == STATE_CREATING {
            let too_early = update_mapping(&router, &uuid, &case.update).await;
            assert_eq!(
                too_early.status,
                400,
                "case {label:?}: an update inside the create window is refused, got {} {}",
                too_early.status,
                too_early.text()
            );
            assert!(
                too_early.error_code().contains("ResourceInUseException"),
                "case {label:?}: expected ResourceInUseException, got {}",
                too_early.error_code()
            );
        } else {
            let settled = get_mapping(&router, &uuid).await;
            assert_eq!(
                (settled.status, settled.string("State")),
                (200, Some(STATE_DISABLED.to_string())),
                "case {label:?}: a mapping created paused is already {STATE_DISABLED}, got \
                 {} {}",
                settled.status,
                settled.text()
            );
        }

        tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;
        let updated = update_mapping(&router, &uuid, &case.update).await;
        assert_eq!(
            updated.status,
            202,
            "case {label:?}: expected the modelled 202 from UpdateEventSourceMapping, got {} {}",
            updated.status,
            updated.text()
        );
        assert_eq!(
            (
                updated.string("State"),
                updated.string("UUID"),
                updated.string("EventSourceMappingArn"),
                updated.string("EventSourceArn"),
            ),
            (
                Some(STATE_UPDATING.to_string()),
                Some(uuid.clone()),
                Some(arn.clone()),
                Some(case.event_source.to_string()),
            ),
            "case {label:?}: the update must answer the stored mapping as {STATE_UPDATING}, \
             got {}",
            updated.text()
        );
        for (member, value) in &case.expected {
            assert_eq!(
                updated.json().get(*member),
                Some(value),
                "case {label:?}: expected {member} to report {value}, got {}",
                updated.text()
            );
        }

        // Read back: the update persisted what it answered, still `Updating` until the
        // clock makes the move due.
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (reading.status, reading.string("State")),
            (200, Some(STATE_UPDATING.to_string())),
            "case {label:?}: the mapping must read back as {STATE_UPDATING} before the \
             settle is due, got {} {}",
            reading.status,
            reading.text()
        );
        for (member, value) in &case.expected {
            assert_eq!(
                reading.json().get(*member),
                Some(value),
                "case {label:?}: GetEventSourceMapping must report the stored {member}, got {}",
                reading.text()
            );
        }

        // And once it is due, the mapping is in the state its `Enabled` setting asks for.
        tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;
        let settled = get_mapping(&router, &uuid).await;
        assert_eq!(
            (settled.status, settled.string("State")),
            (200, Some(case.settles_into.to_string())),
            "case {label:?}: the mapping must settle into {}, got {} {}",
            case.settles_into,
            settled.status,
            settled.text()
        );
    }
}

#[tokio::test]
async fn of_two_concurrent_updates_of_one_mapping_exactly_one_wins() {
    // Each row races two updates of one mapping. They differ in what the two writers ask
    // for, so the winner is observable in the stored mapping rather than only in the
    // status codes.
    for (label, function, first, second) in [
        (
            "two writers setting different batch sizes",
            "esm-race-batch",
            json!({ "BatchSize": 2 }),
            json!({ "BatchSize": 8 }),
        ),
        (
            "one writer pausing, one re-batching",
            "esm-race-mixed",
            json!({ "Enabled": false }),
            json!({ "BatchSize": 6 }),
        ),
    ] {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, function).await;
        let created = create_mapping(
            &router,
            &create_request(function, Some(QUEUE_SOURCE), &json!({ "BatchSize": 4 })),
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
        tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;

        let mut answers = Vec::new();
        for body in [&first, &second] {
            answers.push(update_mapping(&router, &uuid, body).await);
        }
        let statuses: Vec<u16> = answers.iter().map(|answer| answer.status).collect();
        let winners = answers.iter().filter(|answer| answer.status == 202).count();
        assert_eq!(
            winners, 1,
            "case {label:?}: expected exactly one winner among the racers, got statuses \
             {statuses:?}"
        );

        let loser = answers
            .iter()
            .find(|answer| answer.status != 202)
            .unwrap_or_else(|| panic!("case {label:?}: one racer must be refused"));
        assert!(
            loser.error_code().contains("ResourceInUseException"),
            "case {label:?}: the loser is refused as the resource being unavailable, got {}",
            loser.error_code()
        );
        assert_eq!(
            loser
                .string("message")
                .or_else(|| loser.string("Message"))
                .as_deref(),
            Some("Cannot update the event source mapping because it is in use."),
            "case {label:?}: the refusal is the fixed in-use sentence, got {:?}",
            loser.text()
        );

        // The stored mapping is the WINNER's.
        let winner = answers
            .iter()
            .find(|answer| answer.status == 202)
            .unwrap_or_else(|| panic!("case {label:?}: one racer must succeed"));
        let held = stored(&engine, &uuid)
            .unwrap_or_else(|| panic!("case {label:?}: the mapping must still be held"));
        let winner_batch_size = i32::try_from(
            winner
                .json()
                .get("BatchSize")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        )
        .unwrap_or_default();
        assert_eq!(
            held.0, winner_batch_size,
            "case {label:?}: the stored mapping must be the winner's, got {held:?}"
        );
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            reading.string("State"),
            Some(STATE_UPDATING.to_string()),
            "case {label:?}: the winner's change is still being applied, got {}",
            reading.text()
        );
    }
}

#[tokio::test]
async fn an_update_of_a_mapping_that_cannot_take_one_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "esm-up-refusals").await;
    create_function(&router, "esm-up-target").await;
    let created = create_mapping(
        &router,
        &create_request(
            "esm-up-refusals",
            Some(QUEUE_SOURCE),
            &json!({ "BatchSize": 4 }),
        ),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "the mapping under test must be created, got {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("the create answers a UUID"));
    tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;
    let before = stored(&engine, &uuid).expect("the mapping is held");

    // A UUID nothing is held under, and a re-target onto a function that does not exist.
    // A mapping is addressed by its own identifier, so the refusal for a UUID that names
    // none is the fixed `ResourceNotFoundException` sentence rather than the resource
    // named back; a `FunctionName` the caller chose IS named back.
    for (label, target_uuid, body, code, expected_message) in [
        (
            "a uuid no mapping is held under",
            UNHELD_UUID.to_string(),
            json!({ "BatchSize": 2 }),
            "ResourceNotFoundException",
            "The resource you requested does not exist.".to_string(),
        ),
        (
            "a re-target onto a function that does not exist",
            uuid.clone(),
            json!({ "FunctionName": "esm-up-absent" }),
            "ResourceNotFoundException",
            "Function not found: arn:aws:lambda:us-east-1:000000000000:function:esm-up-absent"
                .to_string(),
        ),
    ] {
        let answer = update_mapping(&router, &target_uuid, &body).await;
        assert_eq!(
            answer.status,
            404,
            "case {label:?}: expected 404 from UpdateEventSourceMapping, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(code),
            "case {label:?}: expected {code}, got {}",
            answer.error_code()
        );
        assert_eq!(
            answer
                .string("message")
                .or_else(|| answer.string("Message")),
            Some(expected_message.clone()),
            "case {label:?}: expected the message {expected_message:?}, got {}",
            answer.text()
        );
    }
    assert_eq!(
        stored(&engine, &uuid),
        Some(before.clone()),
        "a refused update must leave the mapping exactly as it was, got {:?}",
        stored(&engine, &uuid)
    );

    // A re-target that DOES name a function moves the mapping onto it.
    let retargeted = update_mapping(
        &router,
        &uuid,
        &json!({ "FunctionName": "esm-up-target" }),
    )
    .await;
    assert_eq!(
        retargeted.status,
        202,
        "a re-target onto a function that exists must be accepted, got {} {}",
        retargeted.status,
        retargeted.text()
    );
    let after = stored(&engine, &uuid).expect("the mapping is held");
    assert_eq!(
        after.1, "arn:aws:lambda:us-east-1:000000000000:function:esm-up-target",
        "the mapping must report the function it was re-targeted onto, got {after:?}"
    );

    // And a mapping whose deletion is owed is not available to take a change either.
    tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;
    let deleted = delete_mapping(&router, &uuid).await;
    assert_eq!(
        deleted.status,
        202,
        "the delete must be accepted, got {} {}",
        deleted.status,
        deleted.text()
    );
    let while_deleting = update_mapping(&router, &uuid, &json!({ "BatchSize": 3 })).await;
    assert_eq!(
        while_deleting.status,
        400,
        "an update of a mapping being deleted is refused, got {} {}",
        while_deleting.status,
        while_deleting.text()
    );
    assert!(
        while_deleting
            .error_code()
            .contains("ResourceInUseException"),
        "expected ResourceInUseException, got {}",
        while_deleting.error_code()
    );
}
