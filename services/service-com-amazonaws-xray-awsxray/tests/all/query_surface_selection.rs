// SPDX-License-Identifier: CC0-1.0

//! What a QUERY of this service selects, and what the selection reports, end to
//! end over the wire (the harness in `xray_wire`).
//!
//! Every case ingests through `PutTraceSegments` — or seeds the one store no
//! operation in the API writes, the insights — and then asserts the answer is
//! recomputed from what is stored. Each test is a table whose rows differ along
//! what the rule it checks generalises over, because the rules are:
//!
//! * `GetInsightEvents` reads the event timeline of the insight its `InsightId`
//!   names, and an id that names none is refused with `InvalidRequestException`
//!   (HTTP 400) in the same words every reader of an insight refuses it with —
//!   never with a 501 and never with an empty timeline;
//! * `GetTimeSeriesServiceStatistics$EntitySelectorExpression` RESTRICTS the
//!   aggregation to the entities its `id` / `service` / `edge` functions name:
//!   a service selector reports `ServiceSummaryStatistics` for that service's
//!   own requests, an edge selector `EdgeSummaryStatistics` for the calls that
//!   edge carried, and a selector naming nothing the store recorded is answered
//!   with an empty series rather than refused;//! * `GetTraceSummaries$TimeRangeType` chooses WHICH instant of a trace the
//!   window is applied to — its start time, its update time or its segment end
//!   time — so the same window over the same store selects differently under
//!   each member;
//! * a `rootcause.<class>.<part> { … }` block scopes its keywords to the members
//!   of that part of a root cause, so it selects exactly what the written-out
//!   path selects, and a comparison inside it is type-checked like any other;
//! * a trace's `ResponseTimeRootCauses` is derived from the TIMING the stored
//!   documents recorded: the chain of components the response time was spent
//!   in, each frame carrying the part of it the frame ran for, which is what
//!   separates a dependency-bound trace from a pipeline-bound one; and
//! * the account's encryption configuration identifies its KMS key by ARN,
//!   qualified with the region the configuration was put in.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::get_insight::INSIGHT_NOT_FOUND;
use service_com_amazonaws_xray_awsxray::ops::types::{Insight, RequestImpactStatistics};
use service_com_amazonaws_xray_awsxray::state::{
    ComAmazonawsXrayAwsxrayState, RESPONSE_TIME_ROOT_CAUSE_MIN_COVERAGE,
};

use crate::xray_wire::{array, close, root_document, trace_id, Answer, Xray};

/// A 60-second-aligned epoch instant, so a case's periods are easy to read:
/// `1_699_999_980 == 60 * 28_333_333`.
const ALIGNED: f64 = 1_699_999_980.0;

// =============================================================================
// 1. GetInsightEvents: the timeline of the insight the id names
// =============================================================================

/// Seed one insight into the store no operation in the API writes, with the
/// instants and statistics an event is reported from.
fn seed_insight(engine: &Arc<engine::InMemoryEngine>, id: &str, ended: Option<f64>) {
    let state = engine.state::<ComAmazonawsXrayAwsxrayState>();
    state.lock().insights.insert(
        id.to_string(),
        Insight {
            insight_id: Some(id.to_string()),
            state: Some(if ended.is_some() { "CLOSED" } else { "ACTIVE" }.to_string()),
            summary: Some("Fault rate above the expected range".to_string()),
            start_time: Some(::aws_smithy_types::DateTime::from_secs_f64(ALIGNED)),
            end_time: ended.map(::aws_smithy_types::DateTime::from_secs_f64),
            client_request_impact_statistics: Some(RequestImpactStatistics {
                fault_count: Some(7),
                ok_count: Some(93),
                total_count: Some(100),
            }),
            ..Insight::default()
        },
    );
}

/// `GetInsightEvents` for an insight id, optionally paged.
async fn insight_events(
    xray: &Xray,
    id: &str,
    max_results: Option<i64>,
    token: Option<&str>,
) -> Answer {
    let mut body = json!({ "InsightId": id });
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    xray.call("/InsightEvents", &body).await
}

