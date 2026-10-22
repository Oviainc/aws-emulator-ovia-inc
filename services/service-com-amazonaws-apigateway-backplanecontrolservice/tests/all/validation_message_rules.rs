// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a request that breaks a modelled constraint, asserted on the
//! wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the request builder are this
//! crate's existing harness, reused from `rest_api_lifecycle`.
//!
//! The RULE under test, which is this service's and not one operation's: a refused
//! request is answered HTTP 400 `ValidationException` whose message names the offending
//! value, the member's path ROOTED AT THE OPERATION'S INPUT SHAPE (`<operation>Input` in
//! lowerCamel), and the constraint it failed — with a `@pattern` emitted without the
//! whole-value anchors the public model adds, and an enum value set listed in the order
//! the service declares the enum in.
//!
//! The rows span what the rule generalises over rather than repeating one recorded call:
//! six different operations (so the root is shown to be derived per operation, not a
//! constant), four constraint classes (an enum value set, a `@pattern`, an absent
//! required member, several violations at once), both HTTP bindings a violated member
//! can have (a body member and a URI label), and a nested member at two different list
//! positions.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{Answer, Gateway};

/// The error code the framework's refusal channel names a constraint failure by for this
/// `restJson1` service, and which this service's wording keeps.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `Op` enum's value set as API Gateway lists it. Source: the recorded refusals of an
/// invalid patch `op` on `UpdateDocumentationVersion`, `UpdateRestApi` and `UpdateApiKey`
/// — three operations, one order. (The public model declares the same six values in a
/// different order: `smithy_query(query="shape", shape="com.amazonaws.apigateway#Op")`.)
const OP_VALUES: [&str; 6] = ["add", "remove", "move", "test", "replace", "copy"];

/// The `ApiKeySourceType` value set, in the order the model declares it. Source:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#ApiKeySourceType")`.
const API_KEY_SOURCE_VALUES: [&str; 2] = ["HEADER", "AUTHORIZER"];

/// The `GatewayResponseType` value set, in the order the model declares it. Source:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#GatewayResponseType")`.
const GATEWAY_RESPONSE_TYPE_VALUES: [&str; 21] = [
    "DEFAULT_4XX",
    "DEFAULT_5XX",
    "RESOURCE_NOT_FOUND",
    "UNAUTHORIZED",
    "INVALID_API_KEY",
    "ACCESS_DENIED",
    "AUTHORIZER_FAILURE",
    "AUTHORIZER_CONFIGURATION_ERROR",
    "INVALID_SIGNATURE",
    "EXPIRED_TOKEN",
    "MISSING_AUTHENTICATION_TOKEN",
    "INTEGRATION_FAILURE",
    "INTEGRATION_TIMEOUT",
    "API_CONFIGURATION_ERROR",
    "UNSUPPORTED_MEDIA_TYPE",
    "BAD_REQUEST_PARAMETERS",
    "BAD_REQUEST_BODY",
    "REQUEST_TOO_LARGE",
    "THROTTLED",
    "QUOTA_EXCEEDED",
    "WAF_FILTERED",
];

/// The `StatusCode` shape's regular expression as the service emits it: the modelled
/// `^[1-5]\d\d$` without the whole-value anchors. Source:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#StatusCode")`.
const STATUS_CODE_PATTERN: &str = r"[1-5]\d\d";

