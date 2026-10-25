// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListAuditFindings` over HTTP.
//!
//! The rule under test: the findings reported are RESOLVED AGAINST THE STORE — each
//! `AuditTarget` names an entity, and what is reported for it is the analysis of the records
//! this emulator holds for that entity over the requested window, with `Auditors` selecting
//! which auditors' findings are reported and `DetailLevel` how much context each one
//! carries. A target the store holds nothing for contributes no finding.
//!
//! Every objective these tests read back is created over HTTP first, so each reported fact
//! is a round trip through the stored record rather than an echo of the audit request.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification
/// is off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("serialize the request body")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
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

/// Seconds since the epoch, for a window around the instant the stored objectives are
/// created at.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The key attributes of one Application Signals service entity.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": "prod" })
}

/// One `MetricDataQuery` selecting `metric_name` in `namespace`.
fn metric_query(id: &str, namespace: &str, metric_name: &str) -> Value {
    json!({
        "Id": id,
        "MetricStat": {
            "Metric": { "Namespace": namespace, "MetricName": metric_name },
            "Period": 60,
            "Stat": "Sum"
        },
        "ReturnData": true
    })
}

/// Create one SLO, assert it was stored, and hand back the ARN the create minted.
async fn create_slo(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, answer) = call(router, "POST", "/slo", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the SLO create to answer 200, got {status} with {answer:?}"
    );
    answer["Slo"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the created SLO to carry an Arn"))
        .to_string()
}

/// One audit target, in the shape the request carries it: the target kind and the entity
/// union member that names what to audit.
fn target(kind: &str, data: Value) -> Value {
    json!({ "Type": kind, "Data": data })
}

/// Ask for the audit findings of `targets` over `window`, with the rest of the request's
/// members as `extra` supplies them.
async fn audit(
    router: &MultiServiceRouter,
    window: &str,
    targets: Vec<Value>,
    extra: Value,
) -> (u16, Value) {
    let mut body = json!({ "AuditTargets": targets });
    if let Some(extra) = extra.as_object() {
        for (member, value) in extra {
            body[member] = value.clone();
        }
    }
    call(
        router,
        "POST",
        &format!("/auditFindings?{window}"),
        Some(body),
    )
    .await
}

/// The objectives the reported findings were computed from, in the order they are reported:
/// each finding's SLO auditor result names the record it assessed.
fn assessed_objectives(answer: &Value) -> Vec<String> {
    answer["AuditFindings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|finding| {
            finding["AuditorResults"][0]["Data"]["SloName"]
                .as_str()
                .unwrap_or("<no SloName>")
                .to_string()
        })
        .collect()
}

/// Seed the objectives every resolution test reads back, and hand back the ARN of each.
///
/// `checkout-latency` and `cart-availability` monitor two operations of ONE entity and
/// measure different metric types; `payments-health` monitors a second entity and names no
/// operation; `lambda-duration` reads a plain CloudWatch metric, so it identifies no
/// Application Signals entity at all.
///
/// The two objectives carrying a `MetricType` name no `MetricDataQueries`, because the
/// stored `MetricType` is reported only for an indicator that selects an Application
/// Signals standard metric (`src/state.rs`, `reported_metric_type`).
async fn seed(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let mut arns = Vec::new();
    for (name, body) in [
        (
            "checkout-latency",
            json!({
                "Name": "checkout-latency",
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": service_entity("checkout"),
                        "OperationName": "POST /checkout",
                        "MetricType": "LATENCY"
                    }
                }
            }),
        ),
        (
            "cart-availability",
            json!({
                "Name": "cart-availability",
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": service_entity("checkout"),
                        "OperationName": "GET /cart",
                        "MetricType": "AVAILABILITY"
                    }
                }
            }),
        ),
        (
            "payments-health",
            json!({
                "Name": "payments-health",
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": service_entity("payments"),
                        "MetricDataQueries": [metric_query("m1", "ApplicationSignals", "Fault")]
                    }
                }
            }),
        ),
        (
            "lambda-duration",
            json!({
                "Name": "lambda-duration",
                "SliConfig": {
                    "SliMetricConfig": {
                        "MetricDataQueries": [metric_query("m1", "AWS/Lambda", "Duration")]
                    }
                }
            }),
        ),
    ] {
        let arn = create_slo(router, name, body).await;
        arns.push((name.to_string(), arn));
    }
    arns
}

