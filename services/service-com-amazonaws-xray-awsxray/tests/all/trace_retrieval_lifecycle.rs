// SPDX-License-Identifier: CC0-1.0

//! The trace-retrieval lifecycle: `StartTraceRetrieval` ->
//! `ListRetrievedTraces` / `GetRetrievedTracesGraph` -> `CancelTraceRetrieval`,
//! end to end over the wire (the harness in `trace_ingest_and_groups`), plus the
//! stored record's own state machine over a clock the test controls.
//!
//! The tests are tables, and each row differs along what the rule it checks
//! generalises over — the instant a reader asks, the shape of the window a
//! segment lands in, the number of traces relative to a page, which reader is
//! called, which token is named — because the rules are:
//!
//! * a started retrieval is a STORED job whose `RetrievalStatus` is recomputed
//!   from the instants it was stored with: `SCHEDULED`, then `RUNNING`, then
//!   `COMPLETE`, and `CANCELLED` for good once a cancel stopped it;
//! * a reader of a retrieval token answers the status alone until that status is
//!   `COMPLETE` ("the API will return an empty response", `ListRetrievedTraces`;
//!   "The response is empty until the RetrievalStatus is COMPLETE",
//!   `GetRetrievedTracesGraph`);
//! * once it is `COMPLETE`, the traces are the STORED segment documents of the
//!   requested ids whose `start_time` falls in the retrieval's inclusive
//!   `[StartTime, EndTime]` window — nothing echoed from the request — and the
//!   graph over them is the one derivation every graph reader of this service
//!   uses;
//! * a page carries at most the modelled maximum of traces, and a `NextToken`
//!   is emitted only when traces are still unread;
//! * a `RetrievalToken` that is not of the form this service mints is refused
//!   with `InvalidRequestException` at HTTP 400 — before any lookup — by every
//!   operation that takes one, while a WELL-FORMED token no stored retrieval
//!   stands behind is refused with `ResourceNotFoundException` at HTTP 404;
//! * `ListRetrievedTraces` requires a `TraceFormat`, and judges it before it
//!   resolves the token; and
//! * `CancelTraceRetrieval` moves an ONGOING retrieval to `CANCELLED` and is
//!   refused with `InvalidRequestException` at HTTP 400 for one that is not
//!   ongoing, leaving that retrieval as it was.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::list_retrieved_traces::RETRIEVED_TRACES_PAGE_SIZE;
use service_com_amazonaws_xray_awsxray::state::{
    ComAmazonawsXrayAwsxrayState, RETRIEVAL_STATUS_CANCELLED, RETRIEVAL_STATUS_COMPLETE,
    RETRIEVAL_STATUS_RUNNING, RETRIEVAL_STATUS_SCHEDULED, TRACE_FORMAT_OTEL, TRACE_FORMAT_XRAY,
    TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS, TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS,
    TRACE_RETRIEVAL_TIMEOUT_SECONDS,
};

use crate::trace_ingest_and_groups::{array, close, root_document, trace_id, Answer, Xray};

/// Now, in the epoch seconds every timestamp on this wire travels as.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds()
}

/// Long enough for the clock to owe a started retrieval its `COMPLETE`: both
/// stored spans, plus slack for a loaded test machine.
fn completion_wait() -> Duration {
    Duration::from_secs_f64(
        TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS + TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS + 0.4,
    )
}

/// A window start ten minutes ago, on a WHOLE second.
///
/// A whole second is exactly representable both as the JSON double a segment
/// document carries its `start_time` as and as the modelled timestamp
/// `StartTraceRetrieval` takes, so a case about a segment landing exactly ON a
/// window bound compares exactly rather than against a value the nanosecond
/// resolution of the wire rounded.
fn base_window_start() -> f64 {
    (now() - 600.0).trunc()
}

/// A child segment document under `parent`.
fn child_document(id: &str, parent: &str, trace: &str, name: &str, start: f64, end: f64) -> Value {
    json!({
        "trace_id": trace,
        "id": id,
        "parent_id": parent,
        "name": name,
        "start_time": start,
        "end_time": end,
    })
}

/// Ingest segment documents through the front door.
async fn put_segments(xray: &Xray, documents: &[Value]) -> Answer {
    let documents: Vec<String> = documents.iter().map(Value::to_string).collect();
    xray.call(
        "/TraceSegments",
        &json!({ "TraceSegmentDocuments": documents }),
    )
    .await
}

