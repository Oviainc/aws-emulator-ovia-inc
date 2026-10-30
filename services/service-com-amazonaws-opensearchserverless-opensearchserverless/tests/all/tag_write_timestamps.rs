// SPDX-License-Identifier: CC0-1.0

//! What an accepted TAG write leaves on the resource's `lastModifiedDate`, end to end over
//! the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — through the harness this crate's
//! `collection_lifecycle` suite already owns, so the boundary validator, the protocol codec
//! and the handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: a resource's tags are part of the resource, so an ACCEPTED tag
//! write modifies it and the resource's `lastModifiedDate` becomes that write's instant,
//! while `createdDate` and everything the write did not touch stay where they were; a
//! REFUSED write modifies nothing and so moves neither field. `lastModifiedDate` is
//! modelled as "The date and time when the collection was last modified"
//! (`smithy_query(query="shape", shape="CollectionDetail")`).
//!
//! The table spans what the rule generalises over — which tagging operation writes, which
//! KIND of resource the `resourceArn` addresses, and the shape of the write (adding keys,
//! replacing a value, storing the same set again, an empty list, removing a subset,
//! removing a key never carried, removing every key) — rather than the one request the
//! issue quoted.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::{
    now_epoch_millis, service_state, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_group_lifecycle::create_group;
use crate::collection_lifecycle::{at, call, emulator_with_engine, text, CLIENT_ERROR};
use crate::encryption_indexes_and_tagging::active_collection;

/// The region the harness signs every request below for.
const REGION: &str = "us-east-1";

/// Long enough for the epoch-millisecond clock `lastModifiedDate` is held in to advance, so
/// a stamp a write really applied is STRICTLY later than the one before it.
const CLOCK_STEP: Duration = Duration::from_millis(3);

/// One row of the tag-write table: the write, and the tag set the resource reports after it.
struct Write {
    case: &'static str,
    operation: &'static str,
    /// The request, without `resourceArn` — the harness fills in the resource under test,
    /// so the same table runs against every KIND of taggable resource.
    members: Value,
    expected_tags: Vec<(&'static str, &'static str)>,
}

/// Every shape of accepted tag write, over a resource whose stored tag set starts empty.
fn writes() -> Vec<Write> {
    vec![
        Write {
            case: "adding two keys",
            operation: "TagResource",
            members: json!({ "tags": [{ "key": "env", "value": "prod" }, { "key": "owner", "value": "data" }] }),
            expected_tags: vec![("env", "prod"), ("owner", "data")],
        },
        Write {
            case: "replacing the value of a key already carried",
            operation: "TagResource",
            members: json!({ "tags": [{ "key": "env", "value": "staging" }] }),
            expected_tags: vec![("env", "staging"), ("owner", "data")],
        },
        Write {
            case: "storing the same tag set again",
            operation: "TagResource",
            members: json!({ "tags": [{ "key": "env", "value": "staging" }] }),
            expected_tags: vec![("env", "staging"), ("owner", "data")],
        },
        Write {
            case: "an empty tag list",
            operation: "TagResource",
            members: json!({ "tags": [] }),
            expected_tags: vec![("env", "staging"), ("owner", "data")],
        },
        Write {
            case: "a value at the empty string, which the model allows",
            operation: "TagResource",
            members: json!({ "tags": [{ "key": "owner", "value": "" }] }),
            expected_tags: vec![("env", "staging"), ("owner", "")],
        },
        Write {
            case: "removing one key",
            operation: "UntagResource",
            members: json!({ "tagKeys": ["owner"] }),
            expected_tags: vec![("env", "staging")],
        },
        Write {
            case: "removing a key the resource never carried",
            operation: "UntagResource",
            members: json!({ "tagKeys": ["never-set"] }),
            expected_tags: vec![("env", "staging")],
        },
        Write {
            case: "an empty key list",
            operation: "UntagResource",
            members: json!({ "tagKeys": [] }),
            expected_tags: vec![("env", "staging")],
        },
        Write {
            case: "removing every key",
            operation: "UntagResource",
            members: json!({ "tagKeys": ["env"] }),
            expected_tags: vec![],
        },
    ]
}

