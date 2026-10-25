// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAutomationEventSummaries`.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same [`MultiServiceRouter`] the emulator binary mounts — so a
//! response that is not computed from persisted state fails here.
//!
//! The rule under test: a summaries list reports ONE row per distinct value of
//! the dimension it groups by — `eventStatus` for an automation event, the one
//! value the model's `SummaryDimensionKey` declares — over the events that pass
//! the request's filters and fall inside the half-open
//! `[startDateInclusive, endDateExclusive)` window, in the order the store first
//! holds an event in that status, each row keyed by that value, naming the
//! dimension it is grouped by, covering the queried period and totalling only
//! its own events. A status nothing matched is no row, so a request nothing
//! matched reports no rows.
//!
//! The events are SEEDED into the shared store through the helpers
//! [`crate::automation_events`] owns, because an event created through
//! `StartAutomationEvent` needs a stored recommended action, which only Compute
//! Optimizer's analysis produces.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    EstimatedMonthlySavings, AUTOMATION_EVENT_FILTER_NAME_VALUES, SUMMARY_DIMENSION_KEY_VALUES,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    StoredEvent, EVENT_RETRIEVAL_WINDOW, INVALID_TIME_RANGE_MESSAGE,
    SUMMARY_DIMENSION_EVENT_STATUS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_events::{emulator, now_secs, seed, stored_event};
use crate::automation_rule_lifecycle::{
    call, call_ok, error_code, is_next_token, member, string_member,
};

/// The operation under test.
const OPERATION: &str = "ListAutomationEventSummaries";

/// How close two floating-point savings figures must be to count as the same
/// total.
const SAVINGS_EPSILON: f64 = 1e-9;

/// One `EstimatedMonthlySavings`, as Compute Optimizer's analysis records it on
/// an event.
fn savings(currency: &str, before: f64, after: f64, mode: &str) -> EstimatedMonthlySavings {
    EstimatedMonthlySavings {
        currency: currency.to_string(),
        before_discount_savings: before,
        after_discount_savings: after,
        savings_estimation_mode: mode.to_string(),
    }
}

/// One stored event, built on the fixture builder [`crate::automation_events`]
/// owns with the member these cases add overridden: the estimate its status
/// group sums. An event Compute Optimizer has not estimated savings for is
/// written by passing `None`.
fn event(
    index: usize,
    account_id: &str,
    event_type: &str,
    event_status: &str,
    created_secs: i64,
    estimate: Option<EstimatedMonthlySavings>,
) -> StoredEvent {
    StoredEvent {
        estimated_monthly_savings: estimate,
        ..stored_event(index, account_id, event_type, event_status, created_secs)
    }
}

/// One summary as a response reports it.
#[derive(Debug, Clone)]
struct Row {
    key: String,
    dimensions: Vec<(String, String)>,
    count: i64,
    /// The group's aggregated estimate as `(currency, before, after, mode)`, or
    /// `None` where the response carries none.
    savings: Option<(String, f64, f64, String)>,
}

/// The summaries one response reports, in response order. A response that
/// carries no summaries at all reads as the empty list, whether it renders the
/// member as `[]` or leaves it out.
fn reported(body: &Value) -> Vec<Row> {
    let listed = member(body, "automationEventSummaries");
    if listed.is_null() {
        return Vec::new();
    }
    listed
        .as_array()
        .unwrap_or_else(|| panic!("automationEventSummaries must be a list, got {body}"))
        .iter()
        .map(|summary| {
            let total = member(summary, "total");
            let estimate = member(total, "estimatedMonthlySavings");
            Row {
                key: string_member(summary, "key"),
                dimensions: member(summary, "dimensions")
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|dimension| {
                        (
                            string_member(dimension, "key"),
                            string_member(dimension, "value"),
                        )
                    })
                    .collect(),
                count: member(total, "automationEventCount")
                    .as_i64()
                    .unwrap_or_default(),
                savings: if estimate.is_null() {
                    None
                } else {
                    Some((
                        string_member(estimate, "currency"),
                        member(estimate, "beforeDiscountSavings")
                            .as_f64()
                            .unwrap_or_default(),
                        member(estimate, "afterDiscountSavings")
                            .as_f64()
                            .unwrap_or_default(),
                        string_member(estimate, "savingsEstimationMode"),
                    ))
                },
            }
        })
        .collect()
}