/// The ARN seeded under `name`.
fn arn_of(seeded: &[(String, String)], name: &str) -> String {
    seeded
        .iter()
        .find(|(seeded_name, _)| seeded_name == name)
        .map(|(_, arn)| arn.clone())
        .unwrap_or_else(|| panic!("the suite seeded an objective called {name:?}"))
}

/// Every kind of audit target is resolved against the store, and the findings reported are
/// the assessments of the objectives it resolves to — in objective-name order, once per
/// objective however many targets name it.
///
/// The rows differ along what the rule generalises over: the KIND of target (service,
/// objective, service operation, canary), the FORM of an objective's identity (name, ARN,
/// both), how much of an entity's identity the target names (all of it, one attribute), the
/// COUNT of targets in one request, whether the named entity is stored at all, and whether
/// the stored record carries what the target narrows on.
#[tokio::test]
async fn findings_are_resolved_against_the_stored_objectives() {
    /// One audit request's targets and the objectives the findings must be computed from.
    struct Case {
        case: &'static str,
        targets: Vec<Value>,
        assessed: Vec<&'static str>,
        /// The `Type` every finding of this row must carry, where the row's targets are all
        /// of one kind: the token the `AuditTarget.Type` doc spells that kind with ("such as
        /// `service`, `SLO`, `service_operation`, or `canary`"). The rows send a target
        /// `Type` of their own in a different casing, so a finding carrying this token shows
        /// the kind is read off the modelled entity union rather than echoed from the
        /// request.
        kind: Option<&'static str>,
    }

    let router = router();
    let seeded = seed(&router).await;
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    let cases = vec![
        Case {
            case: "an objective named by name",
            targets: vec![target("slo", json!({ "Slo": { "SloName": "checkout-latency" } }))],
            assessed: vec!["checkout-latency"],
            kind: Some("SLO"),
        },
        Case {
            case: "an objective named by ARN",
            targets: vec![target(
                "slo",
                json!({ "Slo": { "SloArn": arn_of(&seeded, "checkout-latency") } }),
            )],
            assessed: vec!["checkout-latency"],
            kind: Some("SLO"),
        },
        Case {
            case: "an objective named by name and ARN together",
            targets: vec![target(
                "slo",
                json!({ "Slo": {
                    "SloName": "checkout-latency",
                    "SloArn": arn_of(&seeded, "checkout-latency")
                } }),
            )],
            assessed: vec!["checkout-latency"],
            kind: Some("SLO"),
        },
        Case {
            case: "a name and an ARN naming two different objectives",
            targets: vec![target(
                "slo",
                json!({ "Slo": {
                    "SloName": "checkout-latency",
                    "SloArn": arn_of(&seeded, "cart-availability")
                } }),
            )],
            assessed: vec![],
            kind: None,
        },
        Case {
            case: "an objective that was never created",
            targets: vec![target("slo", json!({ "Slo": { "SloName": "no-such-slo" } }))],
            assessed: vec![],
            kind: None,
        },
        Case {
            case: "an objective that identifies no entity",
            targets: vec![target("slo", json!({ "Slo": { "SloName": "lambda-duration" } }))],
            assessed: vec![],
            kind: None,
        },
        Case {
            case: "a service named by its whole identity",
            targets: vec![target(
                "service",
                json!({ "Service": {
                    "Type": "Service", "Name": "checkout", "Environment": "prod"
                } }),
            )],
            assessed: vec!["cart-availability", "checkout-latency"],
            kind: Some("service"),
        },
        Case {
            case: "a service named by one attribute of its identity",
            targets: vec![target("service", json!({ "Service": { "Name": "checkout" } }))],
            assessed: vec!["cart-availability", "checkout-latency"],
            kind: Some("service"),
        },
        Case {
            case: "a second service of the same store",
            targets: vec![target(
                "service",
                json!({ "Service": {
                    "Type": "Service", "Name": "payments", "Environment": "prod"
                } }),
            )],
            assessed: vec!["payments-health"],
            kind: Some("service"),
        },
        Case {
            case: "a service nothing monitors",
            targets: vec![target(
                "service",
                json!({ "Service": {
                    "Type": "Service", "Name": "ghost", "Environment": "prod"
                } }),
            )],
            assessed: vec![],
            kind: None,
        },
        Case {
            case: "a service in another account",
            targets: vec![target(
                "service",
                json!({ "Service": {
                    "Type": "Service",
                    "Name": "checkout",
                    "Environment": "prod",
                    "AwsAccountId": "999988887777"
                } }),
            )],
            assessed: vec![],
            kind: None,
        },
        Case {
            case: "an operation of a stored service",
            targets: vec![target(
                "service_operation",
                json!({ "ServiceOperation": {
                    "Service": { "Type": "Service", "Name": "checkout", "Environment": "prod" },
                    "Operation": "GET /cart"
                } }),
            )],
            assessed: vec!["cart-availability"],
            kind: Some("service_operation"),
        },
        Case {
            case: "an operation narrowed to the metric type it measures",
            targets: vec![target(
                "service_operation",
                json!({ "ServiceOperation": {
                    "Service": { "Type": "Service", "Name": "checkout", "Environment": "prod" },
                    "Operation": "GET /cart",
                    "MetricType": "AVAILABILITY"
                } }),
            )],
            assessed: vec!["cart-availability"],
            kind: Some("service_operation"),
        },
        Case {
            case: "an operation narrowed to a metric type it does not measure",
            targets: vec![target(
                "service_operation",
                json!({ "ServiceOperation": {
                    "Service": { "Type": "Service", "Name": "checkout", "Environment": "prod" },
                    "Operation": "GET /cart",
                    "MetricType": "LATENCY"
                } }),
            )],
            assessed: vec![],
            kind: None,
        },
        Case {
            case: "an operation of a service nothing monitors",
            targets: vec![target(
                "service_operation",
                json!({ "ServiceOperation": {
                    "Service": { "Type": "Service", "Name": "ghost", "Environment": "prod" },
                    "Operation": "GET /cart"
                } }),
            )],
            assessed: vec![],
            kind: None,
        },
        Case {
            case: "a service operation naming no operation",
            targets: vec![target(
                "service_operation",
                json!({ "ServiceOperation": {
                    "Service": { "Type": "Service", "Name": "checkout", "Environment": "prod" }
                } }),
            )],
            assessed: vec![],
            kind: None,
        },
        Case {
            case: "a canary, which this service holds none of",
            targets: vec![target(
                "canary",
                json!({ "Canary": { "CanaryName": "checkout-canary" } }),
            )],
            assessed: vec![],
            kind: None,
        },
        Case {
            case: "three targets of two kinds",
            targets: vec![
                target("slo", json!({ "Slo": { "SloName": "payments-health" } })),
                target("service", json!({ "Service": { "Name": "checkout" } })),
                target(
                    "canary",
                    json!({ "Canary": { "CanaryName": "checkout-canary" } }),
                ),
            ],
            assessed: vec!["cart-availability", "checkout-latency", "payments-health"],
            kind: None,
        },
        Case {
            case: "one objective named by two targets",
            targets: vec![
                target("slo", json!({ "Slo": { "SloName": "checkout-latency" } })),
                target(
                    "service_operation",
                    json!({ "ServiceOperation": {
                        "Service": { "Name": "checkout" },
                        "Operation": "POST /checkout"
                    } }),
                ),
            ],
            assessed: vec!["checkout-latency"],
            kind: Some("SLO"),
        },
    ];

    for case in cases {
        let (status, answer) = audit(&router, &window, case.targets, json!({})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {answer:?}",
            case.case
        );
        assert_eq!(
            assessed_objectives(&answer),
            case.assessed,
            "case {:?}: expected the findings of {:?}, got {:?}",
            case.case,
            case.assessed,
            answer["AuditFindings"]
        );
        if let Some(kind) = case.kind {
            for finding in answer["AuditFindings"].as_array().unwrap_or(&Vec::new()) {
                assert_eq!(
                    finding["Type"].as_str(),
                    Some(kind),
                    "case {:?}: expected the finding to report target kind {kind:?}, got {:?}",
                    case.case,
                    finding["Type"]
                );
            }
        }
    }
}

