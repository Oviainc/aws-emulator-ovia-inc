// SPDX-License-Identifier: CC0-1.0

//! What this service answers at the END of a query's life — the three rules the
//! query plane's terminal states and result reporting follow — driven over the
//! service's own `restJson1` wire protocol through the one harness in
//! [`crate::harness`].
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalizes
//! over, and the shape the issue recorded is one row of each:
//!
//! * RULE 1 (a stop asks only that a query stop running): `StopQuery*` is accepted
//!   whatever status the query is in — it carries an EMPTY output shape and reports
//!   no status of its own — and it moves ONLY a query that is still running, so a
//!   query that already reached a terminal status keeps the outcome it reached and
//!   the next `GetQueryStatus*` still reports it.
//! * RULE 2 (an optional member that describes a result set is reported only with
//!   one): `GetQueryResultsMonitorTopContributors`' `unit` — which the model leaves
//!   OPTIONAL — is absent when `topContributors` has no rows, while the sibling
//!   reader whose `unit` is `@required` always carries it.
//! * RULE 3 (one wording for a `queryId` that names no query): every operation of
//!   the query plane words that condition the same way, naming the id the REQUEST
//!   carried, in the error class its own model declares — the modelled 404 where
//!   `ResourceNotFoundException` is declared, the modelled 400 where it is not.

use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::{targets, Answer, Emulator, PAST_PROVISIONING};

/// Longer than `nfm::state::QUERY_COMPLETION_DELAY`, so a poll after this wait is
/// one the clock already owes the terminal status to.
const PAST_QUERY_COMPLETION: Duration = Duration::from_millis(1_000);

/// Which of the three query lifecycles the model binds a row exercises: each has
/// its own URI family, its own metric enum and its own output shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    /// `/monitors/{monitorName}/topContributorsQueries`.
    Monitor,
    /// `/workloadInsights/{scopeId}/topContributorsQueries`.
    WorkloadInsights,
    /// `/workloadInsights/{scopeId}/topContributorsDataQueries`.
    WorkloadInsightsData,
}

/// Every family, so a table spans the whole query plane rather than the one
/// lifecycle the issue recorded.
const FAMILIES: &[Family] = &[
    Family::Monitor,
    Family::WorkloadInsights,
    Family::WorkloadInsightsData,
];

/// Every value of `MonitorMetric` with the `MetricUnit` its values are measured in
/// (`smithy_query(query="shape",shape="MonitorMetric,MetricUnit")`): a byte count
/// for data transferred, a duration for a round trip, a plain count for the two
/// event metrics. The issue cited only `DATA_TRANSFERRED`.
const MONITOR_METRIC_UNITS: &[(&str, &str)] = &[
    ("DATA_TRANSFERRED", "Bytes"),
    ("ROUND_TRIP_TIME", "Milliseconds"),
    ("TIMEOUTS", "Count"),
    ("RETRANSMISSIONS", "Count"),
];

/// The values of `WorkloadInsightsMetric`, which the DATA query's `@required`
/// `unit` is reported from (`smithy_query(query="shape",
/// shape="WorkloadInsightsMetric")`).
const WORKLOAD_INSIGHTS_METRIC_UNITS: &[(&str, &str)] = &[
    ("DATA_TRANSFERRED", "Bytes"),
    ("TIMEOUTS", "Count"),
    ("RETRANSMISSIONS", "Count"),
];

