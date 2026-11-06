// SPDX-License-Identifier: CC0-1.0

//! How an integration whose own HTTP method carries no value is refused, asserted on the
//! wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`PutMethod` helpers are this crate's existing harness,
//! reused from `rest_api_lifecycle` and `method_integration`.
//!
//! The RULE under test: API Gateway reads an integration's own HTTP method as an
//! ENUMERATION of HTTP verbs, so a record that would be stored with NO value for it —
//! unset where the integration type needs one, or carried as a blank string — is refused
//! as a `BadRequestException` whose message is the fixed sentence naming that member in the
//! integration record's spelling, `HttpMethod`, with nothing of the request interpolated:
//! not the integration type, not the submitted value, not the wire member name.
//!
//! The rows span what the rule generalises over rather than repeating one recorded call:
//! every `IntegrationType` the model declares that needs a method
//! (`smithy_query(query="shape", shape="IntegrationType")`: HTTP, AWS, HTTP_PROXY,
//! AWS_PROXY — `MOCK` is the one type whose method is optional, which is a row of its own),
//! both ways the value can be empty (absent, and blank strings of two shapes), and both
//! calls that store an integration record — the create and the patch that reaches the same
//! record check.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_integration::MOCK_INTEGRATION_TYPE;

use crate::method_integration::{
    aws_integration_uri, get_integration, put_integration, resource_with_a_method,
};
use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};

/// The sentence an integration with no value for its own HTTP method is refused with:
/// entirely fixed text, spelled out here rather than imported, so the assertion fails if
/// the implementation's wording ever moves off it.
const EMPTY_INTEGRATION_HTTP_METHOD: &str = "Enumeration value for HttpMethod must be non-empty";

/// The verb the method under test is put on; the integration's own method is a member of
/// the request body, so the two are independent.
const VERB: &str = "GET";

/// An endpoint for the two integration types whose `uri` is an HTTP(S) endpoint.
const HTTP_ENDPOINT: &str = "https://example.com/pets";

/// Assert `answer` is the modelled 400 `BadRequestException` carrying exactly the
/// enumeration sentence.
fn assert_refused(answer: &Answer, case: &str) {
    assert_failure(answer, 400, "BadRequestException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(EMPTY_INTEGRATION_HTTP_METHOD),
        "case {case}: expected the message {EMPTY_INTEGRATION_HTTP_METHOD:?}, got {:?}",
        answer.body["message"]
    );
}

/// The `uri` each integration type takes, so every row below is wrong in its HTTP method
/// alone.
fn uri_for(integration_type: &str) -> Option<String> {
    match integration_type {
        "HTTP" | "HTTP_PROXY" => Some(HTTP_ENDPOINT.to_string()),
        "AWS" | "AWS_PROXY" => Some(aws_integration_uri(
            "lambda:path/2015-03-31/functions/pets/invocations",
        )),
        _ => None,
    }
}

/// A `PutIntegration` body of `integration_type`, with `http_method` set only when it is
/// `Some` — the integration's own method travels as `httpMethod`, the `jsonName` of the
/// input's `integrationHttpMethod` member.
fn integration_request(integration_type: &str, http_method: Option<&str>) -> Value {
    let mut request = json!({ "type": integration_type });
    let body = request
        .as_object_mut()
        .expect("the request body is a JSON object");
    if let Some(uri) = uri_for(integration_type) {
        body.insert("uri".to_string(), Value::from(uri));
    }
    if let Some(http_method) = http_method {
        body.insert("httpMethod".to_string(), Value::from(http_method));
    }
    request
}

