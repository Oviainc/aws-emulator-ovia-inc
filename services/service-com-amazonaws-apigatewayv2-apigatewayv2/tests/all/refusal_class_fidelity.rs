// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for HOW this service CLASSIFIES and WORDS its 400-class
//! refusals.
//!
//! A client keys on all three parts of a refusal — the HTTP status, the error code it
//! carries (in `__type` and in the `x-amzn-errortype` header) and the message — so each
//! rule below fixes all three, over a SET of inputs rather than over one request:
//!
//! * **A write that would claim an identity the collection already holds is refused the
//!   way API Gateway classifies that KIND of resource**: a duplicate custom domain name,
//!   integration-response key and route-response key are `BadRequestException` 400, a
//!   duplicate model name is `ConflictException` 409 — and the sentence is a CONSTANT for
//!   the kinds whose refusal states the requirement (so no row carries the value it was
//!   sent) and a TEMPLATE over the request's own members for the kinds whose refusal
//!   names what collided.
//! * **A write addressed to a resource by path resolves that identifier BEFORE it reads
//!   the body**: `ReimportApi` against an `ApiId` the account holds no api under is
//!   `NotFoundException` 404 whatever the body then turns out to say, while the same
//!   bodies over an api that EXISTS are the body's own `BadRequestException` 400.
//! * **A `maxResults` a collection reader cannot serve as a page size is one sentence**,
//!   shared by every paginated reader and echoing none of the value the caller sent.
//! * **A `VPC_LINK` connection with no `ConnectionId` is refused with API Gateway's own
//!   sentence**, by every writer of an integration.
//!
//! The rows vary what each rule generalises over: the kind of resource, the FORM of the
//! identity that collided (a short and a deeply nested domain name, several model names,
//! the two forms an integration-response key takes, several route-response keys), which
//! writer makes the claim (a create and an update), the form of the identifier that
//! resolves to nothing, the body a refused reimport carries, the collection being paged
//! and the value `maxResults` carries. Each recorded request that filed the issue is ONE
//! row of its table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::invalid_api_identifier_message;

use crate::api_lifecycle::{as_str, call, call_reading_headers, emulator, error_code};
use crate::domain_name_lifecycle::certificate_arn;

/// AWS's sentence for a `CreateDomainName` naming a domain name the account already
/// holds. The whole sentence is fixed text: the domain name is NOT interpolated.
const DOMAIN_NAME_ALREADY_EXISTS: &str = "The domain name you provided already exists.";

/// AWS's sentence for a `CreateModel` naming a model name the api already holds. The
/// whole sentence is fixed text: the model name is NOT interpolated.
const MODEL_NAME_ALREADY_EXISTS: &str = "Model name already exists for this REST API";

/// AWS's sentence for a `maxResults` that is no page size. Fixed text: the offending
/// value is NOT echoed back.
const MAX_RESULTS_NOT_AN_INTEGER: &str = "maxResults must be an integer.";

/// AWS's sentence for a `VPC_LINK` connection that names no VPC link to connect through.
const VPC_LINK_NEEDS_CONNECTION_ID: &str =
    "ConnectionId must be set to vpcLinkId for ConnectionType VPC_LINK";

/// AWS's sentence for an integration-response key the integration already holds: fixed
/// text around the key the request's body carried and the integration its path named.
fn integration_response_key_taken(key: &str, integration_id: &str) -> String {
    format!(
        "IntegrationResponse with key {key} is already present in Integration with ID \
         {integration_id}"
    )
}

/// AWS's sentence for a route-response key the route already holds: fixed text around
/// the key the request's body carried, then the requirement it breaks.
fn route_response_key_taken(key: &str) -> String {
    format!(
        "RouteResponse with key {key} already exists on this Route. RouteResponse keys \
         must be unique within a given Route."
    )
}

/// Send one request and assert it was ACCEPTED with `expected_status`, answering the
/// parsed body so the caller can read the identifiers the write minted.
async fn accepted(
    emulator: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    expected_status: u16,
) -> Value {
    let (status, answer) = call(emulator, method, uri, body).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected {expected_status} from {method} {uri}, got {status} \
         {answer}"
    );
    answer
}

/// The refusal a row expects: the HTTP status, the error code and the message.
///
/// This suite sends the request and reads the response HEADERS itself, because the
/// `x-amzn-errortype` a client branches on is part of every rule below;
/// `refusal_message_text`'s helper judges an answer another call already made, and
/// reads the body alone, so the two cannot be one.
struct Refusal<'a> {
    status: u16,
    code: &'a str,
    message: &'a str,
}

