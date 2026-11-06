// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SENTENCE this service answers when a request names a
//! resource it does not hold, when a create names a resource that is already there,
//! and when a vector bucket cannot be deleted because it still holds indexes — the
//! `message` member of `NotFoundException` (404) and of `ConflictException` (409),
//! which
//! `smithy_query(query="operation",operation="GetIndex,DeleteIndex,TagResource,ListTagsForResource,CreateIndex,CreateVectorBucket,DeleteVectorBucket")`
//! shows are the modelled refusals of those classes, each with a required `message`.
//!
//! RULE: each of these refusals carries ONE FIXED sentence per error class and
//! resource kind — the same text for every request in that class, naming the kind of
//! resource the OPERATION resolves (not whichever lookup missed on the way to it) and
//! no value the request carried (no name, no ARN, no count). So a client that branches
//! on the text matches one constant string, and the sentence for a missing vector
//! bucket, a missing index, a vector bucket that already exists, an index that already
//! exists and a vector bucket that is not empty are five different strings a client can
//! tell apart.
//!
//! The suites are TABLES whose rows differ along what the rule generalises over: the
//! resource NAME (both ends of the modelled 3..63 length range, the punctuation the
//! name grammar allows, an opaque generated name), HOW the resource came to be
//! missing (never created, created then deleted, a sibling of the same bucket, the
//! parent bucket gone too), WHICH operation and which input form
//! (`vectorBucketName`/`indexName`, `indexArn`, the `/tags/{resourceArn}` URIs)
//! resolves it, what ELSE a duplicate create asks for, and how many indexes a bucket
//! that cannot be deleted holds. Every case drives real HTTP through the production
//! front door, so what is asserted is what an SDK client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{
    call, error_code, expected_bucket_arn, expected_index_arn, list_tags, message_of, ok, router,
    tag,
};

/// The sentence the real service answered for an index it does not hold, as the
/// recorded response carried it. A LITERAL, not the handler's own constant: a suite
/// asserting the constant would agree with any wording the handler chose.
const RECORDED_INDEX_NOT_FOUND: &str = "The specified index could not be found";

/// See [`RECORDED_INDEX_NOT_FOUND`]: the recorded sentence for the other resource
/// kind, which words that kind differently ("vector bucket").
const RECORDED_BUCKET_NOT_FOUND: &str = "The specified vector bucket could not be found";

/// The recorded sentence for a `CreateIndex` whose name the parent vector bucket
/// already holds an index under.
const RECORDED_INDEX_EXISTS: &str = "An index with the specified name already exists";

/// The recorded sentence for a `CreateVectorBucket` whose name this account already
/// holds a vector bucket under.
const RECORDED_BUCKET_EXISTS: &str = "A vector bucket with the specified name already exists";

/// The sentence the service answers for deleting a vector bucket that still holds
/// vector indexes — the other conflict a vector bucket's own state can raise.
const RECORDED_BUCKET_NOT_EMPTY: &str = "The specified vector bucket is not empty";

/// The dimension and distance metric every fixture index is created with. Both are
/// inside the members' modelled bounds (`Dimension` is `range 1..4096`,
/// `DistanceMetric` is `cosine | euclidean`).
const DIMENSION: u32 = 8;
/// See [`DIMENSION`].
const METRIC: &str = "cosine";

/// A `CreateIndex` request body for `index` in `bucket`.
fn create_index_request(bucket: &str, index: &str, dimension: u32, metric: &str) -> Value {
    json!({
        "vectorBucketName": bucket,
        "indexName": index,
        "dataType": "float32",
        "dimension": dimension,
        "distanceMetric": metric,
    })
}

/// Resource names the tables span: the bottom of the modelled length range
/// (`VectorBucketName` and `IndexName` are both `len 3..63`), the top of it, a name
/// carrying both punctuation characters the ARN patterns' name grammar allows
/// (`[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]`), and a name of the opaque generated form a
/// client tends to use.
fn resource_names(stem: &str) -> Vec<(&'static str, String)> {
    let mut shortest = format!("{stem}00");
    shortest.truncate(3);
    vec![
        ("the shortest accepted name", shortest),
        (
            "the longest accepted name",
            format!("{stem}{}z", "0".repeat(62 - stem.len())),
        ),
        ("a name carrying dots and hyphens", format!("{stem}.probe-v2")),
        ("an opaque generated name", format!("{stem}-9f41c2ab7de0")),
    ]
}

