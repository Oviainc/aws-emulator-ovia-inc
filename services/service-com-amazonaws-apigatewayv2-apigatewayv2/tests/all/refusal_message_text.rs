// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the TEXT of five ApiGatewayV2 refusals.
//!
//! An error's HTTP status, its code AND its message are part of the contract a client
//! keys on, so each rule below fixes all three. Every suite asserts its rule over a SET
//! of inputs, because what the rule fixes is the SENTENCE, not one request's answer:
//!
//! * a refusal whose sentence is a CONSTANT for its whole class carries nothing from the
//!   request, so every row of the class gets character-for-character the same message —
//!   no value, no identifier, and no reader's diagnostic (a parser position, the JSON
//!   type a document turned out to be) that would vary with the input or with the
//!   library version;
//! * a refusal whose sentence is BUILT from the request carries exactly the members the
//!   rule is about, in one template, so the rows span the values those members take.
//!
//! The recorded requests that filed the issue — one malformed model schema, one repeated
//! stage name, one `HTTP_PROXY` integration asking for payload format 2.0, one `MOCK`
//! integration on an HTTP api, one route patched to `JWT` with no authorizer — are ONE
//! row each.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    non_websocket_integration_types, UNRESOLVED_AUTHORIZER_MESSAGE,
    WEBSOCKET_ONLY_INTEGRATION_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// AWS's sentence for a `Schema` that is no schema document: the result of validating
/// the model, carrying its one error and no warnings.
const INVALID_MODEL_SCHEMA: &str = "Invalid model specified: Validation Result: \
     warnings : [], errors : [Invalid model schema specified]";

/// AWS's sentence for a `CreateStage` naming a stage its api already holds.
const STAGE_ALREADY_EXISTS: &str = "Stage already exists";

/// AWS's sentence for an integration type an api's protocol has no behaviour for: it
/// names the types that api MAY be associated with.
const PROXY_INTEGRATIONS_ONLY: &str = "Currently, an API with a protocol type of HTTP \
     may only be associated with proxy integrations (AWS_PROXY, HTTP_PROXY)";

/// AWS's sentence for a route whose `AuthorizationType` is `JWT` and whose authorizer
/// reference is not a valid one.
const JWT_AUTHORIZER_REQUIRED: &str = "Invalid authorizer ID specified. Setting the \
     authorization type to JWT requires a valid JWT authorizer.";

/// Create one api of `protocol_type` and answer its `ApiId`.
async fn create_api(
    emulator: &MultiServiceRouter,
    case: &str,
    name: &str,
    protocol_type: &str,
) -> String {
    let mut request = json!({ "name": name, "protocolType": protocol_type });
    if protocol_type == "WEBSOCKET" {
        // "This property is required for WebSocket APIs"
        // (`CreateApi$RouteSelectionExpression`).
        request["routeSelectionExpression"] = json!("$request.body.action");
    }
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi for a {protocol_type} api, \
         got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Assert one refusal end to end: the status, the error code, and the message.
fn assert_refusal(
    case: &str,
    expected_status: u16,
    code: &str,
    message: &str,
    answer: &(u16, Value),
) {
    let (status, body) = answer;
    assert_eq!(
        *status, expected_status,
        "case {case:?}: expected {expected_status}, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        code,
        "case {case:?}: expected {code}, got {body}"
    );
    assert_eq!(
        as_str(body, "message"),
        message,
        "case {case:?}: expected the refusal {message:?}, got {body}"
    );
}

/// Every malformed `Schema` is refused with ONE sentence, whatever was malformed about
/// it: the rows span how the document fails (unparseable, or parsed to a value that is
/// no JSON object), the JSON type it parsed as, and the content type that selects JSON
/// Schema as the schema language.
#[tokio::test]
async fn a_schema_that_is_no_schema_document_is_refused_with_one_sentence() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the models api", "schema-text", "WEBSOCKET").await;

    // (case, the CreateModel request)
    let rows: Vec<(&str, Value)> = vec![
        (
            "a truncated document",
            json!({ "name": "Truncated", "contentType": "application/json",
                    "schema": "{\"type\":" }),
        ),
        (
            "an unquoted key, which no JSON parser reads",
            json!({ "name": "Unquoted", "contentType": "application/json",
                    "schema": "{not json" }),
        ),
        (
            "a document with trailing garbage",
            json!({ "name": "Trailing", "contentType": "application/json",
                    "schema": "{} }" }),
        ),
        (
            "prose that is not JSON at all",
            json!({ "name": "Prose", "contentType": "application/json",
                    "schema": "this is not a schema" }),
        ),
        (
            "an empty schema",
            json!({ "name": "Blank", "contentType": "application/json", "schema": "" }),
        ),
        (
            "a JSON array, not an object",
            json!({ "name": "Array", "contentType": "application/json", "schema": "[]" }),
        ),
        (
            "a JSON number, not an object",
            json!({ "name": "Number", "contentType": "application/json", "schema": "42" }),
        ),
        (
            "a JSON string, not an object",
            json!({ "name": "Str", "contentType": "application/json",
                    "schema": "\"an object, please\"" }),
        ),
        (
            "a JSON boolean, not an object",
            json!({ "name": "Boolean", "contentType": "application/json", "schema": "true" }),
        ),
        (
            "a JSON null, not an object",
            json!({ "name": "Null", "contentType": "application/json", "schema": "null" }),
        ),
        (
            "a malformed document under a +json media type",
            json!({ "name": "Suffixed", "contentType": "application/vnd.example+json",
                    "schema": "{\"type\": }" }),
        ),
        (
            "a malformed document under a parameterised media type",
            json!({ "name": "Parameterised", "contentType": "application/json; charset=utf-8",
                    "schema": "{oops}" }),
        ),
        (
            "a malformed document under no content type at all",
            json!({ "name": "Defaulted", "schema": "{oops}" }),
        ),
    ];

    for (case, request) in rows {
        let answer = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/models"),
            Some(request),
        )
        .await;
        assert_refusal(
            case,
            400,
            "BadRequestException",
            INVALID_MODEL_SCHEMA,
            &answer,
        );
    }

    // The sentence is the refusal's, not CreateModel's: a schema that IS a JSON Schema
    // document, and one in a language this service does not read, are both stored.
    for (case, request) in [
        (
            "a draft-04 document",
            json!({ "name": "Order", "contentType": "application/json",
                    "schema": "{\"$schema\":\"http://json-schema.org/draft-04/schema#\",\
                                \"type\":\"object\"}" }),
        ),
        (
            "an XML model, whose schema language is not JSON Schema",
            json!({ "name": "Xml", "contentType": "application/xml",
                    "schema": "<xs:schema><xs:element name=\"order\"/></xs:schema>" }),
        ),
    ] {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/models"),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateModel, got {status} {created}"
        );
    }
}

