// SPDX-License-Identifier: CC0-1.0

//! What a trace-query read accepts about the WINDOW it is asked about, the
//! aggregation PERIOD it is asked for, and the GROUP a query scopes itself to.
//!
//! Four rules are asserted here, each over a table that spans what the rule
//! generalises over rather than over one request:
//!
//! * each read that takes a `[StartTime, EndTime]` window publishes the widest
//!   window it serves and refuses a wider one, so the bound is a property of the
//!   READ: the service map serves a narrower window than the trace summaries,
//!   and each refusal names its own bound. The rows differ along the width of
//!   the window (nothing, well inside, exactly the bound, a second past it, far
//!   past it) and along which read is asked, including the width one read serves
//!   and the other does not;
//! * the aggregated statistics read serves the aggregation periods the service
//!   publishes and refuses any other, naming the set that is served. The rows
//!   differ along the period asked for: each served one, an omitted one, periods
//!   either side of each served one, a much wider and a much narrower one, and
//!   periods that are no period at all;
//! * a sampling strategy is served only under the time-range type that admits
//!   it, and one named beside any other range type is refused naming both. The
//!   rows differ along the strategy named and along the range type asked for;
//! * a query scoped to a GROUP is resolved against the stored groups: a group
//!   that exists narrows the query by the expression it carries, and one that
//!   does not is refused with the service's one sentence for an unknown group —
//!   the same sentence every other group-scoped read answers. The rows differ
//!   along the form the reference takes (the keyword alone, each selector, the
//!   call form), along which selector it names the group by (name, ARN), and
//!   along what else the query asks (nothing, a term before the reference, a
//!   term after it).
//!
//! A query that matches nothing is an ACCEPTED query over its window, so the
//! last case asserts the empty page it answers with rather than a refusal.

use serde_json::{json, Value};

use super::xray_wire::{array, close, root_document, trace_id, Answer, Xray};

/// An instant well in the past, so no window a case asks about reaches ahead of
/// the clock-skew allowance the window check applies.
const BASE: f64 = 1_700_000_000.0;

/// Seconds in one hour, the unit the windows below are stated in.
const HOUR: f64 = 60.0 * 60.0;

/// The refusal message of an answer, or the empty string.
fn said(answer: &Answer) -> &str {
    answer.message().unwrap_or_default()
}

/// Now, in the epoch seconds every timestamp on this wire travels as.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds()
}

// =============================================================================
// 1. Each read publishes the widest window it serves
// =============================================================================

/// One read that takes a required window, and the widest one it serves.
struct WindowedRead {
    path: &'static str,
    /// The widest window the read serves, in whole hours — what its refusal
    /// names.
    max_hours: i64,
    /// The request body for a `[start, end]` window.
    body: fn(f64, f64) -> Value,
}

fn windowed_reads() -> Vec<WindowedRead> {
    vec![
        WindowedRead {
            path: "/TraceSummaries",
            max_hours: 24,
            body: |start, end| json!({"StartTime": start, "EndTime": end}),
        },
        WindowedRead {
            path: "/ServiceGraph",
            max_hours: 6,
            body: |start, end| json!({"StartTime": start, "EndTime": end}),
        },
    ]
}

/// The widths each read is asked about, as a multiple of the bound it publishes
/// plus an offset in seconds — so one table spans both reads and the row that
/// is inside one read's bound and past the other's is the same row.
const WIDTHS: [(&str, f64, f64); 6] = [
    ("a window of a single instant", 0.0, 0.0),
    ("a window well inside the bound", 0.25, 0.0),
    ("a window a minute short of the bound", 1.0, -60.0),
    ("a window exactly at the bound", 1.0, 0.0),
    ("a window one second past the bound", 1.0, 1.0),
    ("a window twice the bound", 2.0, 0.0),
];

