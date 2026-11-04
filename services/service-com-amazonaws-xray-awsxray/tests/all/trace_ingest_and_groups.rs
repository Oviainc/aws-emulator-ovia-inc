// SPDX-License-Identifier: CC0-1.0

//! The X-Ray ingest -> query -> read loop and the Group lifecycle, end to end
//! over the wire.
//!
//! Every case drives the SAME front door the production router does: a
//! `MultiServiceRouter` with this service mounted, addressed by the `xray`
//! endpoint host, speaking restJson1. So the assertions below are about HTTP
//! responses, not about internal state.
//!
//! The tests are tables, and each row differs along what the rule it checks
//! generalises over — the number of segments, the recorded HTTP status, whether
//! a segment is complete, the shape of the submitted document, the count of
//! resources relative to a page — because the rules are:
//!
//! * a reader's answer is recomputed from the stored segment documents, and
//!   `Segment$Document` is the byte-for-byte string that was submitted;
//! * a document the service cannot read is reported as one
//!   `UnprocessedTraceSegment` and does not fail the rest of the batch;
//! * `GetTraceSummaries` selects on the half-open interval
//!   [`StartTime`, `EndTime`);
//! * a paginated operation emits a `NextToken` only when its results were
//!   truncated, and resuming from that token yields the remainder exactly once;
//! * a group is answered from its stored record, by either selector, until it
//!   is deleted, and the reserved account-level `Default` group is listed
//!   beside the stored ones (`group_control_plane.rs` owns that rule);
//! * behaviour this slice does not implement (a `GetTraceSummaries`
//!   `FilterExpression` that is not of the language, sampling, a token this
//!   service never issued) is REFUSED with the service's own
//!   `InvalidRequestException`, never answered with a success that ignored it,
//!   while a violated MODELLED constraint is refused by the request layer as
//!   `ValidationException`;
//! * caller-driven work and growth are bounded: a batch, a requested-id list, a
//!   trace's segments and the stored trace count each have a cap, and the cap is
//!   reported — as a refusal, as an `UnprocessedTraceSegment`, as
//!   `LimitExceeded`, or as an eviction — never silently exceeded.

use std::collections::BTreeSet;

use serde_json::{json, Value};

// The wire harness lives ONCE, in `xray_wire`. Every item it owns is
// re-exported here as well, so a suite that reaches the harness through this
// module keeps finding that one definition instead of standing up a second copy
// of the front door.
pub(crate) use crate::xray_wire::{
    array, close, root_document, trace_id, Answer, Xray, DEFAULT_REGION,
};

// =============================================================================
// 1. Every TraceSummary / Trace field is recomputed from the stored segments
// =============================================================================

/// One ingest -> read case. Rows differ in segment count, recorded HTTP status,
/// completeness, flags and annotations.
struct TraceCase {
    case: &'static str,
    trace: String,
    documents: Vec<String>,
    duration: f64,
    response_time: Option<f64>,
    has_error: bool,
    has_fault: bool,
    has_throttle: bool,
    is_partial: bool,
    http_status: Option<i64>,
    http_url: Option<&'static str>,
    service_names: Vec<&'static str>,
    annotation_keys: Vec<&'static str>,
}

fn trace_cases() -> Vec<TraceCase> {
    // Row 1 is the request the X-Ray segment-document documentation itself
    // shows; every other row varies one axis of the rule.
    let one = trace_id(1);
    let two = trace_id(2);
    let three = trace_id(3);
    let four = trace_id(4);
    let five = trace_id(5);
    let six = trace_id(6);
    let seven = trace_id(7);
    let eight = trace_id(8);
    vec![
        TraceCase {
            case: "single complete root segment with a 200 response",
            trace: one.clone(),
            documents: vec![json!({
                "trace_id": one,
                "id": "70de5b6f19ff9a0a",
                "name": "svc",
                "start_time": 1_478_293_361.271_f64,
                "end_time": 1_478_293_361.449_f64,
                "http": {"request": {"method": "GET", "url": "http://localhost/"}, "response": {"status": 200}},
            })
            .to_string()],
            duration: 0.178,
            response_time: Some(0.178),
            has_error: false,
            has_fault: false,
            has_throttle: false,
            is_partial: false,
            http_status: Some(200),
            http_url: Some("http://localhost/"),
            service_names: vec!["svc"],
            annotation_keys: vec![],
        },
        TraceCase {
            case: "root plus a subsegment that outlives it",
            trace: two.clone(),
            documents: vec![
                root_document("1111111111111111", &two, "frontend", 100.0, Some(100.5))
                    .to_string(),
                json!({
                    "trace_id": two,
                    "id": "2222222222222222",
                    "parent_id": "1111111111111111",
                    "name": "backend",
                    "start_time": 100.1_f64,
                    "end_time": 100.9_f64,
                })
                .to_string(),
            ],
            // Duration spans the earliest start to the latest end; ResponseTime
            // is the ROOT segment's own span.
            duration: 0.9,
            response_time: Some(0.5),
            has_error: false,
            has_fault: false,
            has_throttle: false,
            is_partial: false,
            http_status: None,
            http_url: None,
            service_names: vec!["frontend", "backend"],
            annotation_keys: vec![],
        },
        TraceCase {
            case: "a declared error beside the 404 response an SDK records with it",
            trace: three.clone(),
            documents: vec![json!({
                "trace_id": three,
                "id": "3333333333333333",
                "name": "svc",
                "start_time": 200.0_f64,
                "end_time": 200.25_f64,
                "error": true,
                "http": {"request": {"method": "POST", "url": "http://localhost/missing"}, "response": {"status": 404}},
            })
            .to_string()],
            duration: 0.25,
            response_time: Some(0.25),
            has_error: true,
            has_fault: false,
            has_throttle: false,
            is_partial: false,
            http_status: Some(404),
            http_url: Some("http://localhost/missing"),
            service_names: vec!["svc"],
            annotation_keys: vec![],
        },
        TraceCase {
            // The same response with NO flag declared: the status is reported
            // as `Http$HttpStatus` and sets no flag, because the status-to-flag
            // convention belongs to the instrumentation that writes the
            // document.
            case: "a 503 response that declares no fault is no fault",
            trace: four.clone(),
            documents: vec![json!({
                "trace_id": four,
                "id": "4444444444444444",
                "name": "svc",
                "start_time": 300.0_f64,
                "end_time": 300.5_f64,
                "http": {"request": {"method": "GET", "url": "http://localhost/down"}, "response": {"status": 503}},
            })
            .to_string()],
            duration: 0.5,
            response_time: Some(0.5),
            has_error: false,
            has_fault: false,
            has_throttle: false,
            is_partial: false,
            http_status: Some(503),
            http_url: Some("http://localhost/down"),
            service_names: vec!["svc"],
            annotation_keys: vec![],
        },
        TraceCase {
            // A throttle DECLARED as a flag, beside the 429 an SDK records with
            // it: X-Ray reclassifies a declared throttle rather than unioning
            // it, so `$HasThrottle` alone is set and the counters count it under
            // `ErrorStatistics$ThrottleCount`.
            case: "a declared throttle beside a 429 response",
            trace: five.clone(),
            documents: vec![json!({
                "trace_id": five,
                "id": "5555555555555555",
                "name": "svc",
                "start_time": 400.0_f64,
                "end_time": 400.125_f64,
                "throttle": true,
                "http": {"request": {"method": "GET", "url": "http://localhost/busy"}, "response": {"status": 429}},
            })
            .to_string()],
            duration: 0.125,
            response_time: Some(0.125),
            has_error: false,
            has_fault: false,
            has_throttle: true,
            is_partial: false,
            http_status: Some(429),
            http_url: Some("http://localhost/busy"),
            service_names: vec!["svc"],
            annotation_keys: vec![],
        },
        TraceCase {
            case: "an in-progress segment makes the trace partial",
            trace: six.clone(),
            documents: vec![
                root_document("6666666666666666", &six, "slow", 500.0, None).to_string(),
            ],
            duration: 0.0,
            response_time: None,
            has_error: false,
            has_fault: false,
            has_throttle: false,
            is_partial: true,
            http_status: None,
            http_url: None,
            service_names: vec!["slow"],
            annotation_keys: vec![],
        },
        TraceCase {
            case: "the fault flag alone, with no http block",
            trace: seven.clone(),
            documents: vec![json!({
                "trace_id": seven,
                "id": "7777777777777777",
                "name": "worker",
                "start_time": 600.0_f64,
                "end_time": 601.0_f64,
                "fault": true,
            })
            .to_string()],
            duration: 1.0,
            response_time: Some(1.0),
            has_error: false,
            has_fault: true,
            has_throttle: false,
            is_partial: false,
            http_status: None,
            http_url: None,
            service_names: vec!["worker"],
            annotation_keys: vec![],
        },
        TraceCase {
            case: "annotations of all three value types",
            trace: eight.clone(),
            documents: vec![json!({
                "trace_id": eight,
                "id": "8888888888888888",
                "name": "annotated",
                "start_time": 700.0_f64,
                "end_time": 700.5_f64,
                "origin": "AWS::EC2::Instance",
                "annotations": {"stage": "beta", "attempt": 3, "cached": false},
            })
            .to_string()],
            duration: 0.5,
            response_time: Some(0.5),
            has_error: false,
            has_fault: false,
            has_throttle: false,
            is_partial: false,
            http_status: None,
            http_url: None,
            service_names: vec!["annotated"],
            annotation_keys: vec!["stage", "attempt", "cached"],
        },
    ]
}

