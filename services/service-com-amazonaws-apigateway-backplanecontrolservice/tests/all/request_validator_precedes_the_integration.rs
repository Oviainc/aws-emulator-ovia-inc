// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN a deployed method's `RequestValidator` decides a request:
//! before the method's integration is reached, whatever backend that integration names.
//!
//! The RULE under test, over a SET of inputs rather than one recorded call: a method whose
//! `requestValidatorId` names a body-validating `RequestValidator` answers a payload the
//! `Model` its `requestModels` maps to the request's content type does not admit with the
//! `BAD_REQUEST_BODY` gateway response, and the integration is not performed for that
//! request at all — so the integration's own answer cannot reach a caller whose payload the
//! model refused.
//!
//! The rows span the dimension that rule generalises over: WHICH integration the method
//! points at. A `MOCK` is served in full from the api's own records, while every other
//! integration type names a backend this emulator cannot call and is therefore answered
//! through the api's `API_CONFIGURATION_ERROR` gateway response — two visibly different
//! answers for a request that REACHES the integration, and neither of them is what a
//! refused payload earns. Each row is driven twice: once with a payload the model admits,
//! which must get the integration's own answer, and once with each way a payload fails the
//! model, which must get the validator's 400. The pair is what shows the order — the same
//! route answers the integration for one payload and refuses the other before it.
//!
//! Every case drives the real wire path — an unsigned request to the api's own invoke host
//! through the multi-service router, with the control plane driven through the same
//! emulator — and reuses this crate's one validator harness from `stage_request_validation`
//! and its `Model` reader from `models`, so the model under test is stood up and read back
//! exactly as the rest of the suite does it.

use serde_json::{json, Value};

use crate::execute_api_data_plane::{integration_response, method_response, put_integration, put_method};
use crate::models::get_model;
use crate::rest_api_lifecycle::{create_api, resource_under, string_member, Gateway};
use crate::stage_request_validation::{
    create_model, create_validator, deploy, invoke, order_schema, GOOD_BODY, JSON, STAGE,
};
use crate::test_invoke_method::lambda_uri;

/// The model every method below declares for [`JSON`] payloads.
const MODEL: &str = "Order";

/// The body the `MOCK` route answers with, so a served call is distinguishable from a
/// refused one by its body alone.
const MOCK_SERVED_BODY: &str = r#"{"integration":"mock"}"#;

/// The sentence the `API_CONFIGURATION_ERROR` gateway response carries, which is what a
/// call that REACHED an integration this emulator cannot perform is answered with.
const CONFIGURATION_ERROR: &str = "Internal server error";

/// The sentence the `BAD_REQUEST_BODY` gateway response carries, which is what a payload
/// the method's model does not admit is refused with.
const INVALID_BODY: &str = "Invalid request body";

/// One integration a method under test points at.
struct Backend {
    /// The case name, and the route the method is put on.
    case: &'static str,
    route: &'static str,
    /// What `PutIntegration` stores for the method.
    integration: Value,
    /// The status and `message` a request the validator ADMITS is answered with — the
    /// integration's own answer, since such a request reaches it.
    admitted_status: u16,
    admitted_body: &'static str,
}

/// The integration types a method may point at, each with what a request that gets past
/// the validator earns from it. `MOCK` is the one type served from records; the others name
/// a backend this emulator cannot call, so reaching them is itself observable.
fn backends() -> Vec<Backend> {
    vec![
        Backend {
            case: "a Lambda proxy integration",
            route: "lambdaproxy",
            integration: json!({
                "type": "AWS_PROXY",
                "httpMethod": "POST",
                "uri": lambda_uri(),
            }),
            admitted_status: 500,
            admitted_body: CONFIGURATION_ERROR,
        },
        Backend {
            case: "a Lambda non-proxy integration",
            route: "lambdaaws",
            integration: json!({
                "type": "AWS",
                "httpMethod": "POST",
                "uri": lambda_uri(),
                "requestTemplates": { JSON: r#"{"statusCode": 200}"# },
            }),
            admitted_status: 500,
            admitted_body: CONFIGURATION_ERROR,
        },
        Backend {
            case: "an HTTP proxy integration",
            route: "httpproxy",
            integration: json!({
                "type": "HTTP_PROXY",
                "httpMethod": "POST",
                "uri": "https://orders.example.com/orders",
            }),
            admitted_status: 500,
            admitted_body: CONFIGURATION_ERROR,
        },
        Backend {
            case: "a MOCK integration",
            route: "mock",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": { JSON: r#"{"statusCode": 200}"# },
            }),
            admitted_status: 200,
            admitted_body: MOCK_SERVED_BODY,
        },
    ]
}

