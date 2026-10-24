// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Network Flow Monitor QUERY plane — the three
//! `StartQuery*` / `StopQuery*` / `GetQueryStatus*` / `GetQueryResults*` families —
//! driven over the service's own `restJson1` wire protocol through the same
//! `MultiServiceRouter` the production emulator dispatches with.
//!
//! Every test asserts a RULE over a SET of inputs; the shape the issue recorded is
//! one row each.
//!
//! The rules pinned here:
//!
//! 1. A `Timestamp` member the model marks `@timestampFormat("date-time")` is read
//!    off the restJson1 wire as an RFC 3339 STRING — for every such member of this
//!    service, in every rendering of that format, and not as an epoch integer.
//! 2. A `StartQuery*` request whose `endTime` precedes its `startTime` names no
//!    window and is refused with the modelled `ValidationException`.
//! 3. A started query is a record in this service's store: its status moves
//!    `QUEUED` -> `RUNNING` -> `SUCCEEDED` on the clock alone, `StopQuery*` moves it
//!    to the terminal `CANCELED`, its results are reported only once it has
//!    SUCCEEDED, and then in the unit of the metric it was STARTED with.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_networkflowmonitor_networkflowmonitor as nfm;

use crate::harness::{is_uuid_shaped, targets, Answer, Emulator, PAST_PROVISIONING};

/// Longer than `nfm::state::QUERY_COMPLETION_DELAY`, so a poll after this wait is
/// one the clock already owes the terminal status to.
const PAST_QUERY_COMPLETION: Duration = Duration::from_millis(1_000);

/// The statuses `QueryStatus` declares (`smithy_query(query="shape",
/// shape="QueryStatus")`: QUEUED | RUNNING | SUCCEEDED | FAILED | CANCELED). Used to
/// assert a reported status is one the model declares at all.
const QUERY_STATUS_VALUES: &[&str] = &["QUEUED", "RUNNING", "SUCCEEDED", "FAILED", "CANCELED"];

/// The statuses that are NOT terminal: a query in one of them is still moving.
const NON_TERMINAL: &[&str] = &["QUEUED", "RUNNING"];

/// Which of the three query lifecycles the model binds a row exercises. Each one has
/// its own URI family, its own metric enum and its own output shape, so every table
/// below runs over all three rather than over the one the issue recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    /// `/monitors/{monitorName}/topContributorsQueries`.
    Monitor,
    /// `/workloadInsights/{scopeId}/topContributorsQueries`.
    WorkloadInsights,
    /// `/workloadInsights/{scopeId}/topContributorsDataQueries`.
    WorkloadInsightsData,
}

/// Every family, so a table spans the whole query plane.
const FAMILIES: &[Family] = &[
    Family::Monitor,
    Family::WorkloadInsights,
    Family::WorkloadInsightsData,
];

impl Family {
    /// A metric this family's own metric enum declares: `MonitorMetric` for the
    /// monitor family, `WorkloadInsightsMetric` for the other two.
    fn metric(self) -> &'static str {
        match self {
            // Deliberately NOT the issue's `DATA_TRANSFERRED`.
            Family::Monitor => "ROUND_TRIP_TIME",
            _ => "RETRANSMISSIONS",
        }
    }
}

/// One emulator with a scope and a monitor already in it: the parents every query
/// needs, and the two ids the query URIs are built from.
struct Plane {
    emulator: Emulator,
    monitor_name: String,
    scope_id: String,
}

impl Plane {
    async fn new() -> Self {
        let emulator = Emulator::new();
        let scope = emulator
            .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
            .await;
        assert_eq!(
            scope.status, 200,
            "the parent scope must be created, got {} with body {}",
            scope.status, scope.body
        );
        let scope_id = scope.text("scopeId");
        let scope_arn = scope.text("scopeArn");

        let monitor_name = "query-plane-monitor".to_string();
        let monitor = emulator
            .create_monitor(json!({
                "monitorName": monitor_name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-0123456789abcdef0" }]
            }))
            .await;
        assert_eq!(
            monitor.status, 200,
            "the parent monitor must be created, got {} with body {}",
            monitor.status, monitor.body
        );
        // The monitor is ACTIVE before any query is started, exactly as a client
        // script waits for it to be.
        tokio::time::sleep(PAST_PROVISIONING).await;

        Self {
            emulator,
            monitor_name,
            scope_id,
        }
    }