#[tokio::test]
async fn trace_summaries_and_traces_are_recomputed_from_the_stored_segment_documents() {
    for case in trace_cases() {
        let xray = Xray::new();
        let label = case.case;

        let put = xray.put_segments(&case.documents).await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: PutTraceSegments must succeed, got {put:?}"
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "case {label:?}: every document was well formed, so UnprocessedTraceSegments must be empty, got {:?}",
            put.get("UnprocessedTraceSegments")
        );

        // The earliest submitted start_time, so the window brackets the trace.
        let earliest = case
            .documents
            .iter()
            .filter_map(|document| serde_json::from_str::<Value>(document).ok())
            .filter_map(|document| document.get("start_time").and_then(Value::as_f64))
            .fold(f64::INFINITY, f64::min);

        let got = xray.summaries(earliest - 1.0, earliest + 1.0, None).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: GetTraceSummaries must succeed, got {got:?}"
        );
        let summaries = array(&got.body, "TraceSummaries");
        assert_eq!(
            summaries.len(),
            1,
            "case {label:?}: exactly the one ingested trace must be summarised, got {summaries:?}"
        );
        let summary = &summaries[0];
        assert_eq!(
            summary.get("Id").and_then(Value::as_str),
            Some(case.trace.as_str()),
            "case {label:?}: summary Id must be the submitted trace_id, got {summary:?}"
        );
        let duration = summary.get("Duration").and_then(Value::as_f64);
        assert!(
            duration.is_some_and(|actual| close(actual, case.duration)),
            "case {label:?}: expected Duration {}, got {duration:?}",
            case.duration
        );
        let response_time = summary.get("ResponseTime").and_then(Value::as_f64);
        match case.response_time {
            Some(expected) => assert!(
                response_time.is_some_and(|actual| close(actual, expected)),
                "case {label:?}: expected ResponseTime {expected}, got {response_time:?}"
            ),
            None => assert!(
                response_time.is_none(),
                "case {label:?}: an in-progress root has no ResponseTime, got {response_time:?}"
            ),
        }
        for (member, expected) in [
            ("HasError", case.has_error),
            ("HasFault", case.has_fault),
            ("HasThrottle", case.has_throttle),
            ("IsPartial", case.is_partial),
        ] {
            assert_eq!(
                summary.get(member).and_then(Value::as_bool),
                Some(expected),
                "case {label:?}: expected {member} {expected}, got {:?}",
                summary.get(member)
            );
        }
        let start_time = summary.get("StartTime").and_then(Value::as_f64);
        assert!(
            start_time.is_some_and(|actual| close(actual, earliest)),
            "case {label:?}: expected StartTime {earliest}, got {start_time:?}"
        );
        assert_eq!(
            summary
                .get("Http")
                .and_then(|http| http.get("HttpStatus"))
                .and_then(Value::as_i64),
            case.http_status,
            "case {label:?}: expected Http.HttpStatus {:?}, got {:?}",
            case.http_status,
            summary.get("Http")
        );
        assert_eq!(
            summary
                .get("Http")
                .and_then(|http| http.get("HttpURL"))
                .and_then(Value::as_str),
            case.http_url,
            "case {label:?}: expected Http.HttpURL {:?}, got {:?}",
            case.http_url,
            summary.get("Http")
        );
        let service_names: Vec<&str> = array(summary, "ServiceIds")
            .iter()
            .filter_map(|id| id.get("Name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            service_names, case.service_names,
            "case {label:?}: expected ServiceIds names {:?}, got {service_names:?}",
            case.service_names
        );
        let annotation_keys: BTreeSet<&str> = summary
            .get("Annotations")
            .and_then(Value::as_object)
            .map(|map| map.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let expected_keys: BTreeSet<&str> = case.annotation_keys.iter().copied().collect();
        assert_eq!(
            annotation_keys, expected_keys,
            "case {label:?}: expected Annotations keys {expected_keys:?}, got {annotation_keys:?}"
        );
        let scanned = got.get("TracesProcessedCount").as_i64();
        assert_eq!(
            scanned,
            Some(1),
            "case {label:?}: one stored trace was scanned, got {scanned:?}"
        );
        assert!(
            got.get("ApproximateTime").as_f64().is_some_and(|at| at > 0.0),
            "case {label:?}: ApproximateTime must be set, got {:?}",
            got.get("ApproximateTime")
        );
        assert!(
            got.get("NextToken").is_null(),
            "case {label:?}: a single-page result emits no NextToken, got {:?}",
            got.get("NextToken")
        );

        // And the same trace read by id returns the documents VERBATIM.
        let read = xray.batch_get(std::slice::from_ref(&case.trace), None).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: BatchGetTraces must succeed, got {read:?}"
        );
        assert!(
            array(&read.body, "UnprocessedTraceIds").is_empty(),
            "case {label:?}: the stored id is processed, got {:?}",
            read.get("UnprocessedTraceIds")
        );
        let traces = array(&read.body, "Traces");
        assert_eq!(
            traces.len(),
            1,
            "case {label:?}: one trace was requested and stored, got {traces:?}"
        );
        assert_eq!(
            traces[0].get("Id").and_then(Value::as_str),
            Some(case.trace.as_str()),
            "case {label:?}: Trace.Id must be the stored trace_id, got {:?}",
            traces[0]
        );
        let read_duration = traces[0].get("Duration").and_then(Value::as_f64);
        assert!(
            read_duration.is_some_and(|actual| close(actual, case.duration)),
            "case {label:?}: expected Trace.Duration {}, got {read_duration:?}",
            case.duration
        );
        assert_eq!(
            traces[0].get("LimitExceeded").and_then(Value::as_bool),
            Some(false),
            "case {label:?}: nothing was dropped or truncated, so LimitExceeded must be false, got {:?}",
            traces[0]
        );
        let documents: Vec<&str> = array(&traces[0], "Segments")
            .iter()
            .filter_map(|segment| segment.get("Document").and_then(Value::as_str))
            .collect();
        let submitted: Vec<&str> = case.documents.iter().map(String::as_str).collect();
        assert_eq!(
            documents, submitted,
            "case {label:?}: Segment.Document must be the submitted string byte for byte, got {documents:?}"
        );
    }
}