/// A `StageName` the api already holds is refused with ONE sentence, whatever the name:
/// the rows span the name's length, alphabet and form, including the reserved one.
#[tokio::test]
async fn a_duplicate_stage_name_is_refused_with_one_sentence() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the stages api", "stage-conflict-text", "HTTP").await;

    // "Stage names can only contain alphanumeric characters, hyphens, and underscores.
    // Maximum length is 128 characters" (`CreateStage$StageName`), plus the reserved
    // `$default`, so the rows are the forms that alphabet admits — including its bound.
    let longest = "s".repeat(128);
    let names = ["prod", "a", "Stage_2-beta", "$default", longest.as_str()];

    for name in names {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": name, "description": "the first one" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {name:?}: expected 201 from the first CreateStage, got {status} {created}"
        );

        let answer = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": name, "description": "the second one" })),
        )
        .await;
        assert_refusal(name, 409, "ConflictException", STAGE_ALREADY_EXISTS, &answer);

        // The refused create wrote nothing: the stage a later caller reads is the first.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from GetStage, got {status} {fetched}"
        );
        assert_eq!(
            as_str(&fetched, "description"),
            "the first one",
            "case {name:?}: the refused create must not have rewritten the stage, got {fetched}"
        );
    }
}

/// One row of the payload-format-version table: an integration of `integration_type`
/// that the api accepts at version 1.0, and a version that type does not support.
struct PayloadRow {
    case: &'static str,
    protocol_type: &'static str,
    integration_type: &'static str,
    /// The members that integration needs besides its type and payload format.
    extra: Value,
    version: &'static str,
}

