// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the instance event window slice: `CreateInstanceEventWindow` and
//! `DescribeInstanceEventWindows`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateInstanceEventWindow` MINTS A WINDOW FROM THE REQUEST and
//!    `DescribeInstanceEventWindows` REPORTS THE STORE: every member the request sets
//!    round-trips, and the members the service owns (the id, the state) are derived rather
//!    than taken;
//! 2. a window is `creating` and the CLOCK, not a caller, moves it to `active`;
//! 3. a window says when it opens with EXACTLY ONE of `CronExpression` and `TimeRanges`,
//!    and what it says must satisfy the constraints the model states for it: five cron
//!    fields of which only the hour and the day of the week carry values, each range at
//!    least two hours and the ranges at least four hours between them;
//! 4. the describe narrows by the filters it documents and pages in id order.
//!
//! Each table row is a distinct input along what the rule generalises over — which form the
//! schedule takes, how many ranges, which days (including a range that runs across the end
//! of the week), each published bound and one past it, which filter, how many filters, which
//! page size — so no row is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    MAX_EVENT_WINDOW_CRON_BYTES, MAX_EVENT_WINDOW_TIME_RANGES, MAX_INSTANCE_EVENT_WINDOWS,
    MIN_EVENT_WINDOW_RANGE_HOURS, MIN_EVENT_WINDOW_TOTAL_HOURS, TRANSITION_DELAY,
    WEEK_DAYS_IN_ORDER,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. An event window carries no ARN and no zone, so
/// nothing in these assertions is derived from it — it is only the scope the ec2Query
/// envelope is selected by.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a refusal: `(case, the parameters, the error code)`.
type RefusalRow = (&'static str, Params, &'static str);

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateInstanceEventWindow", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeInstanceEventWindows", params).await
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// An `InstanceEventWindowId` is `iew-` followed by 8 or 17 lowercase hex digits.
fn is_event_window_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("iew-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17) && suffix.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The modelled `WeekDay` member at `index` of the calendar week, from the service's own
/// calendar-ordered table.
fn week_day(index: usize) -> &'static str {
    WEEK_DAYS_IN_ORDER[index % WEEK_DAYS_IN_ORDER.len()].0
}

/// One `TimeRange.N.*` group of form fields, as ec2Query flattens the `TimeRanges` list
/// (its member is named `TimeRange` on the wire).
fn time_range(index: usize, start: (usize, i64), end: (usize, i64)) -> Params {
    vec![
        p(
            &format!("TimeRange.{index}.StartWeekDay"),
            week_day(start.0),
        ),
        p(&format!("TimeRange.{index}.StartHour"), start.1.to_string()),
        p(&format!("TimeRange.{index}.EndWeekDay"), week_day(end.0)),
        p(&format!("TimeRange.{index}.EndHour"), end.1.to_string()),
    ]
}

/// The whole `TimeRanges` list, each entry `(start day, start hour, end day, end hour)`.
fn time_ranges(ranges: &[(usize, i64, usize, i64)]) -> Params {
    ranges
        .iter()
        .enumerate()
        .flat_map(|(index, (start_day, start_hour, end_day, end_hour))| {
            time_range(index + 1, (*start_day, *start_hour), (*end_day, *end_hour))
        })
        .collect()
}

/// A `TagSpecifications` entry naming `resource_type` and carrying `tags`.
fn tag_specification(resource_type: Option<&str>, tags: &[(&str, &str)]) -> Params {
    let mut params: Params = resource_type
        .map(|resource_type| p("TagSpecification.1.ResourceType", resource_type))
        .into_iter()
        .collect();
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
        params.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    params
}

/// The tags of an event window, for the one resource `CreateInstanceEventWindow` tags.
fn window_tags(tags: &[(&str, &str)]) -> Params {
    tag_specification(Some("instance-event-window"), tags)
}

