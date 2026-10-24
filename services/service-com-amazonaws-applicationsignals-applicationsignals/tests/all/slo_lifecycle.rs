// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the service level objective (SLO) lifecycle over HTTP:
//! `CreateServiceLevelObjective`, `GetServiceLevelObjective`,
//! `ListServiceLevelObjectives`, `UpdateServiceLevelObjective` and
//! `DeleteServiceLevelObjective`.
//!
//! Every test drives the real `restJson1` surface through the router that production
//! mounts, so the routes, the status codes and the JSON bodies are the ones a client sees.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service and names the region the
/// minted ARN must carry. Signature verification is off (see `set_request_authentication`),
/// so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every minted ARN must name.
pub(crate) const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN carries (`engine::emulator_account_id`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// A router with just this service mounted on a fresh engine.
///
/// `pub(crate)` so the other suites in this test target drive the same mounted router
/// rather than re-deriving a harness of their own.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
pub(crate) async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("serialize the request body")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://application-signals.{REGION}.amazonaws.com{uri}"))
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A period-based `SliConfig` on an Application Signals service operation.
pub(crate) fn sli_config(service_name: &str, operation: &str, threshold: f64) -> Value {
    json!({
        "SliMetricConfig": {
            "KeyAttributes": { "Type": "Service", "Name": service_name, "Environment": "prod" },
            "OperationName": operation,
            "MetricType": "LATENCY",
            "PeriodSeconds": 60
        },
        "MetricThreshold": threshold,
        "ComparisonOperator": "LessThan"
    })
}

/// Create one SLO and return the `Slo` the service answers with.
pub(crate) async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/slo", Some(body)).await
}

