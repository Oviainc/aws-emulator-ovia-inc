// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SENTENCE three more refusal classes of this service carry
//! in the `message` member a client reads: the degenerate-vector refusal of `PutVectors`
//! and `QueryVectors`, the segment-pairing refusal of `ListVectors`, and the tag
//! read-modify-write conflict of `TagResource` / `UntagResource`.
//! `smithy_query(query="operation",operation="PutVectors,QueryVectors,ListVectors,TagResource,UntagResource")`
//! shows `ValidationException` (400) and `ConflictException` (409) on them, each with a
//! required `message`.
//!
//! RULE: a refusal's `message` is the service's own sentence for that error class — it
//! names the RULE or the state that was broken, interpolating only the variable parts
//! the service's own template has (for one record of a batch, that record's `key`), and
//! never a wire member name, a value echoed back from the request, or a count read out
//! of stored state. So two refusals that share an error code on one operation are told
//! apart by their text alone, and the text a client matches is one constant string.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over:
//!
//! * the degenerate-vector rows differ in the index's `dimension` (both ends of the
//!   values this suite writes), in the record's position in the batch, in the form of
//!   its `key`, in which member the vector travelled in (`vectors[n].data`, `queryVector`)
//!   and in the index's `distanceMetric` — `cosine` refuses the zero-norm vector and
//!   `euclidean` stores it, which is the positive control;
//! * the segment rows differ in the submitted `segmentCount` and `segmentIndex`,
//!   spanning each published bound of the two members' modelled ranges and one pairing
//!   past the rule, with the accepted pairings as the positive control;
//! * the tag-conflict rows differ in which pair of mutations overlaps, which kind of
//!   resource they address and how many callers overlap at once.
//!
//! Every case drives real HTTP through the production front door, so what is asserted is
//! what an SDK client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{
    call, error_code, expected_bucket_arn, expected_index_arn, keys, ok, router, tag, untag,
};

/// The wire error code and status of an input refusal on these operations.
const VALIDATION_EXCEPTION: &str = "ValidationException";
/// See [`VALIDATION_EXCEPTION`].
const BAD_REQUEST: u16 = 400;
/// The wire error code and status of a tag write that lost its optimistic lock.
const CONFLICT_EXCEPTION: &str = "ConflictException";
/// See [`CONFLICT_EXCEPTION`].
const CONFLICT: u16 = 409;

/// The sentence the service answers for a vector whose norm is zero written to (or
/// queried against) an index whose `distanceMetric` is `cosine`. A LITERAL, not the
/// handler's own constant: a suite asserting the constant would agree with any wording
/// the handler chose.
const ZERO_NORM_SENTENCE: &str = "cosine distance does not support vectors with zero norm";

/// The sentence the service answers for a parallel `ListVectors` whose `segmentIndex` is
/// not inside the partition its `segmentCount` describes. All fixed text restating the
/// rule `ListVectorsInput$segmentIndex` documents.
const SEGMENT_SENTENCE: &str = "Segment index is zero-indexed and must be smaller than segment count";

/// The sentence the service answers a tag mutation whose read-modify-write was
/// invalidated by an overlapping mutation of the same resource.
const TAG_CONFLICT_SENTENCE: &str =
    "The resource tags were modified by another request. Please retry the operation.";

/// The `message` member of a refusal body, whichever casing the protocol rendered.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `fieldList` of a refusal body as (path, message) pairs.
fn field_list(body: &Value) -> Vec<(String, String)> {
    body.get("fieldList")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|field| {
            (
                field["path"].as_str().unwrap_or_default().to_string(),
                field["message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The sentence a refusal about ONE record of a batch carries: the service names the
/// record by the `key` the caller submitted it under, then states the defect.
fn record_sentence(key: &str, defect: &str) -> String {
    format!("Invalid record for key '{key}': {defect}")
}

/// A router holding `bucket` with a `dimension`-wide `metric` index called `index`.
async fn with_index(
    bucket: &str,
    index: &str,
    dimension: usize,
    metric: &str,
) -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": bucket,
            "indexName": index,
            "dataType": "float32",
            "dimension": dimension,
            "distanceMetric": metric,
        }),
    )
    .await;
    router
}

/// The bucket and index every vector suite below creates.
const VECTOR_BUCKET: &str = "zero-norm-bucket";
/// See [`VECTOR_BUCKET`].
const VECTOR_INDEX: &str = "zero-norm-index";

