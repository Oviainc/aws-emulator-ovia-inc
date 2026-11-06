// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteGatewayResponse`, read back through the two operations
//! that report what it removed (`GetGatewayResponse` and `GetGatewayResponses`).
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi` helper, the `PutGatewayResponse`/`GetGatewayResponse` wrappers, the
//! generated-status-code catalog, the generated-record builder, the collection reader and
//! the PATCH wrappers are this crate's existing harness, reused from `rest_api_lifecycle`,
//! `gateway_response`, `gateway_response_scope`, `gateway_response_collection` and
//! `patch_updates`.
//!
//! The RULE under test: the call clears the developer's customization of the one
//! `(restApiId, responseType)` pair its URI names, so from then on both readers report the
//! response API Gateway generates for that type while every other pair keeps whatever it
//! held; a pair that holds no customization has nothing to clear and is the modelled
//! `NotFoundException` (404) instead of a success, and an identifier naming no API is the
//! same refusal.
//!
//! The tables span what the rule generalises over rather than one recorded call: EVERY
//! modelled `GatewayResponseType` (including the two whose generated response carries no
//! status code of its own), the status codes the modelled `^[1-5]\d\d$` pattern admits,
//! the presence and size of the two caller-supplied maps, how the customization was
//! created (a put or a patch), which API of several the delete addresses, the position of
//! the cleared pair among its siblings, and how many deletes are applied to one pair.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::GATEWAY_RESPONSE_TYPE_VALUES;

use crate::gateway_response::{get_gateway_response, put_gateway_response};
use crate::gateway_response_collection::collection_by_type;
use crate::gateway_response_scope::generated_response;
use crate::patch_updates::{patch, replace};
use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};

/// Status codes spanning the modelled `StatusCode` pattern `^[1-5]\d\d$`
/// (`smithy_query(query="shape", shape="com.amazonaws.apigateway#StatusCode")`): both
/// published bounds of the leading digit and values in between, so the rows do not all
/// carry one status class.
const STATUS_CODES: [&str; 5] = ["100", "202", "333", "444", "599"];

/// `DeleteGatewayResponse` for `response_type` on `api`.
async fn delete_gateway_response(gateway: &Gateway, api: &str, response_type: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/gatewayresponses/{response_type}"),
            None,
        )
        .await
}

/// The customization row number `index` sends: the status code rotates through
/// [`STATUS_CODES`] and the two caller-supplied maps rotate through absent, one entry and
/// several entries, so the table spans the bodies a customization can carry instead of
/// repeating one.
fn customization(index: usize) -> Value {
    let status = STATUS_CODES[index % STATUS_CODES.len()];
    match index % 3 {
        0 => json!({ "statusCode": status }),
        1 => json!({
            "statusCode": status,
            "responseParameters": { "gatewayresponse.header.X-Case": format!("'{index}'") }
        }),
        _ => json!({
            "statusCode": status,
            "responseParameters": {
                "gatewayresponse.header.X-Case": format!("'{index}'"),
                "gatewayresponse.header.X-Reason": "'customized'"
            },
            "responseTemplates": {
                "application/json": format!("{{\"case\":{index}}}"),
                "text/plain": format!("case {index}")
            }
        }),
    }
}

/// Customize `response_type` on `api`, asserting the modelled 201 and that the stored
/// record is reported as a developer's customization, and hand back that record.
async fn seed(gateway: &Gateway, api: &str, response_type: &str, request: Value) -> Value {
    let created = put_gateway_response(gateway, api, response_type, request).await;
    assert_eq!(
        created.status, 201,
        "case PutGatewayResponse({response_type}): expected 201, got {} with {:?}",
        created.status, created.body
    );
    assert_eq!(
        created.body["defaultResponse"].as_bool(),
        Some(false),
        "case PutGatewayResponse({response_type}): a put stores a customization, got {:?}",
        created.body
    );
    created.body
}

/// Assert the delete answered the status its model declares, with an empty body.
fn assert_cleared(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 202,
        "case {case}: expected 202, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body.is_null(),
        "case {case}: expected an empty body, got {:?}",
        answer.body
    );
}

/// Assert `GetGatewayResponse` reports `expected` for `response_type` on `api`.
async fn assert_reports(
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
        "case {case}: expected {expected:?}, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn clearing_a_customization_leaves_the_response_api_gateway_generates_for_that_type() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-clears").await;

    // The rows are the modelled enum's own values, so every response type is covered —
    // the two that generate no status code of their own included — and each one carries a
    // different customization body.
    let mut expected: BTreeMap<&str, Value> = BTreeMap::new();
    for (index, response_type) in GATEWAY_RESPONSE_TYPE_VALUES.iter().enumerate() {
        let record = seed(&gateway, &api, response_type, customization(index)).await;
        expected.insert(response_type, record);
    }

    for response_type in GATEWAY_RESPONSE_TYPE_VALUES {
        let case = format!("DeleteGatewayResponse({response_type})");
        assert_cleared(
            &delete_gateway_response(&gateway, &api, response_type).await,
            &case,
        );
        // The API still HAS a gateway response of this type — every modelled type always
        // does — and it is the one API Gateway generates, member for member: the flag, the
        // generated status code (absent where the type has none) and neither map.
        expected.insert(response_type, generated_response(response_type));
        assert_reports(
            &gateway,
            &api,
            response_type,
            &format!("{case}, then GetGatewayResponse"),
            &expected[response_type],
        )
        .await;

        // Nothing is left to clear, so a second delete of the same pair is the modelled
        // not-found rather than a second success — the answer one of two concurrent
        // deletes of one customization gets.
        assert_failure(
            &delete_gateway_response(&gateway, &api, response_type).await,
            404,
            "NotFoundException",
            &format!("{case} twice"),
        );

        // Both readers answer from one rule: the collection reports the cleared type as
        // generated and every type not yet deleted with the customization it still holds.
        let listed: BTreeMap<String, Value> = collection_by_type(&gateway, &api, &case)
            .await
            .into_iter()
            .collect();
        for (kind, record) in &expected {
            assert_eq!(
                listed.get(*kind),
                Some(record),
                "case {case}: the collection must report {record:?} for {kind}, got {:?}",
                listed.get(*kind)
            );
        }
    }
}

#[tokio::test]
async fn a_delete_clears_the_pair_its_uri_names_on_that_api_alone() {
    let gateway = Gateway::new();

    // Three APIs of one account, so the only thing separating their customizations is the
    // API the request addressed.
    let mut apis = Vec::new();
    for name in ["clear-first", "clear-second", "clear-third"] {
        let (id, _root) = create_api(&gateway, name).await;
        apis.push(id);
    }

    // The SAME response types customized on every API, with bodies that differ per API, so
    // a record surviving on one API cannot be another's. Rows span the position of the
    // deleted pair among its siblings: the API deleted from is the middle one and the type
    // deleted is one of several.
    let types = ["DEFAULT_4XX", "INTEGRATION_TIMEOUT", "QUOTA_EXCEEDED"];
    let mut expected: BTreeMap<(usize, &str), Value> = BTreeMap::new();
    for (api_index, api) in apis.iter().enumerate() {
        for (type_index, response_type) in types.iter().enumerate() {
            let record = seed(
                &gateway,
                api,
                response_type,
                customization(api_index * types.len() + type_index),
            )
            .await;
            expected.insert((api_index, response_type), record);
        }
    }

    // Each row deletes ONE pair, and only that pair changes.
    let deletes = [
        ("the middle API's fallback type", 1usize, "DEFAULT_4XX"),
        ("the middle API's integration type", 1, "INTEGRATION_TIMEOUT"),
        ("the first API's quota type", 0, "QUOTA_EXCEEDED"),
    ];
    for (case, api_index, response_type) in deletes {
        assert_cleared(
            &delete_gateway_response(&gateway, &apis[api_index], response_type).await,
            case,
        );
        expected.insert((api_index, response_type), generated_response(response_type));

        // Every modelled type on every API: the customization still held, or the generated
        // response where this pair was cleared or never customized at all.
        for (other_index, api) in apis.iter().enumerate() {
            for kind in GATEWAY_RESPONSE_TYPE_VALUES {
                let want = expected
                    .get(&(other_index, kind))
                    .cloned()
                    .unwrap_or_else(|| generated_response(kind));
                assert_reports(
                    &gateway,
                    api,
                    kind,
                    &format!("{case}, then api {other_index}/{kind}"),
                    &want,
                )
                .await;
            }
        }

        // The pair it cleared is the only one with nothing left to clear: the same delete
        // on the other APIs still names a customization and succeeds nowhere else yet.
        assert_failure(
            &delete_gateway_response(&gateway, &apis[api_index], response_type).await,
            404,
            "NotFoundException",
            &format!("{case} twice"),
        );
    }
}

#[tokio::test]
async fn a_customization_a_patch_created_is_cleared_the_same_way() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-patch-clears").await;

    // Rows differ along HOW the customization was created — a patch of a type that held
    // none, a patch over a put, and a put alone — because what the delete clears is the
    // customization, not the call that wrote it.
    let rows: Vec<(&str, &str, Option<Value>, Vec<Value>)> = vec![
        (
            "a patch of an uncustomized type",
            "UNAUTHORIZED",
            None,
            vec![json!({
                "op": "add",
                "path": "/responseParameters/gatewayresponse.header.X-Reason",
                "value": "'unauthorized'"
            })],
        ),
        (
            "a patch over a put",
            "INTEGRATION_FAILURE",
            Some(json!({ "statusCode": "504" })),
            vec![replace("/responseTemplates/application~1json", "{}")],
        ),
        (
            "a patch adding a map entry",
            "WAF_FILTERED",
            None,
            vec![json!({
                "op": "add",
                "path": "/responseTemplates/application~1json",
                "value": "{\"message\":\"filtered\"}"
            })],
        ),
    ];

    for (case, response_type, put_first, operations) in &rows {
        if let Some(request) = put_first {
            seed(&gateway, &api, response_type, request.clone()).await;
        }
        let patched = patch(
            &gateway,
            &format!("/restapis/{api}/gatewayresponses/{response_type}"),
            operations.clone(),
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected 200 from UpdateGatewayResponse, got {} with {:?}",
            patched.status, patched.body
        );
        assert_eq!(
            patched.body["defaultResponse"].as_bool(),
            Some(false),
            "case {case}: a patch leaves a customization, got {:?}",
            patched.body
        );

        assert_cleared(
            &delete_gateway_response(&gateway, &api, response_type).await,
            case,
        );
        assert_reports(
            &gateway,
            &api,
            response_type,
            &format!("{case}, then GetGatewayResponse"),
            &generated_response(response_type),
        )
        .await;
        assert_failure(
            &delete_gateway_response(&gateway, &api, response_type).await,
            404,
            "NotFoundException",
            &format!("{case} twice"),
        );
    }
}

#[tokio::test]
async fn a_delete_naming_no_customization_or_no_api_is_refused_and_clears_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-clear-refusals").await;
    let kept = "INTEGRATION_TIMEOUT";
    let record = seed(&gateway, &api, kept, json!({ "statusCode": "504" })).await;

    // A response type outside the modelled `GatewayResponseType` enum, in the ways a path
    // label can miss it: a value shaped like the enum's but not in it, the right value in
    // the wrong case, an unknown word, and a declared value with a trailing character.
    // Each is refused by the CONSTRAINT layer in front of the handler, whose error class
    // is `ValidationException` rather than the operation's own `BadRequestException`.
    for response_type in [
        "DEFAULT_3XX",
        "integration_timeout",
        "NOT_A_RESPONSE_TYPE",
        "DEFAULT_4XX_",
    ] {
        assert_failure(
            &delete_gateway_response(&gateway, &api, response_type).await,
            400,
            "ValidationException",
            &format!("DeleteGatewayResponse({response_type})"),
        );
    }

    // An API identifier naming no API, for a type this account has customized elsewhere
    // and for one nobody has: neither resolves, so both are the modelled not-found.
    for response_type in [kept, "ACCESS_DENIED"] {
        assert_failure(
            &delete_gateway_response(&gateway, "zzzzzzzzzz", response_type).await,
            404,
            "NotFoundException",
            &format!("DeleteGatewayResponse(an unknown api, {response_type})"),
        );
    }

    // A modelled type of this API that holds no customization: there is nothing to clear,
    // and the type keeps reporting the generated response afterwards.
    for response_type in ["DEFAULT_4XX", "DEFAULT_5XX", "ACCESS_DENIED"] {
        assert_failure(
            &delete_gateway_response(&gateway, &api, response_type).await,
            404,
            "NotFoundException",
            &format!("DeleteGatewayResponse({response_type}) with no customization"),
        );
        assert_reports(
            &gateway,
            &api,
            response_type,
            &format!("DeleteGatewayResponse({response_type}) refused"),
            &generated_response(response_type),
        )
        .await;
    }

    // Every refusal above cleared nothing: the one customization this API holds stands.
    assert_reports(
        &gateway,
        &api,
        kept,
        "after the refusals",
        &record,
    )
    .await;
}