/// Stand a `POST` route up under the root: the method declaring `MODEL` for [`JSON`] and
/// `validator` as its request validator, the integration the row names, and the 200 method
/// and integration responses a served call maps to.
async fn validated_route(
    gateway: &Gateway,
    api: &str,
    root: &str,
    backend: &Backend,
    validator: &str,
) {
    let resource = resource_under(gateway, api, root, backend.route).await;
    put_method(
        gateway,
        api,
        &resource,
        "POST",
        json!({
            "authorizationType": "NONE",
            "requestModels": { JSON: MODEL },
            "requestValidatorId": validator,
        }),
    )
    .await;
    put_integration(gateway, api, &resource, "POST", backend.integration.clone()).await;
    method_response(gateway, api, &resource, "POST", "200", json!({})).await;
    integration_response(
        gateway,
        api,
        &resource,
        "POST",
        "200",
        json!({ "responseTemplates": { JSON: MOCK_SERVED_BODY } }),
    )
    .await;
}

/// The ways a payload fails [`order_schema`], as `(case, payload)`. The rows differ along
/// WHAT about the payload the model refuses — a missing property, a wrong type, a value
/// outside the declared bound, an undeclared property, a document that is not JSON at all,
/// an empty one, and a JSON value that is not an object.
const REFUSED_PAYLOADS: &[(&str, &str)] = &[
    ("a required property absent", r#"{"id":"a1"}"#),
    ("every required property absent", "{}"),
    ("a property of the wrong type", r#"{"id":"a1","qty":"two"}"#),
    ("a number below the declared minimum", r#"{"id":"a1","qty":0}"#),
    (
        "a property the schema does not declare",
        r#"{"id":"a1","qty":1,"extra":true}"#,
    ),
    ("a payload that is not a JSON document", "not json at all"),
    ("an empty payload", ""),
    ("a JSON value that is not an object", "[1,2,3]"),
];

#[tokio::test]
async fn a_payload_the_model_refuses_never_reaches_the_methods_integration() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "validator-before-integration").await;
    create_model(&gateway, &api, MODEL, order_schema()).await;
    // The record the validator consumes is the one `CreateModel` stored: read back through
    // `GetModel` before any invoke, so a route that refuses a payload is known to be
    // refusing it against the stored schema rather than against nothing.
    let stored = get_model(&gateway, &api, MODEL, false).await;
    assert_eq!(
        stored.status, 200,
        "case GetModel({MODEL}): expected 200, got {} with {:?}",
        stored.status, stored.body
    );
    assert_eq!(
        string_member(&stored.body, "schema", "GetModel"),
        order_schema().to_string(),
        "case GetModel({MODEL}): expected the stored schema verbatim, got {:?}",
        stored.body
    );
    assert_eq!(
        string_member(&stored.body, "contentType", "GetModel"),
        JSON,
        "case GetModel({MODEL}): expected the content type the model was created with, got {:?}",
        stored.body
    );

    let validator = create_validator(&gateway, &api, "bodyonly", true, false).await;
    let backends = backends();
    for backend in &backends {
        validated_route(&gateway, &api, &root, backend, &validator).await;
    }
    deploy(&gateway, &api).await;

    for backend in &backends {
        let path = format!("/{STAGE}/{}", backend.route);
        // A payload the model ADMITS reaches the integration, so this row's own answer is
        // the integration's: the 200 a `MOCK` serves from records, or the
        // API_CONFIGURATION_ERROR a backend this emulator cannot call earns. This is the
        // control the refusals below are read against.
        let admitted = invoke(
            &gateway,
            &api,
            "POST",
            &path,
            &[("content-type", JSON)],
            GOOD_BODY,
        )
        .await;
        assert_eq!(
            admitted.status, backend.admitted_status,
            "case {} with a payload the model admits: expected {}, got {} with {:?}",
            backend.case, backend.admitted_status, admitted.status, admitted.body
        );
        assert_eq!(
            admitted.message(),
            backend.admitted_body,
            "case {} with a payload the model admits: expected the integration's own answer \
             {:?}, got {:?}",
            backend.case,
            backend.admitted_body,
            admitted.message()
        );

        for (payload_case, payload) in REFUSED_PAYLOADS {
            let case = format!("{} with {payload_case}", backend.case);
            let answered = invoke(
                &gateway,
                &api,
                "POST",
                &path,
                &[("content-type", JSON)],
                payload,
            )
            .await;
            assert_eq!(
                answered.status, 400,
                "case {case}: expected the validator's 400, got {} with {:?}",
                answered.status, answered.body
            );
            assert_eq!(
                answered.message(),
                INVALID_BODY,
                "case {case}: expected the BAD_REQUEST_BODY sentence, got {:?}",
                answered.message()
            );
            assert_eq!(
                answered.error_type.as_deref(),
                Some("BadRequestException"),
                "case {case}: expected the refusal to name its error shape, got {:?}",
                answered.error_type
            );
            // The integration was never performed for this request: neither the answer it
            // gives this very route for an admitted payload, nor anything else of its own,
            // is in what the caller holds.
            assert_ne!(
                answered.message(),
                backend.admitted_body,
                "case {case}: expected nothing of the integration's answer, got {:?}",
                answered.body
            );
            assert!(
                !answered.body.contains("integration"),
                "case {case}: expected nothing of the integration's answer, got {:?}",
                answered.body
            );
        }
    }
}

#[tokio::test]
async fn a_lambda_proxy_method_with_no_validator_reaches_its_integration_for_any_payload() {
    // The counterpart that keeps the test above honest: without a request validator the
    // same refused payloads are NOT screened, so each one reaches the integration and earns
    // the integration's own answer. The 400s above are therefore the validator's decision
    // and not something the backend type produces by itself.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "no-validator-before-integration").await;
    create_model(&gateway, &api, MODEL, order_schema()).await;
    let backend = Backend {
        case: "a Lambda proxy integration with no request validator",
        route: "unvalidated",
        integration: json!({
            "type": "AWS_PROXY",
            "httpMethod": "POST",
            "uri": lambda_uri(),
        }),
        admitted_status: 500,
        admitted_body: CONFIGURATION_ERROR,
    };
    let resource = resource_under(&gateway, &api, &root, backend.route).await;
    put_method(
        &gateway,
        &api,
        &resource,
        "POST",
        json!({
            "authorizationType": "NONE",
            // The model is declared, and NO validator is: a `requestModels` entry on its
            // own declares what a payload is, not that it is checked.
            "requestModels": { JSON: MODEL },
        }),
    )
    .await;
    put_integration(&gateway, &api, &resource, "POST", backend.integration.clone()).await;
    deploy(&gateway, &api).await;

    let mut payloads: Vec<(&str, &str)> = vec![("a payload the model admits", GOOD_BODY)];
    payloads.extend_from_slice(REFUSED_PAYLOADS);
    for (payload_case, payload) in payloads {
        let case = format!("{} and {payload_case}", backend.case);
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/{}", backend.route),
            &[("content-type", JSON)],
            payload,
        )
        .await;
        assert_eq!(
            answered.status, backend.admitted_status,
            "case {case}: expected the integration's own answer {}, got {} with {:?}",
            backend.admitted_status, answered.status, answered.body
        );
        assert_eq!(
            answered.message(),
            backend.admitted_body,
            "case {case}: expected the integration's own answer {:?}, got {:?}",
            backend.admitted_body,
            answered.message()
        );
    }
}
