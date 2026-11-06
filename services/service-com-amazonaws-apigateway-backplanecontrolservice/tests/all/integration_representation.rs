// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an `Integration` and an `IntegrationResponse` REPORT, in
//! every reading of them this service serves.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi`/`CreateResource`/`PutMethod`/`PutIntegration` helpers, the PATCH
//! wrappers and the definition builders are this crate's existing harness, reused from
//! `rest_api_lifecycle`, `method_integration`, `integration_response`, `patch_updates`,
//! `rest_api_import` and `rest_api_import_integrations`.
//!
//! Three RULES, each asserted over a set of inputs rather than one call:
//!
//! * every `Integration` reports a `responseTransferMode` — the one it was stored with, or
//!   the `BUFFERED` the service fills in — wherever an `Integration` is embedded: the
//!   integration's own operations, the `Method` that owns it, and the `Resource` tree the
//!   method hangs off;
//! * `connectionType` names "the type of the network connection to the integration
//!   endpoint", and an integration built from an imported definition reports the one its
//!   own extension declares and none when it declares none — while the member's documented
//!   default (INTERNET) is what the CALL that sets an integration up fills in;
//! * a mapping template whose body is empty is no template, so the content type it is
//!   keyed by is reported with a null value rather than an empty string.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_integration::{
    DEFAULT_CONNECTION_TYPE, DEFAULT_RESPONSE_TRANSFER_MODE,
};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::RESPONSE_TRANSFER_MODE_VALUES;

use crate::execute_api_data_plane::deploy;
use crate::integration_response::{
    get_integration_response, put_integration_response, resource_with_an_integration,
};
use crate::method_integration::{
    aws_integration_uri, get_integration, get_method, put_integration, resource_with_a_method,
};
use crate::patch_updates::{op, patch_ok, replace};
use crate::rest_api_import::put_definition;
use crate::rest_api_import_integrations::{json_definition, json_operation, resource_at};
use crate::rest_api_lifecycle::{create_api, page_items, Gateway};
use crate::stage_export::export;

/// The content type a mapping template is keyed by in most of these cases.
const JSON: &str = "application/json";

/// One row of the import table: the case, the path the definition declares, the verb it
/// declares it under, the integration extension itself, the `connectionType` the imported
/// integration must report (`None` for the member reported at all) and the transfer mode
/// it must report.
type ImportedIntegrationRow<'a> = (
    &'a str,
    &'a str,
    &'a str,
    String,
    Option<&'a str>,
    &'a str,
);

/// One row of the template table: the case, the resource's path part, the method's verb,
/// the status code the integration response is keyed by, the request, and the entries the
/// reported `responseTemplates` must hold — `None` for the null an empty template is
/// reported as.
type TemplateRow<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    Value,
    Vec<(&'a str, Option<&'a str>)>,
);

/// The `Integration` URI of one method, which both the integration's own operations and
/// their PATCH are addressed at.
fn integration_uri(api: &str, resource: &str, verb: &str) -> String {
    format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration")
}

/// The `IntegrationResponse` URI of one status code of one method's integration.
fn integration_response_uri(api: &str, resource: &str, verb: &str, status: &str) -> String {
    format!("{}/responses/{status}", integration_uri(api, resource, verb))
}

/// The `methodIntegration` the resource TREE reports for the `verb` method of `resource`:
/// `GetResources` with `embed=methods`, which is the reading an SDK client compares a
/// whole API against.
async fn embedded_integration(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    case: &str,
) -> Value {
    let answer = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources?embed=methods"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetResources, got {} with {:?}",
        answer.status, answer.body
    );
    let items = page_items(&answer.body, case);
    let item = items
        .iter()
        .find(|item| item["id"].as_str() == Some(resource))
        .unwrap_or_else(|| panic!("case {case}: expected the resource {resource} in {items:?}"));
    item["resourceMethods"][verb]["methodIntegration"].clone()
}

