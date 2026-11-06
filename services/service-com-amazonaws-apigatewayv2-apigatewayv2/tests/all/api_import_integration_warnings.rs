// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `ImportApi` and `ReimportApi` do with a problem that
//! belongs to ONE path of the OpenAPI definition they are given.
//!
//! The rules these cases assert:
//!
//! * **A problem with one path's `x-amazon-apigateway-integration` is not a problem with
//!   the definition**: the api is created, that one integration is skipped, the route the
//!   operation declares is still imported (with no `Target`), and the problem is reported
//!   in `Warnings` as `Unable to create integration for resource at path
//!   '<METHOD> <path>': <reason>. Ignoring.`, naming the method and path the submitted
//!   document declares the operation under.
//! * **`FailOnWarnings` is the only thing that turns such a warning into a refusal**:
//!   without it the same definition is imported, with it the call is refused 400 and
//!   persists nothing.
//! * **A key of the extension an HTTP api's integration has no member for is carried over
//!   as nothing and reported in `ImportInfo`** as
//!   `<location>: key [<key>] will be ignored by API Gateway`, where the location is where
//!   the document wrote the key. Such a key does not stop the integration being created.
//! * **`ReimportApi` reads a definition exactly as `ImportApi` does**, so the same body is
//!   as lenient over an existing api as it is over a new one.
//!
//! The rows span the classes of per-path integration problem the reader can meet — a
//! missing required member, a member outside its catalog, a member outside its modelled
//! range, a combination of members that cannot hold together, and an extension that is not
//! an object — over several methods (including the catch-all one), several paths and both
//! operations. The recorded case that filed the issue — a non-proxy integration on a `GET`
//! path — is ONE row.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_import::{import, stored_integrations, stored_routes};
use crate::api_lifecycle::{as_str, call, emulator, error_code};
use crate::api_reimport::reimport;

/// The fixed text of the warning an import reports for an integration it could not create,
/// as a template over the resource the document declared and the reason the integration
/// was not created.
fn unable_to_create(method: &str, path: &str, reason: &str) -> String {
    format!("Unable to create integration for resource at path '{method} {path}': {reason}. Ignoring.")
}

/// The reason API Gateway gives for an integration whose extension names no `type`.
const INTEGRATION_TYPE_REQUIRED: &str =
    "IntegrationType is a required parameter for creating integration.";

/// The reason API Gateway gives for an integration of a type an HTTP api cannot be
/// associated with: the two proxy types are the ones every non-WebSocket api may carry.
const ONLY_PROXY_INTEGRATIONS: &str = "Currently, an API with a protocol type of HTTP may \
                                       only be associated with proxy integrations \
                                       (AWS_PROXY, HTTP_PROXY)";

/// A definition declaring ONE operation under `method` at `path`, carrying `integration`
/// as its `x-amazon-apigateway-integration` extension.
fn definition(name: &str, method: &str, path: &str, integration: Value) -> String {
    json!({
        "openapi": "3.0.1",
        "info": { "title": name, "version": "1.0" },
        "paths": { path: { method: { "x-amazon-apigateway-integration": integration } } }
    })
    .to_string()
}