/// A zero-norm vector written to a `cosine` index is refused as the RECORD it is: the
/// top-level message names the record by its own `key` and then states the zero-norm
/// rule, `fieldList` names the element of `vectors` it was submitted in and carries the
/// rule alone, and nothing in the batch is written. The SAME payload into a `euclidean`
/// index is stored, so the sentence is about the metric's rule rather than about the
/// vector's shape.
#[tokio::test]
async fn a_zero_norm_vector_reports_the_metrics_own_rule_named_by_the_records_key() {
    // (case, the index dimension, the offending record's position in the batch, its key)
    let rows: [(&str, usize, usize, &str); 5] = [
        ("a one-dimension index", 1, 0, "zero"),
        ("the only record of a batch", 3, 0, "vec-all-zeros"),
        ("the second record of a batch", 3, 1, "second"),
        ("the fourth record of a batch", 8, 3, "fourth-record"),
        (
            "a key of the opaque generated form",
            4,
            1,
            "9f41c2ab-7de0-4a11-8f3e-0b1d2c3e4f55",
        ),
    ];

    for (case, dimension, position, key) in rows {
        let zeros: Vec<f32> = vec![0.0; dimension];
        // Well-formed records at every other position, so what is refused is the one
        // record and the atomicity of the batch is what is left to check.
        let good: Vec<f32> = (0..dimension).map(|nth| 0.5 + nth as f32).collect();
        let vectors: Vec<Value> = (0..=position)
            .map(|nth| {
                if nth == position {
                    json!({ "key": key, "data": { "float32": zeros } })
                } else {
                    json!({ "key": format!("good-{nth}"), "data": { "float32": good } })
                }
            })
            .collect();

        // The cosine index refuses it.
        let router = with_index(VECTOR_BUCKET, VECTOR_INDEX, dimension, "cosine").await;
        let (status, body) = call(
            &router,
            "PutVectors",
            &json!({
                "vectorBucketName": VECTOR_BUCKET,
                "indexName": VECTOR_INDEX,
                "vectors": vectors,
            }),
        )
        .await;
        let message = message_of(&body);
        assert_eq!(
            (status, error_code(&body).as_str(), message.as_str()),
            (
                BAD_REQUEST,
                VALIDATION_EXCEPTION,
                record_sentence(key, ZERO_NORM_SENTENCE).as_str()
            ),
            "case {case:?}: a zero-norm vector must be the modelled 400 naming the record \
             by its key and stating the metric's rule, got {status} {body}"
        );
        assert_eq!(
            field_list(&body),
            vec![(
                format!("vectors[{position}]"),
                ZERO_NORM_SENTENCE.to_string()
            )],
            "case {case:?}: fieldList must name the list ELEMENT and carry the rule alone, \
             got {body}"
        );
        assert!(
            !message.contains(VECTOR_INDEX)
                && !message.contains(VECTOR_BUCKET)
                && !message.contains('`'),
            "case {case:?}: the sentence must name neither the index, the bucket nor a wire \
             member, got {message:?}"
        );

        // A refused batch writes nothing, not even the records that were well formed.
        let listed = ok(
            &router,
            "ListVectors",
            &json!({ "vectorBucketName": VECTOR_BUCKET, "indexName": VECTOR_INDEX }),
        )
        .await;
        assert_eq!(
            listed["vectors"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused batch must write nothing, got {listed}"
        );

        // The positive control: the same payload into a euclidean index is stored, and
        // reads back through GetVectors as the vector that was sent.
        let euclidean = with_index(VECTOR_BUCKET, VECTOR_INDEX, dimension, "euclidean").await;
        ok(
            &euclidean,
            "PutVectors",
            &json!({
                "vectorBucketName": VECTOR_BUCKET,
                "indexName": VECTOR_INDEX,
                "vectors": vectors,
            }),
        )
        .await;
        let got = ok(
            &euclidean,
            "GetVectors",
            &json!({
                "vectorBucketName": VECTOR_BUCKET,
                "indexName": VECTOR_INDEX,
                "keys": [key],
                "returnData": true,
            }),
        )
        .await;
        assert_eq!(
            got["vectors"][0]["data"]["float32"],
            json!(vec![0.0_f32; dimension]),
            "case {case:?}: a euclidean index must store the zero-norm vector, got {got}"
        );
    }
}

/// The same rule holds of the vector a QUERY is run with, where the refusal names the
/// MEMBER the vector travelled in — there is no record to name — and the sentence it
/// carries is the same one.
#[tokio::test]
async fn a_zero_norm_query_vector_reports_the_same_rule_named_by_its_member() {
    for dimension in [1_usize, 3, 8] {
        let case = format!("a {dimension}-dimension cosine index");
        let router = with_index(VECTOR_BUCKET, VECTOR_INDEX, dimension, "cosine").await;
        let (status, body) = call(
            &router,
            "QueryVectors",
            &json!({
                "vectorBucketName": VECTOR_BUCKET,
                "indexName": VECTOR_INDEX,
                "topK": 1,
                "queryVector": { "float32": vec![0.0_f32; dimension] },
            }),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&body).as_str(),
                message_of(&body).as_str()
            ),
            (BAD_REQUEST, VALIDATION_EXCEPTION, ZERO_NORM_SENTENCE),
            "case {case:?}: a zero-norm query vector must carry the same rule, \
             got {status} {body}"
        );
        assert_eq!(
            field_list(&body),
            vec![("queryVector".to_string(), ZERO_NORM_SENTENCE.to_string())],
            "case {case:?}: fieldList must name the member it travelled in, got {body}"
        );
    }
}

