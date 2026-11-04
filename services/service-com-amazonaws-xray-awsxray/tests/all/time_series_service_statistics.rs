// SPDX-License-Identifier: CC0-1.0

//! `GetTimeSeriesServiceStatistics` end to end over the wire.
//!
//! Every case drives the same front door the production router does (the shared
//! harness in `trace_ingest_and_groups`), ingests segment documents through
//! `PutTraceSegments`, and then asserts the aggregation is RECOMPUTED from those
//! stored documents. The rules under test are:
//!
//! * every COMPLETED segment whose `start_time` falls in the half-open window
//!   [`StartTime`, `EndTime`) is one request, counted into the `Period`-wide
//!   aggregation period that contains it, and classified by the outcome its
//!   document DECLARED — a throttle as `ErrorStatistics$ThrottleCount`, another
//!   error as `ErrorStatistics$OtherCount`, a fault under `FaultStatistics`, and
//!   every other completed request as `OkCount` — with `TotalCount` the sum,
//!   `TotalResponseTime` the aggregate of the counted requests' spans and
//!   `ResponseTimeHistogram` their (value, prevalence) pairs;
//! * aggregation periods are aligned to multiples of `Period` since the epoch,
//!   so the same request is reported under the same `Timestamp` whatever window
//!   asks about it, and a wider `Period` merges what a narrower one separated;
//! * the operation only REPORTS: two identical calls answer identically, and
//!   nothing a call does changes what a later caller sees;
//! * a `NextToken` is emitted only when the aggregation was truncated, and
//!   resuming from it yields the remaining periods exactly once;
//! * a group selector narrows the aggregation to the traffic that group's own
//!   `FilterExpression` selects \u2014 all of it when the group carries none, none of
//!   it when the expression matches nothing \u2014 and either way the request is
//!   ANSWERED, with `ContainsOldGroupVersions` false;
//! * an `EntitySelectorExpression` narrows the aggregation to the entities it
//!   names — one service's segments under `ServiceSummaryStatistics`, one edge's
//!   calls under `EdgeSummaryStatistics` — and a selector naming nothing the
//!   store recorded is ANSWERED with an empty series;
//! * a selection this slice cannot read (a selector that is not of the
//!   language, a forecast, a group that does not exist), a `Period` that is not
//!   a positive number of seconds and a window the crate's shared check refuses
//!   (inverted, or a bound no recorded traffic could carry) are REFUSED with
//!   `InvalidRequestException`, never answered with an aggregation that ignored
//!   them.

use serde_json::{json, Value};

use crate::trace_ingest_and_groups::{array, close, root_document, trace_id, Xray};

/// The `/TimeSeriesServiceStatistics` URI this operation is bound to.
const PATH: &str = "/TimeSeriesServiceStatistics";

/// A 60-second-aligned epoch instant, so a case's expected `Timestamp` values
/// are easy to read: `1_699_999_980 == 60 * 28_333_333`.
const ALIGNED: f64 = 1_699_999_980.0;

/// The outcome an instrumentation DECLARES on a document beside the status it
/// recorded: a 5xx answer is a fault, a 429 a throttle, any other 4xx an error,
/// and anything else none. The service classifies a request by the flag its
/// document declared and never by the status it recorded, so a row that wants a
/// request counted as a fault, an error or a throttle has to declare it — which
/// is what this does for the status each row names.
fn declared_for(status: Option<i64>) -> Option<&'static str> {
    match status? {
        429 => Some("throttle"),
        status if (400..500).contains(&status) => Some("error"),
        status if status >= 500 => Some("fault"),
        _ => None,
    }
}

/// One submitted segment document: a root segment named `svc`, optionally
/// carrying an `http.response.status` and declaring an outcome beside it, and in
/// progress when it has no end.
fn document(
    n: usize,
    trace: &str,
    start: f64,
    end: Option<f64>,
    status: Option<i64>,
    flag: Option<&str>,
) -> String {
    let mut segment = root_document(&format!("{n:016x}"), trace, "svc", start, end);
    if let Some(status) = status {
        segment["http"] = json!({
            "request": {"method": "GET", "url": "http://localhost/"},
            "response": {"status": status},
        });
    }
    if let Some(flag) = flag {
        segment[flag] = json!(true);
    }
    segment.to_string()
}

