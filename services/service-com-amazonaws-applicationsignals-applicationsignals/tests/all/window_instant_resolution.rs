// SPDX-License-Identifier: CC0-1.0

//! How this service compares a STORED instant against a requested window, end to end over
//! HTTP through the router production mounts.
//!
//! The rule every row asserts: every window test this service makes on a stored instant is
//! exact at the full resolution that instant carries — whole seconds AND the nanoseconds
//! within them — so an objective created a fraction of a microsecond after a window's
//! `EndTime` is not evidence of its entity over that window, while one created a
//! nanosecond before the end, or exactly at it, is (the bound is inclusive). One rule, one
//! resolution, so every read over one window answers the same way: the topology reads that
//! report what was observed in it and the reads that report the changes recorded inside it
//! cannot disagree about whether the objective's creation falls in the window.
//!
//! An objective's `CreatedTime` is this service's own clock, which no request can name, so
//! each row creates the objective over HTTP and then places that instant in the store the
//! handlers share — the one engine [`crate::instrumentation_configuration_tags::router_with_engine`]
//! hands back, as this crate's other clock-exact suites do — before asking every windowed
//! read about it through the wire.
//!
//! The rows vary what the rule generalises over: the sub-second part of the window's end
//! (none, half a second), the SIGN and SIZE of the gap between the stored instant and that
//! end (a whole second either way, half a microsecond, 123 nanoseconds, a single
//! nanosecond, none at all), whether that gap crosses a second boundary, and which read
//! makes the comparison (all seven windowed reads, plus the budget report's own window).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state::ComAmazonawsApplicationsignalsApplicationsignalsState;
use smithy_server_runtime::MultiServiceRouter;

use crate::instrumentation_configuration_tags::router_with_engine;
use crate::slo_lifecycle::{call, create};

/// How many nanoseconds make a second.
///
/// `pub(crate)` so the sibling suites that place an instant against a window bound compare
/// on the one scale this one does.
pub(crate) const NANOS_PER_SECOND: i64 = 1_000_000_000;

/// How far before the window's end the window starts: an hour, so every row's window has
/// begun and the recorded creation falls inside it whenever it is not past the end.
const WINDOW_SECONDS: i64 = 3_600;

/// The objective every row stores, and the entities and operation it monitors.
const OBJECTIVE: &str = "window-resolution";
const SERVICE: &str = "checkout";
const OPERATION: &str = "POST /checkout";
const DEPENDENCY: &str = "payments";
const DEPENDENCY_OPERATION: &str = "Charge";

/// The `BudgetStatus` a report states for a window the objective was evaluated over, and
/// the one it states for an empty window ("the specified start and end times were before
/// the SLO was created", `smithy_query(query="doc",
/// shape="ServiceLevelObjectiveBudgetReport", member="BudgetStatus")`).
const STATUS_OK: &str = "OK";
const STATUS_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// One instant as the parts a `Timestamp` carries: epoch seconds and the nanoseconds within
/// that second.
pub(crate) type Instant = (i64, u32);

/// The whole second every window here ends on: an hour in the past, so the window has begun
/// and the requested end is behind the clock whatever the row shifts by.
fn base_second() -> i64 {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("epoch seconds fit an i64");
    now - WINDOW_SECONDS
}

/// `at` moved by `offset` nanoseconds, exactly — so a row can place a stored instant one
/// nanosecond from a requested bound, including across a second boundary.
pub(crate) fn shifted(at: Instant, offset: i64) -> Instant {
    let total = i128::from(at.0) * i128::from(NANOS_PER_SECOND)
        + i128::from(at.1)
        + i128::from(offset);
    let secs = i64::try_from(total.div_euclid(i128::from(NANOS_PER_SECOND)))
        .unwrap_or_else(|_| panic!("instant {at:?} shifted by {offset} ns is out of range"));
    let nanos = u32::try_from(total.rem_euclid(i128::from(NANOS_PER_SECOND)))
        .expect("a sub-second remainder is below one second");
    (secs, nanos)
}

/// `at` in the ISO 8601 `date-time` form a query-bound `Timestamp` carries, with the
/// fraction spelled to the nanosecond — the precision the REQUEST has to carry for the rule
/// to be compared at it.
pub(crate) fn query_instant(at: Instant) -> String {
    let whole = DateTime::from_secs(at.0)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at:?}: {error}"));
    if at.1 == 0 {
        return whole;
    }
    let without_zone = whole.strip_suffix('Z').unwrap_or(&whole);
    format!("{without_zone}.{:09}Z", at.1)
}