/// The modelled `BadRequestException` (400) carrying `message`.
fn bad_request(message: &str) -> Refusal<'_> {
    Refusal {
        status: 400,
        code: "BadRequestException",
        message,
    }
}

/// The modelled `ConflictException` (409) carrying `message`.
fn conflict(message: &str) -> Refusal<'_> {
    Refusal {
        status: 409,
        code: "ConflictException",
        message,
    }
}

/// The modelled `NotFoundException` (404) carrying `message`.
fn not_found(message: &str) -> Refusal<'_> {
    Refusal {
        status: 404,
        code: "NotFoundException",
        message,
    }
}

/// Send one request and assert the refusal it answers END TO END: the status, the error
/// code in the body AND on the response headers, and the message.
async fn assert_refused(
    emulator: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    expected: Refusal<'_>,
) {
    let Refusal {
        status: expected_status,
        code: expected_code,
        message: expected_message,
    } = expected;
    let (status, headers, answer) = call_reading_headers(emulator, method, uri, body).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected {expected_status} from {method} {uri}, got {status} \
         {answer}"
    );
    assert_eq!(
        error_code(&answer),
        expected_code,
        "case {case:?}: expected {expected_code}, got {answer}"
    );
    assert_eq!(
        headers
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok()),
        Some(expected_code),
        "case {case:?}: expected {expected_code} on the response headers, got {headers:?}"
    );
    assert_eq!(
        as_str(&answer, "message"),
        expected_message,
        "case {case:?}: expected the refusal {expected_message:?}, got {answer}"
    );
}

/// One WEBSOCKET api — the protocol that carries models and route responses — answering
/// its `ApiId`.
async fn websocket_api(emulator: &MultiServiceRouter, name: &str) -> String {
    let created = accepted(
        emulator,
        name,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": name,
            "protocolType": "WEBSOCKET",
            // "This property is required for WebSocket APIs"
            // (`CreateApi$RouteSelectionExpression`).
            "routeSelectionExpression": "$request.body.action"
        })),
        201,
    )
    .await;
    as_str(&created, "apiId")
}

/// One HTTP api, answering its `ApiId`.
async fn http_api(emulator: &MultiServiceRouter, name: &str) -> String {
    let created = accepted(
        emulator,
        name,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
        201,
    )
    .await;
    as_str(&created, "apiId")
}

/// RULE: a second `CreateDomainName` under a name the account already holds is
/// `BadRequestException` 400 with ONE sentence, whatever the name looks like.
#[tokio::test]
async fn a_domain_name_the_account_already_holds_is_refused_as_a_bad_request() {
    let emulator = emulator();
    // The rows span the FORM of the name that collides: a plain host, a deeply nested
    // one, a very long one, and one carrying a hyphen and digits.
    let names = [
        "taken.example.com".to_string(),
        "api.eu.internal.taken.example.com".to_string(),
        format!("{}.example.com", "d".repeat(60)),
        "api-2.example.com".to_string(),
    ];

    for name in names {
        let request = json!({
            "domainName": name,
            "domainNameConfigurations": [{ "certificateArn": certificate_arn("first") }]
        });
        accepted(
            &emulator,
            &name,
            "POST",
            "/v2/domainnames",
            Some(request.clone()),
            201,
        )
        .await;

        // The second create asks for a DIFFERENT configuration, so what collides is the
        // name alone.
        let second = json!({
            "domainName": name,
            "domainNameConfigurations": [{ "certificateArn": certificate_arn("second") }],
            "tags": { "owner": "second" }
        });
        assert_refused(
            &emulator,
            &name,
            "POST",
            "/v2/domainnames",
            Some(second),
            bad_request(DOMAIN_NAME_ALREADY_EXISTS),
        )
        .await;

        // The refusal states the requirement rather than quoting the request, so no row
        // carries the name it was sent.
        assert!(
            !DOMAIN_NAME_ALREADY_EXISTS.contains(&name),
            "case {name:?}: the sentence must not interpolate the domain name"
        );

        // And the stored record survived: the name is still the FIRST create's resource.
        let read = accepted(
            &emulator,
            &name,
            "GET",
            &format!("/v2/domainnames/{name}"),
            None,
            200,
        )
        .await;
        assert_eq!(
            read["domainNameConfigurations"][0]["certificateArn"]
                .as_str()
                .unwrap_or_default(),
            certificate_arn("first"),
            "case {name:?}: expected the refused create to leave the stored record, got \
             {read}"
        );
    }
}

