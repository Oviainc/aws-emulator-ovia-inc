// SPDX-License-Identifier: CC0-1.0

//! The refusal MESSAGES this control plane answers with, asserted on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness is this crate's existing one
//! (`rest_api_lifecycle`), reused rather than stood up again.
//!
//! The RULE under test: where API Gateway words a refusal as a SENTENCE OF ITS OWN for
//! the condition — an unaddressable patch path, a member outside its published window, a
//! record that already exists, a body no importer can read, a cursor the service never
//! issued, a reserved tag key, a quota that has been reached — the message is that fixed
//! sentence, carrying only the values the condition itself names (the path a patch sent,
//! the location or version that already exists, the member's own published bounds). It
//! never quotes the rejected value back, never names the member the handler happened to
//! check, and never enumerates what the request could have said instead.
//!
//! The sentence for an unaddressable patch path is the one the OPERATION words, and not
//! every operation words it the same way: the rows below are the operations whose
//! refusal is the path alone. `UpdateApiKey` words its own, which the
//! `update_api_key_patch_path_refusal` suite covers.
//!
//! Each test's rows span what the rule generalises over rather than repeating one
//! recorded call: several operations per rule, both the create and the patch writer of
//! one member, each published bound and one past it, and every form a path, a location or
//! a tag key can take.

use serde_json::{json, Value};

use crate::authorizer_lifecycle::{create_authorizer, lambda_uri, token_request};
use crate::documentation_parts::{content_map, created_part_id, import_parts};
use crate::documentation_versions::{create_version, snapshot_request};
use crate::method_integration::put_integration;
use crate::method_response::put_method_response;
use crate::patch_updates::{patch, replace};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::{api_with_a_method, create_deployed_stage, create_deployment};
use crate::tag_reads_by_arn::{apigateway_arn, encode_label};
use crate::usage_plan_lifecycle::create_usage_plan;

/// The code every refusal below carries: a bad request this service models as
/// `BadRequestException` (HTTP 400).
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");

/// The code a refusal that names a record the API does not hold carries.
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// The code a refusal that names a record the API ALREADY holds carries.
const CONFLICT: (u16, &str) = (409, "ConflictException");

/// The code a refusal past a quota carries, with the status its model declares.
const LIMIT_EXCEEDED: (u16, &str) = (429, "LimitExceededException");

