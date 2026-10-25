// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the non-fatal diagnostic an API definition import reports for a
//! method that declares a security requirement the import could not turn into an
//! authorizer, over `PutRestApi` and `ImportRestApi`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is the `warnings`
//! list a client reads, not what a handler returns. The emulator and the `CreateRestApi`
//! helper are this crate's existing harness, reused from `rest_api_lifecycle`.
//!
//! The rule under test: a method the import leaves OPEN because it specified security
//! while no custom authorizer was created and the method carried no
//! `x-amazon-apigateway-auth` extension is reported with the importer's fixed sentence,
//! whose load-bearing clause is the OUTCOME — the method will not be secured — with the
//! method's verb and the resource path taken from the request. The rows differ along what
//! the rule generalises over: the verb, the resource path, the spelling of the declared
//! scheme set (Swagger 2.0's `securityDefinitions` and OpenAPI 3's
//! `components.securitySchemes`), the scheme's type, where the requirement is declared
//! (on the operation or across the document), and the operation that carries the
//! definition. When the caller asks to roll back on a warning, the refusal states those
//! same sentences, one per line under the importer's fixed lead.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, page_items, string_member, Gateway};

/// The security scheme types an import carries over as an `Authorizer`, so a scheme of any
/// OTHER type is reported with the sibling "unsupported security definition type" warning.
/// Source: the `type` values of a `securityDefinitions` /
/// `components.securitySchemes` entry API Gateway has an authorizer for — the `apiKey`
/// scheme a key or custom authorizer is declared as, and the `oauth2` scheme a user-pool
/// authorizer is.
const IMPORTABLE_SCHEME_TYPES: &[&str] = &["apiKey", "oauth2"];

/// AWS's diagnostic for an imported method that specified security while the import
/// created no custom authorizer and the method carried no `x-amazon-apigateway-auth`
/// extension: a fixed sentence whose variable parts are the method's verb and the resource
/// path of the declaring path item.
fn unsecured_warning(verb: &str, path: &str) -> String {
    format!(
        "Method '{verb}' on resource '{path}' specified security, but no custom authorizers \
         were created and the extension x-amazon-apigateway-auth was not properly set. This \
         method will be not be secured."
    )
}

/// The sibling diagnostic for a declared scheme whose type no authorizer covers.
fn unsupported_scheme_warning(kind: &str, name: &str) -> String {
    format!("Unsupported security definition type '{kind}' for '{name}'. Ignoring.")
}

/// One row of the table: a definition that declares a security scheme and a method under a
/// security requirement.
struct Row {
    /// The case's name, carried into every assertion.
    case: &'static str,
    /// Swagger 2.0 (`securityDefinitions`) when true, OpenAPI 3
    /// (`components.securitySchemes`) when false.
    swagger: bool,
    /// The declared scheme's `type`.
    scheme_type: &'static str,
    /// The name the scheme is declared under.
    scheme_name: &'static str,
    /// Whether the requirement is declared on the OPERATION; otherwise it is the
    /// DOCUMENT's, which every operation declaring none of its own is imported under.
    on_operation: bool,
    /// The path the method is declared on.
    path: &'static str,
    /// The Path Item Object key the operation is declared under.
    operation_key: &'static str,
    /// The HTTP verb that key becomes.
    verb: &'static str,
    /// Whether the operation carries the `x-amazon-apigateway-auth` extension, which is
    /// the other way a method keeps its authorization.
    auth_extension: bool,
}

/// The scheme object a `type` is declared with, carrying the members that type needs.
fn scheme_object(kind: &str) -> Value {
    match kind {
        "apiKey" => json!({"type": "apiKey", "name": "x-key", "in": "header"}),
        "oauth2" => json!({"type": "oauth2", "flows": {}}),
        "http" => json!({"type": "http", "scheme": "bearer"}),
        other => json!({"type": other}),
    }
}

/// The definition document a row declares.
fn definition(row: &Row) -> String {
    let requirement = json!([{ row.scheme_name: [] }]);
    let mut operation = json!({"responses": {"200": {"description": "ok"}}});
    if row.on_operation {
        operation["security"] = requirement.clone();
    }
    if row.auth_extension {
        operation["x-amazon-apigateway-auth"] = json!({"type": "none"});
    }
    let mut document = if row.swagger {
        json!({
            "swagger": "2.0",
            "info": {"title": row.case, "version": "2026-01-01"},
            "securityDefinitions": { row.scheme_name: scheme_object(row.scheme_type) },
        })
    } else {
        json!({
            "openapi": "3.0.1",
            "info": {"title": row.case, "version": "2026-01-01"},
            "components": {
                "securitySchemes": { row.scheme_name: scheme_object(row.scheme_type) },
            },
        })
    };
    if !row.on_operation {
        document["security"] = requirement;
    }
    document["paths"] = json!({ row.path: { row.operation_key: operation } });
    document.to_string()
}