/// Create one window and answer its id, failing the case if the create was refused.
async fn create_window(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateInstanceEventWindow, \
         got {status} with body {body}"
    );
    let id = element(&body, "instanceEventWindowId").unwrap_or_default();
    assert!(
        is_event_window_id(&id),
        "case {case:?}: expected a minted event window id, got {id:?} in body {body}"
    );
    id
}

/// The ids `DescribeInstanceEventWindows` reports for `params`, in the order it reported
/// them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeInstanceEventWindows, \
         got {status} with body {body}"
    );
    elements(&body, "instanceEventWindowId")
}

/// One row of the round-trip table: a window defined one way, and what the service must
/// report back for it.
struct WindowCase {
    case: &'static str,
    /// "The name of the event window" (`CreateInstanceEventWindowRequest$Name`), or `None`
    /// for a window created without one.
    name: Option<&'static str>,
    /// The cron expression the window opens on, for a window defined by one.
    cron_expression: Option<&'static str>,
    /// The time ranges the window opens in, for a window defined by them.
    ranges: Vec<(usize, i64, usize, i64)>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The windows the round-trip table creates. They differ along every form a schedule takes
/// — a single hour range, several hour ranges, named and numbered days of the week, one
/// time range, several, and one that runs across the end of the week — and along whether
/// the window is named and tagged at all.
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            case: "one cron hour range, numbered days",
            name: Some("weekly-maintenance"),
            cron_expression: Some("* 0-4 * * 1,5"),
            ranges: Vec::new(),
            tags: vec![("Owner", "platform"), ("Tier", "web")],
        },
        WindowCase {
            case: "several cron hour ranges, named days",
            name: Some("two-windows-a-day"),
            cron_expression: Some("* 2-4,20-23 * * SUN,SAT"),
            ranges: Vec::new(),
            tags: vec![("Owner", "payments")],
        },
        WindowCase {
            case: "cron covering every day of the week",
            name: None,
            cron_expression: Some("* 0-23 * * *"),
            ranges: Vec::new(),
            tags: Vec::new(),
        },
        WindowCase {
            case: "one multi-day time range",
            name: Some("monday-to-wednesday"),
            cron_expression: None,
            ranges: vec![(1, 2, 3, 8)],
            tags: vec![("Owner", "platform")],
        },
        WindowCase {
            case: "two time ranges at the smallest span each",
            name: Some("two-short-ranges"),
            cron_expression: None,
            ranges: vec![(1, 0, 1, 2), (2, 0, 2, 2)],
            tags: Vec::new(),
        },
        WindowCase {
            // `InstanceEventWindowTimeRangeRequest$StartHour` / `$EndHour` target `Hour`,
            // modelled `@range(min: 0, max: 23)`
            // (`smithy_query(query="shape",shape="InstanceEventWindowTimeRangeRequest")`),
            // so this row is a range running from one published bound to the other.
            case: "a time range spanning the published hours of one day",
            name: Some("all-day-monday"),
            cron_expression: None,
            ranges: vec![(1, 0, 1, 23)],
            tags: Vec::new(),
        },
        WindowCase {
            case: "a time range across the end of the week",
            name: Some("weekend-wrap"),
            cron_expression: None,
            ranges: vec![(6, 20, 0, 4)],
            tags: vec![("Owner", "ops"), ("Stage", "prod")],
        },
    ]
}

impl WindowCase {
    /// This case as request parameters.
    fn params(&self) -> Params {
        let mut params: Params = self
            .name
            .map(|name| p("Name", name))
            .into_iter()
            .collect();
        if let Some(cron_expression) = self.cron_expression {
            params.push(p("CronExpression", cron_expression));
        }
        params.extend(time_ranges(&self.ranges));
        if !self.tags.is_empty() {
            params.extend(window_tags(&self.tags));
        }
        params
    }

