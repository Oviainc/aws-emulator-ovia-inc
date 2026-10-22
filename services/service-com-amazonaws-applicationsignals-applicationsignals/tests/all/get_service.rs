// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetService` and of the refusal every route that binds a
//! `@required` member to the QUERY STRING owes a request that omits it.
//!
//! The time-ranged reads' own behaviour — both documented timestamp forms, the inverted
//! range, the topology the stored SLOs are evidence of — is covered in
//! `time_ranged_reads.rs`. What is covered here is the case where a required query member is
//! ABSENT: the method and path still name the operation, so the answer is the modelled
//! `ValidationException` naming the absent member, never the 501 `NotImplemented` the
//! framework answers a request that names no operation at all.
//!
//! Every test drives the real `restJson1` surface through the router that production mounts,
//! so the routes, the status codes and the JSON bodies are the ones a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
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

/// Send one request and return its status, parsed JSON body (`Value::Null` when empty) and
/// `x-amzn-errortype` header (empty when absent).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value, String) {
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
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed, errortype)
}

/// Seconds since the epoch, so a window can be placed around the instant a stored SLO is
/// created at — an SLO is evidence of its entity only for a window that has begun and does
/// not end before the SLO existed.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The start of the window every read below asks for: an hour ago, in the epoch-seconds form
/// the members document for the raw HTTP API.
fn window_start() -> String {
    (now_seconds() - 3_600).to_string()
}

/// The end of that window: an hour ahead, so a record created now falls inside it.
fn window_end() -> String {
    (now_seconds() + 3_600).to_string()
}

/// `GetService` over a window, for the given key attributes. An empty `start` or `end` omits
/// that query member entirely.
async fn get_service(
    router: &MultiServiceRouter,
    start: &str,
    end: &str,
    key_attributes: Option<Value>,
) -> (u16, Value, String) {
    let mut query: Vec<String> = Vec::new();
    if !start.is_empty() {
        query.push(format!("StartTime={start}"));
    }
    if !end.is_empty() {
        query.push(format!("EndTime={end}"));
    }
    let body = key_attributes.map(|attributes| json!({ "KeyAttributes": attributes }));
    call(
        router,
        "POST",
        &format!("/service?{}", query.join("&")),
        body,
    )
    .await
}

/// Create one SLO whose indicator monitors `key_attributes`, failing the case loudly if the
/// create itself did not persist it.
async fn create_slo(router: &MultiServiceRouter, case: &str, name: &str, key_attributes: &Value) {
    let body = json!({
        "Name": name,
        "SliConfig": {
            "SliMetricConfig": {
                "KeyAttributes": key_attributes,
                "MetricType": "LATENCY"
            }
        }
    });
    let (status, answer, _) = call(router, "POST", "/slo", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the SLO create to answer 200, got {status} with {answer}"
    );
}

