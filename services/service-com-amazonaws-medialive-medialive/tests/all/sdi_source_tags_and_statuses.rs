// SPDX-License-Identifier: CC0-1.0

//! What every MediaLive SdiSource response reports about the source's TAGS, and
//! the success statuses the create and the delete answer with, end to end over
//! the restJson1 wire.
//!
//! Two rules, each asserted over a table of inputs — the recorded
//! `medialive-sdi-source-lifecycle` case (an untagged SINGLE source) is ONE row:
//!
//! 1. Every operation that reports an SdiSource (`CreateSdiSource`,
//!    `DescribeSdiSource`, `UpdateSdiSource`, `DeleteSdiSource`) reports the
//!    source's STORED tag set as a map, empty when the source carries none —
//!    never an absent member — the same way every other taggable MediaLive
//!    resource reports its tags.
//! 2. `CreateSdiSource` answers `201 Created` and `DeleteSdiSource` answers
//!    `200 OK`: the recorded success statuses, where the model's `@http` codes
//!    say 200 and 202.
//!
//! The rows differ along the tag set (absent, empty, one pair, several, a value
//! with non-ASCII and reserved characters, the published maximum of
//! [`MAX_RESOURCE_TAGS`] pairs) and along the source's own `Type`/`Mode`,
//! because that is what the rules generalise over.

use serde_json::{json, Map, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_RESOURCE_TAGS;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::percent_encoded;
use crate::wire::{call, error_code, medialive};

/// The collection URI `CreateSdiSource` is bound to
/// (`smithy_query(query="operation", operation="CreateSdiSource")`:
/// `POST /prod/sdiSources`).
const SDI_SOURCES: &str = "/prod/sdiSources";

/// The URI the tag operations are bound to
/// (`smithy_query(query="operation", operation="CreateTags,DeleteTags")`:
/// `POST`/`DELETE /prod/tags/{ResourceArn}`).
const TAGS: &str = "/prod/tags";

/// The recorded success status of a MediaLive create: `201 Created`, where the
/// model declares `@http` code 200 (see `CreateSdiSource`'s `SUCCESS_STATUS`).
const CREATED: u16 = 201;

/// The recorded success status of `DeleteSdiSource`: `200 OK`, where the model
/// declares `@http` code 202 (see `DeleteSdiSource`'s `SUCCESS_STATUS`). The
/// deletion is complete when the call returns, so there is nothing to poll.
const DELETED_OK: u16 = 200;

/// One row's tag set: the pairs the create carries, or `None` for a create that
/// names no `Tags` member at all.
type TagPairs = Option<Vec<(String, String)>>;

/// A tag set as a request body carries it.
fn tag_body(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// The same pairs as the map a response must report.
fn expected_map(pairs: &[(String, String)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| (key.clone(), json!(value)))
        .collect()
}

/// The `tags` member of a reported SdiSource, which must be a MAP — the point of
/// rule 1. A null or absent member fails here with the case and what was found,
/// rather than reading as an empty set.
fn projected_tags(case: &str, who: &str, source: &Value) -> Map<String, Value> {
    match source.get("tags") {
        Some(Value::Object(tags)) => tags.clone(),
        other => panic!(
            "case {case:?}: {who}: expected sdiSource.tags to be a map, got {other:?} \
             in {source}"
        ),
    }
}

/// `CreateSdiSource`, answering the reported source with the recorded create
/// status asserted.
async fn create(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, response) = call(router, "POST", SDI_SOURCES, Some(body)).await;
    assert_eq!(
        status, CREATED,
        "case {case:?}: CreateSdiSource: expected {CREATED}, got {status}: {response}"
    );
    response["sdiSource"].clone()
}

/// `DescribeSdiSource`, answering the reported source with its 200 asserted.
async fn describe(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, response) = call(router, "GET", &format!("{SDI_SOURCES}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeSdiSource: expected 200, got {status}: {response}"
    );
    response["sdiSource"].clone()
}

/// `UpdateSdiSource` (`PUT /prod/sdiSources/{SdiSourceId}`), answering the
/// reported source with its 200 asserted.
async fn update(router: &MultiServiceRouter, case: &str, id: &str, body: Value) -> Value {
    let (status, response) =
        call(router, "PUT", &format!("{SDI_SOURCES}/{id}"), Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateSdiSource: expected 200, got {status}: {response}"
    );
    response["sdiSource"].clone()
}

/// `DeleteSdiSource`, answering its status and the body it carried.
async fn delete(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    let (status, response) = call(router, "DELETE", &format!("{SDI_SOURCES}/{id}"), None).await;
    if status == DELETED_OK {
        (status, response["sdiSource"].clone())
    } else {
        (status, response)
    }
}

/// `CreateTags` on an ARN: the tag writer every MediaLive resource shares.
async fn create_tags(router: &MultiServiceRouter, case: &str, arn: &str, body: Value) {
    let uri = format!("{TAGS}/{}", percent_encoded(arn));
    let (status, response) = call(router, "POST", &uri, Some(body)).await;
    assert_eq!(
        status, 204,
        "case {case:?}: CreateTags: expected 204, got {status}: {response}"
    );
}

/// `DeleteTags` on an ARN. `TagKeys` is an `@httpQuery` list, so each key is one
/// repetition of the `tagKeys` parameter.
async fn delete_tags(router: &MultiServiceRouter, case: &str, arn: &str, keys: &[String]) {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encoded(key)))
        .collect::<Vec<_>>()
        .join("&");
    let uri = format!("{TAGS}/{}?{query}", percent_encoded(arn));
    let (status, response) = call(router, "DELETE", &uri, None).await;
    assert_eq!(
        status, 204,
        "case {case:?}: DeleteTags: expected 204, got {status}: {response}"
    );
}