/// One row of the per-path problem table: the document's operation, the extension that
/// cannot be created, and the warning the import must report for it.
struct Row {
    case: &'static str,
    /// The operation key the document declares, as written.
    declared_method: &'static str,
    /// The method the route key — and the warning — names for that operation key.
    route_method: &'static str,
    path: &'static str,
    integration: Value,
    /// The whole warning, when the recorded service's wording for this class is known;
    /// otherwise the reason this emulator words it with.
    reason: String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case: an api of protocol type HTTP and an integration that is
            // not one of the two proxy types.
            case: "a non-proxy HTTP integration",
            declared_method: "get",
            route_method: "GET",
            path: "/probe",
            integration: json!({
                "type": "http",
                "httpMethod": "GET",
                "uri": "https://probe.example.com/probe"
            }),
            reason: ONLY_PROXY_INTEGRATIONS.to_string(),
        },
        Row {
            case: "an integration type supported only for WebSocket APIs",
            declared_method: "post",
            route_method: "POST",
            path: "/mocked",
            integration: json!({ "type": "mock" }),
            reason: ONLY_PROXY_INTEGRATIONS.to_string(),
        },
        Row {
            case: "an extension naming no type, under the catch-all method",
            declared_method: "x-amazon-apigateway-any-method",
            route_method: "ANY",
            path: "/anything/{proxy+}",
            integration: json!({ "uri": "https://anything.example.com" }),
            reason: INTEGRATION_TYPE_REQUIRED.to_string(),
        },
        Row {
            case: "an integration type outside the modelled catalog",
            declared_method: "delete",
            route_method: "DELETE",
            path: "/sidecar",
            integration: json!({ "type": "sidecar", "uri": "https://sidecar.example.com" }),
            reason: "invalid integration type 'SIDECAR'. Valid values are: AWS, AWS_PROXY, \
                     HTTP, HTTP_PROXY, MOCK."
                .to_string(),
        },
        Row {
            case: "an HTTP_PROXY integration naming no endpoint",
            declared_method: "put",
            route_method: "PUT",
            path: "/uriless",
            integration: json!({ "type": "http_proxy" }),
            reason: "IntegrationUri is required for an integration of type HTTP_PROXY."
                .to_string(),
        },
        Row {
            case: "a VPC_LINK connection naming no link",
            declared_method: "patch",
            route_method: "PATCH",
            path: "/private",
            integration: json!({
                "type": "http_proxy",
                "uri": "https://private.example.com",
                "connectionType": "VPC_LINK"
            }),
            reason: "ConnectionId is required for a connection of type VPC_LINK.".to_string(),
        },
        Row {
            case: "a timeout past the modelled range",
            declared_method: "head",
            route_method: "HEAD",
            path: "/slow",
            integration: json!({
                "type": "http_proxy",
                "uri": "https://slow.example.com",
                "timeoutInMillis": 30_001_i64
            }),
            reason: "'x-amazon-apigateway-integration.timeoutInMillis' must be between 50 \
                     and 30000, got 30001"
                .to_string(),
        },
        Row {
            case: "an extension that is not an object",
            declared_method: "options",
            route_method: "OPTIONS",
            path: "/scalar",
            integration: json!("http_proxy"),
            reason: "'x-amazon-apigateway-integration' must be an object".to_string(),
        },
    ]
}

/// Every warning an answer reports, in order.
fn warnings(answer: &Value) -> Vec<String> {
    answer["warnings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry.as_str().map(ToString::to_string))
        .collect()
}

/// Every `ImportInfo` entry an answer reports, in order.
fn import_info(answer: &Value) -> Vec<String> {
    answer["importInfo"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry.as_str().map(ToString::to_string))
        .collect()
}

/// How many apis the store holds, through `GetApis`.
async fn stored_api_count(router: &MultiServiceRouter) -> usize {
    let (status, body) = call(router, "GET", "/v2/apis", None).await;
    assert_eq!(status, 200, "expected 200 from GetApis, got {status} {body}");
    body["items"].as_array().map(Vec::len).unwrap_or_default()
}

#[tokio::test]
async fn a_per_path_integration_problem_is_a_warning_and_not_a_refusal() {
    let router = emulator();

    for row in rows() {
        let case = row.case;
        let body = definition(
            case,
            row.declared_method,
            row.path,
            row.integration.clone(),
        );
        let (status, imported) = import(&router, "", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the api must be created, got {status} {imported}"
        );
        assert_eq!(
            warnings(&imported),
            vec![unable_to_create(row.route_method, row.path, &row.reason)],
            "case {case:?}: expected the warning naming the resource and the reason, got \
             {imported}"
        );

        // The api, its route and the absence of the integration are read back through the
        // service's own readers, so nothing here is echoed from the request.
        let api_id = as_str(&imported, "apiId");
        let (status, read_back) = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetApi, got {status} {read_back}"
        );
        assert_eq!(
            as_str(&read_back, "name"),
            case,
            "case {case:?}: GetApi must report the imported api, got {read_back}"
        );
        assert_eq!(
            stored_routes(&router, &api_id).await,
            BTreeMap::from([(
                format!("{} {}", row.route_method, row.path),
                ::std::option::Option::None
            )]),
            "case {case:?}: the route must be imported with no target"
        );
        let integrations = stored_integrations(&router, &api_id).await;
        assert!(
            integrations.is_empty(),
            "case {case:?}: the skipped integration must not be created, got {integrations:?}"
        );
    }
}

