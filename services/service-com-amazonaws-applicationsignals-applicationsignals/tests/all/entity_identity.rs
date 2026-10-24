// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the ENTITY-IDENTITY rule the `KeyAttributes` member carries on
//! every read that takes it as the entity to report on: `GetService`,
//! `ListServiceOperations`, `ListServiceDependencies` and `ListServiceDependents`.
//!
//! The rule under test: a key-attribute map that is supposed to identify an entity must name
//! the `Type` attribute that "designates the type of object this is", with one of the entity
//! types the member's doc names — `Service`, `RemoteService`, `AWS::Service`, `Resource`,
//! `AWS::Resource`
//! (`smithy_query(query="doc",shape="GetServiceInput",member="KeyAttributes")`, the same text
//! on `ListServiceOperationsInput`, `ListServiceDependenciesInput` and
//! `ListServiceDependentsInput`). A map that names no `Type`, or a `Type` outside that set,
//! identifies no entity and is refused with the modelled `ValidationException` (HTTP 400)
//! rather than answered 200 with the caller's own map echoed back as a discovered entity.
//!
//! The boundary matters too: the same member is NOT an entity lookup everywhere it appears.
//! `ListServiceLevelObjectives` takes it as an optional FILTER whose doc drops the "must
//! specify" sentence, and an SLO's own indicator records the object the SLO relates to — so
//! neither carries the rule, which the last test here pins.
//!
//! Every case drives the real `restJson1` surface through the router production mounts, so
//! the statuses and bodies asserted here are the ones a client sees. (The harness below is
//! this file's own, as in every other suite in this crate: the suites are separate modules of
//! one test target and none of them exports its helpers.)

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// Every read that takes `KeyAttributes` as the entity to report on: the route, and the
/// output member carrying its entries (`None` for `GetService`, which reports one entity).
///
/// From `smithy_query(query="digest")`: the four operations whose `@required`
/// `KeyAttributes` doc opens with "Use this field to specify which service you want to
/// retrieve information for. You must specify at least the Type, Name, and Environment
/// attributes."
const ENTITY_READS: [(&str, &str, Option<&str>); 4] = [
    ("GetService", "/service", None),
    (
        "ListServiceOperations",
        "/service-operations",
        Some("ServiceOperations"),
    ),
    (
        "ListServiceDependencies",
        "/service-dependencies",
        Some("ServiceDependencies"),
    ),
    (
        "ListServiceDependents",
        "/service-dependents",
        Some("ServiceDependents"),
    ),
];

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
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
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

/// Seconds since the epoch, so every read below asks for a window that has begun and has
/// not ended: the identity rule is about the body, so no case may be refused for its range.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The query string every read below carries: a window around now, in the epoch-seconds form
/// the members document for the raw HTTP API.
fn window() -> String {
    let now = now_seconds();
    format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600)
}