/// Assert `answer` is the modelled failure `(status, code)` carrying exactly `message`.
fn assert_refusal(answer: &Answer, expected: (u16, &str), message: &str, case: &str) {
    assert_failure(answer, expected.0, expected.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// `UpdateStage` for one stage of `api`.
async fn patch_stage(gateway: &Gateway, api: &str, stage: &str, operations: Vec<Value>) -> Answer {
    patch(gateway, &format!("/restapis/{api}/stages/{stage}"), operations).await
}

/// `CreateUsagePlan` with `request` sent as given.
async fn create_plan(gateway: &Gateway, request: Value) -> Answer {
    gateway.call("POST", "/usageplans", Some(request)).await
}

/// `CreateDocumentationPart` for `location`, sent as given.
async fn create_part(gateway: &Gateway, api: &str, location: Value) -> Answer {
    gateway
        .call(
            "POST",
            &format!("/restapis/{api}/documentation/parts"),
            Some(json!({ "location": location, "properties": content_map("documented") })),
        )
        .await
}

/// `DeleteGatewayResponse` for one response type of `api`.
async fn delete_gateway_response(gateway: &Gateway, api: &str, response_type: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/gatewayresponses/{response_type}"),
            None,
        )
        .await
}

/// `ImportRestApi` with `body` sent verbatim (`POST /restapis?mode=import`).
async fn import_rest_api(gateway: &Gateway, body: &str) -> Answer {
    gateway.call_raw("POST", "/restapis?mode=import", body).await
}

/// `TagResource` for `resource_arn` with one tag.
async fn tag_resource(gateway: &Gateway, resource_arn: &str, key: &str, value: &str) -> Answer {
    gateway
        .call(
            "PUT",
            &format!("/tags/{}", encode_label(resource_arn)),
            Some(json!({ "tags": { key: value } })),
        )
        .await
}

#[tokio::test]
async fn a_patch_path_the_resource_cannot_be_addressed_by_is_refused_with_the_path_alone() {
    let gateway = Gateway::new();
    let (api, resource) = api_with_a_method(&gateway, "patch-path-sentence").await;
    let method_response =
        put_method_response(&gateway, &api, &resource, "GET", "200", json!({})).await;
    assert_eq!(
        method_response.status, 201,
        "case PutMethodResponse(200): expected 201, got {} with {:?}",
        method_response.status, method_response.body
    );
    create_deployed_stage(&gateway, &api, "prod").await;
    // Rows differ along what the rule generalises over: the operation and so the resource
    // the pointer was applied to, and the FORM of the unaddressable pointer — a member the
    // service assigns, a member no shape declares, a pointer reaching into a single-valued
    // member, a pointer naming a map member with no key, and a path that is no JSON Pointer
    // at all. The operations that enumerate their published paths instead — which is most
    // of the `Update*` surface — are the `patch_path_refusal_templates` suite's.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "UpdateRestApi names a service-assigned member",
            format!("/restapis/{api}"),
            "/id",
        ),
        (
            "UpdateRestApi names no member of the shape",
            format!("/restapis/{api}"),
            "/nosuchmember",
        ),
        (
            "UpdateRestApi reaches into a single-valued member",
            format!("/restapis/{api}"),
            "/name/first",
        ),
        (
            "UpdateStage names a map member with no key",
            format!("/restapis/{api}/stages/prod"),
            "/variables",
        ),
        (
            "UpdateMethodResponse names the response's own identity",
            format!("/restapis/{api}/resources/{resource}/methods/GET/responses/200"),
            "/statusCode",
        ),
        (
            "UpdateMethodResponse sends a path that is no JSON Pointer",
            format!("/restapis/{api}/resources/{resource}/methods/GET/responses/200"),
            "statusCode",
        ),
    ];

    for (case, uri, path) in rows {
        let answer = patch(&gateway, &uri, vec![replace(path, "x")]).await;
        assert_refusal(
            &answer,
            BAD_REQUEST,
            &format!("Invalid patch path {path}"),
            case,
        );
    }
}