#[tokio::test]
async fn an_integration_reports_its_transfer_mode_in_every_reading_of_it() {
    // Rows differ along what the rule generalises over: whether the request asks for a
    // transfer mode and which of the modelled values it asks for, and the integration
    // type the mode travels on.
    let cases: Vec<(&str, &str, &str, Value, &str)> = vec![
        (
            "a mock integration that asks for no transfer mode",
            "mock",
            "GET",
            json!({ "type": "MOCK" }),
            DEFAULT_RESPONSE_TRANSFER_MODE,
        ),
        (
            "a mock integration that asks to buffer",
            "mock-buffered",
            "POST",
            json!({ "type": "MOCK", "responseTransferMode": "BUFFERED" }),
            "BUFFERED",
        ),
        (
            "an http integration that asks to stream",
            "http-stream",
            "PUT",
            json!({
                "type": "HTTP",
                "httpMethod": "GET",
                "uri": "https://example.com/pets",
                "responseTransferMode": "STREAM"
            }),
            "STREAM",
        ),
        (
            "a lambda proxy integration that asks for no transfer mode",
            "lambda",
            "ANY",
            json!({
                "type": "AWS_PROXY",
                "httpMethod": "POST",
                "uri": aws_integration_uri(
                    "lambda:path/2015-03-31/functions/arn:aws:lambda:us-east-1:123456789012:function:pets/invocations"
                )
            }),
            DEFAULT_RESPONSE_TRANSFER_MODE,
        ),
        (
            "an http proxy integration that asks to stream",
            "http-proxy-stream",
            "HEAD",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "HEAD",
                "uri": "http://example.com/pets",
                "responseTransferMode": "STREAM"
            }),
            "STREAM",
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-transfer-mode").await;

    for (case, path_part, verb, request, expected) in &cases {
        let resource =
            resource_with_a_method(&gateway, &api, &root, path_part, verb, json!({})).await;
        let created = put_integration(&gateway, &api, &resource, verb, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutIntegration, got {} with {:?}",
            created.status, created.body
        );

        // Every reading of the stored record reports the same transfer mode: the
        // integration's own, the method that owns it, and the resource tree.
        let readings: Vec<(&str, Value)> = vec![
            ("PutIntegration", created.body.clone()),
            (
                "GetIntegration",
                get_integration(&gateway, &api, &resource, verb).await.body,
            ),
            (
                "GetMethod",
                get_method(&gateway, &api, &resource, verb).await.body["methodIntegration"].clone(),
            ),
            (
                "GetResources(embed=methods)",
                embedded_integration(&gateway, &api, &resource, verb, case).await,
            ),
            (
                "UpdateIntegration of another member",
                patch_ok(
                    &gateway,
                    case,
                    &integration_uri(&api, &resource, verb),
                    vec![replace("/timeoutInMillis", "1000")],
                )
                .await,
            ),
        ];
        for (reading, integration) in &readings {
            assert_eq!(
                integration["responseTransferMode"].as_str(),
                Some(*expected),
                "case {case} ({reading}): expected the transfer mode {expected:?}, got {:?}",
                integration["responseTransferMode"]
            );
        }
    }
}