/// A request's recorded status and span, as one row of a case's ingest.
#[derive(Debug, Clone, Copy)]
struct Request {
    /// When the request started, in epoch seconds.
    start: f64,
    /// How long it took, or `None` while it is still in progress.
    duration: Option<f64>,
    /// Its `http.response.status`, when it recorded one. The outcome its
    /// document declares beside it is [`declared_for`].
    status: Option<i64>,
}

/// What one aggregation period must report.
#[derive(Debug, Clone)]
struct ExpectedPeriod {
    /// The period's start, in epoch seconds.
    timestamp: f64,
    /// `OkCount`.
    ok: i64,
    /// `ErrorStatistics$ThrottleCount`.
    throttle: i64,
    /// `ErrorStatistics$OtherCount`.
    error_other: i64,
    /// `FaultStatistics$TotalCount`.
    fault: i64,
    /// `TotalResponseTime`.
    total_response_time: f64,
    /// `ResponseTimeHistogram` as (value, prevalence) pairs, ascending.
    histogram: Vec<(f64, i64)>,
}

/// One aggregation case: what was ingested, what was asked, what must come back.
struct Case {
    case: &'static str,
    requests: Vec<Request>,
    window: (f64, f64),
    period: Option<i64>,
    expected: Vec<ExpectedPeriod>,
}