#[tokio::test]
async fn an_authorizer_configuration_its_type_does_not_admit_is_refused_with_one_sentence() {
    // The window `authorizerResultTtlInSeconds` publishes: "If it equals 0, authorization
    // caching is disabled" and "The maximum value is 3600, or 1 hour"
    // (`smithy_query(query="shape", shape="com.amazonaws.apigateway#Authorizer",
    // member="authorizerResultTtlInSeconds")`).
    const TTL_RANGE: &str =
        "Authorizer result TTL outside allowable range. TTL must be between 0 and 3600 seconds.";
    const INVALID_INPUT: &str = "Invalid request input";
    // The one member of the three whose absence API Gateway names in the refusal; see
    // the `authorizer_required_members` suite for the rule.
    const URI_REQUIRED: &str = "URI is a required field in an Authorizer";

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-sentences").await;
    let created = create_authorizer(&gateway, &api, token_request("ttl-baseline")).await;
    assert_eq!(
        created.status, 201,
        "case CreateAuthorizer(baseline): expected 201, got {} with {:?}",
        created.status, created.body
    );
    let authorizer = string_member(&created.body, "id", "CreateAuthorizer(baseline)");

    // Rows differ along what the rule generalises over: how far outside the window the TTL
    // is and on which side of it, and whether the TTL rode in on the create or on a patch
    // of the stored record. Each published bound and one past it is covered: 0 and 3600 are
    // accepted below, -1 and 3601 are the first values outside.
    for ttl in [-1, -300, 3601, 7200, 86_400] {
        let mut request = token_request(&format!("ttl{ttl}"));
        request["authorizerResultTtlInSeconds"] = json!(ttl);
        let answer = create_authorizer(&gateway, &api, request).await;
        assert_refusal(
            &answer,
            BAD_REQUEST,
            TTL_RANGE,
            &format!("CreateAuthorizer(authorizerResultTtlInSeconds={ttl})"),
        );

        let patched = patch(
            &gateway,
            &format!("/restapis/{api}/authorizers/{authorizer}"),
            vec![replace("/authorizerResultTtlInSeconds", &ttl.to_string())],
        )
        .await;
        assert_refusal(
            &patched,
            BAD_REQUEST,
            TTL_RANGE,
            &format!("UpdateAuthorizer(/authorizerResultTtlInSeconds={ttl})"),
        );
    }

    // Each published bound of the window is ACCEPTED, so the refusal above is the window's
    // and not a blanket one.
    for ttl in [0, 1, 3600] {
        let mut request = token_request(&format!("accepted-ttl{ttl}"));
        request["authorizerResultTtlInSeconds"] = json!(ttl);
        let answer = create_authorizer(&gateway, &api, request).await;
        assert_eq!(
            answer.status, 201,
            "case CreateAuthorizer(authorizerResultTtlInSeconds={ttl}): expected 201, got {} \
             with {:?}",
            answer.status, answer.body
        );
    }

    // Rows differ along which member the authorizer's own `type` requires and the request
    // leaves out — the catalog is
    // `smithy_query(query="shape", shape="com.amazonaws.apigateway#AuthorizerType")`. The
    // missing `authorizerUri` is the one API Gateway names the field for; for the others the
    // service makes no claim about which member is at fault.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a TOKEN authorizer with no identitySource",
            json!({
                "name": "token-no-identity-source",
                "type": "TOKEN",
                "authorizerUri": lambda_uri("authorize"),
            }),
            INVALID_INPUT,
        ),
        (
            "a TOKEN authorizer with no authorizerUri",
            json!({
                "name": "token-no-uri",
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
            }),
            URI_REQUIRED,
        ),
        (
            "a REQUEST authorizer with caching on and no identitySource",
            json!({
                "name": "request-no-identity-source",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("authorize"),
            }),
            INVALID_INPUT,
        ),
        (
            "a COGNITO_USER_POOLS authorizer with no providerARNs",
            json!({
                "name": "cognito-no-pools",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Auth",
            }),
            INVALID_INPUT,
        ),
    ];

    for (case, request, expected) in rows {
        let answer = create_authorizer(&gateway, &api, request).await;
        assert_refusal(&answer, BAD_REQUEST, expected, case);
    }
}

