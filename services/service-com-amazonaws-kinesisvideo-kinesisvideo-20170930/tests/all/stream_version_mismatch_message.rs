// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the SENTENCES the stream data-retention guards
//! refuse with.
//!
//! The rules under test:
//!
//! * a stream mutation whose `CurrentVersion` is not the stream's current version
//!   is refused with HTTP 400 `VersionMismatchException` carrying the service's
//!   fixed refusal sentence — the same sentence for every such request, with no
//!   stream name, ARN or version string interpolated into it — and the stored
//!   record is left untouched;
//! * a retention change whose result would fall outside the published 0..87600
//!   hours is refused with HTTP 400 `InvalidArgumentException` naming the bound it
//!   fell outside, followed by the stream's CURRENT retention and the `Operation`
//!   and `DataRetentionChangeInHours` the request carried.
//!
//! Every case drives the real request walk (route -> deserialize -> validate ->
//! handle -> serialize) through `MultiServiceRouter::dispatch`, so what is
//! asserted is the body a client on the socket reads.
//!
//! The router harness and the create/describe helpers are the ones the sibling
//! `stream_data_retention` suite already owns — both suites are modules of this
//! crate's single `tests/all.rs` target, so they are shared rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::stream_data_retention::{
    call, create_stream, error_code, harness, retention, stream_info, version,
};

/// The sentence AWS sends for a stream version mismatch. Wholly fixed text: it
/// names the two versions it compared without quoting either, which is why every
/// row below expects the very same string.
const EXPECTED_MESSAGE: &str =
    "The provided stream version does not match the existing stream version.";

/// The stream's `StreamARN`, read back through `DescribeStream`.
async fn stream_arn(router: &MultiServiceRouter, name: &str) -> String {
    let info = stream_info(router, name).await;
    info.get("StreamARN")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case arn {name:?}: no StreamARN in {info:?}"))
        .to_string()
}

/// The `Message` member of a refusal body.
fn message(body: &serde_json::Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("no Message in {body:?}"))
        .to_string()
}

/// Assert one refused call: 400, `VersionMismatchException`, and the fixed
/// sentence with nothing of the request interpolated into it.
fn assert_version_mismatch(
    case: &str,
    status: u16,
    body: &serde_json::Value,
    quoted_version: &str,
    stream_name: &str,
) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(body),
        "VersionMismatchException",
        "case {case}: expected VersionMismatchException, got {body:?}"
    );
    let actual = message(body);
    assert_eq!(
        actual, EXPECTED_MESSAGE,
        "case {case}: expected Message {EXPECTED_MESSAGE:?}, got {actual:?}"
    );
    // The sentence is fixed, so neither the rejected version nor the addressed
    // stream appears in it.
    assert!(
        !actual.contains(quoted_version),
        "case {case}: the fixed sentence must not quote the rejected version \
         {quoted_version:?}, got {actual:?}"
    );
    assert!(
        !actual.contains(stream_name),
        "case {case}: the fixed sentence must not name the stream {stream_name:?}, \
         got {actual:?}"
    );
}