/// The tag sets the tables below span. A row is (case, the pairs a create
/// carries or `None` for a create that names no `Tags`).
fn tag_rows() -> Vec<(String, TagPairs)> {
    let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
    vec![
        // The recorded case: a create that names no tags at all.
        ("no tags member".to_string(), None),
        ("an explicitly empty map".to_string(), Some(Vec::new())),
        (
            "one pair".to_string(),
            Some(vec![pair("owner", "video")]),
        ),
        (
            "three pairs".to_string(),
            Some(vec![
                pair("owner", "video"),
                pair("env", "test"),
                pair("cost-centre", "1234"),
            ]),
        ),
        (
            // Tag values are Unicode text and may carry characters the URI and
            // the JSON body both have to survive.
            "a non-ASCII value with reserved characters".to_string(),
            Some(vec![pair("studio", "caméra-à/b c:d+e&f")]),
        ),
        (
            // The published maximum: "Maximum number of tags per resource — 50"
            // (`MAX_RESOURCE_TAGS`).
            format!("the maximum of {MAX_RESOURCE_TAGS} pairs"),
            Some(
                (0..MAX_RESOURCE_TAGS)
                    .map(|index| (format!("key-{index}"), format!("value-{index}")))
                    .collect(),
            ),
        ),
    ]
}