/// A `PayloadFormatVersion` the integration type does not support is refused with one
/// template naming the member and the value the request sent for it, on the create AND on
/// the update: the rows span the integration type, the protocol of the api that holds it,
/// and the version asked for.
#[tokio::test]
async fn an_unsupported_payload_format_version_is_refused_naming_the_version_and_the_type() {
    let rows = [
        PayloadRow {
            case: "an HTTP proxy asking for the Lambda proxy's version",
            protocol_type: "HTTP",
            integration_type: "HTTP_PROXY",
            extra: json!({ "integrationUri": "https://example.com/backend",
                           "integrationMethod": "GET" }),
            version: "2.0",
        },
        PayloadRow {
            case: "an AWS_PROXY naming a service action, not a function",
            protocol_type: "HTTP",
            integration_type: "AWS_PROXY",
            extra: json!({ "integrationSubtype": "SQS-SendMessage" }),
            version: "2.0",
        },
        PayloadRow {
            case: "a WebSocket AWS integration",
            protocol_type: "WEBSOCKET",
            integration_type: "AWS",
            extra: json!({
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:ws",
                "integrationMethod": "POST"
            }),
            version: "2.0",
        },
        PayloadRow {
            case: "a WebSocket MOCK integration, which invokes nothing",
            protocol_type: "WEBSOCKET",
            integration_type: "MOCK",
            extra: json!({}),
            version: "2.0",
        },
        PayloadRow {
            case: "a version no integration type supports",
            protocol_type: "HTTP",
            integration_type: "HTTP_PROXY",
            extra: json!({ "integrationUri": "https://example.com/backend" }),
            version: "3.0",
        },
        PayloadRow {
            case: "a Lambda proxy asking for a version outside its two",
            protocol_type: "HTTP",
            integration_type: "AWS_PROXY",
            extra: json!({
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:handler"
            }),
            version: "9.9",
        },
    ];

    let emulator = emulator();
    for (index, row) in rows.iter().enumerate() {
        let api_id = create_api(
            &emulator,
            row.case,
            &format!("payload-text-{index}"),
            row.protocol_type,
        )
        .await;
        let expected = format!("Unsupported PayloadFormatVersion: {}", row.version);

        // The create leg: the refused version never reaches the store.
        let mut request = row.extra.clone();
        request["integrationType"] = json!(row.integration_type);
        request["payloadFormatVersion"] = json!(row.version);
        let answer = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(request.clone()),
        )
        .await;
        assert_refusal(row.case, 400, "BadRequestException", &expected, &answer);

        // The same integration at the one version every type supports is accepted...
        request["payloadFormatVersion"] = json!("1.0");
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 for the same integration at 1.0, got {status} {created}",
            row.case
        );
        let integration_id = as_str(&created, "integrationId");

        // ... and the update leg answers the create's sentence for the same pair.
        let uri = format!("/v2/apis/{api_id}/integrations/{integration_id}");
        let answer = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ "payloadFormatVersion": row.version })),
        )
        .await;
        assert_refusal(row.case, 400, "BadRequestException", &expected, &answer);

        let (status, fetched) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetIntegration, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "payloadFormatVersion"),
            "1.0",
            "case {:?}: the refused patch must leave the stored version as it was, got {fetched}",
            row.case
        );
    }
}

/// An integration type an HTTP api has no behaviour for is refused with the sentence
/// that names the types such an api MAY carry — on the create and on the update, for
/// every value of the service's WebSocket-only catalog.
#[tokio::test]
async fn an_integration_type_an_http_api_cannot_carry_is_refused_naming_the_permitted_set() {
    // The message's parenthesised set is the service's own catalog, not a copy: the
    // types left for an api whose protocol is not WebSocket.
    let permitted = non_websocket_integration_types();
    assert!(
        PROXY_INTEGRATIONS_ONLY.ends_with(&format!("({})", permitted.join(", "))),
        "expected the refusal to name the permitted set {permitted:?}, got \
         {PROXY_INTEGRATIONS_ONLY:?}"
    );

    let emulator = emulator();
    let api_id = create_api(&emulator, "the http api", "proxy-only-text", "HTTP").await;

    // The integration an HTTP api does take, so the update leg has a record to patch.
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": "https://example.com/backend",
            "integrationMethod": "GET",
            "payloadFormatVersion": "1.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateIntegration for an HTTP_PROXY, got {status} {created}"
    );
    let integration_id = as_str(&created, "integrationId");
    let uri = format!("/v2/apis/{api_id}/integrations/{integration_id}");

    for integration_type in WEBSOCKET_ONLY_INTEGRATION_TYPES {
        let mut request = json!({
            "integrationType": integration_type,
            "payloadFormatVersion": "1.0"
        });
        // "MOCK: for integrating the route or method request with API Gateway as a
        // 'loopback' endpoint without invoking any backend", so it is the one type that
        // names no endpoint (`CreateIntegration$IntegrationType`).
        if *integration_type != "MOCK" {
            request["integrationUri"] = json!("https://example.com/backend");
        }

        let answer = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(request),
        )
        .await;
        assert_refusal(
            integration_type,
            400,
            "BadRequestException",
            PROXY_INTEGRATIONS_ONLY,
            &answer,
        );

        let answer = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ "integrationType": integration_type })),
        )
        .await;
        assert_refusal(
            integration_type,
            400,
            "BadRequestException",
            PROXY_INTEGRATIONS_ONLY,
            &answer,
        );
    }

    // Every type the refusal names IS accepted by that api, so the sentence is
    // actionable: it tells the caller what to send instead.
    for integration_type in &permitted {
        let mut request = json!({
            "integrationType": integration_type,
            "payloadFormatVersion": "1.0"
        });
        if *integration_type == "AWS_PROXY" {
            request["integrationUri"] =
                json!("arn:aws:lambda:eu-west-1:000000000000:function:handler");
        } else {
            request["integrationUri"] = json!("https://example.com/backend");
        }
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {integration_type:?}: the refusal names this type as permitted, so an \
             HTTP api must accept it, got {status} {body}"
        );
    }
}

