// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the INTEGRATION an imported API definition materialises, read
//! back through `GetIntegration` — the operation a client uses to verify an import.
//!
//! RULE: an operation of an imported API definition becomes a method whose integration is
//! the one API Gateway's own `x-amazon-apigateway-integration` extension declares on that
//! operation, so `GetIntegration` for an imported method answers the declared integration;
//! the modelled `NotFoundException` is left for a method that carries none. The rule holds
//! for every operation that imports a definition and in either `PutMode`, so `PutRestApi`
//! imports the same integrations `ImportRestApi` does.
//!
//! The sibling `rest_api_create_from_import` suite asserts that rule for the operation
//! that creates an API from a definition; this one asserts it for the operation that
//! imports a definition INTO an existing API, which is the half a client reaches when it
//! re-provisions an API from an OpenAPI document.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The emulator, the `CreateRestApi` helper, the `PutRestApi` call and the
//! `GetResources` read are this crate's existing harness.
//!
//! The rows differ along what the rule generalises over: the `PutMode` the import runs in
//! (`overwrite`, `merge`, and the mode left unset, which the `mode` member documents as
//! `merge`), the form the document is written in (JSON and YAML, the two the `body` member
//! names), the integration type the extension declares (every type whose required members
//! a document can carry), the verb the operation is declared under, and how deeply the
//! path nests.

use serde_json::Value;

use crate::method_integration::{aws_integration_uri, get_integration, put_integration};
use crate::rest_api_import::{put_definition, resources_by_path};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, resource_under, string_member, Gateway,
};

/// What one imported method must read back as: the path its resource is at, the verb it
/// is declared under, and the integration `type` `GetIntegration` must report for it —
/// `None` for an operation the document declares with no integration extension, which has
/// no integration to report.
type ExpectedIntegration<'a> = (&'a str, &'a str, Option<&'a str>);

/// One row of the import table: the case's name, the `mode` query value (`None` leaves the
/// member unset), the definition text, and what each of its operations must read back as.
type ImportRow<'a> = (
    &'a str,
    Option<&'a str>,
    String,
    &'a [ExpectedIntegration<'a>],
);

/// The id of the resource at `path` in `api`.
///
/// `pub(crate)` for the reason [`json_operation`] gives.
pub(crate) async fn resource_at(gateway: &Gateway, api: &str, path: &str, case: &str) -> String {
    let resources = resources_by_path(gateway, api, case).await;
    let (_, resource) = resources
        .iter()
        .find(|(candidate, _)| candidate == path)
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a resource at {path:?}, got {:?}",
                resources
                    .iter()
                    .map(|(candidate, _)| candidate.as_str())
                    .collect::<Vec<_>>()
            )
        });
    string_member(resource, "id", case)
}

/// An operation object declaring `integration` as its API Gateway integration extension,
/// in JSON.
///
/// `pub(crate)` so the sibling `integration_representation` suite, which asserts what an
/// imported integration REPORTS, declares an extension the same way.
pub(crate) fn json_operation(integration: &str) -> String {
    format!(
        "{{\"responses\": {{\"200\": {{\"description\": \"ok\"}}}}, \
          \"x-amazon-apigateway-integration\": {integration}}}"
    )
}

/// A whole JSON definition for one path carrying one operation. `pub(crate)` for the
/// reason [`json_operation`] gives.
pub(crate) fn json_definition(title: &str, path: &str, verb: &str, operation: &str) -> String {
    format!(
        "{{\"openapi\": \"3.0.1\", \"info\": {{\"title\": \"{title}\"}}, \
          \"paths\": {{\"{path}\": {{\"{verb}\": {operation}}}}}}}"
    )
}