/// `at` as the epoch-seconds number a body-bound `Timestamp` carries on `restJson1`.
///
/// Every instant a request names here has a sub-second part of none or half a second, both
/// of which this scale holds exactly, so the bound a body carries is the bound the row
/// chose.
fn body_instant(at: Instant) -> f64 {
    at.0 as f64 + f64::from(at.1) / NANOS_PER_SECOND as f64
}

/// The key attributes of one Application Signals service entity.
pub(crate) fn entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": "prod" })
}

/// Create the objective this suite stores: it monitors one operation of one service and
/// names one dependency, so a single stored record is evidence for every windowed read.
async fn create_objective(router: &MultiServiceRouter, case: &str) {
    let (status, answer) = create(
        router,
        json!({
            "Name": OBJECTIVE,
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": entity(SERVICE),
                    "OperationName": OPERATION,
                    "MetricType": "LATENCY",
                    "DependencyConfig": {
                        "DependencyKeyAttributes": entity(DEPENDENCY),
                        "DependencyOperationName": DEPENDENCY_OPERATION
                    },
                    "PeriodSeconds": 60
                },
                "MetricThreshold": 100.0,
                "ComparisonOperator": "LessThan"
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the objective create to answer 200, got {status} with {answer:?}"
    );
}

/// Place the stored objective's creation at `at`, in the store the handlers share.
///
/// Both instants move together, so the record carries exactly ONE change — its creation at
/// `at` — and a change read over the window reports an event exactly when that instant
/// falls inside it.
fn place_created_time(engine: &Arc<engine::InMemoryEngine>, case: &str, at: Instant) {
    let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
    let mut store = state.lock();
    let objective = store
        .slos
        .get_mut(OBJECTIVE)
        .unwrap_or_else(|| panic!("case {case:?}: the create stored the objective"));
    objective.created_time = DateTime::from_secs_and_nanos(at.0, at.1);
    objective.last_updated_time = objective.created_time;
}

/// Where a read binds the window it reports over.
#[derive(Debug, Clone, Copy)]
enum Window {
    /// The query string (`@httpQuery`), which carries the `date-time` form.
    Query,
    /// The JSON body, which carries the epoch-seconds form.
    Body,
}

/// How a read's answer shows that the entity was observed over the window.
#[derive(Debug, Clone, Copy)]
enum Observation {
    /// A `@required` list member carrying one entry per observation.
    List(&'static str),
    /// `GetService` reports back the identity it was asked about whatever the window, and
    /// carries the metrics Application Signals collects only for an entity the store is
    /// evidence of over it.
    ServiceMetrics,
}

/// One windowed read: the route it answers on, what it carries beside the window, where the
/// window binds and how its answer shows the entity was observed.
#[derive(Debug, Clone)]
struct Read {
    label: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    window: Window,
    observation: Observation,
}

/// Every read of this service that tests a stored instant against a requested window.
fn reads() -> Vec<Read> {
    vec![
        Read {
            label: "ListServices",
            method: "GET",
            path: "/services",
            body: None,
            window: Window::Query,
            observation: Observation::List("ServiceSummaries"),
        },
        Read {
            label: "GetService",
            method: "POST",
            path: "/service",
            body: Some(json!({ "KeyAttributes": entity(SERVICE) })),
            window: Window::Query,
            observation: Observation::ServiceMetrics,
        },
        Read {
            label: "ListServiceOperations",
            method: "POST",
            path: "/service-operations",
            body: Some(json!({ "KeyAttributes": entity(SERVICE) })),
            window: Window::Query,
            observation: Observation::List("ServiceOperations"),
        },
        Read {
            label: "ListServiceDependencies",
            method: "POST",
            path: "/service-dependencies",
            body: Some(json!({ "KeyAttributes": entity(SERVICE) })),
            window: Window::Query,
            observation: Observation::List("ServiceDependencies"),
        },
        Read {
            label: "ListServiceDependents",
            method: "POST",
            path: "/service-dependents",
            body: Some(json!({ "KeyAttributes": entity(DEPENDENCY) })),
            window: Window::Query,
            observation: Observation::List("ServiceDependents"),
        },
        Read {
            label: "ListAuditFindings",
            method: "POST",
            path: "/auditFindings",
            body: Some(json!({
                "AuditTargets": [{ "Type": "service", "Data": { "Service": entity(SERVICE) } }]
            })),
            window: Window::Query,
            observation: Observation::List("AuditFindings"),
        },
        Read {
            label: "ListServiceStates",
            method: "POST",
            path: "/service/states",
            body: Some(json!({})),
            window: Window::Body,
            observation: Observation::List("ServiceStates"),
        },
        Read {
            label: "ListEntityEvents",
            method: "POST",
            path: "/events",
            body: Some(json!({ "Entity": entity(SERVICE) })),
            window: Window::Body,
            observation: Observation::List("ChangeEvents"),
        },
    ]
}

/// Ask one read about the window `start`..`end` and answer whether it reported the entity
/// as observed in it.
async fn reports_the_entity(
    router: &MultiServiceRouter,
    case: &str,
    read: &Read,
    start: Instant,
    end: Instant,
) -> bool {
    let (uri, body) = match read.window {
        Window::Query => (
            format!(
                "{}?StartTime={}&EndTime={}",
                read.path,
                query_instant(start),
                query_instant(end)
            ),
            read.body.clone(),
        ),
        Window::Body => {
            let mut body = read.body.clone().unwrap_or_else(|| json!({}));
            body["StartTime"] = json!(body_instant(start));
            body["EndTime"] = json!(body_instant(end));
            (read.path.to_string(), Some(body))
        }
    };
    let (status, answer) = call(router, read.method, &uri, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
    );
    match read.observation {
        Observation::List(member) => {
            let entries = answer[member].as_array().unwrap_or_else(|| {
                panic!("case {case:?}: expected a {member} list, got {answer:?}")
            });
            !entries.is_empty()
        }
        Observation::ServiceMetrics => {
            let references = answer["Service"]["MetricReferences"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a MetricReferences list, got {answer:?}")
                });
            !references.is_empty()
        }
    }
}

/// One placement of the stored creation against one window end.
#[derive(Debug)]
struct Row {
    label: &'static str,
    /// The sub-second part of the window's `EndTime`.
    end_subsec_nanos: u32,
    /// Where the creation sits, in nanoseconds from that end.
    created_offset_nanos: i64,
    /// Whether the objective is therefore evidence of its entity over the window.
    observed: bool,
}

/// The table: a creation on either side of the end, and exactly at it, at gaps from a whole
/// second down to a single nanosecond, against an end on a whole second and an end mid-second.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "created a whole second before an end on a whole second",
            end_subsec_nanos: 0,
            created_offset_nanos: -NANOS_PER_SECOND,
            observed: true,
        },
        Row {
            label: "created 123 nanoseconds before an end on a whole second",
            end_subsec_nanos: 0,
            created_offset_nanos: -123,
            observed: true,
        },
        Row {
            label: "created a nanosecond before an end on a whole second, across the second boundary",
            end_subsec_nanos: 0,
            created_offset_nanos: -1,
            observed: true,
        },
        Row {
            label: "created exactly at an end on a whole second",
            end_subsec_nanos: 0,
            created_offset_nanos: 0,
            observed: true,
        },
        Row {
            label: "created a nanosecond after an end on a whole second",
            end_subsec_nanos: 0,
            created_offset_nanos: 1,
            observed: false,
        },
        Row {
            label: "created 123 nanoseconds after an end on a whole second",
            end_subsec_nanos: 0,
            created_offset_nanos: 123,
            observed: false,
        },
        Row {
            label: "created half a microsecond after an end on a whole second",
            end_subsec_nanos: 0,
            created_offset_nanos: 500,
            observed: false,
        },
        Row {
            label: "created a whole second after an end on a whole second",
            end_subsec_nanos: 0,
            created_offset_nanos: NANOS_PER_SECOND,
            observed: false,
        },
        Row {
            label: "created a nanosecond before an end mid-second",
            end_subsec_nanos: 500_000_000,
            created_offset_nanos: -1,
            observed: true,
        },
        Row {
            label: "created exactly at an end mid-second",
            end_subsec_nanos: 500_000_000,
            created_offset_nanos: 0,
            observed: true,
        },
        Row {
            label: "created a nanosecond after an end mid-second",
            end_subsec_nanos: 500_000_000,
            created_offset_nanos: 1,
            observed: false,
        },
        Row {
            label: "created 123 nanoseconds after an end mid-second",
            end_subsec_nanos: 500_000_000,
            created_offset_nanos: 123,
            observed: false,
        },
    ]
}