/// One expected group of a summary, as `(eventStatus, count)`.
type Group = (String, i64);

/// One row of the filter table: what a failing assertion calls it, the filters
/// the row sends, and the groups they must leave.
type FilterCase = (&'static str, Value, Vec<Group>);

/// One row of the window table: what a failing assertion calls it, the bounds
/// the row sends (absent when `None`), and the groups the window must leave.
type WindowCase = (&'static str, Option<String>, Option<String>, Vec<Group>);

/// The `(key, count)` of every summary a response reports, in response order:
/// what a case about which groups exist asserts on.
fn keyed(body: &Value) -> Vec<Group> {
    reported(body)
        .into_iter()
        .map(|row| (row.key, row.count))
        .collect()
}

/// Call the operation, requiring the modelled `InvalidParameterValueException`
/// (HTTP 400), and hand the body back so a case can read its message.
async fn refused(router: &MultiServiceRouter, body: &Value, case: &str) -> Value {
    let (status, parsed) = call(router, OPERATION, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} and {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "InvalidParameterValueException",
        "case {case:?}: expected InvalidParameterValueException, got {parsed}"
    );
    parsed
}

/// `instant` rendered the way this service's `@timestampFormat`-less timestamps
/// are, RFC 3339 in UTC with a `Z` suffix.
fn z_form(instant: i64) -> String {
    DateTime::from_secs(instant)
        .fmt(Format::DateTime)
        .expect("a renderable instant")
}

/// The CIVIL part of `instant`'s rendering: the `Z` suffix removed, so a case
/// can write it with a numeric UTC offset instead.
fn civil(instant: i64) -> String {
    z_form(instant).trim_end_matches('Z').to_string()
}

/// `instant`, written as the local time of a zone whose offset from UTC is
/// `offset_secs` and whose suffix is `suffix`. The reader hands such text to the
/// library exactly as written and the library reads UTC, so a bound in this form
/// names no instant and is refused rather than read as some other instant.
fn offset_form(instant: i64, offset_secs: i64, suffix: &str) -> String {
    format!("{}{suffix}", civil(instant + offset_secs))
}

/// The calendar DATE `instant` falls on, with no time of day.
fn date_only(instant: i64) -> String {
    z_form(instant).chars().take(10).collect()
}

/// A request body naming only the window bounds a case sends.
fn window(start: Option<&str>, end: Option<&str>) -> Value {
    let mut body = json!({});
    if let Some(start) = start {
        body["startDateInclusive"] = json!(start);
    }
    if let Some(end) = end {
        body["endDateExclusive"] = json!(end);
    }
    body
}

/// Assert one reported row against what the rule requires of it.
fn assert_row(row: &Row, expected: &Row, case: &str) {
    assert_eq!(
        row.key, expected.key,
        "case {case:?}: the row's key must be the group's eventStatus, got {row:?}"
    );
    assert_eq!(
        row.dimensions, expected.dimensions,
        "case {case:?}: the row must name the dimension it is grouped by, got {row:?}"
    );
    assert_eq!(
        row.count, expected.count,
        "case {case:?}: the row must count only its own events, got {row:?}"
    );
    match (&row.savings, &expected.savings) {
        (None, None) => {}
        (Some(seen), Some(want)) => {
            assert_eq!(
                (&seen.0, &seen.3),
                (&want.0, &want.3),
                "case {case:?}: the total must name the currency and mode its estimates agree \
                 on, got {row:?}"
            );
            assert!(
                (seen.1 - want.1).abs() < SAVINGS_EPSILON
                    && (seen.2 - want.2).abs() < SAVINGS_EPSILON,
                "case {case:?}: expected the sums {:?}, got {row:?}",
                (want.1, want.2)
            );
        }
        _ => panic!(
            "case {case:?}: expected the estimate {:?}, got {row:?}",
            expected.savings
        ),
    }
}

/// An expected row, written as a call so a table stays readable.
fn expect(key: &str, count: i64, savings: Option<(&str, f64, f64, &str)>) -> Row {
    Row {
        key: key.to_string(),
        dimensions: vec![(
            SUMMARY_DIMENSION_EVENT_STATUS.to_string(),
            key.to_string(),
        )],
        count,
        savings: savings.map(|(currency, before, after, mode)| {
            (currency.to_string(), before, after, mode.to_string())
        }),
    }
}

/// Eight events spanning four statuses, two currencies, both savings modes and
/// events carrying no estimate at all, created one hour apart in store order.
fn mixed_events(base: i64) -> Vec<StoredEvent> {
    let at = |hours_ago: i64| base - hours_ago * 3600;
    vec![
        event(
            0,
            "111111111111",
            "UpgradeEbsVolumeType",
            "Complete",
            at(8),
            Some(savings("USD", 10.0, 8.0, "BeforeDiscount")),
        ),
        event(
            1,
            "222222222222",
            "UpgradeEbsVolumeType",
            "Failed",
            at(7),
            None,
        ),
        event(
            2,
            "111111111111",
            "SnapshotAndDeleteUnattachedEbsVolume",
            "Complete",
            at(6),
            Some(savings("USD", 5.0, 4.0, "BeforeDiscount")),
        ),
        event(
            3,
            "222222222222",
            "UpgradeEbsVolumeType",
            "InProgress",
            at(5),
            Some(savings("EUR", 7.0, 6.0, "AfterDiscount")),
        ),
        event(
            4,
            "111111111111",
            "UpgradeEbsVolumeType",
            "RollbackComplete",
            at(4),
            Some(savings("USD", 1.0, 1.0, "AfterDiscount")),
        ),
        event(
            5,
            "333333333333",
            "SnapshotAndDeleteUnattachedEbsVolume",
            "Complete",
            at(3),
            None,
        ),
        event(
            6,
            "333333333333",
            "UpgradeEbsVolumeType",
            "Failed",
            at(2),
            None,
        ),
        event(
            7,
            "111111111111",
            "SnapshotAndDeleteUnattachedEbsVolume",
            "RollbackComplete",
            at(1),
            Some(savings("EUR", 2.0, 2.0, "AfterDiscount")),
        ),
    ]
}

/// Four events, each in a DIFFERENT status, created one hour apart in store
/// order: a window or filter case reads the statuses of the events it selected
/// straight off the rows.
fn one_per_status(base: i64) -> Vec<StoredEvent> {
    vec![
        event(
            10,
            "111111111111",
            "SnapshotAndDeleteUnattachedEbsVolume",
            "Complete",
            base - 4 * 3600,
            Some(savings("USD", 4.0, 3.0, "BeforeDiscount")),
        ),
        event(
            11,
            "222222222222",
            "UpgradeEbsVolumeType",
            "Failed",
            base - 3 * 3600,
            None,
        ),
        event(
            12,
            "111111111111",
            "UpgradeEbsVolumeType",
            "RollbackComplete",
            base - 2 * 3600,
            Some(savings("USD", 2.0, 2.0, "AfterDiscount")),
        ),
        event(
            13,
            "333333333333",
            "SnapshotAndDeleteUnattachedEbsVolume",
            "InProgress",
            base - 3600,
            None,
        ),
    ]
}

// ---------------------------------------------------------------------------
// One row per group, computed from what the store holds
// ---------------------------------------------------------------------------

/// An unfiltered summary reports one row per `eventStatus` the stored events
/// hold, in the order the store first holds an event in it, each row naming the
/// dimension, counting only its own events and totalling only their estimates.
#[tokio::test]
async fn summaries_group_the_stored_events_by_event_status() {
    let (router, engine) = emulator();
    let base = now_secs();
    seed(&engine, &mixed_events(base));

    let case = "unfiltered";
    // The dimension the rows are grouped by is one the model declares
    // (`smithy_query(query="shape",shape="SummaryDimensionKey")`), so the
    // generated value set is what the service's constant is checked against.
    assert!(
        SUMMARY_DIMENSION_KEY_VALUES.contains(&SUMMARY_DIMENSION_EVENT_STATUS),
        "case {case:?}: the dimension grouped by must be a modelled SummaryDimensionKey, {:?} \
         is not one of {SUMMARY_DIMENSION_KEY_VALUES:?}",
        SUMMARY_DIMENSION_EVENT_STATUS
    );
    let body = call_ok(&router, OPERATION, &json!({}), case).await;
    let rows = reported(&body);

    let expected = [
        // Two USD before-discount estimates and one event with none: the count
        // is three and the sums are of the estimates that exist.
        expect("Complete", 3, Some(("USD", 15.0, 12.0, "BeforeDiscount"))),
        // A group whose events carry NO estimate reports no savings figure.
        expect("Failed", 2, None),
        expect("InProgress", 1, Some(("EUR", 7.0, 6.0, "AfterDiscount"))),
        // Estimates in two currencies: the amounts still sum, and the total
        // names no currency because they do not agree on one.
        expect("RollbackComplete", 2, Some(("", 3.0, 3.0, "AfterDiscount"))),
    ];
    assert_eq!(
        rows.len(),
        expected.len(),
        "case {case:?}: expected one row per status the store holds, got {body}"
    );
    for (row, want) in rows.iter().zip(expected.iter()) {
        assert_row(row, want, case);
    }
    assert!(
        member(&body, "nextToken").is_null(),
        "case {case:?}: a single page that holds every group carries no token, got {body}"
    );

    // A reporting operation must not change what a later caller sees: the same
    // request answers the same rows, and the events are all still listed.
    let again = call_ok(&router, OPERATION, &json!({}), "repeated").await;
    assert_eq!(
        keyed(&again),
        keyed(&body),
        "case {:?}: summarising must not change what a later caller sees, got {again}",
        "repeated"
    );
    let listed = call_ok(&router, "ListAutomationEvents", &json!({}), "repeated").await;
    assert_eq!(
        member(&listed, "automationEvents")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        8,
        "case {:?}: summarising must consume no event, got {listed}",
        "repeated"
    );
}

/// A store holding no event at all is summarised as no rows — not as a refusal,
/// and not as a row carrying a zero total for a status nothing is in.
#[tokio::test]
async fn an_empty_store_is_summarised_as_no_rows() {
    let (router, _engine) = emulator();
    let body = call_ok(&router, OPERATION, &json!({}), "empty-store").await;
    assert_eq!(
        keyed(&body),
        Vec::new(),
        "case {:?}: an empty store must answer no summaries, got {body}",
        "empty-store"
    );
}

/// The window every row reports: it starts at the retrieval floor when the
/// caller named no `startDateInclusive`, and ends at the instant the summary was
/// computed when it named no `endDateExclusive`.
#[tokio::test]
async fn an_unbounded_query_reports_the_window_it_summarised_over() {
    let (router, engine) = emulator();
    let base = now_secs();
    seed(&engine, &one_per_status(base));

    let case = "no-bounds";
    let body = call_ok(&router, OPERATION, &json!({}), case).await;
    let window_floor = base - i64::try_from(EVENT_RETRIEVAL_WINDOW.as_secs()).expect("a window");
    for summary in member(&body, "automationEventSummaries")
        .as_array()
        .unwrap_or_else(|| panic!("automationEventSummaries must be a list, got {body}"))
    {
        let period = member(summary, "timePeriod");
        let start = member(period, "startTimeInclusive")
            .as_f64()
            .unwrap_or_default();
        let end = member(period, "endTimeExclusive").as_f64().unwrap_or_default();
        assert!(
            (start - window_floor as f64).abs() <= 60.0,
            "case {case:?}: an unbounded start reports the retrieval floor, got {summary}"
        );
        assert!(
            end >= base as f64 && end <= (base + 60) as f64,
            "case {case:?}: an unbounded end reports the instant summarised at, got {summary}"
        );
        assert!(
            start < end,
            "case {case:?}: the period must be a window that can hold an event, got {summary}"
        );
    }

    // A query that names both bounds reports exactly those bounds.
    let start = base - 3 * 3600;
    let end = base - 3600;
    let bounded = call_ok(
        &router,
        OPERATION,
        &window(Some(&z_form(start)), Some(&z_form(end))),
        "both-bounds",
    )
    .await;
    for summary in member(&bounded, "automationEventSummaries")
        .as_array()
        .unwrap_or_else(|| panic!("automationEventSummaries must be a list, got {bounded}"))
    {
        let period = member(summary, "timePeriod");
        assert_eq!(
            (
                member(period, "startTimeInclusive").as_f64(),
                member(period, "endTimeExclusive").as_f64()
            ),
            (Some(start as f64), Some(end as f64)),
            "case {:?}: the period must be the window the caller named, got {summary}",
            "both-bounds"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// A filter narrows WHAT IS SUMMARISED, exactly as it narrows the listing: each
/// `AutomationEventFilterName` selects on the field it names, a filter's values
/// are ORed, several filters are ANDed, and a status no selected event is in is
/// no row at all.
#[tokio::test]
async fn filters_narrow_what_is_summarised() {
    let (router, engine) = emulator();
    let base = now_secs();
    seed(&engine, &mixed_events(base));

    // (case, filters, the rows the filter must leave) — one row per value of the
    // model's `AutomationEventFilterName`
    // (`smithy_query(query="shape",shape="AutomationEventFilterName")`), plus the
    // OR, AND and no-match rows.
    let cases: Vec<FilterCase> = vec![
        (
            "AccountId",
            json!([{"name": "AccountId", "values": ["111111111111"]}]),
            vec![
                ("Complete".to_string(), 2),
                ("RollbackComplete".to_string(), 2),
            ],
        ),
        (
            "ResourceType",
            json!([{"name": "ResourceType", "values": ["EbsVolume"]}]),
            vec![
                ("Complete".to_string(), 3),
                ("Failed".to_string(), 2),
                ("InProgress".to_string(), 1),
                ("RollbackComplete".to_string(), 2),
            ],
        ),
        (
            "EventType",
            json!([{"name": "EventType", "values": ["UpgradeEbsVolumeType"]}]),
            vec![
                ("Complete".to_string(), 1),
                ("Failed".to_string(), 2),
                ("InProgress".to_string(), 1),
                ("RollbackComplete".to_string(), 1),
            ],
        ),
        (
            "EventStatus",
            json!([{"name": "EventStatus", "values": ["Complete"]}]),
            vec![("Complete".to_string(), 3)],
        ),
        (
            "values-are-ored",
            json!([{"name": "AccountId", "values": ["222222222222", "333333333333"]}]),
            vec![
                ("Failed".to_string(), 2),
                ("InProgress".to_string(), 1),
                ("Complete".to_string(), 1),
            ],
        ),
        (
            "filters-are-anded",
            json!([
                {"name": "AccountId", "values": ["111111111111"]},
                {"name": "EventType", "values": ["UpgradeEbsVolumeType"]}
            ]),
            vec![
                ("Complete".to_string(), 1),
                ("RollbackComplete".to_string(), 1),
            ],
        ),
        (
            "filters-are-anded-to-nothing",
            json!([
                {"name": "AccountId", "values": ["333333333333"]},
                {"name": "EventStatus", "values": ["InProgress"]}
            ]),
            vec![],
        ),
        (
            "no-match",
            json!([{"name": "AccountId", "values": ["999999999999"]}]),
            vec![],
        ),
    ];

    for (case, filters, expected) in &cases {
        let body = call_ok(&router, OPERATION, &json!({"filters": filters}), case).await;
        assert_eq!(
            &keyed(&body),
            expected,
            "case {case:?}: the filter must leave exactly those groups, got {body}"
        );
    }

    // A filter name outside the modelled value set is refused, not ignored, and
    // the refusal names the member and enumerates the set without echoing the
    // value sent.
    let body = refused(
        &router,
        &json!({"filters": [{"name": "EventId", "values": ["evt0000000000000"]}]}),
        "unknown-filter-name",
    )
    .await;
    let message = string_member(&body, "message");
    assert!(
        message.contains("filters")
            && message.contains("name")
            && AUTOMATION_EVENT_FILTER_NAME_VALUES
                .iter()
                .all(|value| message.contains(value)),
        "case {:?}: the refusal must name the member and the modelled value set, got {body}",
        "unknown-filter-name"
    );
}

// ---------------------------------------------------------------------------
// The queried date window
// ---------------------------------------------------------------------------

/// `[startDateInclusive, endDateExclusive)` is half-open: an event created at
/// the start bound is summarised, one created at the end bound is not, and an
/// absent bound leaves that side unbounded.
#[tokio::test]
async fn the_queried_date_window_is_half_open() {
    let (router, engine) = emulator();
    let base = now_secs();
    let events = one_per_status(base);
    seed(&engine, &events);

    let status = |index: usize| (events[index].event_status.clone(), 1_i64);
    let at = |index: usize| events[index].created_timestamp.secs();

    // (case, start, end, the groups the window must leave) — one row per way a
    // half-open window can fall against the stored events.
    let cases: Vec<WindowCase> = vec![
        (
            "both-bounds-absent",
            None,
            None,
            vec![status(0), status(1), status(2), status(3)],
        ),
        (
            "start-is-inclusive",
            Some(z_form(at(1))),
            None,
            vec![status(1), status(2), status(3)],
        ),
        (
            "end-is-exclusive",
            None,
            Some(z_form(at(2))),
            vec![status(0), status(1)],
        ),
        (
            "both-bounds",
            Some(z_form(at(1))),
            Some(z_form(at(3))),
            vec![status(1), status(2)],
        ),
        (
            "one-second-inside-the-start",
            Some(z_form(at(1) + 1)),
            Some(z_form(at(3))),
            vec![status(2)],
        ),
        (
            "window-after-every-event",
            Some(z_form(at(3) + 1)),
            None,
            vec![],
        ),
        ("window-before-every-event", None, Some(z_form(at(0))), vec![]),
        (
            "a-date-covering-every-event",
            Some(date_only(base - 30 * 86_400)),
            Some(date_only(base + 2 * 86_400)),
            vec![status(0), status(1), status(2), status(3)],
        ),
        (
            "a-date-before-every-event",
            None,
            Some(date_only(base - 10 * 86_400)),
            vec![],
        ),
    ];

    for (case, start, end, expected) in &cases {
        let body = call_ok(
            &router,
            OPERATION,
            &window(start.as_deref(), end.as_deref()),
            case,
        )
        .await;
        assert_eq!(
            &keyed(&body),
            expected,
            "case {case:?}: the window must leave exactly those groups, got {body}"
        );
    }
}

/// The SAME instant, written in any of the forms this member is read in, selects
/// the same events: the bound is the instant the text names, not the text.
#[tokio::test]
async fn equivalent_bound_forms_select_the_same_events() {
    let (router, engine) = emulator();
    let base = now_secs();
    let events = one_per_status(base);
    seed(&engine, &events);

    let bound = events[1].created_timestamp.secs();
    let expected: Vec<Group> = events[1..]
        .iter()
        .map(|event| (event.event_status.clone(), 1_i64))
        .collect();

    // (case, the same instant in one of the forms the member is read in): the
    // UTC `Z` form the service renders every instant in, that form carrying a
    // zero fractional second, both of them with the UTC designator left off
    // (read as UTC), and the seconds since the epoch this protocol renders
    // every modelled timestamp as.
    let forms: Vec<(&str, String)> = vec![
        ("Z", z_form(bound)),
        ("no-zone", civil(bound)),
        ("fractional-second", format!("{}.000Z", civil(bound))),
        ("fractional-second-no-zone", format!("{}.000", civil(bound))),
        ("epoch-seconds", bound.to_string()),
    ];

    for (case, start) in &forms {
        let body = call_ok(&router, OPERATION, &window(Some(start), None), case).await;
        assert_eq!(
            &keyed(&body),
            &expected,
            "case {case:?}: {start:?} names the same instant, so it must select the same \
             events, got {body}"
        );
    }
}

/// Text that names no instant is refused with the modelled
/// `InvalidParameterValueException`, naming the member it was sent for, rather
/// than ignored — which would summarise a window the caller did not ask for.
#[tokio::test]
async fn a_bound_that_names_no_instant_is_refused() {
    let (router, engine) = emulator();
    let base = now_secs();
    seed(&engine, &one_per_status(base));

    // Text that names no instant here: empty, prose, a non-ISO ordering, a
    // month and day past the calendar, an hour past it, and a date and time
    // written as the local time of a zone other than UTC in each offset form
    // RFC 3339 admits — handed to the library as written, which reads UTC.
    let unreadable = [
        ("empty", String::new()),
        ("prose", "yesterday".to_string()),
        ("day-first", "03/02/2026".to_string()),
        ("past-the-calendar", "2026-13-45".to_string()),
        (
            "hour-past-the-calendar",
            format!("{}T25:00:00Z", date_only(base)),
        ),
        ("zero-offset", offset_form(base, 0, "+00:00")),
        ("negative-offset", offset_form(base, -7_200, "-02:00")),
        ("offset-with-minutes", offset_form(base, 19_800, "+05:30")),
        ("hour-only-offset", offset_form(base, -3_600, "-01")),
    ];

    for (case, value) in unreadable {
        let value = value.as_str();
        for member_name in ["startDateInclusive", "endDateExclusive"] {
            let case = format!("{member_name}-{case}");
            let body = refused(&router, &json!({member_name: value}), &case).await;
            let message = string_member(&body, "message");
            assert!(
                message.contains(member_name) && (value.is_empty() || !message.contains(value)),
                "case {case:?}: the refusal must name the member and echo no value, got {body}"
            );
        }
    }

    // The readable side of the same rule: a date, an instant, and the epoch
    // seconds form.
    for (case, value) in [
        ("a-date", date_only(base)),
        ("an-instant", z_form(base - 86_400)),
        ("epoch-seconds", (base - 86_400).to_string()),
    ] {
        call_ok(
            &router,
            OPERATION,
            &json!({"startDateInclusive": value}),
            case,
        )
        .await;
    }
}

/// A window whose end does not come after its start holds nothing, and is
/// refused rather than answered with an empty summary. A window with one bound
/// is always valid.
#[tokio::test]
async fn a_window_that_cannot_hold_anything_is_refused() {
    let (router, engine) = emulator();
    let base = now_secs();
    seed(&engine, &one_per_status(base));

    // (case, start, end)
    let refusals: Vec<(&str, String, String)> = vec![
        (
            "end-one-second-before-start",
            z_form(base - 3600),
            z_form(base - 3601),
        ),
        ("end-equals-start", z_form(base), z_form(base)),
        (
            "end-long-before-start",
            z_form(base),
            z_form(base - 30 * 86_400),
        ),
        (
            "a-date-before-a-date",
            date_only(base),
            date_only(base - 86_400),
        ),
    ];

    for (case, start, end) in &refusals {
        let body = refused(&router, &window(Some(start), Some(end)), case).await;
        assert_eq!(
            string_member(&body, "message"),
            INVALID_TIME_RANGE_MESSAGE,
            "case {case:?}: the refusal must say why the window cannot hold anything, got {body}"
        );
    }

    // The accepted side: a window with room, and either bound on its own.
    for (case, body) in [
        (
            "end-one-second-after-start",
            window(Some(&z_form(base - 3600)), Some(&z_form(base - 3599))),
        ),
        ("start-only", window(Some(&z_form(base - 86_400)), None)),
        ("end-only", window(None, Some(&z_form(base + 60)))),
        ("neither-bound", window(None, None)),
    ] {
        call_ok(&router, OPERATION, &body, case).await;
    }
}

/// An event created before the retrieval window is never summarised, however far
/// back the caller's `startDateInclusive` points: only events created within the
/// past year are included in the summary.
#[tokio::test]
async fn events_outside_the_retrieval_window_are_never_summarised() {
    let (router, engine) = emulator();
    let base = now_secs();
    let span = i64::try_from(EVENT_RETRIEVAL_WINDOW.as_secs()).expect("a representable window");

    // Just inside the far edge of the window, and two ways of being outside it.
    let edge = event(
        20,
        "111111111111",
        "UpgradeEbsVolumeType",
        "Complete",
        base - span + 60,
        None,
    );
    let outside = event(
        21,
        "111111111111",
        "UpgradeEbsVolumeType",
        "Failed",
        base - span - 60,
        None,
    );
    let long_outside = event(
        22,
        "111111111111",
        "UpgradeEbsVolumeType",
        "RollbackComplete",
        base - 5 * span,
        None,
    );
    seed(
        &engine,
        &[outside.clone(), long_outside.clone(), edge.clone()],
    );

    // (case, request) — every way of asking for everything still stops at the
    // retrieval window, so the statuses only the unreachable events are in are
    // no rows.
    let cases: Vec<(&str, Value)> = vec![
        ("no-bounds", json!({})),
        (
            "a-start-at-the-epoch",
            window(Some("1970-01-01T00:00:00Z"), None),
        ),
        (
            "a-start-well-before-the-window",
            window(Some(&z_form(base - 10 * span)), Some(&z_form(base + 60))),
        ),
    ];
    for (case, body) in &cases {
        let summarised = call_ok(&router, OPERATION, body, case).await;
        assert_eq!(
            keyed(&summarised),
            vec![(edge.event_status.clone(), 1_i64)],
            "case {case:?}: only events created inside the retrieval window are summarised, \
             got {summarised}"
        );
    }

    // A window entirely outside the retrieval window summarises nothing.
    let outside_window = call_ok(
        &router,
        OPERATION,
        &window(
            Some(&z_form(base - 4 * span)),
            Some(&z_form(base - 2 * span)),
        ),
        "window-entirely-outside",
    )
    .await;
    assert_eq!(
        keyed(&outside_window),
        Vec::new(),
        "case {:?}: a window outside the retrieval window holds nothing, got {outside_window}",
        "window-entirely-outside"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Paging walks the whole sequence of groups exactly once, whatever the page
/// size, and a token is only honoured for the query it was issued for.
#[tokio::test]
async fn paging_walks_every_group_exactly_once() {
    let (router, engine) = emulator();
    let base = now_secs();
    // Seven distinct `EventStatus` values, so the summary has seven groups —
    // the value set is `EventStatus`
    // (`smithy_query(query="shape",shape="EventStatus")`).
    let statuses = [
        "Complete",
        "Failed",
        "InProgress",
        "Ready",
        "RollbackComplete",
        "RollbackFailed",
        "RollbackInProgress",
    ];
    let events: Vec<StoredEvent> = statuses
        .iter()
        .enumerate()
        .map(|(index, status)| {
            event(
                30 + index,
                "111111111111",
                "UpgradeEbsVolumeType",
                status,
                base - 7200 + i64::try_from(index).expect("a small index") * 60,
                None,
            )
        })
        .collect();
    seed(&engine, &events);
    let all: Vec<Group> = statuses
        .iter()
        .map(|status| ((*status).to_string(), 1_i64))
        .collect();

    // Page sizes spanning the model's `maxResults` range (1..=100): one at a
    // time, sizes that do not divide the sequence, the exact length, and more
    // than the summary holds.
    for page_size in [1_usize, 2, 3, 7, 100] {
        let case = format!("page-size-{page_size}");
        let mut walked: Vec<Group> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut body = json!({"maxResults": page_size});
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let listed = call_ok(&router, OPERATION, &body, &case).await;
            let page = keyed(&listed);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must not exceed maxResults, got {} rows",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: the walk must terminate, it has taken {pages} pages"
            );
            match member(&listed, "nextToken").as_str() {
                Some(next) => {
                    assert!(
                        is_next_token(next),
                        "case {case:?}: a token must match the modelled NextToken pattern, got \
                         {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: the walk must report every group exactly once, in store order"
        );
        assert_eq!(
            pages,
            all.len().div_ceil(page_size.min(all.len().max(1))),
            "case {case:?}: no page beyond the last, and no token on it"
        );
    }

    // A page size the caller did not send: the default page holds every group.
    let listed = call_ok(&router, OPERATION, &json!({}), "default-page-size").await;
    assert_eq!(
        keyed(&listed),
        all,
        "case {:?}: the default page must hold every group, got {listed}",
        "default-page-size"
    );

    // A token this service did not issue, and one issued for a different query,
    // are both refused rather than answered with a page of something else.
    let first = call_ok(&router, OPERATION, &json!({"maxResults": 1}), "token-reuse").await;
    let token = string_member(&first, "nextToken");
    for (case, body) in [
        (
            "token-with-different-filters",
            json!({"maxResults": 1, "nextToken": token.clone(), "filters": [
                {"name": "EventStatus", "values": ["Complete"]}
            ]}),
        ),
        (
            "token-with-a-different-window",
            json!({"maxResults": 1, "nextToken": token.clone(),
                   "startDateInclusive": z_form(base - 3600)}),
        ),
        (
            "token-from-another-operation",
            json!({"nextToken": "notatokenweissued"}),
        ),
    ] {
        refused(&router, &body, case).await;
    }
}
