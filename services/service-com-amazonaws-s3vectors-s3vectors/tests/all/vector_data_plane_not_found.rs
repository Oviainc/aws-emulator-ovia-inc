// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH RESOURCE KIND the vector data plane names when it
//! cannot resolve the target of the call.
//!
//! `smithy_query(query="operation",operation="PutVectors,GetVectors,ListVectors,QueryVectors,DeleteVectors")`
//! shows that all five data-plane operations take the SAME target selector —
//! `vectorBucketName` + `indexName`, or `indexArn` — and that each models
//! `NotFoundException` (HTTP 404) with a required `message`.
//!
//! RULE: that selector names ONE resource, the vector index, so a data-plane call
//! whose index the store does not hold is refused with the modelled 404 carrying
//! the service's fixed INDEX sentence — including when the parent vector bucket is
//! itself absent, because an index of a bucket that does not exist is exactly as
//! absent as one of a bucket that does. The parent bucket is an intermediate lookup
//! on the way to the addressed resource, never the kind the refusal reports; only an
//! operation whose own target IS the vector bucket reports the bucket sentence.
//!
//! The suites are TABLES whose rows differ along what the rule generalises over:
//! which of the five operations is called, which input form names the index
//! (the name pair or `indexArn`), the bucket and index NAMES (both ends of the
//! modelled 3..63 length range and the punctuation the ARN name grammar allows),
//! HOW the index came to be absent (the parent bucket never created, the bucket
//! there but the index never created, the index created then deleted, only a
//! sibling index present), and whether the request's own payload would have been
//! refused had the index existed. Every case drives real HTTP through the
//! production front door, so what is asserted is what an SDK client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{call, error_code, expected_index_arn, message_of, ok, router};

/// The sentence the real service answered for an index it does not hold, as the
/// recorded response carried it. A LITERAL, not the handler's own constant: a suite
/// asserting the constant would agree with any wording the handler chose.
const RECORDED_INDEX_NOT_FOUND: &str = "The specified index could not be found";

/// See [`RECORDED_INDEX_NOT_FOUND`]: the recorded sentence for the other resource
/// kind a `NotFoundException` of this service can be about, which words that kind
/// differently ("vector bucket"). The defect this suite pins is a data-plane call
/// answering THIS sentence for a target whose addressed resource is the index.
const RECORDED_BUCKET_NOT_FOUND: &str = "The specified vector bucket could not be found";

/// The dimension and distance metric every fixture index is created with. Both lie
/// inside the members' modelled bounds (`Dimension` is `range 1..4096`,
/// `DistanceMetric` is `cosine | euclidean`).
const DIMENSION: usize = 8;
/// See [`DIMENSION`].
const METRIC: &str = "cosine";

/// `DIMENSION` float32 values an index of that dimension accepts.
fn data(seed: f32) -> Value {
    let values: Vec<f32> = (0..DIMENSION).map(|nth| seed + nth as f32).collect();
    json!({ "float32": values })
}

/// Every vector data-plane operation, with the complete request body it needs
/// BESIDES the index selector. `vectors` and `keys` are required members within
/// their modelled list bounds (`PutVectors$vectors` 1..500, `GetVectors$keys`
/// 1..100, `DeleteVectors$keys` 1..500) and `QueryVectors` needs `topK` (range
/// 1..) and `queryVector`, so each row is a request the framework accepts and the
/// handler must answer.
fn data_plane_operations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "PutVectors",
            json!({ "vectors": [{ "key": "probe-key", "data": data(0.5) }] }),
        ),
        (
            "GetVectors",
            json!({ "keys": ["probe-key"], "returnData": true, "returnMetadata": true }),
        ),
        ("ListVectors", json!({ "maxResults": 10 })),
        (
            "QueryVectors",
            json!({ "topK": 3, "queryVector": data(0.25), "returnDistance": true }),
        ),
        ("DeleteVectors", json!({ "keys": ["probe-key"] })),
    ]
}

/// How the index a probe addresses is named: the two MUTUALLY EXCLUSIVE forms the
/// model's selector offers.
#[derive(Debug, Clone, Copy)]
enum Selector {
    /// `vectorBucketName` + `indexName`.
    NamePair,
    /// `indexArn`.
    Arn,
}

