// SPDX-License-Identifier: CC0-1.0

//! Which `MetricName` values a widget query's `metrics` admits, end to end over the wire.
//!
//! One rule is under test, applied wherever a widget list is submitted — `CreateDashboard`
//! and `UpdateDashboard`: a query's `metrics` may name only the `MetricName` values the
//! member carrying that query documents, and a write naming any other is refused with
//! `ValidationException` (HTTP 400) and stores nothing.
//!
//! `metrics` targets `MetricNames` in every query that models it, whose member is the
//! `MetricName` enum holding the UNION of the three documented sets
//! (`smithy_query(query="shape",shape="MetricName,MetricNames")`), so nothing at the
//! boundary can state the rule: which part of that union applies is decided by the member of
//! the `QueryParameters` union carrying the query.
//!
//! The refusal rows are the whole cross product of the eleven `MetricName` values with the
//! three query members that model `metrics`, keeping the pairs the documentation does not
//! admit, and they differ along which member carries the query, which value is asked for,
//! whether the bad value sits first or second in the list, how deep in the two nested lists
//! the query sits and which operation submits it. The acceptance rows are the other side of
//! the same product — every documented pair, one metric at a time and all of a member's
//! metrics at once — plus the two queries that model no `metrics` member at all.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use serde_json::{json, Value};

use super::dashboard_lifecycle::{call, error_code, graph_config, ok_call, router};

/// A well-formed `DashboardArn` nothing is stored under: a widget list is judged before the
/// dashboard it names is resolved, so a refused row needs no stored dashboard.
const UNKNOWN_DASHBOARD_ARN: &str = "arn:aws:bcm-dashboards::000000000000:dashboard/never-created";

/// Every value the `MetricName` enum declares, in the order the model lists them
/// (`smithy_query(query="shape",shape="MetricName")`): the union a boundary validator holds
/// a submitted metric to, and the catalog each query's own set is a part of.
const EVERY_METRIC: [&str; 11] = [
    "AmortizedCost",
    "BlendedCost",
    "NetAmortizedCost",
    "NetUnblendedCost",
    "NormalizedUsageAmount",
    "UnblendedCost",
    "UsageQuantity",
    "SpendCoveredBySavingsPlans",
    "Hour",
    "Unit",
    "Cost",
];

/// One member of the `QueryParameters` union that models a `metrics` list: the member name,
/// the shape its documentation names the rule under, and the `MetricName` values that
/// documentation admits.
struct QueryKind {
    member: &'static str,
    shape: &'static str,
    documented: &'static [&'static str],
}

/// The three query members that model `metrics`, with the values each one documents.
///
/// Sources, each the note on that member (`smithy_query(query="shape",
/// shape="CostAndUsageQuery,SavingsPlansCoverageQuery,ReservationCoverageQuery")`):
/// "Valid values for CostAndUsageQuery metrics are AmortizedCost, BlendedCost,
/// NetAmortizedCost, NetUnblendedCost, NormalizedUsageAmount, UnblendedCost, and
/// UsageQuantity"; "Valid value for SavingsPlansCoverageQuery metrics is
/// SpendCoveredBySavingsPlans"; "Valid values for ReservationCoverageQuery metrics are Hour,
/// Unit, and Cost".
const QUERY_KINDS: [QueryKind; 3] = [
    QueryKind {
        member: "costAndUsage",
        shape: "CostAndUsageQuery",
        documented: &[
            "AmortizedCost",
            "BlendedCost",
            "NetAmortizedCost",
            "NetUnblendedCost",
            "NormalizedUsageAmount",
            "UnblendedCost",
            "UsageQuantity",
        ],
    },
    QueryKind {
        member: "savingsPlansCoverage",
        shape: "SavingsPlansCoverageQuery",
        documented: &["SpendCoveredBySavingsPlans"],
    },
    QueryKind {
        member: "reservationCoverage",
        shape: "ReservationCoverageQuery",
        documented: &["Hour", "Unit", "Cost"],
    },
];

/// A relative time range every rule admits, so a row only ever fails on its metrics.
fn time_range() -> Value {
    json!({
        "startTime": {"type": "RELATIVE", "value": "-P3M"},
        "endTime": {"type": "RELATIVE", "value": "P0D"}
    })
}

/// A widget config whose query is `member` and whose `metrics` is `metrics`, displayed as a
/// table (a display config carrying no metric of its own).
fn config_asking_for(member: &str, metrics: &[&str]) -> Value {
    json!({
        "queryParameters": {
            member: {
                "metrics": metrics,
                "timeRange": time_range(),
                "granularity": "MONTHLY"
            }
        },
        "displayConfig": {"table": {}}
    })
}

/// A widget config whose query is `member` and submits no `metrics` member at all — the
/// form the two utilization queries take, which document no metric.
fn config_without_metrics(member: &str) -> Value {
    json!({
        "queryParameters": {
            member: {"timeRange": time_range(), "granularity": "MONTHLY"}
        },
        "displayConfig": {"table": {}}
    })
}

