// SPDX-License-Identifier: CC0-1.0

//! Three rules of the trace-retrieval and sampling data planes, end to end over
//! the wire (the shared `Xray` harness in [`super::xray_wire`]):
//!
//! * `CancelTraceRetrieval` asks for a retrieval to be stopped, so a retrieval
//!   already in a status it can no longer leave is answered the same empty
//!   result as one the call stops, and is left exactly as it stands — only a
//!   token naming no retrieval is refused;
//! * `StartTraceRetrieval` refuses a `[StartTime, EndTime]` window outside the
//!   interval a retrieval can cover with the service's FIXED text for that
//!   class, interpolating no bound of the request and no limit, which is a
//!   different class from a window that merely runs backwards; and
//! * a reported sampling window belongs to the service that reported it, so one
//!   aggregates across the services reporting it while a service restating the
//!   window it is in replaces its own figures rather than being counted twice.
//!
//! Each table's rows differ along what its rule generalises over: the status the
//! retrieval is in and how often the cancel is repeated, which bound of the
//! window is out of range and how far, and how many services report a window
//! and how often each repeats itself.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::state::{
    retrieval_token_check, INVALID_INPUT_PARAMETERS_PREAMBLE, INVERTED_WINDOW_MESSAGE,
    MAX_WINDOW_FUTURE_SKEW_SECONDS, RETRIEVAL_STATUS_CANCELLED, RETRIEVAL_STATUS_COMPLETE,
    RETRIEVAL_TOKEN_HEX_DIGITS, RETRIEVAL_WINDOW_OUT_OF_RANGE_MESSAGE, SAMPLING_WINDOW_SECONDS,
    TRACE_FORMAT_XRAY, TRACE_RETRIEVAL_RUNNING_SPAN_SECONDS,
    TRACE_RETRIEVAL_SCHEDULED_SPAN_SECONDS, XRAY_GENERAL_AVAILABILITY_EPOCH_SECONDS,
};

use super::xray_wire::{array, root_document, trace_id, Answer, Xray};

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

/// Ingest one root segment and start a retrieval over it, answering the
/// `RetrievalToken` and the trace id the retrieval holds.
async fn started_retrieval(xray: &Xray, seed: u32) -> (String, String) {
    let base = (now() - 600.0).trunc();
    let trace = trace_id(seed);
    let document = root_document(
        &format!("{seed:016x}"),
        &trace,
        "frontend",
        base + 1.0,
        Some(base + 2.0),
    );
    let ingested = xray.put_segments(&[document.to_string()]).await;
    assert_eq!(
        ingested.status, 200,
        "the segment must be ingested, got {ingested:?}"
    );
    let started = xray
        .call(
            "/StartTraceRetrieval",
            &json!({"TraceIds": [&trace], "StartTime": base, "EndTime": base + 300.0}),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "starting a retrieval must succeed, got {started:?}"
    );
    let token = started
        .get("RetrievalToken")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "a started retrieval must answer a token, got {started:?}"
    );
    (token, trace)
}

/// `CancelTraceRetrieval` for `token`.
async fn cancel(xray: &Xray, token: &str) -> Answer {
    xray.call("/CancelTraceRetrieval", &json!({"RetrievalToken": token}))
        .await
}

/// `ListRetrievedTraces` for `token`.
async fn listed(xray: &Xray, token: &str) -> Answer {
    xray.call(
        "/ListRetrievedTraces",
        &json!({"RetrievalToken": token, "TraceFormat": TRACE_FORMAT_XRAY}),
    )
    .await
}

// =============================================================================
// 1. Cancelling is answered the empty result in every state a retrieval holds
// =============================================================================

/// One retrieval to cancel: how long to let the clock run before cancelling,
/// how many times to cancel, and the status the retrieval must report
/// afterwards.
#[derive(Debug)]
struct CancelCase {
    case: &'static str,
    wait: Option<Duration>,
    cancels: usize,
    expected: &'static str,
}