#[tokio::test]
async fn each_read_serves_the_window_it_publishes_and_refuses_a_wider_one() {
    for read in windowed_reads() {
        let path = read.path;
        #[allow(clippy::cast_precision_loss)]
        let bound = read.max_hours as f64 * HOUR;
        let expected = format!("Time range cannot be longer than {} hours", read.max_hours);
        for (width, factor, offset) in WIDTHS {
            let xray = Xray::new();
            let span = bound * factor + offset;
            let end = now() - 60.0;
            let answered = xray.call(path, &(read.body)(end - span, end)).await;
            let label = format!("{width} on {path}");
            if span <= bound {
                assert_eq!(
                    answered.status, 200,
                    "case {label:?}: a window the read serves must be answered, got {answered:?}"
                );
            } else {
                assert_eq!(
                    (answered.status, answered.error_type.as_deref()),
                    (400, Some("InvalidRequestException")),
                    "case {label:?}: a wider window must be refused as an invalid request, got \
                     {answered:?}"
                );
                assert_eq!(
                    said(&answered),
                    expected,
                    "case {label:?}: expected {expected:?}, got {:?}",
                    said(&answered)
                );
            }
        }
    }
}

#[tokio::test]
async fn the_trace_query_reaches_a_published_allowance_past_the_clock() {
    let xray = Xray::new();

    // A window may end a little past now — a client's clock runs ahead, and a
    // dashboard asks for "up to now" — and the read publishes how far it will
    // go. The rows differ along how far ahead the window ends: inside the
    // allowance, at it, and past it by a minute and by an hour.
    let rows: [(&str, f64, bool); 5] = [
        ("a window ending now", 0.0, true),
        ("a window ending a minute ahead", 60.0, true),
        ("a window ending at the allowance", 15.0 * 60.0, true),
        ("a window ending a minute past the allowance", 16.0 * 60.0, false),
        ("a window ending an hour ahead", HOUR, false),
    ];

    for (label, ahead, served) in rows {
        let body = json!({"StartTime": now() - 60.0, "EndTime": now() + ahead});
        let answered = xray.call("/TraceSummaries", &body).await;
        if served {
            assert_eq!(
                answered.status, 200,
                "case {label:?}: a window inside the allowance must be answered, got {answered:?}"
            );
        } else {
            assert_eq!(
                (
                    answered.status,
                    answered.error_type.as_deref(),
                    said(&answered)
                ),
                (
                    400,
                    Some("InvalidRequestException"),
                    "End time cannot be later than 15 minutes in the future"
                ),
                "case {label:?}: a window ending past the allowance must be refused naming it, \
                 got {answered:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_bound_belongs_to_the_read_not_to_the_service() {
    // The same twelve-hour window: inside what the trace summaries serve, past
    // what the service map does. One window, two answers, each naming its own
    // read's bound.
    let xray = Xray::new();
    let end = now() - 60.0;
    let window = json!({"StartTime": end - 12.0 * HOUR, "EndTime": end});

    let summaries = xray.call("/TraceSummaries", &window).await;
    assert_eq!(
        summaries.status, 200,
        "a twelve-hour window is inside the window the trace summaries serve, got {summaries:?}"
    );

    let graph = xray.call("/ServiceGraph", &window).await;
    assert_eq!(
        (graph.status, said(&graph)),
        (400, "Time range cannot be longer than 6 hours"),
        "a twelve-hour window is past the window the service map serves, got {graph:?}"
    );
}

// =============================================================================
// 2. The statistics read serves the aggregation periods the service publishes
// =============================================================================

/// The one refusal for a period the read does not serve: the set it serves,
/// and nothing from the request.
const UNSERVED_PERIOD: &str = "Time period must be one of [60, 300]";

/// The aggregation periods the read serves, in seconds.
///
/// `Period` targets `NullableInteger` and carries no modelled range
/// (`smithy_query(query="operation",operation="GetTimeSeriesServiceStatistics")`),
/// so the set is the service's own: the one-minute and the five-minute
/// aggregation, which is what its refusal lists.
const SERVED_PERIODS: [i64; 2] = [60, 300];

#[tokio::test]
async fn the_statistics_read_serves_two_aggregation_periods_and_refuses_the_rest() {
    let xray = Xray::new();
    let end = now() - 60.0;
    // The rows span the period asked for: each served one, an omitted one, and
    // periods either side of each served one, far outside them, and periods that
    // describe no period at all.
    let periods: [Option<i64>; 14] = [
        None,
        Some(60),
        Some(300),
        Some(1),
        Some(30),
        Some(59),
        Some(61),
        Some(120),
        Some(299),
        Some(301),
        Some(900),
        Some(3_600),
        Some(0),
        Some(-60),
    ];
    for period in periods {
        let mut body = json!({"StartTime": end - HOUR, "EndTime": end});
        if let Some(period) = period {
            body["Period"] = json!(period);
        }
        let answered = xray.call("/TimeSeriesServiceStatistics", &body).await;
        let label = format!("Period {period:?}");
        if period.is_none_or(|period| SERVED_PERIODS.contains(&period)) {
            assert_eq!(
                answered.status, 200,
                "case {label:?}: a period the read serves must be answered, got {answered:?}"
            );
        } else {
            assert_eq!(
                (
                    answered.status,
                    answered.error_type.as_deref(),
                    said(&answered)
                ),
                (400, Some("InvalidRequestException"), UNSERVED_PERIOD),
                "case {label:?}: a period the read does not serve must be refused naming the set \
                 it serves, got {answered:?}"
            );
        }
    }
}

// =============================================================================
// 3. A sampling strategy belongs to one time-range type
// =============================================================================

#[tokio::test]
async fn a_sampling_strategy_names_the_time_range_type_that_admits_it() {
    let xray = Xray::new();
    let end = now() - 60.0;

    // A strategy picks its subset as traces are UPDATED, so the read admits one
    // only where the window is applied to each trace's update time. The rows
    // differ along the strategy named and along the range type asked for,
    // including the one the request leaves to the default.
    let rows: Vec<(&str, &str, Option<&str>, &str)> = vec![
        (
            "a fixed rate under the default range type",
            "FixedRate",
            None,
            "TraceId",
        ),
        (
            "a fixed rate under the trace-id range type",
            "FixedRate",
            Some("TraceId"),
            "TraceId",
        ),
        (
            "a fixed rate under the service range type",
            "FixedRate",
            Some("Service"),
            "Service",
        ),
        (
            "a partial scan under the default range type",
            "PartialScan",
            None,
            "TraceId",
        ),
        (
            "a partial scan under the service range type",
            "PartialScan",
            Some("Service"),
            "Service",
        ),
    ];

    for (label, strategy, range_type, named_range) in rows {
        let mut body = json!({
            "StartTime": end - 60.0,
            "EndTime": end,
            "SamplingStrategy": {"Name": strategy, "Value": 0.5},
        });
        if let Some(range_type) = range_type {
            body["TimeRangeType"] = json!(range_type);
        }
        let answered = xray.call("/TraceSummaries", &body).await;
        let expected = format!(
            "Cannot use {strategy} with input time range type: {named_range}. Please try using \
             time range type: Event"
        );
        assert_eq!(
            (
                answered.status,
                answered.error_type.as_deref(),
                said(&answered)
            ),
            (400, Some("InvalidRequestException"), expected.as_str()),
            "case {label:?}: expected {expected:?}, got {answered:?}"
        );
    }
}


#[tokio::test]
async fn a_sampled_query_states_a_rate_and_selects_a_subset_at_it() {
    let xray = Xray::new();

    // A sampled query names the subset it wants by a RATE. The rows differ along
    // what the request states it as: each end of the interval, a rate inside it,
    // a rate outside it on each side, a strategy carrying none, and the boolean
    // asking for a subset with no rate at all.
    let end = now() - 60.0;
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a rate inside the interval",
            json!({"SamplingStrategy": {"Name": "FixedRate", "Value": 0.5}}),
            true,
        ),
        (
            "a rate of none",
            json!({"SamplingStrategy": {"Name": "FixedRate", "Value": 0.0}}),
            true,
        ),
        (
            "a rate of one",
            json!({"SamplingStrategy": {"Name": "FixedRate", "Value": 1.0}}),
            true,
        ),
        (
            "a partial scan at a rate inside the interval",
            json!({"SamplingStrategy": {"Name": "PartialScan", "Value": 0.25}}),
            true,
        ),
        (
            "a strategy carrying no rate",
            json!({"SamplingStrategy": {"Name": "FixedRate"}}),
            false,
        ),
        (
            "a rate above the interval",
            json!({"SamplingStrategy": {"Name": "FixedRate", "Value": 1.5}}),
            false,
        ),
        (
            "a rate below the interval",
            json!({"SamplingStrategy": {"Name": "FixedRate", "Value": -0.1}}),
            false,
        ),
        (
            // `Sampling` on its own asks for a subset and names no strategy to
            // draw it by, so the read draws one at its own rate rather than
            // refusing a request that asked for nothing contradictory.
            "sampling asked for with no strategy",
            json!({"Sampling": true}),
            true,
        ),
        (
            "sampling asked for beside a strategy stating no rate",
            json!({"Sampling": true, "SamplingStrategy": {"Name": "FixedRate"}}),
            false,
        ),
    ];

    for (label, extra, served) in rows {
        let mut body = json!({
            "StartTime": end - 60.0,
            "EndTime": end,
            // The range type a strategy is admitted under, so each row is about
            // the RATE and not about the range type.
            "TimeRangeType": "Event",
        });
        for (member, value) in extra.as_object().expect("the row is an object") {
            body[member] = value.clone();
        }
        let answered = xray.call("/TraceSummaries", &body).await;
        if served {
            assert_eq!(
                answered.status, 200,
                "case {label:?}: a sampled query stating a usable rate must be answered, got \
                 {answered:?}"
            );
        } else {
            assert_eq!(
                (
                    answered.status,
                    answered.error_type.as_deref(),
                    said(&answered)
                ),
                (
                    400,
                    Some("InvalidRequestException"),
                    "sampling rate must be specified and its value must be between [0, 1]"
                ),
                "case {label:?}: a sampled query stating no usable rate must be refused naming \
                 what a rate has to be, got {answered:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_sample_is_a_stable_subset_of_what_the_query_selected() {
    let xray = Xray::new();

    // As many traces in one window as a page carries, so a sample of them can
    // be told from all of them and from none without a second page entering it.
    let base = BASE + 1_000.0;
    const STORED: u32 = 5;
    for n in 0..STORED {
        let document = root_document(
            &format!("c0000000000000{n:02x}"),
            &trace_id(100 + n),
            "sampled",
            base + f64::from(n),
            Some(base + f64::from(n) + 0.25),
        )
        .to_string();
        let put = xray.put_segments(&[document]).await;
        assert_eq!(
            put.status, 200,
            "the traces the sample is drawn from must be stored, got {put:?}"
        );
    }

    let whole = sampled_ids(&xray, 1.0).await;
    assert_eq!(
        whole.len(),
        STORED as usize,
        "a rate of one selects every trace the window holds, got {whole:?}"
    );
    let none = sampled_ids(&xray, 0.0).await;
    assert!(
        none.is_empty(),
        "a rate of none selects no trace, got {none:?}"
    );
    let half = sampled_ids(&xray, 0.5).await;
    assert!(
        half.len() <= whole.len() && half.iter().all(|id| whole.contains(id)),
        "a rate inside the interval selects a SUBSET of what the window holds, got {half:?}"
    );
    // The subset is a function of the traces and the rate, so asking twice
    // answers the same traces: no clock and no random source enters it.
    let again = sampled_ids(&xray, 0.5).await;
    assert_eq!(
        half, again,
        "the same query at the same rate must answer the same subset, got {half:?} then {again:?}"
    );
}

/// The trace ids a query sampled at `rate` reports, over the window holding
/// every trace this suite's sampling case stored.
async fn sampled_ids(xray: &Xray, rate: f64) -> Vec<String> {
    let answered = xray
        .call(
            "/TraceSummaries",
            &json!({
                // The window the sample is drawn over, applied to each trace's
                // UPDATE time — the range type a strategy is admitted under —
                // which for a just-ingested trace is when this service accepted
                // it.
                "StartTime": now() - 300.0,
                "EndTime": now() + 60.0,
                "TimeRangeType": "Event",
                "SamplingStrategy": {"Name": "FixedRate", "Value": rate},
            }),
        )
        .await;
    assert_eq!(
        answered.status, 200,
        "a sampled query at {rate} must be answered, got {answered:?}"
    );
    // The count is of the traces the WINDOW holds, which sampling does not
    // narrow: the sample is of what is reported, not of what was processed.
    assert_eq!(
        answered.get("TracesProcessedCount").as_i64(),
        Some(5),
        "a sampled query still processed every trace in the window, got {answered:?}"
    );
    reported(&answered)
}

#[tokio::test]
async fn terms_joined_by_the_doubled_symbols_are_read_like_the_worded_operators() {
    let xray = Xray::new();
    seed(&xray).await;

    // The boolean operators are written either as words or as the doubled
    // symbols, so an expression using them is READ — and selects what the same
    // expression written with the words selects — rather than refused as input
    // the language does not admit. The rows differ along which operator is
    // doubled, how many terms it joins, and whether the two forms are mixed.
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "a conjunction written as the doubled symbol",
            "ok && service(\"checkout\")",
            vec![trace_id(1)],
        ),
        (
            "a disjunction written as the doubled symbol",
            "service(\"checkout\") || service(\"payments\")",
            vec![trace_id(1), trace_id(2)],
        ),
        (
            "both forms in one expression",
            "ok and service(\"checkout\") || service(\"payments\") && ok",
            vec![trace_id(1), trace_id(2)],
        ),
        (
            "the doubled symbol beside a group scope",
            "group.name = \"checkout-errors\" && ok",
            vec![trace_id(1)],
        ),
    ];

    for (label, expression, expected) in rows {
        let answered = xray.filtered(BASE, BASE + 60.0, expression).await;
        assert_eq!(
            answered.status, 200,
            "case {label:?}: an expression joined by the doubled symbols must be read, got \
             {answered:?}"
        );
        assert_eq!(
            reported(&answered),
            expected,
            "case {label:?}: expected the traces {expected:?}, got {answered:?}"
        );
    }

    // A single `&` joins nothing, so it stays input the language does not
    // admit — reported where it stands.
    let refused = xray.filtered(BASE, BASE + 60.0, "ok & ok").await;
    assert_eq!(
        (refused.status, said(&refused)),
        (
            400,
            "Error Location: line 1, position 4 : Invalid input symbol: '&'"
        ),
        "a single ampersand is not of the language, got {refused:?}"
    );
}

// =============================================================================
// 4. A group-scoped query resolves the group it names
// =============================================================================

/// The one sentence a read that SCOPES a derivation to a group answers for a
/// group that does not exist — fixed text, the closing full stop included,
/// naming neither selector.
const GROUP_NOT_FOUND: &str = "Group not found.";

/// The group the cases scope a query to, and the service its expression selects.
const SCOPED_GROUP: &str = "checkout-errors";

/// The service whose traces the group's expression selects.
const SELECTED_SERVICE: &str = "checkout";

/// A service the group's expression does not select.
const OTHER_SERVICE: &str = "payments";

/// Create the group and store one trace per service, answering the group's ARN.
///
/// The two traces differ in the service that reported them and in when they
/// started, so a scoped query's own page can be told from the window's contents.
async fn seed(xray: &Xray) -> String {
    let created = xray
        .call(
            "/CreateGroup",
            &json!({
                "GroupName": SCOPED_GROUP,
                "FilterExpression": format!("service(\"{SELECTED_SERVICE}\")"),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the group the cases scope to must be created, got {created:?}"
    );
    let arn = created
        .get("Group")
        .get("GroupARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        arn.contains(SCOPED_GROUP),
        "CreateGroup must answer the group's ARN, got {created:?}"
    );

    let documents = [
        root_document(
            "aaaaaaaaaaaaaaaa",
            &trace_id(1),
            SELECTED_SERVICE,
            BASE + 10.0,
            Some(BASE + 10.5),
        )
        .to_string(),
        root_document(
            "bbbbbbbbbbbbbbbb",
            &trace_id(2),
            OTHER_SERVICE,
            BASE + 20.0,
            Some(BASE + 20.5),
        )
        .to_string(),
    ];
    let put = xray.put_segments(&documents).await;
    assert_eq!(
        (put.status, array(&put.body, "UnprocessedTraceSegments").len()),
        (200, 0),
        "both traces must be stored whole, got {put:?}"
    );
    arn
}

/// The trace ids a `GetTraceSummaries` answer reports, in the order it reports
/// them.
fn reported(answer: &Answer) -> Vec<String> {
    array(&answer.body, "TraceSummaries")
        .iter()
        .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn a_query_scoped_to_a_stored_group_is_narrowed_by_that_group() {
    let xray = Xray::new();
    let arn = seed(&xray).await;

    // Every form a reference takes, by each selector, alone and beside another
    // term on either side of it. Each one scopes the query to the group, so each
    // reports exactly the trace the group's own expression selects — never the
    // other trace in the same window, and never a refusal.
    let forms: Vec<(String, String)> = vec![
        ("the keyword alone".to_string(), format!("group = \"{SCOPED_GROUP}\"")),
        (
            "the name selector".to_string(),
            format!("group.name = \"{SCOPED_GROUP}\""),
        ),
        (
            "the modelled member spelling".to_string(),
            format!("group.groupName = \"{SCOPED_GROUP}\""),
        ),
        (
            "the call form".to_string(),
            format!("group(\"{SCOPED_GROUP}\")"),
        ),
        ("the arn selector".to_string(), format!("group.arn = \"{arn}\"")),
        (
            "the keyword alone carrying an ARN".to_string(),
            format!("group = \"{arn}\""),
        ),
        (
            "a term after the reference".to_string(),
            format!("group.name = \"{SCOPED_GROUP}\" AND ok"),
        ),
        (
            "a term before the reference".to_string(),
            format!("ok and group.name = \"{SCOPED_GROUP}\""),
        ),
        (
            "a term on each side of the reference".to_string(),
            format!("ok and group.name = \"{SCOPED_GROUP}\" and duration < 10"),
        ),
    ];

    for (label, expression) in forms {
        let answered = xray
            .filtered(BASE, BASE + 60.0, &expression)
            .await;
        assert_eq!(
            answered.status, 200,
            "case {label:?}: a query scoped to a stored group must be answered, got {answered:?}"
        );
        assert_eq!(
            reported(&answered),
            vec![trace_id(1)],
            "case {label:?}: the group's own expression selects only the {SELECTED_SERVICE} \
             trace, got {answered:?}"
        );
        // The count is of the traces the WINDOW holds, which the group's
        // expression does not narrow: both traces were taken through it.
        assert_eq!(
            answered.get("TracesProcessedCount").as_i64(),
            Some(2),
            "case {label:?}: the window holds both traces however the query is scoped, got \
             {answered:?}"
        );
        // "The start time of this page of results": the earliest instant this
        // page's own summaries were windowed on, which is the selected trace's
        // start and not the other trace's, nor the clock.
        assert!(
            answered
                .get("ApproximateTime")
                .as_f64()
                .is_some_and(|at| close(at, BASE + 10.0)),
            "case {label:?}: expected ApproximateTime {} (the page's own start), got {answered:?}",
            BASE + 10.0
        );
    }
}

#[tokio::test]
async fn a_group_that_does_not_exist_is_refused_as_an_unknown_group_by_every_read() {
    let xray = Xray::new();
    seed(&xray).await;

    // One sentence for one mistake, across every read that SCOPES a derivation
    // to a group and every form the reference takes: the group-scoped graph and
    // statistics reads, which name it by a modelled selector, and the trace
    // query, which carries it beside its filter expression.
    let absent_arn = "arn:aws:xray:us-east-1:123456789012:group/absent/0123456789ab";
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "GetServiceGraph by a name that names no group",
            "/ServiceGraph",
            json!({"StartTime": BASE, "EndTime": BASE + 60.0, "GroupName": "absent"}),
        ),
        (
            "GetServiceGraph by an ARN that names no group",
            "/ServiceGraph",
            json!({"StartTime": BASE, "EndTime": BASE + 60.0, "GroupARN": absent_arn}),
        ),
        (
            "GetTimeSeriesServiceStatistics by a name that names no group",
            "/TimeSeriesServiceStatistics",
            json!({"StartTime": BASE, "EndTime": BASE + 60.0, "GroupName": "absent"}),
        ),
        (
            "a trace query scoped to a name that names no group",
            "/TraceSummaries",
            json!({
                "StartTime": BASE,
                "EndTime": BASE + 60.0,
                "FilterExpression": "group.name = \"absent\"",
            }),
        ),
        (
            "a trace query scoped by the keyword alone",
            "/TraceSummaries",
            json!({
                "StartTime": BASE,
                "EndTime": BASE + 60.0,
                "FilterExpression": "group = \"absent\"",
            }),
        ),
        (
            "a trace query scoped by the call form",
            "/TraceSummaries",
            json!({
                "StartTime": BASE,
                "EndTime": BASE + 60.0,
                "FilterExpression": "group(\"absent\") and fault",
            }),
        ),
        (
            "a trace query scoped to an ARN that names no group",
            "/TraceSummaries",
            json!({
                "StartTime": BASE,
                "EndTime": BASE + 60.0,
                "FilterExpression": format!("group.arn = \"{absent_arn}\""),
            }),
        ),
    ];

    for (label, path, body) in rows {
        let answered = xray.call(path, &body).await;
        assert_eq!(
            (
                answered.status,
                answered.error_type.as_deref(),
                said(&answered)
            ),
            (400, Some("InvalidRequestException"), GROUP_NOT_FOUND),
            "case {label:?}: an unknown group must be refused with the one sentence for it, got \
             {answered:?}"
        );
    }
}

#[tokio::test]
async fn a_group_arn_that_is_not_one_is_a_malformed_parameter() {
    let xray = Xray::new();
    seed(&xray).await;

    // A value that is not of the form a group's ARN has could never have named a
    // group, so it is refused for the MEMBER that carried it rather than
    // reported as a group that does not exist. The rows differ along how the
    // value misses that form — no ARN at all, another service's ARN, another
    // resource of this service, a truncated ARN — and along which read was
    // asked.
    let malformed = [
        "not-an-arn",
        "arn:aws:s3:::my-bucket",
        "arn:aws:xray:us-east-1:123456789012:sampling-rule/Default",
        "arn:aws:xray:us-east-1:123456789012",
        "group/checkout-errors/0123456789ab",
    ];
    /// How one read asks about a `GroupARN`: its path, and the body it carries
    /// that ARN in.
    type GroupArnRead = (&'static str, fn(&str) -> Value);
    let reads: [GroupArnRead; 3] = [
        (
            "/ServiceGraph",
            |arn| json!({"StartTime": BASE, "EndTime": BASE + 60.0, "GroupARN": arn}),
        ),
        (
            "/TimeSeriesServiceStatistics",
            |arn| json!({"StartTime": BASE, "EndTime": BASE + 60.0, "GroupARN": arn}),
        ),
        ("/GetGroup", |arn| json!({"GroupARN": arn})),
    ];

    for (path, body_of) in reads {
        for arn in malformed {
            let answered = xray.call(path, &body_of(arn)).await;
            let label = format!("{arn:?} on {path}");
            assert_eq!(
                (
                    answered.status,
                    answered.error_type.as_deref(),
                    said(&answered)
                ),
                (400, Some("InvalidRequestException"), "Invalid GroupARN"),
                "case {label:?}: a value that is not a group's ARN must be refused for the \
                 member, got {answered:?}"
            );
        }
    }

    // And a value that IS of that form, naming no group, is the group that does
    // not exist: the two refusals are not interchangeable.
    let absent = xray
        .call(
            "/ServiceGraph",
            &json!({
                "StartTime": BASE,
                "EndTime": BASE + 60.0,
                "GroupARN": "arn:aws:xray:us-east-1:123456789012:group/absent/0123456789ab",
            }),
        )
        .await;
    assert_eq!(
        (absent.status, said(&absent)),
        (400, GROUP_NOT_FOUND),
        "a well-formed ARN naming no group is the unknown group, got {absent:?}"
    );
}

#[tokio::test]
async fn a_scoped_query_that_matches_nothing_is_an_empty_page_and_not_a_refusal() {
    let xray = Xray::new();
    seed(&xray).await;

    // Rows: a window the store holds nothing in, a window holding only the
    // trace the group's expression does NOT select, and a scope-plus-filter that
    // can match no stored trace. Each is an accepted query that simply matched
    // nothing, so each answers an empty page over the window it asked about.
    let rows: Vec<(&str, f64, f64, String)> = vec![
        (
            "a window the store holds nothing in",
            BASE + HOUR,
            BASE + HOUR + 60.0,
            format!("group.name = \"{SCOPED_GROUP}\""),
        ),
        (
            "a window holding only a trace the group does not select",
            BASE + 15.0,
            BASE + 30.0,
            format!("group.name = \"{SCOPED_GROUP}\""),
        ),
        (
            "a term beside the scope that no stored trace satisfies",
            BASE,
            BASE + 60.0,
            format!("group.name = \"{SCOPED_GROUP}\" and fault"),
        ),
    ];

    for (label, start, end, expression) in rows {
        let answered = xray.filtered(start, end, &expression).await;
        assert_eq!(
            answered.status, 200,
            "case {label:?}: a query that matched nothing is accepted, got {answered:?}"
        );
        assert_eq!(
            array(&answered.body, "TraceSummaries").len(),
            0,
            "case {label:?}: the page must carry no summary, got {answered:?}"
        );
        // Every member the read answers is reported, so a client reading an
        // empty page is not left to tell absence from zero.
        assert!(
            answered
                .get("ApproximateTime")
                .as_f64()
                .is_some_and(|at| close(at, start)),
            "case {label:?}: an empty page starts where the requested window does ({start}), got \
             {answered:?}"
        );
        assert!(
            answered.get("NextToken").is_null(),
            "case {label:?}: a page with nothing after it carries no token, got {answered:?}"
        );
    }

    // And the count is of the window, not of the page: the window holding only
    // the unselected trace took exactly that one trace through the query.
    let answered = xray
        .filtered(
            BASE + 15.0,
            BASE + 30.0,
            &format!("group.name = \"{SCOPED_GROUP}\""),
        )
        .await;
    assert_eq!(
        answered.get("TracesProcessedCount").as_i64(),
        Some(1),
        "the window holding one trace reports one processed, got {answered:?}"
    );
}
