// SPDX-License-Identifier: CC0-1.0

//! The rules the X-Ray trace data plane follows, over the wire.
//!
//! Each test is a TABLE whose rows differ along what the rule it checks
//! generalises over — the number of requested ids, the shape of a submitted
//! document, which outcome fields it sets, the expression a query carries — and
//! the rules are:
//!
//! * `BatchGetTraces` retrieves at most five traces per call, and a longer list
//!   of ids, or one holding an id that is not a well-formed trace id, is refused
//!   as an invalid request;
//! * a well-formed id with nothing stored under it is a MISS: it is neither
//!   answered for in `Traces` nor reported in `UnprocessedTraceIds`, which is
//!   for ids the service failed to process;
//! * a subsegment that names a downstream service (`namespace: "remote"`) is
//!   answered for by an extra, service-synthesised top-level segment carrying
//!   `"inferred": true` whose `parent_id` is that subsegment's id, and every
//!   submitted document comes back byte for byte;
//! * each entity of a trace is classified EXCLUSIVELY as a fault, a throttle, an
//!   error or none of them — so a document setting both `throttle` and `error`
//!   is a throttle and not also an error — and each flag is then read in the
//!   scope the model documents for it: `HasFault`/`HasError` off the ROOT
//!   segment document, `HasThrottle` off any of them;
//! * an entity's `cause` block becomes a root cause of the class its outcome
//!   was classified as, naming the services and frames it travelled through and
//!   the exceptions the document named;
//! * a document the service cannot read is reported as one
//!   `UnprocessedTraceSegment`, identified by the document's own `Id`, whose
//!   `ErrorCode` names the fault and whose `Message` restates that code, and the
//!   valid members of the same batch still land;
//! * a `FilterExpression` selects the subset of the traces in
//!   [`StartTime`, `EndTime`) that it matches, and every trace in the window is
//!   still counted by `TracesProcessedCount`;
//! * an expression the language cannot read is refused with
//!   `InvalidRequestException` by BOTH surfaces that take one
//!   (`GetTraceSummaries` and `CreateGroup`);
//! * an `EndTime` that precedes `StartTime` is refused.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::xray_wire::{array, close, root_document, trace_id, Answer, Xray};

/// The window every seeded trace below falls inside.
const WINDOW: (f64, f64) = (1_000.0, 2_000.0);

/// Assert a refusal carries the modelled `InvalidRequestException` at HTTP 400
/// and says something.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "case {case:?}: expected 400 InvalidRequestException, got {answer:?}"
    );
    assert!(
        answer.message().is_some_and(|message| !message.is_empty()),
        "case {case:?}: a refusal must say what was wrong, got {answer:?}"
    );
}

/// Assert a request was refused at HTTP 400, by the handler's modelled
/// `InvalidRequestException` or by the boundary's `ValidationException` — which
/// one depends on whether the value also breaks a MODELLED constraint, and that
/// is the boundary's call, not this service's.
fn assert_refused_at_400(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    let refusal = answer.error_type.as_deref().unwrap_or_default();
    assert!(
        matches!(refusal, "InvalidRequestException" | "ValidationException"),
        "case {case:?}: expected a modelled refusal, got {answer:?}"
    );
    assert!(
        answer.message().is_some_and(|message| !message.is_empty()),
        "case {case:?}: a refusal must say what was wrong, got {answer:?}"
    );
}

// =============================================================================
// 1. BatchGetTraces retrieves at most five traces per call
// =============================================================================

#[tokio::test]
async fn batch_get_traces_accepts_at_most_five_trace_ids() {
    use service_com_amazonaws_xray_awsxray::state::MAX_TRACE_IDS_PER_CALL;

    // The bound AWS publishes for BatchGetTraces: five traces per request.
    assert_eq!(
        MAX_TRACE_IDS_PER_CALL, 5,
        "BatchGetTraces retrieves at most five traces per call"
    );

    let xray = Xray::new();
    let stored = trace_id(1);
    let put = xray
        .put_segments(&[
            root_document("0000000000000001", &stored, "svc", 1_000.0, Some(1_000.5)).to_string()
        ])
        .await;
    assert_eq!(put.status, 200, "seeding must succeed, got {put:?}");

    // Rows either side of the bound, including both the bound itself and one
    // past it.
    for count in 1..=8usize {
        let ids: Vec<String> = (0..count)
            .map(|index| {
                #[allow(clippy::cast_possible_truncation)]
                let index = index as u32;
                trace_id(index + 1)
            })
            .collect();
        let answer = xray.batch_get(&ids, None).await;
        let case = format!("{count} trace ids");
        if count <= MAX_TRACE_IDS_PER_CALL {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a call within the bound is served, got {answer:?}"
            );
            assert_eq!(
                array(&answer.body, "Traces").len(),
                1,
                "case {case:?}: only the one stored trace is answered for, got {:?}",
                answer.get("Traces")
            );
        } else {
            assert_refused(&case, &answer);
        }
    }
}

#[tokio::test]
async fn a_trace_id_that_is_not_well_formed_is_refused() {
    let xray = Xray::new();

    // A trace id is `1-<8 hex>-<24 hex>` — the three-part form the
    // PutTraceSegments documentation states (version, 8 hexadecimal digits of
    // request time, 24 hexadecimal digits of unique id). Rows walk the ways a
    // value misses it, plus two that hit it.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("the documented example", "1-58406520-a006649127e371903a2de979", true),
        ("upper-case hexadecimal", "1-58406520-A006649127E371903A2DE979", true),
        ("no version prefix", "58406520-a006649127e371903a2de979", false),
        ("a version other than 1", "2-58406520-a006649127e371903a2de979", false),
        ("a short time field", "1-5840652-a006649127e371903a2de979", false),
        ("a long time field", "1-584065200-a006649127e371903a2de979", false),
        ("a short unique field", "1-58406520-a006649127e371903a2de97", false),
        ("a long unique field", "1-58406520-a006649127e371903a2de9790", false),
        ("a non-hexadecimal digit", "1-58406520-a006649127e371903a2de97z", false),
        ("a fourth field", "1-58406520-a006649127e371903a2de979-1", false),
        ("not an id at all", "not-a-trace-id", false),
        ("empty", "", false),
    ];

    for (case, id, well_formed) in rows {
        // On the read side a malformed id is a refusal, not an empty answer.
        let read = xray.batch_get(&[id.to_string()], None).await;
        if well_formed {
            assert_eq!(
                read.status, 200,
                "case {case:?}: {id:?} is a trace id, so the call is served, got {read:?}"
            );
            assert!(
                array(&read.body, "Traces").is_empty()
                    && array(&read.body, "UnprocessedTraceIds").is_empty(),
                "case {case:?}: nothing is stored under {id:?}, got {read:?}"
            );
        } else {
            // A value the MODEL also refuses (`TraceId` is 1..35 characters) is
            // refused by the boundary before the handler sees it; either way the
            // call is refused rather than answered as a miss.
            assert_refused_at_400(case, &read);
        }

        // And on the ingest side it is one unprocessed segment, identified by
        // the document's own id, while the rest of the batch lands.
        let xray = Xray::new();
        let good_trace = trace_id(9);
        let good =
            root_document("aaaaaaaaaaaaaaaa", &good_trace, "ok", 1_000.0, Some(1_000.5)).to_string();
        let submitted =
            root_document("bbbbbbbbbbbbbbbb", id, "bad", 1_000.0, Some(1_000.5)).to_string();
        let put = xray.put_segments(&[good, submitted]).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: one bad document must not fail the call, got {put:?}"
        );
        let unprocessed = array(&put.body, "UnprocessedTraceSegments");
        if well_formed {
            assert!(
                unprocessed.is_empty(),
                "case {case:?}: {id:?} is a trace id, so the document lands, got {unprocessed:?}"
            );
            continue;
        }
        assert_eq!(
            unprocessed.len(),
            1,
            "case {case:?}: exactly the malformed document is unprocessed, got {unprocessed:?}"
        );
        assert_eq!(
            unprocessed[0].get("Id").and_then(Value::as_str),
            Some("bbbbbbbbbbbbbbbb"),
            "case {case:?}: the rejection names the document's own segment Id, got {:?}",
            unprocessed[0]
        );
        assert_eq!(
            unprocessed[0].get("ErrorCode").and_then(Value::as_str),
            Some("InvalidTraceId"),
            "case {case:?}: the code names the member at fault, got {:?}",
            unprocessed[0]
        );
        assert_eq!(
            unprocessed[0].get("Message").and_then(Value::as_str),
            Some("Invalid segment. ErrorCode: InvalidTraceId"),
            "case {case:?}: the Message restates the ErrorCode, got {:?}",
            unprocessed[0]
        );
        let read = xray.batch_get(std::slice::from_ref(&good_trace), None).await;
        assert_eq!(
            array(&read.body, "Traces").len(),
            1,
            "case {case:?}: the valid member of the batch still landed, got {read:?}"
        );
    }
}