/// How the index a probe names came to be absent. The rule holds for each: the store
/// does not hold the index the request asked for, however that came about.
#[derive(Debug, Clone, Copy)]
enum Absent {
    /// The parent bucket exists and the index was never created.
    NeverCreated,
    /// The index was created and then deleted.
    Deleted,
    /// The parent bucket holds a DIFFERENT index, so the lookup misses among siblings.
    SiblingOnly,
    /// The parent vector bucket does not exist either, so the index named cannot
    /// exist. The request still asked for an INDEX, so this is the index refusal.
    BucketMissing,
}

/// Every operation whose missing-index refusal this suite checks, with the input form
/// the request names the index by.
const INDEX_PROBES: &[&str] = &[
    "GetIndex by name",
    "GetIndex by indexArn",
    "DeleteIndex by name",
    "DeleteIndex by indexArn",
    "ListTagsForResource by indexArn",
    "TagResource by indexArn",
    "GetVectors by name",
    "ListVectors by indexArn",
    "DeleteVectors by name",
];

/// Drive one probe against `bucket`/`index` and return its status and body.
async fn probe_index(
    router: &MultiServiceRouter,
    probe: &str,
    bucket: &str,
    index: &str,
) -> (u16, Value) {
    let arn = expected_index_arn(bucket, index);
    let by_name = json!({ "vectorBucketName": bucket, "indexName": index });
    let by_arn = json!({ "indexArn": arn });
    let with_keys = json!({
        "vectorBucketName": bucket,
        "indexName": index,
        "keys": ["probe-key"],
    });
    match probe {
        "GetIndex by name" => call(router, "GetIndex", &by_name).await,
        "GetIndex by indexArn" => call(router, "GetIndex", &by_arn).await,
        "DeleteIndex by name" => call(router, "DeleteIndex", &by_name).await,
        "DeleteIndex by indexArn" => call(router, "DeleteIndex", &by_arn).await,
        "ListTagsForResource by indexArn" => list_tags(router, &arn).await,
        "TagResource by indexArn" => tag(router, &arn, &json!({ "team": "web" })).await,
        "GetVectors by name" => call(router, "GetVectors", &with_keys).await,
        "ListVectors by indexArn" => call(router, "ListVectors", &by_arn).await,
        "DeleteVectors by name" => call(router, "DeleteVectors", &with_keys).await,
        other => panic!("case {other:?}: the suite names no such probe"),
    }
}

/// The vector bucket the missing-index fixtures create.
const FIXTURE_BUCKET: &str = "refusal-wording-bucket";

/// A vector bucket name no fixture ever creates.
const ABSENT_BUCKET: &str = "refusal-wording-absent";

/// A router with `index` absent in the way `absent` describes, and the vector bucket
/// name a probe must then name — the created one, or (for [`Absent::BucketMissing`])
/// one that was never created.
async fn fixture(index: &str, absent: Absent) -> (MultiServiceRouter, &'static str) {
    let router = router();
    let bucket = FIXTURE_BUCKET;
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;
    match absent {
        Absent::BucketMissing => return (router, ABSENT_BUCKET),
        Absent::NeverCreated => {}
        Absent::Deleted => {
            ok(
                &router,
                "CreateIndex",
                &create_index_request(bucket, index, DIMENSION, METRIC),
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
                &create_index_request(bucket, "sibling-index", DIMENSION, METRIC),
            )
            .await;
        }
    }
    (router, bucket)
}