#[tokio::test]
async fn cancelling_is_answered_the_empty_result_in_every_state_a_retrieval_holds() {
    // Rows differ in the status the retrieval is in when the cancel lands —
    // still ongoing, already stopped, already finished — and in how many times
    // a client that cancels defensively repeats the call.
    let cases = vec![
        CancelCase {
            case: "an ongoing retrieval",
            wait: None,
            cancels: 1,
            expected: RETRIEVAL_STATUS_CANCELLED,
        },
        CancelCase {
            case: "a retrieval already cancelled",
            wait: None,
            cancels: 2,
            expected: RETRIEVAL_STATUS_CANCELLED,
        },
        CancelCase {
            case: "a retrieval cancelled again and again",
            wait: None,
            cancels: 4,
            expected: RETRIEVAL_STATUS_CANCELLED,
        },
        CancelCase {
            case: "a retrieval that already completed",
            wait: Some(completion_wait()),
            cancels: 1,
            expected: RETRIEVAL_STATUS_COMPLETE,
        },
        CancelCase {
            case: "a completed retrieval cancelled twice",
            wait: Some(completion_wait()),
            cancels: 3,
            expected: RETRIEVAL_STATUS_COMPLETE,
        },
    ];

    for case in cases {
        let label = case.case;
        // A fresh emulator per row, so no row spends another row's request
        // budget.
        let xray = Xray::new();
        let (token, trace) = started_retrieval(&xray, 820).await;
        if let Some(wait) = case.wait {
            tokio::time::sleep(wait).await;
        }

        for attempt in 1..=case.cancels {
            let answer = cancel(&xray, &token).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: cancel {attempt} of {} must answer HTTP 200, got {answer:?}",
                case.cancels
            );
            assert_eq!(
                answer.body,
                json!({}),
                "case {label:?}: cancel {attempt} must answer an empty body, got {:?}",
                answer.body
            );
        }

        // The stored retrieval is where the cancel left it: stopped if the call
        // had a job to stop, and untouched if it did not.
        let read = listed(&xray, &token).await;
        let status = read.get("RetrievalStatus").as_str().unwrap_or_default();
        assert_eq!(
            (read.status, status),
            (200, case.expected),
            "case {label:?}: the retrieval must report {}, got {read:?}",
            case.expected
        );
        let ids: Vec<&str> = array(&read.body, "Traces")
            .iter()
            .filter_map(|answered| answered.get("Id").and_then(Value::as_str))
            .collect();
        let expected_ids: Vec<&str> = if case.expected == RETRIEVAL_STATUS_COMPLETE {
            vec![trace.as_str()]
        } else {
            Vec::new()
        };
        assert_eq!(
            ids, expected_ids,
            "case {label:?}: a cancel must leave the retrieval's traces as they were, expected {expected_ids:?}, got {read:?}"
        );
    }
}

#[tokio::test]
async fn only_a_token_naming_no_retrieval_is_refused_by_a_cancel() {
    let xray = Xray::new();
    let (real, _) = started_retrieval(&xray, 821).await;

    // Rows differ in HOW the token fails to name a retrieval: a value of the
    // minted form that verifies but stands for nothing, and one of a form this
    // service could not have issued at all.
    let intact = {
        let identity = "0123456789abcdef";
        format!("{identity}{}", retrieval_token_check(identity))
    };
    let refused: [(&str, String, u16, &str); 3] = [
        (
            "an intact token nothing minted",
            intact,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a token of another form entirely",
            "not-a-retrieval-token".to_string(),
            400,
            "InvalidRequestException",
        ),
        (
            "a token of the minted form whose check does not verify",
            "f".repeat(RETRIEVAL_TOKEN_HEX_DIGITS),
            400,
            "InvalidRequestException",
        ),
    ];

    for (case, token, status, code) in &refused {
        let answer = cancel(&xray, token).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (*status, Some(*code)),
            "case {case:?}: a token naming no retrieval must be refused, got {answer:?}"
        );
    }

    // And the retrieval that does exist is untouched by every refusal.
    let read = listed(&xray, &real).await;
    assert_eq!(
        read.status, 200,
        "the real retrieval must still be served, got {read:?}"
    );
}