// =============================================================================
// 2. A well-formed id with nothing stored is a miss, not an unprocessed id
// =============================================================================

#[tokio::test]
async fn a_document_nested_deeper_than_the_bound_is_refused_per_document() {
    use service_com_amazonaws_xray_awsxray::state::MAX_SUBSEGMENT_DEPTH;

    // Every reader walks a document's `subsegments` tree recursively, so its
    // DEPTH is caller-driven work on the read path: a document nested deeper
    // than the bound is refused at ingest, like any other member the service
    // cannot use, and the valid members of the same batch still land. Rows walk
    // either side of the bound.
    for levels in [1, MAX_SUBSEGMENT_DEPTH, MAX_SUBSEGMENT_DEPTH + 1] {
        let xray = Xray::new();
        let good_trace = trace_id(16);
        let good =
            root_document("aaaaaaaaaaaaaaaa", &good_trace, "ok", 1_000.0, Some(1_000.5)).to_string();

        let trace = trace_id(17);
        let mut nested = json!([]);
        for level in (0..levels).rev() {
            let mut subsegment = json!({
                "id": format!("{:016x}", level + 1),
                "name": format!("level-{level}"),
                "start_time": 1_000.1_f64,
                "end_time": 1_000.4_f64,
            });
            if !nested.as_array().is_some_and(Vec::is_empty) {
                subsegment["subsegments"] = nested;
            }
            nested = json!([subsegment]);
        }
        let mut document =
            root_document("1111111111111111", &trace, "client", 1_000.0, Some(1_000.5));
        document["subsegments"] = nested;

        let put = xray
            .put_segments(&[good, document.to_string()])
            .await;
        assert_eq!(
            put.status, 200,
            "case {levels} levels: one refused document must not fail the call, got {put:?}"
        );
        let unprocessed = array(&put.body, "UnprocessedTraceSegments");
        let read = xray.batch_get(&[trace.clone(), good_trace.clone()], None).await;
        let stored: Vec<&str> = array(&read.body, "Traces")
            .iter()
            .filter_map(|answered| answered.get("Id").and_then(Value::as_str))
            .collect();
        if levels <= MAX_SUBSEGMENT_DEPTH {
            assert!(
                unprocessed.is_empty(),
                "case {levels} levels: nesting within the bound is stored, got {unprocessed:?}"
            );
            assert_eq!(
                stored.len(),
                2,
                "case {levels} levels: both documents landed, got {stored:?}"
            );
        } else {
            assert_eq!(
                unprocessed.len(),
                1,
                "case {levels} levels: exactly the over-nested document is unprocessed, got {unprocessed:?}"
            );
            assert_eq!(
                unprocessed[0].get("ErrorCode").and_then(Value::as_str),
                Some("InvalidSubsegments"),
                "case {levels} levels: the code names the member at fault, got {:?}",
                unprocessed[0]
            );
            assert_eq!(
                stored,
                vec![good_trace.as_str()],
                "case {levels} levels: only the valid member of the batch landed, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_id_with_nothing_stored_is_omitted_rather_than_reported() {
    // Rows differ in how many of the requested ids are stored: none, some, all.
    let rows: Vec<(&str, usize, usize)> = vec![
        ("no id is stored", 0, 3),
        ("one of three is stored", 1, 3),
        ("two of five are stored", 2, 5),
        ("every requested id is stored", 4, 4),
        ("the whole call is misses", 0, 5),
    ];

    for (case, stored_count, requested_count) in rows {
        let xray = Xray::new();
        let stored: Vec<String> = (0..stored_count)
            .map(|index| {
                #[allow(clippy::cast_possible_truncation)]
                let index = index as u32;
                trace_id(index + 1)
            })
            .collect();
        for (index, trace) in stored.iter().enumerate() {
            let document = root_document(
                &format!("{:016x}", index + 1),
                trace,
                "svc",
                1_000.0,
                Some(1_000.5),
            )
            .to_string();
            let put = xray.put_segments(&[document]).await;
            assert_eq!(put.status, 200, "case {case:?}: seeding must succeed, got {put:?}");
        }
        let requested: Vec<String> = (0..requested_count)
            .map(|index| {
                #[allow(clippy::cast_possible_truncation)]
                let index = index as u32;
                trace_id(index + 1)
            })
            .collect();

        let read = xray.batch_get(&requested, None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a miss is not a failure, got {read:?}"
        );
        let answered: Vec<&str> = array(&read.body, "Traces")
            .iter()
            .filter_map(|trace| trace.get("Id").and_then(Value::as_str))
            .collect();
        let expected: Vec<&str> = stored.iter().map(String::as_str).collect();
        assert_eq!(
            answered, expected,
            "case {case:?}: exactly the stored ids are answered for, got {answered:?}"
        );
        assert!(
            array(&read.body, "UnprocessedTraceIds").is_empty(),
            "case {case:?}: a miss must not be reported as unprocessed, got {:?}",
            read.get("UnprocessedTraceIds")
        );
    }
}

// =============================================================================
// 3. A remote subsegment infers a segment for the service it called
// =============================================================================

/// One ingest case for the inferred-segment rule: the submitted document, how
/// many segments the trace then holds, and the `parent_id` each inferred
/// segment carries.
struct InferCase {
    case: &'static str,
    document: Value,
    segments: usize,
    inferred_parents: Vec<&'static str>,
    inferred_names: Vec<&'static str>,
}

/// A subsegment, with `namespace` set only when `namespace` is `Some`.
fn subsegment(id: &str, name: &str, namespace: Option<&str>, children: Vec<Value>) -> Value {
    let mut value = json!({
        "id": id,
        "name": name,
        "start_time": 1_400.1_f64,
        "end_time": 1_400.5_f64,
    });
    if let Some(namespace) = namespace {
        value["namespace"] = json!(namespace);
    }
    if !children.is_empty() {
        value["subsegments"] = Value::Array(children);
    }
    value
}

fn infer_cases(trace: &str) -> Vec<InferCase> {
    vec![
        InferCase {
            case: "no subsegment at all",
            document: root_document("1111111111111111", trace, "client", 1_400.0, Some(1_400.6)),
            segments: 1,
            inferred_parents: vec![],
            inferred_names: vec![],
        },
        InferCase {
            case: "a subsegment with no namespace is the service's own work",
            document: {
                let mut document =
                    root_document("1111111111111111", trace, "client", 1_400.0, Some(1_400.6));
                document["subsegments"] =
                    json!([subsegment("2222222222222222", "quartz", None, vec![])]);
                document
            },
            segments: 1,
            inferred_parents: vec![],
            inferred_names: vec![],
        },
        InferCase {
            case: "a remote subsegment is a downstream service",
            document: {
                let mut document =
                    root_document("1111111111111111", trace, "client", 1_400.0, Some(1_400.6));
                document["subsegments"] =
                    json!([subsegment("2222222222222222", "quartz", Some("remote"), vec![])]);
                document
            },
            segments: 2,
            inferred_parents: vec!["2222222222222222"],
            inferred_names: vec!["quartz"],
        },
        InferCase {
            case: "another namespace is not a downstream service",
            document: {
                let mut document =
                    root_document("1111111111111111", trace, "client", 1_400.0, Some(1_400.6));
                document["subsegments"] =
                    json!([subsegment("2222222222222222", "quartz", Some("aws"), vec![])]);
                document
            },
            segments: 1,
            inferred_parents: vec![],
            inferred_names: vec![],
        },
        InferCase {
            case: "two remote subsegments infer one service each",
            document: {
                let mut document =
                    root_document("1111111111111111", trace, "client", 1_400.0, Some(1_400.6));
                document["subsegments"] = json!([
                    subsegment("2222222222222222", "quartz", Some("remote"), vec![]),
                    subsegment("3333333333333333", "topaz", Some("remote"), vec![]),
                ]);
                document
            },
            segments: 3,
            inferred_parents: vec!["2222222222222222", "3333333333333333"],
            inferred_names: vec!["quartz", "topaz"],
        },
        InferCase {
            case: "a remote subsegment nested inside another subsegment",
            document: {
                let mut document =
                    root_document("1111111111111111", trace, "client", 1_400.0, Some(1_400.6));
                document["subsegments"] = json!([subsegment(
                    "2222222222222222",
                    "handler",
                    None,
                    vec![subsegment("4444444444444444", "quartz", Some("remote"), vec![])],
                )]);
                document
            },
            segments: 2,
            inferred_parents: vec!["4444444444444444"],
            inferred_names: vec!["quartz"],
        },
    ]
}

#[tokio::test]
async fn a_remote_subsegment_infers_a_segment_for_the_service_it_called() {
    let trace = trace_id(10);
    for case in infer_cases(&trace) {
        let label = case.case;
        let xray = Xray::new();
        let submitted = case.document.to_string();
        let put = xray.put_segments(std::slice::from_ref(&submitted)).await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: PutTraceSegments must succeed, got {put:?}"
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "case {label:?}: the document is well formed, got {:?}",
            put.get("UnprocessedTraceSegments")
        );

        let read = xray.batch_get(std::slice::from_ref(&trace), None).await;
        let traces = array(&read.body, "Traces");
        assert_eq!(
            traces.len(),
            1,
            "case {label:?}: the stored trace must be answered for, got {read:?}"
        );
        let segments = array(&traces[0], "Segments");
        assert_eq!(
            segments.len(),
            case.segments,
            "case {label:?}: expected {} segments, got {segments:?}",
            case.segments
        );

        // The submitted document comes back byte for byte, subsegments and all.
        let documents: Vec<&str> = segments
            .iter()
            .filter_map(|segment| segment.get("Document").and_then(Value::as_str))
            .collect();
        assert!(
            documents.contains(&submitted.as_str()),
            "case {label:?}: Segment.Document must be the submitted string byte for byte, got {documents:?}"
        );

        // And each inferred segment is a top-level segment of the same trace
        // parented on the remote subsegment it stands for.
        let inferred: Vec<Value> = documents
            .iter()
            .filter_map(|document| serde_json::from_str::<Value>(document).ok())
            .filter(|document| document.get("inferred").and_then(Value::as_bool) == Some(true))
            .collect();
        let parents: Vec<&str> = inferred
            .iter()
            .filter_map(|document| document.get("parent_id").and_then(Value::as_str))
            .collect();
        assert_eq!(
            parents, case.inferred_parents,
            "case {label:?}: an inferred segment hangs off the remote subsegment, got {parents:?}"
        );
        let names: Vec<&str> = inferred
            .iter()
            .filter_map(|document| document.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names, case.inferred_names,
            "case {label:?}: an inferred segment is named for the service called, got {names:?}"
        );
        for document in &inferred {
            assert_eq!(
                document.get("trace_id").and_then(Value::as_str),
                Some(trace.as_str()),
                "case {label:?}: an inferred segment belongs to the same trace, got {document:?}"
            );
            let id = document.get("id").and_then(Value::as_str).unwrap_or_default();
            assert!(
                id.len() == 16 && id.chars().all(|c| c.is_ascii_hexdigit()),
                "case {label:?}: a segment id is 16 hexadecimal digits, got {id:?}"
            );
        }

        // Re-submitting the same document derives the same segments again
        // rather than a second copy of them.
        let again = xray.put_segments(std::slice::from_ref(&submitted)).await;
        assert_eq!(again.status, 200, "case {label:?}: re-submitting must succeed");
        let reread = xray.batch_get(std::slice::from_ref(&trace), None).await;
        assert_eq!(
            array(&array(&reread.body, "Traces")[0], "Segments").len(),
            case.segments,
            "case {label:?}: re-submitting a document must not add segments, got {reread:?}"
        );
    }
}

// =============================================================================
// 4. The outcome flags are the entity classifications, unioned
// =============================================================================

#[tokio::test]
async fn the_outcome_flags_are_derived_from_each_recorded_outcome() {
    // Rows: which outcome fields the document sets, or which status it
    // recorded, and the three flags that follow.
    //
    // Two rules meet here. The COUNTERS classify an entity exclusively (a fault
    // outranks a throttle, which outranks an error), which is what
    // `ErrorStatistics` splits a call into. The FLAGS are the model's own
    // wording: `$HasFault` is "a 500 series error", `$HasError` "a 400 series
    // error" and `$HasThrottle` "a 429 throttling error", and nothing makes
    // them exclusive — so a 429, being a 400-series status, is reported as both
    // a throttle and an error. The one case X-Ray RECLASSIFIES rather than
    // unions is a document that DECLARES the outcome a throttle: `throttle:
    // true` with `error: true` is a throttle and not also an error.
    let rows: Vec<(&str, Value, (bool, bool, bool))> = vec![
        ("nothing set", json!({}), (false, false, false)),
        ("fault", json!({"fault": true}), (true, false, false)),
        ("error", json!({"error": true}), (false, true, false)),
        ("throttle", json!({"throttle": true}), (false, false, true)),
        (
            "throttle and error together is a throttle",
            json!({"throttle": true, "error": true}),
            (false, false, true),
        ),
        (
            "fault and error together reports both",
            json!({"fault": true, "error": true}),
            (true, true, false),
        ),
        (
            "fault and throttle together is a fault",
            json!({"fault": true, "throttle": true}),
            (true, false, false),
        ),
        (
            "a 500 status is a fault",
            json!({"http": {"response": {"status": 500}}}),
            (true, false, false),
        ),
        (
            "a 404 status is an error",
            json!({"http": {"response": {"status": 404}}}),
            (false, true, false),
        ),
        (
            "a 429 status is a throttle and a 400-series error",
            json!({"http": {"response": {"status": 429}}}),
            (false, true, true),
        ),
        (
            "a throttle flag beside the 429 status an SDK records with it",
            json!({"throttle": true, "error": true, "http": {"response": {"status": 429}}}),
            (false, true, true),
        ),
        (
            "a 200 status is none of them",
            json!({"http": {"response": {"status": 200}}}),
            (false, false, false),
        ),
        (
            "a fault inside the root segment document",
            json!({"subsegments": [{"id": "9999999999999999", "name": "inner", "start_time": 1_000.1_f64, "end_time": 1_000.2_f64, "fault": true}]}),
            (true, false, false),
        ),
        (
            "two entities classified differently report both flags",
            json!({
                "error": true,
                "subsegments": [{"id": "9999999999999999", "name": "inner", "start_time": 1_000.1_f64, "end_time": 1_000.2_f64, "throttle": true}],
            }),
            (false, true, true),
        ),
    ];

    for (case, extra, (fault, error, throttle)) in rows {
        let xray = Xray::new();
        let trace = trace_id(11);
        let mut document =
            root_document("1111111111111111", &trace, "svc", 1_000.0, Some(1_000.5));
        for (member, value) in extra.as_object().expect("the row is an object") {
            document[member.clone()] = value.clone();
        }
        let put = xray.put_segments(&[document.to_string()]).await;
        assert_eq!(put.status, 200, "case {case:?}: seeding must succeed, got {put:?}");

        let got = xray.summaries(WINDOW.0, WINDOW.1, None).await;
        let summaries = array(&got.body, "TraceSummaries");
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: the stored trace must be summarised, got {got:?}"
        );
        for (member, expected) in [
            ("HasFault", fault),
            ("HasError", error),
            ("HasThrottle", throttle),
        ] {
            assert_eq!(
                summaries[0].get(member).and_then(Value::as_bool),
                Some(expected),
                "case {case:?}: expected {member} {expected}, got {:?}",
                summaries[0]
            );
        }
    }
}

#[tokio::test]
async fn each_outcome_flag_is_read_in_the_scope_the_model_documents_for_it() {
    /// One scope case: what the child document recorded, the three flags the
    /// trace then reports, and how many root causes of each class it carries.
    struct ScopeCase {
        case: &'static str,
        /// The members the child document carries beside the required ones.
        recorded: Value,
        /// `(HasFault, HasError, HasThrottle)`.
        flags: (bool, bool, bool),
        /// `(FaultRootCauses, ErrorRootCauses)` counts.
        causes: (usize, usize),
    }

    // `TraceSummary$HasFault` is "The root segment document has a 500 series
    // error" and `$HasError` "…a 400 series error", while `$HasThrottle` is
    // "One or more of the segment documents has a 429 throttling error"
    // (`smithy_query(query="doc",shape="TraceSummary",member="HasFault")`), so
    // an outcome recorded by a document that is NOT the root sets only the
    // third. Each row puts the outcome on a second, child document of a root
    // that recorded nothing.
    let rows: Vec<ScopeCase> = vec![
        ScopeCase {
            case: "a child document's fault",
            recorded: json!({"fault": true}),
            flags: (false, false, false),
            causes: (1, 0),
        },
        ScopeCase {
            case: "a child document's error",
            recorded: json!({"error": true}),
            flags: (false, false, false),
            causes: (0, 1),
        },
        ScopeCase {
            case: "a child document's throttle",
            recorded: json!({"throttle": true}),
            flags: (false, false, true),
            causes: (0, 0),
        },
        ScopeCase {
            case: "a child document's 500 status",
            recorded: json!({"http": {"response": {"status": 500}}}),
            flags: (false, false, false),
            causes: (1, 0),
        },
        ScopeCase {
            case: "a child document's 429 status",
            recorded: json!({"http": {"response": {"status": 429}}}),
            flags: (false, false, true),
            causes: (0, 0),
        },
    ];

    for row in rows {
        let ScopeCase {
            case,
            recorded,
            flags: (fault, error, throttle),
            causes: (fault_causes, error_causes),
        } = row;
        let xray = Xray::new();
        let trace = trace_id(15);
        let root = root_document("1111111111111111", &trace, "frontend", 1_000.0, Some(1_000.5));
        let mut child =
            root_document("2222222222222222", &trace, "backend", 1_000.1, Some(1_000.4));
        child["parent_id"] = json!("1111111111111111");
        for (member, value) in recorded.as_object().expect("the row is an object") {
            child[member.clone()] = value.clone();
        }
        let put = xray
            .put_segments(&[root.to_string(), child.to_string()])
            .await;
        assert_eq!(put.status, 200, "case {case:?}: seeding must succeed, got {put:?}");

        let got = xray.summaries(WINDOW.0, WINDOW.1, None).await;
        let summaries = array(&got.body, "TraceSummaries");
        assert_eq!(summaries.len(), 1, "case {case:?}: one trace, got {got:?}");
        for (member, expected) in [
            ("HasFault", fault),
            ("HasError", error),
            ("HasThrottle", throttle),
        ] {
            assert_eq!(
                summaries[0].get(member).and_then(Value::as_bool),
                Some(expected),
                "case {case:?}: expected {member} {expected}, got {:?}",
                summaries[0]
            );
        }
        // The outcome is still a root cause of its own class: a root cause is
        // reported per entity that recorded one, whatever the trace-level flags
        // say.
        assert_eq!(
            array(&summaries[0], "FaultRootCauses").len(),
            fault_causes,
            "case {case:?}: expected {fault_causes} FaultRootCauses, got {:?}",
            summaries[0].get("FaultRootCauses")
        );
        assert_eq!(
            array(&summaries[0], "ErrorRootCauses").len(),
            error_causes,
            "case {case:?}: expected {error_causes} ErrorRootCauses, got {:?}",
            summaries[0].get("ErrorRootCauses")
        );
    }
}

// =============================================================================
// 5. A cause block becomes a root cause
// =============================================================================
/// A `cause` block naming one exception.
fn cause(exception_type: &str, message: &str) -> Value {
    json!({"exceptions": [{"id": "1234567890abcdef", "type": exception_type, "message": message}]})
}

#[tokio::test]
async fn a_cause_block_becomes_a_root_cause_of_its_own_class() {
    // Rows: which outcome the document recorded, and which root cause list the
    // exception it named lands in.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a segment-level fault",
            json!({"fault": true, "cause": cause("java.lang.RuntimeException", "boom")}),
            "FaultRootCauses",
            "ErrorRootCauses",
        ),
        (
            "a segment-level error",
            json!({"error": true, "cause": cause("ValidationError", "bad input")}),
            "ErrorRootCauses",
            "FaultRootCauses",
        ),
        (
            "a 500 status with a cause",
            json!({
                "http": {"response": {"status": 500}},
                "cause": cause("System.NullReferenceException", "null"),
            }),
            "FaultRootCauses",
            "ErrorRootCauses",
        ),
    ];

    for (case, extra, populated, empty) in rows {
        let xray = Xray::new();
        let trace = trace_id(12);
        let mut document =
            root_document("1111111111111111", &trace, "api.example.com", 1_000.0, Some(1_000.5));
        document["origin"] = json!("AWS::EC2::Instance");
        for (member, value) in extra.as_object().expect("the row is an object") {
            document[member.clone()] = value.clone();
        }
        let expected_exception = extra
            .get("cause")
            .and_then(|cause| cause.get("exceptions"))
            .and_then(Value::as_array)
            .and_then(|exceptions| exceptions.first())
            .and_then(|exception| exception.get("type"))
            .and_then(Value::as_str)
            .expect("the row names an exception type")
            .to_string();

        let put = xray.put_segments(&[document.to_string()]).await;
        assert_eq!(put.status, 200, "case {case:?}: seeding must succeed, got {put:?}");

        let got = xray.summaries(WINDOW.0, WINDOW.1, None).await;
        let summaries = array(&got.body, "TraceSummaries");
        assert_eq!(summaries.len(), 1, "case {case:?}: one trace, got {got:?}");
        let causes = array(&summaries[0], populated);
        assert_eq!(
            causes.len(),
            1,
            "case {case:?}: the outcome is one root cause in {populated}, got {causes:?}"
        );
        assert!(
            array(&summaries[0], empty).is_empty(),
            "case {case:?}: {empty} must stay empty, got {:?}",
            summaries[0].get(empty)
        );
        let services = array(&causes[0], "Services");
        assert_eq!(
            services.len(),
            1,
            "case {case:?}: one instrumented service recorded it, got {services:?}"
        );
        assert_eq!(
            services[0].get("Name").and_then(Value::as_str),
            Some("api.example.com"),
            "case {case:?}: the service is the segment's own name, got {:?}",
            services[0]
        );
        assert_eq!(
            services[0].get("Type").and_then(Value::as_str),
            Some("AWS::EC2::Instance"),
            "case {case:?}: the service type is the segment's origin, got {:?}",
            services[0]
        );
        assert_eq!(
            services[0].get("Inferred").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a submitted segment is not an inferred service, got {:?}",
            services[0]
        );
        let path = array(&services[0], "EntityPath");
        assert_eq!(
            path.len(),
            1,
            "case {case:?}: the segment itself is the only frame, got {path:?}"
        );
        assert_eq!(
            path[0].get("Remote").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: the service's own work is not a dependency frame, got {:?}",
            path[0]
        );
        let exceptions = array(&path[0], "Exceptions");
        assert_eq!(
            exceptions.len(),
            1,
            "case {case:?}: the frame carries the exception it named, got {exceptions:?}"
        );
        assert_eq!(
            exceptions[0].get("Name").and_then(Value::as_str),
            Some(expected_exception.as_str()),
            "case {case:?}: the exception Name is the submitted type, got {:?}",
            exceptions[0]
        );
        assert_eq!(
            causes[0].get("ClientImpacting").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: the trace's root recorded the outcome, so the client saw it, got {:?}",
            causes[0]
        );
    }
}