/// The rows differ along what the rule generalises over: the recorded status,
/// the number of requests, whether a request completed, where it sits relative
/// to the window's bounds and to a period boundary, and the width of `Period`.
fn aggregation_cases() -> Vec<Case> {
    vec![
        Case {
            case: "one completed 2xx request is one Ok in the period containing it",
            requests: vec![Request {
                start: ALIGNED + 10.0,
                duration: Some(0.25),
                status: Some(200),
            }],
            window: (ALIGNED, ALIGNED + 60.0),
            period: None,
            expected: vec![ExpectedPeriod {
                timestamp: ALIGNED,
                ok: 1,
                throttle: 0,
                error_other: 0,
                fault: 0,
                total_response_time: 0.25,
                histogram: vec![(0.25, 1)],
            }],
        },
        Case {
            case: "a request with no recorded status counts as Ok",
            requests: vec![Request {
                start: ALIGNED + 1.0,
                duration: Some(2.0),
                status: None,
            }],
            window: (ALIGNED, ALIGNED + 60.0),
            period: None,
            expected: vec![ExpectedPeriod {
                timestamp: ALIGNED,
                ok: 1,
                throttle: 0,
                error_other: 0,
                fault: 0,
                total_response_time: 2.0,
                histogram: vec![(2.0, 1)],
            }],
        },
        Case {
            case: "2xx, 404, 429 and 503 split across Ok, error, throttle and fault",
            requests: vec![
                Request {
                    start: ALIGNED + 1.0,
                    duration: Some(0.1),
                    status: Some(200),
                },
                Request {
                    start: ALIGNED + 2.0,
                    duration: Some(0.2),
                    status: Some(404),
                },
                Request {
                    start: ALIGNED + 3.0,
                    duration: Some(0.3),
                    status: Some(429),
                },
                Request {
                    start: ALIGNED + 4.0,
                    duration: Some(0.4),
                    status: Some(503),
                },
            ],
            window: (ALIGNED, ALIGNED + 60.0),
            period: None,
            expected: vec![ExpectedPeriod {
                timestamp: ALIGNED,
                ok: 1,
                throttle: 1,
                error_other: 1,
                fault: 1,
                total_response_time: 1.0,
                histogram: vec![(0.1, 1), (0.2, 1), (0.3, 1), (0.4, 1)],
            }],
        },
        Case {
            case: "a 301 is neither an error nor a fault, and a 500 is a fault",
            requests: vec![
                Request {
                    start: ALIGNED + 5.0,
                    duration: Some(0.5),
                    status: Some(301),
                },
                Request {
                    start: ALIGNED + 6.0,
                    duration: Some(1.5),
                    status: Some(500),
                },
            ],
            window: (ALIGNED, ALIGNED + 60.0),
            period: None,
            expected: vec![ExpectedPeriod {
                timestamp: ALIGNED,
                ok: 1,
                throttle: 0,
                error_other: 0,
                fault: 1,
                total_response_time: 2.0,
                histogram: vec![(0.5, 1), (1.5, 1)],
            }],
        },
        Case {
            case: "requests a minute apart land in different 60-second periods",
            requests: vec![
                Request {
                    start: ALIGNED + 10.0,
                    duration: Some(0.125),
                    status: Some(200),
                },
                Request {
                    start: ALIGNED + 70.0,
                    duration: Some(0.375),
                    status: Some(403),
                },
            ],
            window: (ALIGNED, ALIGNED + 120.0),
            period: Some(60),
            expected: vec![
                ExpectedPeriod {
                    timestamp: ALIGNED,
                    ok: 1,
                    throttle: 0,
                    error_other: 0,
                    fault: 0,
                    total_response_time: 0.125,
                    histogram: vec![(0.125, 1)],
                },
                ExpectedPeriod {
                    timestamp: ALIGNED + 60.0,
                    ok: 0,
                    throttle: 0,
                    error_other: 1,
                    fault: 0,
                    total_response_time: 0.375,
                    histogram: vec![(0.375, 1)],
                },
            ],
        },
        Case {
            case: "a wider Period merges those two requests into one period, aligned to the epoch",
            requests: vec![
                Request {
                    start: ALIGNED + 10.0,
                    duration: Some(0.125),
                    status: Some(200),
                },
                Request {
                    start: ALIGNED + 70.0,
                    duration: Some(0.375),
                    status: Some(403),
                },
            ],
            window: (ALIGNED, ALIGNED + 120.0),
            period: Some(300),
            // 300-second periods are multiples of 300 since the epoch, not
            // offsets from StartTime: 1_699_999_980 sits inside the period that
            // starts at 1_699_999_800 == 300 * 5_666_666.
            expected: vec![ExpectedPeriod {
                timestamp: 1_699_999_800.0,
                ok: 1,
                throttle: 0,
                error_other: 1,
                fault: 0,
                total_response_time: 0.5,
                histogram: vec![(0.125, 1), (0.375, 1)],
            }],
        },
        Case {
            // The periods a statistics read serves are the one-minute and the
            // five-minute aggregation, so the five-minute one separates as well
            // as it merges: two requests in two different 300-second periods
            // occupy two datapoints, each aligned to a multiple of 300 since the
            // epoch (1_699_999_800 == 300 * 5_666_666, and the next one 300
            // seconds later).
            case: "the five-minute Period reports requests in two of its periods apart",
            requests: vec![
                Request {
                    start: ALIGNED + 10.25,
                    duration: Some(0.1),
                    status: Some(200),
                },
                Request {
                    start: ALIGNED + 320.0,
                    duration: Some(0.2),
                    status: Some(200),
                },
            ],
            window: (ALIGNED, ALIGNED + 400.0),
            period: Some(300),
            expected: vec![
                ExpectedPeriod {
                    timestamp: 1_699_999_800.0,
                    ok: 1,
                    throttle: 0,
                    error_other: 0,
                    fault: 0,
                    total_response_time: 0.1,
                    histogram: vec![(0.1, 1)],
                },
                ExpectedPeriod {
                    timestamp: 1_700_000_100.0,
                    ok: 1,
                    throttle: 0,
                    error_other: 0,
                    fault: 0,
                    total_response_time: 0.2,
                    histogram: vec![(0.2, 1)],
                },
            ],
        },
        Case {
            case: "equal response times share one histogram entry",
            requests: vec![
                Request {
                    start: ALIGNED + 7.0,
                    duration: Some(0.5),
                    status: Some(200),
                },
                Request {
                    start: ALIGNED + 8.0,
                    duration: Some(0.5),
                    status: Some(200),
                },
                Request {
                    start: ALIGNED + 9.0,
                    duration: Some(0.75),
                    status: Some(200),
                },
            ],
            window: (ALIGNED, ALIGNED + 60.0),
            period: None,
            expected: vec![ExpectedPeriod {
                timestamp: ALIGNED,
                ok: 3,
                throttle: 0,
                error_other: 0,
                fault: 0,
                total_response_time: 1.75,
                histogram: vec![(0.5, 2), (0.75, 1)],
            }],
        },
        Case {
            case: "an in-progress request is not a completed one, so no period reports it",
            requests: vec![Request {
                start: ALIGNED + 10.0,
                duration: None,
                status: None,
            }],
            window: (ALIGNED, ALIGNED + 60.0),
            period: None,
            expected: vec![],
        },
        Case {
            case: "the window is half-open: a request at StartTime counts, one at EndTime does not",
            requests: vec![
                Request {
                    start: ALIGNED,
                    duration: Some(1.0),
                    status: Some(200),
                },
                Request {
                    start: ALIGNED + 60.0,
                    duration: Some(1.0),
                    status: Some(200),
                },
            ],
            window: (ALIGNED, ALIGNED + 60.0),
            period: None,
            expected: vec![ExpectedPeriod {
                timestamp: ALIGNED,
                ok: 1,
                throttle: 0,
                error_other: 0,
                fault: 0,
                total_response_time: 1.0,
                histogram: vec![(1.0, 1)],
            }],
        },
        Case {
            case: "a request whose end precedes its start is counted nowhere",
            requests: vec![Request {
                start: ALIGNED + 10.0,
                duration: Some(-1.0),
                status: Some(200),
            }],
            window: (ALIGNED, ALIGNED + 60.0),
            period: None,
            expected: vec![],
        },
        Case {
            case: "a request before the window is not counted",
            requests: vec![Request {
                start: ALIGNED - 120.0,
                duration: Some(1.0),
                status: Some(200),
            }],
            window: (ALIGNED, ALIGNED + 60.0),
            period: None,
            expected: vec![],
        },
        Case {
            case: "a zero-width window contains nothing",
            requests: vec![Request {
                start: ALIGNED + 10.0,
                duration: Some(1.0),
                status: Some(200),
            }],
            window: (ALIGNED, ALIGNED),
            period: None,
            expected: vec![],
        },
    ]
}