/// One emulator with the parents every query needs: a scope and a monitor.
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

        let monitor_name = "terminal-plane-monitor".to_string();
        let monitor = emulator
            .create_monitor(json!({
                "monitorName": monitor_name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::Region", "identifier": "us-west-2" }]
            }))
            .await;
        assert_eq!(
            monitor.status, 200,
            "the parent monitor must be created, got {} with body {}",
            monitor.status, monitor.body
        );
        // Every query below is started against an ACTIVE monitor, exactly as a
        // client script waits for it to be.
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

    /// The collection URI of `family`, from the operation's `@http` trait.
    fn base(&self, family: Family) -> String {
        let owner = self.owner(family);
        match family {
            Family::Monitor => format!("/monitors/{owner}/topContributorsQueries"),
            Family::WorkloadInsights => format!("/workloadInsights/{owner}/topContributorsQueries"),
            Family::WorkloadInsightsData => {
                format!("/workloadInsights/{owner}/topContributorsDataQueries")
            }
        }
    }

    /// `StartQuery*` with `metric`, over a window every family accepts.
    async fn start(&self, family: Family, metric: &str) -> Answer {
        let body = json!({
            "startTime": "2026-10-22T19:07:07Z",
            "endTime": "2026-10-22T20:07:07Z",
            "metricName": metric,
            "destinationCategory": "UNCLASSIFIED"
        });
        self.emulator
            .call("POST", &self.base(family), Some(body))
            .await
    }

    /// `StartQuery*` with a metric the family's own metric enum declares, for the
    /// rows that are not about the metric.
    async fn start_any(&self, family: Family) -> Answer {
        match family {
            Family::Monitor => self.start(family, "ROUND_TRIP_TIME").await,
            _ => self.start(family, "RETRANSMISSIONS").await,
        }
    }

    async fn status(&self, family: Family, query_id: &str) -> Answer {
        let path = format!("{}/{query_id}/status", self.base(family));
        self.emulator.call("GET", &path, None).await
    }

    async fn results(&self, family: Family, query_id: &str) -> Answer {
        let path = format!("{}/{query_id}/results", self.base(family));
        self.emulator.call("GET", &path, None).await
    }

    async fn stop(&self, family: Family, query_id: &str) -> Answer {
        let path = format!("{}/{query_id}", self.base(family));
        self.emulator.call("DELETE", &path, None).await
    }

    /// A query of `family` in the terminal status `terminal`, and the id it was
    /// minted with: `SUCCEEDED` by letting the clock finish it, `CANCELED` by
    /// stopping it while it was still moving.
    async fn terminal_query(&self, family: Family, terminal: &str) -> String {
        let started = self.start_any(family).await;
        assert_eq!(
            started.status, 200,
            "case {family:?}/{terminal}: the query must start, got {} with body {}",
            started.status, started.body
        );
        let query_id = started.text("queryId");
        match terminal {
            "CANCELED" => {
                let stopped = self.stop(family, &query_id).await;
                assert_eq!(
                    stopped.status, 200,
                    "case {family:?}/{terminal}: the first stop must be accepted, got {} \
                     with body {}",
                    stopped.status, stopped.body
                );
            }
            _ => tokio::time::sleep(PAST_QUERY_COMPLETION).await,
        }
        let reported = self.status(family, &query_id).await;
        assert_eq!(
            reported.text("status"),
            terminal,
            "case {family:?}/{terminal}: the query must be in that status before the stop, \
             got {:?}",
            reported.text("status")
        );
        query_id
    }
}

/// RULE 1: a stop of a query that has already reached a terminal status is accepted
/// and leaves that status alone, however many times it is repeated.
///
/// The rows differ along the family (all three lifecycles), the terminal status the
/// query reached (`SUCCEEDED`, which the clock owes it, and `CANCELED`, which an
/// earlier stop left it in — the issue cited only the first) and the number of stops
/// issued, which is what an idempotent fire-and-forget has to tolerate.
#[tokio::test]
async fn a_stop_of_a_terminal_query_is_accepted_and_leaves_its_outcome_alone() {
    let plane = Plane::new().await;

    for family in FAMILIES {
        for terminal in ["SUCCEEDED", "CANCELED"] {
            let query_id = plane.terminal_query(*family, terminal).await;
            let case = format!("{family:?}/{terminal}");

            for attempt in 1..=3 {
                let stopped = plane.stop(*family, &query_id).await;
                assert_eq!(
                    stopped.status, 200,
                    "case {case:?} (stop {attempt}): expected 200, got {} with body {}",
                    stopped.status, stopped.body
                );
                // The output shape is EMPTY, so the answer carries no member of its
                // own — in particular no status, which is the query's to report.
                assert!(
                    stopped.body == Value::Null
                        || stopped.body.as_object().is_some_and(|body| body.is_empty()),
                    "case {case:?} (stop {attempt}): expected an empty body, got {}",
                    stopped.body
                );
                let after = plane.status(*family, &query_id).await;
                assert_eq!(
                    after.text("status"),
                    terminal,
                    "case {case:?} (stop {attempt}): the stop must not overwrite the \
                     terminal outcome, got {:?}",
                    after.text("status")
                );
            }

            // The clock does not move a terminal query either, so the outcome the
            // stops left alone is still the one a later reader sees.
            tokio::time::sleep(PAST_QUERY_COMPLETION).await;
            let settled = plane.status(*family, &query_id).await;
            assert_eq!(
                settled.text("status"),
                terminal,
                "case {case:?}: a terminal status never moves again, got {:?}",
                settled.text("status")
            );
        }
    }
}

