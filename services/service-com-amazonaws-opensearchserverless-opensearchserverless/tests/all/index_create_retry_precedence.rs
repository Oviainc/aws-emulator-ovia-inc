// SPDX-License-Identifier: CC0-1.0

//! `CreateIndex` retried while its collection is mid-transition, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: an `@idempotent` create resolves the resource its request names
//! BEFORE any condition that constrains adding a new one, so a repeat of a create this
//! service already accepted is answered with the resource that create made, and the
//! conditions for adding one — here the collection's status and the per-collection index
//! cap — are applied only to a request that would actually add one. The tables below span
//! the transient statuses a collection reaches on its own and the document shapes an
//! `indexSchema` takes, rather than the one request the issue quoted.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{call, emulator, text, CLIENT_ERROR};
use crate::encryption_indexes_and_tagging::{active_collection, read_collection};
use crate::index_delete::create_index;
use crate::index_schema_update::stored_schema;

/// The error code a state conflict and a name already taken both carry
/// (`smithy_query(query="operation", operation="CreateIndex")` -> errors).
const CONFLICT: &str = "ConflictException";

/// The error code an index the store does not hold carries.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The status a collection is "ready to use" in (`CollectionStatus$ACTIVE`).
const ACTIVE: &str = "ACTIVE";

/// `CollectionStatus$UPDATING`, the status an update that RECONFIGURES the collection (its
/// vector-search acceleration) leaves behind until the clock settles it back.
const UPDATING: &str = "UPDATING";

/// `CollectionStatus$DELETING`, the status an accepted deletion leaves until the purge.
const DELETING: &str = "DELETING";

/// How many times a row that wants a collection reported in a TRANSIENT status may set one
/// up. The status is owed back to the clock, so a host that stalls the test thread past the
/// emulator's span closes the window before the row can read it; a closed window means the
/// row proved nothing, not that the service is wrong, so it is set up again rather than
/// failed. Exhausting the attempts fails loudly.
const WINDOW_ATTEMPTS: usize = 5;

/// How this service words a name already taken by an index with another schema.
fn name_taken_message(index_name: &str, id: &str) -> String {
    format!("An index with name '{index_name}' already exists in the collection with ID {id}.")
}

/// How this service words a collection that cannot take a NEW index yet.
fn status_conflict_message(id: &str, status: &str) -> String {
    format!(
        "Collection with ID {id} is in status {status} and cannot take an index; it must be {ACTIVE}"
    )
}

/// Drive `id` into the non-terminal `status` through the operation that leaves it there,
/// and answer whether the store still reports it there.
async fn drive_into(router: &MultiServiceRouter, id: &str, status: &str, case: &str) -> bool {
    let (operation, request) = match status {
        UPDATING => (
            "UpdateCollection",
            json!({ "id": id, "vectorOptions": { "ServerlessVectorAcceleration": "ENABLED" } }),
        ),
        DELETING => ("DeleteCollection", json!({ "id": id })),
        other => panic!("case {case}: no operation in this crate leaves a collection {other}"),
    };
    let (http_status, code, answer) = call(router, operation, request).await;
    assert_eq!(
        http_status, 200,
        "case {operation}({case}): expected 200, got {http_status} {code} with {answer}"
    );
    text(&read_collection(router, id).await, "status") == status
}

/// (case, the transient status the collection sits in, the schema the first create stored,
/// a DIFFERENT schema for the same name) — the rows span the non-terminal statuses a
/// collection reaches on its own in this crate and the document shapes an `indexSchema`
/// takes (an object, a scalar, nested objects with a list, and no schema at all).
fn rows() -> Vec<(&'static str, &'static str, Option<Value>, Value)> {
    vec![
        (
            "an object schema while an unrelated update is in flight",
            UPDATING,
            Some(json!({ "mappings": { "properties": { "ts": { "type": "date" } } } })),
            json!({ "mappings": { "properties": { "ts": { "type": "keyword" } } } }),
        ),
        (
            "an object schema while an accepted deletion is pending",
            DELETING,
            Some(json!({ "mappings": { "properties": { "ts": { "type": "date" } } } })),
            json!({ "mappings": { "properties": { "body": { "type": "text" } } } }),
        ),
        (
            "a scalar schema while an unrelated update is in flight",
            UPDATING,
            Some(json!("text")),
            json!("keyword"),
        ),
        (
            "nested objects and a list while an unrelated update is in flight",
            UPDATING,
            Some(json!({
                "settings": { "index": { "number_of_shards": 2 } },
                "mappings": {
                    "properties": {
                        "embedding": { "type": "knn_vector", "dimension": 1536 },
                        "tags": { "type": "keyword" },
                    },
                },
                "aliases": ["read", "write"],
            })),
            json!({
                "settings": { "index": { "number_of_shards": 2 } },
                "mappings": {
                    "properties": {
                        "embedding": { "type": "knn_vector", "dimension": 1536 },
                        "tags": { "type": "keyword" },
                    },
                },
                "aliases": ["write", "read"],
            }),
        ),
        (
            "no schema at all while an accepted deletion is pending",
            DELETING,
            None,
            json!({ "mappings": {} }),
        ),
        (
            "no schema at all while an unrelated update is in flight",
            UPDATING,
            None,
            json!({}),
        ),
    ]
}