// =============================================================================
// 2. A document the service cannot read is reported, not fatal
// =============================================================================

#[tokio::test]
async fn an_unreadable_document_is_reported_and_the_rest_of_the_batch_still_lands() {
    let good_trace = trace_id(20);
    let good = root_document("aaaaaaaaaaaaaaaa", &good_trace, "ok", 1_000.0, Some(1_000.5))
        .to_string();

    // Each row is a different way of being unreadable; the expected ErrorCode
    // is X-Ray's own (`ParseError`, or `Missing`/`Invalid` plus the member in
    // PascalCase), and the reported Id is the document's own `id` when it
    // carried one.
    let rows: Vec<(&str, String, &str, Option<&str>)> = vec![
        (
            "not JSON at all",
            "{\"trace_id\": ".to_string(),
            "ParseError",
            None,
        ),
        (
            "a JSON array has no segment fields",
            json!([{"id": "bbbbbbbbbbbbbbbb"}]).to_string(),
            "ParseError",
            None,
        ),
        (
            "a JSON scalar has no segment fields",
            "42".to_string(),
            "ParseError",
            None,
        ),
        (
            "missing trace_id",
            json!({"id": "cccccccccccccccc", "name": "x", "start_time": 1.0_f64})
                .to_string(),
            "MissingTraceId",
            Some("cccccccccccccccc"),
        ),
        (
            "missing id",
            json!({"trace_id": trace_id(21), "name": "x", "start_time": 1.0_f64}).to_string(),
            "MissingId",
            None,
        ),
        (
            "missing name",
            json!({"trace_id": trace_id(22), "id": "dddddddddddddddd", "start_time": 1.0_f64})
                .to_string(),
            "MissingName",
            Some("dddddddddddddddd"),
        ),
        (
            "missing start_time",
            json!({"trace_id": trace_id(23), "id": "eeeeeeeeeeeeeeee", "name": "x"}).to_string(),
            "MissingStartTime",
            Some("eeeeeeeeeeeeeeee"),
        ),
        (
            "trace_id of the wrong JSON type",
            json!({"trace_id": 7, "id": "ffffffffffffffff", "name": "x", "start_time": 1.0_f64})
                .to_string(),
            "InvalidTraceId",
            Some("ffffffffffffffff"),
        ),
        (
            "a trace_id that is not of the documented form",
            json!({"trace_id": "not-a-trace-id", "id": "1212121212121212", "name": "x", "start_time": 1.0_f64})
                .to_string(),
            "InvalidTraceId",
            Some("1212121212121212"),
        ),
        (
            "a trace_id of the right shape but the wrong lengths",
            json!({"trace_id": "1-581cf7-a006649127e371903a2de9", "id": "1313131313131313", "name": "x", "start_time": 1.0_f64})
                .to_string(),
            "InvalidTraceId",
            Some("1313131313131313"),
        ),
        (
            "start_time of the wrong JSON type",
            json!({"trace_id": trace_id(24), "id": "0000000000000000", "name": "x", "start_time": "soon"})
                .to_string(),
            "InvalidStartTime",
            Some("0000000000000000"),
        ),
        (
            // No reader can convert an instant this far from the epoch to the
            // modelled `Timestamp`, so the document is refused rather than
            // stored.
            "start_time further from the epoch than a recorded time can be",
            json!({"trace_id": trace_id(25), "id": "1111000000000000", "name": "x", "start_time": 1e300_f64})
                .to_string(),
            "InvalidStartTime",
            Some("1111000000000000"),
        ),
        (
            "a negative start_time further from the epoch than a recorded time can be",
            json!({"trace_id": trace_id(26), "id": "2222000000000000", "name": "x", "start_time": -1e300_f64})
                .to_string(),
            "InvalidStartTime",
            Some("2222000000000000"),
        ),
        (
            "end_time further from the epoch than a recorded time can be",
            json!({"trace_id": trace_id(27), "id": "3333000000000000", "name": "x", "start_time": 1.0_f64, "end_time": 1e300_f64})
                .to_string(),
            "InvalidEndTime",
            Some("3333000000000000"),
        ),
    ];

    for (label, bad, expected_code, expected_id) in rows {
        let xray = Xray::new();
        let put = xray.put_segments(&[good.clone(), bad.clone()]).await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: one bad document must not fail the call, got {put:?}"
        );
        let unprocessed = array(&put.body, "UnprocessedTraceSegments");
        assert_eq!(
            unprocessed.len(),
            1,
            "case {label:?}: exactly the bad document is unprocessed, got {unprocessed:?}"
        );
        assert_eq!(
            unprocessed[0].get("ErrorCode").and_then(Value::as_str),
            Some(expected_code),
            "case {label:?}: expected ErrorCode {expected_code:?}, got {:?}",
            unprocessed[0]
        );
        assert_eq!(
            unprocessed[0].get("Id").and_then(Value::as_str),
            expected_id,
            "case {label:?}: expected the reported Id {expected_id:?}, got {:?}",
            unprocessed[0]
        );
        assert_eq!(
            unprocessed[0].get("Message").and_then(Value::as_str),
            Some(format!("Invalid segment. ErrorCode: {expected_code}").as_str()),
            "case {label:?}: the rejection's Message restates its ErrorCode, got {:?}",
            unprocessed[0]
        );

        // The good document in the same batch still landed, and nothing was
        // stored for the bad one.
        let read = xray
            .batch_get(std::slice::from_ref(&good_trace), None)
            .await;
        let traces = array(&read.body, "Traces");
        assert_eq!(
            traces.len(),
            1,
            "case {label:?}: the well-formed document in the same batch must still be stored, got {traces:?}"
        );
        // A window inside the widest one this read serves, holding the stored
        // document's own instant.
        let summaries = xray.summaries(0.0, 86_400.0, None).await;
        assert_eq!(
            array(&summaries.body, "TraceSummaries").len(),
            1,
            "case {label:?}: only the accepted document created a trace, got {:?}",
            summaries.get("TraceSummaries")
        );
    }
}

// =============================================================================
// 3. GetTraceSummaries selects on the half-open interval [StartTime, EndTime)
// =============================================================================