/// The degenerate-vector refusal and the dimension refusal share `ValidationException`
/// on `PutVectors`, so their sentences must differ — that difference is the only thing
/// telling a client it submitted a degenerate vector rather than a mis-sized one.
#[tokio::test]
async fn the_degenerate_and_mis_sized_vector_refusals_are_different_sentences() {
    let dimension = 4_usize;
    let router = with_index(VECTOR_BUCKET, VECTOR_INDEX, dimension, "cosine").await;
    let key = "probe";
    // (case, the submitted data, the sentence the refusal must carry)
    let rows: Vec<(&str, Vec<f32>, String)> = vec![
        (
            "a zero-norm vector of the right length",
            vec![0.0; dimension],
            ZERO_NORM_SENTENCE.to_string(),
        ),
        (
            "a vector one value short",
            vec![0.5; dimension - 1],
            format!(
                "vector must have length {dimension}, but has length {}",
                dimension - 1
            ),
        ),
        (
            "a zero-norm vector of the wrong length",
            vec![0.0; dimension + 2],
            format!(
                "vector must have length {dimension}, but has length {}",
                dimension + 2
            ),
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, data, defect) in rows {
        let (status, body) = call(
            &router,
            "PutVectors",
            &json!({
                "vectorBucketName": VECTOR_BUCKET,
                "indexName": VECTOR_INDEX,
                "vectors": [{ "key": key, "data": { "float32": data } }],
            }),
        )
        .await;
        let message = message_of(&body);
        assert_eq!(
            (status, error_code(&body).as_str(), message.as_str()),
            (
                BAD_REQUEST,
                VALIDATION_EXCEPTION,
                record_sentence(key, &defect).as_str()
            ),
            "case {case:?}: must carry this class's own sentence, got {status} {body}"
        );
        seen.push(message);
    }
    assert_ne!(
        seen[0], seen[1],
        "the degenerate and the mis-sized refusal must be different sentences, \
         got {:?} twice",
        seen[0]
    );
    // A vector that is both degenerate and mis-sized is the LENGTH refusal: the length
    // is checked first, so one request yields exactly one of the two sentences.
    assert!(
        !seen[2].contains(ZERO_NORM_SENTENCE),
        "a vector that is both must report the length refusal, got {:?}",
        seen[2]
    );
}

/// The bucket and index the segment suite lists.
const SEGMENT_BUCKET: &str = "segment-bucket";
/// See [`SEGMENT_BUCKET`].
const SEGMENT_INDEX: &str = "segment-index";

/// A router holding a 4-dimension index with `written` vectors in it.
async fn with_vectors(written: usize) -> MultiServiceRouter {
    let router = with_index(SEGMENT_BUCKET, SEGMENT_INDEX, 4, "euclidean").await;
    let vectors: Vec<Value> = (0..written)
        .map(|nth| {
            json!({
                "key": format!("vec-{nth}"),
                "data": { "float32": [nth as f32, 0.5, 1.5, 2.5] },
            })
        })
        .collect();
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": SEGMENT_BUCKET,
            "indexName": SEGMENT_INDEX,
            "vectors": vectors,
        }),
    )
    .await;
    router
}

