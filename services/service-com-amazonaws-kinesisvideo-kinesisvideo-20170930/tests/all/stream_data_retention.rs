// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `UpdateDataRetention`.
//!
//! Every case drives the real request walk (route -> deserialize -> validate ->
//! handle -> serialize) through [`MultiServiceRouter::dispatch`], and reads the
//! result back through `DescribeStream`, so what is asserted is the STORED
//! retention rather than the update's own answer.
//!
//! The rule under test: the operation "Increases or decreases the stream's data
//! retention period by the value that you specify", so the stored retention is
//! its current value moved by that delta, bounded by the published 0..87600
//! hours, and only for a caller quoting the stream's current version.
//!
//! The harness below, and the helpers around it, are this crate's ONE copy for
//! the retention rule: the sibling `stream_data_retention_projection` suite —
//! which covers what the `StreamInfo` readers project once a CHAIN of updates has
//! been accepted — reuses them rather than carrying a second copy.

use std::sync::Arc;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::MultiServiceRouter;

/// The emulator endpoint every case addresses.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// A router carrying only this service, over its own engine, with request
/// authentication off so a case can send an unsigned-but-scoped request.
pub(crate) fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(kvs::EmulatedService::new(engine));
    router
}

/// POST one restJson1 call and return its status and parsed JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    path: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{ENDPOINT_HOST}{path}"))
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/json")
        // The SigV4 credential scope routes the request and names its region;
        // the signature itself is not checked here.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisvideo/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, parsed)
}

/// The wire error code of a failure body (`__type`), with any Smithy namespace
/// stripped.
pub(crate) fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .rsplit('#')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Create `name` with `retention` hours of data retention.
pub(crate) async fn create_stream(router: &MultiServiceRouter, name: &str, retention: i32) {
    let (status, body) = call(
        router,
        "/createStream",
        serde_json::json!({ "StreamName": name, "DataRetentionInHours": retention }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create {name:?}: expected HTTP 200, got {status} with {body:?}"
    );
}

/// The stream's `StreamInfo`.
pub(crate) async fn stream_info(router: &MultiServiceRouter, name: &str) -> serde_json::Value {
    let (status, body) = call(
        router,
        "/describeStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case describe {name:?}: expected HTTP 200, got {status} with {body:?}"
    );
    body.get("StreamInfo")
        .cloned()
        .unwrap_or_else(|| panic!("case describe {name:?}: no StreamInfo in {body:?}"))
}

/// The stream's current `Version`.
pub(crate) async fn version(router: &MultiServiceRouter, name: &str) -> String {
    let info = stream_info(router, name).await;
    info.get("Version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case version {name:?}: no Version in {info:?}"))
        .to_string()
}

/// The stream's stored retention, as `DescribeStream` projects it.
pub(crate) async fn retention(router: &MultiServiceRouter, name: &str) -> i64 {
    let info = stream_info(router, name).await;
    info.get("DataRetentionInHours")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_else(|| panic!("case retention {name:?}: no DataRetentionInHours in {info:?}"))
}