/// Two audit requests that ask DIFFERENT questions of the same store do not answer alike:
/// the reported findings are a function of `AuditTargets`, `Auditors` and `DetailLevel`
/// together, so changing any one of them changes the `AuditFindings` payload, and only
/// requests asking the same question answer the same.
///
/// The rows differ along each of the three members in turn — the entity a target names
/// (stored, never created), the kind of target, the auditor selected (the one whose input
/// this store holds, one that reads telemetry it never receives) and the detail level asked
/// for — and every pair of rows is compared: equal payloads exactly where the rows ask the
/// same question, different payloads everywhere else.
#[tokio::test]
async fn requests_asking_different_questions_do_not_answer_alike() {
    /// One audit request, the objectives its findings must be computed from, whether each
    /// finding must carry the DETAILED context, and a label naming the QUESTION it asks:
    /// two rows share a label exactly when they must answer identically.
    struct Case {
        case: &'static str,
        targets: Vec<Value>,
        extra: Value,
        assessed: Vec<&'static str>,
        detailed: bool,
        answer: &'static str,
    }

    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    let cases = vec![
        Case {
            case: "a stored objective, audited in detail",
            targets: vec![target("slo", json!({ "Slo": { "SloName": "payments-health" } }))],
            extra: json!({ "Auditors": ["slo"], "DetailLevel": "DETAILED" }),
            assessed: vec!["payments-health"],
            detailed: true,
            answer: "the payments objective in detail",
        },
        Case {
            case: "the same objective, audited briefly",
            targets: vec![target("slo", json!({ "Slo": { "SloName": "payments-health" } }))],
            extra: json!({ "Auditors": ["slo"], "DetailLevel": "BRIEF" }),
            assessed: vec!["payments-health"],
            detailed: false,
            answer: "the payments objective briefly",
        },
        Case {
            case: "the entity that objective monitors, audited in detail",
            targets: vec![target("service", json!({ "Service": { "Name": "payments" } }))],
            extra: json!({ "Auditors": ["slo"], "DetailLevel": "DETAILED" }),
            assessed: vec!["payments-health"],
            detailed: true,
            // The same objective is assessed, but the finding states the kind of entity the
            // target resolved, so a service target and an objective target differ.
            answer: "the payments entity in detail",
        },
        Case {
            case: "a different stored objective, audited in detail",
            targets: vec![target(
                "slo",
                json!({ "Slo": { "SloName": "checkout-latency" } }),
            )],
            extra: json!({ "Auditors": ["slo"], "DetailLevel": "DETAILED" }),
            assessed: vec!["checkout-latency"],
            detailed: true,
            answer: "the checkout objective in detail",
        },
        Case {
            case: "an objective that was never created",
            targets: vec![target("slo", json!({ "Slo": { "SloName": "no-such-slo" } }))],
            extra: json!({ "Auditors": ["slo"], "DetailLevel": "DETAILED" }),
            assessed: vec![],
            detailed: false,
            answer: "nothing to report",
        },
        Case {
            case: "a stored objective, audited by an auditor that reads traces",
            targets: vec![target("slo", json!({ "Slo": { "SloName": "payments-health" } }))],
            extra: json!({ "Auditors": ["trace"], "DetailLevel": "DETAILED" }),
            assessed: vec![],
            detailed: false,
            answer: "nothing to report",
        },
    ];

    // Each row's `AuditFindings`, so every pair can be compared once every request is made.
    let mut answers: Vec<(&'static str, &'static str, Value)> = Vec::new();
    for case in &cases {
        let (status, answer) =
            audit(&router, &window, case.targets.clone(), case.extra.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {answer:?}",
            case.case
        );
        assert_eq!(
            assessed_objectives(&answer),
            case.assessed,
            "case {:?}: expected the findings of {:?}, got {:?}",
            case.case,
            case.assessed,
            answer["AuditFindings"]
        );
        for finding in answer["AuditFindings"].as_array().unwrap_or(&Vec::new()) {
            assert_eq!(
                !finding["MetricGraph"].is_null(),
                case.detailed,
                "case {:?}: expected the DETAILED context to be present={}, got {:?}",
                case.case,
                case.detailed,
                finding["MetricGraph"]
            );
        }
        answers.push((case.case, case.answer, answer["AuditFindings"].clone()));
    }

    for (index, (case, question, findings)) in answers.iter().enumerate() {
        for (other_case, other_question, other_findings) in answers.iter().skip(index + 1) {
            if question == other_question {
                assert_eq!(
                    findings, other_findings,
                    "case {case:?} vs {other_case:?}: both ask {question:?}, so expected the \
                     same findings, got {findings:?} and {other_findings:?}"
                );
            } else {
                assert_ne!(
                    findings, other_findings,
                    "case {case:?} vs {other_case:?}: expected {question:?} and \
                     {other_question:?} to answer differently, got {findings:?} for both"
                );
            }
        }
    }
}