#[tokio::test]
async fn a_usage_plan_limit_that_is_no_limit_is_refused_with_the_plans_own_sentence() {
    const QUOTA_LIMIT: &str = "Usage Plan quota limit must be a non-negative numeric";
    const QUOTA_OFFSET: &str = "Usage Plan quota offset must be a non-negative numeric";
    const THROTTLE: &str =
        "Usage Plan throttle rate limit and burst limit must be numeric and greater than zero";

    let gateway = Gateway::new();

    // Rows differ along which limit is not a limit (the quota's two counts, the throttle's
    // rate and burst), how far below zero it is, and the period it rides with — the quota's
    // two counts each have their own sentence and the throttle's two limits share one.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a quota limit one below zero",
            json!({ "name": "quota-limit-minus-one", "quota": { "limit": -1, "period": "DAY" } }),
            QUOTA_LIMIT,
        ),
        (
            "a quota limit far below zero",
            json!({ "name": "quota-limit-minus-many", "quota": { "limit": -5000, "period": "MONTH" } }),
            QUOTA_LIMIT,
        ),
        (
            "a quota offset below zero",
            json!({
                "name": "quota-offset-negative",
                "quota": { "limit": 100, "offset": -1, "period": "WEEK" },
            }),
            QUOTA_OFFSET,
        ),
        (
            "a throttle rate below zero",
            json!({ "name": "throttle-rate-negative", "throttle": { "rateLimit": -5.0 } }),
            THROTTLE,
        ),
        (
            "a throttle burst below zero",
            json!({
                "name": "throttle-burst-negative",
                "throttle": { "rateLimit": 10.0, "burstLimit": -1 },
            }),
            THROTTLE,
        ),
    ];

    for (case, request, message) in rows {
        let answer = create_plan(&gateway, request).await;
        assert_refusal(&answer, BAD_REQUEST, message, case);
    }

    // Zero is a limit this service meters, so the refusals above are the rule's and not a
    // blanket one.
    let accepted = create_plan(
        &gateway,
        json!({
            "name": "zero-limits",
            "quota": { "limit": 0, "offset": 0, "period": "DAY" },
            "throttle": { "rateLimit": 0.0, "burstLimit": 0 },
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "case CreateUsagePlan(zero limits): expected 201, got {} with {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn a_documentation_record_that_already_exists_is_refused_with_the_record_named() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-conflicts").await;

    // Rows differ along the location's TYPE and so along which of its members the type
    // makes valid — the valid-field sets are
    // `smithy_query(query="shape", shape="com.amazonaws.apigateway#DocumentationPartLocation")`
    // — because the sentence names the location that is already documented, member by
    // member.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a second part of the whole API",
            json!({ "type": "API" }),
            "type 'API'.",
        ),
        (
            "a second part of one resource",
            json!({ "type": "RESOURCE", "path": "/pets" }),
            "type 'RESOURCE', path '/pets'.",
        ),
        (
            "a second part of one method",
            json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
            "type 'METHOD', path '/pets', method 'GET'.",
        ),
        (
            "a second part of one response",
            json!({ "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "404" }),
            "type 'RESPONSE', path '/pets', method 'GET', statusCode '404'.",
        ),
        (
            "a second part of one query parameter",
            json!({
                "type": "QUERY_PARAMETER",
                "path": "/pets",
                "method": "GET",
                "name": "petId",
            }),
            "type 'QUERY_PARAMETER', path '/pets', method 'GET', name 'petId'.",
        ),
    ];

    for (case, location, named) in rows {
        created_part_id(
            &gateway,
            &api,
            location.clone(),
            &content_map("documented"),
            case,
        )
        .await;
        let answer = create_part(&gateway, &api, location).await;
        assert_refusal(
            &answer,
            CONFLICT,
            &format!("Documentation part already exists for the specified location: {named}"),
            case,
        );
    }

    // Rows differ along the form of the version identifier, because the sentence names the
    // identifier that already exists.
    for version in ["1.0.0", "v2", "2026-01-01 snapshot"] {
        let created = create_version(&gateway, &api, snapshot_request(version, None)).await;
        assert_eq!(
            created.status, 201,
            "case CreateDocumentationVersion({version:?}): expected 201, got {} with {:?}",
            created.status, created.body
        );
        let answer = create_version(&gateway, &api, snapshot_request(version, Some("again"))).await;
        assert_refusal(
            &answer,
            CONFLICT,
            &format!("Documentation version {version} already exists."),
            &format!("CreateDocumentationVersion({version:?}) twice"),
        );
    }
}

#[tokio::test]
async fn deleting_a_gateway_response_the_api_never_customized_is_refused_with_one_sentence() {
    const NOT_DEFINED: &str = "Gateway response type not defined on api";

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-sentence").await;

    // Rows differ along the response type the request names — each of them has the response
    // API Gateway generates itself, so what is missing is the customization and the sentence
    // names neither the type nor the API.
    for response_type in ["DEFAULT_4XX", "DEFAULT_5XX", "UNAUTHORIZED", "WAF_FILTERED"] {
        let answer = delete_gateway_response(&gateway, &api, response_type).await;
        assert_refusal(
            &answer,
            NOT_FOUND,
            NOT_DEFINED,
            &format!("DeleteGatewayResponse({response_type}) with no customization"),
        );
    }

    // A customization that WAS stored is cleared, and the second delete of it is the same
    // refusal: what the sentence reports is the absent customization.
    let put = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/gatewayresponses/DEFAULT_4XX"),
            Some(json!({ "statusCode": "400" })),
        )
        .await;
    assert_eq!(
        put.status, 201,
        "case PutGatewayResponse(DEFAULT_4XX): expected 201, got {} with {:?}",
        put.status, put.body
    );
    let first = delete_gateway_response(&gateway, &api, "DEFAULT_4XX").await;
    assert_eq!(
        first.status, 202,
        "case DeleteGatewayResponse(DEFAULT_4XX) once: expected 202, got {} with {:?}",
        first.status, first.body
    );
    let second = delete_gateway_response(&gateway, &api, "DEFAULT_4XX").await;
    assert_refusal(
        &second,
        NOT_FOUND,
        NOT_DEFINED,
        "DeleteGatewayResponse(DEFAULT_4XX) twice",
    );
}