    /// Assert that `body` reports this window the way the request defined it, whichever
    /// operation answered it.
    fn assert_reported(&self, operation: &str, body: &str) {
        let case = self.case;
        assert_eq!(
            element(body, "name"),
            self.name.map(str::to_string),
            "case {case:?}: {operation} must report the name the request named, \
             got body {body}"
        );
        assert_eq!(
            element(body, "cronExpression"),
            self.cron_expression.map(str::to_string),
            "case {case:?}: {operation} must report the cron expression the request \
             named, got body {body}"
        );
        let reported_ranges: Vec<(String, String, String, String)> = elements(body, "startWeekDay")
            .into_iter()
            .zip(elements(body, "startHour"))
            .zip(elements(body, "endWeekDay").into_iter().zip(elements(body, "endHour")))
            .map(|((start_day, start_hour), (end_day, end_hour))| {
                (start_day, start_hour, end_day, end_hour)
            })
            .collect();
        let expected_ranges: Vec<(String, String, String, String)> = self
            .ranges
            .iter()
            .map(|(start_day, start_hour, end_day, end_hour)| {
                (
                    week_day(*start_day).to_string(),
                    start_hour.to_string(),
                    week_day(*end_day).to_string(),
                    end_hour.to_string(),
                )
            })
            .collect();
        assert_eq!(
            reported_ranges, expected_ranges,
            "case {case:?}: {operation} must report the time ranges the request named, \
             got body {body}"
        );
        let reported_tags: Vec<(String, String)> = elements(body, "key")
            .into_iter()
            .zip(elements(body, "value"))
            .collect();
        let expected_tags: Vec<(String, String)> = self
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: {operation} must report the tags the request named, \
             got body {body}"
        );
    }
}

#[tokio::test]
async fn a_created_window_round_trips_through_the_describe() {
    let router = emulator();
    for case in window_cases() {
        let params = case.params();
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}",
            case = case.case
        );
        let id = element(&body, "instanceEventWindowId").unwrap_or_default();
        assert!(
            is_event_window_id(&id),
            "case {case:?}: the service must mint the id, got {id:?} in body {body}",
            case = case.case
        );
        // "When you create the event window, targets (instance IDs, Dedicated Host IDs, or
        // tags) are not yet associated with it" (`CreateInstanceEventWindow`).
        assert_eq!(
            element(&body, "associationTarget"),
            None,
            "case {case:?}: a fresh window is associated with nothing, got body {body}",
            case = case.case
        );
        case.assert_reported("CreateInstanceEventWindow", &body);

        // The window is reported back by the describe exactly as the create reported it.
        let read_back = indexed("InstanceEventWindowId", &[&id]);
        let (status, body) = describe(&router, &read_back).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the describe, got {status} \
             with body {body}",
            case = case.case
        );
        assert_eq!(
            elements(&body, "instanceEventWindowId"),
            vec![id.clone()],
            "case {case:?}: the describe must report the window the request named, \
             got body {body}",
            case = case.case
        );
        case.assert_reported("DescribeInstanceEventWindows", &body);
    }
}

#[tokio::test]
async fn the_clock_moves_a_created_window_to_active() {
    let router = emulator();
    for case in window_cases() {
        let params = case.params();
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}",
            case = case.case
        );
        // A caller acting straight after the create still sees the transient state.
        assert_eq!(
            element(&body, "state").as_deref(),
            Some("creating"),
            "case {case:?}: a fresh window is 'creating', got body {body}",
            case = case.case
        );
        let id = element(&body, "instanceEventWindowId").unwrap_or_default();
        let read_back = indexed("InstanceEventWindowId", &[&id]);
        let (_, body) = describe(&router, &read_back).await;
        assert_eq!(
            element(&body, "state").as_deref(),
            Some("creating"),
            "case {case:?}: the window is still 'creating' before its move falls due, \
             got body {body}",
            case = case.case
        );

        // Nobody asks for the move: the clock owes it.
        past_the_transition().await;
        let (_, body) = describe(&router, &read_back).await;
        assert_eq!(
            element(&body, "state").as_deref(),
            Some("active"),
            "case {case:?}: the clock must move the window to 'active', got body {body}",
            case = case.case
        );
    }
}