#[tokio::test]
async fn insight_events_refuse_an_id_that_names_no_insight() {
    let (xray, engine) = Xray::with_engine();
    // One stored insight, so the refusals below are about the id and not about
    // an empty store.
    seed_insight(&engine, "11111111-2222-4333-8444-555555555555", None);

    // Rows differ in how the id is written and which insight it misses: the
    // digits, the UUID version and variant nibbles the modelled `@pattern`
    // allows, upper case, and an id one digit off the stored one. Every row
    // satisfies `InsightId`'s pattern, so each reaches the handler.
    let unknown = [
        ("the id the issue's repro names", "12345678-1234-4123-8123-123456789012"),
        ("another version nibble", "12345678-1234-5123-9123-123456789012"),
        ("the lowest digits", "00000000-0000-1000-8000-000000000000"),
        ("upper-case hexadecimal", "ABCDEF12-3456-4789-BCDE-F123456789AB"),
        (
            "one digit off the stored insight",
            "11111111-2222-4333-8444-555555555556",
        ),
    ];
    for (case, id) in unknown {
        let refused = insight_events(&xray, id, None, None).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: an id naming no insight must be refused with the modelled error, got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            Some(INSIGHT_NOT_FOUND),
            "case {case:?}: the refusal must read as every reader of an insight words it, got {refused:?}"
        );
    }

    // And the stored insight's own timeline is answered from the insight: one
    // event per state it is known to have been in, oldest first, each carrying
    // the insight's summary and impact statistics.
    let answered = insight_events(&xray, "11111111-2222-4333-8444-555555555555", None, None).await;
    assert_eq!(
        answered.status, 200,
        "a stored insight must be answered, got {answered:?}"
    );
    let events = array(&answered.body, "InsightEvents");
    assert_eq!(
        events.len(),
        1,
        "an insight that has not ended is known in one state, got {answered:?}"
    );
    assert!(
        events[0]
            .get("EventTime")
            .and_then(Value::as_f64)
            .is_some_and(|at| close(at, ALIGNED)),
        "the event must be timed from the insight, got {answered:?}"
    );
    assert_eq!(
        events[0].get("Summary").and_then(Value::as_str),
        Some("Fault rate above the expected range"),
        "the event must carry the insight's summary, got {answered:?}"
    );
    assert_eq!(
        events[0]
            .get("ClientRequestImpactStatistics")
            .and_then(|statistics| statistics.get("FaultCount"))
            .and_then(Value::as_i64),
        Some(7),
        "the event must carry the insight's client impact statistics, got {answered:?}"
    );
    assert!(
        answered.get("NextToken").is_null(),
        "a timeline that fits one page carries no token, got {answered:?}"
    );
}

#[tokio::test]
async fn insight_events_page_the_timeline_and_resume_exactly_once() {
    let (xray, engine) = Xray::with_engine();
    let id = "22222222-3333-4444-8555-666666666666";
    // An insight that has ended is known in two states: the one it began in and
    // the one it was last reevaluated into.
    seed_insight(&engine, id, Some(ALIGNED + 300.0));

    let whole = insight_events(&xray, id, None, None).await;
    assert_eq!(
        array(&whole.body, "InsightEvents").len(),
        2,
        "an insight that began and ended is known in two states, got {whole:?}"
    );

    // One event per page: a token is emitted only while events are unread, and
    // resuming from it yields the remainder exactly once.
    let mut seen: Vec<f64> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..4 {
        let answered = insight_events(&xray, id, Some(1), token.as_deref()).await;
        assert_eq!(
            answered.status, 200,
            "page {page}: expected HTTP 200, got {answered:?}"
        );
        let events = array(&answered.body, "InsightEvents");
        assert_eq!(
            events.len(),
            1,
            "page {page}: MaxResults 1 carries one event, got {answered:?}"
        );
        seen.push(
            events[0]
                .get("EventTime")
                .and_then(Value::as_f64)
                .unwrap_or(-1.0),
        );
        token = answered
            .get("NextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen.len(),
        2,
        "every event must be read exactly once, got {seen:?}"
    );
    assert!(
        close(seen[0], ALIGNED) && close(seen[1], ALIGNED + 300.0),
        "the timeline must ascend: expected [{ALIGNED}, {}], got {seen:?}",
        ALIGNED + 300.0
    );

    // A token this service never issued is refused rather than answered.
    let refused = insight_events(&xray, id, None, Some("page-two")).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "a token this service never issued must be refused, got {refused:?}"
    );
}

// =============================================================================
// 2. An EntitySelectorExpression restricts which entities are aggregated
// =============================================================================

