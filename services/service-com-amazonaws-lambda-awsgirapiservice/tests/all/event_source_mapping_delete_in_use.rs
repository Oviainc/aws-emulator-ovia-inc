// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one state a `DeleteEventSourceMapping` is REFUSED in.
//!
//! The rule under test: a mapping that is still applying a change it already acknowledged
//! is not available to be deleted. `ResourceInUseException` names that window itself — "The
//! operation conflicts with the resource's availability. For example, you tried to update
//! an event source mapping in the CREATING state, or you tried to delete an event source
//! mapping currently UPDATING" — so while `State=Updating` the delete is refused with that
//! modelled error (HTTP 400) and the fixed sentence `Cannot delete the event source
//! mapping because it is in use.`, the mapping stays readable, and the IDENTICAL delete is
//! accepted once the clock has closed the window. A mapping nobody has updated is
//! deletable throughout, which is the positive control every row carries.
//!
//! The rows differ along what the rule generalises over: the event source the mapping
//! polls (a queue, a Kinesis stream, a DynamoDB stream), the member the update stated (the
//! batch size, a pause, filter criteria) and the state the mapping settles into. The
//! refusal is the same for all of them, because the rule is about the STATE and not about
//! what the update changed.
//!
//! The settle is owed by the CLOCK, so nothing here sleeps on the emulator's behalf: it
//! waits past the instant the update made due.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::EVENT_SOURCE_MAPPING_SETTLE_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_function, create_mapping, create_request, get_mapping, router_on, Answer,
    DYNAMODB_SOURCE, KINESIS_SOURCE, MAPPINGS_URI, QUEUE_SOURCE,
};

/// The `State` a mapping an update was accepted for carries, from
/// `EventSourceMappingConfiguration$State`.
const STATE_UPDATING: &str = "Updating";

/// See [`STATE_UPDATING`]: the state a delete moves a mapping into.
const STATE_DELETING: &str = "Deleting";

/// The modelled error a mapping that is not available answers with, at HTTP 400.
const RESOURCE_IN_USE: &str = "ResourceInUseException";

/// The whole sentence that refusal carries — entirely fixed text, naming neither the
/// mapping nor the state it is in (`state::EVENT_SOURCE_MAPPING_DELETE_IN_USE_MESSAGE`), so
/// a change to that constant fails this suite.
const IN_USE_MESSAGE: &str = "Cannot delete the event source mapping because it is in use.";

/// How long past a due instant this suite waits before calling.
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

/// The `message` of a wire error, however the protocol spells the member.
fn message(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default()
}

/// The `UUID` an answer that created a mapping carries.
fn uuid_of(case: &str, answer: &Answer) -> String {
    answer
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", answer.text()))
}

/// One row: a mapping on one event source, and the update that makes it unavailable.
struct InUseCase {
    name: &'static str,
    function: &'static str,
    event_source: &'static str,
    /// What the mapping is created with, so the source's own required members are stated.
    create_extras: Value,
    /// What the update states — the change the mapping is still applying when the delete
    /// arrives.
    update: Value,
}

/// The rows: one per event source, each updating a different member.
fn in_use_cases() -> Vec<InUseCase> {
    vec![
        InUseCase {
            name: "a queue mapping whose batch size is being changed",
            function: "esm-inuse-batch",
            event_source: QUEUE_SOURCE,
            create_extras: json!({ "BatchSize": 5 }),
            update: json!({ "BatchSize": 10 }),
        },
        InUseCase {
            name: "a queue mapping that is being paused",
            function: "esm-inuse-pause",
            event_source: QUEUE_SOURCE,
            create_extras: json!({ "BatchSize": 5 }),
            update: json!({ "Enabled": false }),
        },
        InUseCase {
            name: "a kinesis mapping whose filter criteria are being changed",
            function: "esm-inuse-filter",
            event_source: KINESIS_SOURCE,
            create_extras: json!({ "StartingPosition": "LATEST", "BatchSize": 100 }),
            update: json!({
                "FilterCriteria": { "Filters": [{ "Pattern": "{\"eventName\":[\"INSERT\"]}" }] },
            }),
        },
        InUseCase {
            name: "a dynamodb mapping whose retry settings are being changed",
            function: "esm-inuse-stream",
            event_source: DYNAMODB_SOURCE,
            create_extras: json!({ "StartingPosition": "TRIM_HORIZON" }),
            update: json!({ "MaximumRetryAttempts": 4 }),
        },
    ]
}