/// Every operation that resolves an index answers the SAME fixed sentence for an
/// index the store does not hold, whatever the name, however it came to be absent —
/// including when the parent vector bucket is what is missing — and whichever input
/// form named it; and the sentence carries no value from the request.
#[tokio::test]
async fn a_missing_index_reports_one_fixed_sentence_on_every_operation_that_resolves_it() {
    for (name_case, index) in resource_names("idx") {
        for absent in [
            Absent::NeverCreated,
            Absent::Deleted,
            Absent::SiblingOnly,
            Absent::BucketMissing,
        ] {
            let (router, bucket) = fixture(&index, absent).await;
            for probe in INDEX_PROBES {
                let case = format!("{name_case} ({absent:?}) on {probe}");
                let (status, body) = probe_index(&router, probe, bucket, &index).await;
                let message = message_of(&body);
                assert_eq!(
                    (status, error_code(&body).as_str(), message.as_str()),
                    (404, "NotFoundException", RECORDED_INDEX_NOT_FOUND),
                    "case {case:?}: a missing index must be the modelled 404 carrying the \
                     service's fixed index sentence, got {status} {body}"
                );
                assert!(
                    !message.contains(&index) && !message.contains(bucket),
                    "case {case:?}: the sentence must carry no value from the request, \
                     got {message:?}"
                );
            }
        }
    }
}

/// What a second `CreateVectorBucket` of a name already taken asks for BESIDES the
/// name, so the one sentence is shown not to depend on the rest of the request.
fn duplicate_bucket_variants(bucket: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("the same request again", json!({ "vectorBucketName": bucket })),
        (
            "the same name with tags",
            json!({ "vectorBucketName": bucket, "tags": { "team": "web" } }),
        ),
        (
            "the same name with an encryption configuration",
            json!({
                "vectorBucketName": bucket,
                "encryptionConfiguration": { "sseType": "AES256" },
            }),
        ),
    ]
}