// =============================================================================
// 2. A window no retrieval could cover is refused with the service's fixed text
// =============================================================================

/// One window `StartTraceRetrieval` is asked for, and the message it must be
/// answered with — `None` for a window that must be accepted.
#[derive(Debug)]
struct WindowCase {
    case: &'static str,
    start: f64,
    end: f64,
    refusal: Option<String>,
}

#[tokio::test]
async fn a_window_no_retrieval_could_cover_is_refused_with_the_services_fixed_text() {
    // The fixed text interpolates nothing, so it cannot vary from run to run.
    assert_eq!(
        RETRIEVAL_WINDOW_OUT_OF_RANGE_MESSAGE,
        "Invalid input parameters for starting trace retrieval.End time should not be before the service was generally available.",
        "the fixed text must be the service's own, got {RETRIEVAL_WINDOW_OUT_OF_RANGE_MESSAGE:?}"
    );

    let out_of_range = Some(RETRIEVAL_WINDOW_OUT_OF_RANGE_MESSAGE.to_string());
    let inverted = Some(format!(
        "{INVALID_INPUT_PARAMETERS_PREAMBLE} {INVERTED_WINDOW_MESSAGE}"
    ));
    // The floor the message states: when X-Ray became generally available.
    let floor = XRAY_GENERAL_AVAILABILITY_EPOCH_SECONDS;
    let skew = MAX_WINDOW_FUTURE_SKEW_SECONDS;
    // Rows differ in WHICH bound leaves the interval a retrieval can cover and
    // in which direction, and include each bound of that interval and one step
    // past it.
    let cases = vec![
        WindowCase {
            case: "an hour-long window ending now",
            start: now() - 3_600.0,
            end: now(),
            refusal: None,
        },
        WindowCase {
            case: "a window of a single instant on the availability floor",
            start: floor,
            end: floor,
            refusal: None,
        },
        WindowCase {
            case: "a window ending one second below the availability floor",
            start: floor - 3_600.0,
            end: floor - 1.0,
            refusal: out_of_range.clone(),
        },
        WindowCase {
            case: "a window ending at the epoch",
            start: 0.0,
            end: 0.0,
            refusal: out_of_range.clone(),
        },
        WindowCase {
            case: "a window ending before the epoch",
            start: -7_200.0,
            end: -3_600.0,
            refusal: out_of_range.clone(),
        },
        WindowCase {
            case: "a window ending just inside the clock-skew allowance",
            start: now() - 60.0,
            end: now() + skew - 60.0,
            refusal: None,
        },
        WindowCase {
            case: "a window ending just past the clock-skew allowance",
            start: now() - 60.0,
            end: now() + skew + 60.0,
            refusal: out_of_range.clone(),
        },
        WindowCase {
            case: "the classic epoch-seconds-scaled-by-1000 anchor",
            start: now() * 1_000.0,
            end: now() * 1_000.0 + 3_600.0,
            refusal: out_of_range.clone(),
        },
        WindowCase {
            case: "a millisecond-scale end of a well-formed start",
            start: now() - 3_600.0,
            end: now() * 1_000.0,
            refusal: out_of_range,
        },
        WindowCase {
            case: "a window running backwards inside the interval",
            start: now(),
            end: now() - 1.0,
            refusal: inverted.clone(),
        },
        WindowCase {
            case: "a window running backwards by a day",
            start: now(),
            end: now() - 86_400.0,
            refusal: inverted,
        },
    ];

    for case in cases {
        let label = case.case;
        let xray = Xray::new();
        let answer = xray
            .call(
                "/StartTraceRetrieval",
                &json!({
                    "TraceIds": [trace_id(830)],
                    "StartTime": case.start,
                    "EndTime": case.end,
                }),
            )
            .await;
        let Some(expected) = case.refusal.as_deref() else {
            assert_eq!(
                answer.status, 200,
                "case {label:?}: a window a retrieval can cover must be accepted, got {answer:?}"
            );
            continue;
        };
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: the window must be refused, got {answer:?}"
        );
        let said = answer.message().unwrap_or_default();
        assert_eq!(
            said, expected,
            "case {label:?}: the refusal must state the service's text for its class, expected {expected:?}, got {said:?}"
        );
        // Neither bound reaches the refusal: the text is fixed.
        for bound in [case.start, case.end] {
            let rendered = format!("{bound}");
            assert!(
                !said.contains(&rendered),
                "case {label:?}: the refusal must interpolate no bound of the request, got {said:?}"
            );
        }
    }
}