/// A request that omits a `@required` `@httpQuery` member is refused with the modelled
/// HTTP 400 `ValidationException` naming that member — on every route of this service that
/// binds one, and never with the 501 `NotImplemented` that means "this names no operation".
///
/// The rows differ along what the rule generalises over: the route and its method (POST and
/// GET), which required query member is omitted (`StartTime`, `EndTime`, both, and a
/// non-timestamp one — `ResourceArn`), and whether the other query members are present. Each
/// route's required query members come from
/// `smithy_query(query="operation",operation="GetService,ListServices,ListServiceOperations,
/// ListServiceDependencies,ListServiceDependents,ListAuditFindings,ListTagsForResource")`.
#[tokio::test]
async fn an_absent_required_query_member_is_refused_as_a_validation_error() {
    let entity = json!({ "Type": "Service", "Name": "checkout", "Environment": "eks:prod/default" });
    let start = window_start();
    let end = window_end();

    struct Case {
        label: &'static str,
        method: &'static str,
        /// The path with its query string, already assembled.
        uri: String,
        body: Option<Value>,
        /// The member path the refusal must name, in the lowerCamel casing the
        /// `ValidationException` envelope states a member with.
        names: &'static str,
    }

    let cases = vec![
        Case {
            label: "GetService without EndTime",
            method: "POST",
            uri: format!("/service?StartTime={start}"),
            body: Some(json!({ "KeyAttributes": entity })),
            names: "'endTime'",
        },
        Case {
            label: "GetService without StartTime",
            method: "POST",
            uri: format!("/service?EndTime={end}"),
            body: Some(json!({ "KeyAttributes": entity })),
            names: "'startTime'",
        },
        Case {
            label: "GetService without either timestamp",
            method: "POST",
            uri: "/service".to_string(),
            body: Some(json!({ "KeyAttributes": entity })),
            names: "'startTime'",
        },
        Case {
            label: "ListServices without EndTime",
            method: "GET",
            uri: format!("/services?StartTime={start}"),
            body: None,
            names: "'endTime'",
        },
        Case {
            label: "ListServiceOperations without StartTime",
            method: "POST",
            uri: format!("/service-operations?EndTime={end}&MaxResults=10"),
            body: Some(json!({ "KeyAttributes": entity })),
            names: "'startTime'",
        },
        Case {
            label: "ListServiceDependencies without EndTime",
            method: "POST",
            uri: format!("/service-dependencies?StartTime={start}"),
            body: Some(json!({ "KeyAttributes": entity })),
            names: "'endTime'",
        },
        Case {
            label: "ListServiceDependents without EndTime",
            method: "POST",
            uri: format!("/service-dependents?StartTime={start}"),
            body: Some(json!({ "KeyAttributes": entity })),
            names: "'endTime'",
        },
        Case {
            label: "ListAuditFindings without EndTime",
            method: "POST",
            uri: format!("/auditFindings?StartTime={start}"),
            body: Some(json!({
                "AuditTargets": [{ "Type": "service", "Data": { "Service": entity } }]
            })),
            names: "'endTime'",
        },
        Case {
            label: "ListTagsForResource without ResourceArn",
            method: "GET",
            uri: "/tags".to_string(),
            body: None,
            names: "'resourceArn'",
        },
    ];

    for case in cases {
        let router = router();
        let (status, body, errortype) =
            call(&router, case.method, &case.uri, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} with {body}",
            case.label
        );
        assert!(
            errortype.starts_with("ValidationException"),
            "case {:?}: expected a ValidationException errortype, got {errortype:?}",
            case.label
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains("validation error detected")
                && message.contains(case.names)
                && message.contains("Member must not be null"),
            "case {:?}: expected a validation envelope naming {} as absent, got {message:?}",
            case.label,
            case.names
        );
    }
}

/// An input the model refuses on a member it does NOT bind to the query string is refused the
/// same way, so the refusal does not depend on where a member rides.
///
/// The rows differ along which modelled constraint the body breaks: the `@required`
/// `KeyAttributes` absent, and the `Attributes` map on either side of its `length 1..4`.
#[tokio::test]
async fn get_service_refuses_a_body_the_model_rejects() {
    // Five entries: one past the modelled `Attributes` maximum of four.
    let five_attributes = json!({
        "Type": "Service",
        "Name": "checkout",
        "Environment": "eks:prod/default",
        "Identifier": "checkout-7f9",
        "ResourceType": "AWS::ECS::Service"
    });

    let cases: Vec<(&str, Option<Value>, &str)> = vec![
        ("KeyAttributes absent", None, "Member must not be null"),
        (
            "KeyAttributes empty, below the modelled minimum of one",
            Some(json!({})),
            "length greater than or equal to 1",
        ),
        (
            "KeyAttributes above the modelled maximum of four",
            Some(five_attributes),
            "length less than or equal to 4",
        ),
    ];

    for (label, key_attributes, constraint) in cases {
        let router = router();
        let (status, body, errortype) =
            get_service(&router, &window_start(), &window_end(), key_attributes).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} with {body}"
        );
        assert!(
            errortype.starts_with("ValidationException"),
            "case {label:?}: expected a ValidationException errortype, got {errortype:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("validation error detected")
                && message.contains("'keyAttributes'")
                && message.contains(constraint),
            "case {label:?}: expected a validation envelope naming 'keyAttributes' and \
             {constraint:?}, got {message:?}"
        );
    }
}

