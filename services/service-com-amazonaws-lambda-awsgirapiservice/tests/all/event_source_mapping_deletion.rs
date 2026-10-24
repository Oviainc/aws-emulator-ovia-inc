// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteEventSourceMapping`: "Deletes an event source mapping. You
//! can get the identifier of a mapping from the output of ListEventSourceMappings. When
//! you delete an event source mapping, it enters a Deleting state and might not be
//! completely deleted for several seconds."
//!
//! The rule under test: a delete addressed by the UUID of a mapping this account holds
//! moves THAT mapping into `Deleting` and answers the stored record in that state; the
//! REMOVAL is owed by the clock, so until it is due the mapping is still reported (by
//! `GetEventSourceMapping` and by a listing) as `Deleting` and a second delete of it is
//! refused with the modelled `ResourceInUseException`, and once it is due the mapping is
//! held no more — a read of it is the modelled not-found, a listing omits it, and its
//! event source is free to be mapped again. A UUID no mapping is held under names nothing
//! to delete and is refused with the modelled `ResourceNotFoundException`, leaving every
//! stored mapping where it was.
//!
//! Every row drives real HTTP requests through the router — over the ONE harness this test
//! binary has for them, [`crate::event_source_mappings`] — and checks the engine's own
//! store afterwards, so a delete that answered 202 without persisting the state, or that
//! removed a record the clock had not yet made due, fails here. The rows differ along what
//! the rule generalises over: WHICH event source the deleted mapping polls (a queue, each
//! of the two shard sources, an MSK cluster and a self-managed Kafka cluster), whether the
//! mapping had already settled out of `Creating` when the delete arrived, whether it was
//! enabled, and which optional settings it carried.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ComAmazonawsLambdaAwsgirapiserviceState, EVENT_SOURCE_MAPPING_DELETE_DELAY,
    EVENT_SOURCE_MAPPING_SETTLE_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_function, create_mapping, create_request, get_mapping, list_mappings, router_on,
    Answer, DYNAMODB_SOURCE, KINESIS_SOURCE, MAPPINGS_URI, MSK_SOURCE, QUEUE_SOURCE,
    STATE_CREATING, USER_INITIATED, UUID_LEN,
};

/// The `State` a mapping a delete was accepted for carries, from
/// `EventSourceMappingConfiguration$State`: "It can be one of the following: Creating,
/// Enabling, Enabled, Disabling, Disabled, Updating, or Deleting."
const STATE_DELETING: &str = "Deleting";

/// A UUID of the length `UUIDString` requires (`@length(min: 36, max: 36)`) that no
/// mapping is held under.
const UNHELD_UUID: &str = "11111111-2222-3333-4444-555555555555";

/// How long past a due instant this suite waits before reading. The transitions are owed
/// by the CLOCK, so nothing here sleeps on the emulator's behalf: it waits past the
/// instant the call made due.
const SLACK: Duration = Duration::from_millis(100);

/// `DeleteEventSourceMapping` on the mapping `uuid` names, from
/// `smithy_query(query="operation", operation="DeleteEventSourceMapping")`
/// (`DELETE /2015-03-31/event-source-mappings/{UUID}`).
async fn delete_mapping(router: &MultiServiceRouter, uuid: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &Value::Null,
    )
    .await
}

/// The UUIDs the engine's own store holds mappings under.
fn stored_uuids(engine: &Arc<engine::InMemoryEngine>) -> Vec<String> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store.event_source_mappings.keys().cloned().collect()
}

/// One deletion row: which event source the deleted mapping polls, what else its create
/// stated, and whether the delete arrives after the mapping settled out of `Creating`.
struct DeleteCase {
    name: &'static str,
    function: &'static str,
    /// The event source ARN, or `None` for a self-managed Apache Kafka cluster, which is
    /// named by `SelfManagedEventSource` instead.
    event_source: Option<&'static str>,
    request_extras: Value,
    /// Whether the delete waits for the clock to settle the mapping out of `Creating`
    /// first.
    settled_first: bool,
}