/// A member read as an `i64`, or `-1` so a missing count fails loudly against
/// an expected zero rather than matching it.
fn count(value: &Value, member: &str) -> i64 {
    value.get(member).and_then(Value::as_i64).unwrap_or(-1)
}

/// `GetTimeSeriesServiceStatistics` with the given window, period and token.
async fn statistics(
    xray: &Xray,
    window: (f64, f64),
    period: Option<i64>,
    token: Option<&str>,
) -> crate::trace_ingest_and_groups::Answer {
    let mut body = json!({"StartTime": window.0, "EndTime": window.1});
    if let Some(period) = period {
        body["Period"] = json!(period);
    }
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    xray.call(PATH, &body).await
}

/// Assert one answered period against what the rule says it must report.
fn assert_period(case: &str, actual: &Value, expected: &ExpectedPeriod) {
    let timestamp = actual.get("Timestamp").and_then(Value::as_f64);
    assert!(
        timestamp.is_some_and(|actual| close(actual, expected.timestamp)),
        "case {case:?}: expected Timestamp {}, got {actual:?}",
        expected.timestamp
    );
    // "If no selector expression is specified, edge statistics are returned."
    let edge = actual.get("EdgeSummaryStatistics");
    let Some(edge) = edge else {
        panic!("case {case:?}: expected EdgeSummaryStatistics, got {actual:?}");
    };
    let errors = edge
        .get("ErrorStatistics")
        .cloned()
        .unwrap_or(Value::Null);
    let faults = edge
        .get("FaultStatistics")
        .cloned()
        .unwrap_or(Value::Null);
    let expected_total = expected.ok + expected.throttle + expected.error_other + expected.fault;
    let actual_counts = (
        count(edge, "OkCount"),
        count(&errors, "ThrottleCount"),
        count(&errors, "OtherCount"),
        count(&errors, "TotalCount"),
        count(&faults, "TotalCount"),
        count(&faults, "OtherCount"),
        count(edge, "TotalCount"),
    );
    let expected_counts = (
        expected.ok,
        expected.throttle,
        expected.error_other,
        expected.throttle + expected.error_other,
        expected.fault,
        expected.fault,
        expected_total,
    );
    assert_eq!(
        actual_counts, expected_counts,
        "case {case:?}: expected (Ok, Throttle, ErrorOther, ErrorTotal, FaultTotal, FaultOther, Total) {expected_counts:?}, got {actual:?}"
    );
    let response_time = edge.get("TotalResponseTime").and_then(Value::as_f64);
    assert!(
        response_time.is_some_and(|actual| close(actual, expected.total_response_time)),
        "case {case:?}: expected TotalResponseTime {}, got {actual:?}",
        expected.total_response_time
    );
    let histogram: Vec<(f64, i64)> = array(actual, "ResponseTimeHistogram")
        .iter()
        .map(|entry| {
            (
                entry.get("Value").and_then(Value::as_f64).unwrap_or(-1.0),
                count(entry, "Count"),
            )
        })
        .collect();
    assert_eq!(
        histogram.len(),
        expected.histogram.len(),
        "case {case:?}: expected {} ResponseTimeHistogram entries, got {histogram:?}",
        expected.histogram.len()
    );
    for (actual_entry, expected_entry) in histogram.iter().zip(&expected.histogram) {
        assert!(
            close(actual_entry.0, expected_entry.0) && actual_entry.1 == expected_entry.1,
            "case {case:?}: expected histogram entry {expected_entry:?}, got {actual_entry:?}"
        );
    }
    assert!(
        actual.get("ServiceForecastStatistics").is_none(),
        "case {case:?}: a request that asked for no forecast must get none, got {actual:?}"
    );
}

