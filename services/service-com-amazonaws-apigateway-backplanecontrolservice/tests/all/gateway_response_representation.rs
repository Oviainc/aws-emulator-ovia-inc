// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REPRESENTATION of a `GatewayResponse` and for the one refusal
//! a patch of it earns by naming a response parameter it does not carry.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi` helper, the single-response helpers, the collection reader, the
//! generated-status-code catalog and the PATCH wrappers are this crate's existing harness,
//! reused from `rest_api_lifecycle`, `gateway_response`, `gateway_response_collection` and
//! `patch_updates`.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one call:
//!
//! * every representation of a gateway response — the answer of `PutGatewayResponse`,
//!   `UpdateGatewayResponse`, `GetGatewayResponse` and each item of `GetGatewayResponses` —
//!   carries both of its string-to-string maps: `responseParameters`, empty while the
//!   response has no parameter configured, and `responseTemplates`, carrying API Gateway's
//!   own body template for a response a developer has written no template of;
//! * `GetGatewayResponses` reports the collection in API Gateway's own order, whatever the
//!   API has been customized on;
//! * a response type a developer has not customized individually is reported as the class
//!   default standing in for it serves it (`DEFAULT_4XX` for a 4xx type, `DEFAULT_5XX` for
//!   a 5xx one): that stand-in's parameters, templates and, where it sets one, its status
//!   code — in the single read, in the collection entry, and in the answer of an update
//!   that applies no patch;
//! * a `patchOperations` `remove` naming a `responseParameters` entry the response does not
//!   carry removes nothing, so it is refused as a `NotFoundException` (404) instead of
//!   answering with the unchanged record.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::GATEWAY_RESPONSE_TYPE_VALUES;

use crate::gateway_response::{
    default_response_templates, generated_status_code, get_gateway_response, put_gateway_response,
};
use crate::gateway_response_collection::get_gateway_responses;
use crate::gateway_response_scope::class_default_of;
use crate::patch_updates::{op, patch, replace};
use crate::rest_api_lifecycle::{assert_failure, create_api, page_items, Answer, Gateway};

/// The response types of the gateway response collection, IN THE ORDER API Gateway reports
/// them in.
///
/// Source: the API Gateway REST API reference's `GatewayResponses` example, which lists the
/// whole collection of an API nothing has been customized on; the Smithy model carries the
/// type's values alphabetically and no order at all
/// (`smithy_query(query="shape", shape="com.amazonaws.apigateway#GatewayResponseType")`).
/// Written out here rather than read from the service, so the test checks the order instead
/// of restating it.
const COLLECTION_ORDER: [&str; 21] = [
    "INTEGRATION_FAILURE",
    "RESOURCE_NOT_FOUND",
    "REQUEST_TOO_LARGE",
    "THROTTLED",
    "UNSUPPORTED_MEDIA_TYPE",
    "AUTHORIZER_CONFIGURATION_ERROR",
    "DEFAULT_5XX",
    "DEFAULT_4XX",
    "BAD_REQUEST_PARAMETERS",
    "BAD_REQUEST_BODY",
    "WAF_FILTERED",
    "EXPIRED_TOKEN",
    "ACCESS_DENIED",
    "INVALID_API_KEY",
    "UNAUTHORIZED",
    "API_CONFIGURATION_ERROR",
    "QUOTA_EXCEEDED",
    "INTEGRATION_TIMEOUT",
    "MISSING_AUTHENTICATION_TOKEN",
    "INVALID_SIGNATURE",
    "AUTHORIZER_FAILURE",
];

/// The fixed sentence API Gateway refuses a `remove` of an unconfigured response parameter
/// with (`NotFoundException`).
const INVALID_PARAMETER_NAME: &str = "Invalid parameter name specified";

/// `UpdateGatewayResponse` for `response_type` on `api`.
async fn patch_gateway_response(
    gateway: &Gateway,
    api: &str,
    response_type: &str,
    operations: Vec<Value>,
) -> Answer {
    patch(
        gateway,
        &format!("/restapis/{api}/gatewayresponses/{response_type}"),
        operations,
    )
    .await
}

