// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateGatewayResponse`, read back through `GetGatewayResponse`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi` helper and the PATCH wrappers are this crate's existing harness,
//! reused from `rest_api_lifecycle`, `patch_updates` and the sibling `gateway_response`
//! suite.
//!
//! The RULE under test: an `UpdateGatewayResponse` applies every entry of
//! `patchOperations` to the gateway response its `(restApiId, responseType)` names, in
//! request order, and reports the updated record — so a later read reports exactly what
//! the patch set — while a patch this resource has no updateable member for, an op such a
//! member does not support, or a value outside the member's type is refused with a
//! modelled error and changes nothing. A response type the API holds no customization for
//! is patched from the response API Gateway generates for it, and what the patch produces
//! is a developer's customization, which is what `defaultResponse` reports. The two
//! updateable members are the string-to-string maps `responseParameters` and
//! `responseTemplates` and the single-valued `statusCode`, which takes a `replace`
//! (`smithy_query(query="operation", operation="UpdateGatewayResponse")`);
//! `responseType` is the record's identity and `defaultResponse` the service's own report,
//! so neither is patchable.
//!
//! The tables span what the rule generalises over: which member the path names, that
//! member's kind (single-valued, map entry), the op (`add`, `replace`, `remove`), how many
//! ops one request carries, a key carrying an escaped `/` pointer token, the status code
//! (values across the modelled pattern and each published bound), the response type
//! (values from across the modelled enum, including the two that generate no status code
//! of their own), and whether the type was customized before the patch.

use serde_json::{json, Value};

use crate::gateway_response::{generated_status_code, get_gateway_response, put_gateway_response};
use crate::patch_updates::{patch, patch_ok, replace};
use crate::rest_api_lifecycle::{assert_failure, create_api, Gateway};

/// One `patchOperations` entry, as a client spells it on the wire.
fn op(kind: &str, path: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "op": kind, "path": path, "value": value }),
        None => json!({ "op": kind, "path": path }),
    }
}

/// The `UpdateGatewayResponse` URI for `response_type` on `api`.
fn uri(api: &str, response_type: &str) -> String {
    format!("/restapis/{api}/gatewayresponses/{response_type}")
}

/// Customize `response_type` first, asserting the modelled 201 of `PutGatewayResponse`,
/// so a row that patches an already-customized record starts from a known one.
async fn seed(gateway: &Gateway, api: &str, response_type: &str, request: Value) {
    let created = put_gateway_response(gateway, api, response_type, request).await;
    assert_eq!(
        created.status, 201,
        "case PutGatewayResponse({response_type}): expected 201, got {} with {:?}",
        created.status, created.body
    );
}

/// Assert `GetGatewayResponse` reports `expected` for `response_type`.
async fn assert_read_back(
    gateway: &Gateway,
    api: &str,
    response_type: &str,
    case: &str,
    expected: &Value,
) {
    let read = get_gateway_response(gateway, api, response_type).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from GetGatewayResponse, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        &read.body, expected,
        "case {case}: GetGatewayResponse must report the updated record {expected:?}, got {:?}",
        read.body
    );
}

/// One row of the patch table: the case name, the response type it patches, the
/// customization it starts from (none for a type still reporting the generated response),
/// the `patchOperations` it sends, and the whole record the service must report afterwards.
type Row = (
    &'static str,
    &'static str,
    Option<Value>,
    Vec<Value>,
    Value,
);