/// The SLO names a `ListServiceLevelObjectives` page carries, in order.
fn summary_names(page: &Value) -> Vec<String> {
    page["SloSummaries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A create persists the SLO and answers the stored record, for every name the modelled
/// `ServiceLevelObjectiveName` pattern admits and for both kinds of SLI config; a Get by
/// name and by ARN answers the IDENTICAL record.
///
/// The rows differ along what the rule generalises over: the name's form, which of the two
/// mutually exclusive SLI configs was supplied (hence the derived `EvaluationType`), and
/// which source members the config names (hence the derived `MetricSourceType`).
#[tokio::test]
async fn create_persists_each_slo_and_get_answers_the_stored_record() {
    // Names drawn from the modelled pattern `^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$`
    // (`smithy_query(shape="ServiceLevelObjective")`, member `Name`): the leading and
    // trailing characters are alphanumeric and the middle may carry `-`, `.`, `_`.
    struct Case {
        name: &'static str,
        body: Value,
        evaluation_type: &'static str,
        metric_source_type: &'static str,
    }

    let cases = vec![
        Case {
            name: "checkout-latency",
            body: json!({
                "Name": "checkout-latency",
                "Description": "p99 latency of checkout",
                "SliConfig": sli_config("checkout", "POST /checkout", 250.0),
                "Goal": { "AttainmentGoal": 99.9 }
            }),
            // SliConfig was supplied -> PeriodBased; the config names an operation.
            evaluation_type: "PeriodBased",
            metric_source_type: "ServiceOperation",
        },
        Case {
            name: "a.b_c-9",
            body: json!({
                "Name": "a.b_c-9",
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": { "Type": "Service", "Name": "orders", "Environment": "prod" },
                        "MetricType": "AVAILABILITY"
                    }
                }
            }),
            // No operation and no dependency, but a service is named -> Service.
            evaluation_type: "PeriodBased",
            metric_source_type: "Service",
        },
        Case {
            name: "orders availability",
            body: json!({
                "Name": "orders availability",
                "RequestBasedSliConfig": {
                    "RequestBasedSliMetricConfig": {
                        "KeyAttributes": { "Type": "Service", "Name": "orders", "Environment": "prod" },
                        "DependencyConfig": {
                            "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "payments" },
                            "DependencyOperationName": "Charge"
                        }
                    }
                }
            }),
            // RequestBasedSliConfig was supplied -> RequestBased; a dependency is named.
            evaluation_type: "RequestBased",
            metric_source_type: "ServiceDependency",
        },
        Case {
            name: "raw-cloudwatch-metric",
            body: json!({
                "Name": "raw-cloudwatch-metric",
                "SliConfig": {
                    "SliMetricConfig": {
                        "MetricDataQueries": [{
                            "Id": "m1",
                            "MetricStat": {
                                "Metric": { "Namespace": "AWS/Lambda", "MetricName": "Duration" },
                                "Period": 60,
                                "Stat": "p99"
                            },
                            "ReturnData": true
                        }]
                    },
                    "MetricThreshold": 500.0,
                    "ComparisonOperator": "GreaterThan"
                }
            }),
            // No Application Signals entity at all -> plain CloudWatch metrics.
            evaluation_type: "PeriodBased",
            metric_source_type: "CloudWatchMetric",
        },
    ];

    let router = router();
    for case in &cases {
        let (status, created) = create(&router, case.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from CreateServiceLevelObjective, got {status} with {created}",
            case.name
        );
        let slo = &created["Slo"];

        let expected_arn =
            format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/{}", case.name);
        assert_eq!(
            slo["Arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {:?}: expected Arn {expected_arn}, got {:?}",
            case.name,
            slo["Arn"]
        );
        assert_eq!(
            slo["Name"].as_str(),
            Some(case.name),
            "case {:?}: expected Name {:?}, got {:?}",
            case.name,
            case.name,
            slo["Name"]
        );
        assert_eq!(
            slo["EvaluationType"].as_str(),
            Some(case.evaluation_type),
            "case {:?}: expected EvaluationType {:?}, got {:?}",
            case.name,
            case.evaluation_type,
            slo["EvaluationType"]
        );
        assert_eq!(
            slo["MetricSourceType"].as_str(),
            Some(case.metric_source_type),
            "case {:?}: expected MetricSourceType {:?}, got {:?}",
            case.name,
            case.metric_source_type,
            slo["MetricSourceType"]
        );
        assert!(
            slo["CreatedTime"].is_number() && slo["LastUpdatedTime"].is_number(),
            "case {:?}: expected CreatedTime and LastUpdatedTime to be set, got {slo}",
            case.name
        );

        // A Get by name returns the IDENTICAL record the create answered with — the
        // response is read back out of the store, not recomputed from a request.
        let (status, fetched) = call(
            &router,
            "GET",
            &format!("/slo/{}", name_path_segment(case.name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetServiceLevelObjective, got {status} with {fetched}",
            case.name
        );
        assert_eq!(
            fetched["Slo"], created["Slo"],
            "case {:?}: Get must answer the persisted record, got {:?} after creating {:?}",
            case.name, fetched["Slo"], created["Slo"]
        );

        // The modelled `ServiceLevelObjectiveId` pattern is a name-or-ARN alternation, so
        // the ARN names the same record.
        let (status, by_arn) = call(
            &router,
            "GET",
            &format!("/slo/{}", arn_path_segment(case.name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 for a Get by ARN, got {status} with {by_arn}",
            case.name
        );
        assert_eq!(
            by_arn["Slo"], created["Slo"],
            "case {:?}: a Get by ARN must answer the same record as a Get by name, got {:?}",
            case.name, by_arn["Slo"]
        );
    }

    // Every created SLO is in the store exactly once, in name order.
    let (status, page) = call(&router, "POST", "/slos", Some(json!({}))).await;
    assert_eq!(status, 200, "expected HTTP 200 from ListServiceLevelObjectives, got {status}");
    let mut expected: Vec<String> = cases.iter().map(|case| case.name.to_string()).collect();
    expected.sort();
    assert_eq!(
        summary_names(&page),
        expected,
        "expected every created SLO listed once in name order, got {page}"
    );
}

/// `Goal` carries the defaults the model documents for each member the request omits, and
/// keeps every member the request supplies.
///
/// The documented defaults (`smithy_query(shape="Goal")`): `Interval` -> "a rolling
/// interval of 7 days"; `AttainmentGoal` -> "99 is used to represent 99%";
/// `WarningThreshold` -> "the default of 50.0".
#[tokio::test]
async fn goal_members_the_request_omits_take_their_documented_defaults() {
    struct Case {
        name: &'static str,
        goal: Option<Value>,
        interval: Value,
        attainment_goal: f64,
        warning_threshold: f64,
    }

    let cases = vec![
        Case {
            name: "no-goal-at-all",
            goal: None,
            interval: json!({ "RollingInterval": { "DurationUnit": "DAY", "Duration": 7 } }),
            attainment_goal: 99.0,
            warning_threshold: 50.0,
        },
        Case {
            name: "attainment-goal-only",
            goal: Some(json!({ "AttainmentGoal": 99.95 })),
            interval: json!({ "RollingInterval": { "DurationUnit": "DAY", "Duration": 7 } }),
            attainment_goal: 99.95,
            warning_threshold: 50.0,
        },
        Case {
            name: "warning-threshold-only",
            goal: Some(json!({ "WarningThreshold": 30.5 })),
            interval: json!({ "RollingInterval": { "DurationUnit": "DAY", "Duration": 7 } }),
            attainment_goal: 99.0,
            warning_threshold: 30.5,
        },
        Case {
            name: "rolling-interval-in-hours",
            // `DurationUnit` is tested on an entry the issue did not cite: HOUR, from
            // `smithy_query(shape="DurationUnit")` -> MINUTE | HOUR | DAY | MONTH.
            goal: Some(json!({
                "Interval": { "RollingInterval": { "DurationUnit": "HOUR", "Duration": 6 } },
                "AttainmentGoal": 95.0,
                "WarningThreshold": 10.0
            })),
            interval: json!({ "RollingInterval": { "DurationUnit": "HOUR", "Duration": 6 } }),
            attainment_goal: 95.0,
            warning_threshold: 10.0,
        },
        Case {
            name: "calendar-interval-in-months",
            goal: Some(json!({
                "Interval": {
                    "CalendarInterval": {
                        "StartTime": 1_767_225_600,
                        "DurationUnit": "MONTH",
                        "Duration": 1
                    }
                }
            })),
            interval: json!({
                "CalendarInterval": {
                    "StartTime": 1_767_225_600,
                    "DurationUnit": "MONTH",
                    "Duration": 1
                }
            }),
            attainment_goal: 99.0,
            warning_threshold: 50.0,
        },
    ];

    let router = router();
    for case in &cases {
        let mut body = json!({
            "Name": case.name,
            "SliConfig": sli_config("checkout", "GET /cart", 100.0)
        });
        if let Some(goal) = case.goal.clone() {
            body["Goal"] = goal;
        }
        let (status, created) = create(&router, body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {created}",
            case.name
        );
        let goal = &created["Slo"]["Goal"];
        assert_eq!(
            goal["Interval"], case.interval,
            "case {:?}: expected Interval {:?}, got {:?}",
            case.name, case.interval, goal["Interval"]
        );
        assert_eq!(
            goal["AttainmentGoal"].as_f64(),
            Some(case.attainment_goal),
            "case {:?}: expected AttainmentGoal {}, got {:?}",
            case.name,
            case.attainment_goal,
            goal["AttainmentGoal"]
        );
        assert_eq!(
            goal["WarningThreshold"].as_f64(),
            Some(case.warning_threshold),
            "case {:?}: expected WarningThreshold {}, got {:?}",
            case.name,
            case.warning_threshold,
            goal["WarningThreshold"]
        );
    }
}

/// A second create under a name already stored is a 409 `ConflictException`, whatever the
/// rest of the request looks like, and it leaves the stored record untouched.
#[tokio::test]
async fn a_repeated_name_conflicts_and_does_not_overwrite() {
    let router = router();
    let (status, first) = create(
        &router,
        json!({
            "Name": "duplicate-slo",
            "Description": "the first one",
            "SliConfig": sli_config("checkout", "GET /cart", 100.0)
        }),
    )
    .await;
    assert_eq!(status, 200, "expected the first create to succeed, got {status} with {first}");

    // Each row is a DIFFERENT second request under the same name: the conflict is on the
    // name alone, not on the body matching.
    let retries = [
        json!({ "Name": "duplicate-slo", "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
        json!({ "Name": "duplicate-slo", "Description": "a different description",
                "SliConfig": sli_config("orders", "POST /orders", 999.0) }),
        json!({ "Name": "duplicate-slo", "RequestBasedSliConfig": {
                "RequestBasedSliMetricConfig": { "MetricType": "AVAILABILITY" } } }),
    ];
    for (index, body) in retries.iter().enumerate() {
        let (status, error) = create(&router, body.clone()).await;
        assert_eq!(
            status, 409,
            "case {index}: expected HTTP 409 for a repeated name, got {status} with {error}"
        );
    }

    let (status, fetched) = call(&router, "GET", "/slo/duplicate-slo", None).await;
    assert_eq!(status, 200, "expected the stored SLO to survive, got {status} with {fetched}");
    assert_eq!(
        fetched["Slo"], first["Slo"],
        "a refused create must not overwrite the stored record, got {:?}",
        fetched["Slo"]
    );
}

/// Supplying both mutually exclusive SLI configs in one call is refused with a 400
/// `ValidationException`, on create and on update alike.
#[tokio::test]
async fn both_sli_configs_in_one_call_is_refused() {
    let router = router();
    let both = json!({
        "SliConfig": sli_config("checkout", "GET /cart", 100.0),
        "RequestBasedSliConfig": {
            "RequestBasedSliMetricConfig": { "MetricType": "AVAILABILITY" }
        }
    });

    let mut create_body = both.clone();
    create_body["Name"] = json!("exclusive-configs");
    let (status, error) = create(&router, create_body).await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 when a create names both SLI configs, got {status} with {error}"
    );

    let (status, created) = create(
        &router,
        json!({ "Name": "exclusive-configs", "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
    )
    .await;
    assert_eq!(status, 200, "expected the setup create to succeed, got {status} with {created}");

    let (status, error) = call(&router, "PATCH", "/slo/exclusive-configs", Some(both)).await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 when an update names both SLI configs, got {status} with {error}"
    );
}

/// An update mutates exactly the members the request supplies, leaves every other member
/// of the stored record alone, bumps `LastUpdatedTime`, and a following Get shows it.
#[tokio::test]
async fn update_changes_only_the_supplied_members() {
    let router = router();
    let (status, created) = create(
        &router,
        json!({
            "Name": "mutable-slo",
            "Description": "before",
            "SliConfig": sli_config("checkout", "POST /checkout", 250.0),
            "Goal": { "AttainmentGoal": 99.9, "WarningThreshold": 20.0 },
            "BurnRateConfigurations": [{ "LookBackWindowMinutes": 60 }],
            "AutoInvestigationEnabled": false
        }),
    )
    .await;
    assert_eq!(status, 200, "expected the setup create to succeed, got {status} with {created}");
    let before = created["Slo"].clone();

    // One supplied member per row; every other member of the record must survive.
    struct Case {
        label: &'static str,
        patch: Value,
        /// Members whose value must equal the stored one from before this patch.
        untouched: &'static [&'static str],
    }
    let cases = vec![
        Case {
            label: "Description",
            patch: json!({ "Description": "after" }),
            untouched: &["Sli", "Goal", "BurnRateConfigurations", "AutoInvestigationEnabled"],
        },
        Case {
            label: "Goal",
            patch: json!({ "Goal": { "AttainmentGoal": 95.0 } }),
            untouched: &["Description", "Sli", "BurnRateConfigurations", "AutoInvestigationEnabled"],
        },
        Case {
            label: "BurnRateConfigurations",
            patch: json!({ "BurnRateConfigurations": [{ "LookBackWindowMinutes": 5 },
                                                      { "LookBackWindowMinutes": 1440 }] }),
            untouched: &["Description", "Sli", "Goal", "AutoInvestigationEnabled"],
        },
        Case {
            label: "AutoInvestigationEnabled",
            patch: json!({ "AutoInvestigationEnabled": true }),
            untouched: &["Description", "Sli", "Goal", "BurnRateConfigurations"],
        },
        Case {
            label: "RequestBasedSliConfig",
            patch: json!({ "RequestBasedSliConfig": {
                "RequestBasedSliMetricConfig": {
                    "KeyAttributes": { "Type": "Service", "Name": "checkout", "Environment": "prod" }
                }
            } }),
            untouched: &["Description", "Goal", "BurnRateConfigurations", "AutoInvestigationEnabled"],
        },
    ];

    let mut previous = before.clone();
    for case in &cases {
        // The clock must have moved on since the record was last written, so the bump is
        // observable rather than a same-instant tie.
        std::thread::sleep(std::time::Duration::from_millis(5));

        let (status, updated) =
            call(&router, "PATCH", "/slo/mutable-slo", Some(case.patch.clone())).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UpdateServiceLevelObjective, got {status} with {updated}",
            case.label
        );
        let after = updated["Slo"].clone();

        assert_eq!(
            after["Arn"], before["Arn"],
            "case {:?}: Arn must not change, got {:?}",
            case.label, after["Arn"]
        );
        assert_eq!(
            after["CreatedTime"], before["CreatedTime"],
            "case {:?}: CreatedTime must not change, got {:?}",
            case.label, after["CreatedTime"]
        );
        let (previous_updated, now_updated) = (
            previous["LastUpdatedTime"].as_f64().unwrap_or_default(),
            after["LastUpdatedTime"].as_f64().unwrap_or_default(),
        );
        assert!(
            now_updated > previous_updated,
            "case {:?}: expected LastUpdatedTime to advance past {previous_updated}, got {now_updated}",
            case.label
        );
        for member in case.untouched {
            assert_eq!(
                after[*member], previous[*member],
                "case {:?}: expected {member} to survive the update as {:?}, got {:?}",
                case.label, previous[*member], after[*member]
            );
        }

        // A following Get shows the update: the response came out of the store.
        let (status, fetched) = call(&router, "GET", "/slo/mutable-slo", None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the Get after the update, got {status}",
            case.label
        );
        assert_eq!(
            fetched["Slo"], after,
            "case {:?}: a Get after the update must answer the updated record, got {:?}",
            case.label, fetched["Slo"]
        );
        previous = after;
    }

    // The last patch supplied `RequestBasedSliConfig`, so the record switched kinds.
    assert_eq!(
        previous["EvaluationType"].as_str(),
        Some("RequestBased"),
        "expected the record to become RequestBased after a RequestBasedSliConfig update, got {:?}",
        previous["EvaluationType"]
    );
    assert!(
        previous["Sli"].is_null(),
        "expected the period-based Sli to be dropped when the SLO became request-based, got {:?}",
        previous["Sli"]
    );
}

/// `MaxResults` bounds the page and `NextToken` round-trips, so paging over more SLOs than
/// one page holds terminates and visits each SLO exactly once.
///
/// This is the paging invariant, asserted over every page size the modelled
/// `ListServiceLevelObjectivesMaxResults` range admits at its bounds and inside it, against
/// a stored set both smaller and larger than the page.
#[tokio::test]
async fn paging_terminates_and_visits_every_slo_exactly_once() {
    let router = router();
    let total = 7;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..total {
        let name = format!("paged-slo-{index:02}");
        let (status, created) = create(
            &router,
            json!({ "Name": name, "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
        )
        .await;
        assert_eq!(status, 200, "expected to create {name}, got {status} with {created}");
        expected.push(name);
    }
    expected.sort();

    // `range 1..50` from `smithy_query(query="operation", operation="ListServiceLevelObjectives")`:
    // the published bounds plus sizes either side of the stored set.
    for page_size in [1_usize, 2, 6, 7, 8, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= total + 2,
                "page size {page_size}: paging did not terminate after {pages} pages, seen {seen:?}"
            );
            let mut body = json!({ "MaxResults": page_size });
            if let Some(token) = token.as_ref() {
                body["NextToken"] = json!(token);
            }
            let (status, page) = call(&router, "POST", "/slos", Some(body)).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: expected HTTP 200, got {status} with {page}"
            );
            let names = summary_names(&page);
            assert!(
                names.len() <= page_size,
                "page size {page_size}: expected at most {page_size} summaries, got {}",
                names.len()
            );
            seen.extend(names);
            token = page["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                !seen.is_empty(),
                "page size {page_size}: a NextToken was returned for an empty page"
            );
        }
        assert_eq!(
            seen, expected,
            "page size {page_size}: expected every SLO exactly once in name order, got {seen:?}"
        );
    }

    // A token this service never issued is a refused request, not a silent full scan —
    // whether or not it is well formed. The second row is the hex encoding of a plain SLO
    // name, which decodes cleanly but carries none of this service's token marker.
    for token in [
        "not-a-token",
        &hex::encode("paged-slo-03"),
        &hex::encode(""),
        "7a",
    ] {
        let (status, error) = call(
            &router,
            "POST",
            "/slos",
            Some(json!({ "NextToken": token })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a NextToken this service did not issue, got {status} with {error}"
        );
    }

    // A `MaxResults` outside the modelled `range 1..50` is refused, not coerced: each
    // published bound is accepted (above) and one step past each is refused, so an
    // out-of-range page size can never read as "nothing is stored".
    for max_results in [0, -1, 51, 1_000] {
        let (status, error) = call(
            &router,
            "POST",
            "/slos",
            Some(json!({ "MaxResults": max_results })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected HTTP 400 for a value outside range 1..50, got {status} with {error}"
        );
    }
}


/// Each `ListServiceLevelObjectives` selector filters the STORED set: only the SLOs whose
/// persisted SLI matches come back.
#[tokio::test]
async fn every_list_filter_narrows_the_stored_set() {
    let router = router();

    let fixtures = vec![
        json!({
            "Name": "checkout-op",
            "SliConfig": sli_config("checkout", "POST /checkout", 250.0)
        }),
        json!({
            "Name": "checkout-service",
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": { "Type": "Service", "Name": "checkout", "Environment": "prod" },
                    "MetricType": "AVAILABILITY"
                }
            }
        }),
        json!({
            "Name": "orders-op",
            "SliConfig": sli_config("orders", "POST /orders", 250.0)
        }),
        json!({
            "Name": "orders-dependency",
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": { "Type": "Service", "Name": "orders", "Environment": "prod" },
                    "DependencyConfig": {
                        "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "payments" },
                        "DependencyOperationName": "Charge"
                    }
                }
            }
        }),
        json!({
            "Name": "rum-monitor",
            "SliConfig": {
                "SliMetricConfig": {
                    "MetricSource": {
                        "MetricSourceKeyAttributes": { "Type": "AWS::RUM::AppMonitor", "Name": "web" }
                    }
                }
            }
        }),
    ];
    for fixture in &fixtures {
        let (status, created) = create(&router, fixture.clone()).await;
        assert_eq!(status, 200, "expected to create {fixture}, got {status} with {created}");
    }

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter",
            json!({}),
            vec![
                "checkout-op",
                "checkout-service",
                "orders-dependency",
                "orders-op",
                "rum-monitor",
            ],
        ),
        (
            "KeyAttributes names one service",
            json!({ "KeyAttributes": { "Name": "checkout" } }),
            vec!["checkout-op", "checkout-service"],
        ),
        (
            "KeyAttributes names a service that has no SLO",
            json!({ "KeyAttributes": { "Name": "shipping" } }),
            vec![],
        ),
        (
            "OperationName",
            json!({ "OperationName": "POST /orders" }),
            vec!["orders-op"],
        ),
        (
            "DependencyConfig",
            json!({ "DependencyConfig": {
                "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "payments" },
                "DependencyOperationName": "Charge"
            } }),
            vec!["orders-dependency"],
        ),
        (
            "MetricSourceTypes with one entry",
            json!({ "MetricSourceTypes": ["ServiceOperation"] }),
            vec!["checkout-op", "orders-op"],
        ),
        (
            "MetricSourceTypes with several entries",
            json!({ "MetricSourceTypes": ["Service", "AppMonitor"] }),
            vec!["checkout-service", "rum-monitor"],
        ),
        (
            "MetricSource",
            json!({ "MetricSource": {
                "MetricSourceKeyAttributes": { "Type": "AWS::RUM::AppMonitor", "Name": "web" }
            } }),
            vec!["rum-monitor"],
        ),
        (
            "SloOwnerAwsAccountId naming another account",
            json!({ "SloOwnerAwsAccountId": "111122223333" }),
            vec![],
        ),
        (
            "SloOwnerAwsAccountId naming this account",
            json!({ "SloOwnerAwsAccountId": ACCOUNT }),
            vec![
                "checkout-op",
                "checkout-service",
                "orders-dependency",
                "orders-op",
                "rum-monitor",
            ],
        ),
    ];

    for (label, filter, expected) in &cases {
        let (status, page) = call(&router, "POST", "/slos", Some(filter.clone())).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from ListServiceLevelObjectives, got {status} with {page}"
        );
        assert_eq!(
            summary_names(&page),
            expected.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
            "case {label:?}: expected {expected:?}, got {:?}",
            summary_names(&page)
        );
    }

    // The summary projects the persisted entity, not the request that created the SLO.
    let (_, page) = call(
        &router,
        "POST",
        "/slos",
        Some(json!({ "OperationName": "POST /checkout" })),
    )
    .await;
    let summary = &page["SloSummaries"][0];
    let mut key_attributes = BTreeMap::new();
    key_attributes.insert("Type", "Service");
    key_attributes.insert("Name", "checkout");
    key_attributes.insert("Environment", "prod");
    for (key, value) in &key_attributes {
        assert_eq!(
            summary["KeyAttributes"][*key].as_str(),
            Some(*value),
            "expected the summary's KeyAttributes.{key} to be {value:?}, got {:?}",
            summary["KeyAttributes"][*key]
        );
    }
    assert_eq!(
        summary["EvaluationType"].as_str(),
        Some("PeriodBased"),
        "expected the summary to carry the stored EvaluationType, got {:?}",
        summary["EvaluationType"]
    );
    assert!(
        summary["CreatedTime"].is_number(),
        "expected the summary to carry the stored CreatedTime, got {:?}",
        summary["CreatedTime"]
    );
}

