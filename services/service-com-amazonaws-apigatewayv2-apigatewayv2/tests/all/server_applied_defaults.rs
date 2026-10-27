// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the members of an `Integration` and of a `Stage` that the
//! SERVICE supplies when nothing configured them.
//!
//! The rule both halves assert: **a create resolves every member the service itself
//! decides, stores it, and so every later read reports it** — the read-back of a resource
//! never leaves a member absent that the service has a value for. What each value is comes
//! from the member's own documentation:
//!
//! * `Integration$PassthroughBehavior` — `WHEN_NO_MATCH`, which "passes the request body
//!   for unmapped content types through to the integration backend without
//!   transformation", the behaviour of an integration that mapped no content type. The
//!   member is "Supported only for WebSocket APIs", so only such an api's integration
//!   carries one (catalog: `smithy_query(query="shape", shape="PassthroughBehavior")`).
//! * `Integration$PayloadFormatVersion` — `1.0`: "For all other integrations, 1.0 is the
//!   only supported value", so an integration whose create named no version is served
//!   under the one every type supports. (An HTTP api's integration must name one at
//!   create, so it already carries its own.)
//! * `Integration$IntegrationMethod` — `POST` for the Lambda proxy integration
//!   (`AWS_PROXY` naming no `IntegrationSubtype`), whose endpoint is reached through
//!   Lambda's own function-invoking action, served at
//!   `POST /2015-03-31/functions/{FunctionArn}/invocations`. Every other type's method
//!   belongs to its own endpoint, so the service supplies none.
//! * `Stage$DefaultRouteSettings` — the settings a stage's routes are served under, so a
//!   stage that configured none of them, or only some, still reports every one the service
//!   supplies: each facility `RouteSettings` switches reported off, and the logging level
//!   that pushes no log entries, `OFF` (catalog:
//!   `smithy_query(query="shape", shape="LoggingLevel")`). `RouteSettings$DataTraceEnabled`
//!   and `RouteSettings$LoggingLevel` are "Supported only for WebSocket APIs", so a stage
//!   of an HTTP api reports neither.
//!
//! The rows vary what the rule generalises over: the parent api's protocol, the
//! integration type and whether it names a subtype, which members the request supplied
//! itself (none, some or all — so the service overrides none of them and fills the rest),
//! and which operation created the resource — `CreateIntegration`/`CreateStage`, or the quick create that mints an
//! integration and a `$default` stage on the caller's behalf. The recorded cases that
//! filed the issue — an `AWS_PROXY` integration and a `prod` stage of a WebSocket api,
//! both created with nothing but the required members — are two rows.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator};

/// The `TimeoutInMillis` a WebSocket api's integration carries when the request names
/// none ("The default timeout is 29 seconds for WebSocket APIs",
/// `CreateIntegration$TimeoutInMillis`).
const DEFAULT_WEBSOCKET_TIMEOUT_MILLIS: i64 = 29_000;

/// The same for an HTTP api ("30 seconds for HTTP APIs").
const DEFAULT_HTTP_TIMEOUT_MILLIS: i64 = 30_000;

fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