#[tokio::test]
async fn the_time_window_is_half_open_and_selects_on_the_stored_start_time() {
    let xray = Xray::new();
    // Three traces at known, well-separated start times.
    let at: Vec<(String, f64)> = vec![
        (trace_id(31), 1_000.0),
        (trace_id(32), 2_000.0),
        (trace_id(33), 3_000.0),
    ];
    for (index, (trace, start)) in at.iter().enumerate() {
        let document =
            root_document(&format!("{:016x}", index + 1), trace, "svc", *start, Some(start + 0.1))
                .to_string();
        let put = xray.put_segments(&[document]).await;
        assert_eq!(put.status, 200, "seeding {trace} must succeed, got {put:?}");
    }

    // Rows differ in where the window falls relative to the stored start times,
    // including each boundary and a window past the last one.
    let rows: Vec<(&str, f64, f64, Vec<usize>)> = vec![
        ("StartTime inclusive, EndTime exclusive", 1_000.0, 3_000.0, vec![0, 1]),
        ("an empty window selects nothing", 1_000.0, 1_000.0, vec![]),
        ("a window one millisecond wide at the first start", 1_000.0, 1_000.001, vec![0]),
        ("a window containing everything", 0.0, 10_000.0, vec![0, 1, 2]),
        ("a window starting at the last start time", 3_000.0, 4_000.0, vec![2]),
        ("a window past every stored trace", 9_000.0, 9_999.0, vec![]),
        ("a window before every stored trace", 0.0, 999.0, vec![]),
    ];

    for (label, start, end, expected_rows) in rows {
        let got = xray.summaries(start, end, None).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: GetTraceSummaries must succeed, got {got:?}"
        );
        let ids: Vec<&str> = array(&got.body, "TraceSummaries")
            .iter()
            .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
            .collect();
        let expected: Vec<&str> = expected_rows
            .iter()
            .map(|row| at[*row].0.as_str())
            .collect();
        assert_eq!(
            ids, expected,
            "case {label:?}: window [{start}, {end}) must select {expected:?}, got {ids:?}"
        );
        // The count is of the traces the REQUESTED WINDOW holds, so it moves
        // with the window rather than reporting the whole store: a window past
        // every stored trace reports none processed.
        let in_window = i64::try_from(expected.len()).expect("the row's trace count fits");
        assert_eq!(
            got.get("TracesProcessedCount").as_i64(),
            Some(in_window),
            "case {label:?}: window [{start}, {end}) holds {in_window} trace(s), got {:?}",
            got.get("TracesProcessedCount")
        );
    }
}

// =============================================================================
// 4. The Group lifecycle, answered from the stored record
// =============================================================================

/// One group definition. Rows differ in name length (both published length
/// bounds), character set, the form of the filter expression, whether an
/// insights configuration / tags were supplied, and which selector deletes it.
///
/// Every row carries a filter expression, because X-Ray refuses a `CreateGroup`
/// that supplies none — see `group_control_plane.rs` for that rule.
struct GroupCase {
    case: &'static str,
    name: &'static str,
    filter: &'static str,
    insights: Option<(bool, bool)>,
    tags: Vec<(&'static str, &'static str)>,
    delete_by_arn: bool,
}

fn group_cases() -> Vec<GroupCase> {
    vec![
        GroupCase {
            case: "name and filter only",
            name: "alpha",
            filter: "ok",
            insights: None,
            tags: vec![],
            delete_by_arn: false,
        },
        GroupCase {
            case: "filter expression and insights",
            name: "beta",
            filter: "service(\"svc\")",
            insights: Some((true, false)),
            tags: vec![("Stage", "Production")],
            delete_by_arn: true,
        },
        GroupCase {
            // GroupName's published @length is min 1 ...
            case: "the shortest name the model allows",
            name: "a",
            filter: "fault = true",
            insights: Some((true, true)),
            tags: vec![],
            delete_by_arn: false,
        },
        GroupCase {
            // ... and max 32.
            case: "the longest name the model allows",
            name: "abcdefghijklmnopqrstuvwxyz012345",
            filter: "annotation.probe = \"edge\"",
            insights: Some((false, false)),
            tags: vec![("Team", "payments"), ("Owner", "sre")],
            delete_by_arn: true,
        },
        GroupCase {
            case: "mixed case and punctuation",
            name: "Prod-Group_1.0",
            filter: "responsetime > 2",
            insights: None,
            tags: vec![],
            delete_by_arn: false,
        },
    ]
}

/// The request body `CreateGroup` takes for a case.
fn create_body(case: &GroupCase) -> Value {
    let mut body = json!({ "GroupName": case.name, "FilterExpression": case.filter });
    if let Some((insights, notifications)) = case.insights {
        body["InsightsConfiguration"] =
            json!({"InsightsEnabled": insights, "NotificationsEnabled": notifications});
    }
    if !case.tags.is_empty() {
        body["Tags"] = Value::Array(
            case.tags
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        );
    }
    body
}

/// Every field a stored group must report, checked against the case.
fn assert_group_matches(label: &str, source: &str, group: &Value, case: &GroupCase, arn: &str) {
    assert_eq!(
        group.get("GroupName").and_then(Value::as_str),
        Some(case.name),
        "case {label:?} via {source}: expected GroupName {:?}, got {group:?}",
        case.name
    );
    assert_eq!(
        group.get("GroupARN").and_then(Value::as_str),
        Some(arn),
        "case {label:?} via {source}: expected the persisted GroupARN {arn:?}, got {group:?}"
    );
    assert_eq!(
        group.get("FilterExpression").and_then(Value::as_str),
        Some(case.filter),
        "case {label:?} via {source}: expected FilterExpression {:?}, got {group:?}",
        case.filter
    );
    match case.insights {
        Some((insights, notifications)) => {
            let configuration = group.get("InsightsConfiguration");
            assert_eq!(
                configuration
                    .and_then(|value| value.get("InsightsEnabled"))
                    .and_then(Value::as_bool),
                Some(insights),
                "case {label:?} via {source}: expected InsightsEnabled {insights}, got {configuration:?}"
            );
            assert_eq!(
                configuration
                    .and_then(|value| value.get("NotificationsEnabled"))
                    .and_then(Value::as_bool),
                Some(notifications),
                "case {label:?} via {source}: expected NotificationsEnabled {notifications}, got {configuration:?}"
            );
        }
        None => assert!(
            group
                .get("InsightsConfiguration")
                .is_none_or(Value::is_null),
            "case {label:?} via {source}: no InsightsConfiguration was supplied, got {group:?}"
        ),
    }
}

/// Every `GroupName` `GetGroups` lists, walked across pages.
async fn all_group_names(xray: &Xray) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let page = xray.get_groups(token.as_deref()).await;
        assert_eq!(page.status, 200, "GetGroups must succeed, got {page:?}");
        names.extend(
            array(&page.body, "Groups")
                .iter()
                .filter_map(|group| group.get("GroupName").and_then(Value::as_str))
                .map(str::to_string),
        );
        token = page.get("NextToken").as_str().map(str::to_string);
        if token.is_none() {
            return names;
        }
    }
}