/// One `ListVectors` of the segment `index` of `count` segments.
async fn list_segment(router: &MultiServiceRouter, count: u32, index: u32) -> (u16, Value) {
    call(
        router,
        "ListVectors",
        &json!({
            "vectorBucketName": SEGMENT_BUCKET,
            "indexName": SEGMENT_INDEX,
            "segmentCount": count,
            "segmentIndex": index,
        }),
    )
    .await
}

/// A `segmentIndex` that is not below its `segmentCount` — both values inside the two
/// members' own modelled ranges, so the SERVICE is what refuses the pairing — is refused
/// with ONE fixed sentence restating the rule, which echoes neither submitted number;
/// while every pairing that satisfies the rule, including at each published bound, is
/// served.
#[tokio::test]
async fn a_segment_index_not_below_its_segment_count_reports_one_fixed_sentence() {
    // `ListVectorsInput$segmentCount` is `range 1..16` and `$segmentIndex` is
    // `range 0..15`, so each pairing below leaves the SDK: the rows walk the bottom of
    // both ranges, the equality case the rule forbids, an index well past its count, and
    // the top of the index range under a count that does not reach it.
    let refused: [(&str, u32, u32); 5] = [
        ("the smallest count, one segment past it", 1, 1),
        ("two segments, index equal to the count", 2, 2),
        ("four segments, index equal to the count", 4, 4),
        ("four segments, index well past the count", 4, 15),
        ("fifteen segments, the top index", 15, 15),
    ];
    // The published bounds that DO satisfy the rule: the sequential default, the top
    // index of a four-way split, and the top of both ranges together.
    let served: [(&str, u32, u32); 4] = [
        ("the sequential default", 1, 0),
        ("the first of four segments", 4, 0),
        ("the last of four segments", 4, 3),
        ("the last of the most segments", 16, 15),
    ];

    let router = with_vectors(12).await;
    for (case, count, index) in refused {
        let (status, body) = list_segment(&router, count, index).await;
        let message = message_of(&body);
        assert_eq!(
            (status, error_code(&body).as_str(), message.as_str()),
            (BAD_REQUEST, VALIDATION_EXCEPTION, SEGMENT_SENTENCE),
            "case {case:?}: segmentCount={count} segmentIndex={index} must be the modelled \
             400 carrying the service's fixed sentence, got {status} {body}"
        );
        assert!(
            !message.chars().any(|character| character.is_ascii_digit())
                && !message.contains('`'),
            "case {case:?}: the sentence must echo neither submitted number nor a wire \
             member name, got {message:?}"
        );
        assert_eq!(
            field_list(&body),
            vec![("segmentIndex".to_string(), SEGMENT_SENTENCE.to_string())],
            "case {case:?}: fieldList must name the member at fault, got {body}"
        );
    }

    for (case, count, index) in served {
        let (status, body) = list_segment(&router, count, index).await;
        assert_eq!(
            status, 200,
            "case {case:?}: segmentCount={count} segmentIndex={index} satisfies the rule and \
             must be served, got {status} {body}"
        );
    }
}

/// The bucket and index the tag-conflict suite mutates.
const TAG_BUCKET: &str = "sentence-tag-bucket";
/// See [`TAG_BUCKET`].
const TAG_INDEX: &str = "sentence-tag-index";

/// A router holding a vector bucket and a vector index, each carrying two tags a removal
/// row can remove.
async fn tag_fixture() -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": TAG_BUCKET, "tags": { "team": "web", "env": "test" } }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": TAG_BUCKET,
            "indexName": TAG_INDEX,
            "dataType": "float32",
            "dimension": 3,
            "distanceMetric": "cosine",
            "tags": { "team": "web", "env": "test" },
        }),
    )
    .await;
    router
}

