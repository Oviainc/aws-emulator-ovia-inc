// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning the SCOPE of a gateway response: the record
//! `GetGatewayResponse` reports is resolved from the pair the request URI names —
//! `(restApiId, responseType)` — so a customization put on one API is reported for that
//! API alone, and every other API still reports the response API Gateway generates for
//! the type.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi` helper, the single-response helpers and the generated-status-code
//! catalog are this crate's existing harness, reused from `rest_api_lifecycle` and
//! `gateway_response`; the sibling suites cover the customize/read round trip of ONE API
//! and the whole collection of one API, and this one covers the scoping between APIs that
//! neither can see.
//!
//! The tests assert the rule over a SET of inputs: a table of customizations spanning
//! several APIs, several response types (including the types whose generated response
//! carries no status code), several status codes the modelled `^[1-5]\d\d$` pattern
//! admits and both caller-supplied maps, then EVERY modelled response type read back on
//! EVERY API — and the same rule across the deletion of one of those APIs.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::GATEWAY_RESPONSE_TYPE_VALUES;

use crate::gateway_response::{
    default_response_templates, generated_status_code, get_gateway_response, put_gateway_response,
};
use crate::rest_api_lifecycle::{assert_failure, create_api, Gateway};

/// The record an UNCUSTOMIZED `response_type` is reported as: the type, the flag that
/// says API Gateway generated it, the generated status code where the type has one
/// (`DEFAULT_4XX` and `DEFAULT_5XX` have none), no configured response parameter, and the
/// body template API Gateway generates the response's body from.
///
/// Shared with the sibling `delete_gateway_response` suite, which asserts a cleared
/// customization leaves exactly this record behind.
pub(crate) fn generated_response(response_type: &str) -> Value {
    let mut record = Map::new();
    record.insert("responseType".to_string(), json!(response_type));
    record.insert("defaultResponse".to_string(), json!(true));
    if let Some(status) = generated_status_code(response_type) {
        record.insert("statusCode".to_string(), json!(status));
    }
    record.insert("responseParameters".to_string(), json!({}));
    record.insert(
        "responseTemplates".to_string(),
        default_response_templates(),
    );
    Value::Object(record)
}

/// Which class default stands in for `response_type`: `DEFAULT_4XX` for a response type
/// whose own status code is in the 4xx class, `DEFAULT_5XX` for one in the 5xx class, and
/// none for the two class defaults themselves, which have no status code of their own.
///
/// Source: the API Gateway Developer Guide's gateway response table, where those two rows
/// stand for "all other 4xx" and "all other 5xx" responses rather than for one condition.
pub(crate) fn class_default_of(response_type: &str) -> Option<&'static str> {
    match generated_status_code(response_type)?.chars().next()? {
        '4' => Some("DEFAULT_4XX"),
        '5' => Some("DEFAULT_5XX"),
        _ => None,
    }
}

/// The record an uncustomized `response_type` is reported as on an API whose CLASS DEFAULT
/// for that type's status class carries `class_default` — the record a put or patch of
/// `DEFAULT_4XX`/`DEFAULT_5XX` answered with — and [`generated_response`] where that class
/// default carries no customization of its own.
///
/// A response type a developer has not customized individually is served by the class
/// default standing in for it, so that stand-in's parameters, templates and, where it sets
/// one, its status code are what the type reports; the type itself is still uncustomized,
/// so the record is the one API Gateway generates (`defaultResponse: true`).
///
/// Shared with the sibling suites that customize a class default and then read the types it
/// stands in for.
pub(crate) fn effective_response(response_type: &str, class_default: Option<&Value>) -> Value {
    let mut record = generated_response(response_type);
    if let (Some(class_default), Some(object)) = (class_default, record.as_object_mut()) {
        for member in ["responseParameters", "responseTemplates"] {
            object.insert(member.to_string(), class_default[member].clone());
        }
        if let Some(status) = class_default.get("statusCode") {
            object.insert("statusCode".to_string(), status.clone());
        }
    }
    record
}