/// `arn:aws:xray:<region>:<account>:group/<GroupName>/<12 hex digits>`.
pub(crate) fn assert_group_arn_shape(label: &str, name: &str, arn: &str) {
    let expected_prefix = format!("arn:aws:xray:us-east-1:000000000000:group/{name}/");
    assert!(
        arn.starts_with(&expected_prefix),
        "case {label:?}: expected a GroupARN starting {expected_prefix:?}, got {arn:?}"
    );
    let suffix = &arn[expected_prefix.len()..];
    assert!(
        suffix.len() == 12 && suffix.chars().all(|c| c.is_ascii_hexdigit()),
        "case {label:?}: expected a 12-hex-digit group id, got {suffix:?}"
    );
}

#[tokio::test]
async fn a_group_is_created_read_listed_and_deleted_from_its_stored_record() {
    let xray = Xray::new();
    let cases = group_cases();
    let mut arns = Vec::new();

    for case in &cases {
        let label = case.case;
        let created = xray.call("/CreateGroup", &create_body(case)).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateGroup must succeed, got {created:?}"
        );
        let arn = created
            .get("Group")
            .get("GroupARN")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_group_arn_shape(label, case.name, &arn);
        assert_group_matches(label, "CreateGroup", created.get("Group"), case, &arn);
        arns.push(arn);
    }

    // A minted ARN is unique across groups.
    let distinct: BTreeSet<&String> = arns.iter().collect();
    assert_eq!(
        distinct.len(),
        arns.len(),
        "every minted GroupARN must be distinct, got {arns:?}"
    );

    // Both selectors resolve the same stored record.
    for (case, arn) in cases.iter().zip(&arns) {
        let label = case.case;
        let by_name = xray
            .call("/GetGroup", &json!({"GroupName": case.name}))
            .await;
        assert_eq!(
            by_name.status, 200,
            "case {label:?}: GetGroup by name must succeed, got {by_name:?}"
        );
        assert_group_matches(label, "GetGroup by GroupName", by_name.get("Group"), case, arn);

        let by_arn = xray.call("/GetGroup", &json!({"GroupARN": arn})).await;
        assert_eq!(
            by_arn.status, 200,
            "case {label:?}: GetGroup by ARN must succeed, got {by_arn:?}"
        );
        assert_group_matches(label, "GetGroup by GroupARN", by_arn.get("Group"), case, arn);
        assert_eq!(
            by_name.get("Group"),
            by_arn.get("Group"),
            "case {label:?}: both selectors must resolve the same record, got {:?} and {:?}",
            by_name.get("Group"),
            by_arn.get("Group")
        );
    }

    // GetGroups lists every created group, plus the reserved `Default` group
    // every account/region holds. Six groups exceed one page, so this walks the
    // pages.
    let listed_names = all_group_names(&xray).await;
    let mut expected_names: BTreeSet<String> =
        cases.iter().map(|case| case.name.to_string()).collect();
    expected_names.insert("Default".to_string());
    assert_eq!(
        listed_names, expected_names,
        "GetGroups must list every stored group, got {listed_names:?}"
    );

    // Deleting removes the record: GetGroup fails and GetGroups stops listing it.
    for (case, arn) in cases.iter().zip(&arns) {
        let label = case.case;
        let body = if case.delete_by_arn {
            json!({"GroupARN": arn})
        } else {
            json!({"GroupName": case.name})
        };
        let deleted = xray.call("/DeleteGroup", &body).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteGroup must succeed, got {deleted:?}"
        );

        let gone = xray
            .call("/GetGroup", &json!({"GroupName": case.name}))
            .await;
        assert_eq!(
            (gone.status, gone.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: GetGroup on a deleted group must be refused, got {gone:?}"
        );
        let gone_by_arn = xray.call("/GetGroup", &json!({"GroupARN": arn})).await;
        assert_eq!(
            gone_by_arn.status, 400,
            "case {label:?}: the deleted group's ARN must resolve to nothing, got {gone_by_arn:?}"
        );
        let again = xray.call("/DeleteGroup", &body).await;
        assert_eq!(
            (again.status, again.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: deleting an unknown group must be refused, got {again:?}"
        );
    }

    let empty = xray.get_groups(None).await;
    let remaining: Vec<&str> = array(&empty.body, "Groups")
        .iter()
        .filter_map(|group| group.get("GroupName").and_then(Value::as_str))
        .collect();
    assert_eq!(
        remaining,
        vec!["Default"],
        "every created group was deleted, so only the reserved Default group is left, got {remaining:?}"
    );

    // Re-creating a deleted name mints a NEW ARN, and the superseded one still
    // resolves to nothing: the ARN index never outlives the record it indexed.
    for (case, old_arn) in cases.iter().zip(&arns) {
        let label = case.case;
        let recreated = xray.call("/CreateGroup", &create_body(case)).await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: re-creating a deleted name must succeed, got {recreated:?}"
        );
        let new_arn = recreated
            .get("Group")
            .get("GroupARN")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_group_arn_shape(label, case.name, &new_arn);
        assert_ne!(
            &new_arn, old_arn,
            "case {label:?}: a re-created group must not reuse the superseded ARN, got {new_arn:?}"
        );
        let stale = xray.call("/GetGroup", &json!({"GroupARN": old_arn})).await;
        assert_eq!(
            (stale.status, stale.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: the superseded ARN must resolve to nothing, got {stale:?}"
        );
        let fresh = xray.call("/GetGroup", &json!({"GroupARN": &new_arn})).await;
        assert_eq!(
            fresh.status, 200,
            "case {label:?}: the new ARN must resolve to the new record, got {fresh:?}"
        );
    }
}

// =============================================================================
// 5. The @paginated contract on all three paginated operations
// =============================================================================

