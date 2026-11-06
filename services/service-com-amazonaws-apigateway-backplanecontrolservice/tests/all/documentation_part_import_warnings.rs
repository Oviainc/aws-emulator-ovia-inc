// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `ImportDocumentationParts`'s `failOnWarnings` does to an
//! import that has nothing to warn about.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`import_body`], [`import_parts`] and
//! [`listed_entities`] are the `documentation_parts` suite's own callers, reused here.
//!
//! The RULE under test, over a SET of inputs rather than one recorded call: `failOnWarnings`
//! "specif[ies] whether to rollback the documentation importation (true) or not (false)
//! WHEN A WARNING IS ENCOUNTERED". What a documentation import warns about is what the
//! OpenAPI document it reads does not hold together — a `$ref` naming a model the document
//! does not declare — so for such a body the flag decides between the modelled 400 stating
//! every warning (with the API's documentation set left exactly as it was) and an import
//! carried out that REPORTS them; for a body that earns none it decides nothing, the import
//! being carried out and leaving the same documentation set whichever way the flag and the
//! `mode` are set. An entry the import cannot STORE refuses the whole body either way,
//! because that is an error rather than a warning. The rows differ along the entity types
//! the body documents, the number of entries, the `mode`, the flag, the one entry that
//! cannot be stored, and the spelling, position, media type and verb of the model reference
//! the document does not resolve.

use serde_json::{json, Value};

use crate::authorizer_lifecycle::lambda_uri;
use crate::documentation_parts::{import_body, import_parts, listed_entities};
use crate::rest_api_lifecycle::{create_api, Gateway};

/// A documentation entry for `location` describing it with one property.
fn entry(location: Value, description: &str) -> (Value, Value) {
    (location, json!({ "description": description }))
}

/// The warnings an answered import reported, as plain strings; an absent member is no
/// warnings, because the service omits an empty list.
fn warnings(body: &Value, case: &str) -> Vec<String> {
    match &body["warnings"] {
        Value::Null => Vec::new(),
        Value::Array(reported) => reported
            .iter()
            .map(|warning| {
                warning
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected a string warning, got {warning:?}")
                    })
                    .to_string()
            })
            .collect(),
        other => panic!("case {case}: expected warnings to be a list, got {other:?}"),
    }
}

/// One import body: what it documents, and the records the API must hold for every entry
/// of it to name one.
struct Row {
    case: &'static str,
    /// The models the API holds before the import, so a `MODEL` location names one.
    models: &'static [&'static str],
    /// The authorizers the API holds before the import, for an `AUTHORIZER` location.
    authorizers: &'static [&'static str],
    entries: Vec<(Value, Value)>,
}