/// The traffic the selector rows are evaluated over: a `frontend` request that
/// called `backend`, a second `frontend` request one period later, and one
/// `other` service's request — so a selector along any of name, type, account,
/// edge direction and period selects a different subset.
async fn seed_entities(xray: &Xray) {
    let first = trace_id(701);
    let second = trace_id(702);
    let mut frontend = root_document("7010000000000001", &first, "frontend", ALIGNED + 1.0, Some(ALIGNED + 2.0));
    frontend["origin"] = json!("AWS::EC2::Instance");
    frontend["account_id"] = json!("000000000001");
    let mut backend = root_document("7010000000000002", &first, "backend", ALIGNED + 1.2, Some(ALIGNED + 1.8));
    backend["parent_id"] = json!("7010000000000001");
    let mut later = root_document("7020000000000001", &second, "frontend", ALIGNED + 61.0, Some(ALIGNED + 61.5));
    later["origin"] = json!("AWS::EC2::Instance");
    let other = root_document("7020000000000002", &second, "other", ALIGNED + 61.0, Some(ALIGNED + 61.25));

    let put = xray
        .put_segments(&[
            frontend.to_string(),
            backend.to_string(),
            later.to_string(),
            other.to_string(),
        ])
        .await;
    assert_eq!(
        (put.status, array(&put.body, "UnprocessedTraceSegments").len()),
        (200, 0),
        "seeding the entity fixture must store every document, got {put:?}"
    );
}

/// `GetTimeSeriesServiceStatistics` over the fixture's window, with a selector.
async fn statistics(xray: &Xray, selector: Option<&str>, period: i64) -> Answer {
    let mut body = json!({
        "StartTime": ALIGNED,
        "EndTime": ALIGNED + 120.0,
        "Period": period,
    });
    if let Some(selector) = selector {
        body["EntitySelectorExpression"] = json!(selector);
    }
    xray.call("/TimeSeriesServiceStatistics", &body).await
}

/// One period's `TotalCount`, read from whichever summary the answer carries.
fn totals(period: &Value) -> (Option<i64>, Option<i64>) {
    let count = |member: &str| {
        period
            .get(member)
            .and_then(|summary| summary.get("TotalCount"))
            .and_then(Value::as_i64)
    };
    (count("ServiceSummaryStatistics"), count("EdgeSummaryStatistics"))
}

#[tokio::test]
async fn the_entity_selector_expression_restricts_what_is_aggregated() {
    let xray = Xray::new();
    seed_entities(&xray).await;

    /// `(service totals, edge totals)` a row's periods must report, in period
    /// order.
    type Periods = Vec<(Option<i64>, Option<i64>)>;

    // Rows differ along the selector's function (`id`, `service`, `edge`), the
    // members it labels (name, type, account id), whether they match, and the
    // direction of an edge.
    let rows: Vec<(&str, Option<&str>, Periods)> = vec![
        // No selector at all: every completed request in the window, as edge
        // statistics ("if no selector expression is specified, edge statistics
        // are returned").
        ("no selector", None, vec![(None, Some(2)), (None, Some(2))]),
        // The recorded case: one service's two requests, a period apart, is two
        // periods of its own statistics. A selected service's requests are the
        // calls its inbound edges carried, so the same counts stand under both
        // summaries.
        (
            "a bare id function naming one service",
            Some("id(name: \"frontend\")"),
            vec![(Some(1), Some(1)), (Some(1), Some(1))],
        ),
        (
            "the same service wrapped in a service function",
            Some("service(id(name: \"frontend\"))"),
            vec![(Some(1), Some(1)), (Some(1), Some(1))],
        ),
        (
            "a service whose type the segments report",
            Some("service(id(name: \"frontend\", type: \"AWS::EC2::Instance\"))"),
            vec![(Some(1), Some(1)), (Some(1), Some(1))],
        ),
        (
            "a service whose type they do not",
            Some("service(id(name: \"frontend\", type: \"AWS::ECS::Container\"))"),
            vec![],
        ),
        (
            "a service in the account one segment names",
            Some("service(id(name: \"frontend\", account.id: \"000000000001\"))"),
            vec![(Some(1), Some(1))],
        ),
        (
            "a service in an account nothing names",
            Some("service(id(name: \"frontend\", account.id: \"000000000009\"))"),
            vec![],
        ),
        (
            "one request of another service",
            Some("id(name: \"other\")"),
            vec![(Some(1), Some(1))],
        ),
        (
            "an edge the stored parent link carries",
            Some("edge(id(name: \"frontend\"), id(name: \"backend\"))"),
            vec![(None, Some(1))],
        ),
        (
            "the same edge the other way about",
            Some("edge(id(name: \"backend\"), id(name: \"frontend\"))"),
            vec![],
        ),
        (
            "a selector naming nothing the store recorded",
            Some("service(id(name: \"nobody\"))"),
            vec![],
        ),
    ];

    for (case, selector, expected) in rows {
        let answered = statistics(&xray, selector, 60).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: a well-formed selector is answered, not refused, got {answered:?}"
        );
        let periods = array(&answered.body, "TimeSeriesServiceStatistics");
        let actual: Periods = periods.iter().map(totals).collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected (service, edge) TotalCount per period {expected:?}, got {answered:?}"
        );
        // A selector that narrows the aggregation never claims to have mixed
        // two versions of a group's expression: nothing here reads one.
        assert_eq!(
            answered.get("ContainsOldGroupVersions"),
            &json!(false),
            "case {case:?}: the aggregation spans no changed group version, got {answered:?}"
        );
    }

    // And the two periods the selector reports are the periods the requests
    // landed in, a minute apart: a wider period merges them into one.
    let merged = statistics(&xray, Some("id(name: \"frontend\")"), 300).await;
    let periods = array(&merged.body, "TimeSeriesServiceStatistics");
    assert_eq!(
        periods.len(),
        1,
        "a 300-second period merges the two requests, got {merged:?}"
    );
    assert_eq!(
        totals(&periods[0]),
        (Some(2), Some(2)),
        "the merged period counts both of that service's requests, got {merged:?}"
    );
}

