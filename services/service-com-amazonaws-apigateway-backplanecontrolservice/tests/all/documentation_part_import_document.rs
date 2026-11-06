// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an `ImportDocumentationParts` body DOCUMENTS, and for the
//! text the content maps it carries are stored as.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. [`Gateway`], `create_api` and `string_member` are this crate's one wire-test
//! harness, and `import_parts`, `get_part`, `listed_entities` and `document_with` are the
//! documentation-part suites' own callers, reused rather than written a second way.
//!
//! The RULES these cases assert, each over a SET of inputs rather than one recorded call:
//!
//! * a content map an entry declares as a JSON OBJECT is stored as the SERVICE's own
//!   serialisation of that object — one member per line, two-space indentation per level
//!   of nesting, a space either side of the name/value separator, an array's items spaced
//!   inside its brackets and a space inside the brackets of an empty object or array —
//!   while one an entry carries as a JSON STRING is stored as given;
//! * an OpenAPI document documents its API's entities in its own documentation fields
//!   (`summary`, `description`), so an import writes one documentation part per entity
//!   whose node carries them, over the whole `DocumentationPartType` catalog the document
//!   can name, and none for a node that carries none;
//! * an entry lands on the part that already documents its entity, which therefore keeps
//!   its identifier, whichever `mode` the import runs in and whatever entity type the
//!   part documents; `overwrite` drops the parts whose entities the body does not
//!   document and `merge` keeps them.

use serde_json::{json, Value};

use crate::documentation_part_import_warnings::document_with;
use crate::documentation_parts::{get_part, import_parts, listed_entities};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};

/// The modelled error code a read of a documentation part the API no longer documents
/// answers with; `NotFoundException` is modelled at HTTP 404.
const NOT_FOUND: &str = "NotFoundException";

/// An import body whose documentation set is `parts`, written as TEXT so a case decides
/// the order the document declares a content map's members in — which `serde_json`'s own
/// object would sort away.
fn body_with_parts(parts: &str) -> String {
    format!(
        "{{\"openapi\":\"3.0.1\",\
          \"info\":{{\"title\":\"documentation\",\"version\":\"2026-01-01\"}},\
          \"x-amazon-apigateway-documentation\":{{\"documentationParts\":[{parts}]}}}}"
    )
}

/// The `ids` an accepted import answered with, asserting the modelled 200.
fn imported_ids(answer: &Answer, case: &str) -> Vec<String> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ids"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an ids array, got {:?}", answer.body))
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string id, got {id:?}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_content_map_an_entry_declares_as_an_object_is_stored_as_the_service_writes_it() {
    /// One content map an entry declares, beside the TEXT a read of the part reports.
    struct Row {
        case: &'static str,
        /// The `properties` member as the body writes it.
        declared: &'static str,
        /// The text the part's `properties` must carry.
        stored: &'static str,
    }
    // The rows differ along the FORM of the map the rule generalises over: how many
    // members it has, the order the document declares them in, how deeply it nests, the
    // JSON types its values carry, the characters in a value, and the empty object and
    // array — plus the one row that carries the map as a string instead, which is the
    // rule's own boundary.
    let rows = &[
        Row {
            case: "one member",
            declared: r#"{"description":"list the pets"}"#,
            stored: "{\n  \"description\" : \"list the pets\"\n}",
        },
        Row {
            // Declared summary-first, which is NOT sorted order: the stored text keeps
            // the order the document wrote the members in.
            case: "two members, the document's own order",
            declared: r#"{"summary":"Pets","description":"list the pets"}"#,
            stored: "{\n  \"summary\" : \"Pets\",\n  \"description\" : \"list the pets\"\n}",
        },
        Row {
            case: "a nested object",
            declared: r#"{"info":{"description":"the pet store"}}"#,
            stored: "{\n  \"info\" : {\n    \"description\" : \"the pet store\"\n  }\n}",
        },
        Row {
            case: "an array of values",
            declared: r#"{"tags":["pets","read"]}"#,
            stored: "{\n  \"tags\" : [ \"pets\", \"read\" ]\n}",
        },
        Row {
            case: "values that are not strings",
            declared: r#"{"deprecated":false,"order":2,"note":null}"#,
            stored: "{\n  \"deprecated\" : false,\n  \"order\" : 2,\n  \"note\" : null\n}",
        },
        Row {
            case: "a value outside ASCII",
            declared: r#"{"description":"ペットを一覧する"}"#,
            stored: "{\n  \"description\" : \"ペットを一覧する\"\n}",
        },
        Row {
            case: "the empty content map",
            declared: "{}",
            stored: "{ }",
        },
        Row {
            case: "an empty array",
            declared: r#"{"tags":[]}"#,
            stored: "{\n  \"tags\" : [ ]\n}",
        },
        Row {
            // An entry that already carries the map "encoded as a JSON string" is stored
            // as given: there is no object for the service to write out.
            case: "a map carried as a JSON string",
            declared: r#""{\"description\":\"as given\"}""#,
            stored: r#"{"description":"as given"}"#,
        },
    ];
    for row in rows {
        let case = row.case;
        let gateway = Gateway::new();
        let (api, _root) = create_api(&gateway, "documentation-content-map").await;

        let body = body_with_parts(&format!(
            "{{\"location\":{{\"type\":\"METHOD\",\"path\":\"/pets\",\"method\":\"GET\"}},\
              \"properties\":{}}}",
            row.declared
        ));
        let answer = import_parts(&gateway, &api, None, false, &body).await;
        let ids = imported_ids(&answer, case);
        assert_eq!(
            ids.len(),
            1,
            "case {case}: expected the one documented entity's identifier, got {ids:?}"
        );

        // Both readers report the text the service stored, byte for byte.
        let read = get_part(&gateway, &api, &ids[0]).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            string_member(&read.body, "properties", case),
            row.stored,
            "case {case}: expected the stored content map {:?}, got {:?}",
            row.stored,
            read.body["properties"]
        );
    }
}