// =============================================================================
// 3. A reported window is aggregated across services, not across repeats
// =============================================================================

/// A 24-character hexadecimal `ClientID`, the only length the model accepts
/// (`smithy_query(query="shape",shape="SamplingStatisticsDocument")`:
/// `@length(min: 24, max: 24)`).
fn client_id(seed: u32) -> String {
    format!("{seed:024x}")
}

/// One `SamplingStatisticsDocument` a service reports.
fn statistics(rule: &str, client: &str, at: f64, requests: i64, sampled: i64, borrowed: i64) -> Value {
    json!({
        "RuleName": rule,
        "ClientID": client,
        "Timestamp": at,
        "RequestCount": requests,
        "SampledCount": sampled,
        "BorrowCount": borrowed,
    })
}

/// `CreateSamplingRule` for a rule every request matches.
async fn create_rule(xray: &Xray, name: &str, reservoir_size: i64, fixed_rate: f64) -> Answer {
    xray.call(
        "/CreateSamplingRule",
        &json!({"SamplingRule": {
            "RuleName": name,
            "ResourceARN": "*",
            "Priority": 7,
            "FixedRate": fixed_rate,
            "ReservoirSize": reservoir_size,
            "ServiceName": "*",
            "ServiceType": "*",
            "Host": "*",
            "HTTPMethod": "*",
            "URLPath": "*",
            "Version": 1,
        }}),
    )
    .await
}

/// The start of the ten-second window `at` falls in.
#[allow(clippy::cast_precision_loss)]
fn window_start(at: f64) -> f64 {
    let span = SAMPLING_WINDOW_SECONDS as f64;
    (at / span).floor() * span
}

/// What one reporting window of one rule must summarise to.
#[derive(Debug)]
struct ReportedWindow {
    /// The documents reported, in order: `(client, window offset, requests,
    /// sampled, borrowed)`.
    reports: Vec<(String, f64, i64, i64, i64)>,
    /// The counters the window must summarise to, per window offset.
    expected: Vec<(f64, i64, i64, i64)>,
}