#[tokio::test]
async fn an_imported_operations_integration_extension_is_read_back_through_get_integration() {
    // The MOCK type takes neither an integration method nor a URI; the AWS types take an
    // AWS integration ARN (`aws_integration_uri`), and the Lambda proxy type takes the
    // POST integration method a Lambda integration documents; the HTTP types take a fully
    // formed HTTP(S) URL. One row per type, so no row's expectation rests on another's.
    let lambda_uri = aws_integration_uri(
        "lambda:path/2015-03-31/functions/arn:aws:lambda:us-east-1:123456789012:function:h/invocations",
    );
    let rows: Vec<ImportRow> = vec![
        (
            "overwrite imports a mock integration",
            Some("overwrite"),
            json_definition(
                "replaced",
                "/replaced",
                "get",
                &json_operation(
                    "{\"type\": \"mock\", \
                      \"requestTemplates\": {\"application/json\": \"{\\\"statusCode\\\": 200}\"}, \
                      \"responses\": {\"default\": {\"statusCode\": \"200\"}}}",
                ),
            ),
            &[("/replaced", "GET", Some("MOCK"))],
        ),
        (
            "merge imports an http proxy integration on a greedy path",
            Some("merge"),
            json_definition(
                "merged",
                "/proxy/{proxy+}",
                "x-amazon-apigateway-any-method",
                &json_operation(
                    "{\"type\": \"http_proxy\", \"httpMethod\": \"any\", \
                      \"uri\": \"https://backend.example.com/{proxy}\"}",
                ),
            ),
            &[("/proxy/{proxy+}", "ANY", Some("HTTP_PROXY"))],
        ),
        (
            "the unset mode imports an aws integration two levels down",
            None,
            json_definition(
                "defaulted",
                "/store/orders",
                "put",
                &json_operation(&format!(
                    "{{\"type\": \"aws\", \"httpMethod\": \"POST\", \
                       \"uri\": \"{}\"}}",
                    aws_integration_uri("dynamodb:action/PutItem")
                )),
            ),
            &[("/store/orders", "PUT", Some("AWS"))],
        ),
        (
            "overwrite imports a lambda proxy integration from a YAML document",
            Some("overwrite"),
            format!(
                "openapi: 3.0.1\ninfo:\n  title: yaml-import\npaths:\n  \"/lambda\":\n    \
                 post:\n      responses:\n        '200':\n          description: ok\n      \
                 x-amazon-apigateway-integration:\n        type: aws_proxy\n        \
                 httpMethod: post\n        uri: {lambda_uri}\n"
            ),
            &[("/lambda", "POST", Some("AWS_PROXY"))],
        ),
        (
            "an operation declaring no integration extension gets none",
            Some("overwrite"),
            json_definition(
                "bare",
                "/bare",
                "delete",
                "{\"responses\": {\"200\": {\"description\": \"ok\"}}}",
            ),
            &[("/bare", "DELETE", None)],
        ),
        (
            "one document's two operations each keep their own integration",
            Some("overwrite"),
            format!(
                "{{\"openapi\": \"3.0.1\", \"info\": {{\"title\": \"two\"}}, \"paths\": {{\
                   \"/mock\": {{\"head\": {}}}, \
                   \"/plain\": {{\"options\": {{\"responses\": {{\"200\": {{\"description\": \"ok\"}}}}}}, \
                                 \"post\": {}}}\
                 }}}}",
                json_operation("{\"type\": \"MOCK\"}"),
                json_operation(
                    "{\"type\": \"HTTP\", \"httpMethod\": \"GET\", \
                      \"uri\": \"http://backend.example.com/plain\"}"
                ),
            ),
            &[
                ("/mock", "HEAD", Some("MOCK")),
                ("/plain", "OPTIONS", None),
                ("/plain", "POST", Some("HTTP")),
            ],
        ),
    ];

    for (case, mode, body, expected) in rows {
        let gateway = Gateway::new();
        let (api, _root) = create_api(&gateway, case).await;
        let answer = put_definition(&gateway, &api, mode, false, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: PutRestApi expected 200, got {} with {:?}",
            answer.status, answer.body
        );

        for (path, verb, integration_type) in expected {
            let resource = resource_at(&gateway, &api, path, case).await;
            let read = get_integration(&gateway, &api, &resource, verb).await;
            match integration_type {
                Some(expected_type) => {
                    assert_eq!(
                        read.status, 200,
                        "case {case:?}: GetIntegration for {verb} {path} expected 200, got {} with {:?}",
                        read.status, read.body
                    );
                    assert_eq!(
                        read.body["type"].as_str(),
                        Some(*expected_type),
                        "case {case:?}: GetIntegration for {verb} {path} expected type {expected_type:?}, got {:?}",
                        read.body["type"]
                    );
                }
                None => assert_failure(
                    &read,
                    404,
                    "NotFoundException",
                    &format!("{case:?}: GetIntegration for {verb} {path}"),
                ),
            }
        }
    }
}