/// The collection of `api`, in the order it was reported.
async fn listed_types(gateway: &Gateway, api: &str, case: &str) -> Vec<String> {
    let listed = get_gateway_responses(gateway, api, "").await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected 200 from GetGatewayResponses, got {} with {:?}",
        listed.status, listed.body
    );
    page_items(&listed.body, case)
        .into_iter()
        .map(|item| {
            item["responseType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a responseType, got {item:?}"))
                .to_owned()
        })
        .collect()
}

/// The collection entry of `response_type`, which every type always has.
async fn listed_entry(gateway: &Gateway, api: &str, response_type: &str, case: &str) -> Value {
    let listed = get_gateway_responses(gateway, api, "").await;
    page_items(&listed.body, case)
        .into_iter()
        .find(|item| item["responseType"].as_str() == Some(response_type))
        .unwrap_or_else(|| panic!("case {case}: expected {response_type} in the collection"))
}

/// Assert `record` carries both maps with the values `parameters` and `templates`.
fn assert_maps(record: &Value, parameters: &Value, templates: &Value, case: &str) {
    assert_eq!(
        &record["responseParameters"], parameters,
        "case {case}: expected responseParameters {parameters:?}, got {:?}",
        record["responseParameters"]
    );
    assert_eq!(
        &record["responseTemplates"], templates,
        "case {case}: expected responseTemplates {templates:?}, got {:?}",
        record["responseTemplates"]
    );
}