/// RULE 1's positive control: the same stop on a query that is still moving DOES
/// cancel it, so the acceptance above is about the terminal status and not about the
/// stop having become a no-op everywhere.
#[tokio::test]
async fn a_stop_of_a_query_that_is_still_moving_cancels_it() {
    let plane = Plane::new().await;

    for family in FAMILIES {
        let started = plane.start_any(*family).await;
        assert_eq!(
            started.status, 200,
            "case {family:?}: the query must start, got {} with body {}",
            started.status, started.body
        );
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
            "case {family:?}: a query still moving must be canceled, got {:?}",
            after.text("status")
        );
    }
}

/// RULE 2: `unit` is reported by `GetQueryResultsMonitorTopContributors` only when
/// the result set has rows, because the model leaves that member OPTIONAL there.
///
/// The rows are every value of `MonitorMetric` — so no metric reports a unit for an
/// empty result set, not only the issue's `DATA_TRANSFERRED` — and every value of
/// `WorkloadInsightsMetric` through the sibling reader whose `unit` is `@required`,
/// which must carry it whatever the row count. The emulator observes no network
/// flow, so every window here is one with no rows in it.
#[tokio::test]
async fn an_optional_unit_is_reported_only_for_a_result_set_with_rows() {
    let plane = Plane::new().await;

    // Start every row, then wait once: results are reported for a query that has
    // SUCCEEDED, so the whole table crosses that line together.
    let mut started: Vec<(Family, &str, &str, String)> = Vec::new();
    for (metric, unit) in MONITOR_METRIC_UNITS {
        let answer = plane.start(Family::Monitor, metric).await;
        assert_eq!(
            answer.status, 200,
            "case Monitor/{metric:?}: the query must start, got {} with body {}",
            answer.status, answer.body
        );
        started.push((Family::Monitor, metric, unit, answer.text("queryId")));
    }
    for (metric, unit) in WORKLOAD_INSIGHTS_METRIC_UNITS {
        let answer = plane.start(Family::WorkloadInsightsData, metric).await;
        assert_eq!(
            answer.status, 200,
            "case WorkloadInsightsData/{metric:?}: the query must start, got {} with body {}",
            answer.status, answer.body
        );
        started.push((
            Family::WorkloadInsightsData,
            metric,
            unit,
            answer.text("queryId"),
        ));
    }

    tokio::time::sleep(PAST_QUERY_COMPLETION).await;

    for (family, metric, unit, query_id) in &started {
        let case = format!("{family:?}/{metric}");
        let results = plane.results(*family, query_id).await;
        assert_eq!(
            results.status, 200,
            "case {case:?}: expected 200 from the results read, got {} with body {}",
            results.status, results.body
        );

        match family {
            Family::Monitor => {
                assert_eq!(
                    results.member("topContributors"),
                    &json!([]),
                    "case {case:?}: expected no rows in a window with no observed flow, got {}",
                    results.member("topContributors")
                );
                assert_eq!(
                    results.member("unit"),
                    &Value::Null,
                    "case {case:?}: expected no unit beside an empty row set, got {}",
                    results.member("unit")
                );
            }
            _ => {
                // `unit` is `@required` on this reader, so it is reported whatever
                // the row count — and from the metric the query was STARTED with.
                assert_eq!(
                    results.member("datapoints"),
                    &json!([]),
                    "case {case:?}: expected no datapoints in a window with no observed flow, \
                     got {}",
                    results.member("datapoints")
                );
                assert_eq!(
                    results.text("unit"),
                    *unit,
                    "case {case:?}: expected the required unit {unit:?}, got {:?}",
                    results.text("unit")
                );
            }
        }
    }
}