#[tokio::test]
async fn a_body_no_importer_can_read_is_refused_with_the_importers_own_sentence() {
    const UNREADABLE_PARTS: &str = "Unable to build importer with provided input.";
    const INVALID_DEFINITION: &str = "Invalid OAS input";

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "import-sentences").await;

    // Rows differ along HOW the body is unreadable — not JSON at all, JSON that is not an
    // object, an object declaring no documentation parts, and parts that are not a list —
    // and every one of them is the same sentence.
    let documentation_bodies = [
        ("a body that is not JSON", "not-json-at-all"),
        ("a body that is a JSON array", "[]"),
        ("a body that is a JSON string", "\"documentation\""),
        ("an object declaring no documentation parts", "{}"),
        (
            "documentation parts that are not a list",
            "{\"x-amazon-apigateway-documentation\":{\"documentationParts\":{}}}",
        ),
    ];
    for (case, body) in documentation_bodies {
        let answer = import_parts(&gateway, &api, Some("merge"), false, body).await;
        assert_refusal(&answer, BAD_REQUEST, UNREADABLE_PARTS, case);
    }

    // The same, for the API definition the two import operations read: unparsable, not an
    // object, and an object that declares no OpenAPI version.
    let definitions = [
        ("a definition that is not JSON or YAML", "}not a document{"),
        ("a definition that is a JSON array", "[]"),
        (
            "a definition declaring no OpenAPI version",
            "{\"info\":{\"title\":\"no version\"}}",
        ),
    ];
    for (case, body) in definitions {
        let imported = import_rest_api(&gateway, body).await;
        assert_refusal(
            &imported,
            BAD_REQUEST,
            INVALID_DEFINITION,
            &format!("ImportRestApi with {case}"),
        );
        let put = gateway
            .call_raw("PUT", &format!("/restapis/{api}?mode=overwrite"), body)
            .await;
        assert_refusal(
            &put,
            BAD_REQUEST,
            INVALID_DEFINITION,
            &format!("PutRestApi with {case}"),
        );
    }
}