impl Selector {
    /// The selector members for `bucket`/`index` in this form.
    fn members(self, bucket: &str, index: &str) -> Value {
        match self {
            Self::NamePair => json!({ "vectorBucketName": bucket, "indexName": index }),
            Self::Arn => json!({ "indexArn": expected_index_arn(bucket, index) }),
        }
    }
}

/// How the index a probe addresses came to be absent. The rule holds for each: the
/// store does not hold the index the request asked for, however that came about.
#[derive(Debug, Clone, Copy)]
enum Absent {
    /// Neither the vector bucket nor the index was ever created — the recorded case.
    BucketMissing,
    /// The parent bucket exists and the index was never created.
    IndexNeverCreated,
    /// The index was created and then deleted.
    IndexDeleted,
    /// The parent bucket holds a DIFFERENT index, so the lookup misses among siblings.
    SiblingOnly,
}

/// The (bucket, index) name pairs the tables span: the bottom of the modelled length
/// range (`VectorBucketName` and `IndexName` are both `len 3..63`), the top of it,
/// and a pair carrying both punctuation characters the ARN patterns' name grammar
/// allows (`[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]`).
fn absent_pairs() -> Vec<(&'static str, String, String)> {
    let longest = |stem: &str| format!("{stem}{}z", "0".repeat(62 - stem.len()));
    vec![
        (
            "the shortest accepted names",
            "ab3".to_string(),
            "xy3".to_string(),
        ),
        (
            "the longest accepted names",
            longest("dpb"),
            longest("dpi"),
        ),
        (
            "names carrying dots and hyphens",
            "dp.bucket-v2".to_string(),
            "dp.index-v2".to_string(),
        ),
    ]
}

/// A `CreateIndex` request body for `index` in `bucket`.
fn create_index_request(bucket: &str, index: &str, dimension: usize) -> Value {
    json!({
        "vectorBucketName": bucket,
        "indexName": index,
        "dataType": "float32",
        "dimension": dimension,
        "distanceMetric": METRIC,
    })
}

/// A router in which `bucket`/`index` is absent in the way `absent` describes.
async fn fixture(bucket: &str, index: &str, absent: Absent) -> MultiServiceRouter {
    let router = router();
    if matches!(absent, Absent::BucketMissing) {
        return router;
    }
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;
    match absent {
        Absent::BucketMissing | Absent::IndexNeverCreated => {}
        Absent::IndexDeleted => {
            ok(
                &router,
                "CreateIndex",
                &create_index_request(bucket, index, DIMENSION),
            )
            .await;
            ok(
                &router,
                "DeleteIndex",
                &json!({ "vectorBucketName": bucket, "indexName": index }),
            )
            .await;
        }
        Absent::SiblingOnly => {
            ok(
                &router,
                "CreateIndex",
                &create_index_request(bucket, "sibling-index", DIMENSION),
            )
            .await;
        }
    }
    router
}