#[tokio::test]
async fn a_remote_frame_attributes_the_root_cause_to_the_service_it_inferred() {
    let xray = Xray::new();
    let trace = trace_id(13);
    let mut document = root_document("1111111111111111", &trace, "client", 1_000.0, Some(1_000.6));
    document["subsegments"] = json!([{
        "id": "2222222222222222",
        "name": "quartz",
        "namespace": "remote",
        "start_time": 1_000.1_f64,
        "end_time": 1_000.5_f64,
        "fault": true,
        "cause": cause("RemoteServiceException", "downstream failed"),
    }]);
    let put = xray.put_segments(&[document.to_string()]).await;
    assert_eq!(put.status, 200, "seeding must succeed, got {put:?}");

    let got = xray.summaries(WINDOW.0, WINDOW.1, None).await;
    let summaries = array(&got.body, "TraceSummaries");
    assert_eq!(summaries.len(), 1, "one trace was stored, got {got:?}");
    assert_eq!(
        summaries[0].get("HasFault").and_then(Value::as_bool),
        Some(true),
        "the remote frame recorded a fault, got {:?}",
        summaries[0]
    );
    let causes = array(&summaries[0], "FaultRootCauses");
    assert_eq!(
        causes.len(),
        1,
        "the downstream service is the one root cause, got {causes:?}"
    );
    let services = array(&causes[0], "Services");
    let names: Vec<&str> = services
        .iter()
        .filter_map(|service| service.get("Name").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names,
        vec!["client", "quartz"],
        "the root cause runs from the calling service to the one it called, got {names:?}"
    );
    let inferred: Vec<Option<bool>> = services
        .iter()
        .map(|service| service.get("Inferred").and_then(Value::as_bool))
        .collect();
    assert_eq!(
        inferred,
        vec![Some(false), Some(true)],
        "the downstream service was inferred from the call to it, got {inferred:?}"
    );
    // The calling service's frames: its own work, then the dependency frame it
    // called out through.
    let calling_frames: Vec<(Option<&str>, Option<bool>)> = array(&services[0], "EntityPath")
        .iter()
        .map(|entity| {
            (
                entity.get("Name").and_then(Value::as_str),
                entity.get("Remote").and_then(Value::as_bool),
            )
        })
        .collect();
    assert_eq!(
        calling_frames,
        vec![(Some("client"), Some(false)), (Some("quartz"), Some(true))],
        "a dependency-bound frame is Remote and a pipeline-bound one is not, got {calling_frames:?}"
    );
    let downstream = array(&services[1], "EntityPath");
    assert_eq!(
        downstream.len(),
        1,
        "the inferred service has one frame, got {downstream:?}"
    );
    assert_eq!(
        downstream[0].get("Remote").and_then(Value::as_bool),
        Some(true),
        "the inferred service's frame is remote, got {:?}",
        downstream[0]
    );
    let exceptions = array(&downstream[0], "Exceptions");
    assert_eq!(
        exceptions.first().and_then(|exception| exception
            .get("Name")
            .and_then(Value::as_str)),
        Some("RemoteServiceException"),
        "the exception the remote frame named is the root cause's, got {exceptions:?}"
    );
    assert_eq!(
        causes[0].get("ClientImpacting").and_then(Value::as_bool),
        Some(false),
        "the trace's root did not record the fault, so the client did not see it, got {:?}",
        causes[0]
    );
}