/// RULE 3: a `queryId` that names no query is reported by every operation of the
/// query plane in one wording, built from the request's own id.
///
/// The rows differ along the family, the operation (the results reader, which models
/// `ResourceNotFoundException`, and the status reader, which does not), and the form
/// of the id: an id of the shape this service mints that was never minted, an
/// upper-case rendering of one, a value that is not UUID-shaped at all, and a single
/// character. None of them is the id of the real query the plane holds, so every
/// refusal is about the id.
#[tokio::test]
async fn a_query_id_that_names_no_query_is_reported_in_one_wording() {
    let plane = Plane::new().await;

    // A real query exists, so a refusal is about the id and not about an empty
    // store.
    let known = plane.start_any(Family::Monitor).await.text("queryId");

    for case in [
        "00000000-0000-4000-9000-0000000fffff",
        "1AB289AB-C68B-44F5-96C1-B5538628093D",
        "1ab289ab-c68b-44f5-96c1-b5538628093d",
        "not-a-query-id",
        "0",
    ] {
        assert_ne!(
            case, known,
            "the unknown-id table must not name the query the plane holds"
        );
        let expected = format!("There's no query with id {{{case}}}");

        for family in FAMILIES {
            // The results readers all model `ResourceNotFoundException` (HTTP 404),
            // so an id naming no query is reported there as the absence it is.
            let results = plane.results(*family, case).await;
            assert_eq!(
                results.status, 404,
                "case {family:?}/{case:?}: expected 404 from the results read, got {} \
                 with body {}",
                results.status, results.body
            );
            assert!(
                results.is_error("ResourceNotFoundException"),
                "case {family:?}/{case:?}: expected ResourceNotFoundException, got {:?} / \
                 body {}",
                results.error_type,
                results.body
            );
            assert_eq!(
                results.text("message"),
                expected,
                "case {family:?}/{case:?}: expected the message {expected:?}, got {:?}",
                results.text("message")
            );

            // `GetQueryStatus*` models no 404, so the same condition is that
            // operation's modelled 400 — worded the same way.
            let status = plane.status(*family, case).await;
            assert_eq!(
                status.status, 400,
                "case {family:?}/{case:?}: expected 400 from the status read, got {} \
                 with body {}",
                status.status, status.body
            );
            assert!(
                status.is_error("ValidationException"),
                "case {family:?}/{case:?}: expected ValidationException, got {:?} / body {}",
                status.error_type,
                status.body
            );
            assert_eq!(
                status.text("message"),
                expected,
                "case {family:?}/{case:?}: expected the message {expected:?}, got {:?}",
                status.text("message")
            );
        }
    }
}

/// RULE 3's complement for an update that contradicts itself: every update of this
/// service refuses one item named in both an add list and the matching remove list
/// with the modelled 400 in one wording, and leaves the record exactly as it was.
///
/// The rows differ along the operation and the pair of lists the contradiction sits
/// in: `UpdateMonitor`'s local pair (the issue's row) and its remote pair, and
/// `UpdateScope`'s target pair.
#[tokio::test]
async fn an_update_that_adds_and_removes_one_item_is_refused_in_one_wording() {
    let emulator = Emulator::new();
    let expected = "invalid input -> (detected overlap between add/remove resources)";

    let scope = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-west-2")]) }))
        .await;
    assert_eq!(
        scope.status, 200,
        "the scope must be created, got {} with body {}",
        scope.status, scope.body
    );
    let scope_id = scope.text("scopeId");
    let scope_arn = scope.text("scopeArn");

    let monitor_name = "overlap-worded-monitor";
    let local = json!([{ "type": "AWS::Region", "identifier": "us-west-2" }]);
    let remote = json!([{ "type": "AWS::AWSService", "identifier": "S3" }]);
    let created = emulator
        .create_monitor(json!({
            "monitorName": monitor_name,
            "scopeArn": scope_arn,
            "localResources": local,
            "remoteResources": remote
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "the monitor must be created, got {} with body {}",
        created.status, created.body
    );

    let rows: Vec<(&str, Answer)> = vec![
        (
            "UpdateMonitor, the local pair",
            emulator
                .update_monitor(
                    monitor_name,
                    json!({
                        "localResourcesToAdd": local,
                        "localResourcesToRemove": local
                    }),
                )
                .await,
        ),
        (
            "UpdateMonitor, the remote pair",
            emulator
                .update_monitor(
                    monitor_name,
                    json!({
                        "remoteResourcesToAdd": remote,
                        "remoteResourcesToRemove": remote
                    }),
                )
                .await,
        ),
        (
            "UpdateScope, the target pair",
            emulator
                .update_scope(
                    &scope_id,
                    json!({
                        "resourcesToAdd": targets(&[("123456789012", "us-west-2")]),
                        "resourcesToDelete": targets(&[("123456789012", "us-west-2")])
                    }),
                )
                .await,
        ),
    ];

    for (case, answer) in &rows {
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.is_error("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
        assert_eq!(
            answer.text("message"),
            expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            answer.text("message")
        );
    }

    // Nothing the refused updates named moved.
    let monitor = emulator.get_monitor(monitor_name).await;
    assert_eq!(
        monitor.member("localResources"),
        &local,
        "a refused update must leave localResources alone, got {}",
        monitor.member("localResources")
    );
    assert_eq!(
        monitor.member("remoteResources"),
        &remote,
        "a refused update must leave remoteResources alone, got {}",
        monitor.member("remoteResources")
    );
    let read = emulator.get_scope(&scope_id).await;
    assert_eq!(
        read.member("targets"),
        &targets(&[("123456789012", "us-west-2")]),
        "a refused update must leave the scope's targets alone, got {}",
        read.member("targets")
    );
}