/// Create one SLO whose indicator monitors `key_attributes`, so the reads below run against
/// a store that actually holds an entity.
async fn create_slo(router: &MultiServiceRouter, name: &str, key_attributes: &Value) {
    let (status, answer, _) = call(
        router,
        "POST",
        "/slo",
        Some(json!({
            "Name": name,
            "SliConfig": { "SliMetricConfig": {
                "KeyAttributes": key_attributes,
                "MetricType": "LATENCY"
            }}
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected the SLO create to answer 200, got {status} with {answer}"
    );
}

/// A key-attribute map that names no entity `Type`, or a `Type` outside the documented
/// entity types, is refused by EVERY entity read with the modelled `ValidationException`.
///
/// The rows differ along what the rule generalises over: whether `Type` is absent or carries
/// an unusable value, the SIZE of the map (one entry to the modelled maximum of four), WHICH
/// other attributes it names (`Name`, `Environment`, `ResourceType`, `Identifier`), the
/// CASING of the key (`type` is not `Type`) and the kind of near miss in the value (an
/// unknown type, a documented type in the wrong case, a documented type with a trailing
/// space, a documented type with a plural, and an `AWS::` type that is not one of the two).
/// Each row is sent to all four reads.
#[tokio::test]
async fn an_identity_that_names_no_entity_type_is_refused_by_every_entity_read() {
    /// The requirement a map with NO `Type` violates: the identity names no kind of object.
    const ABSENT: &str = "Member must contain the entity type attribute 'Type'";
    /// The requirement a map whose `Type` is not a documented entity type violates.
    const NOT_AN_ENTITY_TYPE: &str = "Member must satisfy enum value set: \
         [AWS::Resource, AWS::Service, RemoteService, Resource, Service]";

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no Type at all: a single mis-spelled attribute",
            json!({ "Colour": "blue" }),
            ABSENT,
        ),
        ("no Type: the name alone", json!({ "Name": "checkout" }), ABSENT),
        (
            "no Type: the environment alone",
            json!({ "Environment": "eks:prod/default" }),
            ABSENT,
        ),
        (
            "no Type: name and environment, two of the three attributes the doc names",
            json!({ "Name": "checkout", "Environment": "eks:prod/default" }),
            ABSENT,
        ),
        (
            "no Type: a resource identity's three other attributes",
            json!({
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table",
                "Environment": "eks:prod/default"
            }),
            ABSENT,
        ),
        (
            "the key is lower-case 'type', which is a different attribute",
            json!({ "type": "Service", "Name": "checkout", "Environment": "eks:prod/default" }),
            ABSENT,
        ),
        (
            "a Type outside the documented set",
            json!({ "Type": "NotAnEntityType", "Name": "checkout", "Environment": "eks:prod/default" }),
            NOT_AN_ENTITY_TYPE,
        ),
        (
            "a documented type in the wrong case",
            json!({ "Type": "service", "Name": "checkout", "Environment": "eks:prod/default" }),
            NOT_AN_ENTITY_TYPE,
        ),
        (
            "a documented type with a trailing space",
            json!({ "Type": "Service ", "Name": "checkout", "Environment": "eks:prod/default" }),
            NOT_AN_ENTITY_TYPE,
        ),
        (
            "a documented type pluralised",
            json!({ "Type": "AWS::Services", "Name": "checkout", "Environment": "eks:prod/default" }),
            NOT_AN_ENTITY_TYPE,
        ),
        (
            "an AWS:: type that is neither AWS::Service nor AWS::Resource",
            json!({
                "Type": "AWS::Logs::LogGroup",
                "ResourceType": "AWS::Logs::LogGroup",
                "Identifier": "app-logs",
                "Environment": "eks:prod/default"
            }),
            NOT_AN_ENTITY_TYPE,
        ),
        (
            "a four-attribute map whose Type is unusable",
            json!({
                "Type": "Entity",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table",
                "Environment": "eks:prod/default"
            }),
            NOT_AN_ENTITY_TYPE,
        ),
    ];

    let router = router();
    // A store that holds a real entity, so a refusal cannot be the "nothing is stored" path.
    create_slo(
        &router,
        "checkout-latency",
        &json!({ "Type": "Service", "Name": "checkout", "Environment": "eks:prod/default" }),
    )
    .await;

    for (label, key_attributes, requirement) in cases {
        for (operation, path, _) in ENTITY_READS {
            let case = format!("{operation}: {label}");
            let (status, body, errortype) = call(
                &router,
                "POST",
                &format!("{path}?{}", window()),
                Some(json!({ "KeyAttributes": key_attributes })),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert!(
                errortype.starts_with("ValidationException"),
                "case {case:?}: expected a ValidationException errortype, got {errortype:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("validation error detected")
                    && message.contains("'keyAttributes")
                    && message.contains(requirement),
                "case {case:?}: expected a validation envelope naming 'keyAttributes' and \
                 {requirement:?}, got {message:?}"
            );
            // A refusal names the requirement and NOT the value it rejected, which is caller
            // input every client log of the failure would otherwise copy.
            assert!(
                !message.contains("checkout") && !message.contains("blue"),
                "case {case:?}: expected the refusal to name no rejected value, got {message:?}"
            );
        }
    }
}

/// An identity that DOES name a documented entity type is served by every entity read, so the
/// rule refuses only what identifies no entity.
///
/// The rows differ along the `Type` itself — all five values the member's doc names, each
/// tested here including the ones the issue did not cite — and along the SIZE and shape of
/// the identity: the type on its own, a two-attribute identity, the three attributes the doc
/// names for a service, and the modelled maximum of four for a resource.
#[tokio::test]
async fn an_identity_that_names_a_documented_entity_type_is_served() {
    // The documented entity types, from
    // `smithy_query(query="doc",shape="GetServiceInput",member="KeyAttributes")`: `Name` is
    // used for `Service`, `RemoteService` and `AWS::Service`, and `ResourceType`/`Identifier`
    // for `Resource` and `AWS::Resource`.
    let cases: Vec<(&str, Value)> = vec![
        ("the entity type on its own", json!({ "Type": "Service" })),
        (
            "Service, with the three attributes the doc names",
            json!({ "Type": "Service", "Name": "checkout", "Environment": "eks:prod/default" }),
        ),
        (
            "RemoteService, named and placed",
            json!({ "Type": "RemoteService", "Name": "payments", "Environment": "eks:prod/default" }),
        ),
        (
            "AWS::Service, by name alone beside its type",
            json!({ "Type": "AWS::Service", "Name": "sqs" }),
        ),
        (
            "Resource, by the attributes its type makes applicable",
            json!({
                "Type": "Resource",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table",
                "Environment": "eks:prod/default"
            }),
        ),
        (
            "AWS::Resource, the modelled maximum of four attributes",
            json!({
                "Type": "AWS::Resource",
                "ResourceType": "AWS::Logs::LogGroup",
                "Identifier": "app-logs",
                "Environment": "eks:prod/default"
            }),
        ),
    ];

    let router = router();
    create_slo(
        &router,
        "checkout-latency",
        &json!({ "Type": "Service", "Name": "checkout", "Environment": "eks:prod/default" }),
    )
    .await;

    for (label, key_attributes) in cases {
        for (operation, path, member) in ENTITY_READS {
            let case = format!("{operation}: {label}");
            let (status, body, _) = call(
                &router,
                "POST",
                &format!("{path}?{}", window()),
                Some(json!({ "KeyAttributes": key_attributes })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
            match member {
                // The entity is reported by the identity the request named, at least.
                None => assert_eq!(
                    body["Service"]["KeyAttributes"]["Type"], key_attributes["Type"],
                    "case {case:?}: expected the entity's Type back, got {}",
                    body["Service"]["KeyAttributes"]
                ),
                // The list is present, whether or not the store holds an entry for the
                // identity: each is a `@required` output member.
                Some(member) => assert!(
                    body[member].is_array(),
                    "case {case:?}: expected a present {member}, got {}",
                    body[member]
                ),
            }
        }
    }
}

/// The rule belongs to the member that IDENTIFIES the entity a read reports on, and nowhere
/// else: `ListServiceLevelObjectives` takes an optional FILTER over the same shape, and an
/// SLO's indicator records the object the SLO relates to. Neither requires an entity `Type`,
/// so neither is refused for the absence of one.
///
/// The rows differ along the member under test and along why it carries no entity `Type`: a
/// filter naming only a service name, a filter naming only an environment, an SLO whose
/// monitored identity names no `Type`, and an SLO whose metric source names a `Type` that is
/// an AWS resource type rather than an Application Signals entity type.
#[tokio::test]
async fn a_filter_and_an_slos_own_identity_need_no_entity_type() {
    let router = router();

    // An SLO may record an identity with no entity `Type`, and one whose metric source names
    // an AWS resource type: `smithy_query(query="doc",shape="ServiceLevelIndicatorMetric")`
    // drops the "must specify" sentence the entity reads' member carries, and a
    // `MetricSource` identifies "the metric source for SLOs on resources other than
    // Application Signals services", whose `Type` is that resource's type.
    for (label, name, metric_config) in [
        (
            "an SLO whose monitored identity names no Type",
            "billing-latency",
            json!({ "KeyAttributes": { "Name": "billing" }, "MetricType": "LATENCY" }),
        ),
        (
            "an SLO whose metric source names an AWS resource type",
            "web-latency",
            json!({
                "KeyAttributes": { "Type": "Service", "Name": "web", "Environment": "prod" },
                "MetricSource": {
                    "MetricSourceKeyAttributes": { "Type": "AWS::RUM::AppMonitor", "Name": "web" }
                }
            }),
        ),
    ] {
        let (status, answer, _) = call(
            &router,
            "POST",
            "/slo",
            Some(json!({
                "Name": name,
                "SliConfig": { "SliMetricConfig": metric_config }
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the create to answer 200, got {status} with {answer}"
        );
    }

    for (label, filter) in [
        ("a filter naming only a service name", json!({ "Name": "billing" })),
        ("a filter naming only an environment", json!({ "Environment": "prod" })),
    ] {
        let (status, answer, _) = call(
            &router,
            "POST",
            "/slos",
            Some(json!({ "KeyAttributes": filter })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the SLO listing to answer 200, got {status} with {answer}"
        );
        assert!(
            answer["SloSummaries"].is_array(),
            "case {label:?}: expected a present SloSummaries, got {}",
            answer["SloSummaries"]
        );
    }
}