// =============================================================================
// 6. A filter expression selects the traces it matches
// =============================================================================

#[tokio::test]
async fn a_submitted_document_cannot_claim_to_be_an_inferred_service() {
    // `inferred` is service-assigned and is not a member of the segment schema
    // PutTraceSegments documents, so a document that carries it is stored as
    // the instrumented segment it is: it neither suppresses the inference its
    // own remote subsegment is due nor misreports its provenance.
    let xray = Xray::new();
    let trace = trace_id(14);
    let mut document = root_document("1111111111111111", &trace, "client", 1_000.0, Some(1_000.6));
    document["inferred"] = json!(true);
    document["subsegments"] = json!([{
        "id": "2222222222222222",
        "name": "quartz",
        "namespace": "remote",
        "start_time": 1_000.1_f64,
        "end_time": 1_000.5_f64,
        "fault": true,
    }]);
    let put = xray.put_segments(&[document.to_string()]).await;
    assert_eq!(put.status, 200, "seeding must succeed, got {put:?}");

    let read = xray.batch_get(std::slice::from_ref(&trace), None).await;
    let segments = array(&array(&read.body, "Traces")[0], "Segments");
    assert_eq!(
        segments.len(),
        2,
        "the remote subsegment is still inferred for, got {segments:?}"
    );

    let got = xray.summaries(WINDOW.0, WINDOW.1, None).await;
    let summaries = array(&got.body, "TraceSummaries");
    let services = array(&array(&summaries[0], "FaultRootCauses")[0], "Services");
    let claimed: Vec<(Option<&str>, Option<bool>)> = services
        .iter()
        .map(|service| {
            (
                service.get("Name").and_then(Value::as_str),
                service.get("Inferred").and_then(Value::as_bool),
            )
        })
        .collect();
    assert_eq!(
        claimed,
        vec![(Some("client"), Some(false)), (Some("quartz"), Some(true))],
        "only a service-synthesised segment is an inferred service, got {claimed:?}"
    );
}