#[tokio::test]
async fn a_selector_this_language_cannot_read_is_refused_rather_than_ignored() {
    let xray = Xray::new();
    seed_entities(&xray).await;

    // The selector language is X-Ray's own, so its reader is this crate's, and
    // what it does with input it cannot read is part of the rule: a selector is
    // refused by NAME rather than ignored, and the bounds on how much parsing
    // one request may drive are the model's `@length` 1..500 on the member and
    // the parser's own. Rows: prose, a function that selects no entity, a
    // member no entity has, a value that is not a string, a truncated
    // expression, one truncated mid-string, unbalanced punctuation, trailing
    // input after a complete selector, and one longer than the member allows.
    let rows: Vec<(&str, String, &str)> = vec![
        ("prose", "select everything".to_string(), "InvalidRequestException"),
        (
            "a function that selects no entity",
            "group(\"Default\")".to_string(),
            "InvalidRequestException",
        ),
        (
            "a member no entity has",
            "id(colour: \"red\")".to_string(),
            "InvalidRequestException",
        ),
        (
            "a member given no value",
            "id(name)".to_string(),
            "InvalidRequestException",
        ),
        (
            "a name that is not a string",
            "id(name: 7)".to_string(),
            "InvalidRequestException",
        ),
        (
            "an expression truncated after the function",
            "service(".to_string(),
            "InvalidRequestException",
        ),
        (
            "an expression truncated inside a string",
            "service(id(name: \"front".to_string(),
            "InvalidRequestException",
        ),
        (
            "unbalanced punctuation",
            "service(id(name: \"frontend\")".to_string(),
            "InvalidRequestException",
        ),
        (
            "a second selector after a complete one",
            "id(name: \"frontend\") id(name: \"other\")".to_string(),
            "InvalidRequestException",
        ),
        (
            "an edge missing its destination",
            "edge(id(name: \"frontend\"))".to_string(),
            "InvalidRequestException",
        ),
        (
            // `EntitySelectorExpression`'s modelled `@length` is 1..500
            // (`smithy_query(query="operation",operation="GetTimeSeriesServiceStatistics")`),
            // so an oversized selector is refused by the request layer before
            // the handler reads a character of it.
            "a selector longer than the member allows",
            format!("id(name: \"{}\")", "a".repeat(500)),
            "ValidationException",
        ),
    ];

    for (case, selector, code) in rows {
        let refused = xray
            .call(
                "/TimeSeriesServiceStatistics",
                &json!({
                    "StartTime": ALIGNED,
                    "EndTime": ALIGNED + 120.0,
                    "EntitySelectorExpression": selector,
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some(code)),
            "case {case:?}: a selector that cannot be read must be refused, got {refused:?}"
        );
        assert!(
            refused
                .message()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal must say what was wrong, got {refused:?}"
        );
    }

    // And a refusal changes nothing: the aggregation a readable selector asks
    // for is still answered afterwards.
    let answered = statistics(&xray, Some("id(name: \"frontend\")"), 60).await;
    assert_eq!(
        answered.status, 200,
        "a readable selector must still be served after the refusals, got {answered:?}"
    );
}

// =============================================================================
// 3. TimeRangeType selects which instant the window is applied to
// =============================================================================

#[tokio::test]
async fn the_time_range_type_chooses_the_instant_the_window_is_applied_to() {
    let xray = Xray::new();
    // One trace that STARTED well before the window below and ENDED inside it,
    // and was ingested now — so the three instants a `TimeRangeType` names sit
    // in three different places relative to each window.
    let trace = trace_id(710);
    let started = ALIGNED - 600.0;
    let ended = ALIGNED - 60.0;
    let put = xray
        .put_segments(&[
            root_document("7100000000000001", &trace, "svc", started, Some(ended)).to_string(),
        ])
        .await;
    assert_eq!(put.status, 200, "seeding must succeed, got {put:?}");
    let ingested = service_com_amazonaws_xray_awsxray::state::now_epoch_seconds();

    // Rows: each `TimeRangeType` member against a window around each of the
    // three instants, and the default (no member at all), which is the trace's
    // start time. One row is the case, the `TimeRangeType` it names (`None` for
    // the default), the `[StartTime, EndTime)` window it asks over, and whether
    // the seeded trace is selected.
    type Row = (&'static str, Option<&'static str>, (f64, f64), bool);

    let rows: Vec<Row> = vec![
        (
            "TraceId over the trace's start time",
            Some("TraceId"),
            (started - 1.0, started + 1.0),
            true,
        ),
        (
            "TraceId over the trace's end time",
            Some("TraceId"),
            (ended - 1.0, ended + 1.0),
            false,
        ),
        (
            "no member at all is the trace's start time",
            None,
            (started - 1.0, started + 1.0),
            true,
        ),
        (
            "Service over the trace's end time",
            Some("Service"),
            (ended - 1.0, ended + 1.0),
            true,
        ),
        (
            "Service over the trace's start time",
            Some("Service"),
            (started - 1.0, started + 1.0),
            false,
        ),
        (
            "Event over the instant the trace was last updated",
            Some("Event"),
            (ingested - 60.0, ingested + 60.0),
            true,
        ),
        (
            "Event over the trace's start time",
            Some("Event"),
            (started - 1.0, started + 1.0),
            false,
        ),
        (
            "a window around no instant of the trace selects it under none",
            Some("Service"),
            (started + 10.0, started + 20.0),
            false,
        ),
    ];

    for (case, range, (start, end), selected) in rows {
        let mut body = json!({"StartTime": start, "EndTime": end});
        if let Some(range) = range {
            body["TimeRangeType"] = json!(range);
        }
        let answered = xray.call("/TraceSummaries", &body).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: every modelled TimeRangeType is served, got {answered:?}"
        );
        let ids: Vec<&str> = array(&answered.body, "TraceSummaries")
            .iter()
            .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
            .collect();
        assert_eq!(
            ids == vec![trace.as_str()],
            selected,
            "case {case:?}: expected the trace to be selected: {selected}, got {ids:?}"
        );
    }
}