/// One row of the authorizer-reference table.
struct AuthorizerRow {
    case: &'static str,
    authorization_type: &'static str,
    /// What the request carries for `AuthorizerId`: nothing, or the empty id.
    authorizer_id: Option<&'static str>,
    /// The sentence AWS answers this authorization type's invalid reference with.
    expected: &'static str,
}

/// A route of an authorizer-backed `AuthorizationType` that names no valid authorizer is
/// refused with AWS's invalid-reference sentence for that type, on `CreateRoute` and on
/// `UpdateRoute`: the rows span the authorization type and the form the missing
/// reference takes.
#[tokio::test]
async fn a_route_with_no_valid_authorizer_reference_is_refused_with_the_sentence_for_its_type() {
    let rows = [
        AuthorizerRow {
            case: "JWT with no AuthorizerId at all",
            authorization_type: "JWT",
            authorizer_id: None,
            expected: JWT_AUTHORIZER_REQUIRED,
        },
        AuthorizerRow {
            case: "JWT with an empty AuthorizerId",
            authorization_type: "JWT",
            authorizer_id: Some(""),
            expected: JWT_AUTHORIZER_REQUIRED,
        },
        AuthorizerRow {
            case: "CUSTOM with no AuthorizerId at all",
            authorization_type: "CUSTOM",
            authorizer_id: None,
            expected: UNRESOLVED_AUTHORIZER_MESSAGE,
        },
        AuthorizerRow {
            case: "CUSTOM with an empty AuthorizerId",
            authorization_type: "CUSTOM",
            authorizer_id: Some(""),
            expected: UNRESOLVED_AUTHORIZER_MESSAGE,
        },
    ];

    let emulator = emulator();
    let api_id = create_api(&emulator, "the routes api", "authorizer-text", "HTTP").await;

    for (index, row) in rows.iter().enumerate() {
        let mut authorization = json!({ "authorizationType": row.authorization_type });
        if let Some(authorizer_id) = row.authorizer_id {
            authorization["authorizerId"] = json!(authorizer_id);
        }

        // The create leg: the route is never written.
        let mut request = authorization.clone();
        request["routeKey"] = json!(format!("GET /created-{index}"));
        let answer = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(request),
        )
        .await;
        assert_refusal(row.case, 400, "BadRequestException", row.expected, &answer);

        // The update leg: a route that authorizes nobody cannot be patched onto an
        // authorizer-backed type without one, and the refused patch leaves it as it was.
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": format!("GET /patched-{index}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateRoute, got {status} {created}",
            row.case
        );
        let route_id = as_str(&created, "routeId");
        let uri = format!("/v2/apis/{api_id}/routes/{route_id}");

        let answer = call(&emulator, "PATCH", &uri, Some(authorization)).await;
        assert_refusal(row.case, 400, "BadRequestException", row.expected, &answer);

        let (status, fetched) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRoute, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "authorizationType"),
            "NONE",
            "case {:?}: the refused patch must leave the route as it was, got {fetched}",
            row.case
        );
    }

    // The same patch WITH an authorizer of the api is accepted, so the sentence is the
    // missing reference's and not the type's.
    let (status, authorizer) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "jwt-authorizer",
            "authorizerType": "JWT",
            "identitySource": ["$request.header.Authorization"],
            "jwtConfiguration": {
                "issuer": "https://example-issuer.test/oauth2",
                "audience": ["an-audience"]
            }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateAuthorizer, got {status} {authorizer}"
    );
    let authorizer_id = as_str(&authorizer, "authorizerId");

    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({ "routeKey": "GET /authorized" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateRoute, got {status} {created}"
    );
    let route_id = as_str(&created, "routeId");
    let (status, updated) = call(
        &emulator,
        "PATCH",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        Some(json!({ "authorizationType": "JWT", "authorizerId": authorizer_id })),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from UpdateRoute for a JWT route naming this api's authorizer, \
         got {status} {updated}"
    );
    assert_eq!(
        as_str(&updated, "authorizerId"),
        authorizer_id,
        "expected the patched route to name the authorizer, got {updated}"
    );
}
