// SPDX-License-Identifier: CC0-1.0

//! The precondition a TAGGING operation's `resourceArn` carries, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — through the harness this crate's
//! `collection_lifecycle` suite owns, so the boundary validator, the protocol codec and the
//! handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: a `resourceArn` addresses a resource this account holds whose
//! deletion has NOT been accepted, so every operation that takes one — the tag read as much
//! as the two tag writes — refuses an ARN that addresses no such resource with
//! `ResourceNotFoundException`, and accepts one that does whatever non-deleting status the
//! resource is in. Each of the three operations documents the same sentence for the member
//! (`smithy_query(query="doc", shape="ListTagsForResourceRequest", member="resourceArn")`,
//! and the same on `TagResourceRequest` and `UntagResourceRequest`): the resource "must be
//! active (not in the `DELETING` state), and must be owned by the account ID included in
//! the request".
//!
//! The table spans what the rule generalises over — which tagging operation takes the ARN,
//! which KIND of resource it addresses, which status that resource is in (not yet created,
//! mid-create, mid-update, deletion accepted, purged, removed), and each way the ARN can
//! address nothing at all (another Region, an id nothing carries, a resource half naming
//! neither kind) — rather than the one request the issue quoted.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_group_lifecycle::create_group;
use crate::collection_lifecycle::{
    at, call, emulator_with_engine, text, ACCOUNT, CLIENT_ERROR,
};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// Long enough for a collection's clock-owed transitions (`CREATING`/`UPDATING` ->
/// `ACTIVE`, and the purge of a `DELETING` collection) to fall due; the emulator's span is
/// 500 ms and nothing sleeps for it inside a handler.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// Every operation whose input is a `resourceArn`, with the rest of a well-formed request
/// for it. One table, so no case can be asserted of the writes but not of the read.
fn tagging_plane() -> Vec<(&'static str, Value)> {
    vec![
        ("ListTagsForResource", json!({})),
        (
            "TagResource",
            json!({ "tags": [{ "key": "team", "value": "search" }] }),
        ),
        ("UntagResource", json!({ "tagKeys": ["team"] })),
    ]
}

/// The `message` an error body carries, whichever casing the envelope used.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The tags `ListTagsForResource` reports for `arn`, sorted by key.
async fn listed_tags(router: &MultiServiceRouter, arn: &str, case: &str) -> Vec<(String, String)> {
    let (status, code, answer) =
        call(router, "ListTagsForResource", json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case ListTagsForResource({case}): expected 200, got {status} {code} with {answer}"
    );
    let mut tags: Vec<(String, String)> = at(&answer, "tags")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| (text(tag, "key"), text(tag, "value")))
        .collect();
    tags.sort();
    tags
}

/// Assert that every tagging operation REFUSES `arn` with the modelled not-found, carrying
/// `expected_message`.
async fn refused(router: &MultiServiceRouter, arn: &str, case: &str, expected_message: &str) {
    for (operation, extra) in tagging_plane() {
        let mut request = extra.clone();
        request["resourceArn"] = json!(arn);
        let (status, code, answer) = call(router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case {operation}({case}): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
        );
        let said = message(&answer);
        assert_eq!(
            said, expected_message,
            "case {operation}({case}): expected the refusal to say {expected_message:?}, got {said:?}"
        );
    }
}