/// RULE: while a mapping is applying an update it accepted, a delete of it is refused with
/// the modelled `ResourceInUseException` and the mapping stays readable; a twin mapping
/// nobody updated deletes cleanly at the same instant, and the identical delete succeeds
/// once the update has settled.
#[tokio::test]
async fn a_delete_is_refused_while_the_mapping_is_applying_an_update() {
    for case in in_use_cases() {
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
            "case {:?}: the mapping under test must be created, got {} {}",
            case.name,
            created.status,
            created.text()
        );
        let uuid = uuid_of(case.name, &created);

        // A twin on the SAME event source, held by a second function, that no update
        // touches: the POSITIVE CONTROL, so a blanket refusal fails here rather than
        // passing as the rule. (One function polls one source once: a second mapping of
        // the same pair is the modelled conflict, so the twin needs its own function.)
        let twin_function = "esm-inuse-twin";
        create_function(&router, twin_function).await;
        let twin = create_mapping(
            &router,
            &create_request(twin_function, Some(case.event_source), &case.create_extras),
        )
        .await;
        assert_eq!(
            twin.status,
            202,
            "case {:?}: the twin mapping must be created, got {} {}",
            case.name,
            twin.status,
            twin.text()
        );
        let twin_uuid = uuid_of(case.name, &twin);

        // Past the create's own settle, so the window under test is the UPDATE's and not
        // the create's.
        tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;

        let updated = update_mapping(&router, &uuid, &case.update).await;
        assert_eq!(
            (updated.status, updated.string("State")),
            (202, Some(STATE_UPDATING.to_string())),
            "case {:?}: the update must be accepted and report {STATE_UPDATING}, got {} {}",
            case.name,
            updated.status,
            updated.text()
        );

        // The delete that arrives inside that window is refused, by the modelled error and
        // the fixed sentence.
        let refused = delete_mapping(&router, &uuid).await;
        assert_eq!(
            (refused.status, refused.error_code().contains(RESOURCE_IN_USE)),
            (400, true),
            "case {:?}: expected 400 {RESOURCE_IN_USE}, got {} {}",
            case.name,
            refused.status,
            refused.text()
        );
        let reported = message(&refused);
        assert_eq!(
            reported, IN_USE_MESSAGE,
            "case {:?}: expected {IN_USE_MESSAGE:?}, got {reported:?}",
            case.name
        );

        // A refused delete deletes nothing: the mapping is still there, still applying the
        // update it acknowledged.
        let still_there = get_mapping(&router, &uuid).await;
        assert_eq!(
            (still_there.status, still_there.string("State")),
            (200, Some(STATE_UPDATING.to_string())),
            "case {:?}: the refused mapping must still read as {STATE_UPDATING}, got {} {}",
            case.name,
            still_there.status,
            still_there.text()
        );

        // The twin, which nobody updated, is deletable at this same instant: the gate is
        // scoped to the state and not to the source, the function or the account.
        let twin_deleted = delete_mapping(&router, &twin_uuid).await;
        assert_eq!(
            (twin_deleted.status, twin_deleted.string("State")),
            (202, Some(STATE_DELETING.to_string())),
            "case {:?}: the untouched twin must delete cleanly, got {} {}",
            case.name,
            twin_deleted.status,
            twin_deleted.text()
        );

        // And once the clock has closed the update's window, the IDENTICAL delete is
        // accepted — nothing about the request changed.
        tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;
        let accepted = delete_mapping(&router, &uuid).await;
        assert_eq!(
            (accepted.status, accepted.string("State")),
            (202, Some(STATE_DELETING.to_string())),
            "case {:?}: the identical delete must be accepted once the update settled, got \
             {} {}",
            case.name,
            accepted.status,
            accepted.text()
        );
    }
}

/// RULE: the gate is the UPDATE's window alone — a mapping nobody has updated is deletable
/// from the moment it exists, including inside its own `Creating` window, which the
/// modelled sentence does not name for a delete.
#[tokio::test]
async fn a_mapping_no_update_touched_is_deletable_in_every_state_it_reaches() {
    for (name, settled) in [
        ("inside its own creating window", false),
        ("once it has settled", true),
    ] {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, "esm-inuse-control").await;
        let created = create_mapping(
            &router,
            &create_request(
                "esm-inuse-control",
                Some(QUEUE_SOURCE),
                &json!({ "BatchSize": 5 }),
            ),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {name:?}: the mapping must be created, got {} {}",
            created.status,
            created.text()
        );
        let uuid = uuid_of(name, &created);
        if settled {
            tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;
        }

        let deleted = delete_mapping(&router, &uuid).await;
        assert_eq!(
            (deleted.status, deleted.string("State")),
            (202, Some(STATE_DELETING.to_string())),
            "case {name:?}: a mapping no update touched must delete cleanly, got {} {}",
            deleted.status,
            deleted.text()
        );
    }
}