    /// The resource a family's queries hang off: the monitor name for the monitor
    /// family, the scope id for the two workload-insights ones.
    fn owner(&self, family: Family) -> &str {
        match family {
            Family::Monitor => &self.monitor_name,
            _ => &self.scope_id,
        }
    }

    /// The collection URI of `family` under `owner`, from the operation's `@http`.
    fn base(&self, family: Family, owner: &str) -> String {
        match family {
            Family::Monitor => format!("/monitors/{owner}/topContributorsQueries"),
            Family::WorkloadInsights => {
                format!("/workloadInsights/{owner}/topContributorsQueries")
            }
            Family::WorkloadInsightsData => {
                format!("/workloadInsights/{owner}/topContributorsDataQueries")
            }
        }
    }

    async fn start_raw(&self, family: Family, owner: &str, body: Value) -> Answer {
        self.emulator
            .call("POST", &self.base(family, owner), Some(body))
            .await
    }

    /// `StartQuery*` with the window `start`..`end` as the wire carries them (any
    /// JSON value, so a table can send a string, a number or nonsense).
    async fn start(&self, family: Family, start: Value, end: Value) -> Answer {
        let body = json!({
            "startTime": start,
            "endTime": end,
            "metricName": family.metric(),
            "destinationCategory": "UNCLASSIFIED"
        });
        self.start_raw(family, self.owner(family), body).await
    }

    /// `StartQuery*` with an explicit metric, for the unit catalog.
    async fn start_metric(&self, family: Family, metric: &str) -> Answer {
        let body = json!({
            "startTime": "2026-10-22T19:07:07Z",
            "endTime": "2026-10-22T20:07:07Z",
            "metricName": metric,
            "destinationCategory": "UNCLASSIFIED"
        });
        self.start_raw(family, self.owner(family), body).await
    }

    async fn status_of(&self, family: Family, owner: &str, query_id: &str) -> Answer {
        let path = format!("{}/{query_id}/status", self.base(family, owner));
        self.emulator.call("GET", &path, None).await
    }

    async fn status(&self, family: Family, query_id: &str) -> Answer {
        self.status_of(family, self.owner(family), query_id).await
    }

    async fn results_of(&self, family: Family, owner: &str, query_id: &str) -> Answer {
        let path = format!("{}/{query_id}/results", self.base(family, owner));
        self.emulator.call("GET", &path, None).await
    }

    async fn results(&self, family: Family, query_id: &str) -> Answer {
        self.results_of(family, self.owner(family), query_id).await
    }

    async fn stop(&self, family: Family, query_id: &str) -> Answer {
        let path = format!("{}/{query_id}", self.base(family, self.owner(family)));
        self.emulator.call("DELETE", &path, None).await
    }
}

/// Whether an answer carries `code`, whichever slot this protocol puts it in:
/// the harness's own accessor, so every suite reads the code one way.
fn is_error(answer: &Answer, code: &str) -> bool {
    answer.is_error(code)
}

/// One row of the `date-time` wire-form table: a rendering of the RFC 3339
/// `date-time` format the two members declare, and the window it names.
struct WindowRow {
    case: &'static str,
    start: Value,
    end: Value,
}

/// Every form of the modelled wire value this service has to read. The format is
/// `@timestampFormat("date-time")` — RFC 3339 — so the dimensions are the precision
/// of the rendering and the instant it names, including both ends of the range a
/// 32-bit epoch could carry.
fn accepted_windows() -> Vec<WindowRow> {
    vec![
        WindowRow {
            case: "whole seconds, the form a stock SDK renders",
            start: json!("2026-10-22T19:07:07Z"),
            end: json!("2026-10-22T20:07:07Z"),
        },
        WindowRow {
            case: "millisecond precision",
            start: json!("2026-10-22T19:07:07.123Z"),
            end: json!("2026-10-22T20:07:07.987Z"),
        },
        WindowRow {
            case: "nanosecond precision",
            start: json!("2026-10-22T19:07:07.123456789Z"),
            end: json!("2026-10-22T19:07:07.123456790Z"),
        },
        WindowRow {
            case: "the epoch itself",
            start: json!("1970-01-01T00:00:00Z"),
            end: json!("1970-01-01T01:00:00Z"),
        },
        WindowRow {
            case: "past the 32-bit epoch rollover",
            start: json!("2038-01-19T03:14:07Z"),
            end: json!("2038-01-19T03:14:08Z"),
        },
        WindowRow {
            case: "a leap day",
            start: json!("2024-02-29T00:00:00Z"),
            end: json!("2024-02-29T23:59:59Z"),
        },
        WindowRow {
            case: "an empty window: the end equal to the start",
            start: json!("2026-10-22T20:07:07Z"),
            end: json!("2026-10-22T20:07:07Z"),
        },
        WindowRow {
            // The protocol's own default form for a timestamp, which the codec reads
            // from a JSON number whatever format the member declares. Here so the
            // `date-time` fix is shown not to have closed the numeric form.
            case: "epoch seconds as a JSON number",
            start: json!(1_761_163_627i64),
            end: json!(1_761_167_227i64),
        },
    ]
}