/// Assert that every tagging operation SERVES `arn`, by round-tripping a tag write through
/// the read: the write is reported, and its removal is too.
async fn served(router: &MultiServiceRouter, arn: &str, case: &str) {
    let (status, code, answer) = call(
        router,
        "TagResource",
        json!({ "resourceArn": arn, "tags": [{ "key": "team", "value": "search" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case TagResource({case}): expected 200, got {status} {code} with {answer}"
    );
    let got = listed_tags(router, arn, case).await;
    assert_eq!(
        got,
        vec![("team".to_string(), "search".to_string())],
        "case ListTagsForResource({case}): expected the write to be reported, got {got:?}"
    );
    let (status, code, answer) = call(
        router,
        "UntagResource",
        json!({ "resourceArn": arn, "tagKeys": ["team"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case UntagResource({case}): expected 200, got {status} {code} with {answer}"
    );
    let got = listed_tags(router, arn, case).await;
    assert!(
        got.is_empty(),
        "case ListTagsForResource({case}): expected the removal to be reported, got {got:?}"
    );
}

/// The `CollectionDetail` `BatchGetCollection` reports for `id`, or `Value::Null`.
async fn read_collection(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, code, answer) = call(router, "BatchGetCollection", json!({ "ids": [id] })).await;
    assert_eq!(
        status, 200,
        "case BatchGetCollection({id}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "collectionDetails")
        .as_array()
        .and_then(|details| details.first())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Create a collection encrypted with the AWS-owned key, answering its
/// `createCollectionDetail` — the create's own answer, so the caller sees it before the
/// clock has settled its `CREATING` status.
async fn create_collection(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, code, answer) = call(
        router,
        "CreateCollection",
        json!({ "name": name, "encryptionConfig": { "aWSOwnedKey": true } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollection({name}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "createCollectionDetail").clone()
}

/// Poll until `id` reports `wanted`, answering its `CollectionDetail`.
async fn await_status(router: &MultiServiceRouter, id: &str, wanted: &str) -> Value {
    for _ in 0..10 {
        let detail = read_collection(router, id).await;
        if text(&detail, "status") == wanted {
            return detail;
        }
        tokio::time::sleep(SETTLE_WAIT / 5).await;
    }
    panic!("case await_status({id}): the collection never reached {wanted}");
}

// ---------------------------------------------------------------------------
// A resource the account holds and has not deleted is served by all three, in
// whichever non-deleting status it is in.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_tagging_operation_serves_a_resource_whose_deletion_was_not_accepted() {
    let (router, _engine) = emulator_with_engine();

    // A collection still mid-create: `CREATING` is not `DELETING`, and the member's
    // precondition excludes only the latter.
    let creating = create_collection(&router, "tag-precondition-creating").await;
    let creating_id = text(&creating, "id");
    assert_eq!(
        text(&creating, "status"),
        "CREATING",
        "case CreateCollection(tag precondition): the create reports the transient status it starts, got {creating}"
    );
    served(
        &router,
        &text(&creating, "arn"),
        "a collection the clock has not settled yet",
    )
    .await;

    // The same collection once it is ACTIVE.
    let active = await_status(&router, &creating_id, "ACTIVE").await;
    let active_arn = text(&active, "arn");
    served(&router, &active_arn, "an ACTIVE collection").await;

    // And while an accepted UPDATE is still in flight, which is another non-terminal
    // status that is not `DELETING`. `vectorOptions` is the member that reconfigures the
    // collection, so it is the update that leaves one in flight.
    let (status, code, answer) = call(
        &router,
        "UpdateCollection",
        json!({ "id": creating_id, "vectorOptions": { "ServerlessVectorAcceleration": "ENABLED" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateCollection(before tagging): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        text(&answer, "updateCollectionDetail.status"),
        "UPDATING",
        "case UpdateCollection(before tagging): the update reports the transient status it starts, got {answer}"
    );
    served(&router, &active_arn, "a collection mid-update").await;

    // A collection GROUP is the other KIND of resource a `resourceArn` addresses, and the
    // same precondition resolves it.
    let group = create_group(
        &router,
        REGION,
        json!({ "name": "tag-precondition-group", "standbyReplicas": "ENABLED" }),
    )
    .await;
    served(
        &router,
        &text(&group, "arn"),
        "a collection group this account holds",
    )
    .await;
}

// ---------------------------------------------------------------------------
// And the ARNs that address no such resource: refused by all three alike,
// naming the kind and the id the ARN carries.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_tagging_operation_refuses_an_arn_that_addresses_no_live_resource() {
    let (router, engine) = emulator_with_engine();
    let state = service_state(&*engine).expect("the store is reachable from the shared engine");

    let collection = create_collection(&router, "tag-precondition-deleted").await;
    let id = text(&collection, "id");
    let arn = text(&collection, "arn");
    await_status(&router, &id, "ACTIVE").await;
    let (status, code, answer) = call(
        &router,
        "TagResource",
        json!({ "resourceArn": arn, "tags": [{ "key": "team", "value": "search" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case TagResource(before the delete): expected 200, got {status} {code} with {answer}"
    );

    // An ARN that addresses nothing, each way it can miss: another Region's collection of
    // this id, an id no collection carries, a group id no group carries, and a resource
    // half naming neither addressable kind.
    let absent_id = "0".repeat(20);
    let absent_group_id = "1".repeat(20);
    refused(
        &router,
        &format!("arn:aws:aoss:eu-west-1:{ACCOUNT}:collection/{id}"),
        "a collection of another Region",
        &format!("Collection ID {id} is not found"),
    )
    .await;
    refused(
        &router,
        &format!("arn:aws:aoss:{REGION}:{ACCOUNT}:collection/{absent_id}"),
        "an id no collection carries",
        &format!("Collection ID {absent_id} is not found"),
    )
    .await;
    refused(
        &router,
        &format!("arn:aws:aoss:{REGION}:{ACCOUNT}:collection-group/{absent_group_id}"),
        "an id no collection group carries",
        &format!("Collection Group ID {absent_group_id} is not found"),
    )
    .await;
    let unaddressable = format!("arn:aws:aoss:{REGION}:{ACCOUNT}:dashboards/default");
    refused(
        &router,
        &unaddressable,
        "a resource half naming neither kind",
        &format!("Resource {unaddressable} is not found"),
    )
    .await;

    // A collection whose deletion has been ACCEPTED: the tagging plane's precondition
    // names exactly this status, so all three refuse it.
    let (status, code, answer) = call(&router, "DeleteCollection", json!({ "id": id })).await;
    assert_eq!(
        status, 200,
        "case DeleteCollection(tagged collection): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        text(&answer, "deleteCollectionDetail.status"),
        "DELETING",
        "case DeleteCollection(tagged collection): the delete reports the status it accepted, got {answer}"
    );
    refused(
        &router,
        &arn,
        "a collection whose deletion was accepted",
        &format!("Collection ID {id} is not found"),
    )
    .await;

    // The collection itself is still READ in `DELETING` until the clock makes its purge
    // due, so the refusals above are the member's precondition and not the record having
    // gone early — and none of the three refusals touched the stored record.
    let deleting = read_collection(&router, &id).await;
    assert_eq!(
        text(&deleting, "status"),
        "DELETING",
        "case BatchGetCollection(while DELETING): the collection is still reported, got {deleting}"
    );
    let stored = state
        .lock()
        .collection_by_arn(&arn)
        .map(|record| {
            record
                .tags
                .iter()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect::<Vec<(String, String)>>()
        })
        .expect("the deleting collection is still stored under its ARN");
    assert_eq!(
        stored,
        vec![("team".to_string(), "search".to_string())],
        "case the refusals(while DELETING): a refused call stores nothing, got {stored:?}"
    );

    // Once the purge falls due nothing addresses it either, with the same refusal.
    tokio::time::sleep(SETTLE_WAIT).await;
    refused(
        &router,
        &arn,
        "a collection the clock has purged",
        &format!("Collection ID {id} is not found"),
    )
    .await;

    // A collection GROUP's delete removes its record outright, so its ARN addresses
    // nothing from then on.
    let group = create_group(
        &router,
        REGION,
        json!({ "name": "tag-precondition-removed-group", "standbyReplicas": "ENABLED" }),
    )
    .await;
    let group_id = text(&group, "id");
    let group_arn = text(&group, "arn");
    let (status, code, answer) =
        call(&router, "DeleteCollectionGroup", json!({ "id": group_id })).await;
    assert_eq!(
        status, 200,
        "case DeleteCollectionGroup(tag precondition): expected 200, got {status} {code} with {answer}"
    );
    refused(
        &router,
        &group_arn,
        "a collection group that was deleted",
        &format!("Collection Group ID {group_id} is not found"),
    )
    .await;
}
