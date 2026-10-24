// SPDX-License-Identifier: CC0-1.0

//! The OpenSearch Serverless **collection group**: `CreateCollectionGroup` and the
//! `BatchGetCollectionGroup` read that proves what it stored, end to end over the wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body), through the
//! harness this crate's `collection_lifecycle` suite already owns, so the boundary
//! validator, the protocol codec and the handler all take part exactly as they do for an
//! SDK client.
//!
//! The rule under test: a create computes every field of its answer from the record it
//! stored — the server-minted id, the ARN built from that id and the request's Region, the
//! members the request named and the documented default for each it omitted — so a later
//! read of the same group reports the same values, and a value the group does not own
//! (how many collections belong to it) is counted from the collections themselves. The
//! tables below span what the rule generalises over: both `StandbyReplicas` members, every
//! `ServerlessGeneration` member plus the omitted case, capacity limits whole / partial /
//! absent, four Regions, tags including non-ASCII values, and id- as well as
//! name-addressed reads.
//!
//! The effective `capacityLimits` and the `storageConfiguration` every detail reports have
//! their own suite beside this one (`collection_group_capacity`), whose
//! [`expected_capacity_limits`] this suite reuses rather than restating the defaults.

use serde_json::{json, Value};

use crate::collection_group_capacity::expected_capacity_limits;
use crate::collection_lifecycle::{
    at, call, call_in, emulator, is_collection_id, text, ACCOUNT, CLIENT_ERROR,
};

/// Every `ServerlessGeneration` member, from the model's own value list
/// (`smithy_query(query="shape", shape="ServerlessGeneration")` -> `values`).
const SERVERLESS_GENERATIONS: [&str; 2] = ["CLASSIC", "NEXTGEN"];

/// Every `StandbyReplicas` member, from the model's own value list
/// (`smithy_query(query="shape", shape="StandbyReplicas")` -> `values`).
const STANDBY_REPLICAS: [&str; 2] = ["DISABLED", "ENABLED"];

/// The `errorCode` / `errorMessage` a `CollectionGroupErrorDetail` carries for a key that
/// names no group, from `CollectionGroupErrorDetail$errorCode` and `$errorMessage`'s own
/// documentation ("For example, `NOT_FOUND`" / "`The specified Collection Group is not
/// found.`").
pub(crate) const NOT_FOUND_CODE: &str = "NOT_FOUND";
pub(crate) const NOT_FOUND_MESSAGE: &str = "The specified Collection Group is not found.";

/// Create the collection group `body` describes in `region`, asserting it was accepted,
/// and answer its `createCollectionGroupDetail`.
pub(crate) async fn create_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    body: Value,
) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call_in(router, region, "CreateCollectionGroup", body).await;
    assert_eq!(
        status, 200,
        "case CreateCollectionGroup({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "createCollectionGroupDetail").clone()
}

/// Read the groups `body` selects, asserting the read was accepted, and answer the whole
/// `BatchGetCollectionGroup` response.
pub(crate) async fn batch_get(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    body: Value,
) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call_in(router, region, "BatchGetCollectionGroup", body).await;
    assert_eq!(
        status, 200,
        "case BatchGetCollectionGroup({case}): expected 200, got {status} {code} with {answer}"
    );
    answer
}

