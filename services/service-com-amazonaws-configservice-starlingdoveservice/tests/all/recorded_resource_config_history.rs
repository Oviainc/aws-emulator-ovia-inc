// SPDX-License-Identifier: CC0-1.0

//! What one resource's configuration history answers once its states were recorded
//! through the API a client actually has, end to end over the wire.
//!
//! The rules pinned here:
//!
//! 1. every state recorded for a resource is ONE item of its history — the history
//!    reports them newest first, and oldest first when the request asks for `Forward`;
//! 2. a window bound a client took from a capture instant this operation REPORTED
//!    selects the item it was taken from, at either end of the window: the window is
//!    matched at the resolution a reported capture instant carries, which a client reads
//!    back through the epoch-seconds number the wire carries;
//! 3. a resource key the account recorded no configuration item for is refused with
//!    `ResourceNotDiscoveredException`, while a resource whose DELETION was recorded
//!    still has a history, whose newest item is the one carrying `ResourceDeleted`.
//!
//! This suite differs from `get_resource_config_history`, which pins the same reader
//! over states seeded straight into the store: here every state is recorded the way a
//! client records one — `PutResourceConfig` and `DeleteResourceConfig` against a
//! RUNNING configuration recorder — so the capture instants are the ones the service
//! itself assigned, and the window bounds are the numbers it reported them as.
//!
//! Every case drives the emulator through the same `MultiServiceRouter` the production
//! router builds, reusing the `Emulator` harness `config_setup_and_rule_loop` defines.

use std::time::Duration;

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// The operation under test.
const HISTORY: &str = "GetResourceConfigHistory";

/// The name Config assigns a configuration recorder created without one
/// (`ConfigurationRecorder$name`: "Config automatically assigns the name of
/// \"default\" ... if you do not specify a name at creation time").
const RECORDER: &str = "default";

/// Resource types the rows record. Entries of
/// `com.amazonaws.configservice#ResourceType`, from `smithy_query(query="shape",
/// shape="ResourceType")` — the public registry types `PutResourceConfig` records —
/// deliberately including types no recording in the issue used.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";
/// See [`EC2_VOLUME`].
const KMS_KEY: &str = "AWS::KMS::Key";

/// The `ConfigurationItemStatus` of an item that records a configuration the resource
/// UPDATED, of the one that records its first discovery, and of the one that records its
/// deletion (`com.amazonaws.configservice#ConfigurationItemStatus`).
const STATUS_OK: &str = "OK";
/// See [`STATUS_OK`].
const STATUS_RESOURCE_DISCOVERED: &str = "ResourceDiscovered";
/// See [`STATUS_OK`].
const STATUS_RESOURCE_DELETED: &str = "ResourceDeleted";

/// How long one recording is spaced from the next, so each state of a resource is
/// captured at its own instant rather than two sharing one: the rows below name ONE
/// state by its reported capture instant, which needs the instants to differ.
const BETWEEN_RECORDINGS: Duration = Duration::from_millis(5);

/// The `PutResourceConfig` body recording `configuration` for one resource.
fn resource_config(resource_type: &str, resource_id: &str, configuration: &str) -> Value {
    json!({
        "ResourceType": resource_type,
        "SchemaVersionId": "00000000-0000-0000-0000-000000000001",
        "ResourceId": resource_id,
        "ResourceName": resource_id,
        "Configuration": configuration,
    })
}

/// The configuration document the `revision`-th state of a resource records. Each state
/// records a DIFFERENT document, which is what makes the one it replaces a state the
/// resource LEFT rather than an overwrite.
fn configuration(revision: usize) -> String {
    json!({ "revision": revision }).to_string()
}

/// An emulator whose configuration recorder is RECORDING: Config records a
/// configuration item only while one is.
async fn recording_emulator() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    emulator
        .ok(
            "PutDeliveryChannel",
            &json!({ "DeliveryChannel": { "s3BucketName": "config-bucket" } }),
        )
        .await;
    emulator
        .ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER }),
        )
        .await;
    emulator
}

/// Record `revisions` successive states of one resource the way a client does, each one
/// a distinct configuration document.
async fn record_states(
    emulator: &Emulator,
    resource_type: &str,
    resource_id: &str,
    revisions: usize,
) {
    for revision in 0..revisions {
        if revision > 0 {
            tokio::time::sleep(BETWEEN_RECORDINGS).await;
        }
        emulator
            .ok(
                "PutResourceConfig",
                &resource_config(resource_type, resource_id, &configuration(revision)),
            )
            .await;
    }
}