#[tokio::test]
async fn a_gateway_response_is_reported_for_the_api_it_was_put_on_and_no_other() {
    let gateway = Gateway::new();

    // Three APIs of the same account, so the only thing separating their gateway
    // responses is the API the put addressed.
    let mut apis = Vec::new();
    for name in ["scope-first", "scope-second", "scope-third"] {
        let (id, _root) = create_api(&gateway, name).await;
        apis.push(id);
    }

    // Rows differ along what the rule generalises over: which API the customization is
    // put on (one of them is left with no customization at all), the response type, the
    // status code, and which of the two caller-supplied maps the request carries. Two
    // rows customize the SAME response type on two different APIs with different bodies,
    // which is the case a per-type-only lookup would get wrong.
    let puts: Vec<(&str, usize, &str, Value)> = vec![
        (
            "the first API's INTEGRATION_TIMEOUT",
            0,
            "INTEGRATION_TIMEOUT",
            json!({
                "statusCode": "504",
                "responseTemplates": { "application/json": "{\"message\":\"first\"}" }
            }),
        ),
        (
            "the second API's INTEGRATION_TIMEOUT",
            1,
            "INTEGRATION_TIMEOUT",
            json!({
                "statusCode": "503",
                "responseParameters": { "gatewayresponse.header.Retry-After": "'30'" }
            }),
        ),
        (
            "the first API's DEFAULT_4XX, a type with no generated status code",
            0,
            "DEFAULT_4XX",
            json!({ "statusCode": "418" }),
        ),
        (
            "the second API's DEFAULT_5XX, which keeps no status code of its own",
            1,
            "DEFAULT_5XX",
            json!({ "responseTemplates": { "text/plain": "boom" } }),
        ),
        (
            "the second API's THROTTLED, both maps with several entries",
            1,
            "THROTTLED",
            json!({
                "statusCode": "429",
                "responseParameters": {
                    "gatewayresponse.header.Retry-After": "'60'",
                    "gatewayresponse.header.X-Reason": "'throttled'"
                },
                "responseTemplates": {
                    "application/json": "{\"message\":\"slow down\"}",
                    "text/plain": "slow down"
                }
            }),
        ),
        (
            "the first API's UNAUTHORIZED, a 1xx status code the pattern admits",
            0,
            "UNAUTHORIZED",
            json!({ "statusCode": "100" }),
        ),
    ];

    // What each (API, response type) pair must report afterwards, taken from the answer
    // the put itself gave: a put and a read of one pair are two views of one record.
    let mut customized: BTreeMap<(usize, &str), Value> = BTreeMap::new();
    for (case, api_index, response_type, request) in &puts {
        let created =
            put_gateway_response(&gateway, &apis[*api_index], response_type, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutGatewayResponse, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.body["defaultResponse"].as_bool(),
            Some(false),
            "case {case}: a put stores a customization, got {:?}",
            created.body
        );
        customized.insert((*api_index, response_type), created.body);
    }

    // Every modelled response type on every API: the customization where this API has
    // one, and otherwise the response the API serves for that type — the class default
    // standing in for it where this API customized that, and the generated response
    // everywhere else.
    for (api_index, api) in apis.iter().enumerate() {
        for response_type in GATEWAY_RESPONSE_TYPE_VALUES {
            let case = format!("api {api_index}/{response_type}");
            let expected = customized
                .get(&(api_index, response_type))
                .cloned()
                .unwrap_or_else(|| {
                    effective_response(
                        response_type,
                        class_default_of(response_type)
                            .and_then(|kind| customized.get(&(api_index, kind))),
                    )
                });
            let read = get_gateway_response(&gateway, api, response_type).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 from GetGatewayResponse, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body, expected,
                "case {case}: expected {expected:?}, got {:?}",
                read.body
            );
        }
    }

    // The third API was never put on, so every one of its types is still the generated
    // response — stated separately from the loop above so the "no customization leaked
    // here" case is named.
    let untouched = &apis[2];
    for response_type in GATEWAY_RESPONSE_TYPE_VALUES {
        let read = get_gateway_response(&gateway, untouched, response_type).await;
        assert_eq!(
            read.body,
            generated_response(response_type),
            "case the uncustomized API/{response_type}: expected the generated response, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn deleting_one_api_leaves_every_other_apis_gateway_responses_standing() {
    let gateway = Gateway::new();
    let (doomed, _doomed_root) = create_api(&gateway, "scope-deleted").await;
    let (kept, _kept_root) = create_api(&gateway, "scope-kept").await;

    // The same two response types customized on both APIs, with different bodies, so a
    // record surviving on the kept API cannot be the deleted API's.
    let bodies: Vec<(&str, Value, Value)> = vec![
        (
            "INTEGRATION_FAILURE",
            json!({ "statusCode": "504", "responseTemplates": { "application/json": "{\"a\":1}" } }),
            json!({ "statusCode": "502", "responseTemplates": { "application/json": "{\"a\":2}" } }),
        ),
        (
            "QUOTA_EXCEEDED",
            json!({ "statusCode": "429" }),
            json!({
                "statusCode": "403",
                "responseParameters": { "gatewayresponse.header.X-Quota": "'gone'" }
            }),
        ),
    ];

    let mut kept_records: Vec<(&str, Value)> = Vec::new();
    for (response_type, doomed_body, kept_body) in &bodies {
        for (api, request, which) in [
            (&doomed, doomed_body, "the API to delete"),
            (&kept, kept_body, "the API to keep"),
        ] {
            let created = put_gateway_response(&gateway, api, response_type, request.clone()).await;
            assert_eq!(
                created.status, 201,
                "case {which}/{response_type}: expected 201 from PutGatewayResponse, got {} with \
                 {:?}",
                created.status, created.body
            );
            if which == "the API to keep" {
                kept_records.push((response_type, created.body));
            }
        }
    }

    let deleted = gateway
        .call("DELETE", &format!("/restapis/{doomed}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );

    // The deleted API has no gateway responses at all — not even the generated ones,
    // because it has no API.
    for response_type in ["INTEGRATION_FAILURE", "QUOTA_EXCEEDED", "ACCESS_DENIED"] {
        assert_failure(
            &get_gateway_response(&gateway, &doomed, response_type).await,
            404,
            "NotFoundException",
            &format!("GetGatewayResponse(deleted API, {response_type})"),
        );
    }

    // The kept API reports exactly what it was put on, and the generated response for
    // every type it was not.
    for response_type in GATEWAY_RESPONSE_TYPE_VALUES {
        let expected = kept_records
            .iter()
            .find(|(kind, _)| kind == response_type)
            .map(|(_, record)| record.clone())
            .unwrap_or_else(|| generated_response(response_type));
        let read = get_gateway_response(&gateway, &kept, response_type).await;
        assert_eq!(
            read.status, 200,
            "case the kept API/{response_type}: expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, expected,
            "case the kept API/{response_type}: expected {expected:?}, got {:?}",
            read.body
        );
    }
}
