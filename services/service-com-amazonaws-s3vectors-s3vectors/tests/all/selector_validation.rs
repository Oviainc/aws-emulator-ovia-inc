// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the RESOURCE SELECTOR every S3 Vectors operation takes,
//! served over the real `restJson1` wire through the router production uses.
//!
//! RULE: the two forms a selector accepts are MUTUALLY EXCLUSIVE, so a request names
//! exactly ONE complete form — the bucket selector `vectorBucketName` XOR
//! `vectorBucketArn`, or the index selector `vectorBucketName` + `indexName` XOR
//! `indexArn` — and any other selector is refused with HTTP 400
//! `ValidationException` carrying the ONE fixed sentence for that selector: naming
//! neither form, naming both, or naming half of the name pair all take the same
//! text, which carries no value from the request. The selector is decided before the
//! resource is looked up, so the refusal does not depend on what the store holds and
//! a refused request changes nothing.
//!
//! The members and the selector each operation takes come from
//! `smithy_query(query="operation",operation="GetVectorBucket,ListIndexes,CreateIndex,DeleteVectorBucket,GetVectorBucketPolicy,PutVectorBucketPolicy,DeleteVectorBucketPolicy,GetIndex,DeleteIndex,GetVectors,ListVectors,DeleteVectors,PutVectors,QueryVectors")`,
//! which also gives the `VectorBucketName` / `IndexName` length range (`3..63`) and
//! the `VectorBucketArn` / `IndexArn` patterns the rows span.
//!
//! The suites are TABLES whose rows differ along what the rule generalises over:
//! WHICH operation resolves the selector (every implemented one of each selector
//! kind, control plane and data plane), WHICH way the selector is wrong (nothing
//! named, both forms named and agreeing, both named and disagreeing, half of the
//! name pair, the ARN beside either name), and the resource NAMES (both ends of the
//! modelled length range, the punctuation the name grammar allows, an opaque
//! generated name).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{
    call, error_code, expected_bucket_arn, expected_index_arn, message_of, ok, router,
};

/// The sentence the real service answered for a bucket selector that is not exactly
/// one of its two forms, as the recorded response carried it. A LITERAL, not the
/// handler's own constant: a suite asserting the constant would agree with any
/// wording the handler chose.
const RECORDED_BUCKET_SELECTOR: &str =
    "Must specify either vectorBucketName or vectorBucketArn but not both";

/// See [`RECORDED_BUCKET_SELECTOR`]: the recorded sentence for the index selector,
/// which the service words in terms of the ARN first.
const RECORDED_INDEX_SELECTOR: &str =
    "Must specify either indexArn or both vectorBucketName and indexName";

/// The dimension and distance metric every fixture index is created with, both
/// inside the members' modelled bounds (`Dimension` is `range 1..4096`,
/// `DistanceMetric` is `cosine | euclidean`).
const DIMENSION: usize = 4;
/// See [`DIMENSION`].
const METRIC: &str = "cosine";

/// A bucket and an index no fixture ever creates, named by the ARN of a row whose
/// two selector forms DISAGREE — so agreement is shown not to be what decides the
/// refusal.
const OTHER_BUCKET: &str = "selector-other-bucket";
/// See [`OTHER_BUCKET`].
const OTHER_INDEX: &str = "selector-other-index";

/// Resource names the tables span: the bottom of the modelled length range
/// (`VectorBucketName` and `IndexName` are both `len 3..63`), the top of it, a name
/// carrying both punctuation characters the ARN patterns' name grammar allows
/// (`[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]`), and a name of the opaque generated form a
/// client tends to use.
fn resource_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest accepted name", "sel".to_string()),
        (
            "the longest accepted name",
            format!("s{}z", "0".repeat(61)),
        ),
        (
            "a name carrying dots and hyphens",
            "sel.probe-v2".to_string(),
        ),
        (
            "an opaque generated name",
            "sel-9f41c2ab7de04c11b3e8".to_string(),
        ),
    ]
}

/// A selector that is NOT exactly one of the bucket selector's two forms.
#[derive(Debug, Clone, Copy)]
enum BadBucketSelector {
    /// Neither `vectorBucketName` nor `vectorBucketArn`.
    Nothing,
    /// Both forms, naming the same bucket.
    BothAgreeing,
    /// Both forms, naming different buckets.
    BothDisagreeing,
}

