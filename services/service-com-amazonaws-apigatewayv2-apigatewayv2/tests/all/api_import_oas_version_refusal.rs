// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how `ImportApi` and `ReimportApi` refuse a `Body` they cannot
//! build an importer from: one whose OpenAPI (OAS) version cannot be determined.
//!
//! The rule: an OpenAPI document declares its own version — `openapi` from OAS 3.0 on,
//! `swagger` before it — so a `Body` that carries no readable version (it is not a document
//! at all, it is a scalar or a list, or it is an object declaring neither key) is refused
//! with `BadRequestException` 400 carrying API Gateway's one sentence for that class,
//! `Unable to build importer with provided input. Unable to determine OAS version`, and
//! persists nothing. A versionless body is refused the same way whichever of the two
//! operations is given it, since both read one definition reader.
//!
//! The rows span the forms a body with no determinable version arrives in: text that is no
//! document, an empty body, a JSON scalar, a JSON list (including one whose single item
//! declares the version), an object declaring neither key, an object declaring a
//! LOOK-ALIKE key, and the YAML spelling of a document this reader does not read. The row
//! that files this behaviour — a reimport of a body that is not a document — is ONE of
//! them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_import::import;
use crate::api_lifecycle::{as_str, call, emulator, error_code};
use crate::api_reimport::reimport;

/// API Gateway's refusal of a body whose OAS version it cannot determine: fixed text.
const UNDETERMINABLE_OAS_VERSION: &str =
    "Unable to build importer with provided input. Unable to determine OAS version";

/// Every body whose OAS version cannot be determined, as `(case, body)`.
fn versionless_bodies() -> Vec<(&'static str, String)> {
    vec![
        // The recorded row: a body that is not a document at all.
        ("text that is no document", "not an api definition".to_string()),
        ("an empty body", String::new()),
        ("a JSON scalar", json!("3.0.1").to_string()),
        ("a JSON number", json!(3).to_string()),
        (
            "a list whose item declares the version",
            json!([{ "openapi": "3.0.1", "info": { "title": "listed" } }]).to_string(),
        ),
        (
            "an object declaring neither openapi nor swagger",
            json!({ "info": { "title": "versionless" }, "paths": {} }).to_string(),
        ),
        (
            "an object declaring a look-alike version key",
            json!({ "oas": "3.0.1", "info": { "title": "look-alike" }, "paths": {} }).to_string(),
        ),
        (
            "the YAML spelling of a document",
            "openapi: 3.0.1\ninfo:\n  title: yaml-api\npaths: {}\n".to_string(),
        ),
    ]
}

/// How many apis the store holds, through `GetApis`.
async fn stored_api_count(router: &MultiServiceRouter) -> usize {
    let (status, body) = call(router, "GET", "/v2/apis", None).await;
    assert_eq!(status, 200, "expected 200 from GetApis, got {status} {body}");
    body["items"].as_array().map(Vec::len).unwrap_or_default()
}

/// An api to reimport over, imported from a document that declares its version.
async fn existing_api(router: &MultiServiceRouter) -> String {
    let body = json!({
        "openapi": "3.0.1",
        "info": { "title": "versioned", "version": "1.0" },
        "paths": {}
    })
    .to_string();
    let (status, imported) = import(router, "", &body).await;
    assert_eq!(
        status, 200,
        "expected 200 from ImportApi for the api to reimport over, got {status} {imported}"
    );
    as_str(&imported, "apiId")
}

#[tokio::test]
async fn a_body_with_no_determinable_oas_version_is_refused_with_aws_sentence() {
    let router = emulator();
    let api_id = existing_api(&router).await;

    for (case, body) in versionless_bodies() {
        let before = stored_api_count(&router).await;

        let (status, refused) = import(&router, "", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from ImportApi, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {case:?}: expected BadRequestException from ImportApi, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            UNDETERMINABLE_OAS_VERSION,
            "case {case:?}: expected the undeterminable-version refusal, got {refused}"
        );
        assert_eq!(
            stored_api_count(&router).await,
            before,
            "case {case:?}: a refused import must persist nothing"
        );

        // ReimportApi reads the same definition, so it refuses the same body the same way
        // — and leaves the api it was called on as it was.
        let (status, refused) = reimport(&router, &api_id, "", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from ReimportApi, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {case:?}: expected BadRequestException from ReimportApi, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            UNDETERMINABLE_OAS_VERSION,
            "case {case:?}: expected the undeterminable-version refusal, got {refused}"
        );
        let (status, read_back) = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetApi, got {status} {read_back}"
        );
        assert_eq!(
            as_str(&read_back, "name"),
            "versioned",
            "case {case:?}: a refused reimport must leave the api as it was, got {read_back}"
        );
    }
}

#[tokio::test]
async fn a_body_declaring_either_version_key_is_read() {
    let router = emulator();

    // The two keys an OpenAPI document declares its version with are both read: the
    // refusal above is about a body that declares NEITHER, not about one spelling.
    let cases: Vec<(&str, Value)> = vec![
        (
            "openapi, as OAS 3 declares it",
            json!({ "openapi": "3.0.1", "info": { "title": "oas3-api" }, "paths": {} }),
        ),
        (
            "swagger, as OAS 2 declares it",
            json!({ "swagger": "2.0", "info": { "title": "oas2-api" }, "paths": {} }),
        ),
    ];

    for (case, body) in cases {
        let (status, imported) = import(&router, "", &body.to_string()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ImportApi, got {status} {imported}"
        );
        // Read back through the service's own reader, so the import is backed by state.
        let api_id = as_str(&imported, "apiId");
        let (status, read_back) = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetApi, got {status} {read_back}"
        );
        assert_eq!(
            as_str(&read_back, "name"),
            as_str(&imported, "name"),
            "case {case:?}: GetApi must report the imported api, got {read_back}"
        );
    }
}