#[tokio::test]
async fn a_next_token_appears_only_when_truncated_and_resumes_exactly_once() {
    use service_com_amazonaws_xray_awsxray::ops::get_groups::GROUPS_PAGE_SIZE;
    use service_com_amazonaws_xray_awsxray::ops::get_trace_summaries::TRACE_SUMMARIES_PAGE_SIZE;

    // Counts either side of one page, so a row exists where no token is due and
    // rows where several pages are.
    for count in [
        0,
        1,
        GROUPS_PAGE_SIZE,
        GROUPS_PAGE_SIZE + 1,
        GROUPS_PAGE_SIZE * 2 + 2,
    ] {
        let xray = Xray::new();
        // The reserved `Default` group is listed beside the created ones, so it
        // belongs in the expected listing of every row.
        let mut expected: Vec<String> = vec!["Default".to_string()];
        for index in 0..count {
            let name = format!("group-{index:02}");
            let created = xray
                .call(
                    "/CreateGroup",
                    &json!({"GroupName": name, "FilterExpression": "ok"}),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "count {count}: seeding {name} must succeed, got {created:?}"
            );
            expected.push(name);
        }
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = xray.get_groups(token.as_deref()).await;
            assert_eq!(
                page.status, 200,
                "count {count}: GetGroups page {pages} must succeed, got {page:?}"
            );
            let names: Vec<String> = array(&page.body, "Groups")
                .iter()
                .filter_map(|group| group.get("GroupName").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            assert!(
                names.len() <= GROUPS_PAGE_SIZE,
                "count {count}: a page holds at most {GROUPS_PAGE_SIZE}, got {names:?}"
            );
            seen.extend(names);
            token = page
                .get("NextToken")
                .as_str()
                .map(str::to_string);
            assert_eq!(
                token.is_some(),
                seen.len() < expected.len(),
                "count {count}: a NextToken is due only while results remain, got {token:?} after {} of {}",
                seen.len(),
                expected.len()
            );
            pages += 1;
            assert!(pages <= 10, "count {count}: pagination must terminate");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "count {count}: paging must yield every group exactly once in order, got {seen:?}"
        );
    }

    // The same contract over GetTraceSummaries, and over BatchGetTraces, where
    // the page is the requested-id list and missing ids are reported alongside.
    let xray = Xray::new();
    let traces = TRACE_SUMMARIES_PAGE_SIZE * 2 + 2;
    let mut stored: Vec<String> = Vec::new();
    for index in 0..traces {
        #[allow(clippy::cast_possible_truncation)]
        let trace = trace_id(40 + index as u32);
        let document = root_document(
            &format!("{:016x}", index + 1),
            &trace,
            "svc",
            5_000.0,
            Some(5_000.5),
        )
        .to_string();
        let put = xray.put_segments(&[document]).await;
        assert_eq!(put.status, 200, "seeding {trace} must succeed, got {put:?}");
        stored.push(trace);
    }
    stored.sort();

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = xray.summaries(4_999.0, 5_001.0, token.as_deref()).await;
        assert_eq!(
            page.status, 200,
            "GetTraceSummaries page must succeed, got {page:?}"
        );
        let ids: Vec<String> = array(&page.body, "TraceSummaries")
            .iter()
            .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        assert!(
            ids.len() <= TRACE_SUMMARIES_PAGE_SIZE,
            "a page holds at most {TRACE_SUMMARIES_PAGE_SIZE} summaries, got {ids:?}"
        );
        seen.extend(ids);
        token = page.get("NextToken").as_str().map(str::to_string);
        assert_eq!(
            token.is_some(),
            seen.len() < stored.len(),
            "a NextToken is due only while traces remain, got {token:?} after {} of {}",
            seen.len(),
            stored.len()
        );
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, stored,
        "paging GetTraceSummaries must yield every trace exactly once, got {seen:?}"
    );

    // The same contract over BatchGetTraces, where one call carries at most
    // `MAX_TRACE_IDS_PER_CALL` ids: the stored ones come back and the ones that
    // were never ingested are simply not answered for.
    let requested: Vec<String> = stored
        .iter()
        .take(2)
        .cloned()
        .chain(vec![
            "1-00000000-000000000000000000000000".to_string(),
            "1-00000000-000000000000000000000001".to_string(),
            "1-00000000-000000000000000000000002".to_string(),
        ])
        .collect();

    let mut found: Vec<String> = Vec::new();
    let mut unprocessed: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = xray.batch_get(&requested, token.as_deref()).await;
        assert_eq!(
            page.status, 200,
            "BatchGetTraces page must succeed, got {page:?}"
        );
        found.extend(
            array(&page.body, "Traces")
                .iter()
                .filter_map(|trace| trace.get("Id").and_then(Value::as_str))
                .map(str::to_string),
        );
        unprocessed.extend(
            array(&page.body, "UnprocessedTraceIds")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
        token = page.get("NextToken").as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        found,
        stored[..2].to_vec(),
        "BatchGetTraces must return every stored trace it was asked for exactly once, got {found:?}"
    );
    assert!(
        unprocessed.is_empty(),
        "a well-formed id with no stored segments is a miss, not an unprocessed id, got {unprocessed:?}"
    );
}

// =============================================================================
// 6. Refusals carry 400, with the code that class of refusal is answered with:
//    the service's own `InvalidRequestException` for a rule this service
//    enforces, and the protocol's `ValidationException` for a violated
//    MODELLED constraint (see `group_control_plane.rs` for that rule over a
//    table of its own).
// =============================================================================

#[tokio::test]
async fn a_request_the_service_cannot_satisfy_is_refused_as_invalid() {
    let xray = Xray::new();
    // One group exists, so the duplicate-name row has something to collide with.
    let seeded = xray
        .call(
            "/CreateGroup",
            &json!({"GroupName": "taken", "FilterExpression": "ok"}),
        )
        .await;
    assert_eq!(seeded.status, 200, "seeding must succeed, got {seeded:?}");

    // Rows span the ways a request can be invalid: a modelled bound (both ends),
    // an absent required member, a name already in use, a selector naming
    // nothing, no selector at all, and a token this service never issued.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "GroupName shorter than the model's minimum",
            "/CreateGroup",
            json!({"GroupName": ""}),
            "ValidationException",
        ),
        (
            "GroupName longer than the model's maximum",
            "/CreateGroup",
            json!({"GroupName": "a".repeat(33)}),
            "ValidationException",
        ),
        (
            "GroupName absent",
            "/CreateGroup",
            json!({"FilterExpression": "fault = true"}),
            "ValidationException",
        ),
        (
            "GroupName already in use",
            "/CreateGroup",
            json!({"GroupName": "taken", "FilterExpression": "ok"}),
            "InvalidRequestException",
        ),
        (
            "GetGroup with neither selector",
            "/GetGroup",
            json!({}),
            "InvalidRequestException",
        ),
        (
            "GetGroup naming no stored group",
            "/GetGroup",
            json!({"GroupName": "absent"}),
            "InvalidRequestException",
        ),
        (
            "GetGroup naming no stored ARN",
            "/GetGroup",
            json!({"GroupARN": "arn:aws:xray:us-east-1:000000000000:group/absent/0123456789ab"}),
            "InvalidRequestException",
        ),
        (
            "DeleteGroup with neither selector",
            "/DeleteGroup",
            json!({}),
            "InvalidRequestException",
        ),
        (
            "DeleteGroup on the reserved Default group",
            "/DeleteGroup",
            json!({"GroupName": "Default"}),
            "InvalidRequestException",
        ),
        (
            "TraceSegmentDocuments absent",
            "/TraceSegments",
            json!({}),
            "ValidationException",
        ),
        (
            "TraceIds absent",
            "/Traces",
            json!({}),
            "ValidationException",
        ),
        (
            "a NextToken this service never issued",
            "/Traces",
            json!({"TraceIds": [trace_id(1)], "NextToken": "not-a-token"}),
            "InvalidRequestException",
        ),
        (
            "a NextToken past the end of the requested TraceIds",
            "/Traces",
            json!({"TraceIds": [trace_id(1)], "NextToken": "7"}),
            "InvalidRequestException",
        ),
        (
            "more TraceIds than one call accepts",
            "/Traces",
            json!({
                "TraceIds": (0..=service_com_amazonaws_xray_awsxray::state::MAX_TRACE_IDS_PER_CALL)
                    .map(|n| {
                        #[allow(clippy::cast_possible_truncation)]
                        let n = n as u32;
                        trace_id(n % 100)
                    })
                    .collect::<Vec<String>>(),
            }),
            "InvalidRequestException",
        ),
        (
            "a GetGroups NextToken beyond the listed keyspace",
            "/Groups",
            json!({"NextToken": "zzz-never-issued"}),
            "InvalidRequestException",
        ),
        (
            "a GetTraceSummaries NextToken naming no stored trace",
            "/TraceSummaries",
            json!({"StartTime": 0.0_f64, "EndTime": 1.0_f64, "NextToken": trace_id(99)}),
            "InvalidRequestException",
        ),
        (
            "StartTime absent",
            "/TraceSummaries",
            json!({"EndTime": 1.0_f64}),
            "ValidationException",
        ),
        // Behaviour this slice does not implement is refused, not answered with
        // an unfiltered 200, and so is a request that cannot be satisfied at
        // all.
        (
            "a FilterExpression that is not of the language",
            "/TraceSummaries",
            json!({"StartTime": 0.0_f64, "EndTime": 1.0_f64, "FilterExpression": "fault = true AND ("}),
            "InvalidRequestException",
        ),
        (
            "an EndTime that precedes StartTime",
            "/TraceSummaries",
            json!({"StartTime": 2.0_f64, "EndTime": 1.0_f64}),
            "InvalidRequestException",
        ),
        (
            "a trace id that is not of the documented form",
            "/Traces",
            json!({"TraceIds": ["not-a-trace-id"]}),
            "InvalidRequestException",
        ),
        (
            "TimeRangeType outside the model's value set",
            "/TraceSummaries",
            json!({"StartTime": 0.0_f64, "EndTime": 1.0_f64, "TimeRangeType": "Whenever"}),
            "ValidationException",
        ),
        (
            // `Sampling` on its own is SERVED (a subset of the matching traces,
            // at the read's own rate), so it is not a row here; a sampling
            // STRATEGY is admitted only under the range type that takes one.
            "a SamplingStrategy under a time-range type that does not take one",
            "/TraceSummaries",
            json!({
                "StartTime": 0.0_f64,
                "EndTime": 1.0_f64,
                "SamplingStrategy": {"Name": "FixedRate", "Value": 0.5_f64},
            }),
            "InvalidRequestException",
        ),
    ];

    for (label, path, body, code) in rows {
        let refused = xray.call(path, &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some(code)),
            "case {label:?}: expected 400 {code}, got {refused:?}"
        );
        // A framework refusal renders `message`; a modelled error shape renders
        // its member's own casing (`Message`).
        let message = refused
            .get("message")
            .as_str()
            .or_else(|| refused.get("Message").as_str());
        assert!(
            message.is_some_and(|message| !message.is_empty()),
            "case {label:?}: a refusal must say what was wrong, got {refused:?}"
        );
    }

    // And the refused CreateGroup rows stored nothing: the seeded group and the
    // reserved `Default` group are all that is listed.
    let listed = xray.get_groups(None).await;
    let names: Vec<&str> = array(&listed.body, "Groups")
        .iter()
        .filter_map(|group| group.get("GroupName").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names,
        vec!["Default", "taken"],
        "a refused CreateGroup must store nothing, got {names:?}"
    );
}

