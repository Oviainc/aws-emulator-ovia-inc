// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateDocumentationPart`.
//!
//! Every case drives the real wire path — a `restJson1` `PATCH
//! /restapis/{restApiId}/documentation/parts/{documentationPartId}` through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], [`Answer`], `create_api`,
//! `string_member`, `page_items` and `assert_failure` are this crate's one wire-test
//! harness, reused from `rest_api_lifecycle`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a patch of `/properties` stores the op's value as the part's content map, over a
//!   TABLE of content maps differing along what the map holds (count, nesting, value
//!   type, text, size) and over both ops that carry a value (`add`, `replace`), each read
//!   back through `GetDocumentationPart` and `GetDocumentationParts`;
//! * a path naming no updateable member of the `DocumentationPart` shape is refused, over
//!   a table of paths (the service-assigned `id`, each member of the identifying
//!   `location`, a sub-member of the single-valued content map, an unknown member, a
//!   pointer that is not one);
//! * an op the resource does not support is refused, over `remove` (the content map is
//!   required) and every op that names nothing on such a member (`move`, `copy`, `test`);
//! * a value that is not "a content map ... encoded as a JSON string" is refused, over a
//!   table of JSON forms, and an `add`/`replace` carrying no value at all;
//! * a refusal changes nothing — the stored part is re-read after each one, and a list
//!   whose LATER op is refused leaves the whole list unapplied;
//! * an update of an API or a part that does not exist is the modelled 404, including a
//!   part id addressed through another API.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};

/// The modelled error codes these cases assert.
const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";

/// The content map the parts under test are created with, as the JSON string
/// `DocumentationPart$properties` documents ("The map must be encoded as a JSON string").
const ORIGINAL_PROPERTIES: &str = "{\"description\":\"the original content map\"}";

/// The location the parts under test document; one entity per test API, so a case never
/// collides with another's part.
fn location() -> Value {
    json!({ "type": "METHOD", "path": "/pets", "method": "GET" })
}

/// `CreateDocumentationPart`, asserting the modelled 201 and returning the part's id.
async fn create_part(gateway: &Gateway, api: &str, properties: &str) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/documentation/parts"),
            Some(json!({ "location": location(), "properties": properties })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateDocumentationPart({properties:?}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", "CreateDocumentationPart")
}

/// One API with one documentation part on it, as every case below starts.
async fn api_with_part(gateway: &Gateway, name: &str) -> (String, String) {
    let (api, _root) = create_api(gateway, name).await;
    let part = create_part(gateway, &api, ORIGINAL_PROPERTIES).await;
    (api, part)
}

/// `UpdateDocumentationPart` with `patches` sent verbatim as `patchOperations`.
async fn update_part(gateway: &Gateway, api: &str, part: &str, patches: Value) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/documentation/parts/{part}"),
            Some(json!({ "patchOperations": patches })),
        )
        .await
}

/// One `patchOperations` entry, with `value` present only when the case carries one.
fn patch(op: &str, path: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "op": op, "path": path, "value": value }),
        None => json!({ "op": op, "path": path }),
    }
}

/// `GetDocumentationPart` for one part.
async fn get_part(gateway: &Gateway, api: &str, part: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/documentation/parts/{part}"),
            None,
        )
        .await
}

/// The content map `GetDocumentationPart` reports for `part`, asserting the modelled 200.
async fn read_properties(gateway: &Gateway, api: &str, part: &str, case: &str) -> String {
    let answer = get_part(gateway, api, part).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: GetDocumentationPart expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "properties", case)
}

/// The content map `GetDocumentationParts` reports for `part`: the same record read
/// through the collection, so an update is not only visible to the single reader.
async fn listed_properties(gateway: &Gateway, api: &str, part: &str, case: &str) -> String {
    let answer = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/documentation/parts?limit=500"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: GetDocumentationParts expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let items = page_items(&answer.body, case);
    let listed = items
        .iter()
        .find(|item| item["id"].as_str() == Some(part))
        .unwrap_or_else(|| panic!("case {case}: expected the part {part} to be listed, got {items:?}"));
    string_member(listed, "properties", case)
}