impl BadBucketSelector {
    /// The selector members this row contributes for `bucket`.
    fn members(self, bucket: &str) -> Value {
        match self {
            Self::Nothing => json!({}),
            Self::BothAgreeing => json!({
                "vectorBucketName": bucket,
                "vectorBucketArn": expected_bucket_arn(bucket),
            }),
            Self::BothDisagreeing => json!({
                "vectorBucketName": bucket,
                "vectorBucketArn": expected_bucket_arn(OTHER_BUCKET),
            }),
        }
    }
}

/// A selector that is NOT exactly one COMPLETE form of the index selector.
#[derive(Debug, Clone, Copy)]
enum BadIndexSelector {
    /// Neither the name pair nor `indexArn`.
    Nothing,
    /// Half of the name pair: the bucket without the index.
    BucketOnly,
    /// The other half: the index without the bucket.
    IndexOnly,
    /// The ARN beside `vectorBucketName`.
    ArnAndBucket,
    /// The ARN beside `indexName`.
    ArnAndIndex,
    /// The ARN beside a name pair that agrees with it.
    ArnAndPairAgreeing,
    /// The ARN beside a name pair that does not.
    ArnAndPairDisagreeing,
}

impl BadIndexSelector {
    /// The selector members this row contributes for `bucket`/`index`.
    fn members(self, bucket: &str, index: &str) -> Value {
        let arn = expected_index_arn(bucket, index);
        match self {
            Self::Nothing => json!({}),
            Self::BucketOnly => json!({ "vectorBucketName": bucket }),
            Self::IndexOnly => json!({ "indexName": index }),
            Self::ArnAndBucket => json!({ "vectorBucketName": bucket, "indexArn": arn }),
            Self::ArnAndIndex => json!({ "indexName": index, "indexArn": arn }),
            Self::ArnAndPairAgreeing => json!({
                "vectorBucketName": bucket,
                "indexName": index,
                "indexArn": arn,
            }),
            Self::ArnAndPairDisagreeing => json!({
                "vectorBucketName": bucket,
                "indexName": index,
                "indexArn": expected_index_arn(OTHER_BUCKET, OTHER_INDEX),
            }),
        }
    }
}

/// A policy document by the IAM JSON policy grammar, for the operation that
/// requires one.
fn a_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
            "Action": "s3vectors:GetVectors",
            "Resource": "*",
        }],
    })
    .to_string()
}

/// A `float32` vector of the fixture index's dimension.
fn vector_data() -> Value {
    json!({ "float32": vec![0.5_f32; DIMENSION] })
}

/// Every implemented operation that resolves the BUCKET selector, with the other
/// members its model marks required (so the selector is what the request is refused
/// for, not a missing member).
fn bucket_selector_operations(index: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetVectorBucket", json!({})),
        ("DeleteVectorBucket", json!({})),
        ("ListIndexes", json!({})),
        (
            "CreateIndex",
            json!({
                "indexName": index,
                "dataType": "float32",
                "dimension": DIMENSION,
                "distanceMetric": METRIC,
            }),
        ),
        ("GetVectorBucketPolicy", json!({})),
        ("DeleteVectorBucketPolicy", json!({})),
        ("PutVectorBucketPolicy", json!({ "policy": a_policy() })),
    ]
}

/// Every implemented operation that resolves the INDEX selector, with the other
/// members its model marks required.
fn index_selector_operations() -> Vec<(&'static str, Value)> {
    vec![
        ("GetIndex", json!({})),
        ("DeleteIndex", json!({})),
        ("ListVectors", json!({})),
        ("GetVectors", json!({ "keys": ["v1"] })),
        ("DeleteVectors", json!({ "keys": ["v1"] })),
        (
            "PutVectors",
            json!({ "vectors": [{ "key": "v1", "data": vector_data() }] }),
        ),
        (
            "QueryVectors",
            json!({ "topK": 1, "queryVector": vector_data() }),
        ),
    ]
}