/// RULE: `UpdateDataRetention` refuses every stale `CurrentVersion` with the one
/// fixed sentence, whatever the version's shape, the identifier form or the
/// direction and size of the change, and the stored retention does not move.
#[tokio::test]
async fn update_data_retention_refuses_a_stale_version_with_the_fixed_sentence() {
    let router = harness();
    let name = "retention-version-guard";
    create_stream(&router, name, 24).await;
    let arn = stream_arn(&router, name).await;
    let current = version(&router, name).await;

    // A version that WAS current and went stale: the ordinary optimistic-concurrency
    // loser, as against a version the stream never carried.
    let (status, body) = call(
        &router,
        "/updateDataRetention",
        serde_json::json!({
            "StreamName": name,
            "CurrentVersion": &current,
            "Operation": "INCREASE_DATA_RETENTION",
            "DataRetentionChangeInHours": 1,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case prime: expected HTTP 200, got {status} with {body:?}"
    );
    let stale = current;
    let retained = retention(&router, name).await;

    // Rows differ along the rejected version (now-stale, never-carried, the
    // shortest and the longest the model's `Version` allows: @length 1..64,
    // @pattern ^[a-zA-Z0-9]+$), the identifier form, and the direction and
    // magnitude of the change.
    let cases: Vec<(&str, bool, String, &str, i32)> = vec![
        (
            "a version that went stale, by name",
            false,
            stale.clone(),
            "INCREASE_DATA_RETENTION",
            1,
        ),
        (
            "a version that went stale, by ARN",
            true,
            stale,
            "DECREASE_DATA_RETENTION",
            1,
        ),
        (
            "a version the stream never carried",
            false,
            "notthisversion1".to_string(),
            "INCREASE_DATA_RETENTION",
            87_600,
        ),
        (
            "the shortest version the model allows",
            false,
            "1".to_string(),
            "DECREASE_DATA_RETENTION",
            24,
        ),
        (
            "the longest version the model allows",
            true,
            "v".repeat(64),
            "INCREASE_DATA_RETENTION",
            9,
        ),
        (
            "a mixed-case alphanumeric version",
            true,
            "AbC123dEf".to_string(),
            "DECREASE_DATA_RETENTION",
            87_600,
        ),
    ];

    for (case, by_arn, quoted, operation, change) in cases {
        let mut members = serde_json::Map::new();
        if by_arn {
            members.insert("StreamARN".to_string(), serde_json::json!(&arn));
        } else {
            members.insert("StreamName".to_string(), serde_json::json!(name));
        }
        members.insert("CurrentVersion".to_string(), serde_json::json!(&quoted));
        members.insert("Operation".to_string(), serde_json::json!(operation));
        members.insert(
            "DataRetentionChangeInHours".to_string(),
            serde_json::json!(change),
        );
        let (status, body) = call(
            &router,
            "/updateDataRetention",
            serde_json::Value::Object(members),
        )
        .await;
        assert_version_mismatch(case, status, &body, &quoted, name);
        let stored = retention(&router, name).await;
        assert_eq!(
            stored, retained,
            "case {case}: a refused update must leave the retention at {retained}, got {stored}"
        );
    }
}

/// RULE: the same guard on the other stream mutation that takes a version —
/// `DeleteStream` — refuses a stale `CurrentVersion` with that same fixed
/// sentence, and the stream survives the refusal.
#[tokio::test]
async fn delete_stream_refuses_a_stale_version_with_the_same_fixed_sentence() {
    let router = harness();

    // Rows differ along the rejected version's shape, as above.
    let cases: Vec<(&str, String)> = vec![
        ("a version the stream never carried", "notthisversion1".to_string()),
        ("the shortest version the model allows", "9".to_string()),
        ("the longest version the model allows", "d".repeat(64)),
        ("a mixed-case alphanumeric version", "Zz0099Aa".to_string()),
    ];

    for (index, (case, quoted)) in cases.into_iter().enumerate() {
        let name = format!("delete-version-guard-{index}");
        create_stream(&router, &name, 12).await;
        let arn = stream_arn(&router, &name).await;
        let (status, body) = call(
            &router,
            "/deleteStream",
            serde_json::json!({ "StreamARN": &arn, "CurrentVersion": &quoted }),
        )
        .await;
        assert_version_mismatch(case, status, &body, &quoted, &name);
        // The refusal left the record: the stream still reads back with its
        // retention, so nothing was marked for deletion.
        let stored = retention(&router, &name).await;
        assert_eq!(
            stored, 12,
            "case {case}: a refused delete must leave the stream readable at 12 hours, \
             got {stored}"
        );
    }
}

/// RULE: a retention change whose result would leave the published range is
/// refused with the bound it fell outside, the stream's CURRENT retention, and
/// the `Operation` and `DataRetentionChangeInHours` the request carried — so the
/// sentence moves with the state and the request, never with one recorded run.
#[tokio::test]
async fn a_result_outside_the_range_names_the_bound_the_state_and_the_request() {
    let router = harness();

    // Rows differ along the stored retention, the direction, the size of the
    // change and which published bound (0, and the 87600 hours
    // `DataRetentionChangeInHours`' docs publish as the maximum) the result would
    // cross, including a result exactly one hour past each bound.
    let cases: Vec<(&str, i32, &str, i32)> = vec![
        ("one hour below the floor", 0, "DECREASE_DATA_RETENTION", 1),
        ("far below the floor", 25, "DECREASE_DATA_RETENTION", 100),
        (
            "below the floor from the ceiling",
            87_600,
            "DECREASE_DATA_RETENTION",
            87_601,
        ),
        (
            "one hour past the ceiling",
            87_600,
            "INCREASE_DATA_RETENTION",
            1,
        ),
        (
            "far past the ceiling",
            1,
            "INCREASE_DATA_RETENTION",
            87_600,
        ),
    ];

    for (index, (case, initial, operation, change)) in cases.into_iter().enumerate() {
        let name = format!("range-sentence-{index}");
        create_stream(&router, &name, initial).await;
        let current = version(&router, &name).await;
        let (status, body) = call(
            &router,
            "/updateDataRetention",
            serde_json::json!({
                "StreamName": &name,
                "CurrentVersion": &current,
                "Operation": operation,
                "DataRetentionChangeInHours": change,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidArgumentException",
            "case {case}: expected InvalidArgumentException, got {body:?}"
        );
        let below = operation == "DECREASE_DATA_RETENTION";
        let expected = if below {
            format!(
                "The desired data retention cannot be less than 0. Current data retention: \
                 {initial}. Operation: {operation}, Requested change: {change}"
            )
        } else {
            format!(
                "The desired data retention cannot be more than 87600. Current data \
                 retention: {initial}. Operation: {operation}, Requested change: {change}"
            )
        };
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {case}: expected Message {expected:?}, got {actual:?}"
        );
        // The refused change left the record where it was.
        let stored = retention(&router, &name).await;
        assert_eq!(
            stored,
            i64::from(initial),
            "case {case}: a refused update must leave the retention at {initial}, got {stored}"
        );
    }
}

/// RULE: both guarded stream mutations word the refusal the SAME way — one
/// sentence per failure class, not one per operation.
#[tokio::test]
async fn both_guarded_stream_mutations_send_one_sentence() {
    let router = harness();
    let name = "one-sentence-stream";
    create_stream(&router, name, 1).await;
    let arn = stream_arn(&router, name).await;
    let stale = "staleversion1";

    let (update_status, update_body) = call(
        &router,
        "/updateDataRetention",
        serde_json::json!({
            "StreamARN": &arn,
            "CurrentVersion": stale,
            "Operation": "INCREASE_DATA_RETENTION",
            "DataRetentionChangeInHours": 1,
        }),
    )
    .await;
    assert_version_mismatch(
        "UpdateDataRetention",
        update_status,
        &update_body,
        stale,
        name,
    );

    let (delete_status, delete_body) = call(
        &router,
        "/deleteStream",
        serde_json::json!({ "StreamARN": &arn, "CurrentVersion": stale }),
    )
    .await;
    assert_version_mismatch("DeleteStream", delete_status, &delete_body, stale, name);

    let from_update = message(&update_body);
    let from_delete = message(&delete_body);
    assert_eq!(
        from_update, from_delete,
        "case one sentence: expected both mutations to word the refusal alike, got \
         {from_update:?} and {from_delete:?}"
    );
}