/// Which mutation a row fires at the contended resource.
#[derive(Debug, Clone, Copy)]
enum Mutation {
    /// `TagResource` writing one key.
    Write(&'static str),
    /// `UntagResource` removing one key.
    Remove(&'static str),
}

/// Fire one mutation against `arn`.
async fn mutate(router: &MultiServiceRouter, arn: &str, mutation: Mutation) -> (u16, Value) {
    match mutation {
        Mutation::Write(key) => tag(router, arn, &json!({ key: "value" })).await,
        Mutation::Remove(key) => untag(router, arn, &keys(&[key])).await,
    }
}

/// Assert one refused racer carries the tag-conflict sentence and nothing of the request.
fn assert_tag_conflict(case: &str, status: u16, body: &Value, arn: &str) {
    let message = message_of(body);
    assert_eq!(
        (status, error_code(body).as_str(), message.as_str()),
        (CONFLICT, CONFLICT_EXCEPTION, TAG_CONFLICT_SENTENCE),
        "case {case:?}: a refused tag mutation must be the modelled 409 carrying the \
         service's fixed tag-conflict sentence, got {status} {body}"
    );
    assert!(
        !message.contains(arn) && !message.contains("team") && !message.contains("env"),
        "case {case:?}: the sentence must carry neither the resource ARN nor a tag key, \
         got {message:?}"
    );
}

/// Every caller whose tag read-modify-write was invalidated by an overlapping mutation of
/// the same resource is refused with ONE fixed sentence about the TAGS — whichever pair of
/// mutations overlapped and whichever kind of resource they addressed.
#[tokio::test]
async fn an_overlapping_tag_mutation_reports_one_fixed_tag_conflict_sentence() {
    for kind in ["vector bucket", "vector index"] {
        for (first, second) in [
            (Mutation::Write("owner"), Mutation::Write("owner")),
            (Mutation::Write("owner"), Mutation::Remove("team")),
            (Mutation::Remove("team"), Mutation::Write("owner")),
            (Mutation::Remove("team"), Mutation::Remove("env")),
        ] {
            let case = format!("{first:?} overlapping {second:?} on a {kind}");
            let router = tag_fixture().await;
            let arn = if kind == "vector bucket" {
                expected_bucket_arn(TAG_BUCKET)
            } else {
                expected_index_arn(TAG_BUCKET, TAG_INDEX)
            };

            let (left, right) = ::tokio::join!(
                mutate(&router, &arn, first),
                mutate(&router, &arn, second)
            );
            let refused: Vec<&(u16, Value)> = [&left, &right]
                .into_iter()
                .filter(|(status, _)| *status != 200)
                .collect();
            assert_eq!(
                refused.len(),
                1,
                "case {case:?}: exactly one of the two overlapping callers is refused, \
                 got {left:?} {right:?}"
            );
            assert_tag_conflict(&case, refused[0].0, &refused[0].1, &arn);
        }
    }
}

/// With a whole fan-out of overlapping callers, EVERY refused one carries the same
/// sentence — so a client retrying its read-modify-write matches one constant string
/// however many racers there were — and a mutation issued after the storm is served,
/// which proves the lock was released rather than the sentence latched.
#[tokio::test]
async fn every_refused_racer_of_a_fan_out_carries_the_same_sentence() {
    for racers in [3_usize, 5, 8] {
        let case = format!("{racers} overlapping callers");
        let router = tag_fixture().await;
        let arn = expected_bucket_arn(TAG_BUCKET);

        let write = |caller: usize| {
            let tags = json!({ "owner": format!("caller-{caller}") });
            let arn = arn.clone();
            let router = &router;
            async move { tag(router, &arn, &tags).await }
        };
        // Every racer is in flight at once: `join!` polls them all on this task, so each
        // reaches the store while the served one's write is still committing. The arity
        // is fixed per row because the futures are polled, not spawned.
        let answers: Vec<(u16, Value)> = match racers {
            3 => {
                let (a, b, c) = ::tokio::join!(write(0), write(1), write(2));
                vec![a, b, c]
            }
            5 => {
                let (a, b, c, d, e) =
                    ::tokio::join!(write(0), write(1), write(2), write(3), write(4));
                vec![a, b, c, d, e]
            }
            _ => {
                let (a, b, c, d, e, f, g, h) = ::tokio::join!(
                    write(0),
                    write(1),
                    write(2),
                    write(3),
                    write(4),
                    write(5),
                    write(6),
                    write(7)
                );
                vec![a, b, c, d, e, f, g, h]
            }
        };

        let served = answers.iter().filter(|(status, _)| *status == 200).count();
        assert_eq!(
            served, 1,
            "case {case:?}: exactly one racer is served, got {answers:?}"
        );
        for (status, body) in &answers {
            if *status != 200 {
                assert_tag_conflict(&case, *status, body, &arn);
            }
        }

        // The post-storm positive control: the resource still takes a tag write.
        let (status, body) = tag(&router, &arn, &json!({ "after": "storm" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a mutation after the storm must be served, got {status} {body}"
        );
    }
}