/// The stored `Sli` is the supplied `SliConfig` member for member, with the config's own
/// modelled defaults applied to the members the request omits.
///
/// `smithy_query(shape="ServiceLevelIndicatorConfig")`: `MetricThreshold` carries
/// `@default: 0` and `ComparisonOperator` carries `@default: "LessThan"`. The supplied
/// values are drawn from `ServiceLevelIndicatorComparisonOperator`
/// (`GreaterThan | GreaterThanOrEqualTo | LessThan | LessThanOrEqualTo`), including entries
/// the issue did not cite.
#[tokio::test]
async fn the_stored_sli_mirrors_the_config_with_its_documented_defaults() {
    struct Case {
        name: &'static str,
        /// What the request puts in `SliConfig` beside `SliMetricConfig`.
        threshold: Option<f64>,
        comparison_operator: Option<&'static str>,
        expected_threshold: f64,
        expected_comparison_operator: &'static str,
    }

    let cases = vec![
        Case {
            name: "both-omitted",
            threshold: None,
            comparison_operator: None,
            expected_threshold: 0.0,
            expected_comparison_operator: "LessThan",
        },
        Case {
            name: "threshold-only",
            threshold: Some(2000.0),
            comparison_operator: None,
            expected_threshold: 2000.0,
            expected_comparison_operator: "LessThan",
        },
        Case {
            name: "operator-only-greater-than-or-equal-to",
            threshold: None,
            comparison_operator: Some("GreaterThanOrEqualTo"),
            expected_threshold: 0.0,
            expected_comparison_operator: "GreaterThanOrEqualTo",
        },
        Case {
            name: "both-supplied-less-than-or-equal-to",
            threshold: Some(0.5),
            comparison_operator: Some("LessThanOrEqualTo"),
            expected_threshold: 0.5,
            expected_comparison_operator: "LessThanOrEqualTo",
        },
        Case {
            name: "both-supplied-greater-than",
            threshold: Some(-1.5),
            comparison_operator: Some("GreaterThan"),
            expected_threshold: -1.5,
            expected_comparison_operator: "GreaterThan",
        },
    ];

    let router = router();
    for case in &cases {
        let metric_config = json!({
            "KeyAttributes": { "Type": "Service", "Name": "checkout", "Environment": "prod" },
            "OperationName": "GET /cart",
            "MetricType": "LATENCY",
            "MetricDataQueries": [{
                "Id": "sli",
                "MetricStat": {
                    "Metric": {
                        "Namespace": "AWS/ApplicationSignals",
                        "MetricName": "Latency",
                        "Dimensions": [{ "Name": "Service", "Value": "checkout" }]
                    },
                    "Period": 300,
                    "Stat": "p99",
                    "Unit": "Milliseconds"
                },
                "ReturnData": true
            }]
        });
        let mut sli_config = json!({ "SliMetricConfig": metric_config.clone() });
        if let Some(threshold) = case.threshold {
            sli_config["MetricThreshold"] = json!(threshold);
        }
        if let Some(operator) = case.comparison_operator {
            sli_config["ComparisonOperator"] = json!(operator);
        }

        let (status, created) = create(
            &router,
            json!({ "Name": case.name, "SliConfig": sli_config }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {created}",
            case.name
        );
        let sli = &created["Slo"]["Sli"];
        assert_eq!(
            sli["MetricThreshold"].as_f64(),
            Some(case.expected_threshold),
            "case {:?}: expected Sli.MetricThreshold {}, got {:?}",
            case.name,
            case.expected_threshold,
            sli["MetricThreshold"]
        );
        assert_eq!(
            sli["ComparisonOperator"].as_str(),
            Some(case.expected_comparison_operator),
            "case {:?}: expected Sli.ComparisonOperator {:?}, got {:?}",
            case.name,
            case.expected_comparison_operator,
            sli["ComparisonOperator"]
        );
        // Every member the config and the indicator share round-trips unchanged.
        for member in ["KeyAttributes", "OperationName", "MetricDataQueries"] {
            assert_eq!(
                sli["SliMetric"][member], metric_config[member],
                "case {:?}: expected Sli.SliMetric.{member} to mirror the config as {:?}, got {:?}",
                case.name, metric_config[member], sli["SliMetric"][member]
            );
        }
        // `MetricType` is NOT mirrored here: this config spells out its own
        // `MetricDataQueries`, so it selects no Application Signals standard metric and the
        // indicator reports no metric type (see `tests/all/sli_metric_type.rs`).
        assert!(
            sli["SliMetric"]["MetricType"].is_null(),
            "case {:?}: a config that names its own MetricDataQueries must report no MetricType, got {:?}",
            case.name,
            sli["SliMetric"]["MetricType"]
        );
        assert!(
            created["Slo"]["RequestBasedSli"].is_null(),
            "case {:?}: a period-based SLO must carry no RequestBasedSli, got {:?}",
            case.name,
            created["Slo"]["RequestBasedSli"]
        );
    }

    // A config that names only an Application Signals STANDARD metric carries no
    // `MetricDataQueries`, and the indicator's required list is reported EMPTY rather than
    // filled with a query this emulator cannot compute (the real service builds it from its
    // own metric catalogue: a namespace, a dimension set and a query id). The row pins that
    // documented decision, and that the record still round-trips through a read.
    for (label, metric_config) in [
        (
            "metric-name-statistic-and-period",
            json!({
                "KeyAttributes": { "Type": "Service", "Name": "checkout", "Environment": "prod" },
                "MetricName": "Latency",
                "Statistic": "p99",
                "PeriodSeconds": 60
            }),
        ),
        (
            "metric-type-only",
            json!({
                "KeyAttributes": { "Type": "Service", "Name": "orders", "Environment": "prod" },
                "MetricType": "AVAILABILITY"
            }),
        ),
    ] {
        let name = format!("standard-metric-{label}");
        let (status, created) = create(
            &router,
            json!({ "Name": name, "SliConfig": { "SliMetricConfig": metric_config } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with {created}"
        );
        assert_eq!(
            created["Slo"]["Sli"]["SliMetric"]["MetricDataQueries"],
            json!([]),
            "case {label:?}: expected an empty MetricDataQueries rather than a synthesised query, got {:?}",
            created["Slo"]["Sli"]["SliMetric"]["MetricDataQueries"]
        );
        let (status, fetched) = call(
            &router,
            "GET",
            &format!("/slo/{}", name_path_segment(&name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 reading the SLO back, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["Slo"], created["Slo"],
            "case {label:?}: the read-back record must equal the created one, got {:?}",
            fetched["Slo"]
        );
    }
}

/// The store is bounded: once this service holds its quota of SLOs, a create under a fresh
/// name is refused with the modelled `ServiceQuotaExceededException` (HTTP 402), nothing is
/// added, and the SLOs already stored stay readable. Deleting one frees exactly one slot.
///
/// The cap is read from the service's own constant so the test pins the behaviour, not a
/// duplicated number.
#[tokio::test]
async fn a_create_past_the_slo_quota_is_refused() {
    let quota = service_com_amazonaws_applicationsignals_applicationsignals::state::MAX_SLOS;
    let router = router();
    for index in 0..quota {
        let (status, created) = create(
            &router,
            json!({
                "Name": format!("quota-slo-{index:06}"),
                "SliConfig": sli_config("checkout", "GET /cart", 100.0)
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {index}: expected HTTP 200 while under the quota of {quota}, got {status} with {created}"
        );
    }

    // Several different requests, all past the quota, all refused.
    for (index, name) in ["one-too-many", "another-one", "a.third_one-9"].iter().enumerate() {
        let (status, error) = create(
            &router,
            json!({ "Name": name, "SliConfig": sli_config("orders", "POST /orders", 1.0) }),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {index} ({name:?}): expected HTTP 402 past the quota of {quota}, got {status} with {error}"
        );
        let (status, fetched) = call(&router, "GET", &format!("/slo/{name}"), None).await;
        assert_eq!(
            status, 404,
            "case {index} ({name:?}): a refused create must store nothing, got {status} with {fetched}"
        );
    }

    // What was already stored is untouched and still readable.
    let (status, fetched) = call(&router, "GET", "/slo/quota-slo-000000", None).await;
    assert_eq!(
        status, 200,
        "expected the SLOs stored before the quota was reached to stay readable, got {status} with {fetched}"
    );

    // One delete frees exactly one slot.
    let (status, _) = call(&router, "DELETE", "/slo/quota-slo-000000", None).await;
    assert_eq!(status, 200, "expected the delete to succeed, got {status}");
    let (status, created) = create(
        &router,
        json!({ "Name": "one-too-many", "SliConfig": sli_config("orders", "POST /orders", 1.0) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected one freed slot to admit one create, got {status} with {created}"
    );
    let (status, error) = create(
        &router,
        json!({ "Name": "another-one", "SliConfig": sli_config("orders", "POST /orders", 1.0) }),
    )
    .await;
    assert_eq!(
        status, 402,
        "expected the next create to be refused again at the quota, got {status} with {error}"
    );
}

/// A delete removes the record: a later Get, Update or Delete answers 404 and the SLO is
/// gone from the list. An id that names no SLO is a 404 on each of the three.
#[tokio::test]
async fn delete_removes_the_record_and_a_missing_id_is_not_found() {
    let router = router();
    for name in ["gone-by-name", "gone-by-arn"] {
        let (status, created) = create(
            &router,
            json!({ "Name": name, "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
        )
        .await;
        assert_eq!(status, 200, "expected to create {name}, got {status} with {created}");
    }

    // Both id forms the modelled pattern admits delete the same way.
    let by_arn = format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/gone-by-arn");
    for (id, path) in [
        ("gone-by-name".to_string(), name_path_segment("gone-by-name")),
        (by_arn.clone(), arn_path_segment("gone-by-arn")),
    ] {
        let (status, body) = call(&router, "DELETE", &format!("/slo/{path}"), None).await;
        assert_eq!(
            status, 200,
            "case {id:?}: expected HTTP 200 from DeleteServiceLevelObjective, got {status} with {body}"
        );
    }

    let (status, page) = call(&router, "POST", "/slos", Some(json!({}))).await;
    assert_eq!(status, 200, "expected HTTP 200 from ListServiceLevelObjectives, got {status}");
    assert_eq!(
        summary_names(&page),
        Vec::<String>::new(),
        "expected every deleted SLO to be gone from the list, got {page}"
    );

    // Every id that names no stored SLO is a 404 — the deleted ones and one that never
    // existed, by name and by ARN. Whichever form the id took, the failure identifies the
    // SLO by its ARN; `tests/all/error_identity.rs` asserts that rule over more inputs.
    let missing = [
        ("gone-by-name".to_string(), name_path_segment("gone-by-name")),
        (by_arn, arn_path_segment("gone-by-arn")),
        ("never-existed".to_string(), name_path_segment("never-existed")),
        (
            format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/never-existed"),
            arn_path_segment("never-existed"),
        ),
    ];
    for (id, encoded) in &missing {
        let arn = if id.starts_with("arn:") {
            id.clone()
        } else {
            format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/{id}")
        };
        for (method, body) in [
            ("GET", None),
            ("PATCH", Some(json!({ "Description": "x" }))),
            ("DELETE", None),
        ] {
            let (status, error) = call(&router, method, &format!("/slo/{encoded}"), body).await;
            assert_eq!(
                status, 404,
                "case {id:?}: expected HTTP 404 from {method} /slo/{{Id}}, got {status} with {error}"
            );
            assert_eq!(
                error["ResourceId"].as_str(),
                Some(arn.as_str()),
                "case {id:?}: expected the failure to identify the SLO by its ARN {arn:?}, got {error}"
            );
        }
    }
}

/// An SLO name written as one URI path segment.
///
/// No general percent-encoder is needed, and none is re-derived here. The modelled
/// `ServiceLevelObjectiveName` pattern (`^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$`,
/// from `smithy_query(shape="ServiceLevelObjective")`) admits exactly the alphanumerics and
/// `-`, `.`, `_`, ` `. Every one of those but the space is in RFC 3986's unreserved set and
/// is already legal in a path segment, so substituting the space is a COMPLETE escaping of
/// any name the API accepts — a closed character class, not an encoding algorithm.
pub(crate) fn name_path_segment(name: &str) -> String {
    name.replace(' ', "%20")
}

/// The ARN of the SLO called `name`, written as one URI segment.
pub(crate) fn arn_path_segment(name: &str) -> String {
    arn_uri_segment(&format!(
        "arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/{}",
        name_path_segment(name)
    ))
}

/// An ARN written as one URI segment (a path segment, or a query value).
///
/// The ARN's own punctuation is what needs escaping and nothing else: a segment cannot carry
/// `/` (it would split the segment), and `:` is escaped beside it. Every other character an
/// ARN of this service carries is in RFC 3986's unreserved set, so this is a COMPLETE
/// escaping of any ARN the API accepts rather than a general percent-encoder. The ONE place
/// this test target spells a request's ARN, so two suites cannot encode one differently; the
/// unescaped form is what the tests assert the service RETURNS.
pub(crate) fn arn_uri_segment(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}
