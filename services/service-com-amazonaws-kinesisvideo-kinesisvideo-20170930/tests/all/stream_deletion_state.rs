// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the stream DELETION state model:
//! `DeleteStream` marking a stream for deletion, `DescribeStream` projecting the
//! marked record, and the refusal a delete gets when the stream it names is not
//! `ACTIVE`.
//!
//! RULE under test: `DeleteStream` "marks the stream for deletion" rather than
//! removing it, so the record keeps every member it had and only its `Status` moves,
//! to `DELETING`, until the clock owes its removal; a delete addressed to a stream
//! that IS stored but is not in `ACTIVE` status is the modeled
//! `ResourceInUseException` (HTTP 400), and `ResourceNotFoundException` (HTTP 404)
//! is reserved for an identifier no stored stream answers to.
//!
//! Every case drives the real request walk (route -> deserialize -> validate ->
//! handle -> serialize) through `MultiServiceRouter::dispatch`, so a response here
//! is the same bytes a client on the socket would see.
//!
//! The router harness and the request/error helpers are the ones the sibling
//! `stream_data_retention` and `stream_lifecycle` suites already own — all three are
//! modules of this crate's single `tests/all.rs` target, so they are shared rather
//! than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::stream_data_retention::{call, error_code, harness};
use crate::stream_lifecycle::{wait_for_activation, wait_for_deletion};

