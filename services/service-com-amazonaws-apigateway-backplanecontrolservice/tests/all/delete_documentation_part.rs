// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDocumentationPart`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. [`Gateway`], [`Answer`], `create_api`, `string_member`, `page_items` and
//! `assert_failure` are this crate's one wire-test harness, reused from
//! `rest_api_lifecycle`.
//!
//! The rule these cases assert, over a SET of inputs rather than one recorded call: a
//! delete of a subordinate record removes EXACTLY the record its identifiers name and
//! nothing else — every other part of the API is still reported member for member and
//! the removed one is gone from every reader — while a request whose identifiers name no
//! stored record is the modelled `NotFoundException` and changes nothing.
//!
//! * [`deleting_a_part_removes_exactly_that_part_from_every_reader`] runs a table of
//!   collections that differ in size, in the entity types their locations target and in
//!   which part of the creation order is deleted;
//! * [`a_delete_that_names_no_stored_record_is_not_found_and_changes_nothing`] runs a
//!   table of identifier pairs that address nothing — an unknown API, an unknown part, a
//!   part of another API, and a part already deleted;
//! * [`a_deleted_location_is_free_for_a_new_part`] asserts the record really left the
//!   store: the location that was a conflict while the part existed is creatable again
//!   after the delete.

use serde_json::{json, Map, Value};

use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};

/// The modelled error codes these cases assert.
const CONFLICT: &str = "ConflictException";
const NOT_FOUND: &str = "NotFoundException";

/// The modelled success status of `DeleteDocumentationPart`
/// (`smithy_query(query="operation", operation="DeleteDocumentationPart")` reports
/// `http.code` 202).
const DELETE_STATUS: u16 = 202;

/// The location members each entity type used below makes VALID.
///
/// Source: `DocumentationPartLocation`'s own member documentation
/// (`smithy_query(query="shape", shape="DocumentationPartLocation")`) — `path` is "a valid
/// field for the API entity types of RESOURCE, METHOD, PATH_PARAMETER, QUERY_PARAMETER,
/// REQUEST_HEADER, REQUEST_BODY, RESPONSE, RESPONSE_HEADER, and RESPONSE_BODY", `method`
/// for that list without RESOURCE, `statusCode` for "RESPONSE, RESPONSE_HEADER, and
/// RESPONSE_BODY", and `name` for "AUTHORIZER, MODEL, PATH_PARAMETER, QUERY_PARAMETER,
/// REQUEST_HEADER, REQUEST_BODY and RESPONSE_HEADER".
const LOCATION_FIELDS: &[(&str, &[&str])] = &[
    ("API", &[]),
    ("AUTHORIZER", &["name"]),
    ("MODEL", &["name"]),
    ("RESOURCE", &["path"]),
    ("METHOD", &["path", "method"]),
    ("PATH_PARAMETER", &["path", "method", "name"]),
    ("QUERY_PARAMETER", &["path", "method", "name"]),
    ("RESPONSE", &["path", "method", "statusCode"]),
    ("RESPONSE_HEADER", &["path", "method", "statusCode", "name"]),
];

/// One deletion case: the entity types the API's parts target, in creation order, and the
/// index of the part the case deletes.
struct DeleteCase {
    name: &'static str,
    parts: &'static [&'static str],
    deleted: usize,
}

/// The collections a delete is asserted over: one part and several, the first, a middle
/// and the last of the creation order, and a location with every member its type makes
/// valid.
const DELETE_CASES: &[DeleteCase] = &[
    DeleteCase {
        name: "one part, the only one deleted",
        parts: &["API"],
        deleted: 0,
    },
    DeleteCase {
        name: "three parts of one type, the first deleted",
        parts: &["RESOURCE", "RESOURCE", "RESOURCE"],
        deleted: 0,
    },
    DeleteCase {
        name: "three parts, the middle deleted",
        parts: &["METHOD", "RESPONSE", "AUTHORIZER"],
        deleted: 1,
    },
    DeleteCase {
        name: "three parts, the last deleted",
        parts: &["MODEL", "PATH_PARAMETER", "QUERY_PARAMETER"],
        deleted: 2,
    },
    DeleteCase {
        name: "six parts spanning the type catalog, a four-member location deleted",
        parts: &[
            "API",
            "AUTHORIZER",
            "MODEL",
            "RESOURCE",
            "METHOD",
            "RESPONSE_HEADER",
        ],
        deleted: 5,
    },
];

/// The location members this type makes valid, from [`LOCATION_FIELDS`].
fn valid_fields(entity_type: &str) -> &'static [&'static str] {
    LOCATION_FIELDS
        .iter()
        .find(|(name, _)| *name == entity_type)
        .map(|(_, valid)| *valid)
        .unwrap_or_else(|| panic!("{entity_type} is not in the location-field catalog"))
}

