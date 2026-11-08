// SPDX-License-Identifier: CC0-1.0

//! The containment a collection's `collectionGroupName` asserts, end to end over the wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body), through the
//! harness this crate's `collection_lifecycle` suite owns, so the boundary validator, the
//! protocol codec and the handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: a request member that names ANOTHER resource of the account, to be
//! stored on the resource being created, is only admitted when a resource of that kind is
//! stored under the name — otherwise the request is refused and nothing is written, so no
//! read can report a reference the store cannot resolve. In this service that member is
//! `CreateCollectionRequest$collectionGroupName`, documented as "The name of the collection
//! group that contains this collection", so an admitted create is one whose group a
//! collection-group read answers, and a refused one leaves no collection behind.
//!
//! The tables below span what the rule generalises over: names no group carries at both
//! modelled length bounds, names that near-miss a stored group's, a name a resource of
//! ANOTHER kind holds, a name whose group was created and then deleted, and — on the
//! accepted side — groups differing in name length and `StandbyReplicas`, plus the request
//! that names no group at all.

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::collection_group_name_not_found_message;

use crate::collection_group_delete::{detail_at_collection, error_at_collection};
use crate::collection_group_lifecycle::{batch_get, create_group, detail_at, error_at};
use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// `CollectionGroupName`'s modelled `@length(min: 3, max: 32)`
/// (`smithy_query(query="operation", operation="CreateCollection")` ->
/// `collectionGroupName`).
const GROUP_NAME_MIN: usize = 3;
const GROUP_NAME_MAX: usize = 32;

/// A `CollectionGroupName` of exactly `len` characters inside the modelled
/// `^[a-z][a-z0-9-]+$`.
fn group_name_of_length(len: usize) -> String {
    format!("g{}", "r".repeat(len - 1))
}

/// Create the collection `body` describes, answering the status, the error code and the
/// parsed body — the raw call, since this suite is about which creates are refused.
async fn create_collection(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
) -> (u16, String, Value) {
    call(router, "CreateCollection", body).await
}

/// The request body for a collection named `name`, in `group` when one is named.
///
/// A collection's encryption setting has to come from somewhere — the request's
/// `encryptionConfig` or an `encryption` policy covering its name — and this suite is about
/// the GROUP, so every row carries the AWS-owned key.
fn create_body(name: &str, group: Option<&str>) -> Value {
    let mut body = json!({ "name": name, "encryptionConfig": { "aWSOwnedKey": true } });
    if let Some(group) = group {
        body["collectionGroupName"] = json!(group);
    }
    body
}