/// The path of element `metric_position` of the `metrics` of config `config_position` of
/// widget `widget_position`, under the union member `member`, as the framework spells a
/// member path.
fn metric_path(
    widget_position: usize,
    config_position: usize,
    member: &str,
    metric_position: usize,
) -> String {
    format!(
        "widgets.{widget_position}.member.configs.{config_position}.member.\
         queryParameters.{member}.metrics.{metric_position}.member"
    )
}

/// `values` as a documented list reads them. An independent oracle of the clause the
/// refusal publishes, written out here rather than imported.
fn in_prose(values: &[&str]) -> String {
    match values {
        [] => String::new(),
        [only] => (*only).to_string(),
        [first, last] => format!("{first} and {last}"),
        [leading @ .., last] => format!("{}, and {last}", leading.join(", ")),
    }
}

/// How a refusal words a metric the query carrying it does not admit.
fn metric_message(path: &str, metric: &str, kind: &QueryKind) -> String {
    let documented = if kind.documented.len() == 1 {
        format!(
            "valid value for {} metrics is {}",
            kind.shape,
            in_prose(kind.documented)
        )
    } else {
        format!(
            "valid values for {} metrics are {}",
            kind.shape,
            in_prose(kind.documented)
        )
    };
    format!(
        "{path}: '{metric}' is not a valid metric for a {} query; {documented}",
        kind.member
    )
}

/// One row of the refusal table.
struct RefusalCase {
    label: String,
    operation: &'static str,
    request: Value,
    /// The whole `message` the refusal must carry.
    message: String,
}

fn refusal_cases() -> Vec<RefusalCase> {
    let mut cases = Vec::new();

    // The cross product of every `MetricName` value with every query member that models
    // `metrics`, keeping the pairs that member's documentation does not admit. Rows
    // alternate between submitting the bad value ALONE (element 1) and submitting it AFTER
    // a documented one (element 2, which the refusal has to name), and between the two
    // operations that submit a widget list.
    let mut row = 0usize;
    for kind in &QUERY_KINDS {
        for metric in EVERY_METRIC {
            if kind.documented.contains(&metric) {
                continue;
            }
            let after_a_valid_one = row % 2 == 1;
            let (metrics, metric_position) = if after_a_valid_one {
                (vec![kind.documented[0], metric], 2)
            } else {
                (vec![metric], 1)
            };
            let operation = if row % 3 == 2 {
                "UpdateDashboard"
            } else {
                "CreateDashboard"
            };
            let widgets = json!([{
                "title": "Spend",
                "configs": [config_asking_for(kind.member, &metrics)]
            }]);
            let request = if operation == "UpdateDashboard" {
                json!({
                    "arn": UNKNOWN_DASHBOARD_ARN,
                    "name": "Metric Mismatch",
                    "widgets": widgets
                })
            } else {
                json!({"name": format!("Metric Mismatch {row}"), "widgets": widgets})
            };
            cases.push(RefusalCase {
                label: format!(
                    "{operation}: a {} query asking for {metric} at element \
                     {metric_position}",
                    kind.member
                ),
                operation,
                request,
                message: metric_message(
                    &metric_path(1, 1, kind.member, metric_position),
                    metric,
                    kind,
                ),
            });
            row += 1;
        }
    }

    // The rule reaches a query however deep it sits: the second config of the second
    // widget, beside configs every rule admits.
    cases.push(RefusalCase {
        label: "the second config of the second widget".to_string(),
        operation: "CreateDashboard",
        request: json!({
            "name": "Deep Metric Mismatch",
            "widgets": [
                {
                    "title": "First",
                    "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
                },
                {
                    "title": "Second",
                    "configs": [
                        config_asking_for("reservationCoverage", &["Unit"]),
                        config_asking_for("savingsPlansCoverage", &["UsageQuantity"])
                    ]
                }
            ]
        }),
        message: metric_message(
            &metric_path(2, 2, "savingsPlansCoverage", 1),
            "UsageQuantity",
            &QUERY_KINDS[1],
        ),
    });

    // A list whose THIRD entry is the bad one, the two before it documented: the refusal
    // names the element that broke the rule, not the member.
    cases.push(RefusalCase {
        label: "the third metric of a reservation-coverage list".to_string(),
        operation: "CreateDashboard",
        request: json!({
            "name": "Third Metric Mismatch",
            "widgets": [{
                "title": "Coverage",
                "configs": [config_asking_for(
                    "reservationCoverage",
                    &["Hour", "Unit", "NetUnblendedCost"],
                )]
            }]
        }),
        message: metric_message(
            &metric_path(1, 1, "reservationCoverage", 3),
            "NetUnblendedCost",
            &QUERY_KINDS[2],
        ),
    });

    cases
}