#[tokio::test]
async fn a_patched_transfer_mode_takes_every_declared_value_and_clearing_it_restores_the_default() {
    // The value set is the modelled `ResponseTransferMode` enum's own
    // (`smithy_query(query="shape",shape="ResponseTransferMode")`), reached through the
    // generated constant rather than written out, so a value added to the model is
    // covered here too.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-transfer-mode-patch").await;
    assert!(
        RESPONSE_TRANSFER_MODE_VALUES.contains(&DEFAULT_RESPONSE_TRANSFER_MODE),
        "case the service's own transfer mode: expected one of the modelled values {RESPONSE_TRANSFER_MODE_VALUES:?}, got {DEFAULT_RESPONSE_TRANSFER_MODE:?}"
    );
    let resource = resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;
    let created = put_integration(&gateway, &api, &resource, "GET", json!({ "type": "MOCK" })).await;
    assert_eq!(
        created.status, 201,
        "case the seeded integration: expected 201, got {} with {:?}",
        created.status, created.body
    );

    for mode in RESPONSE_TRANSFER_MODE_VALUES {
        let patched = patch_ok(
            &gateway,
            mode,
            &integration_uri(&api, &resource, "GET"),
            vec![replace("/responseTransferMode", mode)],
        )
        .await;
        assert_eq!(
            patched["responseTransferMode"].as_str(),
            Some(*mode),
            "case replace the transfer mode with {mode}: expected it reported, got {patched:?}"
        );
        let read = get_integration(&gateway, &api, &resource, "GET").await;
        assert_eq!(
            read.body["responseTransferMode"].as_str(),
            Some(*mode),
            "case GetIntegration after {mode}: expected the stored mode, got {:?}",
            read.body
        );
    }

    // A member the service reports a value for cannot be left unreported: clearing it
    // puts the service's own value back.
    let cleared = patch_ok(
        &gateway,
        "remove the transfer mode",
        &integration_uri(&api, &resource, "GET"),
        vec![op("remove", "/responseTransferMode", None)],
    )
    .await;
    assert_eq!(
        cleared["responseTransferMode"].as_str(),
        Some(DEFAULT_RESPONSE_TRANSFER_MODE),
        "case remove the transfer mode: expected the service's own value back, got {cleared:?}"
    );
}