/// The rows: one per event source kind (the sources the `EventSourceArn` member
/// enumerates), differing in whether the mapping had settled when the delete arrived, in
/// its `Enabled` setting and in the optional settings it carried.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            name: "a settled queue mapping",
            function: "esm-del-queue",
            event_source: Some(QUEUE_SOURCE),
            request_extras: json!({ "BatchSize": 7 }),
            settled_first: true,
        },
        DeleteCase {
            name: "a queue mapping still being created",
            function: "esm-del-creating",
            event_source: Some(QUEUE_SOURCE),
            request_extras: json!({}),
            settled_first: false,
        },
        DeleteCase {
            name: "a disabled kinesis stream mapping",
            function: "esm-del-kinesis",
            event_source: Some(KINESIS_SOURCE),
            request_extras: json!({ "StartingPosition": "LATEST", "Enabled": false }),
            settled_first: true,
        },
        DeleteCase {
            name: "a dynamodb stream mapping with filter criteria",
            function: "esm-del-dynamodb",
            event_source: Some(DYNAMODB_SOURCE),
            request_extras: json!({
                "StartingPosition": "TRIM_HORIZON",
                "FilterCriteria": { "Filters": [{ "Pattern": "{\"eventName\":[\"INSERT\"]}" }] },
                "MaximumRetryAttempts": 3,
            }),
            settled_first: false,
        },
        DeleteCase {
            name: "an msk cluster mapping",
            function: "esm-del-msk",
            event_source: Some(MSK_SOURCE),
            request_extras: json!({ "Topics": ["orders"] }),
            settled_first: true,
        },
        DeleteCase {
            name: "a self-managed kafka mapping, named by no event source arn",
            function: "esm-del-self-managed",
            event_source: None,
            request_extras: json!({
                "SelfManagedEventSource": {
                    "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": ["broker-1.example.com:9092"] },
                },
                "Topics": ["orders"],
            }),
            settled_first: false,
        },
    ]
}

#[tokio::test]
async fn a_deleted_mapping_reports_deleting_until_the_clock_removes_it() {
    for case in delete_cases() {
        let label = case.name;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, case.function).await;

        // A second mapping, of another event source, that no delete below names: it must
        // survive untouched.
        create_function(&router, "esm-del-bystander-fn").await;
        let bystander = create_mapping(
            &router,
            &create_request(
                "esm-del-bystander-fn",
                Some("arn:aws:sqs:us-east-1:000000000000:bystander"),
                &json!({}),
            ),
        )
        .await;
        assert_eq!(
            bystander.status,
            202,
            "case {label:?}: the bystander mapping must be created, got {} {}",
            bystander.status,
            bystander.text()
        );
        let bystander_uuid = bystander
            .string("UUID")
            .unwrap_or_else(|| panic!("case {label:?}: the bystander create answers a UUID"));

        let created = create_mapping(
            &router,
            &create_request(case.function, case.event_source, &case.request_extras),
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

        if case.settled_first {
            tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;
        } else {
            let reading = get_mapping(&router, &uuid).await;
            assert_eq!(
                (reading.status, reading.string("State")),
                (200, Some(STATE_CREATING.to_string())),
                "case {label:?}: the mapping must still be {STATE_CREATING} before the \
                 settle is due, got {} {}",
                reading.status,
                reading.text()
            );
        }

        // "When you delete an event source mapping, it enters a Deleting state": the
        // delete answers the record it just wrote, not an echo of its own request.
        let deleted = delete_mapping(&router, &uuid).await;
        assert_eq!(
            (
                deleted.status,
                deleted.string("State"),
                deleted.string("UUID"),
                deleted.string("EventSourceMappingArn"),
                deleted.string("StateTransitionReason"),
            ),
            (
                202,
                Some(STATE_DELETING.to_string()),
                Some(uuid.clone()),
                Some(arn.clone()),
                Some(USER_INITIATED.to_string()),
            ),
            "case {label:?}: the delete must answer the stored mapping as {STATE_DELETING}, \
             got {} {}",
            deleted.status,
            deleted.text()
        );
        assert_eq!(
            deleted.json().get("EventSourceArn").and_then(Value::as_str),
            case.event_source,
            "case {label:?}: the delete reports the event source the mapping was created \
             against, got {}",
            deleted.text()
        );
        assert_eq!(
            deleted.json().get("BatchSize"),
            created.json().get("BatchSize"),
            "case {label:?}: the delete reports the stored settings unchanged, got {}",
            deleted.text()
        );

        // "and might not be completely deleted for several seconds": until the removal is
        // due the mapping is still held, and reported as being deleted.
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (reading.status, reading.string("State")),
            (200, Some(STATE_DELETING.to_string())),
            "case {label:?}: the mapping must still be readable as {STATE_DELETING} before \
             its removal is due, got {} {}",
            reading.status,
            reading.text()
        );
        let listed = list_mappings(&router, "").await;
        assert!(
            listed.listed_uuids().contains(&uuid),
            "case {label:?}: a mapping being deleted is still listed, got {}",
            listed.text()
        );
        // A second delete conflicts with "the resource's availability" rather than being
        // accepted again.
        let again = delete_mapping(&router, &uuid).await;
        assert_eq!(
            (
                again.status,
                again.error_code().contains("ResourceInUseException")
            ),
            (400, true),
            "case {label:?}: a second delete must be refused with ResourceInUseException, \
             got {} {}",
            again.status,
            again.text()
        );

        tokio::time::sleep(EVENT_SOURCE_MAPPING_DELETE_DELAY + SLACK).await;

        // The removal the clock owed has happened: the mapping is held no more, by the
        // store or by anything reading it.
        let gone = get_mapping(&router, &uuid).await;
        assert_eq!(
            (
                gone.status,
                gone.error_code().contains("ResourceNotFoundException")
            ),
            (404, true),
            "case {label:?}: the removed mapping must be reported as not found, got {} {}",
            gone.status,
            gone.text()
        );
        let listed = list_mappings(&router, "").await;
        assert_eq!(
            listed.listed_uuids(),
            vec![bystander_uuid.clone()],
            "case {label:?}: only the bystander mapping must be left, got {}",
            listed.text()
        );
        assert_eq!(
            stored_uuids(&engine),
            vec![bystander_uuid.clone()],
            "case {label:?}: the store itself must hold only the bystander mapping"
        );
        let deleted_again = delete_mapping(&router, &uuid).await;
        assert_eq!(
            (
                deleted_again.status,
                deleted_again
                    .error_code()
                    .contains("ResourceNotFoundException")
            ),
            (404, true),
            "case {label:?}: deleting the removed mapping must be the not-found, got {} {}",
            deleted_again.status,
            deleted_again.text()
        );

        // The event source is free again: the pair it formed with the function no longer
        // conflicts with anything.
        let recreated = create_mapping(
            &router,
            &create_request(case.function, case.event_source, &case.request_extras),
        )
        .await;
        assert_eq!(
            recreated.status,
            202,
            "case {label:?}: the event source must be free to be mapped again, got {} {}",
            recreated.status,
            recreated.text()
        );
        let new_uuid = recreated
            .string("UUID")
            .unwrap_or_else(|| panic!("case {label:?}: the re-create answers a UUID"));
        assert_ne!(
            new_uuid, uuid,
            "case {label:?}: a new mapping is a new resource, not the deleted one"
        );
    }
}