/// The warnings a row's import must report, in the order the reader produces them: the
/// scheme's own diagnostic (read from the declared scheme set) ahead of the method's.
fn expected_warnings(row: &Row) -> Vec<String> {
    let mut expected = Vec::new();
    if !IMPORTABLE_SCHEME_TYPES.contains(&row.scheme_type) {
        expected.push(unsupported_scheme_warning(row.scheme_type, row.scheme_name));
    }
    if !row.auth_extension {
        expected.push(unsecured_warning(row.verb, row.path));
    }
    expected
}

/// The `warnings` of an answered import, as plain strings. An absent member is no
/// warnings: the service omits an empty list.
fn warnings(body: &Value, case: &str) -> Vec<String> {
    match &body["warnings"] {
        Value::Null => Vec::new(),
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a string warning, got {entry:?}"))
                    .to_owned()
            })
            .collect(),
        other => panic!("case {case}: expected warnings to be a list, got {other:?}"),
    }
}

/// The table every case in this suite runs over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a basic scheme a swagger document declares on the operation",
            swagger: true,
            scheme_type: "basic",
            scheme_name: "myAuth",
            on_operation: true,
            path: "/thing",
            operation_key: "get",
            verb: "GET",
            auth_extension: false,
        },
        Row {
            case: "an apiKey scheme an openapi document declares on the operation",
            swagger: false,
            scheme_type: "apiKey",
            scheme_name: "keyAuth",
            on_operation: true,
            path: "/pets/{petId}",
            operation_key: "post",
            verb: "POST",
            auth_extension: false,
        },
        Row {
            case: "an oauth2 scheme required across the document",
            swagger: false,
            scheme_type: "oauth2",
            scheme_name: "poolAuth",
            on_operation: false,
            path: "/a/b/c",
            operation_key: "delete",
            verb: "DELETE",
            auth_extension: false,
        },
        Row {
            case: "an http scheme required across a swagger document, on the catch-all verb",
            swagger: true,
            scheme_type: "http",
            scheme_name: "bearerAuth",
            on_operation: false,
            path: "/catch",
            operation_key: "x-amazon-apigateway-any-method",
            verb: "ANY",
            auth_extension: false,
        },
        Row {
            case: "an openid scheme declared on a patch operation",
            swagger: false,
            scheme_type: "openIdConnect",
            scheme_name: "oidcAuth",
            on_operation: true,
            path: "/deep/nested/thing",
            operation_key: "patch",
            verb: "PATCH",
            auth_extension: false,
        },
        Row {
            case: "a method whose authorization comes from the auth extension",
            swagger: false,
            scheme_type: "apiKey",
            scheme_name: "keyAuth",
            on_operation: true,
            path: "/extended",
            operation_key: "get",
            verb: "GET",
            auth_extension: true,
        },
    ]
}

