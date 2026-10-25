// SPDX-License-Identifier: CC0-1.0

//! An SLO's evaluation kind is fixed when it is created.
//!
//! `UpdateServiceLevelObjective`'s own model documentation states the rule: "You cannot
//! change from a period-based SLO to a request-based SLO, or change from a request-based SLO
//! to a period-based SLO." So an update whose SLI config is of the OTHER kind than the stored
//! record is refused with the modelled `ValidationException` (HTTP 400) and writes nothing,
//! while an update whose config is of the SAME kind replaces that indicator and leaves the
//! kind alone.
//!
//! Every request goes through the mounted `restJson1` router, so the statuses and bodies are
//! the ones a client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::slo_lifecycle::{call, create, router, sli_config};

/// A request-based SLI config on an Application Signals service entity.
fn request_based_sli_config(service_name: &str) -> Value {
    json!({
        "RequestBasedSliMetricConfig": {
            "KeyAttributes": { "Type": "Service", "Name": service_name, "Environment": "prod" },
            "MetricType": "AVAILABILITY"
        }
    })
}

/// A request-based SLI config that spells its own CloudWatch metric-math queries rather than
/// naming an Application Signals entity: a different FORM of the same kind of config.
fn request_based_sli_config_with_queries() -> Value {
    json!({
        "RequestBasedSliMetricConfig": {
            "TotalRequestCountMetric": [{
                "Id": "total",
                "MetricStat": {
                    "Metric": { "Namespace": "AWS/ApiGateway", "MetricName": "Count" },
                    "Period": 60,
                    "Stat": "Sum"
                }
            }],
            "MonitoredRequestCountMetric": {
                "GoodCountMetric": [{
                    "Id": "good",
                    "MetricStat": {
                        "Metric": { "Namespace": "AWS/ApiGateway", "MetricName": "2XXCount" },
                        "Period": 60,
                        "Stat": "Sum"
                    }
                }]
            }
        }
    })
}

/// A period-based SLI config that spells its own metric queries instead of naming an entity.
fn sli_config_with_queries() -> Value {
    json!({
        "SliMetricConfig": {
            "MetricDataQueries": [{
                "Id": "errors",
                "MetricStat": {
                    "Metric": { "Namespace": "AWS/ApiGateway", "MetricName": "5XXError" },
                    "Period": 60,
                    "Stat": "Sum"
                }
            }]
        },
        "MetricThreshold": 1.0,
        "ComparisonOperator": "LessThan"
    })
}

/// Create one SLO and return the stored record.
async fn create_slo(router: &MultiServiceRouter, name: &str, config_member: &str, config: Value) -> Value {
    let (status, created) = create(
        router,
        json!({ "Name": name, "Description": "before", config_member: config }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the setup create of {name:?} to succeed, got {status} with {created}"
    );
    created["Slo"].clone()
}

/// Read a stored SLO back.
async fn get_slo(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, fetched) = call(router, "GET", &format!("/slo/{name}"), None).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 reading {name:?} back, got {status} with {fetched}"
    );
    fetched["Slo"].clone()
}