#[tokio::test]
async fn a_member_outside_its_published_window_is_refused_with_that_members_own_sentence() {
    // `RestApi$minimumCompressionSize`: "non-negative between 0 and 10485760 (10M) bytes,
    // inclusive". `Integration$timeoutInMillis`: "Custom timeout between 50 and 29,000
    // milliseconds". `CanarySettings$percentTraffic`: "The percent (0-100) of traffic
    // diverted to a canary deployment."
    const COMPRESSION: &str = "Invalid minimum compression size, must be between 0 and 10485760";
    const TIMEOUT: &str = "Timeout should be between 50 ms and 29000 ms";
    const CANARY: &str = "Invalid traffic settings for stage canary. Must be between 0 and 100";

    let gateway = Gateway::new();
    let (api, resource) = api_with_a_method(&gateway, "window-sentences").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    // Each published bound is accepted and each value one past it is refused, on BOTH
    // writers of the member: the create or put that sets it and the patch that changes it.
    for size in [-1, -10_485_760, 10_485_761, i32::MAX] {
        let created = gateway
            .call(
                "POST",
                "/restapis",
                Some(json!({ "name": format!("compression{size}"), "minimumCompressionSize": size })),
            )
            .await;
        assert_refusal(
            &created,
            BAD_REQUEST,
            COMPRESSION,
            &format!("CreateRestApi(minimumCompressionSize={size})"),
        );
        let patched = patch(
            &gateway,
            &format!("/restapis/{api}"),
            vec![replace("/minimumCompressionSize", &size.to_string())],
        )
        .await;
        assert_refusal(
            &patched,
            BAD_REQUEST,
            COMPRESSION,
            &format!("UpdateRestApi(/minimumCompressionSize={size})"),
        );
    }

    for millis in [49, 0, -1, 29_001, 30_001] {
        let put = put_integration(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({ "type": "MOCK", "timeoutInMillis": millis }),
        )
        .await;
        assert_refusal(
            &put,
            BAD_REQUEST,
            TIMEOUT,
            &format!("PutIntegration(timeoutInMillis={millis})"),
        );
    }
    let stored = put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "type": "MOCK", "timeoutInMillis": 50 }),
    )
    .await;
    assert_eq!(
        stored.status, 201,
        "case PutIntegration(timeoutInMillis=50): expected 201, got {} with {:?}",
        stored.status, stored.body
    );
    for millis in [49, 29_001] {
        let patched = patch(
            &gateway,
            &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            vec![replace("/timeoutInMillis", &millis.to_string())],
        )
        .await;
        assert_refusal(
            &patched,
            BAD_REQUEST,
            TIMEOUT,
            &format!("UpdateIntegration(/timeoutInMillis={millis})"),
        );
    }

    for percent in ["-1", "100.5", "150", "1000"] {
        let created = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/stages"),
                Some(json!({
                    "stageName": "canary",
                    "deploymentId": create_deployment(
                        &gateway,
                        &api,
                        "the deployment the canary rows serve",
                        json!({}),
                    )
                    .await,
                    "canarySettings": { "percentTraffic": percent.parse::<f64>().unwrap_or_default() },
                })),
            )
            .await;
        assert_refusal(
            &created,
            BAD_REQUEST,
            CANARY,
            &format!("CreateStage(canarySettings.percentTraffic={percent})"),
        );
        let patched = patch_stage(
            &gateway,
            &api,
            "prod",
            vec![replace("/canarySettings/percentTraffic", percent)],
        )
        .await;
        assert_refusal(
            &patched,
            BAD_REQUEST,
            CANARY,
            &format!("UpdateStage(/canarySettings/percentTraffic={percent})"),
        );
    }
    for percent in ["0", "0.5", "100"] {
        let patched = patch_stage(
            &gateway,
            &api,
            "prod",
            vec![replace("/canarySettings/percentTraffic", percent)],
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case UpdateStage(/canarySettings/percentTraffic={percent}): expected 200, got {} \
             with {:?}",
            patched.status, patched.body
        );
    }

    // A private integration names the VPC link it reaches the back end through, and the
    // refusal says what that member must be set to rather than which member was missing.
    let private = put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({
            "type": "HTTP_PROXY",
            "integrationHttpMethod": "GET",
            "uri": "http://pets.internal/pets",
            "connectionType": "VPC_LINK",
        }),
    )
    .await;
    assert_refusal(
        &private,
        BAD_REQUEST,
        "ConnectionId should be set to the vpcLinkId or stage variable on connection type \
         VPC_LINK",
        "PutIntegration(connectionType=VPC_LINK) with no connectionId",
    );
}

#[tokio::test]
async fn a_position_this_service_never_issued_is_refused_with_one_sentence() {
    const INVALID_POSITION: &str = "Invalid position parameter";

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "position-sentence").await;
    created_part_id(
        &gateway,
        &api,
        json!({ "type": "API" }),
        &content_map("a part so the part listing is not empty"),
        "CreateDocumentationPart",
    )
    .await;
    create_usage_plan(
        &gateway,
        "a plan so the plan listing is not empty",
        json!({ "name": "position-sentence" }),
    )
    .await;

    // Rows differ along the listing the cursor was offered to and along the FORM of the
    // cursor — a token no listing issued, one that looks like an id, and one carrying
    // characters an id never has — because the sentence names the member and nothing else.
    //
    // Every listing here is cursored by a MINTED key, which a cursor either names or does
    // not. The documentation-parts collection is not: its cursor is the location of the
    // entity a part documents, a point in the order the collection reports, so every text
    // places one and none is unresolvable (`documentation_part_pagination`).
    let listings = [
        ("GetRestApis", "/restapis".to_string()),
        ("GetUsagePlans", "/usageplans".to_string()),
        (
            "GetDeployments",
            format!("/restapis/{api}/deployments"),
        ),
    ];
    for (case, uri) in listings {
        for cursor in ["not-a-real-pagination-token", "abcde12345", "%7Bcursor%7D"] {
            let answer = gateway
                .call("GET", &format!("{uri}?position={cursor}"), None)
                .await;
            assert_refusal(
                &answer,
                BAD_REQUEST,
                INVALID_POSITION,
                &format!("{case}(position={cursor})"),
            );
        }
    }
}