/// The `Message` of a failure body.
fn error_message(body: &serde_json::Value) -> String {
    body.get("Message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// `DescribeStream` for `name`, as (status, `StreamInfo`-or-body). Unlike the
/// retention suite's reader this one does NOT assert success: the cases below assert
/// the refusals too.
async fn describe(router: &MultiServiceRouter, name: &str) -> (u16, serde_json::Value) {
    let (status, body) = call(
        router,
        "/describeStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    let projected = body.get("StreamInfo").cloned().unwrap_or(body);
    (status, projected)
}

/// An ARN of the modeled `ResourceARN` form naming a stream this store never held.
const ABSENT_STREAM_ARN: &str =
    "arn:aws:kinesisvideo:us-east-1:000000000000:stream/never-created/1700000000000";

/// A delete marks the stream for deletion: `DescribeStream` keeps answering 200 with
/// the SAME record, `Status` moved to `DELETING` and every other member — including
/// `Version` — carried over, until the removal falls due and the stream is gone.
///
/// The rows differ along what the rule generalises over: which optional members the
/// stream carries, and the size and form of each (`DataRetentionInHours` at its
/// documented minimum 0, at a middling value, and at the published maximum 87600).
#[tokio::test]
async fn a_delete_marks_the_stream_for_deletion_and_describe_projects_the_marked_record() {
    let router = harness();

    let rows: Vec<(&str, serde_json::Value)> = vec![
        (
            "only the required name",
            serde_json::json!({ "StreamName": "mark-bare" }),
        ),
        (
            "device, media type and the documented minimum retention",
            serde_json::json!({
                "StreamName": "mark.min", "DeviceName": "device_01",
                "MediaType": "video/h264", "DataRetentionInHours": 0
            }),
        ),
        (
            "a caller-chosen key and a middling retention",
            serde_json::json!({
                "StreamName": "mark-keyed", "DataRetentionInHours": 25,
                "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abcd-1234"
            }),
        ),
        (
            "every optional member, retention at the published maximum",
            serde_json::json!({
                "StreamName": "mark-full", "DeviceName": "d", "MediaType": "video/h264,audio/aac",
                "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/ffff-9999",
                "DataRetentionInHours": 87_600
            }),
        ),
    ];

    // Create every row first, then wait out the activation span once: a delete is
    // only accepted for an `ACTIVE` stream.
    for (case, request) in &rows {
        let (status, body) = call(&router, "/createStream", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStream failed with {body:?}"
        );
    }
    wait_for_activation();

    for (case, request) in &rows {
        let name = request
            .get("StreamName")
            .and_then(serde_json::Value::as_str)
            .expect("each row names its stream")
            .to_string();

        let (status, before) = describe(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStream before the delete: got {status} with {before:?}"
        );
        assert_eq!(
            before.get("Status").and_then(serde_json::Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: the row must be ACTIVE before the delete, got {before:?}"
        );
        let arn = before
            .get("StreamARN")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no StreamARN in {before:?}"))
            .to_string();

        let (status, body) = call(
            &router,
            "/deleteStream",
            serde_json::json!({ "StreamARN": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStream of an ACTIVE stream: got {status} with {body:?}"
        );

        // The marked record, member for member: only `Status` moved.
        let mut expected = before.clone();
        expected["Status"] = serde_json::json!("DELETING");
        let (status, after) = describe(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a stream marked for deletion stays describable, got {status} \
             with {after:?}"
        );
        assert_eq!(
            after, expected,
            "case {case:?}: expected {expected:?}, got {after:?}"
        );

        // Reading it again changes nothing: the removal is owed to the clock, not to
        // whoever polls.
        let (status, polled) = describe(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a second poll inside the deletion span, got {status} with {polled:?}"
        );
        assert_eq!(
            polled, after,
            "case {case:?}: polling must not change the record, expected {after:?}, got \
             {polled:?}"
        );
    }

    // Once the deletion falls due, every marked stream is gone — through the name and
    // through the ARN alike.
    wait_for_deletion();
    for (case, request) in &rows {
        let name = request
            .get("StreamName")
            .and_then(serde_json::Value::as_str)
            .expect("each row names its stream");
        let (status, body) = describe(&router, name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a removed stream must not describe, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body:?}"
        );
    }

    let (status, body) = call(&router, "/listStreams", serde_json::json!({})).await;
    assert_eq!(status, 200, "ListStreams after every removal: {body:?}");
    assert_eq!(
        body.get("StreamInfoList")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(0),
        "ListStreams must report no removed stream: {body:?}"
    );
}

/// A delete addressed to a stream that is STORED but not `ACTIVE` is refused as in
/// use (HTTP 400, `ResourceInUseException`, "The requested stream is not active."),
/// whatever `CurrentVersion` it carries, and the stream it named is left as it was.
/// An identifier no stored stream answers to is the other refusal: HTTP 404,
/// `ResourceNotFoundException`.
///
/// The rows differ along which non-`ACTIVE` status the delete meets (`CREATING`, from
/// a create inside its activation span; `DELETING`, from a stream already marked) and
/// along the `CurrentVersion` the request carries (the stored one, a stale one, none).
#[tokio::test]
async fn deleting_a_stream_that_is_not_active_is_refused_as_in_use() {
    /// What the service says when the stream a delete names is not `ACTIVE`.
    const NOT_ACTIVE: &str = "The requested stream is not active.";

    let router = harness();

    // Each row: the case, the status the stream is in when the delete lands, and
    // whether the request carries the stored version, a stale one, or none.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Guard {
        Stored,
        Stale,
        None,
    }
    let rows: Vec<(&str, &str, Guard)> = vec![
        ("creating, with the stored version", "CREATING", Guard::Stored),
        ("creating, with a stale version", "CREATING", Guard::Stale),
        ("creating, with no version", "CREATING", Guard::None),
        ("deleting, with the stored version", "DELETING", Guard::Stored),
        ("deleting, with a stale version", "DELETING", Guard::Stale),
        ("deleting, with no version", "DELETING", Guard::None),
    ];

    // The streams the `DELETING` rows need are created together, so ONE activation
    // wait serves all of them, and each is then marked by an accepted delete (which
    // itself requires `ACTIVE`). The `gate-removed` stream of the absence rows below
    // rides the same wait.
    for (index, (case, status_when_deleted, _)) in rows.iter().enumerate() {
        if *status_when_deleted != "DELETING" {
            continue;
        }
        let (status, body) = call(
            &router,
            "/createStream",
            serde_json::json!({ "StreamName": format!("gate-{index}") }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStream failed with {body:?}"
        );
    }
    let removed_arn = {
        let (status, body) = call(
            &router,
            "/createStream",
            serde_json::json!({ "StreamName": "gate-removed" }),
        )
        .await;
        assert_eq!(status, 200, "CreateStream for the removal row: {body:?}");
        body.get("StreamARN")
            .and_then(serde_json::Value::as_str)
            .expect("CreateStream returns a StreamARN")
            .to_string()
    };
    wait_for_activation();
    for (index, (case, status_when_deleted, _)) in rows.iter().enumerate() {
        if *status_when_deleted != "DELETING" {
            continue;
        }
        let name = format!("gate-{index}");
        let (_, info) = describe(&router, &name).await;
        let arn = info
            .get("StreamARN")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no StreamARN in {info:?}"))
            .to_string();
        let (status, body) = call(
            &router,
            "/deleteStream",
            serde_json::json!({ "StreamARN": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first delete must be accepted, got {status} with {body:?}"
        );
    }

    for (index, (case, status_when_deleted, guard)) in rows.iter().enumerate() {
        let name = format!("gate-{index}");
        if *status_when_deleted == "CREATING" {
            // A `CREATING` row is created HERE, immediately before its delete, so the
            // delete lands inside the activation span.
            let (status, body) = call(
                &router,
                "/createStream",
                serde_json::json!({ "StreamName": name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateStream failed with {body:?}"
            );
        }

        let (_, info) = describe(&router, &name).await;
        assert_eq!(
            info.get("Status").and_then(serde_json::Value::as_str),
            Some(*status_when_deleted),
            "case {case:?}: the row must be {status_when_deleted} when the delete lands, got \
             {info:?}"
        );
        let arn = info
            .get("StreamARN")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no StreamARN in {info:?}"))
            .to_string();
        let stored_version = info
            .get("Version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no Version in {info:?}"))
            .to_string();

        let mut request = serde_json::json!({ "StreamARN": arn });
        match guard {
            // A stale version inside `Version`'s modeled `@length` 1..64 and
            // `^[a-zA-Z0-9]+$` pattern, so the boundary validator passes it through.
            Guard::Stale => request["CurrentVersion"] = serde_json::json!("stale1"),
            Guard::Stored => request["CurrentVersion"] = serde_json::json!(stored_version),
            Guard::None => {}
        }
        let (status, body) = call(&router, "/deleteStream", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceInUseException",
            "case {case:?}: got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            NOT_ACTIVE,
            "case {case:?}: got {body:?}"
        );

        // The refused delete changed nothing a later caller sees.
        let (status, after) = describe(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the named stream must still describe, got {status} with {after:?}"
        );
        assert_eq!(
            after, info,
            "case {case:?}: a refused delete must leave the record as it was, expected \
             {info:?}, got {after:?}"
        );
    }

    // The other refusal: an identifier no stored stream answers to. The rows are the
    // two shapes of absence — never created, and removed once its deletion fell due —
    // each refused with the missing-resource error, not the in-use one.
    let (status, body) = call(
        &router,
        "/deleteStream",
        serde_json::json!({ "StreamARN": removed_arn.clone() }),
    )
    .await;
    assert_eq!(status, 200, "the removal row's delete: {body:?}");
    wait_for_deletion();

    for (case, absent) in [
        ("a stream that was never created", ABSENT_STREAM_ARN),
        ("a stream whose removal has fallen due", removed_arn.as_str()),
    ] {
        let (status, body) = call(
            &router,
            "/deleteStream",
            serde_json::json!({ "StreamARN": absent }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body:?}"
        );
    }
}