/// One row's attempt. Answers `false` when the collection left the transient status before
/// the row could read its answers, so the caller sets the window up again; otherwise every
/// answer is asserted here.
async fn attempt_row(
    router: &MultiServiceRouter,
    suffix: &str,
    case: &str,
    status: &str,
    stored: &Option<Value>,
    other_schema: &Value,
) -> bool {
    let collection = active_collection(router, &format!("retried-index-{suffix}")).await;
    let id = text(&collection, "id");
    let index_name = format!("retried-{suffix}");
    let added = format!("added-{suffix}");

    // The create whose retry the row is about, made while the collection is usable.
    create_index(router, &id, &index_name, stored.as_ref(), case).await;
    let read_back = stored_schema(router, &id, &index_name, case).await;
    assert_eq!(
        &read_back, stored,
        "case GetIndex({case}, after the create): expected the schema the create stored, \
         {stored:?}, got {read_back:?}"
    );

    if !drive_into(router, &id, status, case).await {
        return false;
    }

    // Every answer below is COLLECTED first and asserted only once the collection is known
    // to have stayed in the transient status throughout: the row is about what this service
    // answers inside that window.
    let mut retried = json!({ "id": &id, "indexName": &index_name });
    if let Some(schema) = stored {
        retried["indexSchema"] = schema.clone();
    }
    let retry = call(router, "CreateIndex", retried).await;
    let read_after_retry = call(
        router,
        "GetIndex",
        json!({ "id": &id, "indexName": &index_name }),
    )
    .await;
    let other = call(
        router,
        "CreateIndex",
        json!({ "id": &id, "indexName": &index_name, "indexSchema": other_schema }),
    )
    .await;
    let read_after_refusal = call(
        router,
        "GetIndex",
        json!({ "id": &id, "indexName": &index_name }),
    )
    .await;
    let new_name = call(
        router,
        "CreateIndex",
        json!({ "id": &id, "indexName": &added }),
    )
    .await;
    let read_new_name = call(router, "GetIndex", json!({ "id": &id, "indexName": &added })).await;

    // The transition the clock owes runs one way, so a collection still reported in the
    // transient status now was in it for every call above.
    if text(&read_collection(router, &id).await, "status") != status {
        return false;
    }

    // The rule: the IDENTICAL create again is answered with the index it already made.
    let (http_status, code, answer) = &retry;
    assert_eq!(
        *http_status, 200,
        "case CreateIndex({case}, retried in {status}): expected 200, got {http_status} {code} \
         with {answer}"
    );
    let (http_status, code, answer) = &read_after_retry;
    assert_eq!(
        *http_status, 200,
        "case GetIndex({case}, after the retry): expected 200, got {http_status} {code} with \
         {answer}"
    );
    let read_back = answer.get("indexSchema").cloned();
    assert_eq!(
        &read_back, stored,
        "case GetIndex({case}, after the retry): expected the stored schema untouched, \
         {stored:?}, got {read_back:?}"
    );

    // The same name carrying ANOTHER schema is a different request, so it is still the
    // name-taken conflict rather than a success or the status refusal.
    let (http_status, code, answer) = &other;
    assert_eq!(
        (*http_status, code.as_str()),
        (CLIENT_ERROR, CONFLICT),
        "case CreateIndex({case}, another schema under the same name): expected {CLIENT_ERROR} \
         {CONFLICT}, got {http_status} {code} with {answer}"
    );
    let got = text(answer, "message");
    let expected = name_taken_message(&index_name, &id);
    assert_eq!(
        got, expected,
        "case CreateIndex({case}, another schema under the same name): expected {expected:?}, \
         got {got:?}"
    );
    let (http_status, code, answer) = &read_after_refusal;
    assert_eq!(
        *http_status, 200,
        "case GetIndex({case}, after the refused create): expected 200, got {http_status} {code} \
         with {answer}"
    );
    let read_back = answer.get("indexSchema").cloned();
    assert_eq!(
        &read_back, stored,
        "case GetIndex({case}, after the refused create): expected the stored schema untouched, \
         {stored:?}, got {read_back:?}"
    );

    // A NEW index WOULD be added, so the collection's status still refuses it...
    let (http_status, code, answer) = &new_name;
    assert_eq!(
        (*http_status, code.as_str()),
        (CLIENT_ERROR, CONFLICT),
        "case CreateIndex({case}, a new index name): expected {CLIENT_ERROR} {CONFLICT}, got \
         {http_status} {code} with {answer}"
    );
    let got = text(answer, "message");
    let expected = status_conflict_message(&id, status);
    assert_eq!(
        got, expected,
        "case CreateIndex({case}, a new index name): expected {expected:?}, got {got:?}"
    );
    // ...and nothing was stored under it.
    let (http_status, code, answer) = &read_new_name;
    assert_eq!(
        (*http_status, code.as_str()),
        (CLIENT_ERROR, NOT_FOUND),
        "case GetIndex({case}, the new index name): a refused create stores nothing, got \
         {http_status} {code} with {answer}"
    );
    true
}