/// One integration row: the api to create it under, the `CreateIntegration` body, and
/// what each member the service decides must read back as — `None` for a member the
/// service has no value for, which must be reported absent rather than invented.
struct IntegrationRow {
    case: &'static str,
    api: Value,
    request: Value,
    expected: Vec<(&'static str, Option<Value>)>,
}

/// The Lambda proxy integration's endpoint, in the form
/// `Integration$IntegrationUri` documents for a Lambda integration.
fn lambda_uri(function: &str) -> String {
    format!("arn:aws:lambda:eu-west-1:000000000000:function:{function}")
}

fn integration_rows() -> Vec<IntegrationRow> {
    vec![
        IntegrationRow {
            case: "a WebSocket AWS_PROXY integration carrying only its type and uri",
            api: websocket_api("ws-proxy-api"),
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": lambda_uri("connect-handler")
            }),
            expected: vec![
                ("integrationMethod", Some(json!("POST"))),
                ("payloadFormatVersion", Some(json!("1.0"))),
                ("passthroughBehavior", Some(json!("WHEN_NO_MATCH"))),
                ("connectionType", Some(json!("INTERNET"))),
                (
                    "timeoutInMillis",
                    Some(json!(DEFAULT_WEBSOCKET_TIMEOUT_MILLIS)),
                ),
            ],
        },
        IntegrationRow {
            case: "a WebSocket MOCK integration, which names no endpoint at all",
            api: websocket_api("ws-mock-api"),
            request: json!({ "integrationType": "MOCK" }),
            expected: vec![
                // MOCK invokes nothing, so there is no method for the service to name.
                ("integrationMethod", None),
                ("payloadFormatVersion", Some(json!("1.0"))),
                ("passthroughBehavior", Some(json!("WHEN_NO_MATCH"))),
                (
                    "timeoutInMillis",
                    Some(json!(DEFAULT_WEBSOCKET_TIMEOUT_MILLIS)),
                ),
            ],
        },
        IntegrationRow {
            case: "a WebSocket HTTP integration, whose method belongs to its backend",
            api: websocket_api("ws-http-api"),
            request: json!({
                "integrationType": "HTTP",
                "integrationUri": "https://example.com/upstream"
            }),
            expected: vec![
                ("integrationMethod", None),
                ("payloadFormatVersion", Some(json!("1.0"))),
                ("passthroughBehavior", Some(json!("WHEN_NO_MATCH"))),
            ],
        },
        IntegrationRow {
            case: "a WebSocket AWS integration, whose method its own uri's action decides",
            api: websocket_api("ws-aws-api"),
            request: json!({
                "integrationType": "AWS",
                "integrationUri": lambda_uri("custom-handler")
            }),
            expected: vec![
                ("integrationMethod", None),
                ("payloadFormatVersion", Some(json!("1.0"))),
                ("passthroughBehavior", Some(json!("WHEN_NO_MATCH"))),
            ],
        },
        IntegrationRow {
            case: "a WebSocket integration that supplied every one of those members itself",
            api: websocket_api("ws-explicit-api"),
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": lambda_uri("explicit-handler"),
                "integrationMethod": "GET",
                "passthroughBehavior": "NEVER",
                "payloadFormatVersion": "2.0",
                "timeoutInMillis": 1_500
            }),
            expected: vec![
                ("integrationMethod", Some(json!("GET"))),
                ("payloadFormatVersion", Some(json!("2.0"))),
                ("passthroughBehavior", Some(json!("NEVER"))),
                ("timeoutInMillis", Some(json!(1_500))),
            ],
        },
        IntegrationRow {
            case: "an HTTP api AWS_PROXY integration fronting a function",
            api: http_api("http-proxy-api"),
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": lambda_uri("http-handler"),
                "payloadFormatVersion": "2.0"
            }),
            expected: vec![
                ("integrationMethod", Some(json!("POST"))),
                ("payloadFormatVersion", Some(json!("2.0"))),
                // "Supported only for WebSocket APIs", so an HTTP api's integration
                // carries no passthrough behaviour for the service to report.
                ("passthroughBehavior", None),
                ("timeoutInMillis", Some(json!(DEFAULT_HTTP_TIMEOUT_MILLIS))),
            ],
        },
        IntegrationRow {
            case: "an HTTP api AWS_PROXY integration naming a service action subtype",
            api: http_api("http-subtype-api"),
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationSubtype": "SQS-SendMessage",
                "payloadFormatVersion": "1.0",
                "requestParameters": { "QueueUrl": "$request.header.queue" }
            }),
            expected: vec![
                // A subtype names the action itself, so this invokes no function and the
                // service names no method for it.
                ("integrationMethod", None),
                ("payloadFormatVersion", Some(json!("1.0"))),
                ("passthroughBehavior", None),
            ],
        },
        IntegrationRow {
            case: "an HTTP api HTTP_PROXY integration, whose method belongs to its backend",
            api: http_api("http-upstream-api"),
            request: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "1.0"
            }),
            expected: vec![
                ("integrationMethod", None),
                ("payloadFormatVersion", Some(json!("1.0"))),
                ("passthroughBehavior", None),
            ],
        },
    ]
}