/// Start a retrieval and answer the `RetrievalToken` it minted.
async fn start_retrieval(xray: &Xray, ids: &[String], start: f64, end: f64) -> String {
    let answer = xray
        .call(
            "/StartTraceRetrieval",
            &json!({"TraceIds": ids, "StartTime": start, "EndTime": end}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "starting a retrieval of {} id(s) must succeed, got {answer:?}",
        ids.len()
    );
    let token = answer
        .get("RetrievalToken")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "a started retrieval must answer a RetrievalToken, got {answer:?}"
    );
    token
}

/// `ListRetrievedTraces` for `token`, optionally resuming and in a format.
///
/// `TraceFormat` is required by the service ("TraceFormat cannot be empty"), so
/// a row that is not about that member sends the format the retrieved spans
/// exist in; the rows that ARE about it call the operation directly.
async fn list_retrieved(
    xray: &Xray,
    token: &str,
    next: Option<&str>,
    format: Option<&str>,
) -> Answer {
    let mut body = json!({ "RetrievalToken": token });
    if let Some(next) = next {
        body["NextToken"] = json!(next);
    }
    body["TraceFormat"] = json!(format.unwrap_or(TRACE_FORMAT_XRAY));
    xray.call("/ListRetrievedTraces", &body).await
}

/// `GetRetrievedTracesGraph` for `token`.
async fn retrieved_graph(xray: &Xray, token: &str) -> Answer {
    xray.call(
        "/GetRetrievedTracesGraph",
        &json!({ "RetrievalToken": token }),
    )
    .await
}

/// `CancelTraceRetrieval` for `token`.
async fn cancel_retrieval(xray: &Xray, token: &str) -> Answer {
    xray.call(
        "/CancelTraceRetrieval",
        &json!({ "RetrievalToken": token }),
    )
    .await
}