/// The retry of an accepted create is answered, whatever status the collection is in, while
/// a request that would add a NEW index is still held to that status.
#[tokio::test]
async fn a_retried_create_is_answered_while_its_collection_is_mid_transition() {
    let router = emulator();

    for (position, (case, status, stored, other_schema)) in rows().into_iter().enumerate() {
        let mut held = false;
        for attempt in 0..WINDOW_ATTEMPTS {
            let suffix = format!("{position}-{attempt}");
            if attempt_row(&router, &suffix, case, status, &stored, &other_schema).await {
                held = true;
                break;
            }
        }
        assert!(
            held,
            "case {case:?}: the collection left {status} before the row could read its answers \
             on all {WINDOW_ATTEMPTS} attempts"
        );
    }
}

/// The same retry is answered the same way on either side of the transition: the status the
/// collection happens to be in is not part of what makes a repeat of an accepted create a
/// repeat.
#[tokio::test]
async fn the_answer_to_a_retried_create_does_not_depend_on_the_collections_status() {
    let router = emulator();
    let collection = active_collection(&router, "retry-across-statuses").await;
    let id = text(&collection, "id");
    let index_name = "spanning";
    let schema = json!({ "mappings": { "properties": { "ts": { "type": "date" } } } });

    create_index(&router, &id, index_name, Some(&schema), "the first create").await;

    // Before the transition, while the collection is usable.
    create_index(&router, &id, index_name, Some(&schema), "retried while ACTIVE").await;

    // And inside the `UPDATING` window the update opens.
    let mut held = false;
    for _ in 0..WINDOW_ATTEMPTS {
        if drive_into(&router, &id, UPDATING, "retried while UPDATING").await {
            held = true;
            break;
        }
    }
    assert!(
        held,
        "case retried while UPDATING: the collection never stayed in {UPDATING} long enough to \
         read an answer in it, on all {WINDOW_ATTEMPTS} attempts"
    );
    create_index(&router, &id, index_name, Some(&schema), "retried while UPDATING").await;

    // The `UPDATING -> ACTIVE` move is owed by the clock, so this waits for the wall clock
    // rather than for a call. The bound is many times the emulator's own span, so a loaded
    // host cannot turn the wait into a failure.
    for _ in 0..40 {
        if text(&read_collection(&router, &id).await, "status") == ACTIVE {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let settled = text(&read_collection(&router, &id).await, "status");
    assert_eq!(
        settled, ACTIVE,
        "case await(UPDATING -> ACTIVE): expected the collection back in {ACTIVE}, got {settled:?}"
    );

    create_index(
        &router,
        &id,
        index_name,
        Some(&schema),
        "retried once the update settled",
    )
    .await;
    let read_back = stored_schema(&router, &id, index_name, "after every retry").await;
    assert_eq!(
        read_back.as_ref(),
        Some(&schema),
        "case GetIndex(after every retry): expected the schema the first create stored, \
         {schema}, got {read_back:?}"
    );

    // A NEW index is admitted again now the collection is usable, so the status guard the
    // retries bypassed is still in force where it applies.
    create_index(
        &router,
        &id,
        "added-once-active",
        Some(&schema),
        "a new index once the update settled",
    )
    .await;
    let read_back = stored_schema(
        &router,
        &id,
        "added-once-active",
        "the new index once active",
    )
    .await;
    assert_eq!(
        read_back.as_ref(),
        Some(&schema),
        "case GetIndex(the new index once active): expected {schema}, got {read_back:?}"
    );
}
