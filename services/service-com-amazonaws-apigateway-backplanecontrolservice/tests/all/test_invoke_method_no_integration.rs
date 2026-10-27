// SPDX-License-Identifier: CC0-1.0

//! `TestInvokeMethod` against a method that carries NO integration.
//!
//! The RULE these cases assert, over a set of inputs rather than one recorded call:
//! simulating an invocation invokes the addressed method's integration, so a method that
//! carries none leaves the operation nothing to invoke and the CALL is refused — HTTP 400
//! `BadRequestException` with the fixed sentence the service words that condition as —
//! rather than answered with a simulated response. An integration that EXISTS and cannot
//! be performed is a different condition, and stays a reported gateway response (the
//! sibling `test_invoke_method` suite asserts that half).
//!
//! The rows differ along what the rule generalises over: the verb the method was put
//! under (the catch-all among them), what the simulated request carries (nothing, a
//! header in either member it may travel in, a body, a path with a query string, stage
//! variables, a client certificate id), and what else the method itself carries (a
//! declared method response, or nothing but its authorization type). None of those is
//! part of the condition, so every row earns the same refusal.
//!
//! One case walks the whole lifecycle instead: an integration put, deleted and put again,
//! so the refusal is shown to track the records as they stand rather than a method's
//! history — and the served legs on either side prove the refusal is not a method this
//! operation simply cannot invoke.
//!
//! Every case drives the real wire path — a `restJson1` request through the multi-service
//! router into the mounted service — through the crate's one wire-test harness
//! ([`Gateway`]), so what is asserted is what a client sees.

use serde_json::{json, Value};

use crate::integration_response::put_integration_response;
use crate::method_integration::{delete_integration, get_integration, put_integration, put_method};
use crate::method_response::put_method_response;
use crate::rest_api_lifecycle::{assert_failure, create_api, resource_under, Answer, Gateway};

/// The modelled failure a refused `TestInvokeMethod` carries: `BadRequestException` at
/// its modelled HTTP status
/// (`smithy_query(query="operation",operation="TestInvokeMethod")` -> errors
/// `BadRequestException` httpError 400).
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");

/// The sentence API Gateway refuses a test invocation of an integration-less method with.
/// Fixed text: it names no api, resource or verb, so nothing in it comes from the request
/// or from state.
const NO_INTEGRATION_DEFINED: &str = "No integration defined for method";

/// The sentence a read of an absent integration answers with, which is what proves the
/// integration is structurally gone rather than merely unperformable.
const NO_SUCH_INTEGRATION: &str = "Invalid Integration identifier specified";

/// One method with no integration, and the simulated request sent against it.
struct Bare {
    /// What makes this row its own case.
    case: &'static str,
    /// The path part of the resource this row stands its method up on.
    path_part: &'static str,
    /// The verb the method is put under, and addressed by.
    verb: &'static str,
    /// Whether the method also declares a `200` method response.
    method_response: bool,
    /// The `TestInvokeMethod` request body.
    request: Value,
}

/// `TestInvokeMethod` for `verb` on `resource`, with `request` as the simulated request.
async fn test_invoke(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    request: Value,
) -> Answer {
    gateway
        .call(
            "POST",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            Some(request),
        )
        .await
}

