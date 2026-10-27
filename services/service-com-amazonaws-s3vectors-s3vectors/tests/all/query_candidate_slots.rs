// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a delete costs a nearest-neighbour query, served over the
//! real `restJson1` wire through the router production uses.
//!
//! The rule every row asserts: `QueryVectors$topK` bounds the CANDIDATES the index's
//! approximate-nearest-neighbour graph is asked for, not the vectors that come back. A
//! vector `DeleteVectors` removed keeps the candidate slot it occupies until that slot is
//! given up, so it is still ranked against the query and then not reported: a query
//! reports the LIVE vectors among the `topK` nearest candidates, which makes it short by
//! however many deleted vectors outrank them, and what it does report is always an
//! ordered prefix of the surviving ranking. A query with slots to spare — `topK` past the
//! whole candidate pool — reports the complete surviving ranking.
//!
//! The rows differ along what the rule generalises over: WHERE in the ranking the deleted
//! vector sat (nearest, in the middle, farthest), how many vectors were deleted, whether
//! the deleted key was ever stored, whether it was written again afterwards, and `topK`
//! itself — swept from 1 past the size of the candidate pool.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{call, ok, router};

const BUCKET: &str = "candidate-slot-bucket";
const INDEX: &str = "candidate-slot-index";

/// The five vectors every row starts from, in a one-dimensional `euclidean` index at
/// increasing distance from the query vector `[0.0]`: `v1` is nearest and `v5` farthest,
/// so the surviving ranking of any subset is these keys in order.
const STORED: [(&str, f32); 5] = [("v1", 1.0), ("v2", 2.0), ("v3", 3.0), ("v4", 4.0), ("v5", 5.0)];

/// A fresh router holding the index with [`STORED`] written into it.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": BUCKET }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "dataType": "float32",
            "dimension": 1,
            "distanceMetric": "euclidean",
        }),
    )
    .await;
    put(&router, &STORED.map(|(key, value)| (key, value))).await;
    router
}

/// Write `vectors` into the index.
async fn put(router: &MultiServiceRouter, vectors: &[(&str, f32)]) {
    let payload: Vec<serde_json::Value> = vectors
        .iter()
        .map(|(key, value)| json!({ "key": key, "data": { "float32": [value] } }))
        .collect();
    ok(
        router,
        "PutVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "vectors": payload,
        }),
    )
    .await;
}

/// Delete `keys` from the index.
async fn delete(router: &MultiServiceRouter, keys: &[&str]) {
    ok(
        router,
        "DeleteVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "keys": keys,
        }),
    )
    .await;
}