/// Create one api and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Assert every member of `expected` on `document`.
fn assert_members(case: &str, source: &str, document: &Value, expected: &[(&str, Option<Value>)]) {
    for (member, value) in expected {
        assert_eq!(
            document.get(*member),
            value.as_ref(),
            "case {case:?}: expected {source} to report {member}={value:?}, got {document}"
        );
    }
}

/// The one entry of a collection page whose `key` member reads `id`.
fn listed<'a>(case: &str, page: &'a Value, key: &str, id: &str) -> &'a Value {
    page["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a page of items, got {page}"))
        .iter()
        .find(|item| item.get(key).and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("case {case:?}: expected {key} {id:?} in the page {page}"))
}

#[tokio::test]
async fn an_integration_reads_back_with_the_members_the_service_supplies() {
    let emulator = emulator();

    for row in integration_rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(row.request.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateIntegration, got {status} {created}",
            row.case
        );
        assert_members(row.case, "CreateIntegration", &created, &row.expected);

        let integration_id = as_str(&created, "integrationId");
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetIntegration, got {status} {fetched}",
            row.case
        );
        assert_members(row.case, "GetIntegration", &fetched, &row.expected);

        // And through the collection the issue was filed against: one record, so one set
        // of values whichever reader reports it.
        let (status, page) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetIntegrations, got {status} {page}",
            row.case
        );
        let entry = listed(row.case, &page, "integrationId", &integration_id);
        assert_members(row.case, "GetIntegrations", entry, &row.expected);
        assert_eq!(
            entry, &created,
            "case {:?}: GetIntegrations must report the document CreateIntegration answered with",
            row.case
        );

        // A read must not change what a later caller sees.
        let (_, refetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
            None,
        )
        .await;
        assert_eq!(
            refetched, fetched,
            "case {:?}: GetIntegration changed the record it reported",
            row.case
        );
    }
}

/// One stage row: the api to create it under, the `CreateStage` body, and the
/// `DefaultRouteSettings` the stored stage must report.
struct StageRow {
    case: &'static str,
    api: Value,
    request: Value,
    expected: Value,
}

fn stage_rows() -> Vec<StageRow> {
    vec![
        StageRow {
            case: "a WebSocket stage created with no DefaultRouteSettings",
            api: websocket_api("ws-stage-api"),
            request: json!({ "stageName": "prod" }),
            expected: json!({
                "dataTraceEnabled": false,
                "detailedMetricsEnabled": false,
                "loggingLevel": "OFF"
            }),
        },
        StageRow {
            case: "an HTTP stage created with no DefaultRouteSettings",
            api: http_api("http-stage-api"),
            request: json!({ "stageName": "prod" }),
            // The data trace and the logging level are "Supported only for WebSocket
            // APIs", so an HTTP api's stage reports neither.
            expected: json!({ "detailedMetricsEnabled": false }),
        },
        StageRow {
            case: "the $default stage of an HTTP api, created with AutoDeploy on",
            api: http_api("http-default-stage-api"),
            request: json!({ "stageName": "$default", "autoDeploy": true }),
            expected: json!({ "detailedMetricsEnabled": false }),
        },
        StageRow {
            case: "a WebSocket stage that supplied its own DefaultRouteSettings",
            api: websocket_api("ws-configured-stage-api"),
            request: json!({
                "stageName": "configured",
                "defaultRouteSettings": {
                    "dataTraceEnabled": true,
                    "detailedMetricsEnabled": true,
                    "loggingLevel": "INFO",
                    "throttlingBurstLimit": 100,
                    "throttlingRateLimit": 50.5
                }
            }),
            expected: json!({
                "dataTraceEnabled": true,
                "detailedMetricsEnabled": true,
                "loggingLevel": "INFO",
                "throttlingBurstLimit": 100,
                "throttlingRateLimit": 50.5
            }),
        },
        StageRow {
            case: "an HTTP stage that supplied only its own throttle",
            api: http_api("http-throttled-stage-api"),
            request: json!({
                "stageName": "throttled",
                "defaultRouteSettings": { "throttlingRateLimit": 12.5 }
            }),
            // The throttle the request named, beside the facility member it left to the
            // service: the resolution is per member, not all-or-nothing.
            expected: json!({ "detailedMetricsEnabled": false, "throttlingRateLimit": 12.5 }),
        },
        StageRow {
            case: "a WebSocket stage that supplied only its logging level",
            api: websocket_api("ws-logged-stage-api"),
            request: json!({
                "stageName": "logged",
                "defaultRouteSettings": { "loggingLevel": "INFO" }
            }),
            expected: json!({
                "dataTraceEnabled": false,
                "detailedMetricsEnabled": false,
                "loggingLevel": "INFO"
            }),
        },
    ]
}