/// Every row is refused with HTTP 400 `ValidationException`, carrying the message the
/// documented rule states.
#[tokio::test]
async fn a_metric_outside_its_query_is_refused() {
    let cases = refusal_cases();
    assert!(
        cases.len() >= 22,
        "case the refusal table: the cross product must keep every undocumented pair, got \
         {} rows",
        cases.len()
    );
    for case in cases {
        let router = router();
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 from {}, got {status}: {body}",
            case.label, case.operation
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {:?}: expected ValidationException, got {:?}: {body}",
            case.label,
            error_code(&body)
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>");
        assert_eq!(
            message, case.message,
            "case {:?}: expected the refusal to state its rule, got {message:?}",
            case.label
        );
    }
}

/// A refused write stores nothing: no dashboard of the account's own is created, and a
/// refused update leaves the whole stored record as it was.
#[tokio::test]
async fn a_refused_write_changes_no_state() {
    let router = router();

    for case in refusal_cases() {
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: the row must be refused for this test to mean anything, got \
             {status}: {body}",
            case.label
        );
    }

    let listed = ok_call(&router, "ListDashboards", &json!({"maxResults": 100})).await;
    let own: Vec<&Value> = listed["dashboards"]
        .as_array()
        .expect("dashboards")
        .iter()
        .filter(|item| item["type"].as_str() == Some("CUSTOM"))
        .collect();
    assert!(
        own.is_empty(),
        "the refused creates must have stored no dashboard, got {own:?}"
    );

    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "Untouched By Metrics",
            "widgets": [{
                "title": "Spend",
                "configs": [config_asking_for("costAndUsage", &["UnblendedCost"])]
            }]
        }),
    )
    .await;
    let arn = created["arn"].as_str().expect("an arn").to_string();
    let before = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
    let (status, body) = call(
        &router,
        "UpdateDashboard",
        &json!({
            "arn": arn,
            "name": "Untouched By Metrics",
            "widgets": [{
                "title": "Spend",
                "configs": [config_asking_for("costAndUsage", &["Hour"])]
            }]
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a refused update: expected HTTP 400, got {status}: {body}"
    );
    let after = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
    assert_eq!(
        after, before,
        "case a refused update: the whole record must be untouched, got {after} after \
         {before}"
    );
}

/// Every documented pair is accepted and reads back verbatim: each metric a query admits on
/// its own, and all of a query's metrics in one list — on the create path and on the update
/// path. The two queries that model no `metrics` member are accepted as they are.
#[tokio::test]
async fn every_documented_metric_round_trips() {
    let router = router();

    // (label, the query member, the metrics the config asks for)
    let mut accepted: Vec<(String, &'static str, Vec<&'static str>)> = Vec::new();
    for kind in &QUERY_KINDS {
        for metric in kind.documented {
            accepted.push((
                format!("a {} query asking for {metric}", kind.member),
                kind.member,
                vec![*metric],
            ));
        }
        accepted.push((
            format!("a {} query asking for every metric it documents", kind.member),
            kind.member,
            kind.documented.to_vec(),
        ));
    }

    for (index, (label, member, metrics)) in accepted.into_iter().enumerate() {
        let name = format!("Accepted Metrics {index}");
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": name,
                "widgets": [{
                    "title": "Spend",
                    "configs": [config_asking_for(member, &metrics)]
                }]
            }),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: CreateDashboard answered no arn: {created}"))
            .to_string();

        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        let stored = &fetched["widgets"][0]["configs"][0]["queryParameters"][member]["metrics"];
        assert_eq!(
            stored,
            &json!(metrics),
            "case {label:?}: the stored metrics must be the submitted ones, got {stored}"
        );

        // The same list on the update path, replacing the widget list.
        ok_call(
            &router,
            "UpdateDashboard",
            &json!({
                "arn": arn,
                "name": name,
                "widgets": [{
                    "title": "Updated Spend",
                    "configs": [config_asking_for(member, &metrics)]
                }]
            }),
        )
        .await;
        let updated = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        let stored = &updated["widgets"][0]["configs"][0]["queryParameters"][member]["metrics"];
        assert_eq!(
            stored,
            &json!(metrics),
            "case {label:?}: the updated metrics must be the submitted ones, got {stored}"
        );
    }

    // The two queries that model no `metrics` member document no metric, so there is none
    // of theirs to judge: a config carrying one of them is stored as submitted.
    for member in ["savingsPlansUtilization", "reservationUtilization"] {
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": format!("No Metrics {member}"),
                "widgets": [{"title": "Utilization", "configs": [config_without_metrics(member)]}]
            }),
        )
        .await;
        let arn = created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {member:?}: CreateDashboard answered no arn: {created}"))
            .to_string();
        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        let stored = &fetched["widgets"][0]["configs"][0]["queryParameters"][member];
        assert_eq!(
            stored["timeRange"],
            time_range(),
            "case {member:?}: the query must be stored as submitted, got {stored}"
        );
        assert!(
            stored.get("metrics").is_none(),
            "case {member:?}: a query modelling no metrics must store none, got {stored}"
        );
    }
}