/// One request body: the operation's own members plus the index selector.
fn request(members: &Value, selector: Selector, bucket: &str, index: &str) -> Value {
    let mut body = members.clone();
    let target = selector.members(bucket, index);
    let object = body
        .as_object_mut()
        .expect("the request body is an object");
    for (member, value) in target
        .as_object()
        .expect("the selector is an object")
        .iter()
    {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// Every vector data-plane operation refuses an index the store does not hold with
/// the modelled 404 carrying the service's fixed INDEX sentence — whichever input
/// form named it, whatever the names, and however the index came to be absent,
/// including when the parent vector bucket is what is missing. The sentence never
/// names the other resource kind and carries no value from the request.
#[tokio::test]
async fn every_data_plane_operation_reports_the_index_for_a_target_it_cannot_resolve() {
    for (name_case, bucket, index) in absent_pairs() {
        for absent in [
            Absent::BucketMissing,
            Absent::IndexNeverCreated,
            Absent::IndexDeleted,
            Absent::SiblingOnly,
        ] {
            let router = fixture(&bucket, &index, absent).await;
            for selector in [Selector::NamePair, Selector::Arn] {
                for (operation, members) in data_plane_operations() {
                    let case =
                        format!("{name_case} ({absent:?}) on {operation} by {selector:?}");
                    let body = request(&members, selector, &bucket, &index);
                    let (status, answered) = call(&router, operation, &body).await;
                    let message = message_of(&answered);
                    assert_eq!(
                        (status, error_code(&answered).as_str(), message.as_str()),
                        (404, "NotFoundException", RECORDED_INDEX_NOT_FOUND),
                        "case {case:?}: an unresolvable data-plane target must be the \
                         modelled 404 naming the index, got {status} {answered}"
                    );
                    assert!(
                        !message.contains(&index) && !message.contains(&bucket),
                        "case {case:?}: the sentence must carry no value from the \
                         request, got {message:?}"
                    );
                }
            }
        }
    }
}

/// The kind a refusal names follows the resource the OPERATION addresses, so with
/// the SAME vector bucket absent the five data-plane calls name the index while the
/// calls whose own target is that bucket name the bucket — two different sentences a
/// client can tell apart.
#[tokio::test]
async fn the_kind_named_follows_the_resource_the_operation_addresses() {
    assert_ne!(
        RECORDED_INDEX_NOT_FOUND, RECORDED_BUCKET_NOT_FOUND,
        "the two resource kinds must answer different sentences"
    );

    let bucket = "addressed-resource-bucket";
    let index = "addressed-resource-index";
    let router = router();

    for (operation, members) in data_plane_operations() {
        let body = request(&members, Selector::NamePair, bucket, index);
        let (status, answered) = call(&router, operation, &body).await;
        assert_eq!(
            (
                status,
                error_code(&answered).as_str(),
                message_of(&answered).as_str()
            ),
            (404, "NotFoundException", RECORDED_INDEX_NOT_FOUND),
            "case {operation:?}: a data-plane call addresses the index, got \
             {status} {answered}"
        );
    }

    // The operations whose own target is the vector bucket, under the same absent
    // bucket: these report the bucket, which is what makes the kind informative.
    let bucket_plane: Vec<(&str, Value)> = vec![
        ("GetVectorBucket", json!({ "vectorBucketName": bucket })),
        ("ListIndexes", json!({ "vectorBucketName": bucket })),
        (
            "CreateIndex",
            create_index_request(bucket, index, DIMENSION),
        ),
    ];
    for (operation, body) in bucket_plane {
        let (status, answered) = call(&router, operation, &body).await;
        assert_eq!(
            (
                status,
                error_code(&answered).as_str(),
                message_of(&answered).as_str()
            ),
            (404, "NotFoundException", RECORDED_BUCKET_NOT_FOUND),
            "case {operation:?}: a call that addresses the vector bucket itself \
             reports that kind, got {status} {answered}"
        );
    }
}

/// Resolving the target comes BEFORE checking the request's own payload against it:
/// a payload a live index refuses (400) is still the index 404 when the index is not
/// there, so a client never has to tell a payload defect from an absent target.
#[tokio::test]
async fn an_unresolvable_target_is_reported_before_the_payload_is_checked() {
    let bucket = "payload-order-bucket";
    let live = "payload-order-index";
    let absent = "payload-order-absent";

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
        &create_index_request(bucket, live, DIMENSION),
    )
    .await;

    // Each row is a payload the LIVE index refuses: vector data of the wrong
    // dimension, which `PutInputVector$data` and `QueryVectors$queryVector`
    // document must match "the dimension count that's configured for the vector
    // index".
    let wrong_dimension = json!({ "float32": [1.0, 0.0] });
    let cases: Vec<(&str, Value)> = vec![
        (
            "PutVectors",
            json!({ "vectors": [{ "key": "probe-key", "data": wrong_dimension.clone() }] }),
        ),
        (
            "QueryVectors",
            json!({ "topK": 1, "queryVector": wrong_dimension.clone() }),
        ),
    ];

    for (operation, members) in cases {
        let (status, answered) = call(
            &router,
            operation,
            &request(&members, Selector::NamePair, bucket, live),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {operation:?} on the live index: a payload the index cannot take \
             is the modelled 400, got {status} {answered}"
        );

        let (status, answered) = call(
            &router,
            operation,
            &request(&members, Selector::NamePair, bucket, absent),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&answered).as_str(),
                message_of(&answered).as_str()
            ),
            (404, "NotFoundException", RECORDED_INDEX_NOT_FOUND),
            "case {operation:?} on the absent index: the unresolvable target is \
             reported first, got {status} {answered}"
        );
    }
}