// =============================================================================
// 1. Every count is recomputed from the stored segment documents
// =============================================================================

#[tokio::test]
async fn aggregates_the_stored_segments_into_periods() {
    for (row, case) in aggregation_cases().into_iter().enumerate() {
        let label = case.case;
        let xray = Xray::new();
        let trace = trace_id(u32::try_from(row).expect("the row index fits"));
        let documents: Vec<String> = case
            .requests
            .iter()
            .enumerate()
            .map(|(n, request)| {
                document(
                    n,
                    &trace,
                    request.start,
                    request.duration.map(|duration| request.start + duration),
                    request.status,
                    declared_for(request.status),
                )
            })
            .collect();
        if !documents.is_empty() {
            let put = xray.put_segments(&documents).await;
            assert_eq!(
                (put.status, array(&put.body, "UnprocessedTraceSegments").len()),
                (200, 0),
                "case {label:?}: the ingest must accept every document, got {put:?}"
            );
        }

        let answered = statistics(&xray, case.window, case.period, None).await;
        assert_eq!(
            answered.status, 200,
            "case {label:?}: expected HTTP 200, got {answered:?}"
        );
        assert_eq!(
            answered.get("ContainsOldGroupVersions"),
            &json!(false),
            "case {label:?}: no aggregation here spans a changed filter expression, got {answered:?}"
        );
        assert!(
            answered.get("NextToken").is_null(),
            "case {label:?}: a single page must carry no NextToken, got {answered:?}"
        );
        let periods = array(&answered.body, "TimeSeriesServiceStatistics");
        assert_eq!(
            periods.len(),
            case.expected.len(),
            "case {label:?}: expected {} period(s), got {periods:?}",
            case.expected.len()
        );
        for (actual, expected) in periods.iter().zip(&case.expected) {
            assert_period(label, actual, expected);
        }

        // Reporting, not mutating: the same question answers the same way, so
        // nothing the first call did changed what the second one sees.
        let again = statistics(&xray, case.window, case.period, None).await;
        assert_eq!(
            (again.status, &again.body),
            (200, &answered.body),
            "case {label:?}: a second identical read must answer identically, got {again:?}"
        );
    }
}

// =============================================================================
// 2. Periods are aligned to the epoch, not to the requested StartTime
// =============================================================================

#[tokio::test]
async fn one_request_is_reported_under_one_period_whatever_window_asks() {
    let xray = Xray::new();
    let trace = trace_id(20);
    let at = ALIGNED + 42.0;
    let put = xray
        .put_segments(&[document(0, &trace, at, Some(at + 0.5), Some(200), None)])
        .await;
    assert_eq!(put.status, 200, "the ingest must succeed, got {put:?}");

    // Three windows that all contain the request, each starting at a different
    // offset inside and before its period.
    let windows = [
        ("window starting on the period boundary", ALIGNED, ALIGNED + 60.0),
        ("window starting mid-period", ALIGNED + 41.0, ALIGNED + 50.0),
        ("window spanning several periods", ALIGNED - 125.0, ALIGNED + 300.0),
    ];
    for (label, start, end) in windows {
        let answered = statistics(&xray, (start, end), Some(60), None).await;
        let periods = array(&answered.body, "TimeSeriesServiceStatistics");
        assert_eq!(
            (answered.status, periods.len()),
            (200, 1),
            "case {label:?}: exactly the one period holding the request must be reported, got {answered:?}"
        );
        assert_period(
            label,
            &periods[0],
            &ExpectedPeriod {
                timestamp: ALIGNED,
                ok: 1,
                throttle: 0,
                error_other: 0,
                fault: 0,
                total_response_time: 0.5,
                histogram: vec![(0.5, 1)],
            },
        );
    }
}