#[tokio::test]
async fn every_start_query_reads_its_date_time_window_off_the_wire() {
    let plane = Plane::new().await;

    for family in FAMILIES {
        for row in accepted_windows() {
            let answer = plane.start(*family, row.start.clone(), row.end.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case {family:?}/{:?}: expected 200 for a modelled date-time window, \
                 got {} with body {}",
                row.case, answer.status, answer.body
            );
            let query_id = answer.text("queryId");
            assert!(
                is_uuid_shaped(&query_id),
                "case {family:?}/{:?}: expected a minted queryId, got {query_id:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_value_that_is_not_a_date_time_is_refused_as_invalid() {
    let plane = Plane::new().await;

    // A value the declared format cannot name an instant from. Each is refused as a
    // bad request, never accepted and never answered with a server fault.
    for case in [
        json!("not-a-time"),
        json!(""),
        json!("2026-10-22"),
        json!("2026-13-45T99:99:99Z"),
        json!("Wed, 22 Oct 2026 20:07:07 GMT"),
        json!(true),
    ] {
        for family in FAMILIES {
            let answer = plane
                .start(*family, case.clone(), json!("2026-10-22T20:07:07Z"))
                .await;
            assert_eq!(
                answer.status, 400,
                "case {family:?}/{case:?}: expected 400, got {} with body {}",
                answer.status, answer.body
            );
            assert!(
                is_error(&answer, "ValidationException"),
                "case {family:?}/{case:?}: expected ValidationException, got {:?} / body {}",
                answer.error_type,
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn every_date_time_member_schema_carries_the_declared_format() {
    use aws_smithy_schema::traits::TimestampFormat;
    use nfm::ops::types::{
        StartQueryMonitorTopContributorsInput, StartQueryWorkloadInsightsTopContributorsDataInput,
        StartQueryWorkloadInsightsTopContributorsInput, WorkloadInsightsTopContributorsDataPoint,
    };

    // Every member of this service whose model carries `@timestampFormat("date-time")`:
    // the `startTime`/`endTime` pair of each `StartQuery*` input, and the element of
    // `WorkloadInsightsTopContributorsTimestampsList`, which the codec reads and
    // writes against its enclosing member's schema.
    let members = [
        (
            "StartQueryMonitorTopContributorsInput$startTime",
            &StartQueryMonitorTopContributorsInput::M_START_TIME,
        ),
        (
            "StartQueryMonitorTopContributorsInput$endTime",
            &StartQueryMonitorTopContributorsInput::M_END_TIME,
        ),
        (
            "StartQueryWorkloadInsightsTopContributorsInput$startTime",
            &StartQueryWorkloadInsightsTopContributorsInput::M_START_TIME,
        ),
        (
            "StartQueryWorkloadInsightsTopContributorsInput$endTime",
            &StartQueryWorkloadInsightsTopContributorsInput::M_END_TIME,
        ),
        (
            "StartQueryWorkloadInsightsTopContributorsDataInput$startTime",
            &StartQueryWorkloadInsightsTopContributorsDataInput::M_START_TIME,
        ),
        (
            "StartQueryWorkloadInsightsTopContributorsDataInput$endTime",
            &StartQueryWorkloadInsightsTopContributorsDataInput::M_END_TIME,
        ),
        (
            "WorkloadInsightsTopContributorsDataPoint$timestamps",
            &WorkloadInsightsTopContributorsDataPoint::M_TIMESTAMPS,
        ),
    ];

    for (case, member) in members {
        let format = member.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: expected the member schema to carry date-time, got {format:?}"
        );
    }
}

#[tokio::test]
async fn an_inverted_window_is_refused_with_the_relation_that_failed() {
    let plane = Plane::new().await;

    // Rows differ in HOW MUCH the end precedes the start — a nanosecond, a second,
    // an hour, a decade — so the refusal is about the relation and not about a gap
    // size, and in how the two instants are rendered.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "an hour backwards, the recorded shape",
            json!("2026-10-22T20:07:07Z"),
            json!("2026-10-22T19:07:07Z"),
        ),
        (
            "one second backwards",
            json!("2026-10-22T20:07:07Z"),
            json!("2026-10-22T20:07:06Z"),
        ),
        (
            "one nanosecond backwards",
            json!("2026-10-22T20:07:07.000000001Z"),
            json!("2026-10-22T20:07:07Z"),
        ),
        (
            "a decade backwards",
            json!("2026-10-22T20:07:07Z"),
            json!("2016-10-22T20:07:07Z"),
        ),
        (
            "backwards across the epoch",
            json!("1970-01-01T00:00:01Z"),
            json!("1969-12-31T23:59:59Z"),
        ),
        (
            "backwards, sent as epoch-seconds numbers",
            json!(1_761_167_227i64),
            json!(1_761_163_627i64),
        ),
    ];

    for family in FAMILIES {
        for (case, start, end) in &rows {
            let answer = plane.start(*family, start.clone(), end.clone()).await;
            assert_eq!(
                answer.status, 400,
                "case {family:?}/{case:?}: expected 400, got {} with body {}",
                answer.status, answer.body
            );
            assert!(
                is_error(&answer, "ValidationException"),
                "case {family:?}/{case:?}: expected ValidationException, got {:?} / body {}",
                answer.error_type,
                answer.body
            );
            assert_eq!(
                answer.text("message"),
                "EndTime cannot be before StartTime",
                "case {family:?}/{case:?}: expected the fixed refusal for an inverted \
                 window, got {:?}",
                answer.text("message")
            );
            assert_eq!(
                answer.member("queryId"),
                &Value::Null,
                "case {family:?}/{case:?}: a refused window mints no query, got {}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_well_formed_window_is_accepted_at_and_either_side_of_the_boundary() {
    let plane = Plane::new().await;

    // The bound the refusal above enforces is `end >= start`. Here are the boundary
    // itself and the first value on the accepted side, at second and sub-second
    // precision.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "the boundary: end equal to start",
            json!("2026-10-22T20:07:07Z"),
            json!("2026-10-22T20:07:07Z"),
        ),
        (
            "one nanosecond forwards",
            json!("2026-10-22T20:07:07Z"),
            json!("2026-10-22T20:07:07.000000001Z"),
        ),
        (
            "one second forwards",
            json!("2026-10-22T20:07:06Z"),
            json!("2026-10-22T20:07:07Z"),
        ),
    ];

    for family in FAMILIES {
        for (case, start, end) in &rows {
            let answer = plane.start(*family, start.clone(), end.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case {family:?}/{case:?}: expected 200, got {} with body {}",
                answer.status, answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_started_query_reaches_succeeded_on_the_clock_and_reads_back_its_results() {
    let plane = Plane::new().await;

    for family in FAMILIES {
        let started = plane
            .start(
                *family,
                json!("2026-10-22T19:07:07Z"),
                json!("2026-10-22T20:07:07Z"),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case {family:?}: expected 200, got {} with body {}",
            started.status, started.body
        );
        let query_id = started.text("queryId");

        // A caller polling straight after the start still sees a transient status:
        // the span the clock owes the query is never zero.
        let immediate = plane.status(*family, &query_id).await;
        assert_eq!(
            immediate.status, 200,
            "case {family:?}: expected 200 from the status poll, got {} with body {}",
            immediate.status, immediate.body
        );
        let reported = immediate.text("status");
        assert!(
            QUERY_STATUS_VALUES.contains(&reported.as_str()),
            "case {family:?}: expected a modelled QueryStatus, got {reported:?}"
        );
        assert!(
            NON_TERMINAL.contains(&reported.as_str()),
            "case {family:?}: a freshly started query is still moving, got {reported:?}"
        );

        tokio::time::sleep(PAST_QUERY_COMPLETION).await;

        let settled = plane.status(*family, &query_id).await;
        assert_eq!(
            settled.text("status"),
            "SUCCEEDED",
            "case {family:?}: the clock owes the query SUCCEEDED, got {:?}",
            settled.text("status")
        );
        // A read never changes what the next reader sees.
        let again = plane.status(*family, &query_id).await;
        assert_eq!(
            again.body, settled.body,
            "case {family:?}: two polls of one query must answer the same body, got {} then {}",
            settled.body, again.body
        );

        let results = plane.results(*family, &query_id).await;
        assert_eq!(
            results.status, 200,
            "case {family:?}: expected 200 from the results read, got {} with body {}",
            results.status, results.body
        );
        // Nothing reports network-flow telemetry to this emulator, so the query has
        // no contributing flow in any window: one page, empty, with no resume token.
        let rows = match family {
            Family::WorkloadInsightsData => results.member("datapoints"),
            _ => results.member("topContributors"),
        };
        assert_eq!(
            rows,
            &json!([]),
            "case {family:?}: expected an empty result page, got {rows}"
        );
        assert_eq!(
            results.member("nextToken"),
            &Value::Null,
            "case {family:?}: the only page issues no resume token, got {}",
            results.body
        );
    }
}

#[tokio::test]
async fn query_results_report_the_unit_of_the_metric_the_query_was_started_with() {
    let plane = Plane::new().await;

    // The catalog is the two metric enums crossed with `MetricUnit`
    // (`smithy_query(query="shape", shape="MonitorMetric,WorkloadInsightsMetric,
    // MetricUnit")`): a byte count for data transferred, a duration for a round
    // trip, a plain count for the two event metrics. Every value of `MonitorMetric`
    // is here, not just the issue's `DATA_TRANSFERRED`.
    // Every value of `MonitorMetric` is here, not just the issue's
    // `DATA_TRANSFERRED`; `WorkloadInsightsMetric` declares three of the four, and
    // only the DATA query models a `unit` member.
    let rows: &[(Family, &str, &str)] = &[
        (Family::Monitor, "DATA_TRANSFERRED", "Bytes"),
        (Family::Monitor, "ROUND_TRIP_TIME", "Milliseconds"),
        (Family::Monitor, "TIMEOUTS", "Count"),
        (Family::Monitor, "RETRANSMISSIONS", "Count"),
        (Family::WorkloadInsightsData, "DATA_TRANSFERRED", "Bytes"),
        (Family::WorkloadInsightsData, "TIMEOUTS", "Count"),
        (Family::WorkloadInsightsData, "RETRANSMISSIONS", "Count"),
    ];

    // Start them all, then wait once: results are reported for a query that has
    // SUCCEEDED, so the whole table crosses that line together.
    let mut started: Vec<(Family, &str, &str, String)> = Vec::new();
    for (family, metric, unit) in rows {
        let answer = plane.start_metric(*family, metric).await;
        assert_eq!(
            answer.status, 200,
            "case {family:?}/{metric:?}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        started.push((*family, metric, unit, answer.text("queryId")));
    }

    tokio::time::sleep(PAST_QUERY_COMPLETION).await;

    for (family, metric, unit, query_id) in &started {
        let results = plane.results(*family, query_id).await;
        assert_eq!(
            results.status, 200,
            "case {family:?}/{metric:?}: expected 200 from the results read, got {} with body {}",
            results.status, results.body
        );
        assert_eq!(
            results.text("unit"),
            *unit,
            "case {family:?}/{metric:?}: expected the unit {unit:?}, got {:?}",
            results.text("unit")
        );
    }
}

#[tokio::test]
async fn results_are_refused_for_a_query_that_has_not_succeeded() {
    let plane = Plane::new().await;

    for family in FAMILIES {
        // Still moving: the query has not produced results yet.
        let running = plane
            .start(
                *family,
                json!("2026-10-22T19:07:07Z"),
                json!("2026-10-22T20:07:07Z"),
            )
            .await;
        let running_id = running.text("queryId");
        let answer = plane.results(*family, &running_id).await;
        assert_eq!(
            answer.status, 400,
            "case {family:?}/non-terminal: expected 400, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            is_error(&answer, "ValidationException"),
            "case {family:?}/non-terminal: expected ValidationException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );

        // Stopped: the query never will produce results.
        let canceled = plane
            .start(
                *family,
                json!("2026-10-22T19:07:07Z"),
                json!("2026-10-22T20:07:07Z"),
            )
            .await;
        let canceled_id = canceled.text("queryId");
        let stopped = plane.stop(*family, &canceled_id).await;
        assert_eq!(
            stopped.status, 200,
            "case {family:?}/canceled: the stop must succeed, got {} with body {}",
            stopped.status, stopped.body
        );
        let answer = plane.results(*family, &canceled_id).await;
        assert_eq!(
            answer.status, 400,
            "case {family:?}/canceled: expected 400, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            is_error(&answer, "ValidationException"),
            "case {family:?}/canceled: expected ValidationException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );

        // The same read succeeds once the clock has made the first query SUCCEEDED,
        // so the refusal is about the status and not about the reader.
        tokio::time::sleep(PAST_QUERY_COMPLETION).await;
        let settled = plane.results(*family, &running_id).await;
        assert_eq!(
            settled.status, 200,
            "case {family:?}/succeeded: expected 200, got {} with body {}",
            settled.status, settled.body
        );
        // CANCELED is terminal, so that one is still refused.
        let still_refused = plane.results(*family, &canceled_id).await;
        assert_eq!(
            still_refused.status, 400,
            "case {family:?}/canceled: a canceled query never reports results, got {} with body {}",
            still_refused.status, still_refused.body
        );
    }
}

#[tokio::test]
async fn stopping_a_query_cancels_it_and_a_second_stop_is_a_replay() {
    let plane = Plane::new().await;

    for family in FAMILIES {
        let started = plane
            .start(
                *family,
                json!("2026-10-22T19:07:07Z"),
                json!("2026-10-22T20:07:07Z"),
            )
            .await;
        let query_id = started.text("queryId");

        let stopped = plane.stop(*family, &query_id).await;
        assert_eq!(
            stopped.status, 200,
            "case {family:?}: expected 200 from the stop, got {} with body {}",
            stopped.status, stopped.body
        );

        let after = plane.status(*family, &query_id).await;
        assert_eq!(
            after.text("status"),
            "CANCELED",
            "case {family:?}: a stopped query reports CANCELED, got {:?}",
            after.text("status")
        );

        // The stop is `@idempotent`: repeating it is a replay, not a refusal.
        let repeated = plane.stop(*family, &query_id).await;
        assert_eq!(
            repeated.status, 200,
            "case {family:?}: expected 200 from the repeated stop, got {} with body {}",
            repeated.status, repeated.body
        );

        // CANCELED is terminal: the completion instant the query would otherwise
        // have reached does not move it.
        tokio::time::sleep(PAST_QUERY_COMPLETION).await;
        let settled = plane.status(*family, &query_id).await;
        assert_eq!(
            settled.text("status"),
            "CANCELED",
            "case {family:?}: CANCELED is terminal, got {:?}",
            settled.text("status")
        );
    }
}

#[tokio::test]
async fn stopping_a_query_that_already_finished_is_refused() {
    let plane = Plane::new().await;

    for family in FAMILIES {
        let started = plane
            .start(
                *family,
                json!("2026-10-22T19:07:07Z"),
                json!("2026-10-22T20:07:07Z"),
            )
            .await;
        let query_id = started.text("queryId");

        tokio::time::sleep(PAST_QUERY_COMPLETION).await;
        let settled = plane.status(*family, &query_id).await;
        assert_eq!(
            settled.text("status"),
            "SUCCEEDED",
            "case {family:?}: the query must have finished before the stop, got {:?}",
            settled.text("status")
        );

        let stopped = plane.stop(*family, &query_id).await;
        assert_eq!(
            stopped.status, 400,
            "case {family:?}: expected 400 for stopping a finished query, got {} with body {}",
            stopped.status, stopped.body
        );
        assert!(
            is_error(&stopped, "ValidationException"),
            "case {family:?}: expected ValidationException, got {:?} / body {}",
            stopped.error_type,
            stopped.body
        );
        // The refused stop changed nothing.
        let after = plane.status(*family, &query_id).await;
        assert_eq!(
            after.text("status"),
            "SUCCEEDED",
            "case {family:?}: a refused stop leaves the query as it was, got {:?}",
            after.text("status")
        );
    }
}

#[tokio::test]
async fn a_query_id_is_only_reachable_under_the_resource_and_family_that_minted_it() {
    let plane = Plane::new().await;

    // One query per family, then every (family, id) pair that is NOT the one that
    // minted it is unreachable — including the two workload-insights families, which
    // share one `scopeId` and differ only in their URI.
    let mut minted: Vec<(Family, String)> = Vec::new();
    for family in FAMILIES {
        let started = plane
            .start(
                *family,
                json!("2026-10-22T19:07:07Z"),
                json!("2026-10-22T20:07:07Z"),
            )
            .await;
        minted.push((*family, started.text("queryId")));
    }

    for (minted_by, query_id) in &minted {
        for family in FAMILIES {
            let answer = plane.status(*family, query_id).await;
            if family == minted_by {
                assert_eq!(
                    answer.status, 200,
                    "case {family:?}/{query_id:?}: its own family must find it, got {} with body {}",
                    answer.status, answer.body
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {family:?}/{query_id:?}: a query of {minted_by:?} is not one of \
                 {family:?}, got {} with body {}",
                answer.status, answer.body
            );
        }
    }

    // Nor under another resource of the same family: the monitor query is not a
    // query of a second monitor.
    let other = "query-plane-other-monitor";
    let scope = plane.emulator.get_scope(&plane.scope_id).await;
    assert_eq!(
        scope.status, 200,
        "the parent scope must be readable, got {} with body {}",
        scope.status, scope.body
    );
    let created = plane
        .emulator
        .create_monitor(json!({
            "monitorName": other,
            "scopeArn": scope.text("scopeArn"),
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-00000000000000001" }]
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "the second monitor must be created, got {} with body {}",
        created.status, created.body
    );
    let (_, monitor_query) = minted
        .iter()
        .find(|(family, _)| *family == Family::Monitor)
        .expect("a monitor query was minted above");
    let answer = plane
        .status_of(Family::Monitor, other, monitor_query)
        .await;
    assert_eq!(
        answer.status, 400,
        "case {other:?}: a query of another monitor is not reachable here, got {} with body {}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn an_unknown_query_id_is_refused_by_each_reader_with_its_own_modelled_error() {
    let plane = Plane::new().await;

    // One real query exists, so a refusal is about the id and not about an empty
    // store.
    let started = plane
        .start(
            Family::Monitor,
            json!("2026-10-22T19:07:07Z"),
            json!("2026-10-22T20:07:07Z"),
        )
        .await;
    let known = started.text("queryId");

    for case in [
        "00000000-0000-4000-9000-000000000000",
        "ffffffff-ffff-4fff-9fff-ffffffffffff",
        "not-a-query-id",
        "0",
    ] {
        assert_ne!(case, known, "the unknown-id table must not name a real query");
        for family in FAMILIES {
            // `GetQueryStatus*` and `StopQuery*` model no 404, so an id that names
            // nothing is a bad request there.
            let status = plane.status(*family, case).await;
            assert_eq!(
                status.status, 400,
                "case {family:?}/{case:?}: expected 400 from the status read, got {} with body {}",
                status.status, status.body
            );
            assert!(
                is_error(&status, "ValidationException"),
                "case {family:?}/{case:?}: expected ValidationException, got {:?} / body {}",
                status.error_type,
                status.body
            );

            let stopped = plane.stop(*family, case).await;
            assert_eq!(
                stopped.status, 400,
                "case {family:?}/{case:?}: expected 400 from the stop, got {} with body {}",
                stopped.status, stopped.body
            );

            // `GetQueryResults*` DOES model a 404, so that is what it answers.
            let results = plane.results(*family, case).await;
            assert_eq!(
                results.status, 404,
                "case {family:?}/{case:?}: expected 404 from the results read, got {} with body {}",
                results.status, results.body
            );
            assert!(
                is_error(&results, "ResourceNotFoundException"),
                "case {family:?}/{case:?}: expected ResourceNotFoundException, got {:?} / body {}",
                results.error_type,
                results.body
            );
        }
    }
}

/// A `StartQuery*` whose parent resource the store does not hold is refused: none of
/// the three models a `ResourceNotFoundException`, so the reference it cannot resolve
/// is the modelled `AccessDeniedException` (403).
///
/// Each family's absent owner is of its OWN identifier form — a `ResourceName` for the
/// monitor family, a `scopeId` for the two workload-insights ones — so every row
/// exercises the resolution path rather than the form check that precedes it
/// (`wire_status_and_error_classes.rs` covers that one).
#[tokio::test]
async fn a_start_query_for_a_resource_the_store_does_not_hold_is_refused() {
    let plane = Plane::new().await;

    for family in FAMILIES {
        for owner in absent_owners(*family) {
            let body = json!({
                "startTime": "2026-10-22T19:07:07Z",
                "endTime": "2026-10-22T20:07:07Z",
                "metricName": family.metric(),
                "destinationCategory": "UNCLASSIFIED"
            });
            let answer = plane.start_raw(*family, owner, body).await;
            assert_eq!(
                answer.status, 403,
                "case {family:?}/{owner:?}: expected 403, got {} with body {}",
                answer.status, answer.body
            );
            assert!(
                is_error(&answer, "AccessDeniedException"),
                "case {family:?}/{owner:?}: expected AccessDeniedException, got {:?} / body {}",
                answer.error_type,
                answer.body
            );
        }
    }
}

/// Two owners of `family`'s own identifier form that no test creates: a monitor name
/// for the monitor family, a `scopeId` for the two scope-wide ones.
fn absent_owners(family: Family) -> [&'static str; 2] {
    match family {
        Family::Monitor => ["absent-monitor", "Absent.Monitor_2"],
        _ => [
            "00000000-0000-4000-8000-00000000ffff",
            "ffffffff-ffff-4fff-8fff-ffffffffffff",
        ],
    }
}

#[tokio::test]
async fn a_modelled_constraint_on_a_start_query_member_is_still_refused_at_the_boundary() {
    let plane = Plane::new().await;

    // The hand-written input schemas have to keep every constraint the derive
    // captured, so these are refused before any handler runs: `limit`'s
    // `@range(1, 500)`, and the two enum value sets.
    let rows: Vec<(&str, Value)> = vec![
        (
            "limit below its range",
            json!({
                "startTime": "2026-10-22T19:07:07Z",
                "endTime": "2026-10-22T20:07:07Z",
                "metricName": "ROUND_TRIP_TIME",
                "destinationCategory": "UNCLASSIFIED",
                "limit": 0
            }),
        ),
        (
            "limit above its range",
            json!({
                "startTime": "2026-10-22T19:07:07Z",
                "endTime": "2026-10-22T20:07:07Z",
                "metricName": "ROUND_TRIP_TIME",
                "destinationCategory": "UNCLASSIFIED",
                "limit": 501
            }),
        ),
        (
            "a metric the enum does not declare",
            json!({
                "startTime": "2026-10-22T19:07:07Z",
                "endTime": "2026-10-22T20:07:07Z",
                "metricName": "PACKET_LOSS",
                "destinationCategory": "UNCLASSIFIED"
            }),
        ),
        (
            "a destination category the enum does not declare",
            json!({
                "startTime": "2026-10-22T19:07:07Z",
                "endTime": "2026-10-22T20:07:07Z",
                "metricName": "ROUND_TRIP_TIME",
                "destinationCategory": "INTER_PLANET"
            }),
        ),
        (
            "a required member absent",
            json!({
                "endTime": "2026-10-22T20:07:07Z",
                "metricName": "ROUND_TRIP_TIME",
                "destinationCategory": "UNCLASSIFIED"
            }),
        ),
    ];

    for (case, body) in rows {
        let answer = plane
            .start_raw(Family::Monitor, &plane.monitor_name, body)
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            is_error(&answer, "ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
    }

    // `limit` at each published bound is accepted.
    for limit in [1, 500] {
        let answer = plane
            .start_raw(
                Family::Monitor,
                &plane.monitor_name,
                json!({
                    "startTime": "2026-10-22T19:07:07Z",
                    "endTime": "2026-10-22T20:07:07Z",
                    "metricName": "ROUND_TRIP_TIME",
                    "destinationCategory": "UNCLASSIFIED",
                    "limit": limit
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case limit={limit}: a bound of the published range is accepted, got {} with body {}",
            answer.status, answer.body
        );
    }
}