#[tokio::test]
async fn deleting_a_mapping_this_account_does_not_hold_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-del-refusal-fn";
    create_function(&router, function).await;

    let created = create_mapping(
        &router,
        &create_request(
            function,
            Some("arn:aws:sqs:us-east-1:000000000000:untouched"),
            &json!({}),
        ),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "the untouched mapping must be created, got {} {}",
        created.status,
        created.text()
    );
    let untouched = created.string("UUID").expect("the create answers a UUID");

    // Each row names a UUID of the length the model requires that this account holds no
    // mapping under: one never minted, and one whose hexadecimal digits differ from the
    // stored mapping's.
    let near_miss: String = untouched
        .chars()
        .map(|character| match character {
            'f' => '0',
            other if other.is_ascii_hexdigit() => 'f',
            other => other,
        })
        .collect();

    for (label, uuid) in [
        ("a uuid nothing was ever created under", UNHELD_UUID),
        (
            "a uuid whose digits differ from a stored one",
            near_miss.as_str(),
        ),
    ] {
        assert_eq!(
            uuid.len(),
            UUID_LEN,
            "case {label:?}: the row must carry a UUID of the modelled length"
        );
        let answer = delete_mapping(&router, uuid).await;
        assert_eq!(
            (
                answer.status,
                answer.error_code().contains("ResourceNotFoundException")
            ),
            (404, true),
            "case {label:?}: expected 404 ResourceNotFoundException, got {} {}",
            answer.status,
            answer.text()
        );
    }

    assert_eq!(
        stored_uuids(&engine),
        vec![untouched.clone()],
        "a refused delete must leave every stored mapping where it was"
    );
    let reading = get_mapping(&router, &untouched).await;
    assert_eq!(
        reading.status,
        200,
        "the untouched mapping must still be readable, got {} {}",
        reading.status,
        reading.text()
    );
}