/// The corpus every filter row runs against: one trace per shape the keywords
/// read.
fn corpus() -> Vec<(&'static str, String, Vec<String>)> {
    let ok = trace_id(21);
    let faulting = trace_id(22);
    let throttled = trace_id(23);
    let async_tail = trace_id(24);
    let remote = trace_id(25);
    let errored = trace_id(26);
    let partial = trace_id(27);
    let throttled_status = trace_id(28);
    vec![
        (
            "ok",
            ok.clone(),
            vec![json!({
                "trace_id": ok,
                "id": "0000000000000001",
                "name": "frontend",
                "origin": "AWS::EC2::Instance",
                "start_time": 1_000.0_f64,
                "end_time": 1_000.5_f64,
                "user": "alice",
                "resource_arn": "arn:aws:ec2:us-east-1:000000000000:instance/i-01",
                "http": {
                    "request": {"method": "GET", "url": "http://api.example.com/", "client_ip": "10.0.0.1", "user_agent": "curl/8"},
                    "response": {"status": 200},
                },
                "annotations": {"stage": "beta", "attempt": 3, "cached": false},
            })
            .to_string()],
        ),
        (
            "faulting",
            faulting.clone(),
            vec![json!({
                "trace_id": faulting,
                "id": "0000000000000002",
                "name": "api.example.com",
                "start_time": 1_100.0_f64,
                "end_time": 1_100.5_f64,
                "fault": true,
                "http": {"request": {"method": "POST"}, "response": {"status": 500}},
                "cause": {"exceptions": [{"type": "java.lang.RuntimeException", "message": "boom"}]},
            })
            .to_string()],
        ),
        (
            "throttled",
            throttled.clone(),
            vec![json!({
                "trace_id": throttled,
                "id": "0000000000000003",
                "name": "gateway",
                "start_time": 1_200.0_f64,
                "end_time": 1_200.25_f64,
                "throttle": true,
                "error": true,
            })
            .to_string()],
        ),
        (
            // A root that answered quickly with work that kept running after
            // it: the trace's duration spans the tail, its response time does
            // not.
            "async-tail",
            async_tail.clone(),
            vec![
                root_document("0000000000000004", &async_tail, "worker", 1_300.0, Some(1_300.4))
                    .to_string(),
                json!({
                    "trace_id": async_tail,
                    "id": "0000000000000104",
                    "parent_id": "0000000000000004",
                    "name": "tail",
                    "start_time": 1_300.1_f64,
                    "end_time": 1_305.0_f64,
                })
                .to_string(),
            ],
        ),
        (
            "remote",
            remote.clone(),
            vec![json!({
                "trace_id": remote,
                "id": "0000000000000005",
                "name": "quartz-client",
                "start_time": 1_400.0_f64,
                "end_time": 1_400.6_f64,
                "subsegments": [{
                    "id": "0000000000000105",
                    "name": "quartz",
                    "namespace": "remote",
                    "start_time": 1_400.1_f64,
                    "end_time": 1_400.5_f64,
                    "fault": true,
                    "cause": {"exceptions": [{"type": "RemoteServiceException", "message": "downstream failed"}]},
                }],
            })
            .to_string()],
        ),
        (
            "errored",
            errored.clone(),
            vec![json!({
                "trace_id": errored,
                "id": "0000000000000006",
                "name": "shop",
                "start_time": 1_500.0_f64,
                "end_time": 1_500.2_f64,
                "http": {"request": {"method": "GET", "url": "http://shop.example.com/cart"}, "response": {"status": 404}},
                "annotations": {"stage": "prod"},
            })
            .to_string()],
        ),
        (
            "partial",
            partial.clone(),
            vec![root_document("0000000000000007", &partial, "slow", 1_600.0, None).to_string()],
        ),
        (
            // A gateway that recorded the throttle as a STATUS rather than as a
            // flag. 429 is in the 400 series, so this trace is a throttle AND a
            // 400-series error, where the one above — which DECLARES `throttle`
            // — is only a throttle.
            "throttled-429",
            throttled_status.clone(),
            vec![json!({
                "trace_id": throttled_status,
                "id": "0000000000000008",
                "name": "gateway-status",
                "start_time": 1_700.0_f64,
                "end_time": 1_700.25_f64,
                "http": {"request": {"method": "POST"}, "response": {"status": 429}},
            })
            .to_string()],
        ),
    ]
}

