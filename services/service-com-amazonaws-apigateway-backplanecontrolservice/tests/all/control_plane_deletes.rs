// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for three control-plane DELETE paths: `DeleteStage`,
//! `DeleteGatewayResponse` and `DeleteIntegrationResponse`, each read back through the
//! operation that reports the record it removed (`GetStage`, `GetGatewayResponse`,
//! `GetIntegrationResponse` and the owning `GetIntegration`).
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`CreateDeployment`/`PutIntegration`/
//! `PutGatewayResponse` helpers are this crate's existing harness, reused from
//! `rest_api_lifecycle`, `stage_lifecycle`, `method_integration`, `integration_response`
//! and `gateway_response`.
//!
//! The RULE under test is ONE rule in three places: a delete removes exactly the record
//! its identifiers name, so the read that reported that record stops reporting it while
//! every sibling record is untouched, and a delete whose identifiers name NO such record
//! answers the modelled `NotFoundException` (404) instead of succeeding — which is also
//! what makes a second delete of one record, or two concurrent ones, resolve to one
//! success and one 404 rather than to a conflict.
//!
//! The tables span what the rule generalises over rather than one recorded call: which
//! record is deleted (a table of stages differing in the members they carry, a table of
//! gateway response types, a table of integration-response status codes), the position
//! of the deleted record among its siblings, how many deletes are applied, and which
//! identifier of the request is the one that names nothing.

use serde_json::{json, Value};

use crate::gateway_response::{get_gateway_response, put_gateway_response};
use crate::integration_response::{get_integration_response, put_integration_response};
use crate::method_integration::{get_integration, put_integration, resource_with_a_method};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::{create_deployment, get_stage};

/// The status code the gateway-response rows customize their response type with, so a
/// cleared response is visibly not the customized one.
const CUSTOMIZED_STATUS: &str = "444";

/// `DeleteStage` for `stage_name` of `api`.
async fn delete_stage(gateway: &Gateway, api: &str, stage_name: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/stages/{stage_name}"),
            None,
        )
        .await
}

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

/// `DeleteIntegrationResponse` for `status` on the integration of the `verb` method of
/// `resource`.
async fn delete_integration_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!(
                "/restapis/{api}/resources/{resource}/methods/{verb}/integration/responses/{status}"
            ),
            None,
        )
        .await
}