/// The request body for one history read: the resource key, plus the `extra` members
/// (an order, a window bound, a limit) the case adds.
fn request(resource_type: &str, resource_id: &str, extra: Value) -> Value {
    let mut request = json!({ "resourceType": resource_type, "resourceId": resource_id });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    request
}

/// The configuration items a response reported, in the order it carried them.
fn items(case: &str, body: &Value) -> Vec<Value> {
    body["configurationItems"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected configurationItems, got {body}"))
        .clone()
}

/// The instant each reported item was captured, as the epoch-seconds number the wire
/// carried it as — which is exactly what a client holds after reading a history.
fn captured_at(case: &str, body: &Value) -> Vec<f64> {
    items(case, body)
        .iter()
        .map(|item| {
            item["configurationItemCaptureTime"]
                .as_f64()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected an item capture time, got {item}")
                })
        })
        .collect()
}

/// The `configuration` document of each reported item, in the order reported.
fn configurations(case: &str, body: &Value) -> Vec<String> {
    items(case, body)
        .iter()
        .map(|item| {
            item["configuration"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a configuration, got {item}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn every_state_recorded_through_the_api_is_one_item_of_the_history() {
    // Rows differ along what the rule generalises over: how many states the resource
    // went through, and the resource type they were recorded under.
    let cases: [(&str, &str, usize); 5] = [
        ("one state of a volume", EC2_VOLUME, 1),
        ("two states of a volume", EC2_VOLUME, 2),
        ("five states of a volume", EC2_VOLUME, 5),
        ("three states of a table", DYNAMODB_TABLE, 3),
        ("four states of a key", KMS_KEY, 4),
    ];

    for (case, resource_type, revisions) in cases {
        let emulator = recording_emulator().await;
        let resource_id = format!("r-{revisions:03}");
        record_states(&emulator, resource_type, &resource_id, revisions).await;

        let newest_first: Vec<String> = (0..revisions).rev().map(configuration).collect();
        let oldest_first: Vec<String> = (0..revisions).map(configuration).collect();

        // Every state recorded is one item, reported newest first by default and oldest
        // first when the request asks to go forward.
        for (order, expected) in [
            (json!({}), &newest_first),
            (json!({ "chronologicalOrder": "Reverse" }), &newest_first),
            (json!({ "chronologicalOrder": "Forward" }), &oldest_first),
        ] {
            let body = emulator
                .ok(HISTORY, &request(resource_type, &resource_id, order.clone()))
                .await;
            let reported = configurations(case, &body);
            assert_eq!(
                &reported, expected,
                "case {case:?} order {order}: expected the states {expected:?}, got \
                 {reported:?} in {body}"
            );
        }

        // Each item names the resource it records, in this account and the Region the
        // request was signed for, and the instants they were captured at are strictly
        // ordered: an item records a state the resource was in at one instant.
        let body = emulator
            .ok(HISTORY, &request(resource_type, &resource_id, json!({})))
            .await;
        let reported = items(case, &body);
        for (position, item) in reported.iter().enumerate() {
            // The OLDEST item of the resource is the one that discovered it; every later
            // one records a configuration that has been updated since
            // (`ConfigurationItem$configurationItemStatus`).
            let expected_status = if position + 1 == reported.len() {
                STATUS_RESOURCE_DISCOVERED
            } else {
                STATUS_OK
            };
            assert_eq!(
                (
                    item["resourceType"].as_str(),
                    item["resourceId"].as_str(),
                    item["resourceName"].as_str(),
                    item["awsRegion"].as_str(),
                    item["accountId"].as_str(),
                    item["configurationItemStatus"].as_str(),
                ),
                (
                    Some(resource_type),
                    Some(resource_id.as_str()),
                    Some(resource_id.as_str()),
                    Some(emulator.region.as_str()),
                    Some(emulator_server::emulator_account_id()),
                    Some(expected_status),
                ),
                "case {case:?}: every item must name the recorded resource, got {item}"
            );
        }
        let captured = captured_at(case, &body);
        assert!(
            captured.windows(2).all(|pair| pair[0] > pair[1]),
            "case {case:?}: the items are reported newest first, at distinct instants, \
             got {captured:?}"
        );
    }
}

#[tokio::test]
async fn a_window_bound_taken_from_a_reported_capture_time_selects_that_state() {
    // Four recorded states, so a window can name one end of the history, the other, a
    // span in the middle, or the whole of it — every bound being a capture instant this
    // operation itself reported, which is the only form a client has one in.
    const REVISIONS: usize = 4;
    let emulator = recording_emulator().await;
    let resource_id = "vol-00000000000000001";
    record_states(&emulator, EC2_VOLUME, resource_id, REVISIONS).await;

    // Oldest first, so an index names the state of that revision: `at[0]` is the instant
    // the first state was captured.
    let body = emulator
        .ok(
            HISTORY,
            &request(EC2_VOLUME, resource_id, json!({ "chronologicalOrder": "Forward" })),
        )
        .await;
    let at = captured_at("the reported capture instants", &body);
    assert_eq!(
        at.len(),
        REVISIONS,
        "the four recorded states are what the windows below select from, got {body}"
    );

    // Rows differ along which ends the window names and how much of the history it
    // spans; `expected` names the revisions the window must select, newest first.
    let cases: [(&str, Value, Vec<usize>); 9] = [
        (
            "the whole history, both ends named",
            json!({ "earlierTime": at[0], "laterTime": at[3] }),
            vec![3, 2, 1, 0],
        ),
        (
            "only the earlier end, at the oldest state",
            json!({ "earlierTime": at[0] }),
            vec![3, 2, 1, 0],
        ),
        (
            "only the later end, at the newest state",
            json!({ "laterTime": at[3] }),
            vec![3, 2, 1, 0],
        ),
        (
            "only the earlier end, at the newest state",
            json!({ "earlierTime": at[3] }),
            vec![3],
        ),
        (
            "only the later end, at the oldest state",
            json!({ "laterTime": at[0] }),
            vec![0],
        ),
        (
            "the two middle states",
            json!({ "earlierTime": at[1], "laterTime": at[2] }),
            vec![2, 1],
        ),
        (
            "one state, both ends naming it",
            json!({ "earlierTime": at[2], "laterTime": at[2] }),
            vec![2],
        ),
        (
            "the older half",
            json!({ "earlierTime": at[0], "laterTime": at[1] }),
            vec![1, 0],
        ),
        (
            "the newer half",
            json!({ "earlierTime": at[2], "laterTime": at[3] }),
            vec![3, 2],
        ),
    ];

    for (case, window, expected) in cases {
        // Newest first, the order the operation reports by default.
        let expected_newest_first: Vec<String> =
            expected.iter().map(|revision| configuration(*revision)).collect();
        let body = emulator
            .ok(HISTORY, &request(EC2_VOLUME, resource_id, window.clone()))
            .await;
        let reported = configurations(case, &body);
        assert_eq!(
            reported, expected_newest_first,
            "case {case:?} window {window}: expected the states {expected_newest_first:?}, \
             got {reported:?} in {body}"
        );

        // The same window, oldest first: a window selects the same states whichever
        // order they are reported in.
        let mut forward = window.clone();
        forward["chronologicalOrder"] = json!("Forward");
        let expected_oldest_first: Vec<String> =
            expected_newest_first.iter().rev().cloned().collect();
        let body = emulator
            .ok(HISTORY, &request(EC2_VOLUME, resource_id, forward.clone()))
            .await;
        let reported = configurations(case, &body);
        assert_eq!(
            reported, expected_oldest_first,
            "case {case:?} window {forward}: expected the states {expected_oldest_first:?}, \
             got {reported:?} in {body}"
        );
    }

    // A window that reaches a state's capture instant from outside still selects it, and
    // one that stops short of the whole history selects nothing: the bounds are what the
    // window is about, not the number of states there are.
    for (case, window, expected) in [
        (
            "a window reaching past both ends",
            json!({ "earlierTime": at[0] - 60.0, "laterTime": at[3] + 60.0 }),
            REVISIONS,
        ),
        (
            "a window ending before the first state",
            json!({ "laterTime": at[0] - 60.0 }),
            0,
        ),
        (
            "a window starting after the last state",
            json!({ "earlierTime": at[3] + 60.0, "laterTime": at[3] + 120.0 }),
            0,
        ),
    ] {
        let body = emulator
            .ok(HISTORY, &request(EC2_VOLUME, resource_id, window.clone()))
            .await;
        let reported = configurations(case, &body);
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?} window {window}: expected {expected} states, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_key_nothing_was_recorded_for_is_refused_and_a_deleted_resource_keeps_its_history() {
    let emulator = recording_emulator().await;
    let recorded = "vol-00000000000000001";
    record_states(&emulator, EC2_VOLUME, recorded, 2).await;
    record_states(&emulator, DYNAMODB_TABLE, "orders", 1).await;

    // Rows differ along which half of the key the account recorded nothing for: the id,
    // the type, and a pair whose halves were each recorded under another partner.
    let cases: [(&str, &str, &str); 4] = [
        ("an id nothing was recorded for", EC2_VOLUME, "vol-00000000000000404"),
        ("a type nothing was recorded for", KMS_KEY, "key-1"),
        ("an id recorded under another type", DYNAMODB_TABLE, recorded),
        ("a type recorded under another id", EC2_VOLUME, "orders"),
    ];

    for (case, resource_type, resource_id) in cases {
        let (status, parsed) = emulator
            .call(HISTORY, &request(resource_type, resource_id, json!({})))
            .await;
        let code = parsed["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<none>");
        assert_eq!(
            (status, code),
            (400, "ResourceNotDiscoveredException"),
            "case {case:?}: expected HTTP 400 ResourceNotDiscoveredException, got {status} \
             {code:?} in {parsed}"
        );
        let expected = format!(
            "Resource {resource_id} of resourceType:{resource_type} is unknown or has not \
             been discovered"
        );
        assert_eq!(
            parsed["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {parsed}"
        );
    }

    // Recording the resource's DELETION records one more state: its history keeps every
    // state it went through, and the newest item is the one that records it went away.
    tokio::time::sleep(BETWEEN_RECORDINGS).await;
    emulator
        .ok(
            "DeleteResourceConfig",
            &json!({ "ResourceType": EC2_VOLUME, "ResourceId": recorded }),
        )
        .await;
    let body = emulator
        .ok(HISTORY, &request(EC2_VOLUME, recorded, json!({})))
        .await;
    let case = "a resource whose deletion was recorded";
    let reported: Vec<String> = items(case, &body)
        .iter()
        .map(|item| {
            item["configurationItemStatus"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every item carries a status, got {item}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        reported,
        vec![STATUS_RESOURCE_DELETED, STATUS_OK, STATUS_RESOURCE_DISCOVERED],
        "case {case:?}: the deletion is the newest of the three states, got {body}"
    );
}

#[tokio::test]
async fn a_window_bound_taken_from_a_reported_start_instant_selects_that_evaluation() {
    // The rule about a window bound a client took from a reported instant is about the
    // instants this service reports, not about one operation, so it is pinned here on
    // the other read of this service whose request carries a time window:
    // `ListResourceEvaluations` selects by the instant an evaluation was STARTED, which
    // it reports as `EvaluationStartTimestamp`.
    let emulator = Emulator::new();
    let resource_id = "vol-00000000000000002";
    let started = emulator
        .ok(
            "StartResourceEvaluation",
            &json!({
                "EvaluationMode": "PROACTIVE",
                "ResourceDetails": {
                    "ResourceId": resource_id,
                    "ResourceType": EC2_VOLUME,
                    "ResourceConfiguration": json!({ "Name": resource_id }).to_string(),
                },
            }),
        )
        .await;
    let evaluation_id = started["ResourceEvaluationId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a ResourceEvaluationId, got {started}"))
        .to_string();

    let body = emulator.ok("ListResourceEvaluations", &json!({})).await;
    let at = body["ResourceEvaluations"][0]["EvaluationStartTimestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("expected an EvaluationStartTimestamp, got {body}"));

    // Rows differ along which ends of the window the reported instant is used as.
    for (case, window) in [
        ("both ends at the reported instant", json!({ "StartTime": at, "EndTime": at })),
        ("the start at the reported instant", json!({ "StartTime": at })),
        ("the end at the reported instant", json!({ "EndTime": at })),
        (
            "a window reaching past the reported instant",
            json!({ "StartTime": at - 60.0, "EndTime": at + 60.0 }),
        ),
    ] {
        let body = emulator
            .ok(
                "ListResourceEvaluations",
                &json!({ "Filters": { "TimeWindow": window } }),
            )
            .await;
        let reported: Vec<&str> = body["ResourceEvaluations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected ResourceEvaluations, got {body}"))
            .iter()
            .filter_map(|evaluation| evaluation["ResourceEvaluationId"].as_str())
            .collect();
        assert_eq!(
            reported,
            vec![evaluation_id.as_str()],
            "case {case:?}: a window built from the reported start instant selects the \
             evaluation it was taken from, got {body}"
        );
    }
}