#[tokio::test]
async fn a_filter_expression_selects_the_traces_it_matches() {
    let corpus = corpus();
    let xray = Xray::new();
    for (label, _, documents) in &corpus {
        let put = xray.put_segments(documents).await;
        assert_eq!(
            put.status, 200,
            "seeding {label:?} must succeed, got {put:?}"
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "seeding {label:?} must store every document, got {:?}",
            put.get("UnprocessedTraceSegments")
        );
    }

    // Rows: one expression per keyword class and per operator shape, with the
    // corpus traces it selects.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("ok", vec!["ok", "async-tail", "partial"]),
        ("fault", vec!["faulting", "remote"]),
        // The 429 recorded as a STATUS is a 400-series error as well as a
        // throttle; the one DECLARED with `throttle: true` is only a throttle.
        ("error", vec!["errored", "throttled-429"]),
        ("throttle", vec!["throttled", "throttled-429"]),
        ("partial", vec!["partial"]),
        ("fault = true", vec!["faulting", "remote"]),
        (
            "fault = false",
            vec![
                "ok",
                "throttled",
                "throttled-429",
                "async-tail",
                "errored",
                "partial",
            ],
        ),
        (
            "NOT fault",
            vec![
                "ok",
                "throttled",
                "throttled-429",
                "async-tail",
                "errored",
                "partial",
            ],
        ),
        (
            "NOT (fault OR error OR throttle)",
            vec!["ok", "async-tail", "partial"],
        ),
        (
            "fault OR error",
            vec!["faulting", "remote", "errored", "throttled-429"],
        ),
        ("service(\"api.example.com\")", vec!["faulting"]),
        ("service(\"frontend\", \"AWS::EC2::Instance\")", vec!["ok"]),
        ("service(\"frontend\", \"AWS::ECS::Container\")", vec![]),
        ("service(\"nobody\")", vec![]),
        ("service(\"quartz\")", vec!["remote"]),
        ("edge(\"quartz-client\", \"quartz\")", vec!["remote"]),
        ("edge(\"quartz\", \"quartz-client\")", vec![]),
        ("service(\"frontend\") { http.status = 200 }", vec!["ok"]),
        ("service(\"frontend\") { http.status = 500 }", vec![]),
        ("http.status = 200", vec!["ok"]),
        ("http.status >= 400", vec!["faulting", "errored", "throttled-429"]),
        ("http.status < 400", vec!["ok"]),
        ("http.url = \"http://api.example.com/\"", vec!["ok"]),
        ("http.url != \"http://api.example.com/\"", vec!["errored"]),
        ("http.method = \"GET\"", vec!["ok", "errored"]),
        ("http.useragent = \"curl/8\"", vec!["ok"]),
        ("http.clientip = \"10.0.0.1\"", vec!["ok"]),
        ("user = \"alice\"", vec!["ok"]),
        ("user = \"bob\"", vec![]),
        (
            "resource.arn = \"arn:aws:ec2:us-east-1:000000000000:instance/i-01\"",
            vec!["ok"],
        ),
        ("annotation.stage = \"beta\"", vec!["ok"]),
        ("annotation.stage = \"prod\"", vec!["errored"]),
        ("annotation.attempt = 3", vec!["ok"]),
        ("annotation.attempt > 2", vec!["ok"]),
        ("annotation.attempt > 3", vec![]),
        ("annotation.cached = false", vec!["ok"]),
        ("annotation.missing = \"anything\"", vec![]),
        // The async tail is inside the trace's duration but outside the root's
        // response time, so the two keywords select differently.
        ("duration > 1", vec!["async-tail"]),
        ("responsetime > 1", vec![]),
        ("duration > 1 AND responsetime < 1", vec!["async-tail"]),
        ("service(\"worker\") { responsetime < 1 }", vec!["async-tail"]),
        ("duration >= 0.5 AND duration <= 0.6", vec!["ok", "faulting", "remote"]),
        (
            "rootcause.fault.exception.name = \"java.lang.RuntimeException\"",
            vec!["faulting"],
        ),
        ("rootcause.fault.exception.message = \"boom\"", vec!["faulting"]),
        ("rootcause.fault.entity.name = \"api.example.com\"", vec!["faulting"]),
        ("rootcause.fault.service.inferred = true", vec!["remote"]),
        ("rootcause.fault.entity.remote = true", vec!["remote"]),
        // A root cause has both kinds of frame, so the dependency-bound filter
        // and the pipeline-bound one separate them.
        ("rootcause.fault.entity.remote = false", vec!["faulting", "remote"]),
        // An outcome with no `cause` block is still a root cause: the entity
        // that recorded it is named, with no exceptions.
        ("rootcause.error.entity.name = \"shop\"", vec!["errored"]),
        ("rootcause.error.entity.name = \"frontend\"", vec![]),
        ("rootcause.error.exception.message = \"boom\"", vec![]),
        ("fault AND service(\"api.example.com\")", vec!["faulting"]),
        ("fault AND service(\"gateway\")", vec![]),
        (
            "(fault OR throttle) AND NOT partial",
            vec!["faulting", "throttled", "throttled-429", "remote"],
        ),
        ("not fault and not error and not throttle", vec!["ok", "async-tail", "partial"]),
    ];

    #[allow(clippy::cast_possible_wrap)]
    let scanned = corpus.len() as i64;
    for (expression, expected_labels) in rows {
        // A selection can be longer than one page, so every page of it is
        // walked: the rule here is about WHICH traces an expression selects, not
        // about where the page breaks.
        let mut selected: BTreeSet<&str> = BTreeSet::new();
        let mut processed: Option<i64> = None;
        let mut token: Option<String> = None;
        loop {
            let got = match &token {
                None => xray.filtered(WINDOW.0, WINDOW.1, expression).await,
                Some(next) => {
                    xray.call(
                        "/TraceSummaries",
                        &json!({
                            "StartTime": WINDOW.0,
                            "EndTime": WINDOW.1,
                            "FilterExpression": expression,
                            "NextToken": next,
                        }),
                    )
                    .await
                }
            };
            assert_eq!(
                got.status, 200,
                "case {expression:?}: a well-formed expression is served, got {got:?}"
            );
            if processed.is_none() {
                processed = got.get("TracesProcessedCount").as_i64();
            }
            selected.extend(
                array(&got.body, "TraceSummaries")
                    .iter()
                    .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
                    .map(|id| {
                        corpus
                            .iter()
                            .find(|(_, trace, _)| trace == id)
                            .map_or("unknown", |(label, _, _)| *label)
                    }),
            );
            match got.get("NextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let expected: BTreeSet<&str> = expected_labels.iter().copied().collect();
        assert_eq!(
            selected, expected,
            "case {expression:?}: expected {expected:?}, got {selected:?}"
        );
        assert_eq!(
            processed,
            Some(scanned),
            "case {expression:?}: every trace in the window is processed, matching or not, got {processed:?}"
        );
    }
}

#[tokio::test]
async fn an_expression_the_language_cannot_read_is_refused_on_both_surfaces() {
    /// One unreadable expression, and which surfaces refuse it.
    struct BadExpression {
        case: &'static str,
        expression: &'static str,
        /// Whether a surface that STORES an expression refuses it too.
        ///
        /// The language itself — its grammar and the fields X-Ray indexes — is
        /// judged by one gate (`state::filter_expression`), and every surface
        /// runs it, so a syntax error or an unindexed field is refused
        /// everywhere. A comparison whose OPERANDS cannot be compared is
        /// refused everywhere as well, because it can never be true for any
        /// trace. What only the reader refuses is what only the reader needs:
        /// a field it cannot yet derive a value for.
        stored_too: bool,
    }

    // Rows: each way an expression is unreadable — a syntax error, an unknown
    // keyword, operands that cannot be compared, a keyword this emulator
    // derives no value for.
    let rows: Vec<BadExpression> = vec![
        BadExpression {
            case: "an unbalanced open parenthesis",
            expression: "fault = true AND (",
            stored_too: true,
        },
        BadExpression {
            case: "an unbalanced close parenthesis",
            expression: "fault = true)",
            stored_too: true,
        },
        BadExpression {
            case: "a dangling operator",
            expression: "duration >",
            stored_too: true,
        },
        BadExpression {
            case: "a dangling boolean operator",
            expression: "fault AND",
            stored_too: true,
        },
        BadExpression {
            case: "an unterminated string",
            expression: "http.url = \"http://x",
            stored_too: true,
        },
        BadExpression {
            case: "an unknown keyword",
            expression: "nonsense = 1",
            stored_too: true,
        },
        BadExpression {
            case: "a string keyword against a number",
            expression: "http.url = 404",
            stored_too: true,
        },
        BadExpression {
            case: "a name compared to a number",
            expression: "rootcause.fault.entity.name = 12",
            stored_too: true,
        },
        BadExpression {
            case: "a number keyword against a string",
            expression: "duration = \"slow\"",
            stored_too: true,
        },
        BadExpression {
            case: "a boolean keyword against a string",
            expression: "fault = \"true\"",
            stored_too: true,
        },
        BadExpression {
            case: "a boolean keyword against a number",
            expression: "throttle = 1",
            stored_too: true,
        },
        BadExpression {
            case: "an ordering operator on a string",
            expression: "http.method > \"GET\"",
            stored_too: true,
        },
        BadExpression {
            case: "an ordering operator on a boolean",
            expression: "fault > true",
            stored_too: true,
        },
        BadExpression {
            case: "a service with no name",
            expression: "service()",
            stored_too: true,
        },
        BadExpression {
            case: "an unclosed scope block",
            expression: "service(\"a\") { fault",
            stored_too: true,
        },
        BadExpression {
            case: "a stray character",
            expression: "fault = true & ok",
            stored_too: true,
        },
        BadExpression {
            case: "an unindexed field",
            expression: "metadata.probe = \"x\"",
            stored_too: true,
        },
        // Read-only refusals: the gate accepts these as expressions of the
        // language, and the reader says which value it cannot derive.
        BadExpression {
            case: "an unknown root cause member",
            expression: "rootcause.fault.entity.colour = \"red\"",
            stored_too: false,
        },
        BadExpression {
            case: "an unknown root cause class",
            expression: "rootcause.latency.entity.name = \"x\"",
            stored_too: false,
        },
        BadExpression {
            case: "a keyword with no value derived",
            expression: "rootcause.responsetime.entity.name = \"x\"",
            stored_too: false,
        },
    ];

    for row in rows {
        let BadExpression {
            case,
            expression,
            stored_too,
        } = row;
        let xray = Xray::new();
        let queried = xray.filtered(WINDOW.0, WINDOW.1, expression).await;
        assert_refused(&format!("{case} on GetTraceSummaries"), &queried);

        if !stored_too {
            continue;
        }

        // The same expression is refused everywhere a group stores one, so no
        // surface can accept what another refuses.
        let created = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": "g", "FilterExpression": expression}),
            )
            .await;
        assert_refused(&format!("{case} on CreateGroup"), &created);
        let listed = xray.get_groups(None).await;
        let stored: Vec<&str> = array(&listed.body, "Groups")
            .iter()
            .filter_map(|group| group.get("GroupName").and_then(Value::as_str))
            .filter(|name| *name != "Default")
            .collect();
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused CreateGroup must store nothing beside the reserved group, got {stored:?}"
        );

        // And on the update surface, over a group that already holds a readable
        // expression, which the refusal must leave untouched.
        let seeded = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": "g", "FilterExpression": "fault"}),
            )
            .await;
        assert_eq!(
            seeded.status, 200,
            "case {case:?}: seeding the update surface must succeed, got {seeded:?}"
        );
        let updated = xray
            .call(
                "/UpdateGroup",
                &json!({"GroupName": "g", "FilterExpression": expression}),
            )
            .await;
        assert_refused(&format!("{case} on UpdateGroup"), &updated);
        let read = xray.call("/GetGroup", &json!({"GroupName": "g"})).await;
        assert_eq!(
            read.get("Group")
                .get("FilterExpression")
                .and_then(Value::as_str),
            Some("fault"),
            "case {case:?}: a refused UpdateGroup must leave the stored expression alone, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_group_stores_an_expression_both_surfaces_accept() {
    let xray = Xray::new();
    // Each row is an expression the language reads; a group keeps it, an update
    // replaces it with it, and GetTraceSummaries evaluates it.
    for expression in [
        "fault",
        "service(\"frontend\")",
        "annotation.stage = \"beta\"",
        "duration > 0.1 AND NOT error",
        "rootcause.fault.entity.remote = true",
    ] {
        let name = format!("g{}", expression.len());
        let created = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": name, "FilterExpression": expression}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {expression:?}: a readable expression is stored, got {created:?}"
        );
        assert_eq!(
            created
                .get("Group")
                .get("FilterExpression")
                .and_then(Value::as_str),
            Some(expression),
            "case {expression:?}: the group reports the expression it stored, got {created:?}"
        );
        let updated = xray
            .call(
                "/UpdateGroup",
                &json!({"GroupName": name, "FilterExpression": expression}),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {expression:?}: the update surface reads it too, got {updated:?}"
        );
        let queried = xray.filtered(WINDOW.0, WINDOW.1, expression).await;
        assert_eq!(
            queried.status, 200,
            "case {expression:?}: the query surface reads it too, got {queried:?}"
        );
    }
}