/// Read the collection `name` through `BatchGetCollection`, answering the whole response.
async fn read_collection(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Value {
    let (status, code, answer) = call(router, "BatchGetCollection", json!({ "names": [name] })).await;
    assert_eq!(
        status, 200,
        "case BatchGetCollection({name}): expected 200, got {status} {code} with {answer}"
    );
    answer
}

/// How many collections `ListCollections` reports for the group filter `group`.
async fn listed_in_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    group: &str,
) -> usize {
    let (status, code, answer) = call(
        router,
        "ListCollections",
        json!({ "collectionFilters": { "collectionGroupName": group }, "maxResults": 100 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ListCollections({group}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "collectionSummaries")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// A `collectionGroupName` no stored group carries is refused, and the refusal stores
// nothing: no collection to read, and nothing the group filter lists.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn creating_a_collection_in_an_unstored_group_is_refused_and_stores_nothing() {
    let router = emulator();

    // The stored neighbours the near-miss rows below are measured against: a group, and a
    // COLLECTION whose name a group row then asks for (a name taken by another kind of
    // resource is not a group).
    create_group(
        &router,
        "us-east-1",
        json!({ "name": "stored-group", "standbyReplicas": "ENABLED" }),
    )
    .await;
    let (status, code, answer) =
        create_collection(&router, create_body("named-collection", None)).await;
    assert_eq!(
        status, 200,
        "case CreateCollection(named-collection): expected 200, got {status} {code} with {answer}"
    );

    // A group that existed and was deleted: its name is free again, and a later create
    // naming it is refused like any other absent group — the check reads live state, not a
    // list of names ever used.
    let detail = create_group(
        &router,
        "us-east-1",
        json!({ "name": "temporary-group", "standbyReplicas": "DISABLED" }),
    )
    .await;
    let (status, code, answer) = call(
        &router,
        "DeleteCollectionGroup",
        json!({ "id": text(&detail, "id") }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteCollectionGroup(temporary-group): expected 200, got {status} {code} with {answer}"
    );

    let shortest = group_name_of_length(GROUP_NAME_MIN);
    let longest = group_name_of_length(GROUP_NAME_MAX);
    // (what the row is about, the collection to create, the group name it asks for)
    let rows: Vec<(&str, &str, &str)> = vec![
        ("a name no group was ever created under", "orphan-one", "ghost-group"),
        (
            "the shortest name the member admits, under which no group exists",
            "orphan-two",
            shortest.as_str(),
        ),
        (
            "the longest name the member admits, under which no group exists",
            "orphan-three",
            longest.as_str(),
        ),
        (
            "a stored group's name with one more character",
            "orphan-four",
            "stored-groups",
        ),
        ("a prefix of a stored group's name", "orphan-five", "stored-grou"),
        (
            "a name a COLLECTION holds, which is not a group",
            "orphan-six",
            "named-collection",
        ),
        ("the name of a group that was deleted", "orphan-seven", "temporary-group"),
    ];

    for (what, collection, group) in rows {
        let (status, code, answer) =
            create_collection(&router, create_body(collection, Some(group))).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case CreateCollection({collection}) with {what}: expected {CLIENT_ERROR} \
             ValidationException, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            collection_group_name_not_found_message(group),
            "case CreateCollection({collection}) with {what}: the refusal names the group it \
             could not resolve, got {answer}"
        );

        // The refusal stored nothing: the collection is absent from the read path, and the
        // group filter lists nothing under the name.
        let read = read_collection(&router, collection).await;
        assert_eq!(
            at(&detail_at_collection(&read, 0), "name"),
            &Value::Null,
            "case CreateCollection({collection}) with {what}: no collection was stored, got {read}"
        );
        assert_eq!(
            text(&error_at_collection(&read, 0), "name"),
            collection,
            "case CreateCollection({collection}) with {what}: the read reports the name as \
             not found, got {read}"
        );
        assert_eq!(
            listed_in_group(&router, group).await,
            0,
            "case CreateCollection({collection}) with {what}: the group filter lists nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// A `collectionGroupName` a stored group carries is admitted, and the containment it
// asserts is what every read reports; a request that names no group stores none.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_collection_created_in_a_stored_group_reports_a_resolvable_containment() {
    let router = emulator();

    let shortest = group_name_of_length(GROUP_NAME_MIN);
    let longest = group_name_of_length(GROUP_NAME_MAX);
    // (the group, its standbyReplicas, the collections created in it)
    let groups: Vec<(&str, &str, Vec<&str>)> = vec![
        ("stored-group", "ENABLED", vec!["first-collection"]),
        (
            shortest.as_str(),
            "DISABLED",
            vec!["second-collection", "third-collection"],
        ),
        (longest.as_str(), "ENABLED", vec!["fourth-collection"]),
    ];

    for (group, standby_replicas, collections) in &groups {
        create_group(
            &router,
            "us-east-1",
            json!({ "name": group, "standbyReplicas": standby_replicas }),
        )
        .await;
        for collection in collections {
            let (status, code, answer) =
                create_collection(&router, create_body(collection, Some(group))).await;
            assert_eq!(
                status, 200,
                "case CreateCollection({collection}) in {group}: expected 200, got {status} \
                 {code} with {answer}"
            );
            assert_eq!(
                text(&answer, "createCollectionDetail.collectionGroupName"),
                *group,
                "case CreateCollection({collection}) in {group}: the create reports the group \
                 it stored, got {answer}"
            );

            // The containment round-trips through the collection read...
            let read = read_collection(&router, collection).await;
            assert_eq!(
                text(&detail_at_collection(&read, 0), "collectionGroupName"),
                *group,
                "case BatchGetCollection({collection}): expected group {group}, got {read}"
            );
            // ...and the group the collection names is one a group read answers with a
            // detail, not with a NOT_FOUND error.
            let group_read = batch_get(&router, "us-east-1", json!({ "names": [group] })).await;
            assert_eq!(
                text(&detail_at(&group_read, 0), "name"),
                *group,
                "case BatchGetCollectionGroup({group}): the containing group is a stored \
                 resource, got {group_read}"
            );
            assert_eq!(
                at(&error_at(&group_read, 0), "errorCode"),
                &Value::Null,
                "case BatchGetCollectionGroup({group}): no error detail is owed, got {group_read}"
            );
        }

        // The group counts exactly the collections created in it, and the group filter
        // lists exactly those.
        let group_read = batch_get(&router, "us-east-1", json!({ "names": [group] })).await;
        assert_eq!(
            at(&detail_at(&group_read, 0), "numberOfCollections").as_i64(),
            Some(collections.len() as i64),
            "case numberOfCollections({group}): expected {}, got {group_read}",
            collections.len()
        );
        assert_eq!(
            listed_in_group(&router, group).await,
            collections.len(),
            "case ListCollections({group}): expected {} summaries",
            collections.len()
        );
    }

    // A request that names no group stores none: the member is optional, and a collection
    // outside every group reports nothing for it.
    let (status, code, answer) =
        create_collection(&router, create_body("ungrouped-collection", None)).await;
    assert_eq!(
        status, 200,
        "case CreateCollection(ungrouped-collection): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        at(&answer, "createCollectionDetail.collectionGroupName"),
        &Value::Null,
        "case CreateCollection(ungrouped-collection): no group is reported, got {answer}"
    );
    let read = read_collection(&router, "ungrouped-collection").await;
    assert_eq!(
        at(&detail_at_collection(&read, 0), "collectionGroupName"),
        &Value::Null,
        "case BatchGetCollection(ungrouped-collection): no group is reported, got {read}"
    );
}