/// A window that says when it opens in neither, or in both, of the two ways, and every way
/// a schedule can be outside what the model documents for it. Each row names the EC2 error
/// code the refusal carries.
fn schedule_refusals() -> Vec<RefusalRow> {
    let min_range = MIN_EVENT_WINDOW_RANGE_HOURS;
    let min_total = MIN_EVENT_WINDOW_TOTAL_HOURS;
    vec![
        (
            "neither a cron expression nor a time range",
            vec![p("Name", "says-nothing")],
            "MissingParameter",
        ),
        (
            "both a cron expression and a time range",
            {
                let mut params = vec![p("CronExpression", "* 0-4 * * 1")];
                params.extend(time_ranges(&[(1, 2, 3, 8)]));
                params
            },
            "InvalidParameterCombination",
        ),
        (
            "a cron expression of four fields",
            vec![p("CronExpression", "* 0-4 * *")],
            "InvalidParameterValue",
        ),
        (
            "a cron expression of six fields",
            vec![p("CronExpression", "* 0-4 * * 1 *")],
            "InvalidParameterValue",
        ),
        (
            "a cron expression whose minute is not a wildcard",
            vec![p("CronExpression", "0 0-4 * * 1")],
            "InvalidParameterValue",
        ),
        (
            "a cron expression whose day of the month is not a wildcard",
            vec![p("CronExpression", "* 0-4 1 * 1")],
            "InvalidParameterValue",
        ),
        (
            "a cron expression whose month is not a wildcard",
            vec![p("CronExpression", "* 0-4 * 6 1")],
            "InvalidParameterValue",
        ),
        (
            "a cron hour that is a single hour rather than a range",
            vec![p("CronExpression", "* 5 * * 1")],
            "InvalidParameterValue",
        ),
        (
            "a cron hour range of one hour, one short of the smallest",
            vec![p(
                "CronExpression",
                format!("* 0-{} * * 1", min_range - 1),
            )],
            "InvalidParameterValue",
        ),
        (
            "cron hour ranges one hour short of the smallest total",
            vec![p(
                "CronExpression",
                format!("* 0-{} * * 1", min_total - 1),
            )],
            "InvalidParameterValue",
        ),
        (
            "a cron hour past the last hour of the day",
            vec![p("CronExpression", "* 0-24 * * 1")],
            "InvalidParameterValue",
        ),
        (
            "a cron hour that is not a number",
            vec![p("CronExpression", "* a-b * * 1")],
            "InvalidParameterValue",
        ),
        (
            "a cron day of the week past the last day",
            vec![p("CronExpression", "* 0-4 * * 7")],
            "InvalidParameterValue",
        ),
        (
            "a cron day of the week that is no day",
            vec![p("CronExpression", "* 0-4 * * FUNDAY")],
            "InvalidParameterValue",
        ),
        (
            "a cron hour range that runs backwards",
            vec![p("CronExpression", "* 20-4 * * 1")],
            "InvalidParameterValue",
        ),
        (
            "a cron expression longer than the service reads",
            vec![p(
                "CronExpression",
                format!("* 0-4 * * {}", "1,".repeat(MAX_EVENT_WINDOW_CRON_BYTES)),
            )],
            "InvalidParameterValue",
        ),
        (
            "a time range one hour short of the smallest span",
            time_ranges(&[(1, 1, 1, 1 + min_range - 1)]),
            "InvalidParameterValue",
        ),
        (
            "a time range that starts and ends in the same hour",
            time_ranges(&[(1, 2, 1, 2)]),
            "InvalidParameterValue",
        ),
        (
            "time ranges one hour short of the smallest total",
            time_ranges(&[(1, 0, 1, min_total - 1)]),
            "InvalidParameterValue",
        ),
        // `Hour` is modelled `@range(min: 0, max: 23)`, so an hour outside the day is
        // refused by the boundary validator before the handler runs — at each end of the
        // published range, and for whichever of the two members carries it.
        (
            "a time range start hour one past the last hour of the day",
            time_ranges(&[(1, 24, 2, 8)]),
            "ValidationError",
        ),
        (
            "a time range end hour one past the last hour of the day",
            time_ranges(&[(1, 2, 2, 24)]),
            "ValidationError",
        ),
        (
            "a time range start hour one before the first hour of the day",
            time_ranges(&[(1, -1, 2, 8)]),
            "ValidationError",
        ),
        (
            "a time range end hour one before the first hour of the day",
            time_ranges(&[(1, 2, 2, -1)]),
            "ValidationError",
        ),
        (
            "a time range with no end hour",
            vec![
                p("TimeRange.1.StartWeekDay", "monday"),
                p("TimeRange.1.StartHour", "2"),
                p("TimeRange.1.EndWeekDay", "wednesday"),
            ],
            "MissingParameter",
        ),
        (
            "a time range with no end week day",
            vec![
                p("TimeRange.1.StartWeekDay", "monday"),
                p("TimeRange.1.StartHour", "2"),
                p("TimeRange.1.EndHour", "8"),
            ],
            "MissingParameter",
        ),
    ]
}