/// Assert `answer` reports the part member for member: the id and the location it was
/// created with, and `properties` as the patch set it.
fn assert_reported_part(answer: &Answer, part: &str, properties: &str, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["id"].as_str(),
        Some(part),
        "case {case}: expected id {part:?}, got {:?}",
        answer.body["id"]
    );
    assert_eq!(
        answer.body["location"],
        location(),
        "case {case}: expected location {:?}, got {:?}",
        location(),
        answer.body["location"]
    );
    assert_eq!(
        answer.body["properties"].as_str(),
        Some(properties),
        "case {case}: expected properties {properties:?}, got {:?}",
        answer.body["properties"]
    );
}

/// The content maps a patch must be able to set, differing along what the map holds: no
/// entry, one entry, several entries, a nested object, an array, a non-string value, text
/// outside ASCII, text carrying JSON escapes, and a map far larger than the recorded one.
/// Each row IS "a content map ... encoded as a JSON string"
/// (`DocumentationPart$properties`).
fn content_maps() -> Vec<(&'static str, String)> {
    vec![
        ("an empty content map", "{}".to_string()),
        (
            "one entry",
            json!({ "description": "list the pets" }).to_string(),
        ),
        (
            "several entries",
            json!({ "description": "list the pets", "summary": "pets", "tags": "pet" })
                .to_string(),
        ),
        (
            "a nested object",
            json!({ "schema": { "type": "object", "required": ["id"] } }).to_string(),
        ),
        (
            "an array-valued entry",
            json!({ "produces": ["application/json", "application/xml"] }).to_string(),
        ),
        (
            "non-string values",
            json!({ "deprecated": true, "order": 3, "ratio": 0.5, "none": Value::Null })
                .to_string(),
        ),
        (
            "text outside ASCII",
            json!({ "description": "ロボット — les animaux" }).to_string(),
        ),
        (
            "text carrying JSON escapes",
            json!({ "description": "a \"quoted\" line\nand a tab\there" }).to_string(),
        ),
        (
            "a map far larger than one entry",
            json!({ "description": "x".repeat(4096) }).to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_patch_of_properties_stores_the_content_map_it_carries() {
    let gateway = Gateway::new();
    let (api, part) = api_with_part(&gateway, "update-documentation-part-properties").await;

    // Both ops that carry a value set the member: `value` is "applicable for the add or
    // replace operation" (`PatchOperation$value`).
    for op in ["replace", "add"] {
        for (row, properties) in content_maps() {
            let case = format!("op={op}, {row}");
            let answer = update_part(
                &gateway,
                &api,
                &part,
                json!([patch(op, "/properties", Some(&properties))]),
            )
            .await;
            assert_reported_part(&answer, &part, &properties, &case);

            // Round trip: the patched part is read back through both implemented readers,
            // not just reported by the writer.
            let read = read_properties(&gateway, &api, &part, &case).await;
            assert_eq!(
                read, properties,
                "case {case}: GetDocumentationPart expected {properties:?}, got {read:?}"
            );
            let listed = listed_properties(&gateway, &api, &part, &case).await;
            assert_eq!(
                listed, properties,
                "case {case}: GetDocumentationParts expected {properties:?}, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_list_of_ops_is_applied_in_order_and_an_empty_list_changes_nothing() {
    let gateway = Gateway::new();
    let (api, part) = api_with_part(&gateway, "update-documentation-part-lists").await;

    // Each row is a list of content maps to set in one request; the request's answer is
    // the LAST one, because the ops are applied in request order.
    let rows: &[&[&str]] = &[
        &[],
        &["{\"description\":\"only op\"}"],
        &["{\"description\":\"first\"}", "{\"description\":\"second\"}"],
        &[
            "{\"description\":\"one\"}",
            "{}",
            "{\"description\":\"three\",\"summary\":\"s\"}",
        ],
    ];
    let mut expected = ORIGINAL_PROPERTIES.to_string();
    for values in rows {
        let case = format!("a list of {} ops", values.len());
        let patches: Vec<Value> = values
            .iter()
            .map(|value| patch("replace", "/properties", Some(value)))
            .collect();
        if let Some(last) = values.last() {
            expected = (*last).to_string();
        }
        let answer = update_part(&gateway, &api, &part, json!(patches)).await;
        assert_reported_part(&answer, &part, &expected, &case);
        let read = read_properties(&gateway, &api, &part, &case).await;
        assert_eq!(
            read, expected,
            "case {case}: GetDocumentationPart expected {expected:?}, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_path_naming_no_updateable_member_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, part) = api_with_part(&gateway, "update-documentation-part-paths").await;

    // Each row is a path that names no updateable member of the `DocumentationPart`
    // shape: the service-assigned `id`, the identifying `location` and each of its
    // members, a sub-member of the single-valued content map, a member the shape does not
    // declare, and a `path` that is not a JSON Pointer into the resource at all.
    let refused: &[&str] = &[
        "/id",
        "/location",
        "/location/type",
        "/location/path",
        "/location/method",
        "/location/statusCode",
        "/location/name",
        "/properties/description",
        "/properties/schema/type",
        "/unknown",
        "/",
        "",
        "properties",
    ];
    for path in refused {
        let case = format!("replace {path:?}");
        let answer = update_part(
            &gateway,
            &api,
            &part,
            json!([patch("replace", path, Some("{\"description\":\"changed\"}"))]),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, &case);
        let read = read_properties(&gateway, &api, &part, &case).await;
        assert_eq!(
            read, ORIGINAL_PROPERTIES,
            "case {case}: the refusal changed the stored content map to {read:?}"
        );
    }
}

#[tokio::test]
async fn an_op_the_content_map_does_not_support_is_refused() {
    let gateway = Gateway::new();
    let (api, part) = api_with_part(&gateway, "update-documentation-part-ops").await;

    // `remove` is refused because the content map is a member a stored part always has
    // (`properties` is required on `CreateDocumentationPartRequest`); `move`, `copy` and
    // `test` name nothing on a single-valued member.
    let refused: &[(&str, Option<&str>)] = &[
        ("remove", None),
        ("remove", Some("{\"description\":\"changed\"}")),
        ("move", Some("{}")),
        ("copy", Some("{}")),
        ("test", Some(ORIGINAL_PROPERTIES)),
    ];
    for (op, value) in refused {
        let case = format!("op={op:?} on /properties");
        let answer = update_part(
            &gateway,
            &api,
            &part,
            json!([patch(op, "/properties", *value)]),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, &case);
        let read = read_properties(&gateway, &api, &part, &case).await;
        assert_eq!(
            read, ORIGINAL_PROPERTIES,
            "case {case}: the refusal changed the stored content map to {read:?}"
        );
    }

    // An entry the handler cannot read at all is the same 400: the model declares `op`
    // and `path` optional, so an entry carrying neither reaches the handler.
    let incomplete: &[(&str, Value)] = &[
        ("no op", json!({ "path": "/properties", "value": "{}" })),
        ("no path", json!({ "op": "replace", "value": "{}" })),
        ("neither op nor path", json!({})),
    ];
    for (case, entry) in incomplete {
        let answer = update_part(&gateway, &api, &part, json!([entry.clone()])).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        let read = read_properties(&gateway, &api, &part, case).await;
        assert_eq!(
            read, ORIGINAL_PROPERTIES,
            "case {case}: the refusal changed the stored content map to {read:?}"
        );
    }

    // An op OUTSIDE the modelled `Op` value set is refused with the same 400
    // `BadRequestException` this operation's own refusals carry — the rule the sibling
    // `patch_updates` suite asserts for every `Update*` operation of this service.
    for op in ["delete", "upsert", ""] {
        let case = format!("an off-model op {op:?}");
        let answer = update_part(
            &gateway,
            &api,
            &part,
            json!([patch(op, "/properties", Some("{}"))]),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, &case);
        let read = read_properties(&gateway, &api, &part, &case).await;
        assert_eq!(
            read, ORIGINAL_PROPERTIES,
            "case {case}: the refusal changed the stored content map to {read:?}"
        );
    }
}

#[tokio::test]
async fn a_value_that_is_not_a_content_map_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, part) = api_with_part(&gateway, "update-documentation-part-values").await;

    // Each row is a value that is not "a content map ... encoded as a JSON string"
    // (`DocumentationPart$properties`), differing along the JSON form it carries instead:
    // nothing, text that is not JSON, truncated JSON, and each JSON type that is not an
    // object. The same rows `CreateDocumentationPart` refuses, because one function reads
    // the member for every operation that takes it.
    let refused: &[&str] = &[
        "",
        "not json",
        "{",
        "{\"description\": }",
        "null",
        "42",
        "true",
        "\"text\"",
        "[1, 2]",
        "[{\"description\": \"in an array\"}]",
    ];
    for value in refused {
        for op in ["replace", "add"] {
            let case = format!("op={op}, value={value:?}");
            let answer = update_part(
                &gateway,
                &api,
                &part,
                json!([patch(op, "/properties", Some(value))]),
            )
            .await;
            assert_failure(&answer, 400, BAD_REQUEST, &case);
            let read = read_properties(&gateway, &api, &part, &case).await;
            assert_eq!(
                read, ORIGINAL_PROPERTIES,
                "case {case}: the refusal changed the stored content map to {read:?}"
            );
        }
    }

    // An `add` or `replace` carrying no value at all has nothing to set the member to.
    for op in ["replace", "add"] {
        let case = format!("op={op}, no value");
        let answer = update_part(&gateway, &api, &part, json!([patch(op, "/properties", None)])).await;
        assert_failure(&answer, 400, BAD_REQUEST, &case);
        let read = read_properties(&gateway, &api, &part, &case).await;
        assert_eq!(
            read, ORIGINAL_PROPERTIES,
            "case {case}: the refusal changed the stored content map to {read:?}"
        );
    }
}

#[tokio::test]
async fn a_list_whose_later_op_is_refused_leaves_the_part_untouched() {
    let gateway = Gateway::new();
    let (api, part) = api_with_part(&gateway, "update-documentation-part-atomicity").await;

    let applied = "{\"description\":\"would be applied\"}";
    // Each row is a second op that is refused for a different reason, after an op that
    // would have been applied: an unsupported path, an unsupported op, and a value that
    // is not a content map.
    let rows: &[(&str, Value)] = &[
        (
            "an unsupported path",
            patch("replace", "/id", Some("another-id")),
        ),
        ("an unsupported op", patch("remove", "/properties", None)),
        (
            "a value that is not a content map",
            patch("replace", "/properties", Some("not json")),
        ),
    ];
    for (case, refused) in rows {
        let answer = update_part(
            &gateway,
            &api,
            &part,
            json!([
                patch("replace", "/properties", Some(applied)),
                refused.clone()
            ]),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        let read = read_properties(&gateway, &api, &part, case).await;
        assert_eq!(
            read, ORIGINAL_PROPERTIES,
            "case {case}: the first op of a refused list reached the store as {read:?}"
        );
    }
}

#[tokio::test]
async fn an_update_of_a_record_that_does_not_exist_is_not_found() {
    let gateway = Gateway::new();
    let (api, part) = api_with_part(&gateway, "update-documentation-part-missing").await;
    // A second API with no documentation part of its own, so a part id addressed through
    // the wrong API is a miss rather than a cross-API read.
    let (other, _root) = create_api(&gateway, "update-documentation-part-other").await;

    // Each row is an identifier pair naming no stored part: an API that was never
    // created, a part id that was never minted, a part id addressed through another API,
    // and the API's own id used as a part id.
    let rows: &[(&str, &str, &str)] = &[
        ("an unknown API", "zzzzzzzzzz", part.as_str()),
        ("an unknown part", api.as_str(), "zzzzzzzzzz"),
        ("a part of another API", other.as_str(), part.as_str()),
        ("the API id used as a part id", api.as_str(), api.as_str()),
    ];
    for (case, rest_api, documentation_part) in rows {
        let answer = update_part(
            &gateway,
            rest_api,
            documentation_part,
            json!([patch("replace", "/properties", Some("{}"))]),
        )
        .await;
        assert_failure(&answer, 404, NOT_FOUND, case);
    }

    // None of those refusals touched the part that does exist.
    let read = read_properties(&gateway, &api, &part, "after the not-found cases").await;
    assert_eq!(
        read, ORIGINAL_PROPERTIES,
        "case after the not-found cases: expected {ORIGINAL_PROPERTIES:?}, got {read:?}"
    );
}