// =============================================================================
// 4. A root-cause block, and the response-time root cause it reads
// =============================================================================

/// The two traces the root-cause rows are evaluated over: one whose response
/// time went to a DEPENDENCY it called (a `remote` subsegment), one whose went
/// to its own PIPELINE (a subsegment that never left the process), and one whose
/// time went to nothing in particular.
const DEPENDENCY_BOUND: u32 = 720;
const PIPELINE_BOUND: u32 = 721;
const UNATTRIBUTED: u32 = 722;

/// A root segment of `service` holding one subsegment, which is a call out of
/// the process when `namespace` says so.
fn with_subsegment(
    n: u32,
    service: &str,
    frame: &str,
    namespace: Option<&str>,
    span: f64,
) -> (String, String) {
    let trace = trace_id(n);
    let start = ALIGNED + f64::from(n % 10);
    let mut document = root_document(
        &format!("{:016x}", u64::from(n) << 32),
        &trace,
        service,
        start,
        Some(start + 1.0),
    );
    let mut subsegment = json!({
        "id": format!("{:016x}", u64::from(n) << 16),
        "name": frame,
        "start_time": start + 0.05,
        "end_time": start + 0.05 + span,
    });
    if let Some(namespace) = namespace {
        subsegment["namespace"] = json!(namespace);
    }
    document["subsegments"] = json!([subsegment]);
    (trace, document.to_string())
}