/// Each finding reports the entity the assessed objective monitors and the facts of the
/// stored record, and stops being reported once that record is gone.
///
/// The rows differ along the stored goal (the default the member documents, and an explicit
/// one) and along whether the objective names an operation of its entity.
#[tokio::test]
async fn each_finding_reports_the_stored_record_it_was_computed_from() {
    /// One stored objective and what the finding for it must report.
    struct Case {
        case: &'static str,
        name: &'static str,
        body: Value,
        entity: Value,
        operation: Option<&'static str>,
        /// `Goal.AttainmentGoal` as the stored record carries it: the model's documented
        /// default of 99 when the create named none.
        attainment_goal: &'static str,
    }

    let cases = [
        Case {
            case: "an objective on an operation, with the default goal",
            name: "checkout-latency",
            body: json!({
                "Name": "checkout-latency",
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": service_entity("checkout"),
                        "OperationName": "POST /checkout",
                        "MetricDataQueries": [metric_query("m1", "ApplicationSignals", "Latency")]
                    }
                }
            }),
            entity: service_entity("checkout"),
            operation: Some("POST /checkout"),
            attainment_goal: "99",
        },
        Case {
            case: "an objective on a whole service, with an explicit goal",
            name: "payments-health",
            body: json!({
                "Name": "payments-health",
                "Goal": { "AttainmentGoal": 99.9, "WarningThreshold": 30.0 },
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": service_entity("payments"),
                        "MetricDataQueries": [metric_query("m1", "ApplicationSignals", "Fault")]
                    }
                }
            }),
            entity: service_entity("payments"),
            operation: None,
            attainment_goal: "99.9",
        },
    ];

    for case in cases {
        let router = router();
        let arn = create_slo(&router, case.case, case.body.clone()).await;
        let now = now_seconds();
        let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);
        let targets = vec![target("slo", json!({ "Slo": { "SloName": case.name } }))];

        let (status, answer) = audit(&router, &window, targets.clone(), json!({})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {answer:?}",
            case.case
        );
        let finding = &answer["AuditFindings"][0];
        assert_eq!(
            finding["KeyAttributes"], case.entity,
            "case {:?}: expected the entity the objective monitors, got {:?}",
            case.case, finding["KeyAttributes"]
        );
        assert_eq!(
            finding["Operation"].as_str(),
            case.operation,
            "case {:?}: expected operation {:?}, got {:?}",
            case.case,
            case.operation,
            finding["Operation"]
        );
        assert_eq!(
            finding["AuditorResults"].as_array().map(Vec::len),
            Some(1),
            "case {:?}: expected exactly the SLO auditor's result, got {:?}",
            case.case,
            finding["AuditorResults"]
        );
        let result = &finding["AuditorResults"][0];
        assert_eq!(
            result["Auditor"].as_str(),
            Some("slo"),
            "case {:?}: expected the result to name the slo auditor, got {:?}",
            case.case,
            result["Auditor"]
        );
        // Nothing measured the objective's attainment, so the auditor observed no breached
        // threshold: the modelled severity for no issue, and the budget status of an
        // objective with no evaluated datapoint.
        assert_eq!(
            result["Severity"].as_str(),
            Some("NONE"),
            "case {:?}: expected severity NONE, got {:?}",
            case.case,
            result["Severity"]
        );
        assert_eq!(
            result["Data"]["BudgetStatus"].as_str(),
            Some("INSUFFICIENT_DATA"),
            "case {:?}: expected budget status INSUFFICIENT_DATA, got {:?}",
            case.case,
            result["Data"]["BudgetStatus"]
        );
        assert_eq!(
            result["Data"]["SloArn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: expected the stored ARN {arn}, got {:?}",
            case.case,
            result["Data"]["SloArn"]
        );
        assert_eq!(
            result["Data"]["AttainmentGoal"].as_str(),
            Some(case.attainment_goal),
            "case {:?}: expected attainment goal {:?}, got {:?}",
            case.case,
            case.attainment_goal,
            result["Data"]["AttainmentGoal"]
        );
        assert!(
            result["Description"]
                .as_str()
                .is_some_and(|description| description.contains(case.name)),
            "case {:?}: expected the description to name the assessed objective, got {:?}",
            case.case,
            result["Description"]
        );

        // Delete the record and the finding goes with it: the findings are computed from
        // what the store holds when the audit runs.
        let (status, deleted) = call(&router, "DELETE", &format!("/slo/{}", case.name), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the delete to answer 200, got {status} with {deleted:?}",
            case.case
        );
        let (status, after) = audit(&router, &window, targets, json!({})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 after the delete, got {status} with {after:?}",
            case.case
        );
        assert_eq!(
            assessed_objectives(&after),
            Vec::<String>::new(),
            "case {:?}: expected no finding once the objective is deleted, got {:?}",
            case.case,
            after["AuditFindings"]
        );
    }
}