/// `DeleteIntegration` for `verb` on `resource` through the crate's shared helper,
/// asserted at the modelled 204.
async fn remove_integration(gateway: &Gateway, api: &str, resource: &str, verb: &str) {
    let answer = delete_integration(gateway, api, resource, verb).await;
    assert_eq!(
        answer.status, 204,
        "case DeleteIntegration({verb}): expected 204, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The `200` method response of a method, declared through the crate's shared helper and
/// asserted at the modelled 201.
async fn declare_method_response(gateway: &Gateway, api: &str, resource: &str, verb: &str) {
    let answer = put_method_response(gateway, api, resource, verb, "200", json!({})).await;
    assert_eq!(
        answer.status, 201,
        "case PutMethodResponse({verb}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The `200` integration response of a method's integration, rendering `body` for
/// `application/json`, asserted at the modelled 201.
async fn declare_integration_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    body: &str,
) {
    let answer = put_integration_response(
        gateway,
        api,
        resource,
        verb,
        "200",
        json!({ "responseTemplates": { "application/json": body } }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegrationResponse({verb}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Assert `answer` is the refusal of a method with no integration: the modelled status and
/// code, the fixed sentence, and NO simulated response beside it.
fn assert_refused(answer: &Answer, case: &str) {
    assert_failure(answer, BAD_REQUEST.0, BAD_REQUEST.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(NO_INTEGRATION_DEFINED),
        "case {case:?}: expected the message {NO_INTEGRATION_DEFINED:?}, got {:?}",
        answer.body["message"]
    );
    for member in ["status", "body", "log", "latency"] {
        assert!(
            answer.body.get(member).is_none(),
            "case {case:?}: expected a refusal carrying no {member:?}, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_method_with_no_integration_refuses_the_simulated_invocation() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-no-integration").await;

    // Rows differ along the verb, what the simulated request carries and what else the
    // method carries — none of which is part of the condition. The verbs are from the
    // catalog `crate::ops::put_method::HTTP_METHODS` lists, the `ANY` catch-all among
    // them.
    let cases: Vec<Bare> = vec![
        Bare {
            case: "a GET method and an empty simulated request",
            path_part: "bare",
            verb: "GET",
            method_response: false,
            request: json!({}),
        },
        Bare {
            case: "a GET method whose 200 response is declared",
            path_part: "declared",
            verb: "GET",
            method_response: true,
            request: json!({}),
        },
        Bare {
            case: "a POST method handed a body and a content type",
            path_part: "posted",
            verb: "POST",
            method_response: true,
            request: json!({
                "body": "{\"pet\":\"dog\"}",
                "headers": { "Content-Type": "application/json" }
            }),
        },
        Bare {
            case: "a PUT method whose header travels in multiValueHeaders",
            path_part: "multi",
            verb: "PUT",
            method_response: false,
            request: json!({ "multiValueHeaders": { "Accept": ["application/json"] } }),
        },
        Bare {
            case: "a DELETE method handed a path with a query string",
            path_part: "queried",
            verb: "DELETE",
            method_response: false,
            request: json!({ "pathWithQueryString": "/queried?limit=1&cursor=abc" }),
        },
        Bare {
            case: "a PATCH method handed stage variables",
            path_part: "staged",
            verb: "PATCH",
            method_response: false,
            request: json!({ "stageVariables": { "table": "pets" } }),
        },
        Bare {
            case: "a HEAD method handed a client certificate id",
            path_part: "certified",
            verb: "HEAD",
            method_response: false,
            request: json!({ "clientCertificateId": "abc123" }),
        },
        Bare {
            case: "the ANY catch-all method",
            path_part: "any",
            verb: "ANY",
            method_response: true,
            request: json!({ "headers": { "Accept": "text/plain" } }),
        },
        Bare {
            case: "an OPTIONS method handed every member at once",
            path_part: "everything",
            verb: "OPTIONS",
            method_response: true,
            request: json!({
                "body": "{}",
                "headers": { "Content-Type": "application/json", "Accept": "application/json" },
                "multiValueHeaders": { "X-Trace": ["one", "two"] },
                "pathWithQueryString": "/everything?all=1",
                "stageVariables": { "stage": "prod" },
                "clientCertificateId": "cert-1"
            }),
        },
    ];

    for row in &cases {
        let case = row.case;
        let resource = resource_under(&gateway, &api, &root, row.path_part).await;
        put_method(&gateway, &api, &resource, row.verb).await;
        if row.method_response {
            declare_method_response(&gateway, &api, &resource, row.verb).await;
        }
        let answer = test_invoke(&gateway, &api, &resource, row.verb, row.request.clone()).await;
        assert_refused(&answer, case);
    }
}

#[tokio::test]
async fn the_refusal_tracks_the_integration_as_it_stands_now() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-integration-lifecycle").await;
    let resource = resource_under(&gateway, &api, &root, "pets").await;
    put_method(&gateway, &api, &resource, "GET").await;
    declare_method_response(&gateway, &api, &resource, "GET").await;

    // Put: the same call the refusal legs make is SERVED, which is what makes the refusal
    // a statement about the configuration rather than about the operation.
    let created = put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({
            "type": "MOCK",
            "requestTemplates": { "application/json": "{\"statusCode\": 200}" }
        }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case PutIntegration: expected 201, got {} with {:?}",
        created.status, created.body
    );
    declare_integration_response(&gateway, &api, &resource, "GET", "{\"message\": \"ok\"}").await;

    let served = test_invoke(&gateway, &api, &resource, "GET", json!({})).await;
    assert_eq!(
        served.status, 200,
        "case the served leg: expected the operation to answer 200, got {} with {:?}",
        served.status, served.body
    );
    assert_eq!(
        served.body["status"].as_i64(),
        Some(200),
        "case the served leg: expected the simulated status 200, got {:?}",
        served.body
    );
    assert_eq!(
        served.body["body"].as_str(),
        Some("{\"message\": \"ok\"}"),
        "case the served leg: expected the integration response's body, got {:?}",
        served.body
    );

    // Deleted: the integration is structurally gone — a read of it says so — and the same
    // simulated call is refused.
    remove_integration(&gateway, &api, &resource, "GET").await;
    let read = get_integration(&gateway, &api, &resource, "GET").await;
    assert_failure(&read, 404, "NotFoundException", "GetIntegration after the delete");
    assert_eq!(
        read.body["message"].as_str(),
        Some(NO_SUCH_INTEGRATION),
        "case GetIntegration after the delete: expected {NO_SUCH_INTEGRATION:?}, got {:?}",
        read.body["message"]
    );
    let refused = test_invoke(&gateway, &api, &resource, "GET", json!({})).await;
    assert_refused(&refused, "the simulated call after the integration is deleted");

    // A second refusal of the same call: the refusal is a read of the records, so it does
    // not consume anything and does not change on repetition.
    let again = test_invoke(&gateway, &api, &resource, "GET", json!({})).await;
    assert_refused(&again, "the simulated call repeated");

    // Restored: the integration is put again and the call is served once more, with the
    // body THIS integration's own records render.
    let restored = put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({
            "type": "MOCK",
            "requestTemplates": { "application/json": "{\"statusCode\": 200}" }
        }),
    )
    .await;
    assert_eq!(
        restored.status, 201,
        "case PutIntegration (restored): expected 201, got {} with {:?}",
        restored.status, restored.body
    );
    declare_integration_response(&gateway, &api, &resource, "GET", "{\"message\": \"restored\"}")
        .await;
    let served_again = test_invoke(&gateway, &api, &resource, "GET", json!({})).await;
    assert_eq!(
        served_again.status, 200,
        "case the restored leg: expected the operation to answer 200, got {} with {:?}",
        served_again.status, served_again.body
    );
    assert_eq!(
        served_again.body["body"].as_str(),
        Some("{\"message\": \"restored\"}"),
        "case the restored leg: expected the restored integration's body, got {:?}",
        served_again.body
    );
}

#[tokio::test]
async fn an_unresolvable_identifier_is_still_a_not_found_before_the_integration_is_read() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-no-integration-not-found").await;
    let resource = resource_under(&gateway, &api, &root, "pets").await;
    put_method(&gateway, &api, &resource, "GET").await;

    // The method carries no integration in every row, so what each asserts is that the
    // identifier that resolves NOTHING is answered first: a 404 about the record the
    // request names, not the 400 about the integration behind it.
    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "an api id no record carries",
            "nosuchapi0".to_string(),
            resource.clone(),
            "GET",
        ),
        (
            "a resource id no record carries",
            api.clone(),
            "nosuchres0".to_string(),
            "GET",
        ),
        (
            "a verb the resource has no method for",
            api.clone(),
            resource.clone(),
            "POST",
        ),
    ];

    for (case, rest_api, resource_id, verb) in &cases {
        let answer = test_invoke(&gateway, rest_api, resource_id, verb, json!({})).await;
        assert_failure(&answer, 404, "NotFoundException", case);
        assert_ne!(
            answer.body["message"].as_str(),
            Some(NO_INTEGRATION_DEFINED),
            "case {case:?}: expected the refusal to name the unresolvable record, got {:?}",
            answer.body["message"]
        );
    }
}
