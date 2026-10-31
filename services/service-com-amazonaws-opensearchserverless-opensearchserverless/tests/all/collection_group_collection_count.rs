// SPDX-License-Identifier: CC0-1.0

//! What `numberOfCollections` counts on a collection-group read, end to end over the wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body), through the
//! harness this crate's `collection_lifecycle` suite owns, so the boundary validator, the
//! protocol codec and the handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: a read that derives its answer from this account's collections
//! applies the transitions the clock has ALREADY made due — the status a collection settles
//! into, and the removal of one whose accepted deletion has fallen due — before computing
//! that answer, so every read of the same state reports the same collections however the
//! caller reaches them and whatever was called before. `CollectionGroupDetail`'s
//! `numberOfCollections` ("the number of collections in each group",
//! `smithy_query(query="operation", operation="BatchGetCollectionGroup")`) is such an
//! answer: a group counts the collections that are still stored at the instant of the read,
//! which is what the collection readers and the group delete read too.
//!
//! The tables below span what the rule generalises over: groups holding none, one, two and
//! several collections; none, some and all of them deleted; the window BEFORE the deletion's
//! purge has fallen due as well as after it; id- as well as name-addressed group reads; four
//! Regions and both `StandbyReplicas` members — with the group read taken as the FIRST call
//! after the purge falls due, so no other operation can have settled the store for it.

use std::time::Duration;

use serde_json::{json, Value};

use crate::collection_group_delete::{detail_at_collection, error_at_collection};
use crate::collection_group_lifecycle::{
    batch_get, create_group, detail_at, error_at, NOT_FOUND_CODE,
};
use crate::collection_lifecycle::{at, call_in, emulator, text, CLIENT_ERROR};

/// Long enough for a collection transition this emulator owes — `CREATING -> ACTIVE`, and
/// the purge an accepted deletion owes — to fall due; the emulator's span is 500 ms and
/// nothing sleeps for it inside a handler.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// The `numberOfCollections` reported by the `collectionGroupDetails` entry at `index`.
fn counted_at(answer: &Value, index: usize) -> Option<i64> {
    at(&detail_at(answer, index), "numberOfCollections").as_i64()
}

