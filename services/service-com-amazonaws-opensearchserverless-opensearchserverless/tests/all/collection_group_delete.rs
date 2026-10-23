// SPDX-License-Identifier: CC0-1.0

//! `DeleteCollectionGroup` and the read path that must observe it, end to end over the
//! wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body), through the
//! harness `collection_lifecycle` owns, so the boundary validator, the protocol codec and
//! the handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: a batch read answers EVERY requested key in exactly one of its two
//! output lists — a key with a live record as a detail, a key with none as an error entry
//! whose `errorCode` is `NOT_FOUND` and which echoes the key that was asked for — and a
//! delete removes the record, so the key it addressed moves from the detail list to the
//! error list. The tables below span what the rule generalises over: both `StandbyReplicas`
//! members, both `ServerlessGeneration` members and the omitted case, capacity limits whole
//! / partial / absent, four Regions, names at the published length bounds, id- as well as
//! name-addressed reads, keys that never existed as well as keys whose group was deleted,
//! and batches of one, several and mixed keys.

use std::time::Duration;

use serde_json::{json, Value};

use crate::collection_group_lifecycle::{
    batch_get, create_group, detail_at, error_at, NOT_FOUND_CODE, NOT_FOUND_MESSAGE,
};
use crate::collection_lifecycle::{at, call, call_in, emulator, is_collection_id, text};

/// Long enough for the `CREATING -> ACTIVE` transition `CreateCollection` starts to fall
/// due, so the collection it created can be deleted; the emulator's span is 500 ms and
/// nothing sleeps for it inside a handler.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// Delete the collection group `body` addresses in `region`; answer its HTTP status, the
/// error code when it failed, and the parsed body.
async fn delete_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    body: Value,
) -> (u16, String, Value) {
    call_in(router, region, "DeleteCollectionGroup", body).await
}

/// The ids reported in a `BatchGetCollectionGroup` answer's `collectionGroupDetails`.
fn detail_ids(answer: &Value) -> Vec<String> {
    at(answer, "collectionGroupDetails")
        .as_array()
        .map(|details| details.iter().map(|detail| text(detail, "id")).collect())
        .unwrap_or_default()
}