/// Rule 1: every operation that reports an SdiSource reports the source's stored
/// tag set as a map — empty when it carries none — and that map tracks what the
/// tag writers have since stored, because the projection is read out of the
/// store rather than echoed from any one request.
#[tokio::test]
async fn every_sdi_source_projection_reports_the_stored_tag_set_as_a_map() {
    for (case, pairs) in tag_rows() {
        let router = medialive();

        let mut request = json!({ "name": "curling-cameraA", "type": "SINGLE" });
        if let Some(pairs) = &pairs {
            request["tags"] = tag_body(pairs);
        }
        let stored = pairs.clone().unwrap_or_default();
        let at_birth = expected_map(&stored);

        // The create reports the tag set it stored, as a map.
        let created = create(&router, &case, request).await;
        assert_eq!(
            projected_tags(&case, "CreateSdiSource", &created),
            at_birth,
            "case {case:?}: expected CreateSdiSource to report the stored tags, got {created}"
        );
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let arn = created["arn"].as_str().unwrap_or_default().to_string();

        // So does the read.
        let described = describe(&router, &case, &id).await;
        assert_eq!(
            projected_tags(&case, "DescribeSdiSource", &described),
            at_birth,
            "case {case:?}: expected DescribeSdiSource to report the stored tags, got {described}"
        );

        // `UpdateSdiSource` carries no `Tags` member of its own
        // (`smithy_query(query="operation", operation="UpdateSdiSource")`), so an
        // update leaves the stored set alone — and still reports it as a map.
        let updated = update(&router, &case, &id, json!({ "name": "renamed" })).await;
        assert_eq!(
            projected_tags(&case, "UpdateSdiSource", &updated),
            at_birth,
            "case {case:?}: expected UpdateSdiSource to keep and report the stored tags, \
             got {updated}"
        );

        // A tag written after the create shows up in every projection: the map
        // is the stored set, not the create request's echo. Writing one more key
        // onto a full resource is refused, so the maximum row overwrites a key
        // it already has instead of adding one.
        let (written_key, written_value) = if stored.len() >= MAX_RESOURCE_TAGS {
            (stored[0].0.clone(), "rewritten".to_string())
        } else {
            ("added-by-create-tags".to_string(), "yes".to_string())
        };
        create_tags(
            &router,
            &case,
            &arn,
            json!({ "tags": tag_body(&[(written_key.clone(), written_value.clone())]) }),
        )
        .await;
        let mut after_write = at_birth.clone();
        after_write.insert(written_key.clone(), json!(written_value));
        let described = describe(&router, &case, &id).await;
        assert_eq!(
            projected_tags(&case, "DescribeSdiSource after CreateTags", &described),
            after_write,
            "case {case:?}: expected the written tag in the reported map, got {described}"
        );
        let updated = update(&router, &case, &id, json!({ "type": "QUAD" })).await;
        assert_eq!(
            projected_tags(&case, "UpdateSdiSource after CreateTags", &updated),
            after_write,
            "case {case:?}: expected the written tag in the updated source, got {updated}"
        );

        // Emptying the resource of tags leaves a map, not an absent member.
        let keys: Vec<String> = after_write.keys().cloned().collect();
        delete_tags(&router, &case, &arn, &keys).await;
        let described = describe(&router, &case, &id).await;
        assert_eq!(
            projected_tags(&case, "DescribeSdiSource after DeleteTags", &described),
            Map::new(),
            "case {case:?}: expected an emptied tag set to report as {{}}, got {described}"
        );

        // And so does the delete, which reports the source it deleted.
        let (status, deleted) = delete(&router, &id).await;
        assert_eq!(
            status, DELETED_OK,
            "case {case:?}: DeleteSdiSource: expected {DELETED_OK}, got {status}: {deleted}"
        );
        assert_eq!(
            projected_tags(&case, "DeleteSdiSource", &deleted),
            Map::new(),
            "case {case:?}: expected DeleteSdiSource to report the stored tags, got {deleted}"
        );
        assert_eq!(
            deleted["state"],
            json!("DELETED"),
            "case {case:?}: expected the deleted source to be DELETED, got {:?}",
            deleted["state"]
        );
    }
}