/// A location targeting `entity_type`, distinguished from every other location built with
/// another `rank`: each member the type makes valid carries a value of its own, so two
/// parts of one type are two records rather than one conflict. `statusCode` is a value its
/// documented pattern (`^([1-5]\d\d|\*|\s*)$`) admits.
fn location(entity_type: &str, rank: usize) -> Value {
    let valid = valid_fields(entity_type);
    let mut location = Map::new();
    location.insert("type".to_string(), json!(entity_type));
    if valid.contains(&"path") {
        location.insert("path".to_string(), json!(format!("/entity-{rank}")));
    }
    if valid.contains(&"method") {
        location.insert("method".to_string(), json!("GET"));
    }
    if valid.contains(&"statusCode") {
        location.insert("statusCode".to_string(), json!("404"));
    }
    if valid.contains(&"name") {
        location.insert("name".to_string(), json!(format!("member-{rank}")));
    }
    Value::Object(location)
}

/// A content map, as the JSON string `DocumentationPart$properties` documents ("The map
/// must be encoded as a JSON string"). The `rank` also sizes the description, so the
/// collection spans small and large content maps.
fn content_map(entity_type: &str, rank: usize) -> String {
    json!({
        "description": format!("{entity_type} documentation {}", "d".repeat(rank * 64 + 1)),
        "rank": rank,
    })
    .to_string()
}

/// `CreateDocumentationPart`, asserting the modelled 201 and returning the minted id.
async fn create_part(
    gateway: &Gateway,
    api: &str,
    location: &Value,
    properties: &str,
    case: &str,
) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/documentation/parts"),
            Some(json!({ "location": location, "properties": properties })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: CreateDocumentationPart expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", case)
}

/// `GetDocumentationPart` for one part.
async fn get_part(gateway: &Gateway, api: &str, id: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/documentation/parts/{id}"),
            None,
        )
        .await
}

/// `DeleteDocumentationPart` for one part.
async fn delete_part(gateway: &Gateway, api: &str, id: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/documentation/parts/{id}"),
            None,
        )
        .await
}

/// The ids `GetDocumentationParts` reports for `api`, in the order it reports them.
async fn listed_ids(gateway: &Gateway, api: &str, case: &str) -> Vec<String> {
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
    page_items(&answer.body, case)
        .iter()
        .map(|item| string_member(item, "id", case))
        .collect()
}

/// Assert `GetDocumentationPart` still reports this part exactly as it was stored.
async fn assert_still_reported(
    gateway: &Gateway,
    api: &str,
    id: &str,
    location: &Value,
    properties: &str,
    case: &str,
) {
    let read = get_part(gateway, api, id).await;
    assert_eq!(
        read.status, 200,
        "case {case}: surviving part {id} expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["location"], *location,
        "case {case}: surviving part {id} expected location {location:?}, got {:?}",
        read.body["location"]
    );
    assert_eq!(
        read.body["properties"].as_str(),
        Some(properties),
        "case {case}: surviving part {id} expected properties {properties:?}, got {:?}",
        read.body["properties"]
    );
}

#[tokio::test]
async fn deleting_a_part_removes_exactly_that_part_from_every_reader() {
    for case in DELETE_CASES {
        // A fresh emulator per row, so what a row asserts about the collection is that
        // row's own creations only.
        let gateway = Gateway::new();
        let (api, _root) = create_api(&gateway, "documentation-part-delete").await;

        let mut stored: Vec<(String, Value, String)> = Vec::new();
        for (rank, entity_type) in case.parts.iter().enumerate() {
            let at = location(entity_type, rank);
            let properties = content_map(entity_type, rank);
            let id = create_part(&gateway, &api, &at, &properties, case.name).await;
            stored.push((id, at, properties));
        }
        let created: Vec<String> = stored.iter().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(
            listed_ids(&gateway, &api, case.name).await,
            created,
            "case {}: expected the created parts to be listed in creation order",
            case.name
        );

        let (doomed_id, doomed_location, _) = stored[case.deleted].clone();
        let deleted = delete_part(&gateway, &api, &doomed_id).await;
        assert_eq!(
            deleted.status, DELETE_STATUS,
            "case {}: DeleteDocumentationPart expected {DELETE_STATUS}, got {} with {:?}",
            case.name, deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null(),
            "case {}: DeleteDocumentationPart answers no body, got {:?}",
            case.name,
            deleted.body
        );

        // Gone from the single reader...
        let read = get_part(&gateway, &api, &doomed_id).await;
        assert_failure(
            &read,
            404,
            NOT_FOUND,
            &format!("{}: GetDocumentationPart({doomed_id})", case.name),
        );
        // ...and from the collection, which still reports every survivor in creation
        // order.
        let survivors: Vec<String> = created
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != case.deleted)
            .map(|(_, id)| id.clone())
            .collect();
        let listed = listed_ids(&gateway, &api, case.name).await;
        assert_eq!(
            listed, survivors,
            "case {}: expected the surviving parts {survivors:?}, got {listed:?}",
            case.name
        );
        for (index, (id, at, properties)) in stored.iter().enumerate() {
            if index == case.deleted {
                continue;
            }
            assert_still_reported(&gateway, &api, id, at, properties, case.name).await;
        }

        // The deleted part's location is no longer documented by this API, so a part may
        // be created at it again — the record left the store rather than being hidden.
        let recreated = create_part(
            &gateway,
            &api,
            &doomed_location,
            &content_map("recreated", case.deleted),
            &format!("{}: recreate at the deleted location", case.name),
        )
        .await;
        assert_ne!(
            recreated, doomed_id,
            "case {}: a recreated part carries a freshly minted id",
            case.name
        );
    }
}