#[tokio::test]
async fn a_schedule_outside_the_documented_constraints_is_refused() {
    let router = emulator();
    for (case, params, code) in schedule_refusals() {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the error code {code}, got body {body}"
        );
    }
    // Nothing above was stored: a refused create writes no window.
    assert!(
        described_ids(&router, "after every refusal", &[])
            .await
            .is_empty(),
        "a refused create must store nothing"
    );
}

#[tokio::test]
async fn a_time_range_week_day_that_names_no_day_is_refused() {
    let router = emulator();
    let params = vec![
        p("TimeRange.1.StartWeekDay", "funday"),
        p("TimeRange.1.StartHour", "2"),
        p("TimeRange.1.EndWeekDay", "wednesday"),
        p("TimeRange.1.EndHour", "8"),
    ];
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 400,
        "a week day the `WeekDay` enum does not name must be refused, \
         got {status} with body {body}"
    );
    // `InstanceEventWindowTimeRangeRequest$StartWeekDay` targets the modelled `WeekDay`
    // enum, so the boundary validator refuses a word outside it before the handler runs,
    // naming the member and the enum's members.
    assert_eq!(
        error_code(&body),
        "ValidationError",
        "expected the boundary's refusal of the enum member, got body {body}"
    );
    let message = unescaped(&element(&body, "Message").unwrap_or_default());
    assert!(
        message.contains("startWeekDay") && message.contains("funday"),
        "the refusal must name the member and the value, got message {message:?}"
    );
    // And nothing was stored by the refused create.
    assert!(
        described_ids(&router, "after the refusal", &[])
            .await
            .is_empty(),
        "a refused create must store nothing"
    );
}