#[tokio::test]
async fn a_stage_reads_back_with_the_route_settings_the_service_supplies() {
    let emulator = emulator();

    for row in stage_rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(row.request.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateStage, got {status} {created}",
            row.case
        );
        assert_eq!(
            created.get("defaultRouteSettings"),
            Some(&row.expected),
            "case {:?}: expected CreateStage to report DefaultRouteSettings {}, got {created}",
            row.case,
            row.expected
        );

        let stage_name = as_str(&created, "stageName");
        let stage_path = format!("/v2/apis/{api_id}/stages/{stage_name}");
        let (status, fetched) = call(&emulator, "GET", &stage_path, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetStage, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            fetched.get("defaultRouteSettings"),
            Some(&row.expected),
            "case {:?}: expected GetStage to report DefaultRouteSettings {}, got {fetched}",
            row.case,
            row.expected
        );

        // And through the collection: one record, so one set of values.
        let (status, page) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/stages"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetStages, got {status} {page}",
            row.case
        );
        let entry = listed(row.case, &page, "stageName", &stage_name);
        assert_eq!(
            entry.get("defaultRouteSettings"),
            Some(&row.expected),
            "case {:?}: expected GetStages to report DefaultRouteSettings {}, got {entry}",
            row.case,
            row.expected
        );

        // A read must not change what a later caller sees.
        let (_, refetched) = call(&emulator, "GET", &stage_path, None).await;
        assert_eq!(
            refetched, fetched,
            "case {:?}: GetStage changed the record it reported",
            row.case
        );
    }
}

#[tokio::test]
async fn the_resources_quick_create_mints_carry_the_same_supplied_members() {
    let emulator = emulator();

    // Quick create is "Supported only for HTTP APIs" (`CreateApi$Target`) and mints an
    // integration, a route and the `$default` stage on the caller's behalf — so the
    // members the service supplies are the same ones, resolved by the operation that
    // wrote the records rather than by `CreateIntegration`/`CreateStage`.
    for (case, target, expected_method) in [
        (
            "a function target, which mints the Lambda proxy integration",
            lambda_uri("quick-handler"),
            Some(json!("POST")),
        ),
        (
            "an http target, whose method belongs to its backend",
            "https://example.com/upstream".to_string(),
            None,
        ),
    ] {
        let api_id = create_api(
            &emulator,
            case,
            &json!({
                "name": format!("quick-{}", target.len()),
                "protocolType": "HTTP",
                "target": target
            }),
        )
        .await;

        let (status, page) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetIntegrations, got {status} {page}"
        );
        let integrations = page["items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            integrations.len(),
            1,
            "case {case:?}: quick create mints one integration, got {page}"
        );
        let integration = &integrations[0];
        assert_eq!(
            integration.get("integrationMethod"),
            expected_method.as_ref(),
            "case {case:?}: expected IntegrationMethod {expected_method:?}, got {integration}"
        );

        let (status, stage) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/$default"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage for the managed stage, got {status} {stage}"
        );
        assert_eq!(
            stage.get("defaultRouteSettings"),
            Some(&json!({ "detailedMetricsEnabled": false })),
            "case {case:?}: expected the HTTP api's own default route settings, got {stage}"
        );
    }
}