/// RULE: a second `CreateModel` under a name the api already holds is `ConflictException`
/// 409 with ONE sentence, whatever the name is.
#[tokio::test]
async fn a_model_name_the_api_already_holds_is_refused_with_one_sentence() {
    let emulator = emulator();
    let api_id = websocket_api(&emulator, "model-identity-api").await;
    // "The name of the model. Must be alphanumeric" (`CreateModel$Name`), so the rows
    // span alphanumeric names: one word, mixed case, digits, and a single character.
    let names = ["Order", "orderReply2", "M1", "Model123"];

    for name in names {
        let uri = format!("/v2/apis/{api_id}/models");
        accepted(
            &emulator,
            name,
            "POST",
            &uri,
            Some(json!({
                "name": name,
                "contentType": "application/json",
                "schema": "{}"
            })),
            201,
        )
        .await;

        // A different schema and content type under the same name: the NAME is what
        // collides.
        assert_refused(
            &emulator,
            name,
            "POST",
            &uri,
            Some(json!({
                "name": name,
                "contentType": "application/vnd.example+json",
                "schema": "{\"type\":\"object\"}"
            })),
            conflict(MODEL_NAME_ALREADY_EXISTS),
        )
        .await;
        assert!(
            !MODEL_NAME_ALREADY_EXISTS.contains(name),
            "case {name:?}: the sentence must not interpolate the model name"
        );
    }
}

/// RULE: a duplicate integration-response key is `BadRequestException` 400 naming the key
/// and the integration it collided in — and the same key is free on another integration.
#[tokio::test]
async fn a_duplicate_integration_response_key_names_the_key_and_the_integration() {
    let emulator = emulator();
    let api_id = websocket_api(&emulator, "integration-response-identity").await;
    // Two integrations of one api, so the scope of the key is visible: both are MOCK
    // integrations, whose responses are selected on the status of the call.
    let first = as_str(
        &accepted(
            &emulator,
            "the first integration",
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(json!({ "integrationType": "MOCK" })),
            201,
        )
        .await,
        "integrationId",
    );
    let second = as_str(
        &accepted(
            &emulator,
            "the second integration",
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(json!({ "integrationType": "MOCK" })),
            201,
        )
        .await,
        "integrationId",
    );

    // The rows span the two forms a `SelectionKey` takes: the catch-all and a pattern
    // that matches a status class.
    for key in ["$default", r"/2\d\d/", "/404/"] {
        let uri = format!("/v2/apis/{api_id}/integrations/{first}/integrationresponses");
        accepted(
            &emulator,
            key,
            "POST",
            &uri,
            Some(json!({ "integrationResponseKey": key })),
            201,
        )
        .await;
        assert_refused(
            &emulator,
            key,
            "POST",
            &uri,
            Some(json!({ "integrationResponseKey": key })),
            bad_request(&integration_response_key_taken(key, &first)),
        )
        .await;

        // The key is scoped to its integration: the same key on the sibling integration
        // is a fresh integration response, and the refusal above named the integration
        // the request addressed rather than the key alone.
        accepted(
            &emulator,
            key,
            "POST",
            &format!("/v2/apis/{api_id}/integrations/{second}/integrationresponses"),
            Some(json!({ "integrationResponseKey": key })),
            201,
        )
        .await;
    }
}