#[tokio::test]
async fn every_representation_of_a_gateway_response_carries_both_of_its_maps() {
    // Rows differ along what the rule generalises over: which of the two maps the
    // customization configures (neither, one, the other, both), how many entries each
    // carries, which content type a template is written for, and the response type (values
    // from across the modelled enum, including one that generates no status code of its
    // own). Each row states the two maps EVERY representation of the stored response must
    // carry.
    let cases: Vec<(&str, &str, Value, Value, Value)> = vec![
        (
            "a customization that configures neither map",
            "INTEGRATION_TIMEOUT",
            json!({ "statusCode": "504" }),
            json!({}),
            default_response_templates(),
        ),
        (
            "a customization whose maps are present but empty",
            "DEFAULT_5XX",
            json!({ "responseParameters": {}, "responseTemplates": {} }),
            json!({}),
            default_response_templates(),
        ),
        (
            "a customization configuring one response parameter only",
            "ACCESS_DENIED",
            json!({
                "statusCode": "403",
                "responseParameters": { "gatewayresponse.header.X-Reason": "'denied'" }
            }),
            json!({ "gatewayresponse.header.X-Reason": "'denied'" }),
            default_response_templates(),
        ),
        (
            "a customization configuring one template only, for another content type",
            "QUOTA_EXCEEDED",
            json!({ "responseTemplates": { "text/plain": "too many" } }),
            json!({}),
            json!({ "text/plain": "too many" }),
        ),
        (
            "a customization configuring its own application/json template",
            "UNSUPPORTED_MEDIA_TYPE",
            json!({ "responseTemplates": { "application/json": "{\"media\":false}" } }),
            json!({}),
            json!({ "application/json": "{\"media\":false}" }),
        ),
        (
            "a customization configuring several entries of both maps",
            "THROTTLED",
            json!({
                "statusCode": "429",
                "responseParameters": {
                    "gatewayresponse.header.Retry-After": "'60'",
                    "gatewayresponse.headers.X-Reason": "'throttled'"
                },
                "responseTemplates": {
                    "application/json": "{\"message\":\"slow down\"}",
                    "text/plain": "slow down"
                }
            }),
            json!({
                "gatewayresponse.header.Retry-After": "'60'",
                "gatewayresponse.headers.X-Reason": "'throttled'"
            }),
            json!({
                "application/json": "{\"message\":\"slow down\"}",
                "text/plain": "slow down"
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-representation").await;

    for (case, response_type, request, parameters, templates) in &cases {
        // The answer of the write.
        let created = put_gateway_response(&gateway, &api, response_type, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutGatewayResponse, got {} with {:?}",
            created.status, created.body
        );
        assert_maps(
            &created.body,
            parameters,
            templates,
            &format!("{case}/PutGatewayResponse"),
        );

        // The answer of the single read, and of the collection entry: one record, three
        // representations of it.
        let read = get_gateway_response(&gateway, &api, response_type).await;
        assert_eq!(
            read.body, created.body,
            "case {case}: GetGatewayResponse must report the stored response, got {:?} after {:?}",
            read.body, created.body
        );
        let listed = listed_entry(&gateway, &api, response_type, case).await;
        assert_eq!(
            listed, created.body,
            "case {case}: the collection entry must report the stored response, got {listed:?} \
             after {:?}",
            created.body
        );

        // The answer of a patch that edits neither map: a read-modify-write of a gateway
        // response reads both maps back whatever it changed.
        let patched = patch_gateway_response(
            &gateway,
            &api,
            response_type,
            vec![replace("/statusCode", "502")],
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected 200 from UpdateGatewayResponse, got {} with {:?}",
            patched.status, patched.body
        );
        assert_maps(
            &patched.body,
            parameters,
            templates,
            &format!("{case}/UpdateGatewayResponse"),
        );
    }

    // Every type nobody customized is reported the same way: no parameter configured, and
    // API Gateway's own body template. The rows are the modelled enum's own values, so a
    // value no case above names is covered here.
    let (fresh, _fresh_root) = create_api(&gateway, "gateway-response-generated").await;
    for response_type in GATEWAY_RESPONSE_TYPE_VALUES {
        let read = get_gateway_response(&gateway, &fresh, response_type).await;
        assert_eq!(
            read.status, 200,
            "case {response_type}: expected 200 from GetGatewayResponse, got {} with {:?}",
            read.status, read.body
        );
        assert_maps(
            &read.body,
            &json!({}),
            &default_response_templates(),
            response_type,
        );
        let listed = listed_entry(&gateway, &fresh, response_type, response_type).await;
        assert_eq!(
            listed, read.body,
            "case {response_type}: the collection and the single read must agree, got \
             {listed:?} against {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_collection_is_reported_in_api_gateways_own_order() {
    // The order covers exactly the modelled response types: the same values, so neither
    // list can name one the other does not.
    let mut ordered = COLLECTION_ORDER.to_vec();
    ordered.sort_unstable();
    let mut modelled = GATEWAY_RESPONSE_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        ordered, modelled,
        "case the order: expected the modelled response types, got {COLLECTION_ORDER:?}"
    );

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-order").await;

    let fresh = listed_types(&gateway, &api, "a fresh API").await;
    assert_eq!(
        fresh, COLLECTION_ORDER,
        "case a fresh API: expected the collection in API Gateway's order, got {fresh:?}"
    );

    // Rows differ along WHERE in the collection the customized type sits — the first slot,
    // one in the middle, the last slot — and what the customization sets, because a
    // customization replaces the record in its own slot rather than moving it.
    let customizations: Vec<(&str, &str, Value)> = vec![
        (
            "the first slot",
            COLLECTION_ORDER[0],
            json!({ "statusCode": "500" }),
        ),
        (
            "a slot in the middle",
            COLLECTION_ORDER[COLLECTION_ORDER.len() / 2],
            json!({
                "statusCode": "418",
                "responseParameters": { "gatewayresponse.header.X-Slot": "'middle'" }
            }),
        ),
        (
            "the last slot",
            COLLECTION_ORDER[COLLECTION_ORDER.len() - 1],
            json!({ "responseTemplates": { "text/plain": "last" } }),
        ),
    ];
    for (case, response_type, request) in &customizations {
        let created = put_gateway_response(&gateway, &api, response_type, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutGatewayResponse, got {} with {:?}",
            created.status, created.body
        );
        let listed = listed_types(&gateway, &api, case).await;
        assert_eq!(
            listed, COLLECTION_ORDER,
            "case {case}: a customization must not move its slot, got {listed:?}"
        );
    }

    // Every slot reports the record of its own type, member for member: the customization
    // where one was put (with the status code it set), and the generated response — the
    // type's own status code, no parameter, API Gateway's body template — everywhere else.
    let listed = get_gateway_responses(&gateway, &api, "").await;
    let items = page_items(&listed.body, "the customized collection");
    for (index, response_type) in COLLECTION_ORDER.iter().enumerate() {
        let item = &items[index];
        let case = format!("slot {index} ({response_type})");
        assert_eq!(
            item["responseType"].as_str(),
            Some(*response_type),
            "case {case}: expected the type of this slot, got {item:?}"
        );
        match customizations
            .iter()
            .find(|(_, kind, _)| kind == response_type)
        {
            Some((_, _, request)) => {
                assert_eq!(
                    item["defaultResponse"].as_bool(),
                    Some(false),
                    "case {case}: a customized slot is no default response, got {item:?}"
                );
                let expected_status = request["statusCode"]
                    .as_str()
                    .or_else(|| generated_status_code(response_type));
                assert_eq!(
                    item["statusCode"].as_str(),
                    expected_status,
                    "case {case}: expected the status code {expected_status:?}, got {item:?}"
                );
            }
            None => {
                assert_eq!(
                    item["defaultResponse"].as_bool(),
                    Some(true),
                    "case {case}: an uncustomized slot is the default response, got {item:?}"
                );
                let expected_status = generated_status_code(response_type);
                assert_eq!(
                    item["statusCode"].as_str(),
                    expected_status,
                    "case {case}: expected the generated status code {expected_status:?}, got \
                     {item:?}"
                );
                assert_maps(item, &json!({}), &default_response_templates(), &case);
            }
        }
    }

    // Listing is a read: the order and the records after three listings are the ones
    // before them.
    for round in 0..3 {
        let again = listed_types(&gateway, &api, &format!("round {round}")).await;
        assert_eq!(
            again, COLLECTION_ORDER,
            "case round {round}: a read must not change the collection, got {again:?}"
        );
    }
}

#[tokio::test]
async fn an_uncustomized_type_is_reported_as_the_class_default_standing_in_for_it_serves_it() {
    // Rows differ along WHAT the class default's customization carries, because each member
    // it sets is one the types it stands in for report: response parameters only, a template
    // only, a status code only, and all three at once. The two classes are rows of their own,
    // so neither stands in for the other's types.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "the 4xx class default with response parameters only",
            "DEFAULT_4XX",
            json!({
                "responseParameters": {
                    "gatewayresponse.header.Access-Control-Allow-Origin": "'*'"
                }
            }),
        ),
        (
            "the 4xx class default with a template only",
            "DEFAULT_4XX",
            json!({ "responseTemplates": { "application/json": "{\"error\":\"client\"}" } }),
        ),
        (
            "the 4xx class default with a status code only",
            "DEFAULT_4XX",
            json!({ "statusCode": "444" }),
        ),
        (
            "the 5xx class default with a status code, parameters and a template",
            "DEFAULT_5XX",
            json!({
                "statusCode": "555",
                "responseParameters": { "gatewayresponse.header.X-Class": "'5xx'" },
                "responseTemplates": { "text/plain": "server" }
            }),
        ),
        (
            "the 5xx class default with a template only",
            "DEFAULT_5XX",
            json!({ "responseTemplates": { "text/plain": "boom" } }),
        ),
    ];

    for (case, class_default, request) in &rows {
        // A fresh API per row, so a row's stand-in is the only customization in play, plus
        // ONE type of the same class customized individually — the case a stand-in must not
        // override.
        let gateway = Gateway::new();
        let (api, _root) = create_api(&gateway, "gateway-response-class-default").await;
        let individual = match *class_default {
            "DEFAULT_4XX" => "THROTTLED",
            _ => "INTEGRATION_FAILURE",
        };
        let own = put_gateway_response(
            &gateway,
            &api,
            individual,
            json!({ "statusCode": "400", "responseTemplates": { "text/plain": "mine" } }),
        )
        .await;
        assert_eq!(
            own.status, 201,
            "case {case}: expected 201 from PutGatewayResponse({individual}), got {} with {:?}",
            own.status, own.body
        );

        let stand_in = put_gateway_response(&gateway, &api, class_default, request.clone()).await;
        assert_eq!(
            stand_in.status, 201,
            "case {case}: expected 201 from PutGatewayResponse({class_default}), got {} with {:?}",
            stand_in.status, stand_in.body
        );

        // What a type the stand-in serves must report, stated from the REQUEST: the status
        // code it set or the type's own, its parameters or none, its template or API
        // Gateway's own.
        let inherited_status = request["statusCode"].as_str();
        let inherited_parameters = request
            .get("responseParameters")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let inherited_templates = request
            .get("responseTemplates")
            .cloned()
            .unwrap_or_else(default_response_templates);

        for response_type in GATEWAY_RESPONSE_TYPE_VALUES {
            let case = format!("{case}/{response_type}");
            let read = get_gateway_response(&gateway, &api, response_type).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 from GetGatewayResponse, got {} with {:?}",
                read.status, read.body
            );
            // The collection answers from the same rule as the single read.
            let listed = listed_entry(&gateway, &api, response_type, &case).await;
            assert_eq!(
                listed, read.body,
                "case {case}: the collection and the single read must agree, got {listed:?} \
                 against {:?}",
                read.body
            );
            // So does an update that applies no patch: it changes nothing, so what it
            // answers is what a read of the type answers.
            let untouched = patch_gateway_response(&gateway, &api, response_type, Vec::new()).await;
            assert_eq!(
                untouched.status, 200,
                "case {case}: expected 200 from UpdateGatewayResponse with no ops, got {} with \
                 {:?}",
                untouched.status, untouched.body
            );
            assert_eq!(
                untouched.body, read.body,
                "case {case}: an update that applies no patch must answer the record a read \
                 reports, got {:?} against {:?}",
                untouched.body, read.body
            );

            if *response_type == *class_default || *response_type == individual {
                // A type with a customization of its own is reported as that record,
                // stand-in or no stand-in.
                let expected = match *response_type == individual {
                    true => &own.body,
                    false => &stand_in.body,
                };
                assert_eq!(
                    &read.body, expected,
                    "case {case}: expected the type's own customization {expected:?}, got {:?}",
                    read.body
                );
                continue;
            }

            assert_eq!(
                read.body["defaultResponse"].as_bool(),
                Some(true),
                "case {case}: a type with no customization of its own is the default \
                 response, got {:?}",
                read.body
            );
            let stood_in_for = class_default_of(response_type) == Some(*class_default);
            let expected_status = match stood_in_for && inherited_status.is_some() {
                true => inherited_status,
                false => generated_status_code(response_type),
            };
            assert_eq!(
                read.body["statusCode"].as_str(),
                expected_status,
                "case {case}: expected the status code {expected_status:?}, got {:?}",
                read.body
            );
            let (parameters, templates) = match stood_in_for {
                true => (inherited_parameters.clone(), inherited_templates.clone()),
                false => (json!({}), default_response_templates()),
            };
            assert_maps(&read.body, &parameters, &templates, &case);
        }

        // The stand-in is resolved when a response is READ, not written into the records of
        // the types it serves: clearing it leaves every one of them the response API
        // Gateway generates.
        let cleared = gateway
            .call(
                "DELETE",
                &format!("/restapis/{api}/gatewayresponses/{class_default}"),
                None,
            )
            .await;
        assert_eq!(
            cleared.status, 202,
            "case {case}: expected 202 from DeleteGatewayResponse, got {} with {:?}",
            cleared.status, cleared.body
        );
        for response_type in GATEWAY_RESPONSE_TYPE_VALUES {
            if *response_type == individual {
                continue;
            }
            let case = format!("{case}/{response_type} after clearing the stand-in");
            let read = get_gateway_response(&gateway, &api, response_type).await;
            assert_eq!(
                read.body["statusCode"].as_str(),
                generated_status_code(response_type),
                "case {case}: expected the type's own status code, got {:?}",
                read.body
            );
            assert_maps(
                &read.body,
                &json!({}),
                &default_response_templates(),
                &case,
            );
        }
    }
}

#[tokio::test]
async fn a_remove_of_a_response_parameter_the_response_does_not_carry_is_refused() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-remove-refusals").await;

    // One customization that DOES configure a parameter, so the refusals below can be told
    // apart from "this response has no parameters at all".
    let configured = "gatewayresponse.header.X-Reason";
    let seeded = put_gateway_response(
        &gateway,
        &api,
        "THROTTLED",
        json!({
            "statusCode": "429",
            "responseParameters": { configured: "'throttled'" }
        }),
    )
    .await;
    assert_eq!(
        seeded.status, 201,
        "case PutGatewayResponse(THROTTLED): expected 201, got {} with {:?}",
        seeded.status, seeded.body
    );

    // Rows differ along WHY the name names no configured parameter: the response carries no
    // parameter at all (a type still reporting the generated response, and a customization
    // that configured only a template), the response carries another parameter, the name is
    // the multi-valued form of a configured single-valued one, and the name differs only in
    // case from a configured one.
    let template_only = put_gateway_response(
        &gateway,
        &api,
        "DEFAULT_4XX",
        json!({ "responseTemplates": { "application/json": "{}" } }),
    )
    .await;
    assert_eq!(
        template_only.status, 201,
        "case PutGatewayResponse(DEFAULT_4XX): expected 201, got {} with {:?}",
        template_only.status, template_only.body
    );

    let refusals: Vec<(&str, &str, &str)> = vec![
        (
            "a type still reporting the generated response",
            "INTEGRATION_FAILURE",
            "gatewayresponse.header.X-Reason",
        ),
        (
            "a customization that configured only a template",
            "DEFAULT_4XX",
            "gatewayresponse.header.Access-Control-Allow-Origin",
        ),
        (
            "a name beside the one the response carries",
            "THROTTLED",
            "gatewayresponse.header.Retry-After",
        ),
        (
            "the multi-valued form of the configured name",
            "THROTTLED",
            "gatewayresponse.headers.X-Reason",
        ),
        (
            "the configured name in another case",
            "THROTTLED",
            "gatewayresponse.header.x-reason",
        ),
    ];

    for (case, response_type, name) in &refusals {
        let before = get_gateway_response(&gateway, &api, response_type).await;
        let answer = patch_gateway_response(
            &gateway,
            &api,
            response_type,
            vec![op("remove", &format!("/responseParameters/{name}"), None)],
        )
        .await;
        assert_failure(&answer, 404, "NotFoundException", case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(INVALID_PARAMETER_NAME),
            "case {case}: expected the fixed sentence {INVALID_PARAMETER_NAME:?}, got {:?}",
            answer.body["message"]
        );
        // Refused, so nothing of it was kept.
        let after = get_gateway_response(&gateway, &api, response_type).await;
        assert_eq!(
            after.body, before.body,
            "case {case}: a refused removal must leave the record {:?}, got {:?}",
            before.body, after.body
        );
    }

    // A later op of one request is refused the same way, and the op before it is not kept:
    // the whole request is refused.
    let refused_list = patch_gateway_response(
        &gateway,
        &api,
        "THROTTLED",
        vec![
            op(
                "add",
                "/responseParameters/gatewayresponse.header.X-Added",
                Some("'added'"),
            ),
            op(
                "remove",
                "/responseParameters/gatewayresponse.header.Retry-After",
                None,
            ),
        ],
    )
    .await;
    assert_failure(
        &refused_list,
        404,
        "NotFoundException",
        "an unconfigured removal after an applicable add",
    );
    let unchanged = get_gateway_response(&gateway, &api, "THROTTLED").await;
    assert_eq!(
        unchanged.body, seeded.body,
        "case an unconfigured removal after an applicable add: expected the record {:?}, got {:?}",
        seeded.body, unchanged.body
    );

    // The other side of the rule: a `remove` naming a parameter the response DOES carry
    // takes it away, and the member is then reported empty — which is what makes the
    // SECOND removal of that name a refusal.
    let removed = patch_gateway_response(
        &gateway,
        &api,
        "THROTTLED",
        vec![op("remove", &format!("/responseParameters/{configured}"), None)],
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "case removing the configured parameter: expected 200, got {} with {:?}",
        removed.status, removed.body
    );
    assert_eq!(
        removed.body["responseParameters"],
        json!({}),
        "case removing the configured parameter: expected an empty map, got {:?}",
        removed.body["responseParameters"]
    );
    let answer = patch_gateway_response(
        &gateway,
        &api,
        "THROTTLED",
        vec![op("remove", &format!("/responseParameters/{configured}"), None)],
    )
    .await;
    assert_failure(
        &answer,
        404,
        "NotFoundException",
        "removing the same parameter twice",
    );
}