// =============================================================================
// 8. Caller-driven work is bounded, and the supported options are accepted
// =============================================================================

#[tokio::test]
async fn a_batch_at_the_cap_is_ingested_and_one_past_it_is_refused() {
    use service_com_amazonaws_xray_awsxray::state::MAX_SEGMENT_DOCUMENTS_PER_CALL;

    // Rows either side of the per-call document cap.
    for (label, count, expect_status) in [
        ("one document", 1, 200),
        ("one short of the cap", MAX_SEGMENT_DOCUMENTS_PER_CALL - 1, 200),
        ("exactly the cap", MAX_SEGMENT_DOCUMENTS_PER_CALL, 200),
        ("one past the cap", MAX_SEGMENT_DOCUMENTS_PER_CALL + 1, 400),
    ] {
        let xray = Xray::new();
        let trace = trace_id(70);
        let documents: Vec<String> = (0..count)
            .map(|index| {
                let mut document =
                    root_document(&format!("{:016x}", index + 1), &trace, "svc", 7_000.0, Some(7_000.5));
                if index > 0 {
                    document["parent_id"] = json!("0000000000000001");
                }
                document.to_string()
            })
            .collect();

        let put = xray.put_segments(&documents).await;
        assert_eq!(
            put.status, expect_status,
            "case {label:?}: {count} documents must answer {expect_status}, got {put:?}"
        );
        if expect_status == 400 {
            assert_eq!(
                put.error_type.as_deref(),
                Some("InvalidRequestException"),
                "case {label:?}: an over-long batch is an invalid request, got {put:?}"
            );
            let read = xray.batch_get(std::slice::from_ref(&trace), None).await;
            assert!(
                array(&read.body, "Traces").is_empty(),
                "case {label:?}: a refused batch must store nothing, got {read:?}"
            );
            continue;
        }

        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "case {label:?}: every document was well formed, got {:?}",
            put.get("UnprocessedTraceSegments")
        );
        let read = xray.batch_get(std::slice::from_ref(&trace), None).await;
        let segments = array(&array(&read.body, "Traces")[0], "Segments");
        assert_eq!(
            segments.len(),
            count,
            "case {label:?}: every accepted document must be stored, got {} segments",
            segments.len()
        );
    }
}

#[tokio::test]
async fn the_options_this_slice_does_serve_are_accepted() {
    use service_com_amazonaws_xray_awsxray::ops::get_trace_summaries::TIME_RANGE_TYPE_TRACE_ID;

    let xray = Xray::new();
    let trace = trace_id(80);
    let document = root_document("abcdefabcdefabcd", &trace, "svc", 8_000.0, Some(8_000.5))
        .to_string();
    let put = xray.put_segments(std::slice::from_ref(&document)).await;
    assert_eq!(put.status, 200, "seeding must succeed, got {put:?}");

    // Rows: the trace-id TimeRangeType, sampling explicitly declined, an
    // empty FilterExpression, and one that selects the stored trace — each
    // selects it. (The other two `TimeRangeType` members window a different
    // instant of a trace, which `time_range_type.rs` covers row by row.)
    let rows: Vec<(&str, Value)> = vec![
        (
            "the trace-id TimeRangeType",
            json!({"StartTime": 7_999.0_f64, "EndTime": 8_001.0_f64, "TimeRangeType": TIME_RANGE_TYPE_TRACE_ID}),
        ),
        (
            "sampling explicitly declined",
            json!({"StartTime": 7_999.0_f64, "EndTime": 8_001.0_f64, "Sampling": false}),
        ),
        (
            "an empty FilterExpression",
            json!({"StartTime": 7_999.0_f64, "EndTime": 8_001.0_f64, "FilterExpression": ""}),
        ),
        (
            "a FilterExpression the stored trace satisfies",
            json!({"StartTime": 7_999.0_f64, "EndTime": 8_001.0_f64, "FilterExpression": "service(\"svc\")"}),
        ),
    ];

    for (label, body) in rows {
        let got = xray.call("/TraceSummaries", &body).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: this option is served, got {got:?}"
        );
        let ids: Vec<&str> = array(&got.body, "TraceSummaries")
            .iter()
            .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
            .collect();
        assert_eq!(
            ids,
            vec![trace.as_str()],
            "case {label:?}: the stored trace must be selected, got {ids:?}"
        );
    }
}


