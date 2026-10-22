// SPDX-License-Identifier: CC0-1.0

//! The `SearchFixtures` DATE WINDOW, end to end over the wire.
//!
//! Cause 5 of
//! `20261022T000233Z-elementalinference-error-contract-and-control-plane-gaps-wrong`
//! is the window rule, so it is pinned here over a table of windows; the catalog a
//! search reports from, its filters and its paging are `fixture_catalog.rs`.
//!
//! The rule: the window runs from `startDate` THROUGH `endDate`, both ends included,
//! and must not cover more than seven days
//! (`SearchFixturesRequest$endDate`: "The window from startDate through endDate must
//! not exceed seven days"); `endDate` omitted searches the `startDate` alone. A window
//! that covers more than seven days, one that ends before it starts, and a date that
//! is not a day in the calendar are each refused with the modelled
//! `ValidationException` (HTTP 400) — the model's `@pattern` `^\d{4}-\d{2}-\d{2}$`
//! admits text that names no day, so the calendar check is the service's.

use crate::harness::{call, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::MAX_SEARCH_WINDOW_DAYS;

/// One row of the window table: the window, and whether the service accepts it.
struct WindowCase {
    label: &'static str,
    start: &'static str,
    end: Option<&'static str>,
    accepted: bool,
}

/// Windows that differ along what the rule generalises over: the number of days the
/// window covers (one, up to the documented bound, one past it and far beyond it), the
/// direction, whether `endDate` is supplied at all, and whether the dates are days
/// that exist — across month ends, a year end and a leap day, so the bound is checked
/// against the calendar rather than against the digits.
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            label: "endDate omitted: the startDate alone",
            start: "2026-03-14",
            end: None,
            accepted: true,
        },
        WindowCase {
            label: "a window of one day",
            start: "2026-03-14",
            end: Some("2026-03-14"),
            accepted: true,
        },
        WindowCase {
            label: "a window of two days",
            start: "2026-03-14",
            end: Some("2026-03-15"),
            accepted: true,
        },
        WindowCase {
            label: "a window of six days",
            start: "2026-03-14",
            end: Some("2026-03-19"),
            accepted: true,
        },
        WindowCase {
            label: "a window of seven days: the documented bound",
            start: "2026-03-14",
            end: Some("2026-03-20"),
            accepted: true,
        },
        WindowCase {
            label: "a window of eight days: one past the bound",
            start: "2026-03-14",
            end: Some("2026-03-21"),
            accepted: false,
        },
        WindowCase {
            label: "a window of twelve days",
            start: "2026-03-14",
            end: Some("2026-03-25"),
            accepted: false,
        },
        WindowCase {
            label: "a month-long window",
            start: "2026-03-14",
            end: Some("2026-04-14"),
            accepted: false,
        },
        WindowCase {
            label: "a window that ends before it starts",
            start: "2026-03-14",
            end: Some("2026-03-13"),
            accepted: false,
        },
        WindowCase {
            label: "six days across a year boundary",
            start: "2025-12-28",
            end: Some("2026-01-02"),
            accepted: true,
        },
        WindowCase {
            label: "seven days across a month boundary: the bound",
            start: "2026-02-26",
            end: Some("2026-03-04"),
            accepted: true,
        },
        WindowCase {
            label: "nine days across a month boundary",
            start: "2026-02-26",
            end: Some("2026-03-06"),
            accepted: false,
        },
        WindowCase {
            label: "a leap day that exists",
            start: "2024-02-29",
            end: Some("2024-03-01"),
            accepted: true,
        },
        WindowCase {
            label: "a leap day that does not exist",
            start: "2026-02-29",
            end: None,
            accepted: false,
        },
        WindowCase {
            label: "a day that no month has",
            start: "2026-02-30",
            end: None,
            accepted: false,
        },
        WindowCase {
            label: "a month that no year has",
            start: "2026-13-01",
            end: None,
            accepted: false,
        },
        WindowCase {
            label: "a day no month has, as the endDate",
            start: "2026-03-14",
            end: Some("2026-03-32"),
            accepted: false,
        },
    ]
}

/// The window rule, over every sport the model declares.
#[tokio::test]
async fn the_search_window_must_not_cover_more_than_seven_days() {
    let router = router();
    assert_eq!(
        MAX_SEARCH_WINDOW_DAYS, 7,
        "the table below is written against the documented seven-day bound, got \
         {MAX_SEARCH_WINDOW_DAYS}"
    );

    // `smithy_query(query="shape",shape="DataSourceSport")`: the only two sports.
    for sport in ["basketball", "american-football"] {
        for case in window_cases() {
            let label = format!("{} ({sport})", case.label);
            let mut body = json!({"sport": sport, "startDate": case.start});
            if let Some(end) = case.end {
                body["endDate"] = json!(end);
            }
            let (status, answer) = call(&router, "POST", "/v1/fixtures", Some(body)).await;
            if case.accepted {
                assert_eq!(
                    status, 200,
                    "case {label:?}: this window is accepted, got {status} with {answer}"
                );
                assert!(
                    answer["fixtures"].is_array(),
                    "case {label:?}: an accepted search answers with a fixtures list, \
                     got {answer}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {label:?}: this window is refused, got {status} with {answer}"
                );
                assert_eq!(
                    answer["__type"],
                    json!("ValidationException"),
                    "case {label:?}: that refusal is a ValidationException, got {answer}"
                );
                assert_eq!(
                    answer["nextToken"],
                    Value::Null,
                    "case {label:?}: a refused search pages nothing, got {answer}"
                );
            }
        }
    }
}