#[tokio::test]
async fn an_expression_past_the_bounds_on_its_size_is_refused_rather_than_run() {
    use service_com_amazonaws_xray_awsxray::state::filter_expression::{MAX_DEPTH, MAX_LENGTH};

    let xray = Xray::new();

    // The bounds on how much work one expression may drive belong to the ONE
    // parser this crate reads the language with, so every surface shares them:
    // its length and its nesting depth. Rows walk each at the bound, which is
    // read, and past it, which is refused rather than parsed — nesting is what
    // the parser, the evaluator and the tree's own `Drop` recurse on, and a
    // stack overflow ends the process rather than the request, so the bound has
    // to hold before the recursion starts.
    let at_depth = |levels: usize| format!("{}fault{}", "(".repeat(levels), ")".repeat(levels));
    let not_depth = |levels: usize| format!("{}fault", "NOT ".repeat(levels));
    let wide = |terms: usize| {
        let mut expression = String::from("fault");
        for _ in 1..terms {
            expression.push_str(" OR fault");
        }
        expression
    };

    let rows: Vec<(&str, String, bool)> = vec![
        ("one level of nesting", at_depth(1), true),
        ("nesting at the depth bound", at_depth(MAX_DEPTH), true),
        (
            "nesting one level past the bound",
            at_depth(MAX_DEPTH + 1),
            false,
        ),
        ("nesting far past the bound", at_depth(MAX_DEPTH * 8), false),
        ("chained NOT at the depth bound", not_depth(MAX_DEPTH), true),
        ("chained NOT past the bound", not_depth(MAX_DEPTH + 2), false),
        (
            "a scope block nested past the bound",
            format!("service(\"svc\") {{ {} }}", at_depth(MAX_DEPTH)),
            false,
        ),
        ("a wide expression within the bounds", wide(16), true),
        (
            "a source at the length bound",
            {
                // `fault` plus ` OR fault` terms, trimmed to the bound exactly.
                let mut expression = wide(MAX_LENGTH / 9 + 2);
                expression.truncate(MAX_LENGTH);
                while !expression.ends_with("fault") {
                    expression.pop();
                }
                expression
            },
            true,
        ),
        (
            "a source longer than the bound allows",
            format!("annotation.stage = \"{}\"", "a".repeat(MAX_LENGTH)),
            false,
        ),
    ];

    for (case, expression, readable) in rows {
        let queried = xray.filtered(WINDOW.0, WINDOW.1, &expression).await;
        if readable {
            assert_eq!(
                queried.status, 200,
                "case {case:?}: an expression within the bounds is read, got {queried:?}"
            );
        } else {
            assert_refused(case, &queried);
        }
        // One parser, one set of bounds: the surface that STORES an expression
        // reads it exactly as the one that evaluates it does.
        let created = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": "bounded", "FilterExpression": expression}),
            )
            .await;
        if readable {
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateGroup reads it too, got {created:?}"
            );
            let deleted = xray
                .call("/DeleteGroup", &json!({"GroupName": "bounded"}))
                .await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: cleaning up must succeed, got {deleted:?}"
            );
        } else {
            assert_refused(&format!("{case} on CreateGroup"), &created);
        }
    }
}