#[tokio::test]
async fn a_tag_key_in_the_reserved_namespace_is_refused_with_one_sentence() {
    const RESERVED: &str = "Invalid tag key. Tag keys cannot start with 'aws:' (case insensitive)";

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "reserved-tag-sentence").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    // Rows differ along the CASE of the reserved prefix and what follows it, because the
    // rule holds whatever the key's case and the sentence quotes no key back.
    let keys = ["aws:Reserved", "AWS:Reserved", "Aws:cloudformation", "aws:"];
    // …and along the resource the tags were offered to: the ARN travels in the request's
    // own path label, and the rule is the service's rather than one resource's.
    let resources = [
        format!("/restapis/{api}"),
        format!("/restapis/{api}/stages/prod"),
    ];
    for resource in &resources {
        let arn = apigateway_arn(resource);
        for key in keys {
            let answer = tag_resource(&gateway, &arn, key, "value").await;
            assert_refusal(
                &answer,
                BAD_REQUEST,
                RESERVED,
                &format!("TagResource({resource}, {key})"),
            );
        }
        // A key that merely CONTAINS the prefix is not in the reserved namespace. The
        // modelled success of `TagResource` is 204, which carries no body.
        let accepted = tag_resource(&gateway, &arn, "team:aws:owned", "value").await;
        assert_eq!(
            accepted.status, 204,
            "case TagResource({resource}, team:aws:owned): expected 204, got {} with {:?}",
            accepted.status, accepted.body
        );
    }

    // The same rule on the create operations that carry tags, so a key this service refuses
    // on one path cannot be stored through another.
    let created = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({ "name": "tagged-on-create", "tags": { "aws:Reserved": "value" } })),
        )
        .await;
    assert_refusal(
        &created,
        BAD_REQUEST,
        RESERVED,
        "CreateRestApi(tags={aws:Reserved})",
    );
}

#[tokio::test]
async fn the_per_api_stage_quota_is_reported_without_quoting_the_limit() {
    // The sentence AWS answers a reached quota with: it names the resources the quota
    // counts and the scope it counts them in, and no number anywhere.
    const STAGES_REACHED: &str = "Maximum number of Stages for this API has been reached.  \
                                  Please contact AWS if you need additional Stages.";

    let gateway = Gateway::new();
    let (api, _resource) = api_with_a_method(&gateway, "stage-quota-sentence").await;
    let deployment = create_deployment(
        &gateway,
        &api,
        "the deployment every stage serves",
        json!({}),
    )
    .await;

    // Fill the per-API stage quota, then ask for one more stage through each of the two
    // operations that create one: the sentence is the quota's and not one operation's.
    let mut created = 0usize;
    loop {
        let answer = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/stages"),
                Some(json!({ "stageName": format!("stage{created}"), "deploymentId": deployment })),
            )
            .await;
        if answer.status != 201 {
            assert_refusal(
                &answer,
                LIMIT_EXCEEDED,
                STAGES_REACHED,
                &format!("CreateStage past the quota, with {created} stages held"),
            );
            break;
        }
        created += 1;
        assert!(
            created < 100,
            "case CreateStage: the per-API stage quota must be reached, got {created} stages"
        );
    }

    let deployed = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": "one-stage-too-many" })),
        )
        .await;
    assert_refusal(
        &deployed,
        LIMIT_EXCEEDED,
        STAGES_REACHED,
        "CreateDeployment creating one stage past the quota",
    );
}