/// The `RetrievalStatus` an answer reports.
fn status_of(answer: &Answer) -> String {
    answer
        .get("RetrievalStatus")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every `RetrievedTrace$Id` in a `ListRetrievedTraces` answer.
fn listed_ids(answer: &Answer) -> Vec<String> {
    array(&answer.body, "Traces")
        .iter()
        .filter_map(|trace| trace.get("Id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

// =============================================================================
// 1. The stored retrieval's state machine, over a clock the test states
// =============================================================================

#[tokio::test]
async fn a_stored_retrieval_reports_the_status_its_instants_owe_at_every_offset() {
    // The store is driven directly here so the clock is an argument rather than
    // a sleep: every offset of the lifecycle can be asked about exactly.
    let mut store = ComAmazonawsXrayAwsxrayState::default();
    let started = 1_700_000_000.0_f64;
    let scheduled = TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS;
    let running = TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS;
    let token = store
        .start_trace_retrieval(
            vec![trace_id(1), trace_id(2)],
            started - 300.0,
            started,
            started,
        )
        .retrieval_token
        .clone();

    // Rows differ in WHEN the reader asks, relative to the two stored spans.
    let offsets: [(&str, f64, &str); 6] = [
        ("the instant it was started", 0.0, RETRIEVAL_STATUS_SCHEDULED),
        (
            "just before the scheduled span is up",
            scheduled - 0.001,
            RETRIEVAL_STATUS_SCHEDULED,
        ),
        (
            "the instant the scheduled span is up",
            scheduled,
            RETRIEVAL_STATUS_RUNNING,
        ),
        (
            "just before the running span is up",
            scheduled + running - 0.001,
            RETRIEVAL_STATUS_RUNNING,
        ),
        (
            "the instant the running span is up",
            scheduled + running,
            RETRIEVAL_STATUS_COMPLETE,
        ),
        (
            "a second before the retrieval times out",
            TRACE_RETRIEVAL_TIMEOUT_SECONDS - 1.0,
            RETRIEVAL_STATUS_COMPLETE,
        ),
    ];

    for (case, offset, expected) in offsets {
        let actual = store.retrievals[&token].status(started + offset);
        assert_eq!(
            actual, expected,
            "case {case:?}: the status owed at +{offset}s must be {expected}, got {actual}"
        );
    }

    // The token resolves while the retrieval is inside its documented 60-minute
    // life, and no longer once the clock has run it out.
    assert!(
        store
            .trace_retrieval(&token, started + TRACE_RETRIEVAL_TIMEOUT_SECONDS - 1.0)
            .is_some(),
        "a retrieval inside its {TRACE_RETRIEVAL_TIMEOUT_SECONDS}s life must still resolve"
    );
    assert!(
        store
            .trace_retrieval(&token, started + TRACE_RETRIEVAL_TIMEOUT_SECONDS)
            .is_none(),
        "a retrieval the clock has timed out must resolve to nothing"
    );
}

#[tokio::test]
async fn a_cancelled_retrieval_reports_cancelled_at_every_later_offset() {
    let mut store = ComAmazonawsXrayAwsxrayState::default();
    let started = 1_700_000_000.0_f64;
    let token = store
        .start_trace_retrieval(vec![trace_id(1)], started - 300.0, started, started)
        .retrieval_token
        .clone();

    let cancelled_at = started + TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS;
    assert!(
        store.cancel_trace_retrieval(&token, cancelled_at).is_ok(),
        "a running retrieval must be cancellable"
    );

    // Rows differ in how far past the cancellation the reader asks: the clock
    // never moves a cancelled retrieval on.
    for (case, offset) in [
        ("the instant it was cancelled", 0.0),
        ("once the running span would have been up", 1.0),
        ("much later", TRACE_RETRIEVAL_TIMEOUT_SECONDS - 1.0),
    ] {
        let actual = store.retrievals[&token].status(cancelled_at + offset);
        assert_eq!(
            actual, RETRIEVAL_STATUS_CANCELLED,
            "case {case:?}: a cancelled retrieval must stay {RETRIEVAL_STATUS_CANCELLED}, got {actual}"
        );
    }

    assert_eq!(
        store.cancel_trace_retrieval(&token, cancelled_at + 1.0).err(),
        Some(Some(RETRIEVAL_STATUS_CANCELLED)),
        "cancelling a cancelled retrieval must report the status it is in"
    );
    assert_eq!(
        store.cancel_trace_retrieval("a token nothing minted", cancelled_at).err(),
        Some(None),
        "cancelling an unknown token must report no retrieval at all"
    );
}

// =============================================================================
// 2. Over the wire: empty until COMPLETE, then the stored documents read back
// =============================================================================

#[tokio::test]
async fn a_retrieval_answers_nothing_until_it_completes_and_then_reads_back() {
    let xray = Xray::new();
    let base = base_window_start();
    let trace = trace_id(401);
    let root = root_document("1111111111111111", &trace, "frontend", base + 10.0, Some(base + 11.0));
    let child = child_document(
        "2222222222222222",
        "1111111111111111",
        &trace,
        "backend",
        base + 10.2,
        base + 10.8,
    );
    let ingested = put_segments(&xray, &[root.clone(), child.clone()]).await;
    assert_eq!(
        ingested.status, 200,
        "the segments must be ingested, got {ingested:?}"
    );

    let token = start_retrieval(&xray, std::slice::from_ref(&trace), base, base + 300.0).await;

    // Both readers, asked straight away. The rule asserted here is the one that
    // does not depend on how fast the machine answered: a reader carries results
    // only once the status is COMPLETE. (Which status is owed at which instant is
    // pinned exactly, without a clock race, by the state-machine test above.)
    for (case, answer) in [
        ("ListRetrievedTraces", list_retrieved(&xray, &token, None, None).await),
        ("GetRetrievedTracesGraph", retrieved_graph(&xray, &token).await),
    ] {
        assert_eq!(
            answer.status, 200,
            "case {case:?}: reading a retrieval that has not completed must succeed, got {answer:?}"
        );
        let status = status_of(&answer);
        assert!(
            [
                RETRIEVAL_STATUS_SCHEDULED,
                RETRIEVAL_STATUS_RUNNING,
                RETRIEVAL_STATUS_COMPLETE,
            ]
            .contains(&status.as_str()),
            "case {case:?}: a started retrieval must report {RETRIEVAL_STATUS_SCHEDULED}, {RETRIEVAL_STATUS_RUNNING} or {RETRIEVAL_STATUS_COMPLETE}, got {answer:?}"
        );
        assert!(
            status == RETRIEVAL_STATUS_COMPLETE
                || (array(&answer.body, "Traces").is_empty()
                    && array(&answer.body, "Services").is_empty()),
            "case {case:?}: the response must be empty until the status is {RETRIEVAL_STATUS_COMPLETE}, got {answer:?}"
        );
    }

    tokio::time::sleep(completion_wait()).await;

    let listed = list_retrieved(&xray, &token, None, None).await;
    assert_eq!(
        (listed.status, status_of(&listed).as_str()),
        (200, RETRIEVAL_STATUS_COMPLETE),
        "once the clock has completed the retrieval it must report {RETRIEVAL_STATUS_COMPLETE}, got {listed:?}"
    );
    assert_eq!(
        listed.get("TraceFormat").as_str(),
        Some(TRACE_FORMAT_XRAY),
        "the traces must be reported in the format they were ingested in, got {listed:?}"
    );
    assert_eq!(
        listed_ids(&listed),
        vec![trace.clone()],
        "the retrieval must answer the requested trace, got {listed:?}"
    );

    let traces = array(&listed.body, "Traces");
    let spans = array(&traces[0], "Spans");
    let documents: BTreeSet<String> = spans
        .iter()
        .filter_map(|span| span.get("Document").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let submitted: BTreeSet<String> = [root.to_string(), child.to_string()].into_iter().collect();
    assert_eq!(
        documents, submitted,
        "every span must carry the stored document byte for byte, got {listed:?}"
    );
    let span_ids: BTreeSet<&str> = spans
        .iter()
        .filter_map(|span| span.get("Id").and_then(Value::as_str))
        .collect();
    assert_eq!(
        span_ids,
        ["1111111111111111", "2222222222222222"].into_iter().collect::<BTreeSet<_>>(),
        "every span must be named by its stored segment id, got {listed:?}"
    );
    let duration = traces[0].get("Duration").and_then(Value::as_f64).unwrap_or(-1.0);
    assert!(
        close(duration, 1.0),
        "the duration must be recomputed from the stored segments (1.0s), got {duration}"
    );

    // A read changes nothing a later reader sees.
    let again = list_retrieved(&xray, &token, None, None).await;
    assert_eq!(
        again.body, listed.body,
        "reading a completed retrieval twice must answer the same thing, got {again:?}"
    );

    // The graph over the same retrieval is derived from those documents: a node
    // for every service that handled a stored segment, and an edge from the
    // caller to the service it called. The assertions address nodes by name and
    // role rather than by position, so a node the derivation adds beside them —
    // the synthesised client of the root segment — does not change what the rule
    // says.
    let graphed = retrieved_graph(&xray, &token).await;
    assert_eq!(
        (graphed.status, status_of(&graphed).as_str()),
        (200, RETRIEVAL_STATUS_COMPLETE),
        "the graph of a completed retrieval must report {RETRIEVAL_STATUS_COMPLETE}, got {graphed:?}"
    );
    let services: Vec<&Value> = array(&graphed.body, "Services")
        .iter()
        .filter_map(|retrieved| retrieved.get("Service"))
        .collect();
    let names: BTreeSet<&str> = services
        .iter()
        .filter_map(|service| service.get("Name").and_then(Value::as_str))
        .collect();
    for stored in ["frontend", "backend"] {
        assert!(
            names.contains(stored),
            "the graph must carry a node for the stored service {stored:?}, got {graphed:?}"
        );
    }
    let node = |name: &str, root: bool| -> Option<&Value> {
        services.iter().copied().find(|service| {
            service.get("Name").and_then(Value::as_str) == Some(name)
                && service.get("Root").and_then(Value::as_bool) == Some(root)
        })
    };
    let Some(caller) = node("frontend", true) else {
        panic!("the graph must carry the stored trace's root node, got {graphed:?}");
    };
    let Some(called) = node("backend", false) else {
        panic!("the graph must carry the service the root called, got {graphed:?}");
    };
    let called_reference = called.get("ReferenceId").and_then(Value::as_i64);
    let caller_edges: Vec<i64> = array(caller, "Edges")
        .iter()
        .filter_map(|edge| edge.get("ReferenceId").and_then(Value::as_i64))
        .collect();
    assert!(
        called_reference.is_some_and(|reference| caller_edges.contains(&reference)),
        "the caller's edge must name the service it called ({called_reference:?} among {caller_edges:?}), got {graphed:?}"
    );
}

// =============================================================================
// 3. What a retrieval holds: the requested ids inside the inclusive window
// =============================================================================

/// One requested trace id, and whether the retrieval must answer it.
struct MembershipRow {
    case: &'static str,
    /// Where this trace's one segment starts, relative to the window's start.
    segment_offset: Option<f64>,
    /// How many times the retrieval asks for this id.
    requested: usize,
    answered: bool,
}

#[tokio::test]
async fn a_retrieval_answers_the_requested_ids_whose_segments_fall_in_its_window() {
    let xray = Xray::new();
    let base = base_window_start();
    let window = 300.0;

    // Rows differ in where the trace's segment lands relative to the window's
    // bounds, in whether it is stored at all, and in how often it was asked for.
    let rows = [
        MembershipRow {
            case: "a segment exactly at StartTime",
            segment_offset: Some(0.0),
            requested: 1,
            answered: true,
        },
        MembershipRow {
            case: "a segment inside the window",
            segment_offset: Some(window / 2.0),
            requested: 1,
            answered: true,
        },
        MembershipRow {
            case: "a segment exactly at EndTime",
            segment_offset: Some(window),
            requested: 1,
            answered: true,
        },
        MembershipRow {
            case: "a segment one second past EndTime",
            segment_offset: Some(window + 1.0),
            requested: 1,
            answered: false,
        },
        MembershipRow {
            case: "a segment one second before StartTime",
            segment_offset: Some(-1.0),
            requested: 1,
            answered: false,
        },
        MembershipRow {
            case: "an id that names no stored trace",
            segment_offset: None,
            requested: 1,
            answered: false,
        },
        MembershipRow {
            case: "an id asked for three times",
            segment_offset: Some(1.0),
            requested: 3,
            answered: true,
        },
    ];

    let mut requested: Vec<String> = Vec::new();
    let mut expected: Vec<String> = Vec::new();
    for (at, row) in rows.iter().enumerate() {
        let trace = trace_id(420 + u32::try_from(at).unwrap_or(0));
        if let Some(offset) = row.segment_offset {
            let start = base + offset;
            let document = root_document(
                &format!("{:016x}", 0x5100 + at),
                &trace,
                "frontend",
                start,
                Some(start + 0.5),
            );
            let ingested = put_segments(&xray, &[document]).await;
            assert_eq!(
                ingested.status, 200,
                "case {:?}: the segment must be ingested, got {ingested:?}",
                row.case
            );
        }
        for _ in 0..row.requested {
            requested.push(trace.clone());
        }
        if row.answered {
            expected.push(trace.clone());
        }
    }

    let token = start_retrieval(&xray, &requested, base, base + window).await;
    tokio::time::sleep(completion_wait()).await;
    let listed = list_retrieved(&xray, &token, None, None).await;
    assert_eq!(
        (listed.status, status_of(&listed).as_str()),
        (200, RETRIEVAL_STATUS_COMPLETE),
        "the retrieval must have completed, got {listed:?}"
    );

    let answered = listed_ids(&listed);
    assert_eq!(
        answered, expected,
        "the retrieval must answer exactly the stored ids inside its window, once each, in the order asked for; cases: {:?}",
        rows.iter().map(|row| (row.case, row.answered)).collect::<Vec<_>>()
    );
}

// =============================================================================
// 4. Pagination, at the modelled bound
// =============================================================================

#[tokio::test]
async fn a_retrieval_of_more_traces_than_a_page_is_paginated() {
    let xray = Xray::new();
    let base = base_window_start();
    // Two full pages and a partial one, so a token is emitted, resumed and
    // finally withheld.
    let count = RETRIEVED_TRACES_PAGE_SIZE * 2 + 2;
    let mut ids = Vec::new();
    for at in 0..count {
        let trace = trace_id(450 + u32::try_from(at).unwrap_or(0));
        let document = root_document(
            &format!("{:016x}", 0x6100 + at),
            &trace,
            "frontend",
            base + 1.0,
            Some(base + 2.0),
        );
        let ingested = put_segments(&xray, &[document]).await;
        assert_eq!(
            ingested.status, 200,
            "case {at}: the segment must be ingested, got {ingested:?}"
        );
        ids.push(trace);
    }

    let token = start_retrieval(&xray, &ids, base, base + 300.0).await;
    tokio::time::sleep(completion_wait()).await;

    let mut seen: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = list_retrieved(&xray, &token, next.as_deref(), None).await;
        assert_eq!(
            page.status, 200,
            "page {pages}: walking a retrieval must succeed, got {page:?}"
        );
        let on_page = listed_ids(&page);
        assert!(
            on_page.len() <= RETRIEVED_TRACES_PAGE_SIZE,
            "page {pages}: a page carries at most {RETRIEVED_TRACES_PAGE_SIZE} traces, got {}",
            on_page.len()
        );
        seen.extend(on_page);
        pages += 1;
        next = page
            .get("NextToken")
            .as_str()
            .map(str::to_string);
        if next.is_none() {
            break;
        }
        assert!(
            pages <= count,
            "the walk must terminate; it is on page {pages} of {count} traces"
        );
    }

    assert_eq!(
        seen, ids,
        "the walk must yield every retrieved trace exactly once, in {pages} page(s)"
    );

    // A page token this operation never issued is refused rather than answered.
    let past_end = count.to_string();
    let off_boundary = (RETRIEVED_TRACES_PAGE_SIZE + 1).to_string();
    for (case, bad) in [
        ("a token that is not a number", "page-two"),
        ("a token past the end of the retrieval", past_end.as_str()),
        ("a token that is not on a page boundary", off_boundary.as_str()),
    ] {
        let answer = list_retrieved(&xray, &token, Some(bad), None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: a NextToken this service never issued must be refused, got {answer:?}"
        );
    }
}

// =============================================================================
// 5. A token no retrieval stands behind
// =============================================================================

#[tokio::test]
async fn a_malformed_token_is_a_bad_request_and_a_well_formed_unknown_one_is_not_found() {
    let xray = Xray::new();
    let base = base_window_start();
    // One real retrieval, so the store is not empty and the refusals below are
    // about the token rather than about an empty service.
    let real = start_retrieval(&xray, &[trace_id(480)], base, base + 300.0).await;
    assert!(
        real.len() == 32 && real.chars().all(|c| c.is_ascii_hexdigit()),
        "a minted RetrievalToken is 32 hexadecimal digits, got {real:?}"
    );
    let mut altered: Vec<char> = real.chars().collect();
    altered[0] = if altered[0] == 'a' { 'b' } else { 'a' };
    let altered: String = altered.into_iter().collect();

    // Rows differ in HOW the token misses: one that could never have been
    // minted (400, a request to fix) against one that could have been and was
    // not (404, a retrieval that is not there). The lengths sit either side of
    // the minted width, and `RetrievalToken`'s modelled `@length` is 0..1020, so
    // every row below reaches the handler.
    let malformed = [
        ("the empty token", String::new()),
        ("a token that is not hexadecimal", "not-a-retrieval".to_string()),
        (
            "a UUID, which is not the form this service mints",
            "59adda79-1d1f-4b1e-9f3a-2c7d5e8a9b01".to_string(),
        ),
        ("one digit short of the minted width", "a".repeat(31)),
        ("one digit past the minted width", "a".repeat(33)),
        ("the longest token the model allows", "a".repeat(1_020)),
    ];
    let well_formed = [
        ("a well-formed token nothing minted", "0".repeat(32)),
        ("the same, in upper case", "0123456789ABCDEF".repeat(2)),
        ("one character off a real token", altered),
    ];
    let operations = [
        "/ListRetrievedTraces",
        "/GetRetrievedTracesGraph",
        "/CancelTraceRetrieval",
    ];

    // `TraceFormat` is a member of `ListRetrievedTraces` alone, and that
    // operation requires it, so the body carries it only there: these rows are
    // about the token, not about the format.
    let body = |path: &str, token: &str| -> Value {
        let mut body = json!({ "RetrievalToken": token });
        if path == "/ListRetrievedTraces" {
            body["TraceFormat"] = json!(TRACE_FORMAT_XRAY);
        }
        body
    };

    for path in operations {
        for (case, token) in &malformed {
            let answer = xray.call(path, &body(path, token)).await;
            assert_eq!(
                (answer.status, answer.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} on {path}: a token that could never have been minted must be refused as an invalid request, got {answer:?}"
            );
            assert!(
                answer
                    .message()
                    .is_some_and(|message| message.contains(token.as_str())),
                "case {case:?} on {path}: the refusal must name the token it could not read, got {answer:?}"
            );
        }
        for (case, token) in &well_formed {
            let answer = xray.call(path, &body(path, token)).await;
            assert_eq!(
                (answer.status, answer.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} on {path}: a well-formed token no retrieval stands behind must be refused as not found, got {answer:?}"
            );
        }
    }
}

// =============================================================================
// 6. Cancelling, and refusing to cancel what is not ongoing
// =============================================================================

#[tokio::test]
async fn cancel_stops_an_ongoing_retrieval_and_is_refused_once_it_is_not_ongoing() {
    let xray = Xray::new();
    let base = base_window_start();
    let trace = trace_id(490);
    let document = root_document("7777777777777777", &trace, "frontend", base + 1.0, Some(base + 2.0));
    let ingested = put_segments(&xray, &[document]).await;
    assert_eq!(
        ingested.status, 200,
        "the segment must be ingested, got {ingested:?}"
    );

    // An ongoing retrieval is stopped, and stays stopped.
    let ongoing = start_retrieval(&xray, std::slice::from_ref(&trace), base, base + 300.0).await;
    let cancelled = cancel_retrieval(&xray, &ongoing).await;
    assert_eq!(
        cancelled.status, 200,
        "cancelling an ongoing retrieval must answer HTTP 200, got {cancelled:?}"
    );

    tokio::time::sleep(completion_wait()).await;
    for (case, answer) in [
        ("ListRetrievedTraces", list_retrieved(&xray, &ongoing, None, None).await),
        ("GetRetrievedTracesGraph", retrieved_graph(&xray, &ongoing).await),
    ] {
        assert_eq!(
            (answer.status, status_of(&answer).as_str()),
            (200, RETRIEVAL_STATUS_CANCELLED),
            "case {case:?}: a cancelled retrieval must report {RETRIEVAL_STATUS_CANCELLED} even once its spans are up, got {answer:?}"
        );
        assert!(
            array(&answer.body, "Traces").is_empty() && array(&answer.body, "Services").is_empty(),
            "case {case:?}: a cancelled retrieval must answer nothing, got {answer:?}"
        );
    }

    // A retrieval that completed, and one already cancelled, are both no longer
    // ongoing jobs: there is nothing left to cancel.
    let completed = start_retrieval(&xray, std::slice::from_ref(&trace), base, base + 300.0).await;
    tokio::time::sleep(completion_wait()).await;
    for (case, token) in [
        ("a retrieval that already completed", &completed),
        ("a retrieval that was already cancelled", &ongoing),
    ] {
        let answer = cancel_retrieval(&xray, token).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: cancelling what is not ongoing must be refused, got {answer:?}"
        );
    }

    // And the refused cancel left the completed retrieval readable.
    let listed = list_retrieved(&xray, &completed, None, None).await;
    assert_eq!(
        (listed.status, status_of(&listed).as_str(), listed_ids(&listed)),
        (200, RETRIEVAL_STATUS_COMPLETE, vec![trace.clone()]),
        "a refused cancel must leave the retrieval as it was, got {listed:?}"
    );
}

// =============================================================================
// 7. The format these spans exist in
// =============================================================================

#[tokio::test]
async fn a_retrieval_is_answered_in_the_format_its_spans_were_ingested_in() {
    let xray = Xray::new();
    let base = base_window_start();
    let trace = trace_id(495);
    let document = root_document("8888888888888888", &trace, "frontend", base + 1.0, Some(base + 2.0));
    let ingested = put_segments(&xray, &[document]).await;
    assert_eq!(
        ingested.status, 200,
        "the segment must be ingested, got {ingested:?}"
    );
    let token = start_retrieval(&xray, std::slice::from_ref(&trace), base, base + 300.0).await;
    tokio::time::sleep(completion_wait()).await;

    // Rows differ in the requested TraceFormat: the one the stored spans are in,
    // one no stored span exists in, and the member omitted — which this
    // operation requires, so it is refused as an invalid request.
    for (case, format, expected_status) in [
        (
            "the format the spans were ingested in",
            Some(TRACE_FORMAT_XRAY),
            200,
        ),
        ("a format no stored span exists in", Some(TRACE_FORMAT_OTEL), 400),
        ("TraceFormat omitted", None, 400),
    ] {
        let mut body = json!({ "RetrievalToken": &token });
        if let Some(format) = format {
            body["TraceFormat"] = json!(format);
        }
        let answer = xray.call("/ListRetrievedTraces", &body).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        if expected_status == 200 {
            assert_eq!(
                (answer.get("TraceFormat").as_str(), listed_ids(&answer)),
                (Some(TRACE_FORMAT_XRAY), vec![trace.clone()]),
                "case {case:?}: the answer must report the stored format and the trace, got {answer:?}"
            );
        } else {
            assert_eq!(
                answer.error_type.as_deref(),
                Some("InvalidRequestException"),
                "case {case:?}: a format these spans do not exist in must be refused, got {answer:?}"
            );
        }
    }
}