/// The resource of `api` whose full path is `path`.
async fn resource_id(gateway: &Gateway, api: &str, path: &str, case: &str) -> String {
    let answer = gateway
        .call("GET", &format!("/restapis/{api}/resources"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: GetResources expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let item = page_items(&answer.body, case)
        .into_iter()
        .find(|item| item["path"].as_str() == Some(path))
        .unwrap_or_else(|| panic!("case {case}: expected an imported resource at {path:?}"));
    string_member(&item, "id", case)
}

#[tokio::test]
async fn an_import_reports_a_method_it_left_unsecured_with_the_outcome_in_the_warning() {
    let gateway = Gateway::new();
    for row in rows() {
        let body = definition(&row);
        let (api, _root) = create_api(&gateway, row.case).await;
        let answer = gateway
            .call_raw("PUT", &format!("/restapis/{api}?mode=overwrite"), &body)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {}: PutRestApi expected 200, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        let reported = warnings(&answer.body, row.case);
        let expected = expected_warnings(&row);
        assert_eq!(
            reported, expected,
            "case {}: expected the warnings {expected:?}, got {reported:?}",
            row.case
        );

        // Round trip: the method the warning is about is a readable record, and it is
        // exactly as open as the warning says — no authorization and no authorizer.
        let resource = resource_id(&gateway, &api, row.path, row.case).await;
        let read = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/resources/{resource}/methods/{}", row.verb),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {}: GetMethod({}) expected 200, got {} with {:?}",
            row.case, row.verb, read.status, read.body
        );
        assert_eq!(
            read.body["authorizationType"].as_str(),
            Some("NONE"),
            "case {}: expected the imported method to carry no authorization, got {:?}",
            row.case,
            read.body["authorizationType"]
        );
        assert!(
            read.body["authorizerId"].is_null(),
            "case {}: expected no authorizer on the imported method, got {:?}",
            row.case,
            read.body["authorizerId"]
        );
    }
}

#[tokio::test]
async fn the_same_definition_imported_as_a_new_api_reports_the_same_warning() {
    // `ImportRestApi` reads the definition through the same reader as `PutRestApi`, so the
    // diagnostics a document earns cannot differ between the two.
    let gateway = Gateway::new();
    for row in rows() {
        let body = definition(&row);
        let answer = gateway
            .call_raw("POST", "/restapis?mode=import", &body)
            .await;
        assert_eq!(
            answer.status, 201,
            "case {}: ImportRestApi expected 201, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        let reported = warnings(&answer.body, row.case);
        let expected = expected_warnings(&row);
        assert_eq!(
            reported, expected,
            "case {}: expected the warnings {expected:?}, got {reported:?}",
            row.case
        );
    }
}

/// The refusal `failonwarnings` earns for a definition that reported `warnings`: the fixed
/// lead, then every reported diagnostic on its own indented line.
fn expected_refusal(warnings: &[String]) -> String {
    let mut message = String::from("Warnings found during import:");
    for warning in warnings {
        message.push_str("\n\t");
        message.push_str(warning);
    }
    message
}

#[tokio::test]
async fn failonwarnings_refuses_the_import_stating_the_unsecured_method() {
    // `failonwarnings` rolls the import back and states what was reported, so the same
    // sentences reach the caller through the refusal — on either operation that takes a
    // definition, which read it through the one reader.
    let gateway = Gateway::new();
    for row in rows() {
        let expected = expected_warnings(&row);
        let body = definition(&row);
        let (api, _root) = create_api(&gateway, row.case).await;
        let routes = [
            ("PutRestApi", "PUT", format!("/restapis/{api}?mode=overwrite&failonwarnings=true")),
            (
                "ImportRestApi",
                "POST",
                "/restapis?mode=import&failonwarnings=true".to_string(),
            ),
        ];
        for (operation, verb, uri) in routes {
            let case = format!("{} via {operation}", row.case);
            let answer = gateway.call_raw(verb, &uri, &body).await;
            if expected.is_empty() {
                assert!(
                    answer.status == 200 || answer.status == 201,
                    "case {case}: a definition with nothing to report is served with \
                     failonwarnings, got {} with {:?}",
                    answer.status,
                    answer.body
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {case}: expected failonwarnings to refuse the import, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("BadRequestException"),
                "case {case}: expected BadRequestException, got {:?}",
                answer.error
            );
            let message = answer.body["message"].as_str().unwrap_or_default();
            let want = expected_refusal(&expected);
            assert_eq!(
                message, want,
                "case {case}: expected the refusal {want:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_reported_sentences_are_written_out_in_full() {
    // One document whose answer is written out VERBATIM — the list a client reads and the
    // refusal `failonwarnings` earns — rather than assembled the way this suite's row
    // builders (or the service) assemble them, so a wording change made in both the
    // service and those builders still fails here.
    let gateway = Gateway::new();
    let body = "{\"swagger\": \"2.0\", \"info\": {\"title\": \"verbatim\"}, \
         \"securityDefinitions\": {\"myAuth\": {\"type\": \"basic\"}}, \
         \"paths\": {\"/thing\": {\"get\": {\"security\": [{\"myAuth\": []}], \
             \"responses\": {\"200\": {\"description\": \"ok\"}}}}}}";

    let (api, _root) = create_api(&gateway, "verbatim").await;
    let answer = gateway
        .call_raw("PUT", &format!("/restapis/{api}?mode=overwrite"), body)
        .await;
    assert_eq!(
        answer.status, 200,
        "case verbatim: PutRestApi expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let reported = warnings(&answer.body, "verbatim");
    let want = vec![
        "Unsupported security definition type 'basic' for 'myAuth'. Ignoring.".to_string(),
        "Method 'GET' on resource '/thing' specified security, but no custom authorizers were \
         created and the extension x-amazon-apigateway-auth was not properly set. This method \
         will be not be secured."
            .to_string(),
    ];
    assert_eq!(
        reported, want,
        "case verbatim: expected the warnings {want:?}, got {reported:?}"
    );

    let (rolled_back, _root) = create_api(&gateway, "verbatim refusal").await;
    let refused = gateway
        .call_raw(
            "PUT",
            &format!("/restapis/{rolled_back}?mode=overwrite&failonwarnings=true"),
            body,
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case verbatim refusal: expected 400, got {} with {:?}",
        refused.status, refused.body
    );
    let message = refused.body["message"].as_str().unwrap_or_default();
    let want_message = "Warnings found during import:\n\tUnsupported security definition type \
                        'basic' for 'myAuth'. Ignoring.\n\tMethod 'GET' on resource '/thing' \
                        specified security, but no custom authorizers were created and the \
                        extension x-amazon-apigateway-auth was not properly set. This method \
                        will be not be secured.";
    assert_eq!(
        message, want_message,
        "case verbatim refusal: expected the refusal {want_message:?}, got {message:?}"
    );
}