/// One request body: the selector members plus the operation's other members.
fn request(selector: &Value, extra: &Value) -> Value {
    let mut body = selector.clone();
    let object = body
        .as_object_mut()
        .expect("the selector members are an object");
    for (key, value) in extra.as_object().expect("the extra members are an object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// Assert one request is the modelled 400 carrying `sentence` and nothing from the
/// request.
async fn assert_selector_refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    sentence: &str,
    values: &[&str],
) {
    let (status, response) = call(router, operation, body).await;
    let message = message_of(&response);
    assert_eq!(
        (status, error_code(&response).as_str(), message.as_str()),
        (400, "ValidationException", sentence),
        "case {case:?}: {operation} {body} must be the modelled 400 carrying the \
         service's fixed selector sentence, got {status} {response}"
    );
    for value in values {
        assert!(
            !message.contains(value),
            "case {case:?}: the sentence must carry no value from the request, \
             got {message:?} which carries {value:?}"
        );
    }
}

/// A router holding `bucket` with `index` in it.
async fn with_index(bucket: &str, index: &str) -> MultiServiceRouter {
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
            "dimension": DIMENSION,
            "distanceMetric": METRIC,
        }),
    )
    .await;
    router
}

/// Every operation taking the bucket selector refuses a selector that is not exactly
/// one of its two forms — nothing named, or BOTH named whether or not they agree —
/// with the one fixed sentence, for every resource name.
#[tokio::test]
async fn a_bucket_selector_that_is_not_exactly_one_form_reports_one_fixed_sentence() {
    for (name_case, bucket) in resource_names() {
        let index = "selector-index";
        let router = with_index(&bucket, index).await;
        for bad in [
            BadBucketSelector::Nothing,
            BadBucketSelector::BothAgreeing,
            BadBucketSelector::BothDisagreeing,
        ] {
            let selector = bad.members(&bucket);
            for (operation, extra) in bucket_selector_operations("selector-new-index") {
                let case = format!("{name_case}, {bad:?}, on {operation}");
                assert_selector_refused(
                    &router,
                    &case,
                    operation,
                    &request(&selector, &extra),
                    RECORDED_BUCKET_SELECTOR,
                    &[&bucket, &expected_bucket_arn(&bucket), OTHER_BUCKET],
                )
                .await;
            }
        }

        // Every refusal left the store exactly as it was: the bucket and its index
        // are still there, and no second index was created.
        let listed = ok(
            &router,
            "ListIndexes",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
        assert_eq!(
            listed["indexes"]
                .as_array()
                .map(|indexes| indexes
                    .iter()
                    .map(|entry| entry["indexName"].as_str().unwrap_or_default())
                    .collect::<Vec<&str>>()),
            Some(vec![index]),
            "case {name_case:?}: the refused requests must change nothing, got {listed}"
        );
    }
}

/// Every operation taking the index selector refuses a selector that is not exactly
/// one COMPLETE form — nothing named, half of the name pair, or the ARN beside
/// either name — with the one fixed sentence, for every resource name.
#[tokio::test]
async fn an_index_selector_that_is_not_exactly_one_form_reports_one_fixed_sentence() {
    for (name_case, name) in resource_names() {
        // Bucket and index both take the row's name, so a sentence quoting either
        // would be caught.
        let router = with_index(&name, &name).await;
        for bad in [
            BadIndexSelector::Nothing,
            BadIndexSelector::BucketOnly,
            BadIndexSelector::IndexOnly,
            BadIndexSelector::ArnAndBucket,
            BadIndexSelector::ArnAndIndex,
            BadIndexSelector::ArnAndPairAgreeing,
            BadIndexSelector::ArnAndPairDisagreeing,
        ] {
            let selector = bad.members(&name, &name);
            for (operation, extra) in index_selector_operations() {
                let case = format!("{name_case}, {bad:?}, on {operation}");
                assert_selector_refused(
                    &router,
                    &case,
                    operation,
                    &request(&selector, &extra),
                    RECORDED_INDEX_SELECTOR,
                    &[&name, &expected_index_arn(&name, &name), OTHER_INDEX],
                )
                .await;
            }
        }

        // The refusals wrote nothing and removed nothing: the index still reads back
        // and still holds no vectors.
        let got = ok(
            &router,
            "GetIndex",
            &json!({ "vectorBucketName": name, "indexName": name }),
        )
        .await;
        assert_eq!(
            got["index"]["indexArn"].as_str(),
            Some(expected_index_arn(&name, &name).as_str()),
            "case {name_case:?}: the refused requests must leave the index, got {got}"
        );
        let listed = ok(
            &router,
            "ListVectors",
            &json!({ "indexArn": expected_index_arn(&name, &name) }),
        )
        .await;
        assert_eq!(
            listed["vectors"].as_array().map(Vec::len),
            Some(0),
            "case {name_case:?}: the refused PutVectors must store nothing, got {listed}"
        );
    }
}

/// Exactly one complete form IS accepted, on both selectors and on both the control
/// plane and the data plane: the two forms reach the same resource, and what a
/// request writes through one form reads back through the other.
#[tokio::test]
async fn exactly_one_selector_form_resolves_the_same_resource() {
    let bucket = "selector-accepted";
    let index = "selector-accepted-index";
    let router = router();

    // The bucket selector by name, then by ARN: CreateIndex names its parent bucket
    // by ARN alone, and ListIndexes reads it back by name alone.
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
            "vectorBucketArn": expected_bucket_arn(bucket),
            "indexName": index,
            "dataType": "float32",
            "dimension": DIMENSION,
            "distanceMetric": METRIC,
        }),
    )
    .await;
    let listed = ok(
        &router,
        "ListIndexes",
        &json!({ "vectorBucketName": bucket }),
    )
    .await;
    assert_eq!(
        listed["indexes"][0]["indexArn"].as_str(),
        Some(expected_index_arn(bucket, index).as_str()),
        "an index created by bucket ARN must be listed by bucket name, got {listed}"
    );

    // The index selector by ARN, then by name pair: a put through the ARN reads back
    // through the pair, and both forms report the same index.
    let arn = expected_index_arn(bucket, index);
    ok(
        &router,
        "PutVectors",
        &json!({
            "indexArn": arn,
            "vectors": [{ "key": "v1", "data": vector_data() }],
        }),
    )
    .await;
    let got = ok(
        &router,
        "GetVectors",
        &json!({
            "vectorBucketName": bucket,
            "indexName": index,
            "keys": ["v1"],
            "returnData": true,
        }),
    )
    .await;
    assert_eq!(
        got["vectors"][0]["data"],
        vector_data(),
        "a vector put by index ARN must read back by the name pair, got {got}"
    );
    for (case, selector) in [
        ("by the name pair", json!({ "vectorBucketName": bucket, "indexName": index })),
        ("by indexArn", json!({ "indexArn": arn })),
    ] {
        let got = ok(&router, "GetIndex", &selector).await;
        assert_eq!(
            got["index"]["indexArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the accepted form must resolve the same index, got {got}"
        );
    }
}