/// A well-formed `GetService` reports the entity its key attributes identify: the fullest
/// attribute set this emulator holds for it, and the request's own identity when it holds
/// none.
///
/// The rows differ along how much of the entity the request names (all of it, a subset of a
/// four-attribute entity, an identity no record monitors) and along the entity's own size
/// (three attributes for a service, the modelled maximum of four for a resource).
#[tokio::test]
async fn get_service_reports_the_entity_its_key_attributes_identify() {
    // A resource entity uses all four attributes `Attributes` (`length 1..4`) admits.
    let resource = json!({
        "Type": "AWS::Resource",
        "ResourceType": "AWS::DynamoDB::Table",
        "Identifier": "orders-table",
        "Environment": "eks:prod/default"
    });
    let service = json!({ "Type": "Service", "Name": "checkout", "Environment": "eks:prod/default" });

    struct Case {
        label: &'static str,
        /// The entity a stored SLO monitors, if any.
        monitored: Option<Value>,
        /// The `KeyAttributes` the read sends.
        request: Value,
        /// The `Service.KeyAttributes` the response must carry.
        reported: Value,
    }

    let cases = vec![
        Case {
            label: "the request names the whole monitored identity",
            monitored: Some(service.clone()),
            request: service.clone(),
            reported: service.clone(),
        },
        Case {
            label: "the request names a subset of a four-attribute entity",
            monitored: Some(resource.clone()),
            request: json!({
                "Type": "AWS::Resource",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table"
            }),
            reported: resource.clone(),
        },
        Case {
            label: "no record monitors the identity: it is reported as asked",
            monitored: None,
            request: service.clone(),
            reported: service.clone(),
        },
        Case {
            label: "a record monitors another entity entirely",
            monitored: Some(resource),
            request: service.clone(),
            reported: service,
        },
    ];

    for case in cases {
        let router = router();
        if let Some(monitored) = case.monitored.as_ref() {
            create_slo(&router, case.label, "checkout-latency", monitored).await;
        }

        let start = window_start();
        let end = window_end();
        let (status, body, _) =
            get_service(&router, &start, &end, Some(case.request.clone())).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body}",
            case.label
        );
        assert_eq!(
            body["Service"]["KeyAttributes"], case.reported,
            "case {:?}: expected KeyAttributes {}, got {}",
            case.label, case.reported, body["Service"]["KeyAttributes"]
        );
        assert_eq!(
            body["StartTime"].as_f64(),
            start.parse::<f64>().ok(),
            "case {:?}: expected the requested StartTime back, got {}",
            case.label,
            body["StartTime"]
        );
        assert_eq!(
            body["EndTime"].as_f64(),
            end.parse::<f64>().ok(),
            "case {:?}: expected the requested EndTime back, got {}",
            case.label,
            body["EndTime"]
        );
    }
}

/// `GetService` is a read: it answers identically however often it is called, it does not
/// bring an entity into existence, and it leaves the records it derived the entity from
/// exactly as they were.
#[tokio::test]
async fn get_service_changes_nothing_a_later_caller_sees() {
    let case = "a read leaves the store untouched";
    let router = router();
    let entity = json!({ "Type": "Service", "Name": "checkout", "Environment": "eks:prod/default" });
    let start = window_start();
    let end = window_end();

    // Reading an entity nothing monitors must not register it either: the SLO listing stays
    // empty and a later read answers exactly as the first one did.
    let (first_status, first, _) = get_service(&router, &start, &end, Some(entity.clone())).await;
    let (second_status, second, _) = get_service(&router, &start, &end, Some(entity.clone())).await;
    assert_eq!(
        (first_status, &first),
        (second_status, &second),
        "case {case:?}: two reads of one entity answered differently"
    );
    assert_eq!(
        first_status, 200,
        "case {case:?}: expected 200, got {first_status} with {first}"
    );

    create_slo(&router, case, "checkout-latency", &entity).await;

    let (third_status, third, _) = get_service(&router, &start, &end, Some(entity.clone())).await;
    let (fourth_status, fourth, _) = get_service(&router, &start, &end, Some(entity.clone())).await;
    assert_eq!(
        (third_status, &third),
        (fourth_status, &fourth),
        "case {case:?}: two reads after the create answered differently"
    );

    // The record the entity was read from is still the single stored SLO, unchanged.
    let (status, page, _) = call(
        &router,
        "POST",
        "/slos",
        Some(json!({ "KeyAttributes": entity })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the SLO listing to answer 200, got {status} with {page}"
    );
    let names: Vec<String> = page["SloSummaries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["checkout-latency".to_string()],
        "case {case:?}: expected the stored SLO to be untouched by the reads, got {names:?}"
    );
}