#[tokio::test]
async fn an_imported_integration_reports_the_connection_type_its_definition_declares() {
    // Rows differ along what the rule generalises over: whether the extension declares a
    // connection type and which of the modelled `ConnectionType` values it declares, and
    // the integration type it declares it on. `None` is the expectation that the member
    // is reported at all.
    let cases: Vec<ImportedIntegrationRow> = vec![
        (
            "an extension declaring no connection type",
            "/mock",
            "GET",
            json_operation("{\"type\": \"mock\"}"),
            None,
            DEFAULT_RESPONSE_TRANSFER_MODE,
        ),
        (
            "an extension declaring the public internet connection",
            "/internet",
            "GET",
            json_operation(
                "{\"type\": \"http\", \"httpMethod\": \"get\", \
                  \"uri\": \"https://example.com/pets\", \"connectionType\": \"internet\"}",
            ),
            Some("INTERNET"),
            DEFAULT_RESPONSE_TRANSFER_MODE,
        ),
        (
            "an extension declaring a private connection through a vpc link",
            "/private",
            "GET",
            json_operation(
                "{\"type\": \"http_proxy\", \"httpMethod\": \"get\", \
                  \"uri\": \"nlb-pets.elb.us-east-1.amazonaws.com\", \
                  \"connectionType\": \"vpc_link\", \"connectionId\": \"vpce01234\"}",
            ),
            Some("VPC_LINK"),
            DEFAULT_RESPONSE_TRANSFER_MODE,
        ),
        (
            "an extension declaring a transfer mode and no connection type",
            "/stream",
            "GET",
            json_operation(
                "{\"type\": \"mock\", \"responseTransferMode\": \"STREAM\"}",
            ),
            None,
            "STREAM",
        ),
    ];

    let gateway = Gateway::new();

    for (case, path, verb, operation, expected_connection, expected_mode) in &cases {
        // One API per row, so no row's expectation rests on another's import.
        let (api, _root) = create_api(&gateway, &format!("imported{}", path.replace('/', "-")))
            .await;
        let imported = put_definition(
            &gateway,
            &api,
            Some("overwrite"),
            false,
            &json_definition(case, path, &verb.to_ascii_lowercase(), operation),
        )
        .await;
        assert_eq!(
            imported.status, 200,
            "case {case}: expected 200 from PutRestApi, got {} with {:?}",
            imported.status, imported.body
        );

        let resource = resource_at(&gateway, &api, path, case).await;
        let readings: Vec<(&str, Value)> = vec![
            (
                "GetIntegration",
                get_integration(&gateway, &api, &resource, verb).await.body,
            ),
            (
                "GetResources(embed=methods)",
                embedded_integration(&gateway, &api, &resource, verb, case).await,
            ),
        ];
        for (reading, integration) in &readings {
            match expected_connection {
                Some(connection) => assert_eq!(
                    integration["connectionType"].as_str(),
                    Some(*connection),
                    "case {case} ({reading}): expected the connection type {connection:?}, got {integration:?}"
                ),
                None => assert!(
                    integration
                        .as_object()
                        .is_some_and(|members| !members.contains_key("connectionType")),
                    "case {case} ({reading}): expected no connectionType member, got {integration:?}"
                ),
            }
            assert_eq!(
                integration["responseTransferMode"].as_str(),
                Some(*expected_mode),
                "case {case} ({reading}): expected the transfer mode {expected_mode:?}, got {integration:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_integration_set_up_by_a_call_that_names_no_connection_type_reports_the_documented_default()
{
    // The contrast to the import: `connectionType`'s "The default value is INTERNET" is
    // documented on the member a `PutIntegration` carries, so the call fills it in.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-connection-default").await;
    let resource = resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;
    let created = put_integration(&gateway, &api, &resource, "GET", json!({ "type": "MOCK" })).await;
    assert_eq!(
        created.body["connectionType"].as_str(),
        Some(DEFAULT_CONNECTION_TYPE),
        "case PutIntegration naming no connection type: expected the documented default, got {:?}",
        created.body
    );
}

/// Assert the `responseTemplates` map `templates` holds exactly `expected`: one entry per
/// content type, whose value is the template body or `None` for the null an empty template
/// is reported as.
fn assert_templates(case: &str, reading: &str, templates: &Value, expected: &[(&str, Option<&str>)]) {
    let map = templates.as_object().unwrap_or_else(|| {
        panic!("case {case} ({reading}): expected a responseTemplates map, got {templates:?}")
    });
    assert_eq!(
        map.len(),
        expected.len(),
        "case {case} ({reading}): expected {} template entries, got {templates:?}",
        expected.len()
    );
    for (content_type, body) in expected {
        let value = map.get(*content_type).unwrap_or_else(|| {
            panic!("case {case} ({reading}): expected an entry for {content_type}, got {templates:?}")
        });
        match body {
            Some(body) => assert_eq!(
                value.as_str(),
                Some(*body),
                "case {case} ({reading}): expected the template {body:?} for {content_type}, got {value:?}"
            ),
            None => assert!(
                value.is_null(),
                "case {case} ({reading}): expected a null template for {content_type}, got {value:?}"
            ),
        }
    }
}

#[tokio::test]
async fn an_empty_response_template_is_reported_as_null_in_every_reading_of_it() {
    // Rows differ along what the rule generalises over: how many content types the map
    // carries, which of their templates are empty, and the content type the empty one is
    // keyed by.
    let cases: Vec<TemplateRow> = vec![
        (
            "one content type whose template body is empty",
            "empty",
            "GET",
            "200",
            json!({ "responseTemplates": { JSON: "" } }),
            vec![(JSON, None)],
        ),
        (
            "an empty template beside a rendered one",
            "mixed",
            "POST",
            "201",
            json!({ "responseTemplates": { JSON: "", "text/plain": "ok" } }),
            vec![(JSON, None), ("text/plain", Some("ok"))],
        ),
        (
            "two content types whose template bodies are both empty",
            "both-empty",
            "PUT",
            "400",
            json!({ "responseTemplates": { JSON: "", "text/html": "" } }),
            vec![(JSON, None), ("text/html", None)],
        ),
        (
            "a rendered template only",
            "rendered",
            "DELETE",
            "500",
            json!({ "responseTemplates": { JSON: "{\"message\": \"error\"}" } }),
            vec![(JSON, Some("{\"message\": \"error\"}"))],
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "empty-response-templates").await;

    for (case, path_part, verb, status, request, expected) in &cases {
        let resource = resource_with_an_integration(&gateway, &api, &root, path_part, verb).await;
        let created =
            put_integration_response(&gateway, &api, &resource, verb, status, request.clone())
                .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutIntegrationResponse, got {} with {:?}",
            created.status, created.body
        );

        // The templates are reported the same way by the response's own operations and by
        // every record that embeds it.
        let readings: Vec<(&str, Value)> = vec![
            (
                "PutIntegrationResponse",
                created.body["responseTemplates"].clone(),
            ),
            (
                "GetIntegrationResponse",
                get_integration_response(&gateway, &api, &resource, verb, status)
                    .await
                    .body["responseTemplates"]
                    .clone(),
            ),
            (
                "GetIntegration",
                get_integration(&gateway, &api, &resource, verb).await.body
                    ["integrationResponses"][status]["responseTemplates"]
                    .clone(),
            ),
            (
                "GetMethod",
                get_method(&gateway, &api, &resource, verb).await.body["methodIntegration"]
                    ["integrationResponses"][status]["responseTemplates"]
                    .clone(),
            ),
            (
                "GetResources(embed=methods)",
                embedded_integration(&gateway, &api, &resource, verb, case).await
                    ["integrationResponses"][status]["responseTemplates"]
                    .clone(),
            ),
        ];
        for (reading, templates) in &readings {
            assert_templates(case, reading, templates, expected);
        }
    }
}

#[tokio::test]
async fn a_patch_that_empties_a_response_template_reports_it_as_null() {
    // The same rule over the operation that CHANGES a template: a template patched to an
    // empty body is reported the way one created empty is, and a template patched back to
    // a body is reported as that body.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "patched-response-templates").await;
    let resource = resource_with_an_integration(&gateway, &api, &root, "pets", "GET").await;
    let created = put_integration_response(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        json!({ "responseTemplates": { JSON: "{\"pets\": []}" } }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case the seeded response: expected 201, got {} with {:?}",
        created.status, created.body
    );

    let uri = integration_response_uri(&api, &resource, "GET", "200");
    let emptied = patch_ok(
        &gateway,
        "empty the template",
        &uri,
        vec![replace("/responseTemplates/application~1json", "")],
    )
    .await;
    assert_templates(
        "empty the template",
        "UpdateIntegrationResponse",
        &emptied["responseTemplates"],
        &[(JSON, None)],
    );
    assert_templates(
        "empty the template",
        "GetIntegrationResponse",
        &get_integration_response(&gateway, &api, &resource, "GET", "200")
            .await
            .body["responseTemplates"],
        &[(JSON, None)],
    );

    let refilled = patch_ok(
        &gateway,
        "fill the template back in",
        &uri,
        vec![replace(
            "/responseTemplates/application~1json",
            "{\"pets\": []}",
        )],
    )
    .await;
    assert_templates(
        "fill the template back in",
        "UpdateIntegrationResponse",
        &refilled["responseTemplates"],
        &[(JSON, Some("{\"pets\": []}"))],
    );
}

#[tokio::test]
async fn a_definition_exported_with_an_empty_template_keeps_the_template_it_declares() {
    // The rendering follows the SHAPE the body is a reading of: an exported API
    // definition is not an `IntegrationResponse` rendering, so the templates inside the
    // document it exports are the document's own.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "exported-empty-template").await;
    let resource = resource_with_an_integration(&gateway, &api, &root, "pets", "GET").await;
    let created = put_integration_response(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        json!({ "responseTemplates": { JSON: "" } }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case the seeded response: expected 201, got {} with {:?}",
        created.status, created.body
    );
    deploy(&gateway, &api, "dev").await;

    let document = export(
        &gateway,
        &api,
        "dev",
        "oas30",
        None,
        &[("extensions", "integrations")],
    )
    .await
    .json("GetExport");
    let templates = &document["paths"]["/pets"]["get"]["x-amazon-apigateway-integration"]
        ["responses"]["default"]["responseTemplates"];
    assert_eq!(
        templates[JSON].as_str(),
        Some(""),
        "case GetExport: expected the exported document's own empty template, got {templates:?}"
    );
}