/// The two selectors answer DIFFERENT sentences, and each names only its own
/// members, so a client reads which selector it got wrong from the text.
#[tokio::test]
async fn each_selector_reports_its_own_sentence() {
    assert_ne!(
        RECORDED_BUCKET_SELECTOR, RECORDED_INDEX_SELECTOR,
        "the two selectors must be distinguishable by their sentence"
    );
    assert!(
        !RECORDED_BUCKET_SELECTOR.contains("indexArn")
            && RECORDED_INDEX_SELECTOR.contains("indexArn"),
        "only the index selector's sentence names `indexArn`, got {RECORDED_BUCKET_SELECTOR:?} \
         and {RECORDED_INDEX_SELECTOR:?}"
    );

    let router = with_index("selector-kinds", "selector-kinds-index").await;
    for (case, operation, body, sentence) in [
        (
            "a bucket-selector operation with no selector",
            "GetVectorBucket",
            json!({}),
            RECORDED_BUCKET_SELECTOR,
        ),
        (
            "an index-selector operation with no selector",
            "GetIndex",
            json!({}),
            RECORDED_INDEX_SELECTOR,
        ),
        (
            "a bucket-selector operation naming both forms",
            "ListIndexes",
            json!({
                "vectorBucketName": "selector-kinds",
                "vectorBucketArn": expected_bucket_arn("selector-kinds"),
            }),
            RECORDED_BUCKET_SELECTOR,
        ),
        (
            "an index-selector operation naming both forms",
            "QueryVectors",
            json!({
                "vectorBucketName": "selector-kinds",
                "indexName": "selector-kinds-index",
                "indexArn": expected_index_arn("selector-kinds", "selector-kinds-index"),
                "topK": 1,
                "queryVector": vector_data(),
            }),
            RECORDED_INDEX_SELECTOR,
        ),
    ] {
        assert_selector_refused(&router, case, operation, &body, sentence, &[]).await;
    }
}