// =============================================================================
// 3. A NextToken only when the aggregation was truncated
// =============================================================================

#[tokio::test]
async fn pages_the_periods_and_resumes_exactly_once() {
    // One request per minute, over more minutes than one page carries.
    const MINUTES: usize = 7;
    let xray = Xray::new();
    let trace = trace_id(30);
    #[allow(clippy::cast_precision_loss)]
    let documents: Vec<String> = (0..MINUTES)
        .map(|minute| {
            let start = ALIGNED + 60.0 * minute as f64 + 5.0;
            document(minute, &trace, start, Some(start + 0.25), Some(200), None)
        })
        .collect();
    let put = xray.put_segments(&documents).await;
    assert_eq!(put.status, 200, "the ingest must succeed, got {put:?}");

    #[allow(clippy::cast_precision_loss)]
    let window = (ALIGNED, ALIGNED + 60.0 * MINUTES as f64);
    let mut seen: Vec<f64> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let answered = statistics(&xray, window, Some(60), token.as_deref()).await;
        assert_eq!(
            answered.status, 200,
            "page {pages}: expected HTTP 200, got {answered:?}"
        );
        let periods = array(&answered.body, "TimeSeriesServiceStatistics");
        assert!(
            !periods.is_empty(),
            "page {pages}: a page must make progress, got {answered:?}"
        );
        for period in periods {
            seen.push(
                period
                    .get("Timestamp")
                    .and_then(Value::as_f64)
                    .unwrap_or(-1.0),
            );
        }
        pages += 1;
        token = answered
            .get("NextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        assert!(pages <= MINUTES, "paging must terminate, saw {seen:?}");
        if token.is_none() {
            break;
        }
    }
    assert!(
        pages > 1,
        "{MINUTES} periods must not fit in one page, got {pages} page(s) and {seen:?}"
    );
    #[allow(clippy::cast_precision_loss)]
    let expected: Vec<f64> = (0..MINUTES)
        .map(|minute| ALIGNED + 60.0 * minute as f64)
        .collect();
    assert_eq!(
        seen.len(),
        expected.len(),
        "every period must be reported exactly once, got {seen:?}"
    );
    for (actual, expected) in seen.iter().zip(&expected) {
        assert!(
            close(*actual, *expected),
            "paged periods must ascend without repeating: expected {expected}, got {actual}"
        );
    }
}

// =============================================================================
// 4. A selection this slice does not evaluate is refused, not ignored
// =============================================================================