/// RULE: an accepted `UpdateDataRetention` leaves the stream carrying its
/// previous retention moved by the requested delta, in the direction the
/// `Operation` names, and `DescribeStream` reports that value.
#[tokio::test]
async fn an_accepted_update_moves_the_stored_retention_by_the_delta() {
    let router = harness();

    // Rows differ along the created retention, the direction and the magnitude,
    // including the smallest delta the model allows (`@range` min 1) and a
    // result back at zero.
    let cases: Vec<(&str, i32, &str, i32, i64)> = vec![
        ("zero up by one", 0, "INCREASE_DATA_RETENTION", 1, 1),
        ("zero up by a day", 0, "INCREASE_DATA_RETENTION", 24, 24),
        ("one up by 24", 1, "INCREASE_DATA_RETENTION", 24, 25),
        ("48 down by 24", 48, "DECREASE_DATA_RETENTION", 24, 24),
        ("five down to zero", 5, "DECREASE_DATA_RETENTION", 5, 0),
        (
            "one up to the maximum",
            87_599,
            "INCREASE_DATA_RETENTION",
            1,
            87_600,
        ),
    ];

    for (case, initial, operation, change, expected) in cases {
        let name = format!("retention-{initial}-{operation}-{change}");
        create_stream(&router, &name, initial).await;
        let before = version(&router, &name).await;
        let (status, body) = call(
            &router,
            "/updateDataRetention",
            serde_json::json!({
                "StreamName": &name,
                "CurrentVersion": before,
                "Operation": operation,
                "DataRetentionChangeInHours": change,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {body:?}"
        );
        let stored = retention(&router, &name).await;
        assert_eq!(
            stored, expected,
            "case {case}: expected DataRetentionInHours {expected}, got {stored}"
        );
        // "When you update a stream, Kinesis Video Streams assigns a new version
        // number", so the version the next caller must quote is a fresh one.
        let after = version(&router, &name).await;
        assert_ne!(
            after, before,
            "case {case}: expected a new Version, still {after:?}"
        );
    }
}

/// RULE: a delta that would take the retention outside the published range
/// ("the minimum value for data retention is 0 and the maximum value is 87600")
/// is `InvalidArgumentException` (400) and the stored retention does not move.
#[tokio::test]
async fn a_result_outside_the_published_range_is_refused() {
    let router = harness();

    // Rows sit at each published bound and one step past it.
    let cases: Vec<(&str, i32, &str, i32, u16)> = vec![
        ("at the floor", 0, "DECREASE_DATA_RETENTION", 1, 400),
        ("one above the floor", 1, "DECREASE_DATA_RETENTION", 1, 200),
        ("past the floor", 5, "DECREASE_DATA_RETENTION", 6, 400),
        ("at the ceiling", 87_600, "INCREASE_DATA_RETENTION", 1, 400),
        (
            "one below the ceiling",
            87_599,
            "INCREASE_DATA_RETENTION",
            1,
            200,
        ),
        (
            "far past the ceiling",
            1,
            "INCREASE_DATA_RETENTION",
            87_600,
            400,
        ),
    ];

    for (case, initial, operation, change, expected_status) in cases {
        let name = format!("bounds-{initial}-{operation}-{change}");
        create_stream(&router, &name, initial).await;
        let current = version(&router, &name).await;
        let (status, body) = call(
            &router,
            "/updateDataRetention",
            serde_json::json!({
                "StreamName": &name,
                "CurrentVersion": current,
                "Operation": operation,
                "DataRetentionChangeInHours": change,
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        let stored = retention(&router, &name).await;
        if expected_status == 200 {
            assert_ne!(
                stored, i64::from(initial),
                "case {case}: an accepted update must move the retention off {initial}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "InvalidArgumentException",
                "case {case}: expected InvalidArgumentException, got {body:?}"
            );
            assert_eq!(
                stored,
                i64::from(initial),
                "case {case}: a refused update must leave the retention at {initial}, got {stored}"
            );
        }
    }
}

/// RULE: the identifier and version guards every stream mutation carries — a
/// stream that is not there is 404, a stale `CurrentVersion` is
/// `VersionMismatchException` (400), and neither moves the stored retention.
#[tokio::test]
async fn the_identifier_and_version_guards_leave_the_record_untouched() {
    let router = harness();
    let name = "guarded-stream";
    create_stream(&router, name, 10).await;
    let current = version(&router, name).await;

    // (the case, the addressing and version members, the status and error code)
    let cases: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "an unknown StreamName",
            serde_json::json!({ "StreamName": "absent-stream", "CurrentVersion": &current }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown StreamARN",
            serde_json::json!({
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/absent/1761000000",
                "CurrentVersion": &current,
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "no identifier at all",
            serde_json::json!({ "CurrentVersion": &current }),
            400,
            "InvalidArgumentException",
        ),
        (
            "a version the stream never carried",
            serde_json::json!({ "StreamName": name, "CurrentVersion": "notthisversion1" }),
            400,
            "VersionMismatchException",
        ),
    ];

    for (case, members, expected_status, expected_code) in cases {
        let body = {
            let mut object = members.as_object().cloned().expect("an object of members");
            object.insert(
                "Operation".to_string(),
                serde_json::json!("INCREASE_DATA_RETENTION"),
            );
            object.insert("DataRetentionChangeInHours".to_string(), serde_json::json!(1));
            serde_json::Value::Object(object)
        };
        let (status, answered) = call(&router, "/updateDataRetention", body).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case}: expected {expected_code}, got {answered:?}"
        );
        let stored = retention(&router, name).await;
        assert_eq!(
            stored, 10,
            "case {case}: a refused update must leave the retention at 10, got {stored}"
        );
    }
}