/// The tags `ListTagsForResource` reports for `arn`, sorted by key so the comparison does
/// not depend on write order.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let (status, code, answer) =
        call(router, "ListTagsForResource", json!({ "resourceArn": arn })).await;
    assert_eq!(
        status, 200,
        "case ListTagsForResource({arn}): expected 200, got {status} {code} with {answer}"
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

/// The owned form of a row's expected tag set.
fn owned(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Send the tag write `row` describes against `arn`, asserting it was accepted, and answer
/// the window `[before, after]` the write instant must lie in.
async fn accepted_write(
    router: &MultiServiceRouter,
    arn: &str,
    operation: &str,
    case: &str,
    members: &Value,
) -> (i64, i64) {
    let mut request = members.clone();
    request["resourceArn"] = json!(arn);
    let before = now_epoch_millis();
    let (status, code, answer) = call(router, operation, request).await;
    let after = now_epoch_millis();
    assert_eq!(
        status, 200,
        "case {operation}({case}): expected 200, got {status} {code} with {answer}"
    );
    (before, after)
}

#[tokio::test]
async fn every_accepted_tag_write_stamps_a_collections_last_modified_date() {
    let (router, _engine) = emulator_with_engine();

    let collection = active_collection(&router, "tag-stamped-collection").await;
    let id = text(&collection, "id");
    let arn = text(&collection, "arn");
    let created = at(&collection, "createdDate")
        .as_i64()
        .expect("the create reports a createdDate");
    let mut previous = at(&collection, "lastModifiedDate")
        .as_i64()
        .expect("the create reports a lastModifiedDate");

    for Write {
        case,
        operation,
        members,
        expected_tags,
    } in writes()
    {
        // The clock has to have moved for "the write instant" to be distinguishable from
        // the stamp the previous write left.
        tokio::time::sleep(CLOCK_STEP).await;
        let (before, after) = accepted_write(&router, &arn, operation, case, &members).await;

        let got = listed_tags(&router, &arn).await;
        let expected = owned(&expected_tags);
        assert_eq!(
            got, expected,
            "case {operation}({case}): expected the stored tags {expected:?}, got {got:?}"
        );

        let (status, code, answer) =
            call(&router, "BatchGetCollection", json!({ "ids": [id.clone()] })).await;
        assert_eq!(
            status, 200,
            "case BatchGetCollection(after {operation}({case})): expected 200, got {status} {code} with {answer}"
        );
        let detail = at(&answer, "collectionDetails")
            .as_array()
            .and_then(|details| details.first())
            .cloned()
            .unwrap_or(Value::Null);
        let stamped = at(&detail, "lastModifiedDate").as_i64();
        assert!(
            stamped.is_some_and(|stamped| stamped >= before && stamped <= after),
            "case {operation}({case}): lastModifiedDate must be the write instant, inside [{before}, {after}], got {stamped:?}"
        );
        assert!(
            stamped.is_some_and(|stamped| stamped > previous),
            "case {operation}({case}): lastModifiedDate must advance past the previous write's {previous}, got {stamped:?}"
        );
        assert_eq!(
            at(&detail, "createdDate").as_i64(),
            Some(created),
            "case {operation}({case}): createdDate does not move, got {detail}"
        );
        assert_eq!(
            text(&detail, "status"),
            "ACTIVE",
            "case {operation}({case}): a tag write starts no status transition, got {detail}"
        );
        previous = stamped.unwrap_or_default();
    }
}

#[tokio::test]
async fn every_accepted_tag_write_stamps_a_collection_groups_last_modified_date() {
    let (router, engine) = emulator_with_engine();
    let state = service_state(&*engine).expect("the store is reachable from the shared engine");

    let group = create_group(
        &router,
        REGION,
        json!({ "name": "tag-stamped-group", "standbyReplicas": "ENABLED" }),
    )
    .await;
    let arn = text(&group, "arn");
    let created = at(&group, "createdDate")
        .as_i64()
        .expect("the create reports a createdDate");

    // No modelled read of a GROUP reports its `lastModifiedDate` — `CollectionGroupDetail`
    // and `CollectionGroupSummary` model none and only `UpdateCollectionGroupDetail` does
    // (`smithy_query(query="shape", shape="CollectionGroupDetail,CollectionGroupSummary")`),
    // and an update is itself a write that would stamp the field — so the stored record is
    // what this suite reads the stamp off, while the write under test still goes over the
    // wire.
    let stamp = |arn: &str| -> i64 {
        state
            .lock()
            .collection_group_by_arn(arn)
            .map(|record| record.last_modified_date)
            .expect("the created group is stored under its ARN")
    };
    let stored_created = state
        .lock()
        .collection_group_by_arn(&arn)
        .map(|record| record.created_date)
        .expect("the created group is stored under its ARN");
    assert_eq!(
        stored_created, created,
        "case CreateCollectionGroup: the stored createdDate is the one the create reported, got {stored_created}"
    );
    let mut previous = stamp(&arn);

    for Write {
        case,
        operation,
        members,
        expected_tags,
    } in writes()
    {
        tokio::time::sleep(CLOCK_STEP).await;
        let (before, after) = accepted_write(&router, &arn, operation, case, &members).await;

        let got = listed_tags(&router, &arn).await;
        let expected = owned(&expected_tags);
        assert_eq!(
            got, expected,
            "case {operation}({case}, group): expected the stored tags {expected:?}, got {got:?}"
        );

        let stamped = stamp(&arn);
        assert!(
            stamped >= before && stamped <= after,
            "case {operation}({case}, group): lastModifiedDate must be the write instant, inside [{before}, {after}], got {stamped}"
        );
        assert!(
            stamped > previous,
            "case {operation}({case}, group): lastModifiedDate must advance past the previous write's {previous}, got {stamped}"
        );
        let still_created = state
            .lock()
            .collection_group_by_arn(&arn)
            .map(|record| record.created_date)
            .expect("the group is still stored");
        assert_eq!(
            still_created, created,
            "case {operation}({case}, group): createdDate does not move, got {still_created}"
        );
        previous = stamped;
    }
}

#[tokio::test]
async fn a_refused_tag_write_leaves_last_modified_date_where_it_was() {
    let (router, _engine) = emulator_with_engine();

    let collection = active_collection(&router, "tag-refusal-collection").await;
    let id = text(&collection, "id");
    let arn = text(&collection, "arn");

    /// The `lastModifiedDate` `BatchGetCollection` reports for `id`.
    async fn stamp(router: &MultiServiceRouter, id: &str, case: &str) -> i64 {
        let (status, code, answer) =
            call(router, "BatchGetCollection", json!({ "ids": [id] })).await;
        assert_eq!(
            status, 200,
            "case BatchGetCollection({case}): expected 200, got {status} {code} with {answer}"
        );
        at(&answer, "collectionDetails")
            .as_array()
            .and_then(|details| details.first())
            .and_then(|detail| at(detail, "lastModifiedDate").as_i64())
            .unwrap_or_else(|| panic!("case BatchGetCollection({case}): the read reports a lastModifiedDate"))
    }

    // Fill the resource to the tag cap the model declares, so the LAST refusal below is one
    // the handler reaches only after reading the stored record.
    for chunk in 0..(MAX_TAGS_PER_RESOURCE / 10) {
        let tags: Vec<Value> = (0..10)
            .map(|index| json!({ "key": format!("key-{chunk}-{index}"), "value": "v" }))
            .collect();
        let (status, code, answer) = call(
            &router,
            "TagResource",
            json!({ "resourceArn": arn, "tags": tags }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case TagResource(chunk {chunk} of the cap): expected 200, got {status} {code} with {answer}"
        );
    }

    let unchanged = stamp(&router, &id, "before the refusals").await;
    let tags_before = listed_tags(&router, &arn).await;

    // One row per way a tagging request is refused: a repeated key in either member, and a
    // write that would take the resource past the tag cap. None of them modifies the
    // resource, so none of them may move its `lastModifiedDate`.
    let refusals: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a repeated key in tags",
            "TagResource",
            json!({ "resourceArn": arn, "tags": [{ "key": "env", "value": "a" }, { "key": "env", "value": "b" }] }),
            "ValidationException",
        ),
        (
            "a repeated key in tagKeys",
            "UntagResource",
            json!({ "resourceArn": arn, "tagKeys": ["key-0-0", "key-0-0"] }),
            "ValidationException",
        ),
        (
            "one tag past the cap",
            "TagResource",
            json!({ "resourceArn": arn, "tags": [{ "key": "one-tag-too-many", "value": "v" }] }),
            "ServiceQuotaExceededException",
        ),
    ];
    for (case, operation, request, expected_code) in refusals {
        tokio::time::sleep(CLOCK_STEP).await;
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, expected_code),
            "case {operation}({case}): expected {CLIENT_ERROR} {expected_code}, got {status} {code} with {answer}"
        );
        let got = stamp(&router, &id, case).await;
        assert_eq!(
            got, unchanged,
            "case {operation}({case}): a refused write leaves lastModifiedDate at {unchanged}, got {got}"
        );
        let tags_now = listed_tags(&router, &arn).await;
        assert_eq!(
            tags_now, tags_before,
            "case {operation}({case}): a refused write leaves the stored tags unchanged, got {tags_now:?}"
        );
    }
}