/// Create the collection `name` in `group` in `region`, asserting it was accepted, and
/// answer the `CollectionId` the service minted.
///
/// A collection's encryption setting has to come from somewhere — the request's
/// `encryptionConfig` or an `encryption` security policy covering its name — and this suite
/// is about the GROUP's count, so every row carries the AWS-owned key.
async fn create_collection_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    name: &str,
    group: &str,
) -> String {
    let (status, code, answer) = call_in(
        router,
        region,
        "CreateCollection",
        json!({
            "name": name,
            "collectionGroupName": group,
            "encryptionConfig": { "aWSOwnedKey": true },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollection({name} in {group}): expected 200, got {status} {code} with {answer}"
    );
    text(&answer, "createCollectionDetail.id")
}

/// Delete the collection `id` names in `region`, asserting the delete was accepted, and
/// answer the status the delete reported.
async fn delete_collection(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    id: &str,
) -> String {
    let (status, code, answer) =
        call_in(router, region, "DeleteCollection", json!({ "id": id })).await;
    assert_eq!(
        status, 200,
        "case DeleteCollection({id}): expected 200, got {status} {code} with {answer}"
    );
    text(&answer, "deleteCollectionDetail.status")
}

/// How many collections `ListCollections` reports under the group filter `group`.
async fn listed_in_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    group: &str,
) -> usize {
    let (status, code, answer) = call_in(
        router,
        region,
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

/// Read the collection `name` through `BatchGetCollection` in `region`, answering the whole
/// response.
async fn read_collection(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    name: &str,
) -> Value {
    let (status, code, answer) =
        call_in(router, region, "BatchGetCollection", json!({ "names": [name] })).await;
    assert_eq!(
        status, 200,
        "case BatchGetCollection({name}): expected 200, got {status} {code} with {answer}"
    );
    answer
}

/// Delete the collection group `id` names in `region`, answering the status, the error code
/// and the parsed body — the raw call, since the point is which deletes are refused.
async fn delete_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    id: &str,
) -> (u16, String, Value) {
    call_in(router, region, "DeleteCollectionGroup", json!({ "id": id })).await
}

// ---------------------------------------------------------------------------
// The count is taken as of the read: a collection whose purge the clock has already made
// due is counted by nobody, and the group read says so even when it is the first call
// after that purge fell due.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn number_of_collections_counts_the_collections_still_stored_at_the_read() {
    let router = emulator();

    // (region, standbyReplicas, group, collections to create, how many of them to delete)
    let cases: Vec<(&str, &str, &str, usize, usize)> = vec![
        // A group that never held a collection: nothing is owed and nothing is counted.
        ("us-east-1", "ENABLED", "count-none", 0, 0),
        // One collection, deleted: the group ends up empty.
        ("us-east-1", "DISABLED", "count-one-gone", 1, 1),
        // Two collections, neither deleted: nothing due must change the count.
        ("us-west-2", "ENABLED", "count-two-live", 2, 0),
        // Three collections, two deleted: a PARTIAL purge leaves the remainder counted.
        ("eu-west-1", "DISABLED", "count-three-part", 3, 2),
        // Four collections, all deleted: the count falls to zero in one window.
        ("ap-southeast-2", "ENABLED", "count-four-gone", 4, 4),
    ];

    for (region, standby_replicas, group, created, deleted) in cases {
        let case = format!("{group} in {region}: {created} created, {deleted} deleted");
        let detail = create_group(
            &router,
            region,
            json!({ "name": group, "standbyReplicas": standby_replicas }),
        )
        .await;
        let group_id = text(&detail, "id");

        let names: Vec<String> = (0..created).map(|n| format!("{group}-c{n}")).collect();
        let mut ids: Vec<String> = Vec::new();
        for name in &names {
            ids.push(create_collection_in(&router, region, name, group).await);
        }

        // A collection whose create is still settling `CREATING -> ACTIVE` is a collection
        // the group holds, so the count right after the creates is all of them.
        let answer = batch_get(&router, region, json!({ "ids": [&group_id] })).await;
        assert_eq!(
            counted_at(&answer, 0),
            Some(created as i64),
            "case {case}: a freshly created collection is counted, expected {created}, got {answer}"
        );

        tokio::time::sleep(SETTLE_WAIT).await;
        for id in ids.iter().take(deleted) {
            let status = delete_collection(&router, region, id).await;
            assert_eq!(
                status, "DELETING",
                "case {case}: an accepted delete reports DELETING, got {status:?}"
            );
        }

        // A deleted collection stays stored until its purge falls due, and it still belongs
        // to the group until then: the count is unchanged inside that window.
        let answer = batch_get(&router, region, json!({ "names": [group] })).await;
        assert_eq!(
            counted_at(&answer, 0),
            Some(created as i64),
            "case {case}: a collection whose purge is not yet due is still counted, expected \
             {created}, got {answer}"
        );

        // Past the purge window, with the group read as the FIRST call: no other operation
        // can have settled the store on its behalf.
        tokio::time::sleep(SETTLE_WAIT).await;
        let remaining = (created - deleted) as i64;
        for selector in [json!({ "ids": [&group_id] }), json!({ "names": [group] })] {
            let read_case = format!("{case}, read with {selector}");
            let answer = batch_get(&router, region, selector).await;
            assert_eq!(
                counted_at(&answer, 0),
                Some(remaining),
                "case {read_case}: a purged collection is counted by nobody, expected \
                 {remaining}, got {answer}"
            );
            // The read reports the group itself unchanged — it settled collections, it did
            // not touch the group.
            assert_eq!(
                text(&detail_at(&answer, 0), "id"),
                group_id,
                "case {read_case}: the group itself is still reported, got {answer}"
            );
        }

        // Every other read of the same state agrees with that count.
        assert_eq!(
            listed_in_group(&router, region, group).await as i64,
            remaining,
            "case {case}: ListCollections agrees with the group's count, expected {remaining}"
        );
        for (index, name) in names.iter().enumerate() {
            let read = read_collection(&router, region, name).await;
            if index < deleted {
                assert_eq!(
                    text(&error_at_collection(&read, 0), "errorCode"),
                    NOT_FOUND_CODE,
                    "case {case}: the purged collection {name} is not found, got {read}"
                );
            } else {
                assert_eq!(
                    text(&detail_at_collection(&read, 0), "name"),
                    *name,
                    "case {case}: the live collection {name} is still readable, got {read}"
                );
            }
        }

        // And the group delete, which decides on the same emptiness, agrees with the count
        // the read reported.
        let (status, code, body) = delete_group(&router, region, &group_id).await;
        if remaining == 0 {
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case}: a group the read counts empty is deletable, got {status} {code} \
                 with {body}"
            );
            let answer = batch_get(&router, region, json!({ "ids": [&group_id] })).await;
            assert_eq!(
                text(&error_at(&answer, 0), "errorCode"),
                NOT_FOUND_CODE,
                "case {case}: the deleted group is gone from the read path, got {answer}"
            );
        } else {
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ValidationException"),
                "case {case}: a group the read counts non-empty is refused, got {status} {code} \
                 with {body}"
            );
            // The refused delete changed nothing, and a repeated read reports the same
            // count: the read is not what moves the state.
            let answer = batch_get(&router, region, json!({ "ids": [&group_id] })).await;
            assert_eq!(
                counted_at(&answer, 0),
                Some(remaining),
                "case {case}: the count is stable across reads, expected {remaining}, got {answer}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The read settles what the clock already owed and nothing more: inside the deletion's
// window the collection is still counted, however many times the group is read.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_group_read_does_not_bring_a_pending_purge_forward() {
    let router = emulator();

    // (group, how many collections it holds — all of them deleted in the window below)
    let cases: Vec<(&str, usize)> = vec![("window-one", 1), ("window-three", 3)];

    for (group, held) in cases {
        let detail = create_group(
            &router,
            "us-east-1",
            json!({ "name": group, "standbyReplicas": "ENABLED" }),
        )
        .await;
        let group_id = text(&detail, "id");

        let names: Vec<String> = (0..held).map(|n| format!("{group}-c{n}")).collect();
        let mut ids: Vec<String> = Vec::new();
        for name in &names {
            ids.push(create_collection_in(&router, "us-east-1", name, group).await);
        }
        tokio::time::sleep(SETTLE_WAIT).await;
        for id in &ids {
            delete_collection(&router, "us-east-1", id).await;
        }

        // Repeated reads inside the window all report the collections as held: a read may
        // only materialise what the clock already made due, so none of them may purge
        // early.
        for round in 0..3 {
            let answer = batch_get(&router, "us-east-1", json!({ "ids": [&group_id] })).await;
            assert_eq!(
                counted_at(&answer, 0),
                Some(held as i64),
                "case {group} read {round} inside the purge window: expected {held}, got {answer}"
            );
        }
        // The collections are still readable in `DELETING` after those reads, so nothing
        // the read did removed them.
        for name in &names {
            let read = read_collection(&router, "us-east-1", name).await;
            assert_eq!(
                text(&detail_at_collection(&read, 0), "status"),
                "DELETING",
                "case {group}: {name} is still readable in DELETING, got {read}"
            );
        }

        // Once the window has passed, the same read reports the group as empty.
        tokio::time::sleep(SETTLE_WAIT).await;
        let answer = batch_get(&router, "us-east-1", json!({ "ids": [&group_id] })).await;
        assert_eq!(
            counted_at(&answer, 0),
            Some(0),
            "case {group} read past the purge window: expected 0, got {answer}"
        );
    }
}