/// Every windowed read places the stored creation against the requested `EndTime` at the
/// resolution the stored instant carries, and therefore every one of them answers the same
/// way about one window.
#[tokio::test]
async fn a_window_bound_is_compared_at_the_resolution_a_stored_instant_carries() {
    let base = base_second();
    for row in rows() {
        let end = (base, row.end_subsec_nanos);
        let start = (base - WINDOW_SECONDS, 0);
        let created = shifted(end, row.created_offset_nanos);

        let (router, engine) = router_with_engine();
        create_objective(&router, row.label).await;
        place_created_time(&engine, row.label, created);

        for read in reads() {
            let case = format!("{} / {}", row.label, read.label);
            let observed = reports_the_entity(&router, &case, &read, start, end).await;
            assert_eq!(
                observed, row.observed,
                "case {case:?}: an objective created at {created:?} over a window ending at \
                 {end:?} is {}observed; got observed={observed}",
                if row.observed { "" } else { "not " }
            );
        }

        // The identity reported for an observed entity is the entity's own, so the rows that
        // observe it report THAT service rather than merely a non-empty list.
        let (status, answer) = call(
            &router,
            "GET",
            &format!(
                "/services?StartTime={}&EndTime={}",
                query_instant(start),
                query_instant(end)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from ListServices, got {status} with {answer:?}",
            row.label
        );
        let reported: Vec<Value> = answer["ServiceSummaries"]
            .as_array()
            .map(|summaries| {
                summaries
                    .iter()
                    .map(|summary| summary["KeyAttributes"].clone())
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<Value> = if row.observed {
            vec![entity(SERVICE)]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?} from ListServices, got {reported:?}",
            row.label
        );
    }
}

/// One budget-report row: where the creation sits relative to the requested `Timestamp`,
/// and the `BudgetStatus` that earns.
#[derive(Debug)]
struct BudgetRow {
    label: &'static str,
    created_offset_nanos: i64,
    budget_status: &'static str,
}

/// The budget report's own window is placed by the same rule: a report covers the part of
/// the interval that both follows the creation and has elapsed, so an objective created
/// before the requested instant — by however little — was evaluated over a non-empty
/// window, and one created at or after it was evaluated over nothing.
#[tokio::test]
async fn the_budget_report_window_is_compared_at_the_same_resolution() {
    let rows = vec![
        BudgetRow {
            label: "created an hour before the requested instant",
            created_offset_nanos: -WINDOW_SECONDS * NANOS_PER_SECOND,
            budget_status: STATUS_OK,
        },
        BudgetRow {
            label: "created 123 nanoseconds before the requested instant",
            created_offset_nanos: -123,
            budget_status: STATUS_OK,
        },
        BudgetRow {
            label: "created a nanosecond before the requested instant",
            created_offset_nanos: -1,
            budget_status: STATUS_OK,
        },
        BudgetRow {
            label: "created exactly at the requested instant",
            created_offset_nanos: 0,
            budget_status: STATUS_INSUFFICIENT_DATA,
        },
        BudgetRow {
            label: "created a nanosecond after the requested instant",
            created_offset_nanos: 1,
            budget_status: STATUS_INSUFFICIENT_DATA,
        },
        BudgetRow {
            label: "created 123 nanoseconds after the requested instant",
            created_offset_nanos: 123,
            budget_status: STATUS_INSUFFICIENT_DATA,
        },
    ];

    let timestamp = base_second();
    for row in rows {
        let (router, engine) = router_with_engine();
        create_objective(&router, row.label).await;
        place_created_time(
            &engine,
            row.label,
            shifted((timestamp, 0), row.created_offset_nanos),
        );

        let (status, answer) = call(
            &router,
            "POST",
            "/budget-report",
            Some(json!({ "SloIds": [OBJECTIVE], "Timestamp": timestamp })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {answer:?}",
            row.label
        );
        let reported = answer["Reports"][0]["BudgetStatus"].as_str();
        assert_eq!(
            reported,
            Some(row.budget_status),
            "case {:?}: expected BudgetStatus {:?}, got {reported:?} in {answer:?}",
            row.label,
            row.budget_status
        );
    }
}