/// `Auditors` selects which auditors' findings are reported: only findings generated by a
/// named auditor come back, and an absent list names no filter at all.
///
/// The rows differ along the auditor named: the one whose input this emulator holds, other
/// auditors the member documents (none of which has an input here), a name no auditor
/// carries, a list naming several, the empty list and no list at all.
#[tokio::test]
async fn the_auditors_filter_selects_which_findings_are_reported() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);
    let targets = vec![target("service", json!({ "Service": { "Name": "payments" } }))];

    // The auditor names are the ones the `Auditors` member documents
    // (`smithy_query(query="doc", shape="ListAuditFindingsInput", member="Auditors")`:
    // `slo`, `operation_metric`, `service_quota`, `trace`, `dependency_metric`,
    // `top_contributor`, `log`, `change_indicator`). Only `slo` has an input this emulator
    // holds.
    let cases = [
        ("no filter at all", json!({}), vec!["payments-health"]),
        (
            "the slo auditor",
            json!({ "Auditors": ["slo"] }),
            vec!["payments-health"],
        ),
        (
            "the slo auditor among others",
            json!({ "Auditors": ["trace", "slo", "top_contributor"] }),
            vec!["payments-health"],
        ),
        (
            "an auditor that reads RED metrics",
            json!({ "Auditors": ["operation_metric"] }),
            vec![],
        ),
        (
            "the service quota auditor",
            json!({ "Auditors": ["service_quota"] }),
            vec![],
        ),
        (
            "two auditors that read telemetry",
            json!({ "Auditors": ["log", "change_indicator"] }),
            vec![],
        ),
        (
            "a name no auditor carries",
            json!({ "Auditors": ["not-an-auditor"] }),
            vec![],
        ),
        ("a list naming no auditor", json!({ "Auditors": [] }), vec![]),
    ];

    for (case, extra, assessed) in cases {
        let (status, answer) = audit(&router, &window, targets.clone(), extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        assert_eq!(
            assessed_objectives(&answer),
            assessed,
            "case {case:?}: expected the findings of {assessed:?}, got {:?}",
            answer["AuditFindings"]
        );
    }
}