#[tokio::test]
async fn an_integration_with_no_http_method_is_refused_by_the_enumeration_sentence() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-http-method").await;
    let resource = resource_with_a_method(&gateway, &api, &root, "pets", VERB, json!({})).await;

    // Every type that needs an integration method, with the member ABSENT.
    for integration_type in ["HTTP", "AWS", "HTTP_PROXY", "AWS_PROXY"] {
        let answer = put_integration(
            &gateway,
            &api,
            &resource,
            VERB,
            integration_request(integration_type, None),
        )
        .await;
        assert_refused(&answer, &format!("PutIntegration({integration_type}, no httpMethod)"));
    }

    // And with the member PRESENT but blank, which is no enumeration value either —
    // including on the one type whose method is optional when it is absent.
    for integration_type in ["HTTP", "AWS", "HTTP_PROXY", "AWS_PROXY", MOCK_INTEGRATION_TYPE] {
        for blank in ["", " ", "\t"] {
            let answer = put_integration(
                &gateway,
                &api,
                &resource,
                VERB,
                integration_request(integration_type, Some(blank)),
            )
            .await;
            assert_refused(
                &answer,
                &format!("PutIntegration({integration_type}, httpMethod {blank:?})"),
            );
        }
    }

    // Nothing was stored by any refused row: the method still carries no integration.
    let read = get_integration(&gateway, &api, &resource, VERB).await;
    assert_eq!(
        read.status, 404,
        "case GetIntegration(after the refusals): expected 404, got {} with {:?}",
        read.status, read.body
    );

    // The controls: the one type whose method is optional is accepted with none, and a
    // type that needs one is accepted with a verb and reads it back.
    let mock = put_integration(
        &gateway,
        &api,
        &resource,
        VERB,
        integration_request(MOCK_INTEGRATION_TYPE, None),
    )
    .await;
    assert_eq!(
        mock.status, 201,
        "case PutIntegration({MOCK_INTEGRATION_TYPE}, no httpMethod): expected 201, got {} with {:?}",
        mock.status, mock.body
    );
    let http = put_integration(
        &gateway,
        &api,
        &resource,
        VERB,
        integration_request("HTTP", Some("POST")),
    )
    .await;
    assert_eq!(
        http.status, 201,
        "case PutIntegration(HTTP, httpMethod POST): expected 201, got {} with {:?}",
        http.status, http.body
    );
    let read = get_integration(&gateway, &api, &resource, VERB).await;
    assert_eq!(
        read.body["httpMethod"].as_str(),
        Some("POST"),
        "case GetIntegration(HTTP): expected the stored httpMethod POST, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn a_patch_that_empties_the_integration_http_method_is_refused_the_same_way() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-http-method-patch").await;
    let resource = resource_with_a_method(&gateway, &api, &root, "pets", VERB, json!({})).await;
    let created = put_integration(
        &gateway,
        &api,
        &resource,
        VERB,
        integration_request("HTTP", Some("GET")),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case PutIntegration(HTTP, httpMethod GET): expected 201, got {} with {:?}",
        created.status, created.body
    );

    // The patch reaches the SAME record check, so the same empty value earns the same
    // sentence: removing the member, and replacing it with a blank.
    let patches: Vec<(&str, Value)> = vec![
        (
            "remove /httpMethod",
            json!({ "patchOperations": [{ "op": "remove", "path": "/httpMethod" }] }),
        ),
        (
            "replace /httpMethod with an empty value",
            json!({ "patchOperations": [
                { "op": "replace", "path": "/httpMethod", "value": "" }
            ] }),
        ),
        (
            "replace /httpMethod with a blank value",
            json!({ "patchOperations": [
                { "op": "replace", "path": "/httpMethod", "value": "  " }
            ] }),
        ),
    ];
    for (case, body) in patches {
        let answer = gateway
            .call(
                "PATCH",
                &format!("/restapis/{api}/resources/{resource}/methods/{VERB}/integration"),
                Some(body),
            )
            .await;
        assert_refused(&answer, &format!("UpdateIntegration({case})"));
    }

    // And the stored integration still carries the method it was created with: a refused
    // patch changes nothing.
    let read = get_integration(&gateway, &api, &resource, VERB).await;
    assert_eq!(
        read.body["httpMethod"].as_str(),
        Some("GET"),
        "case GetIntegration(after the refused patches): expected httpMethod GET, got {:?}",
        read.body
    );
}