/// `CreateStage` for `stage_name` on `deployment`, carrying whatever `extra` members the
/// row sets, asserting the modelled 201.
async fn create_stage(
    gateway: &Gateway,
    api: &str,
    deployment: &str,
    stage_name: &str,
    extra: &Value,
) {
    let mut request = json!({ "stageName": stage_name, "deploymentId": deployment });
    if let (Some(target), Some(members)) = (request.as_object_mut(), extra.as_object()) {
        for (member, value) in members {
            target.insert(member.clone(), value.clone());
        }
    }
    let answer = gateway
        .call("POST", &format!("/restapis/{api}/stages"), Some(request))
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateStage({stage_name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Assert a delete succeeded with the status its model declares and an empty body.
fn assert_deleted(answer: &Answer, status: u16, case: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body.is_null(),
        "case {case}: expected an empty body, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn deleting_a_stage_removes_that_stage_alone_and_a_second_delete_is_not_found() {
    // Rows differ along what the rule generalises over: which optional members the stage
    // carries — nothing conditions the removal, so a stage with a cache cluster, with
    // tags or with variables goes exactly as a bare one does — and the stage's position
    // among its siblings, since each row is deleted in turn while the rest stay.
    let rows: [(&str, &str, Value); 4] = [
        ("a bare stage", "bare", json!({})),
        (
            "a stage with a cache cluster",
            "cached",
            json!({ "cacheClusterEnabled": true, "cacheClusterSize": "0.5" }),
        ),
        (
            "a tagged and described stage",
            "tagged-1",
            json!({ "description": "the production stage", "tags": { "team": "api" } }),
        ),
        (
            "a stage with variables and tracing",
            "vars_2",
            json!({ "variables": { "table": "pets" }, "tracingEnabled": true }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-deletes").await;
    let deployment = create_deployment(&gateway, &api, "stage-deletes", json!({})).await;
    for (case, stage_name, extra) in &rows {
        create_stage(&gateway, &api, &deployment, stage_name, extra).await;
        let read = get_stage(&gateway, &api, stage_name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetStage before the delete, got {} with {:?}",
            read.status, read.body
        );
    }

    let mut remaining: Vec<&str> = rows.iter().map(|(_, stage_name, _)| *stage_name).collect();
    for (case, stage_name, _) in &rows {
        // The delete succeeds unconditionally for a stage that exists: the modelled 202
        // with an empty body, whatever the stage carries.
        assert_deleted(
            &delete_stage(&gateway, &api, stage_name).await,
            202,
            &format!("DeleteStage / {case}"),
        );
        // The record is gone: the read that reported it answers the modelled not-found.
        assert_failure(
            &get_stage(&gateway, &api, stage_name).await,
            404,
            "NotFoundException",
            &format!("GetStage after the delete / {case}"),
        );
        // A second delete of the SAME stage names no record, so it is the modelled
        // not-found rather than a second success or a conflict — the answer one of two
        // concurrent deletes of one stage gets.
        assert_failure(
            &delete_stage(&gateway, &api, stage_name).await,
            404,
            "NotFoundException",
            &format!("DeleteStage twice / {case}"),
        );
        // Every stage this delete did not name is untouched.
        remaining.retain(|name| name != stage_name);
        for name in &remaining {
            let read = get_stage(&gateway, &api, name).await;
            assert_eq!(
                read.status, 200,
                "case {case}: the sibling stage {name} must be untouched, \
                 got {} with {:?}",
                read.status, read.body
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_gateway_response_clears_that_customization_alone() {
    // Rows differ along the response type cleared: one of the two fallback types, which
    // carry no generated status code of their own, and two types that do.
    let rows: [(&str, &str); 3] = [
        ("a 4xx fallback", "DEFAULT_4XX"),
        ("an integration failure", "INTEGRATION_TIMEOUT"),
        ("an unauthorized response", "UNAUTHORIZED"),
    ];
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-deletes").await;
    for (case, response_type) in &rows {
        let answer = put_gateway_response(
            &gateway,
            &api,
            response_type,
            json!({ "statusCode": CUSTOMIZED_STATUS }),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201 from PutGatewayResponse, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let mut customized: Vec<&str> = rows.iter().map(|(_, kind)| *kind).collect();
    for (case, response_type) in &rows {
        assert_deleted(
            &delete_gateway_response(&gateway, &api, response_type).await,
            202,
            &format!("DeleteGatewayResponse / {case}"),
        );
        // The API still HAS a gateway response for the type — every modelled type always
        // does — and it is the one API Gateway generates: `defaultResponse` is true again
        // and the customized status code is gone.
        let read = get_gateway_response(&gateway, &api, response_type).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetGatewayResponse, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["defaultResponse"].as_bool(),
            Some(true),
            "case {case}: a cleared type must report the generated response, got {:?}",
            read.body
        );
        assert_ne!(
            read.body["statusCode"].as_str(),
            Some(CUSTOMIZED_STATUS),
            "case {case}: the customized status code must be gone, got {:?}",
            read.body
        );
        // Nothing is left to clear, so a second delete is the modelled not-found.
        assert_failure(
            &delete_gateway_response(&gateway, &api, response_type).await,
            404,
            "NotFoundException",
            &format!("DeleteGatewayResponse twice / {case}"),
        );
        // Every other customization is untouched.
        customized.retain(|kind| kind != response_type);
        for kind in &customized {
            let read = get_gateway_response(&gateway, &api, kind).await;
            assert_eq!(
                read.body["statusCode"].as_str(),
                Some(CUSTOMIZED_STATUS),
                "case {case}: the sibling customization of {kind} must be untouched, \
                 got {:?}",
                read.body
            );
            assert_eq!(
                read.body["defaultResponse"].as_bool(),
                Some(false),
                "case {case}: the sibling customization of {kind} must stay a \
                 customization, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn deleting_an_integration_response_removes_it_from_the_integration() {
    // Rows differ along the status code deleted, spanning the classes the modelled
    // `StatusCode` pattern admits and both ends of the stored collection's order.
    let rows: [(&str, &str); 4] = [
        ("the success code", "200"),
        ("a redirect", "302"),
        ("a client error", "404"),
        ("a server error", "500"),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-response-deletes").await;
    let resource =
        resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;
    let answer = put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "type": "MOCK" }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegration: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    for (case, status) in &rows {
        let answer =
            put_integration_response(&gateway, &api, &resource, "GET", status, json!({})).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201 from PutIntegrationResponse, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let mut remaining: Vec<&str> = rows.iter().map(|(_, status)| *status).collect();
    for (case, status) in &rows {
        // The modelled success for this operation is 204 with an empty body.
        assert_deleted(
            &delete_integration_response(&gateway, &api, &resource, "GET", status).await,
            204,
            &format!("DeleteIntegrationResponse / {case}"),
        );
        assert_failure(
            &get_integration_response(&gateway, &api, &resource, "GET", status).await,
            404,
            "NotFoundException",
            &format!("GetIntegrationResponse after the delete / {case}"),
        );
        assert_failure(
            &delete_integration_response(&gateway, &api, &resource, "GET", status).await,
            404,
            "NotFoundException",
            &format!("DeleteIntegrationResponse twice / {case}"),
        );
        // The owning integration stops reporting it too, and goes on reporting every
        // response this delete did not name.
        remaining.retain(|code| code != status);
        let integration = get_integration(&gateway, &api, &resource, "GET").await;
        assert!(
            integration.body["integrationResponses"][status].is_null(),
            "case {case}: the integration must stop reporting {status}, got {:?}",
            integration.body["integrationResponses"]
        );
        for code in &remaining {
            assert_eq!(
                integration.body["integrationResponses"][code]["statusCode"].as_str(),
                Some(*code),
                "case {case}: the integration must still report {code}, got {:?}",
                integration.body["integrationResponses"]
            );
        }
    }

    // With the last response gone the map goes with it, the way every caller-sized map
    // of this surface is reported only while it has an entry.
    let integration = get_integration(&gateway, &api, &resource, "GET").await;
    assert!(
        integration.body["integrationResponses"].is_null(),
        "case after every delete: expected no integrationResponses map, got {:?}",
        integration.body
    );
    // The method and its integration are untouched by the deletes: only the responses
    // went.
    assert_eq!(
        integration.body["type"].as_str(),
        Some("MOCK"),
        "case after every delete: the integration itself must be untouched, got {:?}",
        integration.body
    );
}

#[tokio::test]
async fn a_delete_whose_identifiers_name_no_record_is_the_modelled_not_found() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "delete-not-founds").await;
    let deployment = create_deployment(&gateway, &api, "delete-not-founds", json!({})).await;
    create_stage(&gateway, &api, &deployment, "prod", &json!({})).await;
    let resource =
        resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;
    let bare = resource_with_a_method(&gateway, &api, &root, "bare", "GET", json!({})).await;
    let answer = put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "type": "MOCK" }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegration: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    let put = put_integration_response(&gateway, &api, &resource, "GET", "200", json!({})).await;
    assert_eq!(
        put.status, 201,
        "case PutIntegrationResponse: expected 201, got {} with {:?}",
        put.status, put.body
    );
    // An identifier of the right SHAPE that no record of this account answers to.
    let missing = "zzzzz99999";

    // (case, method, uri) — every identifier each of the three operations resolves, one
    // row per identifier that can name nothing, so the rule is not keyed to one of them.
    let rows: Vec<(String, &str, String)> = vec![
        (
            "DeleteStage, an unknown api".to_string(),
            "DELETE",
            format!("/restapis/{missing}/stages/prod"),
        ),
        (
            "DeleteStage, an unknown stage of a stored api".to_string(),
            "DELETE",
            format!("/restapis/{api}/stages/{missing}"),
        ),
        (
            "DeleteGatewayResponse, an unknown api".to_string(),
            "DELETE",
            format!("/restapis/{missing}/gatewayresponses/DEFAULT_4XX"),
        ),
        (
            "DeleteGatewayResponse, a type this api never customized".to_string(),
            "DELETE",
            format!("/restapis/{api}/gatewayresponses/DEFAULT_5XX"),
        ),
        (
            "DeleteIntegrationResponse, an unknown api".to_string(),
            "DELETE",
            format!("/restapis/{missing}/resources/{resource}/methods/GET/integration/responses/200"),
        ),
        (
            "DeleteIntegrationResponse, an unknown resource".to_string(),
            "DELETE",
            format!("/restapis/{api}/resources/{missing}/methods/GET/integration/responses/200"),
        ),
        (
            "DeleteIntegrationResponse, a verb with no method".to_string(),
            "DELETE",
            format!("/restapis/{api}/resources/{resource}/methods/PUT/integration/responses/200"),
        ),
        (
            "DeleteIntegrationResponse, a method with no integration".to_string(),
            "DELETE",
            format!("/restapis/{api}/resources/{bare}/methods/GET/integration/responses/200"),
        ),
        (
            "DeleteIntegrationResponse, a status code with no response".to_string(),
            "DELETE",
            format!("/restapis/{api}/resources/{resource}/methods/GET/integration/responses/404"),
        ),
    ];

    for (case, method, uri) in &rows {
        assert_failure(
            &gateway.call(method, uri, None).await,
            404,
            "NotFoundException",
            case,
        );
    }

    // Every refused call removed nothing: the stage, the response type's generated
    // response and the stored integration response are all still as they were.
    let read = get_stage(&gateway, &api, "prod").await;
    assert_eq!(
        read.status, 200,
        "case after the refused deletes: expected 200 from GetStage, got {} with {:?}",
        read.status, read.body
    );
    let read = get_integration_response(&gateway, &api, &resource, "GET", "200").await;
    assert_eq!(
        read.status, 200,
        "case after the refused deletes: expected 200 from GetIntegrationResponse, \
         got {} with {:?}",
        read.status, read.body
    );
    let read = get_gateway_response(&gateway, &api, "DEFAULT_5XX").await;
    assert_eq!(
        string_member(&read.body, "responseType", "after the refused deletes"),
        "DEFAULT_5XX",
        "case after the refused deletes: expected the generated response, got {:?}",
        read.body
    );
}