/// `DetailLevel` selects how much context a finding carries: the DETAILED level attaches the
/// metric graph of the objective that was assessed — its own stored queries, over the
/// audited window — and no other level does.
///
/// The rows differ along the detail level asked for (absent, each modelled value) and along
/// the indicator form the stored queries come from (a period-based one, and a request-based
/// one whose queries are the total and monitored request counts).
#[tokio::test]
async fn the_detail_level_selects_the_per_finding_context() {
    let router = router();
    create_slo(
        &router,
        "a period-based objective",
        json!({
            "Name": "checkout-latency",
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": service_entity("checkout"),
                    "MetricDataQueries": [metric_query("m1", "ApplicationSignals", "Latency")]
                }
            }
        }),
    )
    .await;
    create_slo(
        &router,
        "a request-based objective",
        json!({
            "Name": "checkout-requests",
            "RequestBasedSliConfig": {
                "RequestBasedSliMetricConfig": {
                    "KeyAttributes": service_entity("checkout"),
                    "TotalRequestCountMetric": [
                        metric_query("total", "ApplicationSignals", "Request")
                    ],
                    "MonitoredRequestCountMetric": {
                        "GoodCountMetric": [
                            metric_query("good", "ApplicationSignals", "Success")
                        ]
                    }
                }
            }
        }),
    )
    .await;

    let now = now_seconds();
    let (start, end) = (now - 3_600, now + 3_600);
    let window = format!("StartTime={start}&EndTime={end}");
    let targets = vec![target("service", json!({ "Service": { "Name": "checkout" } }))];

    // The queries each objective's indicator stores, by objective name, in the order the
    // graph reports them.
    let expected_queries = [
        ("checkout-latency", vec!["m1"]),
        ("checkout-requests", vec!["total", "good"]),
    ];

    for (case, extra, detailed) in [
        ("no detail level", json!({}), false),
        ("BRIEF", json!({ "DetailLevel": "BRIEF" }), false),
        ("DETAILED", json!({ "DetailLevel": "DETAILED" }), true),
    ] {
        let (status, answer) = audit(&router, &window, targets.clone(), extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        assert_eq!(
            assessed_objectives(&answer),
            vec!["checkout-latency", "checkout-requests"],
            "case {case:?}: expected both objectives assessed, got {:?}",
            answer["AuditFindings"]
        );
        for finding in answer["AuditFindings"].as_array().unwrap_or(&Vec::new()) {
            let assessed = finding["AuditorResults"][0]["Data"]["SloName"]
                .as_str()
                .unwrap_or("<no SloName>");
            let case = format!("{case} for {assessed}");
            if !detailed {
                assert!(
                    finding["MetricGraph"].is_null(),
                    "case {case:?}: expected no metric graph, got {:?}",
                    finding["MetricGraph"]
                );
                continue;
            }
            let graph = &finding["MetricGraph"];
            assert_eq!(
                (graph["StartTime"].as_f64(), graph["EndTime"].as_f64()),
                (Some(start as f64), Some(end as f64)),
                "case {case:?}: expected the graph to span the audited window, got {graph:?}"
            );
            let ids: Vec<String> = graph["MetricDataQueries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|query| query["Id"].as_str().unwrap_or("<no Id>").to_string())
                .collect();
            let expected = expected_queries
                .iter()
                .find(|(name, _)| *name == assessed)
                .map(|(_, ids)| ids.clone())
                .unwrap_or_default();
            assert_eq!(
                ids, expected,
                "case {case:?}: expected the objective's own stored queries {expected:?}, got \
                 {:?}",
                graph["MetricDataQueries"]
            );
        }
    }
}

/// An objective is assessed only over a window it was monitoring its entity in: the period
/// an objective is evidence for runs from its creation to now.
///
/// The rows differ along where the window sits relative to that period: covering it, ending
/// before the objective existed, and starting after now.
#[tokio::test]
async fn an_objective_is_assessed_only_over_a_window_it_monitored() {
    let router = router();
    create_slo(
        &router,
        "checkout latency",
        json!({
            "Name": "checkout-latency",
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": service_entity("checkout"),
                    "MetricDataQueries": [metric_query("m1", "ApplicationSignals", "Latency")]
                }
            }
        }),
    )
    .await;
    let now = now_seconds();

    for (case, start, end, assessed) in [
        (
            "a window covering the creation",
            now - 3_600,
            now + 3_600,
            vec!["checkout-latency"],
        ),
        (
            "a window ending before the objective existed",
            now - 7_200,
            now - 3_600,
            vec![],
        ),
        (
            "a window that has not begun",
            now + 3_600,
            now + 7_200,
            vec![],
        ),
    ] {
        for data in [
            json!({ "Slo": { "SloName": "checkout-latency" } }),
            json!({ "Service": { "Name": "checkout" } }),
        ] {
            let kind = if data.get("Slo").is_some() {
                "slo"
            } else {
                "service"
            };
            let case = format!("{case} audited by {kind}");
            let (status, answer) = audit(
                &router,
                &format!("StartTime={start}&EndTime={end}"),
                vec![target(kind, data)],
                json!({}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
            );
            assert_eq!(
                assessed_objectives(&answer),
                assessed,
                "case {case:?}: expected the findings of {assessed:?}, got {:?}",
                answer["AuditFindings"]
            );
        }
    }
}