/// Re-creating a vector bucket that exists is refused 409 with ONE fixed sentence
/// carrying no value from the request, for every name and whatever else the second
/// request asks for — and the refused create changes nothing that was stored.
#[tokio::test]
async fn a_duplicate_vector_bucket_create_reports_one_fixed_sentence() {
    for (name_case, bucket) in resource_names("vb") {
        let router = router();
        let created = ok(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
        assert_eq!(
            created["vectorBucketArn"].as_str(),
            Some(expected_bucket_arn(&bucket).as_str()),
            "case {name_case:?}: the first create must mint the modelled ARN, got {created}"
        );

        for (variant, request) in duplicate_bucket_variants(&bucket) {
            let case = format!("{name_case}: {variant}");
            let (status, body) = call(&router, "CreateVectorBucket", &request).await;
            let message = message_of(&body);
            assert_eq!(
                (status, error_code(&body).as_str(), message.as_str()),
                (409, "ConflictException", RECORDED_BUCKET_EXISTS),
                "case {case:?}: a duplicate vector bucket must be the modelled 409 carrying \
                 the service's fixed sentence, got {status} {body}"
            );
            assert!(
                !message.contains(&bucket),
                "case {case:?}: the sentence must carry no value from the request, \
                 got {message:?}"
            );
        }

        // The refused creates left exactly the one bucket the first create stored,
        // with the ARN it minted: the conflict is a refusal, not a second write.
        let listed = ok(&router, "ListVectorBuckets", &json!({})).await;
        assert_eq!(
            listed["vectorBuckets"].as_array().map(Vec::len),
            Some(1),
            "case {name_case:?}: the refused creates must add no bucket, got {listed}"
        );
        let got = ok(
            &router,
            "GetVectorBucket",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
        assert_eq!(
            got["vectorBucket"]["vectorBucketArn"].as_str(),
            Some(expected_bucket_arn(&bucket).as_str()),
            "case {name_case:?}: the stored bucket must read back unchanged, got {got}"
        );
    }
}

/// What a second `CreateIndex` of a name already taken asks for BESIDES the name.
fn duplicate_index_variants(bucket: &str, index: &str) -> Vec<(&'static str, Value)> {
    let mut with_tags = create_index_request(bucket, index, DIMENSION, METRIC);
    with_tags["tags"] = json!({ "team": "web" });
    let mut by_arn = create_index_request(bucket, index, DIMENSION, METRIC);
    by_arn
        .as_object_mut()
        .expect("the request body is an object")
        .remove("vectorBucketName");
    by_arn["vectorBucketArn"] = json!(expected_bucket_arn(bucket));
    vec![
        (
            "the same request again",
            create_index_request(bucket, index, DIMENSION, METRIC),
        ),
        (
            "the same name with another dimension",
            create_index_request(bucket, index, DIMENSION + 1, METRIC),
        ),
        (
            "the same name with the other distance metric",
            create_index_request(bucket, index, DIMENSION, "euclidean"),
        ),
        ("the same name with tags", with_tags),
        ("the same name, bucket named by ARN", by_arn),
    ]
}

/// Re-creating an index that exists is refused 409 with ONE fixed sentence carrying
/// no value from the request — for every name, whatever else the second request asks
/// for and whichever form names the parent bucket — and the stored index keeps the
/// configuration the FIRST create gave it.
#[tokio::test]
async fn a_duplicate_index_create_reports_one_fixed_sentence() {
    let bucket = "duplicate-index-bucket";
    for (name_case, index) in resource_names("idx") {
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
            &create_index_request(bucket, &index, DIMENSION, METRIC),
        )
        .await;

        for (variant, request) in duplicate_index_variants(bucket, &index) {
            let case = format!("{name_case}: {variant}");
            let (status, body) = call(&router, "CreateIndex", &request).await;
            let message = message_of(&body);
            assert_eq!(
                (status, error_code(&body).as_str(), message.as_str()),
                (409, "ConflictException", RECORDED_INDEX_EXISTS),
                "case {case:?}: a duplicate index must be the modelled 409 carrying the \
                 service's fixed sentence, got {status} {body}"
            );
            assert!(
                !message.contains(&index) && !message.contains(bucket),
                "case {case:?}: the sentence must carry no value from the request, \
                 got {message:?}"
            );
        }

        // The refused creates wrote nothing: the index still reads back with the
        // first create's dimension and metric, and the bucket holds just the one.
        let got = ok(
            &router,
            "GetIndex",
            &json!({ "vectorBucketName": bucket, "indexName": index }),
        )
        .await;
        assert_eq!(
            (
                got["index"]["dimension"].as_u64(),
                got["index"]["distanceMetric"].as_str(),
                got["index"]["indexArn"].as_str(),
            ),
            (
                Some(u64::from(DIMENSION)),
                Some(METRIC),
                Some(expected_index_arn(bucket, &index).as_str()),
            ),
            "case {name_case:?}: the stored index must keep the first create's \
             configuration, got {got}"
        );
        let listed = ok(
            &router,
            "ListIndexes",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
        assert_eq!(
            listed["indexes"].as_array().map(Vec::len),
            Some(1),
            "case {name_case:?}: the refused creates must add no index, got {listed}"
        );
    }
}

/// Deleting a vector bucket that still holds indexes is refused 409 with ONE fixed
/// sentence — the same text however many indexes the bucket holds and whatever they
/// or the bucket are called — and the bucket and its indexes are left intact, so the
/// caller can delete the children and retry.
#[tokio::test]
async fn a_non_empty_vector_bucket_delete_reports_one_fixed_sentence() {
    // The rows differ in how many indexes the bucket holds, which is the value the
    // sentence must NOT carry.
    for held in [1_usize, 2, 5] {
        let bucket = "not-empty-bucket";
        let router = router();
        ok(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
        let indexes: Vec<String> = (0..held).map(|nth| format!("held-index-{nth}")).collect();
        for index in &indexes {
            ok(
                &router,
                "CreateIndex",
                &create_index_request(bucket, index, DIMENSION, METRIC),
            )
            .await;
        }

        let case = format!("a bucket holding {held} index(es)");
        let (status, body) = call(
            &router,
            "DeleteVectorBucket",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
        let message = message_of(&body);
        assert_eq!(
            (status, error_code(&body).as_str(), message.as_str()),
            (409, "ConflictException", RECORDED_BUCKET_NOT_EMPTY),
            "case {case:?}: a non-empty vector bucket must be the modelled 409 carrying \
             the service's fixed sentence, got {status} {body}"
        );
        assert!(
            !message.contains(bucket) && !message.contains(&held.to_string()),
            "case {case:?}: the sentence must carry neither the bucket name nor the \
             index count, got {message:?}"
        );

        // The refused delete removed nothing, and the delete succeeds once the
        // indexes it held are gone.
        let listed = ok(
            &router,
            "ListIndexes",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
        assert_eq!(
            listed["indexes"].as_array().map(Vec::len),
            Some(held),
            "case {case:?}: the refused delete must leave every index, got {listed}"
        );
        for index in &indexes {
            ok(
                &router,
                "DeleteIndex",
                &json!({ "vectorBucketName": bucket, "indexName": index }),
            )
            .await;
        }
        ok(
            &router,
            "DeleteVectorBucket",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
    }
}

/// The five refusals a client distinguishes are five different sentences, and each
/// kind and class picks its own: a missing vector bucket and a missing index are both
/// 404 `NotFoundException` with different text; a duplicate vector bucket, a duplicate
/// index and a bucket that is not empty are all 409 `ConflictException` with different
/// text; and which not-found sentence a request gets follows the resource the
/// OPERATION resolves — an index read under a bucket that does not exist is the
/// missing-INDEX refusal, while a bucket read, an index LIST and a create UNDER that
/// bucket all resolve the bucket itself and so are the missing-BUCKET refusal rather
/// than a conflict.
#[tokio::test]
async fn each_resource_kind_and_error_class_reports_its_own_sentence() {
    let sentences = [
        RECORDED_BUCKET_NOT_FOUND,
        RECORDED_INDEX_NOT_FOUND,
        RECORDED_BUCKET_EXISTS,
        RECORDED_INDEX_EXISTS,
        RECORDED_BUCKET_NOT_EMPTY,
    ];
    for (first, left) in sentences.iter().enumerate() {
        for right in &sentences[first + 1..] {
            assert_ne!(
                left, right,
                "every refusal a client distinguishes must have its own sentence, \
                 got {left:?} twice"
            );
        }
    }

    let bucket = "kinds-and-classes";
    let index = "kinds-index";
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
        &create_index_request(bucket, index, DIMENSION, METRIC),
    )
    .await;

    // (case, operation, request, expected status, expected code, expected sentence)
    let cases: Vec<(&str, &str, Value, u16, &str, &str)> = vec![
        (
            "GetIndex under a bucket that does not exist",
            "GetIndex",
            json!({ "vectorBucketName": ABSENT_BUCKET, "indexName": index }),
            404,
            "NotFoundException",
            RECORDED_INDEX_NOT_FOUND,
        ),
        (
            "GetIndex for an index the live bucket does not hold",
            "GetIndex",
            json!({ "vectorBucketName": bucket, "indexName": "absent-index" }),
            404,
            "NotFoundException",
            RECORDED_INDEX_NOT_FOUND,
        ),
        (
            "GetVectorBucket on a bucket that does not exist",
            "GetVectorBucket",
            json!({ "vectorBucketName": ABSENT_BUCKET }),
            404,
            "NotFoundException",
            RECORDED_BUCKET_NOT_FOUND,
        ),
        (
            "ListIndexes of a bucket that does not exist",
            "ListIndexes",
            json!({ "vectorBucketName": ABSENT_BUCKET }),
            404,
            "NotFoundException",
            RECORDED_BUCKET_NOT_FOUND,
        ),
        (
            "CreateIndex under a bucket that does not exist",
            "CreateIndex",
            create_index_request(ABSENT_BUCKET, index, DIMENSION, METRIC),
            404,
            "NotFoundException",
            RECORDED_BUCKET_NOT_FOUND,
        ),
        (
            "CreateIndex of an index that exists",
            "CreateIndex",
            create_index_request(bucket, index, DIMENSION, METRIC),
            409,
            "ConflictException",
            RECORDED_INDEX_EXISTS,
        ),
        (
            "CreateVectorBucket of a bucket that exists",
            "CreateVectorBucket",
            json!({ "vectorBucketName": bucket }),
            409,
            "ConflictException",
            RECORDED_BUCKET_EXISTS,
        ),
        (
            "DeleteVectorBucket of a bucket that still holds an index",
            "DeleteVectorBucket",
            json!({ "vectorBucketName": bucket }),
            409,
            "ConflictException",
            RECORDED_BUCKET_NOT_EMPTY,
        ),
    ];

    for (case, operation, request, status_expected, code, sentence) in cases {
        let (status, body) = call(&router, operation, &request).await;
        let message = message_of(&body);
        assert_eq!(
            (status, error_code(&body).as_str(), message.as_str()),
            (status_expected, code, sentence),
            "case {case:?}: {operation} must answer this kind's refusal, got {status} {body}"
        );
    }
}