/// The bodies every case runs over. The rows differ along the entity types a location
/// names, along how many entries one body declares, and along an entity documented twice —
/// the dimensions an import generalises over. Each row holds the records its locations
/// NAME: an entry naming a model or an authorizer the API does not hold is the warning
/// case, which is the open report's subject, not this suite's.
fn table() -> Vec<Row> {
    vec![
        Row {
            case: "one entry for the api itself",
            models: &[],
            authorizers: &[],
            entries: vec![entry(json!({ "type": "API" }), "the api")],
        },
        Row {
            case: "entries for a resource, a method and a response",
            models: &[],
            authorizers: &[],
            entries: vec![
                entry(json!({ "type": "RESOURCE", "path": "/" }), "the root"),
                entry(
                    json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
                    "list the pets",
                ),
                entry(
                    json!({ "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "200" }),
                    "the pets",
                ),
            ],
        },
        Row {
            case: "entries naming a model and an authorizer the API holds",
            models: &["Order"],
            authorizers: &["guard"],
            entries: vec![
                entry(json!({ "type": "MODEL", "name": "Order" }), "an order"),
                entry(json!({ "type": "AUTHORIZER", "name": "guard" }), "the guard"),
            ],
        },
        Row {
            case: "one entity documented twice",
            models: &["Order"],
            authorizers: &[],
            entries: vec![
                entry(json!({ "type": "MODEL", "name": "Order" }), "first"),
                entry(json!({ "type": "MODEL", "name": "Order" }), "second"),
            ],
        },
    ]
}

/// `CreateModel` of `name`, asserting the modelled 201.
async fn create_model(gateway: &Gateway, api: &str, name: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/models"),
            Some(json!({
                "name": name,
                "contentType": "application/json",
                "schema": json!({ "type": "object" }).to_string(),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateModel({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `CreateAuthorizer` of `name`, asserting the modelled 201.
async fn create_authorizer(gateway: &Gateway, api: &str, name: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/authorizers"),
            Some(json!({
                "name": name,
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
                "authorizerUri": lambda_uri(name),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateAuthorizer({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The `(location, content map)` pairs of `entries`, as the documentation set holds them:
/// one per ENTITY, the last entry for an entity winning, which is what `merge` means.
fn documented(entries: &[(Value, Value)]) -> Vec<(Value, Value)> {
    let mut held: Vec<(Value, Value)> = Vec::new();
    for (location, properties) in entries {
        match held
            .iter_mut()
            .find(|(documented, _)| documented == location)
        {
            Some((_, existing)) => *existing = properties.clone(),
            None => held.push((location.clone(), properties.clone())),
        }
    }
    held
}

/// `pairs` in a stable order of the entity each documents, for comparing WHICH entities
/// a documentation set documents without pinning the order the collection reports them
/// in (that order is the documentation-part pagination suite's subject).
fn by_entity(mut pairs: Vec<(Value, Value)>) -> Vec<(Value, Value)> {
    pairs.sort_by_key(|(location, _)| location.to_string());
    pairs
}

#[tokio::test]
async fn fail_on_warnings_decides_nothing_for_a_body_that_warns_about_nothing() {
    for row in table() {
        // Every combination of the flag and the documented `mode` values: the outcome is
        // the same import, because nothing in these bodies warns.
        for fail_on_warnings in [false, true] {
            for mode in [None, Some("merge"), Some("overwrite")] {
                let case = format!(
                    "{} (failonwarnings {fail_on_warnings}, mode {mode:?})",
                    row.case
                );
                let gateway = Gateway::new();
                let (api, _root) = create_api(&gateway, "documentation-import-flag").await;
                // The records this row's locations NAME, so every entry of the body names
                // an entity the API holds and the body warns about nothing.
                for model in row.models {
                    create_model(&gateway, &api, model).await;
                }
                for authorizer in row.authorizers {
                    create_authorizer(&gateway, &api, authorizer).await;
                }

                let answer = import_parts(
                    &gateway,
                    &api,
                    mode,
                    fail_on_warnings,
                    &import_body(&row.entries),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the import to be carried out, got {} with {:?}",
                    answer.status, answer.body
                );
                let reported = warnings(&answer.body, &case);
                assert!(
                    reported.is_empty(),
                    "case {case}: expected no warnings, got {reported:?}"
                );
                let expected = by_entity(documented(&row.entries));
                let stored = by_entity(listed_entities(&gateway, &api, &case).await);
                assert_eq!(
                    stored, expected,
                    "case {case}: expected the imported documentation set {expected:?}, got \
                     {stored:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_model_reference_the_document_does_not_resolve_is_what_fail_on_warnings_refuses() {
    // The documentation set travels in an OpenAPI document ("to import from an OpenAPI
    // file, this is a JSON object"), so what the import warns about is what that DOCUMENT
    // does not hold together: a `$ref` naming a model it does not declare names no model.
    // What such a reference costs a DEFINITION import besides the model — the format a
    // method would have read that media type under — is not reported here, because this
    // import creates no methods; the reference reader itself is the definition importer's,
    // so one document earns one set of reference warnings whichever import reads it. The
    // rows differ along the spelling of the reference (OpenAPI 3's `content` map and
    // Swagger 2.0's bare `schema`), along where it sits (a response and a request body),
    // along the media type and the verb it is declared for, and along a reference that
    // points outside the document altogether — with one row whose reference resolves, so
    // the rule's own boundary is a row too.
    struct Row {
        case: &'static str,
        /// The document the import reads, without its documentation extension.
        document: Value,
        /// The warnings it earns, in order; empty for a document that earns none.
        warnings: &'static [&'static str],
        /// The entities the DOCUMENT itself documents, beside the extension's entry: a
        /// response object describes itself, and that description documents the API's
        /// `RESPONSE` entity at that status code.
        documents: Vec<(Value, Value)>,
    }
    let rows = vec![
        Row {
            case: "an OpenAPI 3 response schema naming a model the document does not declare",
            document: json!({
                "openapi": "3.0.1",
                "info": { "title": "warn", "version": "1" },
                "paths": { "/": { "get": { "responses": { "200": {
                    "description": "ok",
                    "content": { "application/json": {
                        "schema": { "$ref": "#/components/schemas/DoesNotExist" },
                    } },
                } } } } },
            }),
            warnings: &["Reference to model 'DoesNotExist' not found. Ignoring."],
            documents: vec![entry(
                json!({ "type": "RESPONSE", "path": "/", "method": "GET", "statusCode": "200" }),
                "ok",
            )],
        },
        Row {
            case: "an OpenAPI 3 request body of another media type and another verb",
            document: json!({
                "openapi": "3.0.1",
                "info": { "title": "warn", "version": "1" },
                "paths": { "/orders": { "post": {
                    "requestBody": { "content": { "application/xml": {
                        "schema": { "$ref": "#/components/schemas/Absent" },
                    } } },
                    "responses": { "201": { "description": "created" } },
                } } },
            }),
            warnings: &["Reference to model 'Absent' not found. Ignoring."],
            documents: vec![entry(
                json!({ "type": "RESPONSE", "path": "/orders", "method": "POST", "statusCode": "201" }),
                "created",
            )],
        },
        Row {
            case: "a Swagger 2.0 response schema, whose media type the document produces",
            document: json!({
                "swagger": "2.0",
                "info": { "title": "warn", "version": "1" },
                "produces": ["application/json"],
                "paths": { "/": { "get": { "responses": { "200": {
                    "description": "ok",
                    "schema": { "$ref": "#/definitions/Gone" },
                } } } } },
            }),
            warnings: &["Reference to model 'Gone' not found. Ignoring."],
            documents: vec![entry(
                json!({ "type": "RESPONSE", "path": "/", "method": "GET", "statusCode": "200" }),
                "ok",
            )],
        },
        Row {
            case: "a reference into another document, which an import has no access to",
            document: json!({
                "swagger": "2.0",
                "info": { "title": "warn", "version": "1" },
                "paths": { "/": { "get": { "responses": { "200": {
                    "description": "ok",
                    "schema": { "$ref": "shared.json#/definitions/Thing" },
                } } } } },
            }),
            warnings: &["Reference to model 'Thing' not found. Ignoring."],
            documents: vec![entry(
                json!({ "type": "RESPONSE", "path": "/", "method": "GET", "statusCode": "200" }),
                "ok",
            )],
        },
        Row {
            case: "a reference the document does declare",
            document: json!({
                "openapi": "3.0.1",
                "info": { "title": "warn", "version": "1" },
                "components": { "schemas": { "Order": { "type": "object" } } },
                "paths": { "/": { "get": { "responses": { "200": {
                    "description": "ok",
                    "content": { "application/json": {
                        "schema": { "$ref": "#/components/schemas/Order" },
                    } },
                } } } } },
            }),
            warnings: &[],
            documents: vec![entry(
                json!({ "type": "RESPONSE", "path": "/", "method": "GET", "statusCode": "200" }),
                "ok",
            )],
        },
    ];

    let documented = entry(json!({ "type": "API" }), "the api");
    for row in rows {
        for fail_on_warnings in [false, true] {
            let case = format!("{} (failonwarnings {fail_on_warnings})", row.case);
            let gateway = Gateway::new();
            let (api, _root) = create_api(&gateway, "documentation-import-warnings").await;
            // A documentation set the import must leave exactly as it was when it is
            // refused, so "nothing is created" is asserted against a set that is not empty.
            let seeded = import_parts(
                &gateway,
                &api,
                None,
                false,
                &import_body(std::slice::from_ref(&documented)),
            )
            .await;
            assert_eq!(
                seeded.status, 200,
                "case {case}: expected the seeded import to be carried out, got {} with {:?}",
                seeded.status, seeded.body
            );

            let imported = entry(
                json!({ "type": "RESOURCE", "path": "/" }),
                "the root resource",
            );
            let body = document_with(&row.document, std::slice::from_ref(&imported));
            let answer = import_parts(&gateway, &api, None, fail_on_warnings, &body).await;
            if row.warnings.is_empty() {
                // Nothing to warn about: the flag decides nothing and the import is
                // carried out whichever way it is set.
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the import to be carried out, got {} with {:?}",
                    answer.status, answer.body
                );
                let reported = warnings(&answer.body, &case);
                assert!(
                    reported.is_empty(),
                    "case {case}: expected no warnings, got {reported:?}"
                );
                continue;
            }
            let expected: Vec<String> =
                row.warnings.iter().map(|text| text.to_string()).collect();
            if fail_on_warnings {
                // "Whether to rollback the documentation importation (true) … when a
                // warning is encountered": the modelled 400 states every warning under the
                // fixed lead, and the API's documentation set is what it was.
                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected the import to be refused, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error.as_deref(),
                    Some("BadRequestException"),
                    "case {case}: expected BadRequestException, got {:?}",
                    answer.error
                );
                let want = refusal(&expected);
                let message = answer.body["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, want,
                    "case {case}: expected the refusal {want:?}, got {message:?}"
                );
                let after = listed_entities(&gateway, &api, &case).await;
                assert_eq!(
                    after,
                    vec![documented.clone()],
                    "case {case}: expected the refused import to change nothing, got {after:?}"
                );
            } else {
                // The same body commits, and reports the same sentences.
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the import to be carried out, got {} with {:?}",
                    answer.status, answer.body
                );
                let reported = warnings(&answer.body, &case);
                assert_eq!(
                    reported, expected,
                    "case {case}: expected the warnings {expected:?}, got {reported:?}"
                );
                // What the import left documented: the seeded part, the extension's entry
                // and the entities the document's own fields document.
                let mut expected = vec![documented.clone(), imported.clone()];
                expected.extend(row.documents.iter().cloned());
                let after = by_entity(listed_entities(&gateway, &api, &case).await);
                assert_eq!(
                    after,
                    by_entity(expected),
                    "case {case}: expected the import to be carried out, got {after:?}"
                );
            }
        }
    }
}

/// `document` carrying `entries` as its documentation set, as the body of an import: the
/// same extension [`import_body`] writes, over a document whose own members the row chose.
pub(crate) fn document_with(document: &Value, entries: &[(Value, Value)]) -> String {
    let parts: Vec<Value> = entries
        .iter()
        .map(|(location, properties)| json!({ "location": location, "properties": properties }))
        .collect();
    let mut body = document.clone();
    body["x-amazon-apigateway-documentation"] = json!({
        "version": "1.0.0",
        "documentationParts": parts,
    });
    body.to_string()
}

/// The refusal `failOnWarnings` earns for an import that reported `warnings`: the fixed
/// lead, then every reported diagnostic on its own indented line — the same refusal the
/// sibling definition import answers with, because both read one document through one
/// reader.
fn refusal(warnings: &[String]) -> String {
    let mut message = String::from("Warnings found during import:");
    for warning in warnings {
        message.push_str("\n\t");
        message.push_str(warning);
    }
    message
}

#[tokio::test]
async fn an_entry_the_import_cannot_store_refuses_the_whole_body_either_way() {
    // An ERROR is not a warning: the flag does not decide it, so the same body is refused
    // with the modelled 400 whichever way the flag and the mode are set, and the
    // documentation set a previous import left is untouched.
    let unstorable: &[(&str, Value)] = &[
        (
            "a location type the model does not declare",
            json!({ "type": "NOT_AN_ENTITY" }),
        ),
        (
            "a location member the entity type does not make valid",
            json!({ "type": "API", "path": "/pets" }),
        ),
        (
            "a status code outside the declared pattern",
            json!({ "type": "RESPONSE", "path": "/", "method": "*", "statusCode": "600" }),
        ),
    ];
    for (case, location) in unstorable {
        for fail_on_warnings in [false, true] {
            for mode in [None, Some("merge"), Some("overwrite")] {
                let case = format!("{case} (failonwarnings {fail_on_warnings}, mode {mode:?})");
                let gateway = Gateway::new();
                let (api, _root) = create_api(&gateway, "documentation-import-errors").await;
                let standing = entry(json!({ "type": "API" }), "the api");
                let seeded = import_parts(
                    &gateway,
                    &api,
                    None,
                    false,
                    &import_body(std::slice::from_ref(&standing)),
                )
                .await;
                assert_eq!(
                    seeded.status, 200,
                    "case {case}: expected the seeded import to be carried out, got {} with \
                     {:?}",
                    seeded.status, seeded.body
                );

                let body = import_body(&[
                    entry(json!({ "type": "RESOURCE", "path": "/" }), "the root"),
                    entry(location.clone(), "the unstorable entry"),
                ]);
                let answer = import_parts(&gateway, &api, mode, fail_on_warnings, &body).await;
                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected 400, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error.as_deref(),
                    Some("BadRequestException"),
                    "case {case}: expected BadRequestException, got {:?}",
                    answer.error
                );
                let after = listed_entities(&gateway, &api, &case).await;
                assert_eq!(
                    after,
                    vec![standing.clone()],
                    "case {case}: expected the refused import to change nothing, got {after:?}"
                );
            }
        }
    }
}