#[tokio::test]
async fn tag_specifications_tag_only_the_window_the_call_creates() {
    let router = emulator();
    // Every row names a resource type, or none: the window is the only resource this call
    // creates, so only its own type may be tagged.
    let rows: Vec<(&str, Option<&str>, u16, &str)> = vec![
        ("the window's own type", Some("instance-event-window"), 200, ""),
        ("another resource's type", Some("instance"), 400, "InvalidParameterValue"),
        ("a second other type", Some("volume"), 400, "InvalidParameterValue"),
        ("no resource type at all", None, 400, "MissingParameter"),
    ];
    for (case, resource_type, status, code) in rows {
        let mut params = vec![p("CronExpression", "* 0-4 * * 1")];
        params.extend(tag_specification(resource_type, &[("Owner", "platform")]));
        let (actual, body) = create(&router, &params).await;
        assert_eq!(
            actual, status,
            "case {case:?}: expected HTTP {status}, got {actual} with body {body}"
        );
        if status == 200 {
            assert_eq!(
                element(&body, "key").as_deref(),
                Some("Owner"),
                "case {case:?}: the window must carry the tag the request named, \
                 got body {body}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                code,
                "case {case:?}: expected the error code {code}, got body {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_event_window_carries_as_many_time_ranges_as_fit_in_a_week() {
    let router = emulator();
    // Every range of the smallest span that fits in one week, at the bound and one past it
    // — the count taken from the service's own constant so the two cannot drift. A slot
    // that would end at hour 24 ends at hour 0 of the next day instead, the hours being
    // modelled as 0..=23; the last one of the week therefore runs across its end.
    let slots_per_day = 24 / MIN_EVENT_WINDOW_RANGE_HOURS;
    let every_slot: Vec<(usize, i64, usize, i64)> = (0..MAX_EVENT_WINDOW_TIME_RANGES)
        .map(|slot| {
            let slot = i64::try_from(slot).expect("a slot index fits in i64");
            let day = usize::try_from(slot / slots_per_day).expect("a day index fits in usize");
            let hour = (slot % slots_per_day) * MIN_EVENT_WINDOW_RANGE_HOURS;
            let end = hour + MIN_EVENT_WINDOW_RANGE_HOURS;
            if end == 24 {
                (day, hour, day + 1, 0)
            } else {
                (day, hour, day, end)
            }
        })
        .collect();

    let mut params = vec![p("Name", "every-slot-of-the-week")];
    params.extend(time_ranges(&every_slot));
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}",
        case = format!("{} time ranges", every_slot.len())
    );
    assert_eq!(
        elements(&body, "startWeekDay").len(),
        every_slot.len(),
        "the window must report every time range the request named, got body {body}"
    );

    let mut one_too_many = every_slot.clone();
    one_too_many.push((0, 4, 0, 8));
    let mut params = vec![p("Name", "one-range-too-many")];
    params.extend(time_ranges(&one_too_many));
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 400,
        "a time range list past the maximum must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "expected InvalidParameterValue, got body {body}"
    );
}

#[tokio::test]
async fn a_dry_run_create_reports_what_it_would_have_done_and_stores_nothing() {
    let router = emulator();
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "a request that would have succeeded",
            vec![p("CronExpression", "* 0-4 * * 1"), p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        (
            "a request that would have been refused",
            vec![p("CronExpression", "* 5 * * 1"), p("DryRun", "true")],
            400,
            "InvalidParameterValue",
        ),
    ];
    for (case, params, status, code) in rows {
        let (actual, body) = create(&router, &params).await;
        assert_eq!(
            actual, status,
            "case {case:?}: expected HTTP {status}, got {actual} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the error code {code}, got body {body}"
        );
    }
    assert!(
        described_ids(&router, "after the dry runs", &[])
            .await
            .is_empty(),
        "a DryRun create must store nothing"
    );
}

/// What one seeded window carries, for the filter table to select on.
struct Fixture {
    name: &'static str,
    cron_expression: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

/// The windows the filter table is written against. They differ along every attribute the
/// documented filters select on that a created window has — its name and its tags.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "maintenance-emea",
            cron_expression: "* 0-4 * * 1",
            tags: vec![("Owner", "platform"), ("Stage", "prod")],
        },
        Fixture {
            name: "maintenance-apac",
            cron_expression: "* 2-4,20-23 * * SAT",
            tags: vec![("Owner", "payments"), ("Stage", "prod")],
        },
        Fixture {
            name: "patching-nightly",
            cron_expression: "* 0-23 * * *",
            tags: vec![("Owner", "platform"), ("Stage", "dev")],
        },
    ]
}

/// Seed the fixtures and answer their ids, in the order they were created (which is the
/// order their minted ids sort in).
async fn seed(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for fixture in fixtures() {
        let mut params = vec![
            p("Name", fixture.name),
            p("CronExpression", fixture.cron_expression),
        ];
        params.extend(window_tags(&fixture.tags));
        ids.push(create_window(router, fixture.name, &params).await);
    }
    ids
}