#[tokio::test]
async fn a_trace_reports_at_most_the_bounded_number_of_root_causes() {
    use service_com_amazonaws_xray_awsxray::state::MAX_ROOT_CAUSES_PER_TRACE;

    let trace = trace_id(32);
    // Rows either side of the cap: a trace with fewer failing segments reports
    // one root cause each, and one with more reports the cap rather than making
    // a single read walk the trace once per failure.
    for count in [1, MAX_ROOT_CAUSES_PER_TRACE, MAX_ROOT_CAUSES_PER_TRACE * 2] {
        let fresh = Xray::new();
        let documents: Vec<String> = (0..count)
            .map(|index| {
                let mut document = root_document(
                    &format!("{:016x}", index + 1),
                    &trace,
                    "svc",
                    1_000.0,
                    Some(1_000.5),
                );
                if index > 0 {
                    document["parent_id"] = json!("0000000000000001");
                }
                document["fault"] = json!(true);
                document.to_string()
            })
            .collect();
        let put = fresh.put_segments(&documents).await;
        assert_eq!(put.status, 200, "count {count}: seeding must succeed, got {put:?}");

        let got = fresh.summaries(WINDOW.0, WINDOW.1, None).await;
        let summaries = array(&got.body, "TraceSummaries");
        assert_eq!(summaries.len(), 1, "count {count}: one trace, got {got:?}");
        let causes = array(&summaries[0], "FaultRootCauses").len();
        assert_eq!(
            causes,
            count.min(MAX_ROOT_CAUSES_PER_TRACE),
            "count {count}: a trace reports at most {MAX_ROOT_CAUSES_PER_TRACE} root causes, got {causes}"
        );
    }
}

#[tokio::test]
async fn a_query_that_would_cost_more_than_one_request_may_is_throttled() {
    use service_com_amazonaws_xray_awsxray::state::filter::{
        MAX_QUERY_STEPS,
    };
    use service_com_amazonaws_xray_awsxray::state::{
        MAX_SEGMENTS_PER_TRACE, MAX_SEGMENT_DOCUMENTS_PER_CALL,
    };

    // The parser's bounds cap ONE expression; this one caps what a whole request
    // may cost over the traces it reads. Reaching it needs a store and an
    // expression that are each within their own bounds but expensive together:
    // traces at the per-trace segment cap, and an expression of many terms that
    // are ALL FALSE, so nothing short-circuits and every node of every trace is
    // visited.
    let terms = 128;
    let mut expression = String::from("fault");
    for _ in 0..terms {
        expression.push_str(" OR fault");
    }
    // Each trace costs about (nodes × segments) steps, so this many traces
    // carries the walk past the budget.
    let per_trace = (terms + 1) * MAX_SEGMENTS_PER_TRACE;
    let traces = MAX_QUERY_STEPS / per_trace + 1;

    let xray = Xray::new();
    for index in 0..traces {
        #[allow(clippy::cast_possible_truncation)]
        let trace = trace_id(100 + index as u32);
        let mut position = 0usize;
        while position < MAX_SEGMENTS_PER_TRACE {
            let batch = MAX_SEGMENT_DOCUMENTS_PER_CALL.min(MAX_SEGMENTS_PER_TRACE - position);
            let documents: Vec<String> = (0..batch)
                .map(|at| {
                    let segment = position + at;
                    let mut document = root_document(
                        &format!("{:016x}", segment + 1),
                        &trace,
                        "svc",
                        1_000.0,
                        Some(1_000.5),
                    );
                    if segment > 0 {
                        document["parent_id"] = json!("0000000000000001");
                    }
                    document.to_string()
                })
                .collect();
            let put = xray.put_segments(&documents).await;
            assert_eq!(
                put.status, 200,
                "seeding {trace} at {position} must succeed, got {put:?}"
            );
            position += batch;
        }
    }

    // The same expression over ONE trace is well within the budget and is
    // served; over the whole store it is not, and that is a throttle, not a
    // silently short page.
    let narrow = xray
        .filtered(WINDOW.0, WINDOW.1, "service(\"nobody\")")
        .await;
    assert_eq!(
        narrow.status, 200,
        "a cheap expression over the same store is served, got {narrow:?}"
    );

    let throttled = xray.filtered(WINDOW.0, WINDOW.1, &expression).await;
    assert_eq!(
        (throttled.status, throttled.error_type.as_deref()),
        (429, Some("ThrottledException")),
        "a query past the work one request may do is throttled, not answered with a truncated selection, got {throttled:?}"
    );
    assert!(
        throttled
            .message()
            .is_some_and(|message| !message.is_empty()),
        "the throttle must say what to narrow, got {throttled:?}"
    );
}

// =============================================================================
// 7. The requested time range
// =============================================================================

#[tokio::test]
async fn an_inverted_time_range_is_refused() {
    let xray = Xray::new();
    let trace = trace_id(30);
    let put = xray
        .put_segments(&[
            root_document("0000000000000001", &trace, "svc", 1_000.0, Some(1_000.5)).to_string()
        ])
        .await;
    assert_eq!(put.status, 200, "seeding must succeed, got {put:?}");

    // Rows walk the window relative to the stored trace's start time: a window
    // that brackets it, one that excludes it at each end, the zero-width one at
    // its start, and the inverted ones.
    let rows: Vec<(&str, f64, f64, Option<usize>)> = vec![
        ("a window around the trace", 999.0, 1_001.0, Some(1)),
        ("the half-open lower bound is inclusive", 1_000.0, 1_001.0, Some(1)),
        ("the half-open upper bound is exclusive", 999.0, 1_000.0, Some(0)),
        ("a zero-width window", 1_000.0, 1_000.0, Some(0)),
        ("a window after the trace", 1_001.0, 1_002.0, Some(0)),
        ("an inverted window", 1_001.0, 1_000.0, None),
        ("an inverted window far apart", 2_000.0, 1_000.0, None),
        ("an inverted window by a nanosecond", 1_000.000_002, 1_000.000_001, None),
    ];

    for (case, start, end, expected) in rows {
        let got = xray.summaries(start, end, None).await;
        match expected {
            None => assert_refused(case, &got),
            Some(count) => {
                assert_eq!(
                    got.status, 200,
                    "case {case:?}: a well-formed window is served, got {got:?}"
                );
                assert_eq!(
                    array(&got.body, "TraceSummaries").len(),
                    count,
                    "case {case:?}: expected {count} summaries, got {:?}",
                    got.get("TraceSummaries")
                );
            }
        }
    }
}

// =============================================================================
// 8. A trace's own times still come from its segments
// =============================================================================

#[tokio::test]
async fn an_inferred_segment_does_not_move_the_traces_own_times() {
    let xray = Xray::new();
    let trace = trace_id(31);
    let mut document = root_document("1111111111111111", &trace, "client", 1_000.0, Some(1_000.6));
    document["subsegments"] = json!([{
        "id": "2222222222222222",
        "name": "quartz",
        "namespace": "remote",
        "start_time": 1_000.1_f64,
        "end_time": 1_000.5_f64,
    }]);
    let put = xray.put_segments(&[document.to_string()]).await;
    assert_eq!(put.status, 200, "seeding must succeed, got {put:?}");

    let read = xray.batch_get(std::slice::from_ref(&trace), None).await;
    let traces = array(&read.body, "Traces");
    let duration = traces[0].get("Duration").and_then(Value::as_f64);
    assert!(
        duration.is_some_and(|duration| close(duration, 0.6)),
        "the trace's Duration is its own segments' span, got {duration:?}"
    );
    let got = xray.summaries(WINDOW.0, WINDOW.1, None).await;
    let summaries = array(&got.body, "TraceSummaries");
    let response_time = summaries[0].get("ResponseTime").and_then(Value::as_f64);
    assert!(
        response_time.is_some_and(|response_time| close(response_time, 0.6)),
        "the trace's ResponseTime is its root segment's span, got {response_time:?}"
    );
    let service_names: BTreeSet<&str> = array(&summaries[0], "ServiceIds")
        .iter()
        .filter_map(|id| id.get("Name").and_then(Value::as_str))
        .collect();
    assert_eq!(
        service_names,
        BTreeSet::from(["client", "quartz"]),
        "the inferred service is one of the trace's services, got {service_names:?}"
    );
}