/// A page carries at most `MaxResults` findings, the cursor walks every finding exactly
/// once, and a token this service did not issue for this projection is refused.
#[tokio::test]
async fn pages_are_bounded_and_the_cursor_walks_every_finding_once() {
    let router = router();
    // Three objectives on one entity, so a page of two leaves a second page to walk.
    let objectives = ["audit-a", "audit-b", "audit-c"];
    for name in objectives {
        create_slo(
            &router,
            name,
            json!({
                "Name": name,
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": service_entity("checkout"),
                        "OperationName": name,
                        "MetricDataQueries": [metric_query("m1", "ApplicationSignals", "Fault")]
                    }
                }
            }),
        )
        .await;
    }
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);
    // `MaxResults` and `NextToken` are BODY members of this operation (`OPERATIONS.md`:
    // neither carries an `@httpQuery` binding, unlike its `StartTime`/`EndTime`).
    let targets = json!([
        { "Type": "service", "Data": { "Service": { "Name": "checkout" } } }
    ]);

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..objectives.len() + 1 {
        let case = format!("page {page}");
        let mut body = json!({ "AuditTargets": targets, "MaxResults": 2 });
        if let Some(token) = token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let (status, answer) = call(
            &router,
            "POST",
            &format!("/auditFindings?{window}"),
            Some(body),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        let findings = assessed_objectives(&answer);
        assert!(
            findings.len() <= 2,
            "case {case:?}: expected at most MaxResults=2 findings, got {findings:?}"
        );
        walked.extend(findings);
        token = answer["NextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, objectives,
        "case \"paged walk\": expected every objective exactly once in name order, got {walked:?}"
    );
    assert!(
        token.is_none(),
        "case \"paged walk\": expected the last page to carry no NextToken, got {token:?}"
    );

    let (status, answer) = call(
        &router,
        "POST",
        &format!("/auditFindings?{window}"),
        Some(json!({
            "AuditTargets": targets,
            "MaxResults": 2,
            "NextToken": "not-a-token"
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"forged token\": expected HTTP 400, got {status} with {answer:?}"
    );
    assert!(
        answer["message"]
            .as_str()
            .is_some_and(|message| message.contains("NextToken")),
        "case \"forged token\": expected the refusal to name NextToken, got {:?}",
        answer["message"]
    );
}