/// RULE: a duplicate route-response key is `BadRequestException` 400 naming the key and
/// the requirement it breaks — from the create and from the update alike.
#[tokio::test]
async fn a_duplicate_route_response_key_names_the_key_and_the_requirement() {
    let emulator = emulator();
    let api_id = websocket_api(&emulator, "route-response-identity").await;
    let route_id = as_str(
        &accepted(
            &emulator,
            "the route",
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": "$default" })),
            201,
        )
        .await,
        "routeId",
    );
    let sibling_route = as_str(
        &accepted(
            &emulator,
            "the sibling route",
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": "sendMessage" })),
            201,
        )
        .await,
        "routeId",
    );

    // The rows span the forms a route-response key takes: the catch-all and route keys
    // of the api's own protocol.
    for key in ["$default", "echo", "sendMessage"] {
        let uri = format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses");
        let held = as_str(
            &accepted(
                &emulator,
                key,
                "POST",
                &uri,
                Some(json!({ "routeResponseKey": key })),
                201,
            )
            .await,
            "routeResponseId",
        );
        assert_refused(
            &emulator,
            key,
            "POST",
            &uri,
            Some(json!({ "routeResponseKey": key })),
            bad_request(&route_response_key_taken(key)),
        )
        .await;

        // The UPDATE claims the key the same way: a response moved onto a key a sibling
        // of its own route holds takes the same refusal.
        let moving = as_str(
            &accepted(
                &emulator,
                key,
                "POST",
                &uri,
                Some(json!({ "routeResponseKey": format!("{key}-elsewhere") })),
                201,
            )
            .await,
            "routeResponseId",
        );
        assert_refused(
            &emulator,
            key,
            "PATCH",
            &format!("{uri}/{moving}"),
            Some(json!({ "routeResponseKey": key })),
            bad_request(&route_response_key_taken(key)),
        )
        .await;

        // The key is scoped to its route: the same key on the sibling route is accepted,
        // and the response that holds it is still readable.
        accepted(
            &emulator,
            key,
            "POST",
            &format!("/v2/apis/{api_id}/routes/{sibling_route}/routeresponses"),
            Some(json!({ "routeResponseKey": key })),
            201,
        )
        .await;
        accepted(
            &emulator,
            key,
            "GET",
            &format!("{uri}/{held}"),
            None,
            200,
        )
        .await;
    }
}

/// RULE: `ReimportApi` resolves the `ApiId` its path names BEFORE it reads the body, so
/// an api the account does not hold is a 404 whatever the body says — while the same
/// bodies over an api that exists are the body's own 400.
#[tokio::test]
async fn a_reimport_of_an_unknown_api_is_a_not_found_whatever_the_body_says() {
    let emulator = emulator();

    // A definition this service reads, for the row whose body is fine.
    let readable = json!({
        "openapi": "3.0.1",
        "info": { "title": "readable", "version": "1.0" },
        "paths": { "/items": { "get": {
            "x-amazon-apigateway-integration": {
                "type": "http_proxy",
                "httpMethod": "GET",
                "uri": "https://items.example.com/items",
                "payloadFormatVersion": "1.0"
            }
        } } }
    })
    .to_string();

    // The bodies span what a reimport can carry: a definition this service reads, prose
    // that is no document, an empty body, the YAML spelling this service does not read,
    // and a definition whose integration names no endpoint.
    let bodies: Vec<(&str, String)> = vec![
        ("a definition this service reads", readable.clone()),
        ("prose that is no definition", "not a definition".to_string()),
        ("an empty body", String::new()),
        (
            "a definition in the YAML spelling",
            "openapi: 3.0.1\ninfo:\n  title: yaml\n".to_string(),
        ),
        (
            "an object declaring no OpenAPI version",
            json!({ "info": { "title": "versionless", "version": "1.0" }, "paths": {} })
                .to_string(),
        ),
    ];

    // The identifiers span the forms a client can send one in: an eleven-character
    // lowercase id, a single character, the longest this model admits, and a mixed-case
    // one. None of them is an api of this store.
    let unknown = [
        "nosuchid000".to_string(),
        "z".to_string(),
        "9".repeat(30),
        "MixedCase1".to_string(),
    ];

    for (case, body) in &bodies {
        for api_id in &unknown {
            assert_refused(
                &emulator,
                &format!("{case} over the unknown api {api_id:?}"),
                "PUT",
                &format!("/v2/apis/{api_id}"),
                Some(json!({ "body": body })),
                not_found(&invalid_api_identifier_message(api_id)),
            )
            .await;
        }
    }

    // Over an api that DOES exist, the same bodies are judged on their own merits: the
    // readable one goes through, and every other is the body's own BadRequestException.
    // So the 404 above is the unresolved identifier and not a blanket answer.
    let imported = accepted(
        &emulator,
        "the api to reimport over",
        "PUT",
        "/v2/apis",
        Some(json!({ "body": readable })),
        200,
    )
    .await;
    let api_id = as_str(&imported, "apiId");
    for (case, body) in &bodies {
        let (status, answer) = call(
            &emulator,
            "PUT",
            &format!("/v2/apis/{api_id}"),
            Some(json!({ "body": body })),
        )
        .await;
        if body == &readable {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the readable definition to go through, got \
                 {status} {answer}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the body's own 400 over an api that exists, got \
             {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {answer}"
        );
    }
}