#[tokio::test]
async fn the_describe_narrows_by_the_filters_it_documents() {
    let router = emulator();
    let ids = seed(&router).await;

    // `(case, the filter parameters, the ids it must select)`.
    let rows: Vec<(&str, Params, Vec<String>)> = vec![
        ("no filter at all reports every window", Vec::new(), ids.clone()),
        (
            "event-window-name, exactly",
            filters(&[("event-window-name", &["maintenance-apac"])]),
            vec![ids[1].clone()],
        ),
        (
            "event-window-name, with EC2's wildcard",
            filters(&[("event-window-name", &["maintenance-*"])]),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "event-window-name, two values joined with OR",
            filters(&[("event-window-name", &["maintenance-emea", "patching-nightly"])]),
            vec![ids[0].clone(), ids[2].clone()],
        ),
        (
            "tag:<key>, the key/value combination",
            filters(&[("tag:Owner", &["platform"])]),
            vec![ids[0].clone(), ids[2].clone()],
        ),
        (
            "tag-key, whatever the value",
            filters(&[("tag-key", &["Stage"])]),
            ids.clone(),
        ),
        (
            "tag-value, whatever the key",
            filters(&[("tag-value", &["dev"])]),
            vec![ids[2].clone()],
        ),
        (
            "two filters joined with AND",
            filters(&[("tag:Owner", &["platform"]), ("tag:Stage", &["prod"])]),
            vec![ids[0].clone()],
        ),
        (
            "a tag filter no window carries",
            filters(&[("tag:Owner", &["nobody"])]),
            Vec::new(),
        ),
        // Nothing is associated with a created window — `AssociateInstanceEventWindow` is
        // the operation that gives one a target — so every filter over its association
        // target selects nothing.
        (
            "instance-id, on windows associated with nothing",
            filters(&[("instance-id", &["i-0123456789abcdef0"])]),
            Vec::new(),
        ),
        (
            "dedicated-host-id, on windows associated with nothing",
            filters(&[("dedicated-host-id", &["h-0123456789abcdef0"])]),
            Vec::new(),
        ),
        (
            "instance-tag, on windows associated with nothing",
            filters(&[("instance-tag", &["Owner=platform"])]),
            Vec::new(),
        ),
        (
            "instance-tag-key, on windows associated with nothing",
            filters(&[("instance-tag-key", &["Owner"])]),
            Vec::new(),
        ),
        (
            "instance-tag-value, on windows associated with nothing",
            filters(&[("instance-tag-value", &["platform"])]),
            Vec::new(),
        ),
    ];
    for (case, params, expected) in rows {
        let actual = described_ids(&router, case, &params).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: the describe must report exactly the windows the filters \
             select, got {actual:?}"
        );
    }

    // A filter this operation does not document is refused, whether or not any window would
    // have matched it — including the `tag:` family with no key.
    for name in ["instance-event-window-id", "state", "tag:", "Name"] {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: an undocumented filter must be refused, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got body {body}"
        );
    }
}