#[tokio::test]
async fn fail_on_warnings_refuses_the_definition_the_import_warns_about() {
    let router = emulator();

    for row in rows() {
        let case = row.case;
        let body = definition(
            case,
            row.declared_method,
            row.path,
            row.integration.clone(),
        );
        let before = stored_api_count(&router).await;
        let (status, refused) = import(&router, "?failOnWarnings=true", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: FailOnWarnings must refuse the import, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {refused}"
        );
        assert_eq!(
            stored_api_count(&router).await,
            before,
            "case {case:?}: a refused import must persist nothing"
        );
    }
}

/// The keys an `x-amazon-apigateway-integration` extension can carry that an HTTP api's
/// integration has no member for: the REST-api integration fields, since the members an
/// HTTP api's integration holds are `CreateIntegration`'s.
#[tokio::test]
async fn an_extension_key_an_http_integration_has_no_member_for_is_reported_as_ignored() {
    let router = emulator();

    // Each row carries one key of a REST api's integration — none of which an HTTP api's
    // integration holds a member for — beside the members that DO create the integration,
    // under a different path and method, so the reported location is the document's own.
    let cases: Vec<(&str, &str, &str, &str, Value)> = vec![
        (
            "a request template",
            "get",
            "/probe",
            "requestTemplates",
            json!({ "application/json": "{\"statusCode\": 200}" }),
        ),
        (
            "a passthrough behaviour",
            "post",
            "/orders",
            "passthroughBehavior",
            json!("when_no_match"),
        ),
        (
            "a content handling strategy",
            "put",
            "/images/{id}",
            "contentHandling",
            json!("CONVERT_TO_BINARY"),
        ),
        (
            "a cache namespace",
            "delete",
            "/cached",
            "cacheNamespace",
            json!("items"),
        ),
        (
            "an integration response map",
            "x-amazon-apigateway-any-method",
            "/legacy",
            "responses",
            json!({ "default": { "statusCode": "200" } }),
        ),
    ];

    for (case, method, path, key, value) in cases {
        let mut extension = serde_json::Map::new();
        extension.insert("type".to_string(), json!("http_proxy"));
        extension.insert("uri".to_string(), json!("https://upstream.example.com"));
        extension.insert(key.to_string(), value);
        let body = definition(case, method, path, Value::Object(extension));

        let (status, imported) = import(&router, "", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an ignored key must not fail the import, got {status} {imported}"
        );
        assert_eq!(
            import_info(&imported),
            vec![format!(
                "paths.{path}.{method}.x-amazon-apigateway-integration: key [{key}] will be \
                 ignored by API Gateway"
            )],
            "case {case:?}: expected the ignored key reported at its location, got {imported}"
        );
        // The key is ignored, not fatal: the integration the rest of the extension
        // describes is created, and the route is wired to it.
        assert!(
            warnings(&imported).is_empty(),
            "case {case:?}: an ignored key is no warning, got {imported}"
        );
        let api_id = as_str(&imported, "apiId");
        let integrations = stored_integrations(&router, &api_id).await;
        assert_eq!(
            integrations.len(),
            1,
            "case {case:?}: the integration must still be created, got {integrations:?}"
        );
        let integration_id = integrations.keys().next().cloned().unwrap_or_default();
        assert_eq!(
            stored_routes(&router, &api_id).await.into_values().collect::<Vec<_>>(),
            vec![Some(format!("integrations/{integration_id}"))],
            "case {case:?}: the route must be wired to the created integration"
        );
    }

    // The members an HTTP api's integration DOES hold are not reported: only what the
    // import carries over as nothing is.
    let carried = definition(
        "every-member-carried",
        "get",
        "/carried",
        json!({
            "type": "http_proxy",
            "httpMethod": "GET",
            "uri": "https://upstream.example.com",
            "connectionType": "INTERNET",
            "payloadFormatVersion": "1.0",
            "timeoutInMillis": 12_000_i64,
            "requestParameters": { "append:header.x-probe": "1" },
            "responseParameters": { "200": { "append:header.x-probe": "1" } },
            "tlsConfig": { "serverNameToVerify": "upstream.example.com" }
        }),
    );
    let (status, imported) = import(&router, "", &carried).await;
    assert_eq!(
        status, 200,
        "an extension of carried members imports, got {status} {imported}"
    );
    assert!(
        import_info(&imported).is_empty(),
        "an extension whose every key is carried over reports no ImportInfo, got {imported}"
    );
}