/// An update that supplies the SLI config of the other kind is refused with a 400
/// `ValidationException`, and the stored record is left exactly as it was.
///
/// The rows differ along the stored kind, the form of the offending config (an entity-named
/// config and a metric-query one), and which other members ride along in the same request —
/// none of which may be applied, since the refused call writes nothing at all.
#[tokio::test]
async fn an_update_that_switches_the_evaluation_kind_is_refused() {
    struct Case {
        label: &'static str,
        /// The member the setup create supplies, and its config.
        created_with: (&'static str, Value),
        /// The patch body that tries to switch the kind.
        patch: Value,
    }

    let cases = [
        Case {
            label: "period-based record, request-based config on an entity",
            created_with: ("SliConfig", sli_config("checkout", "POST /checkout", 250.0)),
            patch: json!({ "RequestBasedSliConfig": request_based_sli_config("checkout") }),
        },
        Case {
            label: "period-based record, request-based config with its own queries",
            created_with: ("SliConfig", sli_config("checkout", "POST /checkout", 250.0)),
            patch: json!({ "RequestBasedSliConfig": request_based_sli_config_with_queries() }),
        },
        Case {
            label: "period-based record built from queries, request-based config",
            created_with: ("SliConfig", sli_config_with_queries()),
            patch: json!({ "RequestBasedSliConfig": request_based_sli_config("checkout") }),
        },
        Case {
            label: "request-based record, period-based config on an entity",
            created_with: ("RequestBasedSliConfig", request_based_sli_config("checkout")),
            patch: json!({ "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
        },
        Case {
            label: "request-based record, period-based config with its own queries",
            created_with: ("RequestBasedSliConfig", request_based_sli_config("checkout")),
            patch: json!({ "SliConfig": sli_config_with_queries() }),
        },
        Case {
            label: "request-based record, switch bundled with other members",
            created_with: ("RequestBasedSliConfig", request_based_sli_config_with_queries()),
            patch: json!({
                "SliConfig": sli_config("checkout", "GET /cart", 100.0),
                "Description": "after",
                "Goal": { "AttainmentGoal": 95.0 },
                "BurnRateConfigurations": [{ "LookBackWindowMinutes": 60 }],
                "AutoInvestigationEnabled": true
            }),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let router = router();
        let name = format!("fixed-kind-{index:02}");
        let before = create_slo(
            &router,
            &name,
            case.created_with.0,
            case.created_with.1.clone(),
        )
        .await;

        let (status, error) =
            call(&router, "PATCH", &format!("/slo/{name}"), Some(case.patch.clone())).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for an update that switches the evaluation kind, got {status} with {error}",
            case.label
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ValidationException"),
            "case {:?}: expected a ValidationException, got {error}",
            case.label
        );

        let after = get_slo(&router, &name).await;
        assert_eq!(
            after, before,
            "case {:?}: a refused update must leave the stored record untouched, expected {before:?}, got {after:?}",
            case.label
        );
    }
}

/// An update whose SLI config is of the SAME kind as the stored record replaces that
/// indicator, keeps the record's `EvaluationType`, never grows the indicator of the other
/// kind, and a following Get shows the replacement.
///
/// The rows differ along the stored kind and the form of the replacing config, so the
/// invariant asserted is "the kind survives any update of its own kind", not one example.
#[tokio::test]
async fn an_update_of_the_same_kind_replaces_the_indicator_and_keeps_the_kind() {
    struct Case {
        label: &'static str,
        created_with: (&'static str, Value),
        patch: Value,
        evaluation_type: &'static str,
        /// The indicator member the record must carry, and the one it must not.
        indicator: &'static str,
        other_indicator: &'static str,
    }

    let cases = [
        Case {
            label: "period-based replaced by another entity config",
            created_with: ("SliConfig", sli_config("checkout", "POST /checkout", 250.0)),
            patch: json!({ "SliConfig": sli_config("checkout", "GET /cart", 90.0) }),
            evaluation_type: "PeriodBased",
            indicator: "Sli",
            other_indicator: "RequestBasedSli",
        },
        Case {
            label: "period-based replaced by a metric-query config",
            created_with: ("SliConfig", sli_config("checkout", "POST /checkout", 250.0)),
            patch: json!({ "SliConfig": sli_config_with_queries() }),
            evaluation_type: "PeriodBased",
            indicator: "Sli",
            other_indicator: "RequestBasedSli",
        },
        Case {
            label: "request-based replaced by a metric-query config",
            created_with: ("RequestBasedSliConfig", request_based_sli_config("checkout")),
            patch: json!({ "RequestBasedSliConfig": request_based_sli_config_with_queries() }),
            evaluation_type: "RequestBased",
            indicator: "RequestBasedSli",
            other_indicator: "Sli",
        },
        Case {
            label: "request-based replaced by an entity config",
            created_with: ("RequestBasedSliConfig", request_based_sli_config_with_queries()),
            patch: json!({ "RequestBasedSliConfig": request_based_sli_config("payments") }),
            evaluation_type: "RequestBased",
            indicator: "RequestBasedSli",
            other_indicator: "Sli",
        },
        Case {
            label: "period-based untouched by an update that supplies no config",
            created_with: ("SliConfig", sli_config("checkout", "POST /checkout", 250.0)),
            patch: json!({ "Description": "after" }),
            evaluation_type: "PeriodBased",
            indicator: "Sli",
            other_indicator: "RequestBasedSli",
        },
        Case {
            label: "request-based untouched by an update that supplies no config",
            created_with: ("RequestBasedSliConfig", request_based_sli_config("checkout")),
            patch: json!({ "Description": "after" }),
            evaluation_type: "RequestBased",
            indicator: "RequestBasedSli",
            other_indicator: "Sli",
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let router = router();
        let name = format!("same-kind-{index:02}");
        let before = create_slo(
            &router,
            &name,
            case.created_with.0,
            case.created_with.1.clone(),
        )
        .await;
        assert_eq!(
            before["EvaluationType"].as_str(),
            Some(case.evaluation_type),
            "case {:?}: expected the created record to be {:?}, got {:?}",
            case.label,
            case.evaluation_type,
            before["EvaluationType"]
        );

        let (status, updated) =
            call(&router, "PATCH", &format!("/slo/{name}"), Some(case.patch.clone())).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 for an update of the stored kind, got {status} with {updated}",
            case.label
        );
        let after = updated["Slo"].clone();

        assert_eq!(
            after["EvaluationType"].as_str(),
            Some(case.evaluation_type),
            "case {:?}: expected EvaluationType to stay {:?}, got {:?}",
            case.label,
            case.evaluation_type,
            after["EvaluationType"]
        );
        assert!(
            !after[case.indicator].is_null(),
            "case {:?}: expected the record to carry {}, got {:?}",
            case.label,
            case.indicator,
            after[case.indicator]
        );
        assert!(
            after[case.other_indicator].is_null(),
            "case {:?}: expected no {} on a {} record, got {:?}",
            case.label,
            case.other_indicator,
            case.evaluation_type,
            after[case.other_indicator]
        );

        let fetched = get_slo(&router, &name).await;
        assert_eq!(
            fetched, after,
            "case {:?}: a Get after the update must answer the updated record, got {fetched:?}",
            case.label
        );
    }
}

/// An update that names NEITHER SLI config changes no indicator and no kind: the record's
/// evaluation kind is the one its create fixed, whatever else the update writes.
///
/// Both kinds are rows, since the rule is about the kind the record HAS. (A create that
/// names neither config defines no indicator at all and is refused — see
/// `slo_indicator_required.rs`.)
#[tokio::test]
async fn an_update_naming_no_config_leaves_the_indicator_and_the_kind_alone() {
    let rows = [
        (
            "SliConfig",
            sli_config("checkout", "GET /cart", 100.0),
            "PeriodBased",
            "Sli",
        ),
        (
            "RequestBasedSliConfig",
            request_based_sli_config("checkout"),
            "RequestBased",
            "RequestBasedSli",
        ),
    ];

    for (index, (member, config, evaluation_type, indicator)) in rows.into_iter().enumerate() {
        let router = router();
        let name = format!("configless-update-slo-{index:02}");
        let before = create_slo(&router, &name, member, config).await;

        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/slo/{name}"),
            Some(json!({ "Description": "after" })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {member:?}: expected HTTP 200 from an update naming no SLI config, got {status} with {updated}"
        );
        assert_eq!(
            updated["Slo"]["EvaluationType"].as_str(),
            Some(evaluation_type),
            "case {member:?}: expected EvaluationType {evaluation_type:?} to survive, got {:?}",
            updated["Slo"]["EvaluationType"]
        );
        assert_eq!(
            updated["Slo"][indicator], before[indicator],
            "case {member:?}: expected {indicator} to survive unchanged, got {:?}",
            updated["Slo"][indicator]
        );

        let fetched = get_slo(&router, &name).await;
        assert_eq!(
            fetched[indicator], before[indicator],
            "case {member:?}: expected the stored record to keep {indicator}, got {fetched:?}"
        );
    }
}