/// RULE: a `maxResults` no collection reader can serve as a page size takes ONE sentence,
/// which echoes none of the value the request sent.
#[tokio::test]
async fn a_max_results_that_is_no_page_size_is_refused_with_one_sentence() {
    let emulator = emulator();
    let api_id = http_api(&emulator, "max-results-api").await;
    accepted(
        &emulator,
        "the domain name the account-level reader reports",
        "POST",
        "/v2/domainnames",
        Some(json!({
            "domainName": "paged.example.com",
            "domainNameConfigurations": [{ "certificateArn": certificate_arn("paged") }]
        })),
        201,
    )
    .await;

    // The collections span the readers that resolve this page size: account-level and
    // api-level ones alike.
    let collections = [
        ("GetApis", "/v2/apis".to_string()),
        ("GetRoutes", format!("/v2/apis/{api_id}/routes")),
        ("GetStages", format!("/v2/apis/{api_id}/stages")),
        ("GetIntegrations", format!("/v2/apis/{api_id}/integrations")),
        ("GetDomainNames", "/v2/domainnames".to_string()),
        ("GetVpcLinks", "/v2/vpclinks".to_string()),
    ];
    // The values span what a caller can send in place of a page size: zero, a negative,
    // other notations, text, emptiness, padding and a number past any integer.
    let values = [
        "0",
        "-1",
        "1.5",
        "1e3",
        "abc",
        "",
        "%205",
        &"9".repeat(40),
    ];

    for (reader, collection) in &collections {
        for value in values {
            assert_refused(
                &emulator,
                &format!("{reader} with maxResults={value:?}"),
                "GET",
                &format!("{collection}?maxResults={value}"),
                None,
                bad_request(MAX_RESULTS_NOT_AN_INTEGER),
            )
            .await;
        }
        // A page size the reader CAN serve is served, so the rows above are about the
        // value and not about the parameter.
        for value in ["1", "100", "101"] {
            accepted(
                &emulator,
                &format!("{reader} with maxResults={value:?}"),
                "GET",
                &format!("{collection}?maxResults={value}"),
                None,
                200,
            )
            .await;
        }
    }

    // The sentence carries nothing from the request, so no value a caller sends can turn
    // up in it.
    for value in values {
        assert!(
            value.is_empty() || !MAX_RESULTS_NOT_AN_INTEGER.contains(value),
            "case {value:?}: the sentence must echo no value the request sent"
        );
    }
}

/// RULE: a `VPC_LINK` connection that names no `ConnectionId` is refused with API
/// Gateway's own sentence, by every writer of an integration.
#[tokio::test]
async fn a_vpc_link_connection_with_no_link_is_refused_with_one_sentence() {
    let emulator = emulator();
    let http = http_api(&emulator, "vpc-link-http-api").await;
    let socket = websocket_api(&emulator, "vpc-link-socket-api").await;

    // The rows span the integration types a private integration takes, on an api of
    // either protocol.
    let creates: Vec<(&str, &String, Value)> = vec![
        (
            "an HTTP_PROXY integration of an HTTP api",
            &http,
            json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://private.example.com/upstream",
                "payloadFormatVersion": "1.0",
                "connectionType": "VPC_LINK"
            }),
        ),
        (
            "an HTTP integration of a WebSocket api",
            &socket,
            json!({
                "integrationType": "HTTP",
                "integrationUri": "https://private.example.com/upstream",
                "integrationMethod": "POST",
                "connectionType": "VPC_LINK"
            }),
        ),
        (
            "a VPC_LINK connection naming an empty link",
            &http,
            json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://private.example.com/upstream",
                "payloadFormatVersion": "1.0",
                "connectionType": "VPC_LINK",
                "connectionId": ""
            }),
        ),
    ];

    for (case, api_id, request) in creates {
        assert_refused(
            &emulator,
            case,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(request),
            bad_request(VPC_LINK_NEEDS_CONNECTION_ID),
        )
        .await;
    }

    // The UPDATE goes through the same requirement: an integration patched onto a
    // `VPC_LINK` connection with no link takes the same refusal.
    let integration_id = as_str(
        &accepted(
            &emulator,
            "the integration to patch",
            "POST",
            &format!("/v2/apis/{http}/integrations"),
            Some(json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://public.example.com/upstream",
                "payloadFormatVersion": "1.0"
            })),
            201,
        )
        .await,
        "integrationId",
    );
    assert_refused(
        &emulator,
        "an integration patched onto a VPC_LINK connection with no link",
        "PATCH",
        &format!("/v2/apis/{http}/integrations/{integration_id}"),
        Some(json!({ "connectionType": "VPC_LINK" })),
        bad_request(VPC_LINK_NEEDS_CONNECTION_ID),
    )
    .await;
}