#[tokio::test]
async fn a_patched_gateway_response_is_stored_and_read_back_member_for_member() {
    let rows: Vec<Row> = vec![
        (
            "a template replaced on a customization",
            "INTEGRATION_TIMEOUT",
            Some(json!({
                "statusCode": "504",
                "responseTemplates": { "application/json": "{\"message\":\"timeout\"}" }
            })),
            vec![replace(
                "/responseTemplates/application~1json",
                "{\"message\":\"the back end timed out\"}",
            )],
            json!({
                "responseType": "INTEGRATION_TIMEOUT",
                "statusCode": "504",
                "responseTemplates": {
                    "application/json": "{\"message\":\"the back end timed out\"}"
                },
                "defaultResponse": false
            }),
        ),
        (
            "a status code replaced on a customization",
            "BAD_REQUEST_PARAMETERS",
            Some(json!({ "statusCode": "400" })),
            vec![replace("/statusCode", "422")],
            json!({
                "responseType": "BAD_REQUEST_PARAMETERS",
                "statusCode": "422",
                "defaultResponse": false
            }),
        ),
        (
            "a 1xx status code the modelled pattern admits",
            "WAF_FILTERED",
            None,
            vec![replace("/statusCode", "100")],
            json!({
                "responseType": "WAF_FILTERED",
                "statusCode": "100",
                "defaultResponse": false
            }),
        ),
        (
            "a 599 status code, the pattern's upper published bound",
            "RESOURCE_NOT_FOUND",
            None,
            vec![replace("/statusCode", "599")],
            json!({
                "responseType": "RESOURCE_NOT_FOUND",
                "statusCode": "599",
                "defaultResponse": false
            }),
        ),
        (
            "a template added under an escaped pointer key",
            "UNAUTHORIZED",
            Some(json!({ "statusCode": "401" })),
            vec![op(
                "add",
                "/responseTemplates/application~1json",
                Some("{\"message\":$context.error.messageString}"),
            )],
            json!({
                "responseType": "UNAUTHORIZED",
                "statusCode": "401",
                "responseTemplates": {
                    "application/json": "{\"message\":$context.error.messageString}"
                },
                "defaultResponse": false
            }),
        ),
        (
            "a parameter added on a type still reporting the generated response",
            "DEFAULT_4XX",
            None,
            vec![op(
                "add",
                "/responseParameters/gatewayresponse.header.Access-Control-Allow-Origin",
                Some("'*'"),
            )],
            json!({
                "responseType": "DEFAULT_4XX",
                "responseParameters": {
                    "gatewayresponse.header.Access-Control-Allow-Origin": "'*'"
                },
                "defaultResponse": false
            }),
        ),
        (
            "two ops over both map members in one request",
            "THROTTLED",
            None,
            vec![
                op(
                    "add",
                    "/responseParameters/gatewayresponse.header.Retry-After",
                    Some("'60'"),
                ),
                op(
                    "add",
                    "/responseTemplates/text~1plain",
                    Some("slow down"),
                ),
            ],
            json!({
                "responseType": "THROTTLED",
                "statusCode": generated_status_code("THROTTLED"),
                "responseParameters": { "gatewayresponse.header.Retry-After": "'60'" },
                "responseTemplates": { "text/plain": "slow down" },
                "defaultResponse": false
            }),
        ),
        (
            "two ops on one map entry, the later one winning",
            "QUOTA_EXCEEDED",
            None,
            vec![
                op("add", "/responseTemplates/text~1plain", Some("too many")),
                replace("/responseTemplates/text~1plain", "quota reached"),
            ],
            json!({
                "responseType": "QUOTA_EXCEEDED",
                "statusCode": generated_status_code("QUOTA_EXCEEDED"),
                "responseTemplates": { "text/plain": "quota reached" },
                "defaultResponse": false
            }),
        ),
        (
            "one entry of a map removed, the others kept",
            "REQUEST_TOO_LARGE",
            Some(json!({
                "statusCode": "413",
                "responseTemplates": {
                    "application/json": "{\"message\":\"too large\"}",
                    "text/plain": "too large"
                }
            })),
            vec![op("remove", "/responseTemplates/text~1plain", None)],
            json!({
                "responseType": "REQUEST_TOO_LARGE",
                "statusCode": "413",
                "responseTemplates": { "application/json": "{\"message\":\"too large\"}" },
                "defaultResponse": false
            }),
        ),
        (
            "the last entry of a map removed, which clears the member",
            "INVALID_API_KEY",
            Some(json!({
                "statusCode": "403",
                "responseParameters": { "gatewayresponse.header.X-Reason": "'key'" }
            })),
            vec![op(
                "remove",
                "/responseParameters/gatewayresponse.header.X-Reason",
                None,
            )],
            json!({
                "responseType": "INVALID_API_KEY",
                "statusCode": "403",
                "defaultResponse": false
            }),
        ),
        (
            "a map entry added on a type that generates no status code of its own",
            "DEFAULT_5XX",
            None,
            vec![op(
                "add",
                "/responseTemplates/application~1json",
                Some("{\"message\":$context.error.messageString}"),
            )],
            json!({
                "responseType": "DEFAULT_5XX",
                "responseTemplates": {
                    "application/json": "{\"message\":$context.error.messageString}"
                },
                "defaultResponse": false
            }),
        ),
        (
            "a parameter added on a customization that keeps its status code",
            "ACCESS_DENIED",
            Some(json!({ "statusCode": "402" })),
            vec![op(
                "add",
                "/responseParameters/gatewayresponse.header.X-Reason",
                Some("'denied'"),
            )],
            json!({
                "responseType": "ACCESS_DENIED",
                "statusCode": "402",
                "responseParameters": { "gatewayresponse.header.X-Reason": "'denied'" },
                "defaultResponse": false
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-gateway-response").await;

    for (case, response_type, customization, operations, expected) in &rows {
        if let Some(request) = customization {
            seed(&gateway, &api, response_type, request.clone()).await;
        }
        let updated = patch_ok(
            &gateway,
            case,
            &uri(&api, response_type),
            operations.clone(),
        )
        .await;
        assert_eq!(
            &updated, expected,
            "case {case}: expected the updated record {expected:?}, got {updated:?}"
        );
        assert_read_back(&gateway, &api, response_type, case, expected).await;

        // Every op of these rows is idempotent — a set to one value, or a drop of one key
        // — so sending the same request again reports the same record.
        let again = patch_ok(
            &gateway,
            case,
            &uri(&api, response_type),
            operations.clone(),
        )
        .await;
        assert_eq!(
            &again, expected,
            "case {case}: the same request again must report {expected:?}, got {again:?}"
        );
    }

    // Each row patched its own response type, and they all stand together: an update of
    // one type is not an update of another.
    for (case, response_type, _customization, _operations, expected) in &rows {
        assert_read_back(&gateway, &api, response_type, case, expected).await;
    }

    // A type no row touched is untouched by all of it.
    let untouched = get_gateway_response(&gateway, &api, "EXPIRED_TOKEN").await;
    assert_eq!(
        untouched.body,
        json!({
            "responseType": "EXPIRED_TOKEN",
            "statusCode": generated_status_code("EXPIRED_TOKEN"),
            "defaultResponse": true
        }),
        "case an untouched response type: expected the generated response, got {:?}",
        untouched.body
    );
}

#[tokio::test]
async fn a_request_that_asks_for_no_edit_leaves_the_record_as_it_was() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-gateway-response-no-ops").await;

    // Rows span the two states a response type can be in: still reporting the response
    // API Gateway generates, and carrying a customization.
    let generated = json!({
        "responseType": "MISSING_AUTHENTICATION_TOKEN",
        "statusCode": generated_status_code("MISSING_AUTHENTICATION_TOKEN"),
        "defaultResponse": true
    });
    seed(
        &gateway,
        &api,
        "BAD_REQUEST_BODY",
        json!({ "statusCode": "422" }),
    )
    .await;
    let customized = json!({
        "responseType": "BAD_REQUEST_BODY",
        "statusCode": "422",
        "defaultResponse": false
    });

    for (case, response_type, expected) in [
        (
            "an uncustomized type",
            "MISSING_AUTHENTICATION_TOKEN",
            &generated,
        ),
        ("a customized type", "BAD_REQUEST_BODY", &customized),
    ] {
        let answer = patch_ok(&gateway, case, &uri(&api, response_type), Vec::new()).await;
        assert_eq!(
            &answer, expected,
            "case {case}: expected the record as it was {expected:?}, got {answer:?}"
        );
        assert_read_back(&gateway, &api, response_type, case, expected).await;
    }
}

#[tokio::test]
async fn a_patch_this_resource_does_not_support_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-gateway-response-refusals").await;
    seed(
        &gateway,
        &api,
        "INTEGRATION_FAILURE",
        json!({
            "statusCode": "504",
            "responseTemplates": { "application/json": "{\"message\":\"failed\"}" }
        }),
    )
    .await;
    let stored = json!({
        "responseType": "INTEGRATION_FAILURE",
        "statusCode": "504",
        "responseTemplates": { "application/json": "{\"message\":\"failed\"}" },
        "defaultResponse": false
    });

    // Rows span the ways a patch can miss this resource: a path naming a member that is
    // not patchable (the record's identity, the service's own flag), a path naming no
    // member at all, a path reaching INTO a member that is not a map, a map path naming no
    // key, an op a member of this resource does not take, an add or replace carrying no
    // value, and a status code outside the modelled `^[1-5]\d\d$` pattern — one past each
    // published bound and the lengths and forms it does not admit.
    let mut refused: Vec<(String, Vec<Value>)> = vec![
        (
            "replace /responseType".to_string(),
            vec![replace("/responseType", "DEFAULT_4XX")],
        ),
        (
            "replace /defaultResponse".to_string(),
            vec![replace("/defaultResponse", "true")],
        ),
        (
            "replace /unknownMember".to_string(),
            vec![replace("/unknownMember", "x")],
        ),
        (
            "replace into the single-valued /statusCode".to_string(),
            vec![replace("/statusCode/extra", "503")],
        ),
        (
            "replace the whole /responseTemplates map".to_string(),
            vec![replace("/responseTemplates", "{}")],
        ),
        (
            "add the whole /responseParameters map".to_string(),
            vec![op("add", "/responseParameters", Some("'*'"))],
        ),
        (
            "replace /responseTemplates/application~1json with no value".to_string(),
            vec![op("replace", "/responseTemplates/application~1json", None)],
        ),
        (
            "a supported op after an unsupported one".to_string(),
            vec![
                op(
                    "add",
                    "/responseTemplates/text~1plain",
                    Some("never stored"),
                ),
                replace("/responseType", "DEFAULT_4XX"),
            ],
        ),
    ];
    // The status code takes a `replace` and no other op, whatever value rides along — one
    // the modelled `^[1-5]\d\d$` pattern admits as much as one it does not.
    for kind in ["add", "remove"] {
        for value in [Some("503"), Some("099"), None] {
            refused.push((
                format!("{kind} /statusCode with {value:?}"),
                vec![op(kind, "/statusCode", value)],
            ));
        }
    }
    for status in ["099", "600", "000", "999", "20", "2000", "abc", "5033"] {
        refused.push((
            format!("replace /statusCode with {status}"),
            vec![replace("/statusCode", status)],
        ));
    }
    for kind in ["move", "copy", "test"] {
        refused.push((
            format!("{kind} /statusCode"),
            vec![op(kind, "/statusCode", Some("503"))],
        ));
        refused.push((
            format!("{kind} /responseTemplates/application~1json"),
            vec![op(kind, "/responseTemplates/application~1json", Some("{}"))],
        ));
    }

    for (case, operations) in &refused {
        assert_failure(
            &patch(&gateway, &uri(&api, "INTEGRATION_FAILURE"), operations.clone()).await,
            400,
            "BadRequestException",
            case,
        );
        // Refused, so nothing of it was kept: the record is the one the put stored.
        assert_read_back(&gateway, &api, "INTEGRATION_FAILURE", case, &stored).await;
    }
}

#[tokio::test]
async fn an_unresolvable_identifier_is_refused_on_the_identifier_it_names() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "update-gateway-response-identifiers").await;
    let operations = vec![op(
        "add",
        "/responseTemplates/application~1json",
        Some("{\"message\":\"timeout\"}"),
    )];

    // An API id naming no API is a 404 on the identifier the URI resolves; the two
    // identifiers differ in kind, so each is refused on its own rule.
    for missing in ["zzzzzzzzzz", "not-an-api"] {
        assert_failure(
            &patch(&gateway, &uri(missing, "INTEGRATION_TIMEOUT"), operations.clone()).await,
            404,
            "NotFoundException",
            &format!("UpdateGatewayResponse(restApiId {missing})"),
        );
    }

    // A response type outside the modelled `GatewayResponseType` enum, in the ways a
    // label can miss it: a value shaped like the enum's but not in it, the right value in
    // the wrong case, an unknown word, and a declared value with a trailing character.
    // Each is refused by the CONSTRAINT layer in front of the handler, under its own
    // error class rather than the operation's `BadRequestException`.
    for response_type in [
        "DEFAULT_3XX",
        "integration_timeout",
        "NOT_A_RESPONSE_TYPE",
        "DEFAULT_4XX_",
    ] {
        assert_failure(
            &patch(&gateway, &uri(&api, response_type), operations.clone()).await,
            400,
            "ValidationException",
            &format!("UpdateGatewayResponse(responseType {response_type})"),
        );
    }

    // The gateway responses of a deleted API go with it.
    seed(
        &gateway,
        &api,
        "INTEGRATION_TIMEOUT",
        json!({ "statusCode": "504" }),
    )
    .await;
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &patch(&gateway, &uri(&api, "INTEGRATION_TIMEOUT"), operations).await,
        404,
        "NotFoundException",
        "UpdateGatewayResponse after DeleteRestApi",
    );
}