/// Ingest the three traces, and answer each one's id.
async fn seed_root_causes(xray: &Xray) -> (String, String, String) {
    let (dependency, dependency_document) =
        with_subsegment(DEPENDENCY_BOUND, "quartz-api", "quartz-db", Some("remote"), 0.8);
    let (pipeline, pipeline_document) =
        with_subsegment(PIPELINE_BOUND, "quartz-api", "render", None, 0.8);
    // A frame that ran for a tenth of the response time explains none of it, so
    // this trace has no response-time root cause at all.
    let (spread, spread_document) =
        with_subsegment(UNATTRIBUTED, "quartz-api", "glance", None, 0.1);
    let put = xray
        .put_segments(&[dependency_document, pipeline_document, spread_document])
        .await;
    assert_eq!(
        (put.status, array(&put.body, "UnprocessedTraceSegments").len()),
        (200, 0),
        "seeding the root-cause fixture must store every document, got {put:?}"
    );
    (dependency, pipeline, spread)
}

#[tokio::test]
async fn a_root_cause_block_selects_what_its_written_out_path_selects() {
    let xray = Xray::new();
    let (dependency, pipeline, spread) = seed_root_causes(&xray).await;
    let window = (ALIGNED - 10.0, ALIGNED + 60.0);

    // Rows differ along the part the block names, the member it compares, the
    // type of that member (a string name, a numeric coverage), and the form the
    // keyword is written in — a `{ … }` block or the whole path written out,
    // which have to select the same traces. (`remote` and `inferred` are
    // members a root cause reports but the language's keyword catalog does not
    // admit inside a block, so those rows are written out.)
    let rows: Vec<(&str, Vec<&String>)> = vec![
        (
            "rootcause.responsetime.entity { name = \"quartz-db\" }",
            vec![&dependency],
        ),
        (
            "rootcause.responsetime.entity.name = \"quartz-db\"",
            vec![&dependency],
        ),
        (
            "rootcause.responsetime.entity { name = \"render\" }",
            vec![&pipeline],
        ),
        (
            "rootcause.responsetime.entity.remote = true",
            vec![&dependency],
        ),
        (
            "rootcause.responsetime.entity.remote = false",
            vec![&dependency, &pipeline],
        ),
        (
            "rootcause.responsetime.entity { name = \"glance\" }",
            vec![],
        ),
        (
            "rootcause.responsetime.service { name = \"quartz-api\" }",
            vec![&dependency, &pipeline],
        ),
        (
            "rootcause.responsetime.service { name = \"quartz-db\" }",
            vec![&dependency],
        ),
        (
            "rootcause.responsetime.service.inferred = true",
            vec![&dependency],
        ),
        (
            "rootcause.responsetime.entity { coverage > 0.5 }",
            vec![&dependency, &pipeline],
        ),
        (
            "rootcause.responsetime.entity { coverage > 0.95 }",
            vec![&dependency, &pipeline],
        ),
        (
            "rootcause.responsetime.entity { coverage < 0.1 }",
            vec![],
        ),
        (
            "rootcause.responsetime.entity { name = \"quartz-db\" } AND fault",
            vec![],
        ),
        (
            "rootcause.responsetime.entity { name = \"quartz-db\" } OR rootcause.responsetime.entity { name = \"render\" }",
            vec![&dependency, &pipeline],
        ),
        (
            // The trace with no response-time root cause at all satisfies no
            // comparison on one, so negating the comparison selects it too.
            "NOT rootcause.responsetime.entity.remote = true",
            vec![&pipeline, &spread],
        ),
    ];

    for (expression, expected) in rows {
        let answered = xray.filtered(window.0, window.1, expression).await;
        assert_eq!(
            answered.status, 200,
            "case {expression:?}: a well-formed expression is evaluated, not refused, got {answered:?}"
        );
        let selected: BTreeSet<&str> = array(&answered.body, "TraceSummaries")
            .iter()
            .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
            .collect();
        let wanted: BTreeSet<&str> = expected.iter().map(|id| id.as_str()).collect();
        assert_eq!(
            selected, wanted,
            "case {expression:?}: expected {wanted:?}, got {selected:?}"
        );
    }

    // A comparison inside a block whose operands cannot be compared can never
    // hold for any trace, so it is a mistake in the request: refused, and not
    // answered with an empty selection.
    let refusals = [
        (
            "an entity name compared to a number",
            "rootcause.responsetime.entity { name = 7 }",
        ),
        (
            "a remote flag compared to a string",
            "rootcause.responsetime.entity.remote = \"yes\"",
        ),
        (
            "a coverage compared to a string",
            "rootcause.responsetime.entity { coverage = \"most\" }",
        ),
        (
            "an entity name ordered against a number",
            "rootcause.fault.entity { name > 7 }",
        ),
    ];
    for (case, expression) in refusals {
        let refused = xray.filtered(window.0, window.1, expression).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: a comparison that can never hold must be refused, got {refused:?}"
        );
    }
}