/// The envelope one violation is stated in.
fn one(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause for a value outside its constraint.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The requirement an enum value set states.
fn enum_value_set(values: &[&str]) -> String {
    format!("Member must satisfy enum value set: [{}]", values.join(", "))
}

/// The requirement a `@pattern` states.
fn regular_expression(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// A list of patch operations whose `op` at 1-based `position` is `bad` and every other
/// is a legal one.
fn patch_operations(count: usize, position: usize, bad: &str) -> Value {
    Value::Array(
        (1..=count)
            .map(|at| {
                let op = if at == position { bad } else { "replace" };
                json!({ "op": op, "path": "/description", "value": "v" })
            })
            .collect(),
    )
}

/// Assert `answer` is the 400 `ValidationException` stating exactly `message`.
fn assert_stated(answer: &Answer, case: &str, message: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some(VALIDATION_EXCEPTION),
        "case {case}: expected {VALIDATION_EXCEPTION}, got {:?}",
        answer.error
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

#[tokio::test]
async fn a_refused_request_names_the_member_by_its_path_from_the_operations_input_shape() {
    // (case, method, uri, body, the message the service states)
    let rows: Vec<(String, &str, String, Option<Value>, String)> = vec![
        (
            "UpdateDocumentationVersion, an invalid patch op".to_string(),
            "PATCH",
            "/restapis/abcde12345/documentation/versions/1.0.0".to_string(),
            Some(json!({ "patchOperations": patch_operations(1, 1, "invalid_op") })),
            one(&clause(
                "invalid_op",
                "updateDocumentationVersionInput.patchOperations.1.member.op",
                &enum_value_set(&OP_VALUES),
            )),
        ),
        (
            // The same operation and member, at a different list position: the position
            // is computed from the request, not fixed at the recorded 1.
            "UpdateDocumentationVersion, an invalid patch op third in the list".to_string(),
            "PATCH",
            "/restapis/abcde12345/documentation/versions/1.0.0".to_string(),
            Some(json!({ "patchOperations": patch_operations(4, 3, "nope") })),
            one(&clause(
                "nope",
                "updateDocumentationVersionInput.patchOperations.3.member.op",
                &enum_value_set(&OP_VALUES),
            )),
        ),
        (
            "UpdateRestApi, an invalid patch op".to_string(),
            "PATCH",
            "/restapis/abcde12345".to_string(),
            Some(json!({ "patchOperations": patch_operations(1, 1, "invalid_op") })),
            one(&clause(
                "invalid_op",
                "updateRestApiInput.patchOperations.1.member.op",
                &enum_value_set(&OP_VALUES),
            )),
        ),
        (
            "UpdateApiKey, an invalid patch op".to_string(),
            "PATCH",
            "/apikeys/abcde12345".to_string(),
            Some(json!({ "patchOperations": patch_operations(2, 2, "invalid_op") })),
            one(&clause(
                "invalid_op",
                "updateApiKeyInput.patchOperations.2.member.op",
                &enum_value_set(&OP_VALUES),
            )),
        ),
        (
            // An operation no recording covers, carrying the same member: the root
            // follows the operation, so the rule holds without a case for each one.
            "UpdateStage, an invalid patch op".to_string(),
            "PATCH",
            "/restapis/abcde12345/stages/prod".to_string(),
            Some(json!({ "patchOperations": patch_operations(1, 1, "upsert") })),
            one(&clause(
                "upsert",
                "updateStageInput.patchOperations.1.member.op",
                &enum_value_set(&OP_VALUES),
            )),
        ),
        (
            // A `@pattern` on a BODY member, emitted without the model's anchors.
            "PutGatewayResponse, a status code outside its pattern".to_string(),
            "PUT",
            "/restapis/abcde12345/gatewayresponses/DEFAULT_4XX".to_string(),
            Some(json!({ "statusCode": "999" })),
            one(&clause(
                "999",
                "putGatewayResponseInput.statusCode",
                &regular_expression(STATUS_CODE_PATTERN),
            )),
        ),
        (
            // The SAME pattern on a member bound to the URI, not the body: where the
            // member travels does not change how the refusal is stated.
            "PutIntegrationResponse, a status code label outside its pattern".to_string(),
            "PUT",
            "/restapis/abcde12345/resources/fghij67890/methods/GET/integration/responses/abc"
                .to_string(),
            Some(json!({})),
            one(&clause(
                "abc",
                "putIntegrationResponseInput.statusCode",
                &regular_expression(STATUS_CODE_PATTERN),
            )),
        ),
        (
            // An enum value set on a member bound to the URI.
            "PutGatewayResponse, a response type outside its enum".to_string(),
            "PUT",
            "/restapis/abcde12345/gatewayresponses/NOT_A_RESPONSE_TYPE".to_string(),
            Some(json!({ "statusCode": "404" })),
            one(&clause(
                "NOT_A_RESPONSE_TYPE",
                "putGatewayResponseInput.responseType",
                &enum_value_set(&GATEWAY_RESPONSE_TYPE_VALUES),
            )),
        ),
        (
            "CreateRestApi, an api key source outside its enum".to_string(),
            "POST",
            "/restapis".to_string(),
            Some(json!({ "name": "petstore", "apiKeySource": "SOMEWHERE_ELSE" })),
            one(&clause(
                "SOMEWHERE_ELSE",
                "createRestApiInput.apiKeySource",
                &enum_value_set(&API_KEY_SOURCE_VALUES),
            )),
        ),
        (
            // An absent required member is found while READING the request rather than
            // while walking it, and is stated with the same rooted path.
            "CreateRestApi, no name at all".to_string(),
            "POST",
            "/restapis".to_string(),
            Some(json!({ "description": "nameless" })),
            one(
                "Value null at 'createRestApiInput.name' failed to satisfy constraint: \
                 Member must not be null",
            ),
        ),
    ];

    let gateway = Gateway::new();
    for (case, method, uri, body, expected) in rows {
        let answer = gateway.call(method, &uri, body).await;
        assert_stated(&answer, &case, &expected);
    }
}

#[tokio::test]
async fn every_violation_of_one_request_is_stated_with_the_same_rooted_path() {
    // Two members of one request break their constraints at once: the envelope counts
    // them and every clause carries the operation's root. The ORDER the walk reports
    // them in belongs to the framework, so the clauses are looked for rather than
    // sequenced.
    let gateway = Gateway::new();
    let answer = gateway
        .call(
            "PUT",
            "/restapis/abcde12345/gatewayresponses/NOT_A_RESPONSE_TYPE",
            Some(json!({ "statusCode": "99" })),
        )
        .await;
    let case = "PutGatewayResponse(bad responseType and statusCode)";

    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    let message = answer.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a message, got {:?}", answer.body))
        .to_string();
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case}: expected the two-violation envelope, got {message:?}"
    );
    for expected in [
        clause(
            "NOT_A_RESPONSE_TYPE",
            "putGatewayResponseInput.responseType",
            &enum_value_set(&GATEWAY_RESPONSE_TYPE_VALUES),
        ),
        clause(
            "99",
            "putGatewayResponseInput.statusCode",
            &regular_expression(STATUS_CODE_PATTERN),
        ),
    ] {
        assert!(
            message.contains(&expected),
            "case {case}: expected the clause {expected:?} in {message:?}"
        );
    }
}

#[tokio::test]
async fn a_request_within_every_modelled_constraint_is_not_refused_as_a_validation_error() {
    // The wording seam must not refuse what the model accepts: each row satisfies every
    // constraint its operation declares, so none of them may answer the validation
    // error above. What they DO answer is their own operation's business (a 201, or the
    // modelled 404 for an API that does not exist, or the scaffold's 501).
    let rows: [(&str, &str, &str, Option<Value>); 5] = [
        (
            "CreateRestApi with every modelled member",
            "POST",
            "/restapis",
            Some(json!({
                "name": "petstore",
                "apiKeySource": "HEADER",
                "endpointConfiguration": { "types": ["REGIONAL"] },
            })),
        ),
        (
            "UpdateRestApi with a legal op at each position",
            "PATCH",
            "/restapis/abcde12345",
            Some(json!({ "patchOperations": patch_operations(3, 0, "unused") })),
        ),
        (
            "PutGatewayResponse within the status-code pattern",
            "PUT",
            "/restapis/abcde12345/gatewayresponses/DEFAULT_4XX",
            Some(json!({ "statusCode": "404" })),
        ),
        (
            "PutIntegrationResponse within the status-code pattern",
            "PUT",
            "/restapis/abcde12345/resources/fghij67890/methods/GET/integration/responses/200",
            Some(json!({})),
        ),
        ("GetRestApis, which constrains nothing", "GET", "/restapis", None),
    ];

    let gateway = Gateway::new();
    for (case, method, uri, body) in rows {
        let answer = gateway.call(method, uri, body).await;
        assert_ne!(
            answer.error.as_deref(),
            Some(VALIDATION_EXCEPTION),
            "case {case}: expected no validation refusal, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}