#[tokio::test]
async fn an_imported_integration_carries_the_members_the_extension_declares() {
    // The rule reaches every member of the record, not only its type: one document whose
    // extension declares an integration method, a URI, a request template, a timeout, a
    // passthrough behaviour and an integration response, read back member for member.
    let case = "declared members";
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "members").await;
    let uri = aws_integration_uri("sqs:path//123456789012/queue");
    let body = json_definition(
        "members",
        "/queued",
        "post",
        &json_operation(&format!(
            "{{\"type\": \"aws\", \"httpMethod\": \"post\", \"uri\": \"{uri}\", \
               \"passthroughBehavior\": \"when_no_templates\", \"timeoutInMillis\": 4200, \
               \"requestTemplates\": {{\"application/json\": \"Action=SendMessage\"}}, \
               \"responses\": {{\"default\": {{\"statusCode\": \"200\"}}, \
                                \"5\\\\d{{2}}\": {{\"statusCode\": \"500\"}}}}}}"
        )),
    );
    let answer = put_definition(&gateway, &api, Some("overwrite"), false, &body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: PutRestApi expected 200, got {} with {:?}",
        answer.status, answer.body
    );

    let resource = resource_at(&gateway, &api, "/queued", case).await;
    let read = get_integration(&gateway, &api, &resource, "POST").await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: GetIntegration expected 200, got {} with {:?}",
        read.status, read.body
    );
    for (member, expected) in [
        ("type", Value::from("AWS")),
        // The extension writes the enumerated values in either case; the record carries
        // the modelled spelling.
        ("httpMethod", Value::from("POST")),
        ("uri", Value::from(uri.as_str())),
        ("passthroughBehavior", Value::from("WHEN_NO_TEMPLATES")),
        ("timeoutInMillis", Value::from(4200)),
        // "By default, API Gateway uses the resource ID as the cacheNamespace", which is
        // the resource the imported method landed on.
        ("cacheNamespace", Value::from(resource.as_str())),
    ] {
        assert_eq!(
            read.body[member], expected,
            "case {case:?}: expected {member} {expected:?}, got {:?}",
            read.body[member]
        );
    }
    assert_eq!(
        read.body["requestTemplates"]["application/json"].as_str(),
        Some("Action=SendMessage"),
        "case {case:?}: expected the declared request template, got {:?}",
        read.body["requestTemplates"]
    );
    // An integration response is addressed by the status code it declares, and the entry
    // the extension keys `default` matches everything the others do not, so it carries no
    // selection pattern of its own.
    assert!(
        read.body["integrationResponses"]["200"]["selectionPattern"].is_null(),
        "case {case:?}: expected the default integration response to carry no selection pattern, got {:?}",
        read.body["integrationResponses"]["200"]
    );
    assert_eq!(
        read.body["integrationResponses"]["500"]["selectionPattern"].as_str(),
        Some("5\\d{2}"),
        "case {case:?}: expected the declared selection pattern, got {:?}",
        read.body["integrationResponses"]["500"]
    );
}

#[tokio::test]
async fn overwrite_replaces_the_integrations_the_api_already_carried() {
    // `overwrite` "replaces the API's definition, keeping only its root resource": what
    // the previous definition hung off the root — its resources, its methods and their
    // integrations — is gone, and what the imported document declares is what the API
    // carries. The row the recorded client drives: an API provisioned by hand, then
    // re-provisioned from a document.
    let case = "overwrite replaces";
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "overwritten").await;
    let original = resource_under(&gateway, &api, &root, "orig").await;
    let put_method = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{original}/methods/GET"),
            Some(serde_json::json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        put_method.status, 201,
        "case {case:?}: PutMethod expected 201, got {} with {:?}",
        put_method.status, put_method.body
    );
    let put = put_integration(
        &gateway,
        &api,
        &original,
        "GET",
        serde_json::json!({ "type": "MOCK" }),
    )
    .await;
    assert_eq!(
        put.status, 201,
        "case {case:?}: PutIntegration expected 201, got {} with {:?}",
        put.status, put.body
    );

    let body = json_definition(
        "replaced",
        "/replaced",
        "get",
        &json_operation("{\"type\": \"mock\"}"),
    );
    let answer = put_definition(&gateway, &api, Some("overwrite"), false, &body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: PutRestApi expected 200, got {} with {:?}",
        answer.status, answer.body
    );

    let paths: Vec<String> = resources_by_path(&gateway, &api, case)
        .await
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        paths,
        vec!["/".to_string(), "/replaced".to_string()],
        "case {case:?}: expected the root and the imported path only, got {paths:?}"
    );
    // The replaced resource is gone, so the integration it carried is unreachable.
    let stale = get_integration(&gateway, &api, &original, "GET").await;
    assert_failure(
        &stale,
        404,
        "NotFoundException",
        &format!("{case:?}: GetIntegration on the replaced resource"),
    );
    // And the imported method carries the integration its document declared.
    let resource = resource_at(&gateway, &api, "/replaced", case).await;
    let read = get_integration(&gateway, &api, &resource, "GET").await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: GetIntegration on the imported method expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["type"].as_str(),
        Some("MOCK"),
        "case {case:?}: expected the imported integration type, got {:?}",
        read.body["type"]
    );
}