#[tokio::test]
async fn the_response_time_root_cause_is_the_chain_the_time_was_spent_in() {
    let xray = Xray::new();
    let (dependency, pipeline, spread) = seed_root_causes(&xray).await;
    let answered = xray.summaries(ALIGNED - 10.0, ALIGNED + 60.0, None).await;
    assert_eq!(
        answered.status, 200,
        "the summaries must be answered, got {answered:?}"
    );

    /// The `(Name, Inferred, [(entity name, remote, coverage)])` of each service
    /// of one summary's response-time root cause.
    type Chain = Vec<(String, bool, Vec<(String, bool, f64)>)>;

    let chain_of = |trace: &str| -> Chain {
        let summary = array(&answered.body, "TraceSummaries")
            .iter()
            .find(|summary| summary.get("Id").and_then(Value::as_str) == Some(trace))
            .unwrap_or_else(|| panic!("the trace {trace} must be summarised: {answered:?}"))
            .clone();
        let causes = array(&summary, "ResponseTimeRootCauses");
        assert!(
            causes.len() <= 1,
            "a trace's response time has one chain or none, got {causes:?}"
        );
        causes
            .first()
            .map(|cause| {
                array(cause, "Services")
                    .iter()
                    .map(|service| {
                        (
                            service
                                .get("Name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            service
                                .get("Inferred")
                                .and_then(Value::as_bool)
                                .unwrap_or_default(),
                            array(service, "EntityPath")
                                .iter()
                                .map(|entity| {
                                    (
                                        entity
                                            .get("Name")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_string(),
                                        entity
                                            .get("Remote")
                                            .and_then(Value::as_bool)
                                            .unwrap_or_default(),
                                        entity
                                            .get("Coverage")
                                            .and_then(Value::as_f64)
                                            .unwrap_or(-1.0),
                                    )
                                })
                                .collect(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    // The dependency-bound trace: the chain leaves the service it entered, so
    // the downstream service is a second, INFERRED service of the chain and the
    // frame that called out is remote.
    let dependency_chain = chain_of(&dependency);
    let dependency_names: Vec<(&str, bool)> = dependency_chain
        .iter()
        .map(|(name, inferred, _)| (name.as_str(), *inferred))
        .collect();
    assert_eq!(
        dependency_names,
        vec![("quartz-api", false), ("quartz-db", true)],
        "case \"dependency-bound\": expected the chain to reach the downstream service, got {dependency_chain:?}"
    );
    let dependency_frames: Vec<(&str, bool)> = dependency_chain
        .iter()
        .flat_map(|(_, _, frames)| frames.iter().map(|(name, remote, _)| (name.as_str(), *remote)))
        .collect();
    assert_eq!(
        dependency_frames,
        vec![
            ("quartz-api", false),
            ("quartz-db", true),
            ("quartz-db", true)
        ],
        "case \"dependency-bound\": expected the frames from the entry point to the call out, got {dependency_chain:?}"
    );
    let covered = dependency_chain
        .iter()
        .flat_map(|(_, _, frames)| frames.iter().map(|(_, _, coverage)| *coverage))
        .collect::<Vec<f64>>();
    assert!(
        covered.iter().all(|coverage| *coverage > 0.0 && *coverage <= 1.0),
        "case \"dependency-bound\": every frame covers a part of the response time, got {covered:?}"
    );
    assert!(
        covered
            .iter()
            .skip(1)
            .all(|coverage| close(*coverage, 0.8)),
        "case \"dependency-bound\": the frames below the entry point ran for 0.8 of the 1.0 second response time, got {covered:?}"
    );

    // The pipeline-bound trace: the chain stays inside the one service, and its
    // deepest frame never left the process.
    let pipeline_chain = chain_of(&pipeline);
    let pipeline_names: Vec<(&str, bool)> = pipeline_chain
        .iter()
        .map(|(name, inferred, _)| (name.as_str(), *inferred))
        .collect();
    assert_eq!(
        pipeline_names,
        vec![("quartz-api", false)],
        "case \"pipeline-bound\": the chain must stay in the one service, got {pipeline_chain:?}"
    );
    let pipeline_frames: Vec<(&str, bool)> = pipeline_chain
        .iter()
        .flat_map(|(_, _, frames)| frames.iter().map(|(name, remote, _)| (name.as_str(), *remote)))
        .collect();
    assert_eq!(
        pipeline_frames,
        vec![("quartz-api", false), ("render", false)],
        "case \"pipeline-bound\": expected the pipeline frames, got {pipeline_chain:?}"
    );

    // And a trace whose response time went to nothing in particular has no
    // response-time root cause: the deepest frame ran for less than
    // `RESPONSE_TIME_ROOT_CAUSE_MIN_COVERAGE` of it.
    assert!(
        chain_of(&spread).is_empty(),
        "case \"unattributed\": a frame covering less than {RESPONSE_TIME_ROOT_CAUSE_MIN_COVERAGE} of the response time is no root cause, got {:?}",
        chain_of(&spread)
    );
}

// =============================================================================
// 5. The encryption configuration identifies its key by ARN
// =============================================================================

#[tokio::test]
async fn the_encryption_configuration_reports_the_key_arn_of_the_region_it_was_put_in() {
    // Rows differ along the region the configuration is put in and the form the
    // selector takes: a bare key id and a multi-region key id, which are
    // qualified with the region the request was addressed to and this
    // emulator's account, an alias path, and an ARN of another partition,
    // region and account, which already identifies its key and is reported as
    // submitted.
    let account = emulator_server::emulator_account_id();
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a bare key id in us-west-2",
            "us-west-2",
            "b14ec8ba-5651-42cb-90d5-0a805a6b1eab",
            format!("arn:aws:kms:us-west-2:{account}:key/b14ec8ba-5651-42cb-90d5-0a805a6b1eab"),
        ),
        (
            "the same key id in another region",
            "eu-west-1",
            "b14ec8ba-5651-42cb-90d5-0a805a6b1eab",
            format!("arn:aws:kms:eu-west-1:{account}:key/b14ec8ba-5651-42cb-90d5-0a805a6b1eab"),
        ),
        (
            "a multi-region key id",
            "us-west-2",
            "mrk-1234abcd12ab34cd56ef1234567890ab",
            format!("arn:aws:kms:us-west-2:{account}:key/mrk-1234abcd12ab34cd56ef1234567890ab"),
        ),
        (
            "an alias path",
            "us-west-2",
            "alias/MyKey",
            format!("arn:aws:kms:us-west-2:{account}:alias/MyKey"),
        ),
        (
            "an ARN of another partition, region and account",
            "us-west-2",
            "arn:aws-us-gov:kms:us-gov-west-1:210987654321:key/b14ec8ba-5651-42cb-90d5-0a805a6b1eab",
            "arn:aws-us-gov:kms:us-gov-west-1:210987654321:key/b14ec8ba-5651-42cb-90d5-0a805a6b1eab"
                .to_string(),
        ),
    ];

    for (case, region, submitted, expected) in rows {
        let xray = Xray::new();
        let put = xray
            .call_in_region(
                region,
                "/PutEncryptionConfig",
                &json!({"Type": "KMS", "KeyId": submitted}),
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the configuration must be stored, got {put:?}"
        );
        // Both surfaces report the stored configuration, so both report the ARN.
        for (surface, path, body) in [
            ("PutEncryptionConfig", "/PutEncryptionConfig", None),
            ("GetEncryptionConfig", "/EncryptionConfig", Some(json!({}))),
        ] {
            let answered = match body {
                None => put.get("EncryptionConfig").clone(),
                Some(body) => xray
                    .call_in_region(region, path, &body)
                    .await
                    .get("EncryptionConfig")
                    .clone(),
            };
            assert_eq!(
                answered.get("KeyId").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case:?} on {surface}: expected KeyId {expected:?}, got {answered:?}"
            );
            assert_eq!(
                answered.get("Type").and_then(Value::as_str),
                Some("KMS"),
                "case {case:?} on {surface}: expected Type KMS, got {answered:?}"
            );
        }
    }
}