/// The keys `QueryVectors` reports for `top_k`, in the order it reports them.
async fn query(router: &MultiServiceRouter, case: &str, top_k: u32) -> Vec<String> {
    let (status, body) = call(
        router,
        "QueryVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "topK": top_k,
            "queryVector": { "float32": [0.0] },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: QueryVectors(topK={top_k}) must succeed, got {status} {body}"
    );
    body["vectors"]
        .as_array()
        .map(|vectors| {
            vectors
                .iter()
                .map(|vector| vector["key"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One row: what was deleted, what was written again afterwards, and what each `topK`
/// from 1 upwards must then report.
struct SlotCase {
    case: &'static str,
    deleted: &'static [&'static str],
    rewritten: &'static [&'static str],
    /// The keys expected for `topK` = 1, 2, 3, … in order.
    expected: &'static [&'static [&'static str]],
}

/// The rows.
fn slot_cases() -> Vec<SlotCase> {
    vec![
        SlotCase {
            case: "nothing deleted: every topK reports its own prefix",
            deleted: &[],
            rewritten: &[],
            expected: &[
                &["v1"],
                &["v1", "v2"],
                &["v1", "v2", "v3"],
                &["v1", "v2", "v3", "v4"],
                &["v1", "v2", "v3", "v4", "v5"],
                &["v1", "v2", "v3", "v4", "v5"],
            ],
        },
        SlotCase {
            case: "the NEAREST vector deleted: every topK inside the population is short \
                   by one",
            deleted: &["v1"],
            rewritten: &[],
            expected: &[
                &[],
                &["v2"],
                &["v2", "v3"],
                &["v2", "v3", "v4"],
                &["v2", "v3", "v4", "v5"],
                &["v2", "v3", "v4", "v5"],
            ],
        },
        SlotCase {
            case: "a MIDDLE vector deleted: the slot costs only the queries that reach it",
            deleted: &["v3"],
            rewritten: &[],
            expected: &[
                &["v1"],
                &["v1", "v2"],
                &["v1", "v2"],
                &["v1", "v2", "v4"],
                &["v1", "v2", "v4", "v5"],
                &["v1", "v2", "v4", "v5"],
            ],
        },
        SlotCase {
            case: "the FARTHEST vector deleted: no query that stops before it is short",
            deleted: &["v5"],
            rewritten: &[],
            expected: &[
                &["v1"],
                &["v1", "v2"],
                &["v1", "v2", "v3"],
                &["v1", "v2", "v3", "v4"],
                &["v1", "v2", "v3", "v4"],
                &["v1", "v2", "v3", "v4"],
            ],
        },
        SlotCase {
            case: "the two nearest vectors deleted: short by two",
            deleted: &["v1", "v2"],
            rewritten: &[],
            expected: &[
                &[],
                &[],
                &["v3"],
                &["v3", "v4"],
                &["v3", "v4", "v5"],
                &["v3", "v4", "v5"],
            ],
        },
        SlotCase {
            case: "the nearest and a middle vector deleted, in two calls",
            deleted: &["v1", "v4"],
            rewritten: &[],
            expected: &[
                &[],
                &["v2"],
                &["v2", "v3"],
                &["v2", "v3"],
                &["v2", "v3", "v5"],
                &["v2", "v3", "v5"],
            ],
        },
        SlotCase {
            case: "a key the index never held: a delete that removed nothing costs nothing",
            deleted: &["absent"],
            rewritten: &[],
            expected: &[
                &["v1"],
                &["v1", "v2"],
                &["v1", "v2", "v3"],
                &["v1", "v2", "v3", "v4"],
                &["v1", "v2", "v3", "v4", "v5"],
                &["v1", "v2", "v3", "v4", "v5"],
            ],
        },
        SlotCase {
            case: "the deleted key written again: it is a live candidate, not two",
            deleted: &["v1"],
            rewritten: &["v1"],
            expected: &[
                &["v1"],
                &["v1", "v2"],
                &["v1", "v2", "v3"],
                &["v1", "v2", "v3", "v4"],
                &["v1", "v2", "v3", "v4", "v5"],
                &["v1", "v2", "v3", "v4", "v5"],
            ],
        },
        SlotCase {
            case: "one of two deleted keys written again",
            deleted: &["v1", "v2"],
            rewritten: &["v2"],
            expected: &[
                &[],
                &["v2"],
                &["v2", "v3"],
                &["v2", "v3", "v4"],
                &["v2", "v3", "v4", "v5"],
                &["v2", "v3", "v4", "v5"],
            ],
        },
    ]
}

/// A query reports the live vectors among the `topK` nearest candidates, so each row's
/// deletes cost exactly the queries whose candidates reach them — and every answer is an
/// ordered prefix of the surviving ranking.
#[tokio::test]
async fn a_deleted_vector_keeps_the_candidate_slot_a_query_asked_for() {
    for row in slot_cases() {
        let SlotCase {
            case,
            deleted,
            rewritten,
            expected,
        } = row;
        let router = fixture().await;
        // Deleted one key per call, so a row's deletes are the calls a client makes.
        for key in deleted {
            delete(&router, &[key]).await;
        }
        let rewrite: Vec<(&str, f32)> = STORED
            .iter()
            .filter(|(key, _)| rewritten.contains(key))
            .copied()
            .collect();
        if !rewrite.is_empty() {
            put(&router, &rewrite).await;
        }

        // The surviving ranking, which every answer must be a prefix of.
        let surviving: Vec<String> = STORED
            .iter()
            .filter(|(key, _)| !deleted.contains(key) || rewritten.contains(key))
            .map(|(key, _)| (*key).to_string())
            .collect();

        for (position, expected) in expected.iter().enumerate() {
            let top_k = u32::try_from(position + 1).expect("the sweep fits a u32");
            let reported = query(&router, case, top_k).await;
            let expected: Vec<String> = expected.iter().map(|key| (*key).to_string()).collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: QueryVectors(topK={top_k}) must report {expected:?}, \
                 got {reported:?}"
            );
            assert_eq!(
                reported,
                surviving[..reported.len()].to_vec(),
                "case {case:?}: topK={top_k} must report a prefix of the surviving \
                 ranking {surviving:?}, got {reported:?}"
            );
        }
    }
}

/// The slot a deleted vector keeps is a QUERY's accounting and nothing else: the vector
/// is gone from every read of the index, and its key is free to be written again.
#[tokio::test]
async fn a_kept_candidate_slot_is_invisible_to_every_other_read() {
    let router = fixture().await;
    delete(&router, &["v1"]).await;

    let listed = ok(
        &router,
        "ListVectors",
        &json!({ "vectorBucketName": BUCKET, "indexName": INDEX }),
    )
    .await;
    let listed_keys: Vec<String> = listed["vectors"]
        .as_array()
        .map(|vectors| {
            vectors
                .iter()
                .map(|vector| vector["key"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed_keys,
        vec!["v2", "v3", "v4", "v5"],
        "a deleted vector is in no listing, got {listed}"
    );

    let got = ok(
        &router,
        "GetVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "keys": ["v1", "v2"],
        }),
    )
    .await;
    assert_eq!(
        got["vectors"].as_array().map(Vec::len),
        Some(1),
        "a deleted vector is in no read, got {got}"
    );
    assert_eq!(
        got["vectors"][0]["key"].as_str(),
        Some("v2"),
        "the surviving vector is the one reported, got {got}"
    );
}