#[tokio::test]
async fn the_describe_resolves_every_id_it_is_given() {
    let router = emulator();
    let ids = seed(&router).await;

    // `(case, the ids the request names, the HTTP status, the error code)`.
    let rows: Vec<(&str, Vec<&str>, u16, &str)> = vec![
        ("one window of the account", vec![ids[1].as_str()], 200, ""),
        (
            "every window of the account",
            ids.iter().map(String::as_str).collect(),
            200,
            "",
        ),
        (
            "a well-formed id no window has",
            vec!["iew-0123456789abcdef0"],
            400,
            "InvalidInstanceEventWindowId.NotFound",
        ),
        (
            "an id that is not an event window id at all",
            vec!["not-an-event-window"],
            400,
            "InvalidInstanceEventWindowId.Malformed",
        ),
        (
            "one known id beside one unknown one",
            vec![ids[0].as_str(), "iew-0123456789abcdef0"],
            400,
            "InvalidInstanceEventWindowId.NotFound",
        ),
    ];
    for (case, named, status, code) in rows {
        let params = indexed("InstanceEventWindowId", &named);
        let (actual, body) = describe(&router, &params).await;
        assert_eq!(
            actual, status,
            "case {case:?}: expected HTTP {status}, got {actual} with body {body}"
        );
        if status == 200 {
            let reported = elements(&body, "instanceEventWindowId");
            assert_eq!(
                reported.len(),
                named.len(),
                "case {case:?}: expected {} windows, got {reported:?}",
                named.len()
            );
        } else {
            assert_eq!(
                error_code(&body),
                code,
                "case {case:?}: expected the error code {code}, got body {body}"
            );
        }
    }

    // "You cannot specify this parameter and the event window IDs parameter in the same
    // call" (`DescribeInstanceEventWindowsRequest$MaxResults`).
    let mut params = indexed("InstanceEventWindowId", &[ids[0].as_str()]);
    params.push(p("MaxResults", "20"));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 400,
        "maxResults and instanceEventWindowIds cannot be given together, \
         got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterCombination",
        "expected InvalidParameterCombination, got body {body}"
    );
}

#[tokio::test]
async fn the_describe_pages_in_id_order() {
    let router = emulator();
    // The smallest page size the model admits is 20 ("This value can be between 20 and
    // 500"), so one more window than that is what it takes to need a second page.
    let page_size: usize = 20;
    let mut created = Vec::new();
    for ordinal in 0..=page_size {
        let params = vec![
            p("Name", format!("window-{ordinal:02}")),
            p("CronExpression", "* 0-4 * * 1"),
        ];
        created.push(create_window(&router, "a page of windows", &params).await);
    }
    created.sort();

    let first_page = vec![p("MaxResults", page_size.to_string())];
    let (status, body) = describe(&router, &first_page).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 for the first page, got {status} with body {body}"
    );
    let first = elements(&body, "instanceEventWindowId");
    assert_eq!(
        first,
        created[..page_size].to_vec(),
        "the first page must be the first {page_size} windows in id order"
    );
    let token = element(&body, "nextToken").unwrap_or_default();
    assert!(
        !token.is_empty(),
        "a window follows the first page, so the describe must answer a token, \
         got body {body}"
    );

    let second_page = vec![p("MaxResults", page_size.to_string()), p("NextToken", token)];
    let (status, body) = describe(&router, &second_page).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 for the second page, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "instanceEventWindowId"),
        created[page_size..].to_vec(),
        "the second page must be the windows the first page left, got body {body}"
    );
    assert_eq!(
        element(&body, "nextToken"),
        None,
        "nothing follows the last page, so it carries no token, got body {body}"
    );

    // "The token to request the next page of results": anything else is not a token.
    let bogus = vec![p("NextToken", "not-a-token")];
    let (status, body) = describe(&router, &bogus).await;
    assert_eq!(
        status, 400,
        "a token this service did not mint must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "expected InvalidPaginationToken, got body {body}"
    );
}

#[tokio::test]
async fn the_account_creates_as_many_windows_as_the_quota_allows() {
    let router = emulator();
    // "You can create up to 200 event windows per Amazon Web Services Region"
    // (`CreateInstanceEventWindow`) — asserted at the bound and one past it, from the
    // service's own constant so the two cannot drift.
    for ordinal in 0..MAX_INSTANCE_EVENT_WINDOWS {
        let params = vec![
            p("Name", format!("window-{ordinal}")),
            p("CronExpression", "* 0-4 * * 1"),
        ];
        create_window(&router, "up to the quota", &params).await;
    }
    let params = vec![
        p("Name", "one-too-many"),
        p("CronExpression", "* 0-4 * * 1"),
    ];
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 400,
        "the window past the quota must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InstanceEventWindowLimitExceeded",
        "expected InstanceEventWindowLimitExceeded, got body {body}"
    );
}
