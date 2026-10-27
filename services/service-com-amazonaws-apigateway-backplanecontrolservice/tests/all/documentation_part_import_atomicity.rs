// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning `ImportDocumentationParts` as an ALL-OR-NOTHING import.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], `create_api` and `assert_failure` are
//! this crate's one wire-test harness, and the documentation-part helpers (`import_body`,
//! `import_parts`, `listed_entities`, `content_map`, `created_part_id`) are reused from
//! the sibling `documentation_parts` suite rather than written a second way.
//!
//! The RULE under test, over a SET of inputs rather than one recorded call: an import
//! whose body declares an entry this service cannot store is refused with the modelled
//! 400 `BadRequestException` and applies NOTHING — neither the offending entry nor the
//! storable entries beside it — so the API's documentation set is exactly what it was
//! before the call. The rows differ along what the rule generalises over: WHY the entry
//! cannot be stored, WHERE it sits among the body's entries, how many entries the body
//! declares, the `PutMode` the import runs in, and whether the caller asked to roll back
//! on a warning.

use serde_json::{json, Value};

use crate::documentation_parts::{
    content_map, created_part_id, import_body, import_parts, listed_entities,
};
use crate::rest_api_lifecycle::{assert_failure, create_api, Gateway};

/// The modelled error code an import this service cannot carry out in full carries.
const BAD_REQUEST: &str = "BadRequestException";

/// An import body declaring `entries` verbatim, so a row may declare an entry that is not
/// even a `{ location, properties }` object — which [`import_body`] cannot express.
fn body_of(entries: Vec<Value>) -> String {
    json!({
        "openapi": "3.0.1",
        "x-amazon-apigateway-documentation": { "documentationParts": entries },
    })
    .to_string()
}

/// One entry this service CAN store, documenting `path`.
fn storable(path: &str) -> Value {
    json!({
        "location": { "type": "METHOD", "path": path, "method": "GET" },
        "properties": { "description": format!("the {path} method") },
    })
}

/// Every way an entry can fail to name a storable documentation part, each with the case
/// that describes it.
///
/// Source: `DocumentationPartLocation`'s own member documentation — the entity types each
/// member is a valid field for, the `statusCode` pattern `^([1-5]\d\d|\*|\s*)$`, and the
/// `properties` content map "encoded as a JSON string" (`smithy_query(query="shape",
/// shape="com.amazonaws.apigateway#DocumentationPartLocation")`).
fn unstorable_entries() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a location type outside the modelled enum",
            json!({ "location": { "type": "NOT_AN_ENTITY" }, "properties": {} }),
        ),
        (
            "a location member the entity type makes invalid",
            json!({ "location": { "type": "API", "name": "nope" }, "properties": {} }),
        ),
        (
            "a location with no type at all",
            json!({ "location": { "path": "/pets" }, "properties": {} }),
        ),
        (
            "an entry with no location at all",
            json!({ "properties": { "description": "nowhere" } }),
        ),
        (
            "a location that is not an object",
            json!({ "location": "METHOD", "properties": {} }),
        ),
        (
            "a location member that is not a string",
            json!({ "location": { "type": "METHOD", "path": 7 }, "properties": {} }),
        ),
        (
            "a statusCode outside its published pattern",
            json!({
                "location": { "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "999" },
                "properties": {},
            }),
        ),
        (
            "a properties that is not a content map",
            json!({
                "location": { "type": "METHOD", "path": "/pets", "method": "PUT" },
                "properties": 7,
            }),
        ),
        (
            "an entry that is not an object",
            json!("documentationParts"),
        ),
    ]
}

#[tokio::test]
async fn an_import_with_an_unstorable_entry_applies_nothing() {
    // The documentation set every refusal below must leave exactly as it was: one part,
    // created through the single-part writer.
    let documented = (
        json!({ "type": "METHOD", "path": "/kept", "method": "GET" }),
        json!({ "description": "kept" }),
    );
    // The `PutMode` values an import may run in, beside the rollback-on-warning flag — an
    // entry that cannot be stored is not a warning the caller may tolerate, so every
    // combination answers the same refusal. Cycled across the rows below rather than
    // multiplied by them, so each combination is exercised without re-running the whole
    // table for it.
    let settings: [(Option<&str>, bool); 6] = [
        (None, false),
        (None, true),
        (Some("merge"), false),
        (Some("merge"), true),
        (Some("overwrite"), false),
        (Some("overwrite"), true),
    ];

    let mut round = 0usize;
    for (case, bad) in unstorable_entries() {
        // Rows span WHERE the bad entry sits and how many entries the body declares:
        // alone, first of three, in the middle, and last.
        let positions: Vec<(String, String)> = vec![
            (format!("{case}, alone"), body_of(vec![bad.clone()])),
            (
                format!("{case}, first of three"),
                body_of(vec![bad.clone(), storable("/a"), storable("/b")]),
            ),
            (
                format!("{case}, between two storable entries"),
                body_of(vec![storable("/a"), bad.clone(), storable("/b")]),
            ),
            (
                format!("{case}, last of four"),
                body_of(vec![
                    storable("/a"),
                    storable("/b"),
                    storable("/c"),
                    bad.clone(),
                ]),
            ),
        ];

        for (case, body) in positions {
            let (mode, fail_on_warnings) = settings[round % settings.len()];
            round += 1;
            let case = format!(
                "{case} (mode {}, failonwarnings {fail_on_warnings})",
                mode.unwrap_or("absent")
            );
            let gateway = Gateway::new();
            let (api, _root) = create_api(&gateway, "documentation-import-atomic").await;
            created_part_id(
                &gateway,
                &api,
                documented.0.clone(),
                &content_map("kept"),
                &case,
            )
            .await;

            let answer = import_parts(&gateway, &api, mode, fail_on_warnings, &body).await;
            assert_failure(&answer, 400, BAD_REQUEST, &case);
            assert!(
                answer.body["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case}: expected a message naming what to fix, got {:?}",
                answer.body
            );
            let stored = listed_entities(&gateway, &api, &case).await;
            assert_eq!(
                stored,
                vec![documented.clone()],
                "case {case}: expected the documentation set unchanged, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_import_whose_entries_are_all_storable_applies_every_one_of_them() {
    // The other half of the rule: a body this service CAN store in full is stored in
    // full, and the answer reports no warning — so the refusals above are about the
    // entries, not about the import path.
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-import-atomic-accepted").await;

    let entries: Vec<(Value, Value)> = ["/a", "/b", "/c"]
        .iter()
        .map(|path| {
            (
                json!({ "type": "METHOD", "path": path, "method": "GET" }),
                json!({ "description": format!("the {path} method") }),
            )
        })
        .collect();
    let answer = import_parts(&gateway, &api, None, true, &import_body(&entries)).await;
    assert_eq!(
        answer.status, 200,
        "case an import of three storable entries: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body["warnings"].is_null(),
        "case an import of three storable entries: expected no warnings, got {:?}",
        answer.body["warnings"]
    );
    let stored = listed_entities(&gateway, &api, "an import of three storable entries").await;
    assert_eq!(
        stored.len(),
        entries.len(),
        "case an import of three storable entries: expected one part per entry, got {stored:?}"
    );
}