/// The ids reported in a `BatchGetCollectionGroup` answer's `collectionGroupErrorDetails`.
fn error_ids(answer: &Value) -> Vec<String> {
    at(answer, "collectionGroupErrorDetails")
        .as_array()
        .map(|errors| errors.iter().map(|error| text(error, "id")).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The delete is observable: every row's group is created, read back as a detail, deleted,
// and then read back — by id and by name — as the documented NOT_FOUND error entry.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_collection_group_is_reported_as_a_not_found_error_detail() {
    let router = emulator();

    // (region, create request) — the rows differ in Region, `standbyReplicas`,
    // `generation` (both members and the omitted case), the shape of `capacityLimits`,
    // tags, and name length at both published bounds of `CollectionGroupName`
    // (`@length(min: 3, max: 32)`).
    let cases: Vec<(&str, Value)> = vec![
        (
            "us-east-1",
            json!({
                "name": "abc",
                "standbyReplicas": "ENABLED",
                "generation": "CLASSIC",
                "description": "a name at the published minimum length",
                "capacityLimits": {
                    "minIndexingCapacityInOCU": 2.0,
                    "maxIndexingCapacityInOCU": 10.0,
                    "minSearchCapacityInOCU": 2.0,
                    "maxSearchCapacityInOCU": 20.0,
                },
                "tags": [{ "key": "team", "value": "search" }],
            }),
        ),
        (
            "us-west-2",
            json!({
                "name": "g".repeat(32),
                "standbyReplicas": "DISABLED",
                "generation": "NEXTGEN",
                // Only the ceilings: a partial `capacityLimits` must not change what a
                // delete does.
                "capacityLimits": {
                    "maxIndexingCapacityInOCU": 4.0,
                    "maxSearchCapacityInOCU": 4.0,
                },
                "tags": [{ "key": "owner", "value": "数据团队" }],
            }),
        ),
        (
            "eu-west-1",
            // Every optional member omitted.
            json!({ "name": "gamma-group", "standbyReplicas": "ENABLED" }),
        ),
        (
            "ap-southeast-2",
            json!({
                "name": "delta-group",
                "standbyReplicas": "DISABLED",
                "tags": [],
            }),
        ),
    ];

    for (region, request) in cases {
        let name = text(&request, "name");
        let case = format!("DeleteCollectionGroup({name} in {region})");
        let detail = create_group(&router, region, request).await;
        let id = text(&detail, "id");
        assert!(
            is_collection_id(&id),
            "case {case}: the created id must match ^[a-z0-9]{{3,40}}$, got {id:?}"
        );

        // Before the delete the group is a detail, and the error list is empty.
        let answer = batch_get(&router, region, json!({ "ids": [&id] })).await;
        assert_eq!(
            detail_ids(&answer),
            vec![id.clone()],
            "case {case}: a live group is the only detail, got {answer}"
        );
        assert_eq!(
            error_ids(&answer),
            Vec::<String>::new(),
            "case {case}: a live group reports no error entry, got {answer}"
        );

        let (status, code, body) = delete_group(&router, region, json!({ "id": &id })).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case}: expected an accepted delete, got {status} {code} with {body}"
        );

        // After the delete the group is gone from the detail list and present in the
        // error list, addressed either way.
        for (selector, key_member, key) in [
            (json!({ "ids": [&id] }), "id", id.clone()),
            (json!({ "names": [&name] }), "name", name.clone()),
        ] {
            let case = format!("{case} read back with {selector}");
            let answer = batch_get(&router, region, selector).await;
            assert_eq!(
                at(&answer, "collectionGroupDetails"),
                &json!([]),
                "case {case}: a deleted group must not be a detail, got {answer}"
            );
            let error = error_at(&answer, 0);
            assert_eq!(
                text(&error, key_member),
                key,
                "case {case}: the error entry echoes the {key_member} that was asked for, got {answer}"
            );
            assert_eq!(
                text(&error, "errorCode"),
                NOT_FOUND_CODE,
                "case {case}: errorCode must be {NOT_FOUND_CODE}, got {answer}"
            );
            assert_eq!(
                text(&error, "errorMessage"),
                NOT_FOUND_MESSAGE,
                "case {case}: errorMessage must be the documented one, got {answer}"
            );
        }

        // Deleting the same id again is the modelled 404: the record is already gone.
        let (status, code, body) = delete_group(&router, region, json!({ "id": &id })).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: a second delete must be refused, got {status} {code} with {body}"
        );

        // And the name the delete freed may be created again, under a fresh id.
        let recreated = create_group(
            &router,
            region,
            json!({ "name": &name, "standbyReplicas": "ENABLED" }),
        )
        .await;
        let recreated_id = text(&recreated, "id");
        assert_ne!(
            recreated_id, id,
            "case {case}: recreating the freed name mints a new id, got {recreated}"
        );
    }
}