#[tokio::test]
async fn reimport_api_is_as_lenient_as_import_api() {
    let router = emulator();

    // An api imported from a definition whose integration IS created.
    let clean = definition(
        "reimported",
        "get",
        "/items",
        json!({ "type": "http_proxy", "uri": "https://items.example.com/items" }),
    );
    let (status, imported) = import(&router, "", &clean).await;
    assert_eq!(
        status, 200,
        "expected 200 from ImportApi, got {status} {imported}"
    );
    let api_id = as_str(&imported, "apiId");

    // The same per-path problem over an EXISTING api: the reimport goes through, the
    // route it declares is kept without a target, and the problem is warned about.
    let row = rows().remove(0);
    let body = definition("reimported", row.declared_method, row.path, row.integration);
    let (status, reimported) = reimport(&router, &api_id, "", &body).await;
    assert_eq!(
        status, 200,
        "expected 200 from ReimportApi, got {status} {reimported}"
    );
    assert_eq!(
        warnings(&reimported),
        vec![unable_to_create(row.route_method, row.path, &row.reason)],
        "ReimportApi must report the same warning ImportApi does, got {reimported}"
    );
    assert_eq!(
        stored_routes(&router, &api_id).await,
        BTreeMap::from([(
            format!("{} {}", row.route_method, row.path),
            ::std::option::Option::None
        )]),
        "the reimported definition's route must replace the api's, with no target"
    );
    assert!(
        stored_integrations(&router, &api_id).await.is_empty(),
        "the skipped integration must not be created"
    );

    // ...and `FailOnWarnings` leaves the api exactly as that reimport left it.
    let before = stored_routes(&router, &api_id).await;
    let (status, refused) = reimport(&router, &api_id, "?failOnWarnings=true", &body).await;
    assert_eq!(
        status, 400,
        "FailOnWarnings must refuse the reimport, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "BadRequestException",
        "expected BadRequestException, got {refused}"
    );
    assert_eq!(
        stored_routes(&router, &api_id).await,
        before,
        "a refused reimport must leave the api's routes as they were"
    );
}

#[tokio::test]
async fn a_basepath_option_outside_the_documented_three_is_named_in_the_refusal() {
    let router = emulator();

    // "Valid values are ignore, prepend, and split" (`ImportApi$Basepath`): the refusal
    // names the value that was not recognized, whatever it is.
    let body = definition(
        "basepath-api",
        "get",
        "/items",
        json!({ "type": "http_proxy", "uri": "https://items.example.com/items" }),
    );
    for option in ["bogus", "Prepend", "IGNORE", "stage"] {
        let (status, refused) = import(&router, &format!("?basepath={option}"), &body).await;
        assert_eq!(
            status, 400,
            "case {option:?}: expected 400 from ImportApi, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {option:?}: expected BadRequestException, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            format!("Unrecognized base path option: {option}"),
            "case {option:?}: the refusal must name the option it did not recognize, got \
             {refused}"
        );
    }
}