#[tokio::test]
async fn refuses_what_it_cannot_aggregate() {
    let xray = Xray::new();
    let trace = trace_id(40);
    let put = xray
        .put_segments(&[document(
            0,
            &trace,
            ALIGNED + 1.0,
            Some(ALIGNED + 2.0),
            Some(200),
            None,
        )])
        .await;
    assert_eq!(put.status, 200, "the ingest must succeed, got {put:?}");

    // Three groups to pull statistics from: the reserved account-level `Default`
    // group, which carries NO filter expression (X-Ray refuses a `CreateGroup`
    // that supplies none, see `group_control_plane.rs`), one whose expression
    // SELECTS the ingested traffic, and one whose expression selects none of it.
    // All three are answered — the expression narrows WHAT is aggregated, it is
    // not a reason to refuse.
    let listed = xray.call("/Groups", &json!({})).await;
    assert_eq!(listed.status, 200, "GetGroups must succeed, got {listed:?}");
    let plain_arn = array(&listed.body, "Groups")
        .iter()
        .find(|group| group.get("GroupName").and_then(Value::as_str) == Some("Default"))
        .and_then(|group| group.get("GroupARN").and_then(Value::as_str))
        .expect("the reserved Default group is listed and carries an ARN")
        .to_string();
    let filtered = xray
        .call(
            "/CreateGroup",
            &json!({"GroupName": "filtered", "FilterExpression": "service(\"svc\")"}),
        )
        .await;
    assert_eq!(
        filtered.status, 200,
        "CreateGroup must succeed, got {filtered:?}"
    );
    let unmatched = xray
        .call(
            "/CreateGroup",
            &json!({"GroupName": "unmatched", "FilterExpression": "service(\"other\")"}),
        )
        .await;
    assert_eq!(
        unmatched.status, 200,
        "CreateGroup must succeed, got {unmatched:?}"
    );

    let base = json!({"StartTime": ALIGNED, "EndTime": ALIGNED + 60.0});
    let rows: Vec<(&str, Value)> = vec![
        (
            "an EntitySelectorExpression that is not of the language",
            json!({"EntitySelectorExpression": "select everything"}),
        ),
        (
            "an EntitySelectorExpression naming a function that selects no entity",
            json!({"EntitySelectorExpression": "group(\"Default\")"}),
        ),
        (
            "an id function labelling a member no entity has",
            json!({"EntitySelectorExpression": "id(colour: \"red\")"}),
        ),
        (
            "a forecast is not computed from recorded traffic",
            json!({"ForecastStatistics": true}),
        ),
        (
            "a zero Period is no period at all",
            json!({"Period": 0}),
        ),
        (
            "a negative Period is no period at all",
            json!({"Period": -60}),
        ),
        (
            "a NextToken this service never issued",
            json!({"NextToken": "not-a-period"}),
        ),
        (
            "a GroupName that names no group",
            json!({"GroupName": "absent"}),
        ),
        (
            "a GroupARN that names no group",
            json!({"GroupARN": "arn:aws:xray:us-east-1:123456789012:group/absent/0123456789ab"}),
        ),
        // The crate's one window check, shared with GetServiceGraph and
        // StartTraceRetrieval (`crate::state::validate_window`).
        (
            "a window running backwards names no interval",
            json!({"StartTime": ALIGNED + 60.0, "EndTime": ALIGNED}),
        ),
        (
            "a bound further ahead than the clock skew the check allows",
            json!({"EndTime": ALIGNED * 1_000.0}),
        ),
    ];
    for (label, overrides) in rows {
        let mut body = base.clone();
        for (member, value) in overrides.as_object().expect("the row is an object") {
            body[member] = value.clone();
        }
        let refused = xray.call(PATH, &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: expected 400 InvalidRequestException, got {refused:?}"
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

    // And every group selector answers the aggregation the group's own
    // `FilterExpression` selects: by either selector for the unfiltered group,
    // the matching traffic for a group whose expression selects it, and an empty
    // series for one whose expression selects nothing.
    for (label, selector, periods_expected) in [
        ("by GroupName", json!({"GroupName": "Default"}), 1),
        ("by GroupARN", json!({"GroupARN": plain_arn}), 1),
        (
            "a group whose FilterExpression selects the traffic",
            json!({"GroupName": "filtered"}),
            1,
        ),
        (
            "a group whose FilterExpression selects none of it",
            json!({"GroupName": "unmatched"}),
            0,
        ),
    ] {
        let mut body = base.clone();
        for (member, value) in selector.as_object().expect("the selector is an object") {
            body[member] = value.clone();
        }
        let answered = xray.call(PATH, &body).await;
        let periods = array(&answered.body, "TimeSeriesServiceStatistics");
        assert_eq!(
            (answered.status, periods.len()),
            (200, periods_expected),
            "case {label:?}: a group selector must answer the aggregation its FilterExpression selects, got {answered:?}"
        );
        assert_eq!(
            answered.get("ContainsOldGroupVersions"),
            &json!(false),
            "case {label:?}: the aggregation is recomputed from stored segments, so it mixes no older group version, got {answered:?}"
        );
        if periods_expected == 0 {
            continue;
        }
        assert_period(
            label,
            &periods[0],
            &ExpectedPeriod {
                timestamp: ALIGNED,
                ok: 1,
                throttle: 0,
                error_other: 0,
                fault: 0,
                total_response_time: 1.0,
                histogram: vec![(1.0, 1)],
            },
        );
    }
}