#[tokio::test]
async fn every_entity_the_document_documents_becomes_a_part() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-document").await;

    // One document documenting an entity of EVERY type its own nodes can name — the API,
    // a resource, a method, each parameter kind, a request body, a response, a response
    // header, a model and an authorizer — beside nodes that document nothing (an
    // operation with neither field, a response whose description is blank), so what the
    // rule does NOT create is asserted too.
    let document = json!({
        "openapi": "3.0.1",
        "info": {
            "title": "pets",
            "version": "2026-01-01",
            "description": "the pet store",
        },
        "paths": {
            "/pets": {
                "description": "the pets resource",
                "get": {
                    "summary": "Pets",
                    "description": "list the pets",
                    "parameters": [
                        { "in": "query", "name": "page", "description": "the page to read" },
                        { "in": "header", "name": "X-Trace", "description": "the trace header" },
                        { "in": "path", "name": "petId", "description": "the pet" },
                    ],
                    "responses": {
                        "200": {
                            "description": "the pets",
                            "headers": { "X-Count": { "description": "how many pets" } },
                        },
                        "404": { "description": "no pets" },
                    },
                },
                "put": {
                    // No documentation field of its own: no METHOD part.
                    "requestBody": { "description": "the pet to store" },
                    "responses": { "200": { "description": "stored" } },
                },
            },
            "/toys": {
                "get": {
                    // Neither the operation nor its response documents anything.
                    "operationId": "listToys",
                    "responses": { "204": { "description": "   " } },
                },
            },
        },
        "components": {
            "schemas": { "Pet": { "type": "object", "description": "a pet" } },
            "securitySchemes": { "petAuth": { "type": "apiKey", "description": "the api key" } },
        },
    });
    // The extension's own entry for an entity the document ALSO documents: the declared
    // documentation set is that entity's content map.
    let declared = (
        json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
        json!({ "description": "the declared method documentation" }),
    );
    let body = document_with(&document, std::slice::from_ref(&declared));
    let case = "a document documenting one entity of every type";
    let answer = import_parts(&gateway, &api, None, false, &body).await;
    let ids = imported_ids(&answer, case);

    // Every entity the document documents, with the content map its own fields make.
    let expected: Vec<(Value, Value)> = vec![
        (
            json!({ "type": "API" }),
            json!({ "info": { "description": "the pet store" } }),
        ),
        (
            json!({ "type": "RESOURCE", "path": "/pets" }),
            json!({ "description": "the pets resource" }),
        ),
        (
            json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
            json!({ "description": "the declared method documentation" }),
        ),
        (
            json!({ "type": "QUERY_PARAMETER", "path": "/pets", "method": "GET", "name": "page" }),
            json!({ "description": "the page to read" }),
        ),
        (
            json!({ "type": "REQUEST_HEADER", "path": "/pets", "method": "GET", "name": "X-Trace" }),
            json!({ "description": "the trace header" }),
        ),
        (
            json!({ "type": "PATH_PARAMETER", "path": "/pets", "method": "GET", "name": "petId" }),
            json!({ "description": "the pet" }),
        ),
        (
            json!({ "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "200" }),
            json!({ "description": "the pets" }),
        ),
        (
            json!({
                "type": "RESPONSE_HEADER",
                "path": "/pets",
                "method": "GET",
                "statusCode": "200",
                "name": "X-Count",
            }),
            json!({ "description": "how many pets" }),
        ),
        (
            json!({ "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "404" }),
            json!({ "description": "no pets" }),
        ),
        (
            json!({ "type": "REQUEST_BODY", "path": "/pets", "method": "PUT" }),
            json!({ "description": "the pet to store" }),
        ),
        (
            json!({ "type": "RESPONSE", "path": "/pets", "method": "PUT", "statusCode": "200" }),
            json!({ "description": "stored" }),
        ),
        (
            json!({ "type": "MODEL", "name": "Pet" }),
            json!({ "description": "a pet" }),
        ),
        (
            json!({ "type": "AUTHORIZER", "name": "petAuth" }),
            json!({ "description": "the api key" }),
        ),
    ];
    let stored = listed_entities(&gateway, &api, case).await;
    let sorted = |mut pairs: Vec<(Value, Value)>| {
        pairs.sort_by_key(|(location, _)| location.to_string());
        pairs
    };
    assert_eq!(
        sorted(stored.clone()),
        sorted(expected.clone()),
        "case {case}: expected one part per documented entity, got {stored:?}"
    );
    assert_eq!(
        ids.len(),
        expected.len(),
        "case {case}: expected one identifier per documented entity, got {ids:?}"
    );
    // Nothing is documented for the nodes that carry no documentation field.
    assert!(
        stored
            .iter()
            .all(|(location, _)| location["path"] != json!("/toys")
                && !(location["type"] == json!("METHOD") && location["method"] == json!("PUT"))),
        "case {case}: expected no part for an undocumented node, got {stored:?}"
    );
}

#[tokio::test]
async fn a_method_documented_by_summary_and_description_carries_both() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-document-fields").await;

    // The two documentation fields of one node, written description-first in the
    // document: the content map names them in the order the service writes them.
    let document = json!({
        "openapi": "3.0.1",
        "info": { "title": "pets", "version": "1" },
        "paths": { "/pets": { "get": {
            "description": "list the pets",
            "summary": "Pets",
            "responses": { "200": { "description": "the pets" } },
        } } },
    });
    let case = "a method documented by both fields";
    let body = document_with(&document, &[]);
    let answer = import_parts(&gateway, &api, None, false, &body).await;
    let ids = imported_ids(&answer, case);
    assert_eq!(
        ids.len(),
        2,
        "case {case}: expected the method and its response, got {ids:?}"
    );
    let mut found = false;
    for id in &ids {
        let read = get_part(&gateway, &api, id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            read.status, read.body
        );
        if read.body["location"]["type"] == json!("METHOD") {
            found = true;
            assert_eq!(
                string_member(&read.body, "properties", case),
                "{\n  \"summary\" : \"Pets\",\n  \"description\" : \"list the pets\"\n}",
                "case {case}: expected both documentation fields, got {:?}",
                read.body["properties"]
            );
        }
    }
    assert!(
        found,
        "case {case}: expected a part for the documented method, got {ids:?}"
    );
}

#[tokio::test]
async fn an_entry_lands_on_the_part_documenting_its_entity_whatever_the_mode() {
    // Each row is an entity TYPE the re-imported entry documents — the dimension the
    // identity rule generalises over — with the location that names one of that type.
    let rows: &[(&str, Value)] = &[
        ("the api itself", json!({ "type": "API" })),
        ("a resource", json!({ "type": "RESOURCE", "path": "/pets" })),
        (
            "a method",
            json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
        ),
        (
            "a response",
            json!({ "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "200" }),
        ),
        (
            "a named entity",
            json!({ "type": "MODEL", "name": "Pet" }),
        ),
    ];
    // And each row runs in every `mode`: the documented default, `merge` spelled out and
    // `overwrite`, which differ in what SURVIVES, not in the identity of the part an entry
    // lands on.
    for (case, location) in rows {
        for (mode, keeps_the_rest) in [(None, true), (Some("merge"), true), (Some("overwrite"), false)] {
            let case = format!("{case} re-imported with mode {mode:?}");
            let gateway = Gateway::new();
            let (api, _root) = create_api(&gateway, "documentation-identity").await;

            // One part for the entity the re-import documents, and one for an entity it
            // does not.
            let untouched = json!({ "type": "AUTHORIZER", "name": "petAuth" });
            let first = [
                (location.clone(), json!({ "description": "first" })),
                (untouched.clone(), json!({ "description": "untouched" })),
            ];
            let answer = import_parts(
                &gateway,
                &api,
                None,
                false,
                &document_with(&json!({ "openapi": "3.0.1" }), &first),
            )
            .await;
            let ids = imported_ids(&answer, &case);
            assert_eq!(
                ids.len(),
                2,
                "case {case}: expected one identifier per documented entity, got {ids:?}"
            );
            let (documented, other) = (ids[0].clone(), ids[1].clone());

            let again = [(location.clone(), json!({ "description": "again" }))];
            let answer = import_parts(
                &gateway,
                &api,
                mode,
                false,
                &document_with(&json!({ "openapi": "3.0.1" }), &again),
            )
            .await;
            let reported = imported_ids(&answer, &case);
            assert_eq!(
                reported,
                vec![documented.clone()],
                "case {case}: expected the import to answer with the part's own identifier, \
                 got {reported:?}"
            );

            // The part keeps its identifier and carries the re-imported content map.
            let read = get_part(&gateway, &api, &documented).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected the part to keep its id, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                string_member(&read.body, "properties", &case),
                "{\n  \"description\" : \"again\"\n}",
                "case {case}: expected the re-imported content map, got {:?}",
                read.body["properties"]
            );

            // And the part whose entity the body does not document: kept by `merge`,
            // dropped by `overwrite`.
            let read = get_part(&gateway, &api, &other).await;
            if keeps_the_rest {
                assert_eq!(
                    read.status, 200,
                    "case {case}: expected the part the body left alone to be kept, got {} \
                     with {:?}",
                    read.status, read.body
                );
            } else {
                assert_failure(&read, 404, NOT_FOUND, &case);
            }
        }
    }
}