#[tokio::test]
async fn a_trace_at_its_segment_cap_refuses_more_and_reports_limit_exceeded() {
    use service_com_amazonaws_xray_awsxray::state::{
        MAX_SEGMENT_DOCUMENTS_PER_CALL, MAX_SEGMENTS_PER_TRACE,
    };

    let xray = Xray::new();
    let trace = trace_id(90);

    // Fill the trace to its cap, in batches of the per-call maximum.
    let mut stored = 0usize;
    while stored < MAX_SEGMENTS_PER_TRACE {
        let batch = MAX_SEGMENT_DOCUMENTS_PER_CALL.min(MAX_SEGMENTS_PER_TRACE - stored);
        let documents: Vec<String> = (0..batch)
            .map(|index| {
                let position = stored + index;
                let mut document = root_document(
                    &format!("{:016x}", position + 1),
                    &trace,
                    "svc",
                    9_000.0,
                    Some(9_000.5),
                );
                if position > 0 {
                    document["parent_id"] = json!("0000000000000001");
                }
                document.to_string()
            })
            .collect();
        let put = xray.put_segments(&documents).await;
        assert_eq!(
            put.status, 200,
            "filling to {MAX_SEGMENTS_PER_TRACE} must succeed, got {put:?}"
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "a batch within the cap must be accepted whole, got {:?}",
            put.get("UnprocessedTraceSegments")
        );
        stored += batch;
    }

    // Up to here nothing was dropped.
    let full = xray.batch_get(std::slice::from_ref(&trace), None).await;
    let returned = array(&full.body, "Traces");
    assert_eq!(
        array(&returned[0], "Segments").len(),
        MAX_SEGMENTS_PER_TRACE,
        "a trace at its cap is still returned whole, got {} segments",
        array(&returned[0], "Segments").len()
    );
    assert_eq!(
        returned[0].get("LimitExceeded").and_then(Value::as_bool),
        Some(false),
        "nothing was dropped yet, got {:?}",
        returned[0]
    );

    // One more NEW segment is refused, and the trace now says so.
    let overflow = root_document("ffffffffffffff01", &trace, "svc", 9_000.0, Some(9_000.9));
    let put = xray
        .put_segments(std::slice::from_ref(&overflow.to_string()))
        .await;
    assert_eq!(put.status, 200, "the call still succeeds, got {put:?}");
    let unprocessed = array(&put.body, "UnprocessedTraceSegments");
    assert_eq!(
        unprocessed.len(),
        1,
        "the segment past the cap must be reported, got {unprocessed:?}"
    );
    assert_eq!(
        unprocessed[0].get("ErrorCode").and_then(Value::as_str),
        Some("LimitExceeded"),
        "expected the limit code, got {:?}",
        unprocessed[0]
    );

    let after = xray.batch_get(std::slice::from_ref(&trace), None).await;
    let returned = array(&after.body, "Traces");
    assert_eq!(
        array(&returned[0], "Segments").len(),
        MAX_SEGMENTS_PER_TRACE,
        "the refused segment must not be stored, got {} segments",
        array(&returned[0], "Segments").len()
    );
    assert_eq!(
        returned[0].get("LimitExceeded").and_then(Value::as_bool),
        Some(true),
        "a trace that dropped a segment reports LimitExceeded, got {:?}",
        returned[0]
    );
}

#[tokio::test]
async fn the_trace_store_evicts_its_least_recently_ingested_trace_when_full() {
    use service_com_amazonaws_xray_awsxray::state::{
        MAX_SEGMENT_DOCUMENTS_PER_CALL, MAX_TRACES,
    };

    /// A distinct 35-character trace id (`1-` + 8 hex + `-` + 24 hex) per `n`.
    fn wide_trace_id(n: usize) -> String {
        format!("1-{:08x}-a006649127e371903a{:06x}", n, n & 0xff_ffff)
    }

    let xray = Xray::new();
    // One trace past the cap, so exactly one eviction is due.
    let total = MAX_TRACES + 1;
    let mut ingested: Vec<String> = Vec::new();
    let mut position = 0usize;
    while position < total {
        let batch = MAX_SEGMENT_DOCUMENTS_PER_CALL.min(total - position);
        let documents: Vec<String> = (0..batch)
            .map(|index| {
                let trace = wide_trace_id(position + index);
                ingested.push(trace.clone());
                root_document("0000000000000001", &trace, "svc", 11_000.0, Some(11_000.5))
                    .to_string()
            })
            .collect();
        let put = xray.put_segments(&documents).await;
        assert_eq!(
            put.status, 200,
            "seeding traces {position}.. must succeed, got {put:?}"
        );
        position += batch;
    }

    let first = ingested.first().expect("at least one trace").clone();
    let last = ingested.last().expect("at least one trace").clone();
    let read = xray.batch_get(&[first.clone(), last.clone()], None).await;
    let returned: Vec<&str> = array(&read.body, "Traces")
        .iter()
        .filter_map(|trace| trace.get("Id").and_then(Value::as_str))
        .collect();
    assert_eq!(
        returned,
        vec![last.as_str()],
        "the most recently ingested trace must still be stored, got {returned:?}"
    );
    assert!(
        array(&read.body, "UnprocessedTraceIds").is_empty(),
        "an evicted trace is a miss like any other, not an unprocessed id, got {:?}",
        read.get("UnprocessedTraceIds")
    );
    let summaries = xray.summaries(10_999.0, 11_001.0, None).await;
    let count = summaries.get("TracesProcessedCount").as_i64();
    #[allow(clippy::cast_possible_wrap)]
    let expected = MAX_TRACES as i64;
    assert_eq!(
        count,
        Some(expected),
        "ingesting past {MAX_TRACES} traces evicts the least recently ingested one, so {MAX_TRACES} remain, got {count:?}"
    );
}

// =============================================================================
// 7. Re-submitting a segment id overwrites the stored document
// =============================================================================

#[tokio::test]
async fn a_completed_segment_overwrites_the_in_progress_one_it_replaces() {
    let xray = Xray::new();
    let trace = trace_id(60);
    let in_progress = root_document("9999999999999999", &trace, "svc", 6_000.0, None).to_string();
    let complete =
        root_document("9999999999999999", &trace, "svc", 6_000.0, Some(6_000.75)).to_string();

    let put = xray.put_segments(std::slice::from_ref(&in_progress)).await;
    assert_eq!(put.status, 200, "the in-progress put must succeed, got {put:?}");
    let partial = xray.summaries(5_999.0, 6_001.0, None).await;
    assert_eq!(
        array(&partial.body, "TraceSummaries")[0]
            .get("IsPartial")
            .and_then(Value::as_bool),
        Some(true),
        "an in-progress segment makes the trace partial, got {partial:?}"
    );

    let put = xray.put_segments(std::slice::from_ref(&complete)).await;
    assert_eq!(put.status, 200, "the completing put must succeed, got {put:?}");

    let read = xray.batch_get(std::slice::from_ref(&trace), None).await;
    let segments = array(&array(&read.body, "Traces")[0], "Segments");
    assert_eq!(
        segments.len(),
        1,
        "the completed document replaces the in-progress one under the same id, got {segments:?}"
    );
    assert_eq!(
        segments[0].get("Document").and_then(Value::as_str),
        Some(complete.as_str()),
        "the stored document must be the completing submission, got {segments:?}"
    );

    let done = xray.summaries(5_999.0, 6_001.0, None).await;
    let summary = &array(&done.body, "TraceSummaries")[0];
    assert_eq!(
        summary.get("IsPartial").and_then(Value::as_bool),
        Some(false),
        "the completed trace is no longer partial, got {summary:?}"
    );
    let duration = summary.get("Duration").and_then(Value::as_f64);
    assert!(
        duration.is_some_and(|actual| close(actual, 0.75)),
        "the duration must come from the completing document, got {duration:?}"
    );
}