/// The `collectionGroupDetails` entry at `index`, or `Value::Null`.
pub(crate) fn detail_at(answer: &Value, index: usize) -> Value {
    at(answer, "collectionGroupDetails")
        .as_array()
        .and_then(|details| details.get(index))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `collectionGroupErrorDetails` entry at `index`, or `Value::Null`.
pub(crate) fn error_at(answer: &Value, index: usize) -> Value {
    at(answer, "collectionGroupErrorDetails")
        .as_array()
        .and_then(|errors| errors.get(index))
        .cloned()
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// CreateCollectionGroup: the server-derived fields and the stored members, over a table
// spanning both `StandbyReplicas` members, every `ServerlessGeneration` member and the
// omitted case, three shapes of `capacityLimits`, and four Regions — then the same group
// read back by id and by name.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_collection_group_round_trips_through_batch_get() {
    let router = emulator();

    // (region, request, expected generation, expected standbyReplicas)
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "us-east-1",
            json!({
                "name": "alpha-group",
                "standbyReplicas": STANDBY_REPLICAS[1],
                "generation": SERVERLESS_GENERATIONS[0],
                "description": "the alpha group",
                "capacityLimits": {
                    "minIndexingCapacityInOCU": 2.0,
                    "maxIndexingCapacityInOCU": 10.0,
                    "minSearchCapacityInOCU": 2.0,
                    "maxSearchCapacityInOCU": 20.0,
                },
                "tags": [{ "key": "team", "value": "search" }],
            }),
            SERVERLESS_GENERATIONS[0],
            STANDBY_REPLICAS[1],
        ),
        (
            "us-west-2",
            json!({
                "name": "beta-group",
                "standbyReplicas": STANDBY_REPLICAS[0],
                "generation": SERVERLESS_GENERATIONS[1],
                // Only the ceilings: a partial `capacityLimits` is stored as given.
                "capacityLimits": {
                    "maxIndexingCapacityInOCU": 4.0,
                    "maxSearchCapacityInOCU": 4.0,
                },
                // A non-ASCII tag value must survive the round trip unchanged.
                "tags": [
                    { "key": "owner", "value": "数据团队" },
                    { "key": "note", "value": "naïve-café" },
                ],
            }),
            SERVERLESS_GENERATIONS[1],
            STANDBY_REPLICAS[0],
        ),
        (
            "eu-west-1",
            // Every optional member omitted, so this row pins the default `generation`
            // and the `capacityLimits` the service defaults in for a group that named
            // none.
            json!({ "name": "gamma-group", "standbyReplicas": STANDBY_REPLICAS[1] }),
            "CLASSIC",
            STANDBY_REPLICAS[1],
        ),
        (
            "ap-southeast-2",
            json!({
                "name": "delta-group",
                "standbyReplicas": STANDBY_REPLICAS[0],
                // The floor equal to the ceiling is inside the relation the handler
                // checks, so it is accepted.
                "capacityLimits": {
                    "minIndexingCapacityInOCU": 3.0,
                    "maxIndexingCapacityInOCU": 3.0,
                    "minSearchCapacityInOCU": 1.0,
                    "maxSearchCapacityInOCU": 1.0,
                },
                "tags": [],
            }),
            "CLASSIC",
            STANDBY_REPLICAS[0],
        ),
    ];

    for (region, request, expected_generation, expected_standby) in cases {
        let name = text(&request, "name");
        let case = format!("CreateCollectionGroup({name} in {region})");
        let requested = request.clone();
        let detail = create_group(&router, region, request).await;

        let id = text(&detail, "id");
        assert!(
            is_collection_id(&id),
            "case {case}: id must match ^[a-z0-9]{{3,40}}$, got {id:?}"
        );
        assert_eq!(
            text(&detail, "name"),
            name,
            "case {case}: name must be the requested one, got {detail}"
        );
        assert_eq!(
            text(&detail, "arn"),
            format!("arn:aws:aoss:{region}:{ACCOUNT}:collection-group/{id}"),
            "case {case}: the ARN is built from this group's own id and Region, got {detail}"
        );
        assert_eq!(
            text(&detail, "standbyReplicas"),
            expected_standby,
            "case {case}: standbyReplicas must be {expected_standby}, got {detail}"
        );
        assert_eq!(
            text(&detail, "generation"),
            expected_generation,
            "case {case}: generation must be {expected_generation}, got {detail}"
        );
        assert_eq!(
            at(&detail, "description"),
            at(&requested, "description"),
            "case {case}: description must be the requested one, got {detail}"
        );
        assert_eq!(
            at(&detail, "capacityLimits"),
            &expected_capacity_limits(at(&requested, "capacityLimits")),
            "case {case}: capacityLimits must be the requested ones, with the service \
             default for each ceiling the request omitted, got {detail}"
        );
        let expected_tags = match at(&requested, "tags") {
            Value::Null => json!([]),
            tags => tags.clone(),
        };
        assert_eq!(
            at(&detail, "tags"),
            &expected_tags,
            "case {case}: tags must round-trip unchanged, got {detail}"
        );
        let created = at(&detail, "createdDate").as_i64().unwrap_or_default();
        assert!(
            created > 1_600_000_000_000,
            "case {case}: createdDate must be epoch milliseconds, got {created}"
        );

        // The round trip: the same group, addressed by id and by name, reports exactly
        // what the create stored.
        for selector in [json!({ "ids": [&id] }), json!({ "names": [&name] })] {
            let case = format!("{case} read back with {selector}");
            let answer = batch_get(&router, region, selector).await;
            let read = detail_at(&answer, 0);
            for member in [
                "id",
                "arn",
                "name",
                "standbyReplicas",
                "generation",
                "description",
                "capacityLimits",
                "storageConfiguration",
                "tags",
                "createdDate",
            ] {
                assert_eq!(
                    at(&read, member),
                    at(&detail, member),
                    "case {case}: {member} must be what the create reported, got {read}"
                );
            }
            assert_eq!(
                at(&read, "numberOfCollections").as_i64(),
                Some(0),
                "case {case}: a group with no collections reports 0, got {read}"
            );
            // The current capacity is the floor the group's own limits name, while no
            // autoscaling action is in progress; a group whose limits name no floor
            // reports none. `CapacityDetails` models the member as `capacityInOcu`,
            // unlike `CollectionGroupCapacityLimits`' `...CapacityInOCU`
            // (`smithy_query(query="shape", shape="CapacityDetails")`).
            let expected_capacity = |half: &str| {
                at(&requested, &format!("capacityLimits.min{half}CapacityInOCU"))
                    .as_f64()
                    .map(|floor| json!({ "capacityInOcu": floor, "autoscalingStatus": "NO_ACTION" }))
                    .unwrap_or(Value::Null)
            };
            for (half, member) in [("Search", "search"), ("Indexing", "indexing")] {
                assert_eq!(
                    at(&read, &format!("currentCapacity.{member}")),
                    &expected_capacity(half),
                    "case {case}: currentCapacity.{member} must be the stored floor, got {read}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BatchGetCollectionGroup: the store answers each key, `numberOfCollections` is counted
// over the collections that name the group, and an unknown key is a NOT_FOUND detail
// rather than a failed request.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_get_collection_group_counts_collections_and_reports_missing_keys() {
    let router = emulator();

    // Groups differing in how many collections name them: 0, 1 and 2.
    let groups = ["empty-group", "single-group", "double-group"];
    for name in groups {
        create_group(
            &router,
            "us-east-1",
            json!({ "name": name, "standbyReplicas": "ENABLED" }),
        )
        .await;
    }
    for (collection, group) in [
        ("one-collection", "single-group"),
        ("two-collection", "double-group"),
        ("three-collection", "double-group"),
    ] {
        let (status, code, answer) = call(
            &router,
            "CreateCollection",
            json!({ "name": collection, "collectionGroupName": group }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateCollection({collection}): expected 200, got {status} {code} with {answer}"
        );
    }

    for (name, expected) in [("empty-group", 0), ("single-group", 1), ("double-group", 2)] {
        let answer = batch_get(&router, "us-east-1", json!({ "names": [name] })).await;
        let read = detail_at(&answer, 0);
        assert_eq!(
            at(&read, "numberOfCollections").as_i64(),
            Some(expected),
            "case numberOfCollections({name}): expected {expected}, got {read}"
        );
    }

    // A mixed read: every key is answered in request order, the known ones as details and
    // the unknown ones as the documented NOT_FOUND error detail.
    let answer = batch_get(
        &router,
        "us-east-1",
        json!({ "names": ["single-group", "no-such-group", "empty-group"] }),
    )
    .await;
    assert_eq!(
        text(&detail_at(&answer, 0), "name"),
        "single-group",
        "case mixed read: the first known key is the first detail, got {answer}"
    );
    assert_eq!(
        text(&detail_at(&answer, 1), "name"),
        "empty-group",
        "case mixed read: the second known key is the second detail, got {answer}"
    );
    let error = error_at(&answer, 0);
    assert_eq!(
        text(&error, "name"),
        "no-such-group",
        "case mixed read: the error names the key that was asked for, got {answer}"
    );
    assert_eq!(
        text(&error, "errorCode"),
        NOT_FOUND_CODE,
        "case mixed read: errorCode must be {NOT_FOUND_CODE}, got {answer}"
    );
    assert_eq!(
        text(&error, "errorMessage"),
        NOT_FOUND_MESSAGE,
        "case mixed read: errorMessage must be the documented one, got {answer}"
    );

    // An unknown id is reported under `id`, not under `name`.
    let answer = batch_get(&router, "us-east-1", json!({ "ids": ["abc123"] })).await;
    let error = error_at(&answer, 0);
    assert_eq!(
        text(&error, "id"),
        "abc123",
        "case unknown id: the error names the id that was asked for, got {answer}"
    );
    assert_eq!(
        at(&error, "name"),
        &Value::Null,
        "case unknown id: an id-addressed error carries no name, got {answer}"
    );

    // Neither selector, and both selectors, are refused: "You can't provide names and IDs
    // in the same request", and a request that names neither selects nothing.
    for selector in [
        json!({}),
        json!({ "ids": ["abc123"], "names": ["empty-group"] }),
    ] {
        let case = selector.to_string();
        let (status, code, answer) =
            call(&router, "BatchGetCollectionGroup", selector.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case BatchGetCollectionGroup({case}): expected 400 ValidationException, got {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateCollectionGroup: what it refuses, and with which modelled error.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_collection_group_refuses_a_duplicate_name_and_invalid_input() {
    let router = emulator();

    create_group(
        &router,
        "us-east-1",
        json!({ "name": "taken-group", "standbyReplicas": "ENABLED" }),
    )
    .await;

    // (case, request, expected status, expected error shape)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a name already taken",
            json!({ "name": "taken-group", "standbyReplicas": "DISABLED" }),
            CLIENT_ERROR,
            "ConflictException",
        ),
        (
            "a generation outside the model's value list",
            json!({ "name": "other-group", "standbyReplicas": "ENABLED", "generation": "LEGACY" }),
            400,
            "ValidationException",
        ),
        (
            "a generation in the wrong case",
            json!({ "name": "other-group", "standbyReplicas": "ENABLED", "generation": "classic" }),
            400,
            "ValidationException",
        ),
        (
            "a standbyReplicas outside the model's value list",
            json!({ "name": "other-group", "standbyReplicas": "MAYBE" }),
            400,
            "ValidationException",
        ),
        (
            "an indexing floor above its own ceiling",
            json!({
                "name": "other-group",
                "standbyReplicas": "ENABLED",
                "capacityLimits": {
                    "minIndexingCapacityInOCU": 8.0,
                    "maxIndexingCapacityInOCU": 4.0,
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "a search floor above its own ceiling",
            json!({
                "name": "other-group",
                "standbyReplicas": "ENABLED",
                "capacityLimits": {
                    "minSearchCapacityInOCU": 8.0,
                    "maxSearchCapacityInOCU": 4.0,
                },
            }),
            400,
            "ValidationException",
        ),
        (
            // `CollectionGroupMaxIndexingCapacityValue` is `@range(min: 1)`: one past the
            // published lower bound.
            "a ceiling below the modelled minimum",
            json!({
                "name": "other-group",
                "standbyReplicas": "ENABLED",
                "capacityLimits": { "maxIndexingCapacityInOCU": 0.5 },
            }),
            400,
            "ValidationException",
        ),
        (
            // `CollectionGroupMinSearchCapacityValue` is `@range(min: 0)`.
            "a floor below the modelled minimum",
            json!({
                "name": "other-group",
                "standbyReplicas": "ENABLED",
                "capacityLimits": { "minSearchCapacityInOCU": -1.0 },
            }),
            400,
            "ValidationException",
        ),
        (
            // `CollectionGroupName` is `@length(min: 3, max: 32)` with
            // `^[a-z][a-z0-9-]+$`: one past the lower bound.
            "a name shorter than the modelled minimum",
            json!({ "name": "ab", "standbyReplicas": "ENABLED" }),
            400,
            "ValidationException",
        ),
        (
            "a name outside the modelled pattern",
            json!({ "name": "Upper-Group", "standbyReplicas": "ENABLED" }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in cases {
        let (status, code, answer) = call(&router, "CreateCollectionGroup", request).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case}: expected {expected_status} {expected_code}, got {status} {code} with {answer}"
        );
    }

    // A refusal stores nothing: only the one group that was accepted is readable.
    let answer = batch_get(&router, "us-east-1", json!({ "names": ["other-group"] })).await;
    assert_eq!(
        at(&answer, "collectionGroupDetails"),
        &json!([]),
        "case refusals persist nothing: expected no group named other-group, got {answer}"
    );

    // The names at the published bounds of `CollectionGroupName` are accepted, and so are
    // the capacity values at the published bounds of their own ranges (`max*` min 1,
    // `min*` min 0).
    for name in ["abc", &"g".repeat(32)] {
        let detail = create_group(
            &router,
            "us-east-1",
            json!({ "name": name, "standbyReplicas": "ENABLED" }),
        )
        .await;
        assert_eq!(
            text(&detail, "name"),
            name,
            "case a name at a published bound ({name}): expected it to be stored, got {detail}"
        );
    }
    let detail = create_group(
        &router,
        "us-east-1",
        json!({
            "name": "bounds-group",
            "standbyReplicas": "ENABLED",
            "capacityLimits": {
                "minIndexingCapacityInOCU": 0.0,
                "maxIndexingCapacityInOCU": 1.0,
                "minSearchCapacityInOCU": 0.0,
                "maxSearchCapacityInOCU": 1.0,
            },
        }),
    )
    .await;
    assert_eq!(
        at(&detail, "capacityLimits"),
        &json!({
            "minIndexingCapacityInOCU": 0.0,
            "maxIndexingCapacityInOCU": 1.0,
            "minSearchCapacityInOCU": 0.0,
            "maxSearchCapacityInOCU": 1.0,
        }),
        "case capacity values at their published bounds: expected them to be stored, got {detail}"
    );
}

// ---------------------------------------------------------------------------
// The `@idempotencyToken` on `clientToken`: a retry answers the group the first call
// created rather than creating or conflicting with a second one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_answers_the_group_it_first_created() {
    let router = emulator();

    for token in ["retry-token-1", "a-much-longer-retry-token-0123456789"] {
        let request = json!({
            "name": format!("group-{}", token.len()),
            "standbyReplicas": "ENABLED",
            "clientToken": token,
        });
        let first = create_group(&router, "us-east-1", request.clone()).await;
        let replay = create_group(&router, "us-east-1", request).await;
        assert_eq!(
            first, replay,
            "case replay({token}): a retry must answer the first call's group, got {replay}"
        );

        // And the same name under a DIFFERENT token is the name conflict, not a replay.
        let (status, code, answer) = call(
            &router,
            "CreateCollectionGroup",
            json!({
                "name": format!("group-{}", token.len()),
                "standbyReplicas": "ENABLED",
                "clientToken": format!("{token}-other"),
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case fresh token on a taken name ({token}): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
        );
    }
}