// ---------------------------------------------------------------------------
// The batch contract: every requested key comes back in exactly one list, whether it was
// deleted, never created, or is live — over batches of one, several and mixed keys.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_requested_key_comes_back_in_exactly_one_list() {
    let router = emulator();

    // Three live groups, so the mixed batches below have known keys to interleave with.
    let mut live: Vec<(String, String)> = Vec::new();
    for name in ["live-one", "live-two", "live-three"] {
        let detail = create_group(
            &router,
            "us-east-1",
            json!({ "name": name, "standbyReplicas": "ENABLED" }),
        )
        .await;
        live.push((text(&detail, "id"), name.to_string()));
    }

    // One group created and then deleted, so its id has no live record.
    let deleted = create_group(
        &router,
        "us-east-1",
        json!({ "name": "gone-group", "standbyReplicas": "DISABLED" }),
    )
    .await;
    let deleted_id = text(&deleted, "id");
    let (status, code, body) = delete_group(&router, "us-east-1", json!({ "id": &deleted_id })).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case delete gone-group: expected an accepted delete, got {status} {code} with {body}"
    );

    // Ids that never named a group: at both published bounds of `CollectionGroupId`
    // (`@length(min: 3, max: 40)`, `^[a-z0-9]{3,40}$`) and in between, plus the id whose
    // group was deleted.
    let unknown_ids: Vec<String> = vec![
        "abc".to_string(),
        "z9".repeat(20),
        "0123456789abcdef".to_string(),
        deleted_id.clone(),
    ];

    // Batches of one, of several, and mixed with live keys — the size and the ordering the
    // rule generalises over.
    let batches: Vec<Vec<String>> = vec![
        vec![unknown_ids[0].clone()],
        unknown_ids.clone(),
        vec![live[0].0.clone(), unknown_ids[3].clone(), live[1].0.clone()],
        vec![
            unknown_ids[1].clone(),
            live[2].0.clone(),
            unknown_ids[2].clone(),
        ],
    ];

    for ids in batches {
        let case = format!("BatchGetCollectionGroup(ids={ids:?})");
        let answer = batch_get(&router, "us-east-1", json!({ "ids": &ids })).await;
        let details = detail_ids(&answer);
        let errors = error_ids(&answer);
        assert_eq!(
            details.len() + errors.len(),
            ids.len(),
            "case {case}: every requested id is answered exactly once, got {answer}"
        );
        for id in &ids {
            let known = live.iter().any(|(live_id, _)| live_id == id);
            assert_eq!(
                details.contains(id),
                known,
                "case {case}: {id} must be a detail only when it has a live record, got {answer}"
            );
            assert_eq!(
                errors.contains(id),
                !known,
                "case {case}: {id} must be an error entry only when it has no live record, got {answer}"
            );
        }
        // Every error entry carries the documented code and message, and an id-addressed
        // read reports no name.
        for index in 0..errors.len() {
            let error = error_at(&answer, index);
            assert_eq!(
                text(&error, "errorCode"),
                NOT_FOUND_CODE,
                "case {case}: errorCode must be {NOT_FOUND_CODE}, got {error}"
            );
            assert_eq!(
                text(&error, "errorMessage"),
                NOT_FOUND_MESSAGE,
                "case {case}: errorMessage must be the documented one, got {error}"
            );
            assert_eq!(
                at(&error, "name"),
                &Value::Null,
                "case {case}: an id-addressed error carries no name, got {error}"
            );
        }
    }

    // The same contract for name-addressed reads: a deleted group's name and a name that
    // never existed are both error entries, reported under `name` and not under `id`.
    let answer = batch_get(
        &router,
        "us-east-1",
        json!({ "names": ["live-one", "gone-group", "never-a-group", "live-three"] }),
    )
    .await;
    let names: Vec<String> = at(&answer, "collectionGroupDetails")
        .as_array()
        .map(|details| details.iter().map(|detail| text(detail, "name")).collect())
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["live-one".to_string(), "live-three".to_string()],
        "case name-addressed mixed read: only live groups are details, got {answer}"
    );
    let error_names: Vec<String> = at(&answer, "collectionGroupErrorDetails")
        .as_array()
        .map(|errors| errors.iter().map(|error| text(error, "name")).collect())
        .unwrap_or_default();
    assert_eq!(
        error_names,
        vec!["gone-group".to_string(), "never-a-group".to_string()],
        "case name-addressed mixed read: a deleted name and an unknown name are error \
         entries, in request order, got {answer}"
    );
    for index in 0..error_names.len() {
        let error = error_at(&answer, index);
        assert_eq!(
            at(&error, "id"),
            &Value::Null,
            "case name-addressed mixed read: a name-addressed error carries no id, got {error}"
        );
        assert_eq!(
            text(&error, "errorCode"),
            NOT_FOUND_CODE,
            "case name-addressed mixed read: errorCode must be {NOT_FOUND_CODE}, got {error}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the delete refuses: an id that names no group, and a group that still holds
// collections ("You can only delete empty collection groups that contain no collections").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_collection_group_refuses_an_unknown_id_and_a_non_empty_group() {
    let router = emulator();

    // An id that never named a group, at both published bounds of `CollectionGroupId` and
    // in between, is the modelled 404 — not a silent success.
    for id in ["abc", &"z9".repeat(20), "0123456789abcdef"] {
        let (status, code, body) = delete_group(&router, "us-east-1", json!({ "id": id })).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case DeleteCollectionGroup({id}) with no such group: expected 404, got {status} {code} with {body}"
        );
    }

    // An id outside the modelled pattern or length is refused at the boundary.
    for id in ["ab", "Upper", &"g".repeat(41)] {
        let (status, code, body) = delete_group(&router, "us-east-1", json!({ "id": id })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case DeleteCollectionGroup({id}) outside the modelled constraints: expected 400, \
             got {status} {code} with {body}"
        );
    }

    // Groups holding one and two collections: both are refused, so the refusal is about
    // emptiness rather than about one particular count.
    for (group, collections) in [
        ("single-group", vec!["one-collection"]),
        ("double-group", vec!["two-collection", "three-collection"]),
    ] {
        let detail = create_group(
            &router,
            "us-east-1",
            json!({ "name": group, "standbyReplicas": "ENABLED" }),
        )
        .await;
        let id = text(&detail, "id");
        for collection in &collections {
            let (status, code, body) = call(
                &router,
                "CreateCollection",
                json!({ "name": collection, "collectionGroupName": group }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case CreateCollection({collection}): expected 200, got {status} {code} with {body}"
            );
        }

        let (status, code, body) = delete_group(&router, "us-east-1", json!({ "id": &id })).await;
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case DeleteCollectionGroup({group}) holding {} collection(s): expected 409, got \
             {status} {code} with {body}",
            collections.len()
        );

        // The refusal stored nothing: the group is still a detail, with its collections
        // still counted.
        let answer = batch_get(&router, "us-east-1", json!({ "ids": [&id] })).await;
        assert_eq!(
            at(&detail_at(&answer, 0), "numberOfCollections").as_i64(),
            Some(collections.len() as i64),
            "case DeleteCollectionGroup({group}) refused: the group is unchanged, got {answer}"
        );

        // Emptying the group makes the same delete succeed — the emptiness is what the
        // refusal was about. A collection is deletable once its create has settled to
        // ACTIVE.
        tokio::time::sleep(SETTLE_WAIT).await;
        for collection in &collections {
            let (status, code, body) = call(
                &router,
                "BatchGetCollection",
                json!({ "names": [collection] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case BatchGetCollection({collection}): expected 200, got {status} {code} with {body}"
            );
            let collection_id = text(&detail_at_collection(&body, 0), "id");
            let (status, code, body) = call(
                &router,
                "DeleteCollection",
                json!({ "id": collection_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case DeleteCollection({collection}): expected 200, got {status} {code} with {body}"
            );
        }
        let (status, code, body) = delete_group(&router, "us-east-1", json!({ "id": &id })).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case DeleteCollectionGroup({group}) once emptied: expected 200, got {status} {code} with {body}"
        );
        let answer = batch_get(&router, "us-east-1", json!({ "ids": [&id] })).await;
        assert_eq!(
            text(&error_at(&answer, 0), "errorCode"),
            NOT_FOUND_CODE,
            "case DeleteCollectionGroup({group}) once emptied: the group is gone from the read \
             path, got {answer}"
        );
    }
}

/// The `collectionDetails` entry at `index` of a `BatchGetCollection` answer, or
/// `Value::Null` — the collection-shaped counterpart of
/// `collection_group_lifecycle::detail_at`.
fn detail_at_collection(answer: &Value, index: usize) -> Value {
    at(answer, "collectionDetails")
        .as_array()
        .and_then(|details| details.get(index))
        .cloned()
        .unwrap_or(Value::Null)
}