#[tokio::test]
async fn a_delete_that_names_no_stored_record_is_not_found_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-part-delete-refusals").await;
    let (other_api, _other_root) =
        create_api(&gateway, "documentation-part-delete-refusals-other").await;

    // One part in each API, so a case can address a real part through the WRONG API.
    let kept = create_part(
        &gateway,
        &api,
        &location("RESOURCE", 1),
        &content_map("RESOURCE", 1),
        "the kept part",
    )
    .await;
    let other_kept = create_part(
        &gateway,
        &other_api,
        &location("RESOURCE", 2),
        &content_map("RESOURCE", 2),
        "the other API's part",
    )
    .await;

    // A part that was deleted once, so the repeated delete of it is one of the rows.
    let gone = create_part(
        &gateway,
        &api,
        &location("METHOD", 3),
        &content_map("METHOD", 3),
        "the part deleted once",
    )
    .await;
    let first = delete_part(&gateway, &api, &gone).await;
    assert_eq!(
        first.status, DELETE_STATUS,
        "case the part deleted once: expected {DELETE_STATUS}, got {} with {:?}",
        first.status, first.body
    );

    // Each row is `(case, restApiId, documentationPartId)`: a pair of identifiers that
    // addresses no stored documentation part.
    let cases: Vec<(String, String, String)> = vec![
        (
            "an API id that names no API".to_string(),
            "zzzzzzzzzz".to_string(),
            kept.clone(),
        ),
        (
            "a part id that names no part of this API".to_string(),
            api.clone(),
            "zzzzzzzzzz".to_string(),
        ),
        (
            "a part id that belongs to another API".to_string(),
            api.clone(),
            other_kept.clone(),
        ),
        (
            "a part that was already deleted".to_string(),
            api.clone(),
            gone.clone(),
        ),
        (
            "an API id that names no API, with a part id that names no part".to_string(),
            "zzzzzzzzzz".to_string(),
            "zzzzzzzzzz".to_string(),
        ),
    ];

    for (case, rest_api_id, part_id) in &cases {
        let answer = delete_part(&gateway, rest_api_id, part_id).await;
        assert_failure(&answer, 404, NOT_FOUND, case);

        // Nothing the refusal touched: both APIs still document exactly what they did.
        let listed = listed_ids(&gateway, &api, case).await;
        assert_eq!(
            listed,
            vec![kept.clone()],
            "case {case}: expected the API to still document {kept:?}, got {listed:?}"
        );
        let other_listed = listed_ids(&gateway, &other_api, case).await;
        assert_eq!(
            other_listed,
            vec![other_kept.clone()],
            "case {case}: expected the other API to still document {other_kept:?}, got \
             {other_listed:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_location_is_free_for_a_new_part() {
    // Over the whole catalog of entity types this suite builds locations for, so the
    // invariant is asserted on a location with no members, one with a name, one with a
    // path, and ones with three and four members.
    for (rank, (entity_type, _)) in LOCATION_FIELDS.iter().enumerate() {
        let gateway = Gateway::new();
        let (api, _root) = create_api(&gateway, "documentation-part-relocate").await;
        let at = location(entity_type, rank);
        let properties = content_map(entity_type, rank);
        let case = format!("type={entity_type}");

        let first = create_part(&gateway, &api, &at, &properties, &case).await;

        // While the part exists, its location is taken: a second part at it is the
        // modelled conflict.
        let clash = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/documentation/parts"),
                Some(json!({ "location": at, "properties": properties })),
            )
            .await;
        assert_failure(
            &clash,
            409,
            CONFLICT,
            &format!("{case}: a second part at one location"),
        );

        let deleted = delete_part(&gateway, &api, &first).await;
        assert_eq!(
            deleted.status, DELETE_STATUS,
            "case {case}: DeleteDocumentationPart expected {DELETE_STATUS}, got {} with {:?}",
            deleted.status, deleted.body
        );

        // The same location is creatable again, under a new id, and reads back as the new
        // part's own content map.
        let replacement_properties = content_map("replacement", rank);
        let second = create_part(
            &gateway,
            &api,
            &at,
            &replacement_properties,
            &format!("{case}: recreate after the delete"),
        )
        .await;
        assert_ne!(
            second, first,
            "case {case}: a recreated part carries a freshly minted id"
        );
        assert_still_reported(
            &gateway,
            &api,
            &second,
            &at,
            &replacement_properties,
            &format!("{case}: recreate after the delete"),
        )
        .await;
        let listed = listed_ids(&gateway, &api, &case).await;
        assert_eq!(
            listed,
            vec![second.clone()],
            "case {case}: expected only the recreated part {second:?}, got {listed:?}"
        );
    }
}