/// Rule 2: `CreateSdiSource` answers `201 Created` and `DeleteSdiSource` answers
/// `200 OK`, whatever the source carries — while the REFUSALS keep their own
/// modelled statuses, so the change is scoped to the success path.
///
/// The rows differ along the create's `Type`, `Mode`, `Tags` and `RequestId`,
/// including a retry of an earlier token (which answers with the source that
/// create made, under the same created status), because that is the set of
/// requests the rule covers.
#[tokio::test]
async fn the_create_and_delete_answer_the_recorded_success_statuses() {
    // (case, the create body, a second create body whose answer must be the
    // same source — `None` when the row creates only once)
    let rows: Vec<(String, Value, Option<Value>)> = vec![
        (
            "SINGLE, no mode, no tags".to_string(),
            json!({ "name": "camera-single", "type": "SINGLE" }),
            None,
        ),
        (
            "QUAD in QUADRANT".to_string(),
            json!({ "name": "camera-quad", "type": "QUAD", "mode": "QUADRANT" }),
            None,
        ),
        (
            "QUAD in INTERLEAVE, tagged".to_string(),
            json!({
                "name": "camera-interleave",
                "type": "QUAD",
                "mode": "INTERLEAVE",
                "tags": { "owner": "video" },
            }),
            None,
        ),
        (
            "no member at all".to_string(),
            json!({}),
            None,
        ),
        (
            "a retried create".to_string(),
            json!({ "name": "idempotent", "type": "SINGLE", "requestId": "token-a" }),
            Some(json!({ "name": "idempotent", "type": "SINGLE", "requestId": "token-a" })),
        ),
    ];

    for (case, first, retry) in rows {
        let router = medialive();
        let created = create(&router, &case, first).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        if let Some(retry) = retry {
            // The retry is still a create: same status, same source.
            let again = create(&router, &case, retry).await;
            assert_eq!(
                again, created,
                "case {case:?}: expected the retry to answer with the first source, got {again}"
            );
        }

        let (status, deleted) = delete(&router, &id).await;
        assert_eq!(
            status, DELETED_OK,
            "case {case:?}: DeleteSdiSource: expected {DELETED_OK}, got {status}: {deleted}"
        );
        assert_eq!(
            deleted["id"], created["id"],
            "case {case:?}: expected the delete to report the source it deleted, got {deleted}"
        );

        // A repeated delete of an already DELETED source is the same completed
        // answer, not an accepted-and-pending one.
        let (status, again) = delete(&router, &id).await;
        assert_eq!(
            status, DELETED_OK,
            "case {case:?}: repeated DeleteSdiSource: expected {DELETED_OK}, got {status}: {again}"
        );

        // The refusals are untouched: an id no source has is still a 404.
        let (status, body) = delete(&router, "9999999").await;
        assert_eq!(
            status, 404,
            "case {case:?}: DeleteSdiSource(unknown): expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: DeleteSdiSource(unknown): expected NotFoundException, got {body}"
        );
    }
}

/// Rule 2, the refusal the status change must not swallow: a source an input is
/// using cannot be deleted at all, so the 409 is answered instead of the
/// recorded 200.
#[tokio::test]
async fn a_source_in_use_is_still_refused_rather_than_answered_with_the_success_status() {
    let router = medialive();
    let case = "in use";
    let source = create(
        &router,
        case,
        json!({ "name": "wired", "type": "SINGLE", "tags": { "owner": "video" } }),
    )
    .await;
    let id = source["id"].as_str().unwrap_or_default().to_string();

    let (status, response) = call(
        &router,
        "POST",
        "/prod/inputs",
        Some(json!({ "name": "sdi-input", "type": "SDI", "sdiSources": [id.clone()] })),
    )
    .await;
    assert_eq!(
        status, CREATED,
        "case {case:?}: CreateInput: expected {CREATED}, got {status}: {response}"
    );

    let (status, body) = delete(&router, &id).await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected deleting a source in use to conflict, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {body}"
    );

    // The refused delete changed nothing — the source is still there, still
    // reporting its tag set as a map.
    let described = describe(&router, case, &id).await;
    assert_eq!(
        projected_tags(case, "DescribeSdiSource after a refused delete", &described),
        expected_map(&[("owner".to_string(), "video".to_string())]),
        "case {case:?}: expected the refused delete to leave the tags alone, got {described}"
    );
}