#[tokio::test]
async fn a_reported_window_aggregates_across_services_and_not_across_repeats() {
    let first = client_id(41);
    let second = client_id(42);
    // Rows differ in how many services report a window, how often each repeats
    // itself, whether a repeat restates the same figures or revised ones, and
    // how many windows are reported.
    let cases: Vec<(&str, ReportedWindow)> = vec![
        (
            "one service reporting one window once",
            ReportedWindow {
                reports: vec![(first.clone(), 0.0, 500, 50, 0)],
                expected: vec![(0.0, 500, 50, 0)],
            },
        ),
        (
            "one service polling, restating the same window three times",
            ReportedWindow {
                reports: vec![
                    (first.clone(), 0.0, 500, 50, 0),
                    (first.clone(), 1.0, 500, 50, 0),
                    (first.clone(), 2.0, 500, 50, 0),
                ],
                expected: vec![(0.0, 500, 50, 0)],
            },
        ),
        (
            "one service revising what it reported for the window",
            ReportedWindow {
                reports: vec![
                    (first.clone(), 0.0, 500, 50, 0),
                    (first.clone(), 3.0, 800, 90, 4),
                ],
                expected: vec![(0.0, 800, 90, 4)],
            },
        ),
        (
            "two services reporting the window, each polling twice",
            ReportedWindow {
                reports: vec![
                    (first.clone(), 0.0, 500, 50, 0),
                    (second.clone(), 1.0, 200, 20, 1),
                    (first.clone(), 2.0, 500, 50, 0),
                    (second.clone(), 3.0, 200, 20, 1),
                ],
                expected: vec![(0.0, 700, 70, 1)],
            },
        ),
        (
            "one service reporting two windows, repeating each",
            ReportedWindow {
                reports: vec![
                    (first.clone(), 0.0, 500, 50, 0),
                    (first.clone(), 1.0, 500, 50, 0),
                    (first.clone(), 10.0, 300, 30, 2),
                    (first.clone(), 11.0, 300, 30, 2),
                ],
                expected: vec![(0.0, 500, 50, 0), (10.0, 300, 30, 2)],
            },
        ),
        (
            "a window nothing was recorded for",
            ReportedWindow {
                reports: vec![(first.clone(), 0.0, 0, 0, 0), (first.clone(), 4.0, 0, 0, 0)],
                expected: vec![(0.0, 0, 0, 0)],
            },
        ),
    ];

    for (label, case) in cases {
        let xray = Xray::new();
        let rule = "reported-windows";
        let created = create_rule(&xray, rule, 15, 0.42).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateSamplingRule must succeed, got {created:?}"
        );
        let base = window_start(now());

        for (client, offset, requests, sampled, borrowed) in &case.reports {
            let answer = xray
                .call(
                    "/SamplingTargets",
                    &json!({"SamplingStatisticsDocuments": [statistics(
                        rule,
                        client,
                        base + offset,
                        *requests,
                        *sampled,
                        *borrowed,
                    )]}),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: GetSamplingTargets must succeed, got {answer:?}"
            );
        }

        let summaries = xray.call("/SamplingStatisticSummaries", &json!({})).await;
        assert_eq!(
            summaries.status, 200,
            "case {label:?}: GetSamplingStatisticSummaries must succeed, got {summaries:?}"
        );
        let rows: Vec<&Value> = array(&summaries.body, "SamplingStatisticSummaries")
            .iter()
            .filter(|row| row.get("RuleName").and_then(Value::as_str) == Some(rule))
            .collect();
        assert_eq!(
            rows.len(),
            case.expected.len(),
            "case {label:?}: each reported ten-second window is one row, got {rows:?}"
        );

        for (index, (offset, requests, sampled, borrowed)) in case.expected.iter().enumerate() {
            let row = rows[index];
            let timestamp = row.get("Timestamp").and_then(Value::as_f64);
            assert_eq!(
                timestamp,
                Some(base + offset),
                "case {label:?}: a row reports the START of its window, got {timestamp:?}"
            );
            for (member, expected) in [
                ("RequestCount", requests),
                ("SampledCount", sampled),
                ("BorrowCount", borrowed),
            ] {
                let actual = row.get(member).and_then(Value::as_i64);
                assert_eq!(
                    actual,
                    Some(*expected),
                    "case {label:?}: {member} is what the services reporting the window stated, expected {expected}, got {actual:?}"
                );
            }
        }

        // Reading the summaries changes nothing a later caller sees.
        let again = xray.call("/SamplingStatisticSummaries", &json!({})).await;
        assert_eq!(
            again.body, summaries.body,
            "case {label:?}: a repeated read must answer identically, got {:?} then {:?}",
            summaries.body, again.body
        );
    }
}
