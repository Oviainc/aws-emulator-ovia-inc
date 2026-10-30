// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the index-mode surface of the S3 Vectors control plane:
//! `VectorBucket$defaultIndexMode` and `Index$indexMode`, served over the real
//! `restJson1` wire through the router production uses.
//!
//! The rule under test: a member the SERVICE assigns — a vector bucket's default
//! index mode, which no create request names, and the mode a vector index inherits
//! from that default when it is created — holds a value from the moment the resource
//! exists, so every read of the resource reports it and no read ever omits it. A
//! later change of the bucket's default is assigned to the indexes created after it
//! and to no others.
//!
//! Each table row states the case it covers and every assertion reports the actual
//! value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{
    call, error_code, expected_bucket_arn, expected_index_arn, ok, router,
};

/// The mode a vector bucket's default carries before anything sets one, which is the
/// mode the real service reported for a freshly created vector bucket in the recorded
/// response. Spelled out here rather than read from the crate so the test pins the
/// value a client sees rather than restating the implementation's own constant.
const SERVICE_ASSIGNED_MODE: &str = "ENHANCED";

/// Every value of the model's `IndexMode` enum, from
/// `smithy_query(query="shape",shape="IndexMode")`: `CLASSIC` and `ENHANCED`.
const EVERY_MODE: [&str; 2] = ["CLASSIC", "ENHANCED"];

/// The `defaultIndexMode` GetVectorBucket reports for the bucket `request` names.
///
/// Panics when the member is absent, which is the defect this suite exists for: a
/// client reading the bucket's default before it creates indexes under it must find a
/// value rather than a missing key.
async fn reported_default_mode(router: &MultiServiceRouter, request: &Value) -> String {
    let body = ok(router, "GetVectorBucket", request).await;
    body["vectorBucket"]["defaultIndexMode"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("GetVectorBucket must report a defaultIndexMode string, got {body}")
        })
        .to_string()
}

/// The `indexMode` GetIndex reports for the index `request` names. Panics when the
/// member is absent, for the same reason [`reported_default_mode`] does.
async fn reported_index_mode(router: &MultiServiceRouter, request: &Value) -> String {
    let body = ok(router, "GetIndex", request).await;
    body["index"]["indexMode"]
        .as_str()
        .unwrap_or_else(|| panic!("GetIndex must report an indexMode string, got {body}"))
        .to_string()
}

/// Set the bucket's default index mode, asserting the put succeeded.
async fn put_default_mode(router: &MultiServiceRouter, bucket: &str, mode: &str) {
    ok(
        router,
        "PutVectorBucketDefaultIndexMode",
        &json!({ "vectorBucketName": bucket, "defaultIndexMode": mode }),
    )
    .await;
}

/// A CreateIndex request for `index` in `bucket`, in the smallest form the model takes.
fn index_request(bucket: &str, index: &str) -> Value {
    json!({
        "vectorBucketName": bucket,
        "indexName": index,
        "dataType": "float32",
        "dimension": 8,
        "distanceMetric": "cosine",
    })
}

/// One creation case: what a failing assertion calls it, the CreateVectorBucket
/// request, and the CreateIndex request for an index in that bucket.
type CreationRow = (&'static str, Value, Value);

/// Creation requests that differ in everything the two create operations DO take —
/// neither of them takes a mode — so each row is a different way of creating the pair
/// and every row must still report the mode the service assigns.
///
/// The members and their bounds are the model's:
/// `smithy_query(query="operation",operation="CreateVectorBucket,CreateIndex")` plus
/// `smithy_query(query="shape",shape="EncryptionConfiguration,DataType,MetadataConfiguration")`
/// — `dimension` is `range 1..4096`, a bucket or index name is `length 3..63`,
/// `dataType` holds only `float32`, `distanceMetric` holds `cosine` and `euclidean`,
/// and `kmsKeyArn` is allowed if and only if `sseType` is `aws:kms`.
fn creation_rows() -> Vec<CreationRow> {
    vec![
        (
            "the smallest request either create operation takes",
            json!({ "vectorBucketName": "mode-plain" }),
            index_request("mode-plain", "idx-plain"),
        ),
        (
            "an explicit S3-managed encryption configuration and tags on both resources",
            json!({
                "vectorBucketName": "mode-tagged",
                "encryptionConfiguration": { "sseType": "AES256" },
                "tags": { "team": "search" },
            }),
            json!({
                "vectorBucketName": "mode-tagged",
                "indexName": "idx-tagged",
                "dataType": "float32",
                "dimension": 128,
                "distanceMetric": "euclidean",
                "tags": { "team": "search" },
            }),
        ),
        (
            "a KMS encryption configuration the index overrides with the S3-managed one",
            json!({
                "vectorBucketName": "mode-encrypted",
                "encryptionConfiguration": {
                    "sseType": "aws:kms",
                    "kmsKeyArn":
                        "arn:aws:kms:us-east-1:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                },
            }),
            json!({
                "vectorBucketName": "mode-encrypted",
                "indexName": "idx-encrypted",
                "dataType": "float32",
                "dimension": 4096,
                "distanceMetric": "cosine",
                "encryptionConfiguration": { "sseType": "AES256" },
            }),
        ),
        (
            "the bottom of dimension's modelled range, with a metadata configuration",
            json!({ "vectorBucketName": "mode-metadata" }),
            json!({
                "vectorBucketName": "mode-metadata",
                "indexName": "idx-metadata",
                "dataType": "float32",
                "dimension": 1,
                "distanceMetric": "euclidean",
                "metadataConfiguration": { "nonFilterableMetadataKeys": ["source", "author"] },
            }),
        ),
        (
            "names at the bottom of the modelled length range",
            json!({ "vectorBucketName": "abc" }),
            index_request("abc", "xyz"),
        ),
        (
            "names at the top of the modelled length range",
            json!({ "vectorBucketName": "mode-bucket-with-a-name-of-exactly-sixty-three-characters-xxxxx" }),
            index_request(
                "mode-bucket-with-a-name-of-exactly-sixty-three-characters-xxxxx",
                "mode-index-with-a-name-of-exactly-sixty-three-characters-xxxxxx",
            ),
        ),
    ]
}

/// However a vector bucket and a vector index are created, both report the mode the
/// service assigns them: the bucket's default, and the index's own mode inherited from
/// it. Neither member is ever absent, and reading them does not change them.
#[tokio::test]
async fn every_created_bucket_and_index_reports_the_mode_the_service_assigns() {
    let router = router();

    for (case, create_bucket, create_index) in creation_rows() {
        let bucket = create_bucket["vectorBucketName"]
            .as_str()
            .expect("the row names its bucket")
            .to_string();
        let index = create_index["indexName"]
            .as_str()
            .expect("the row names its index")
            .to_string();
        ok(&router, "CreateVectorBucket", &create_bucket).await;

        // The bucket has a default before any index exists under it: a client reads it
        // to decide whether to override the mode per index.
        let selector = json!({ "vectorBucketName": bucket });
        let reported = reported_default_mode(&router, &selector).await;
        assert_eq!(
            reported, SERVICE_ASSIGNED_MODE,
            "case {case:?}: a bucket created with no mode must report the mode the \
             service assigns, got {reported:?}"
        );

        ok(&router, "CreateIndex", &create_index).await;
        let index_selector = json!({ "vectorBucketName": bucket, "indexName": index });
        let reported_index = reported_index_mode(&router, &index_selector).await;
        assert_eq!(
            reported_index, SERVICE_ASSIGNED_MODE,
            "case {case:?}: an index created with no mode must report the mode it \
             inherited from its bucket, got {reported_index:?}"
        );

        // A read reports, it does not consume: the second read of each resource
        // answers exactly what the first one did.
        let again = reported_default_mode(&router, &selector).await;
        let index_again = reported_index_mode(&router, &index_selector).await;
        assert_eq!(
            (again.as_str(), index_again.as_str()),
            (reported.as_str(), reported_index.as_str()),
            "case {case:?}: a repeated read must report the same modes, got \
             {again:?} and {index_again:?}"
        );

        // The other member the service assigns at create is the creation time, which is
        // minted once and reported by every read: its VALUE is the clock's, so what a
        // read owes is a timestamp that is present and does not move.
        let first = ok(&router, "GetVectorBucket", &selector).await;
        let second = ok(&router, "GetVectorBucket", &selector).await;
        let created = first["vectorBucket"]["creationTime"].clone();
        assert!(
            created.is_number() && created == second["vectorBucket"]["creationTime"],
            "case {case:?}: the bucket's creation time must be a timestamp that does not \
             move, got {created:?} then {:?}",
            second["vectorBucket"]["creationTime"]
        );
        let first = ok(&router, "GetIndex", &index_selector).await;
        let second = ok(&router, "GetIndex", &index_selector).await;
        let created = first["index"]["creationTime"].clone();
        assert!(
            created.is_number() && created == second["index"]["creationTime"],
            "case {case:?}: the index's creation time must be a timestamp that does not \
             move, got {created:?} then {:?}",
            second["index"]["creationTime"]
        );
    }
}

/// A new default applies to the indexes created after it and to no others: the model
/// states it of the member ("this change doesn't affect existing vector indexes") and
/// of the operation that sets it.
///
/// The rows are every value of the `IndexMode` enum, each put over both the mode the
/// service assigned and over the other value, so the table covers a put that changes
/// the default and a put that re-states it.
#[tokio::test]
async fn a_new_default_applies_to_later_indexes_and_leaves_existing_ones_alone() {
    for mode in EVERY_MODE {
        let router = router();
        let bucket = format!("mode-switch-{}", mode.to_lowercase());
        ok(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;

        // An index created BEFORE the put, under the mode the service assigned.
        ok(&router, "CreateIndex", &index_request(&bucket, "idx-before")).await;

        put_default_mode(&router, &bucket, mode).await;
        let reported = reported_default_mode(&router, &json!({ "vectorBucketName": bucket })).await;
        assert_eq!(
            reported, mode,
            "case {mode:?}: the bucket must report the default just put, got {reported:?}"
        );

        // An index created AFTER the put inherits the new default.
        ok(&router, "CreateIndex", &index_request(&bucket, "idx-after")).await;
        let after =
            reported_index_mode(&router, &json!({ "vectorBucketName": bucket, "indexName": "idx-after" }))
                .await;
        assert_eq!(
            after, mode,
            "case {mode:?}: an index created after the put inherits the new default, \
             got {after:?}"
        );

        // The index that already existed keeps the mode it was created with.
        let before = reported_index_mode(
            &router,
            &json!({ "vectorBucketName": bucket, "indexName": "idx-before" }),
        )
        .await;
        assert_eq!(
            before, SERVICE_ASSIGNED_MODE,
            "case {mode:?}: an index that already existed keeps its own mode, got {before:?}"
        );

        // The put is `@idempotent`: repeating it leaves the same default.
        put_default_mode(&router, &bucket, mode).await;
        let repeated = reported_default_mode(&router, &json!({ "vectorBucketName": bucket })).await;
        assert_eq!(
            repeated, mode,
            "case {mode:?}: a repeated put leaves the same default, got {repeated:?}"
        );

        // And one bucket's default is its own: a second bucket still reports the mode
        // the service assigns.
        let other = format!("{bucket}-other");
        ok(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": other }),
        )
        .await;
        let other_mode = reported_default_mode(&router, &json!({ "vectorBucketName": other })).await;
        assert_eq!(
            other_mode, SERVICE_ASSIGNED_MODE,
            "case {mode:?}: another bucket's default is untouched, got {other_mode:?}"
        );
    }
}

/// The mode is reported through every selector the model accepts for the resource it
/// belongs to, and set through either bucket selector: the member is a property of the
/// resource, not of how the request named it.
#[tokio::test]
async fn every_accepted_selector_reports_and_sets_the_same_mode() {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "mode-selectors" }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &index_request("mode-selectors", "idx-selectors"),
    )
    .await;
    let bucket_arn = expected_bucket_arn("mode-selectors");
    let index_arn = expected_index_arn("mode-selectors", "idx-selectors");

    for (case, selector) in [
        ("by name", json!({ "vectorBucketName": "mode-selectors" })),
        ("by ARN", json!({ "vectorBucketArn": bucket_arn })),
    ] {
        let reported = reported_default_mode(&router, &selector).await;
        assert_eq!(
            reported, SERVICE_ASSIGNED_MODE,
            "case {case:?}: the bucket's default must be reported, got {reported:?}"
        );
    }
    for (case, selector) in [
        (
            "by the name pair",
            json!({ "vectorBucketName": "mode-selectors", "indexName": "idx-selectors" }),
        ),
        ("by ARN", json!({ "indexArn": index_arn })),
    ] {
        let reported = reported_index_mode(&router, &selector).await;
        assert_eq!(
            reported, SERVICE_ASSIGNED_MODE,
            "case {case:?}: the index's mode must be reported, got {reported:?}"
        );
    }

    // A put that names the bucket by ARN reaches the bucket the name reads.
    ok(
        &router,
        "PutVectorBucketDefaultIndexMode",
        &json!({ "vectorBucketArn": expected_bucket_arn("mode-selectors"),
                 "defaultIndexMode": "CLASSIC" }),
    )
    .await;
    let reported =
        reported_default_mode(&router, &json!({ "vectorBucketName": "mode-selectors" })).await;
    assert_eq!(
        reported, "CLASSIC",
        "a put by ARN must be readable by name, got {reported:?}"
    );
}

/// A put that cannot resolve the bucket it names is refused the way every other
/// bucket-selector operation refuses it: a selector that is not exactly one accepted
/// form is the modelled 400, a bucket this account does not hold the modelled 404 —
/// and neither leaves a default behind.
#[tokio::test]
async fn a_put_refuses_a_bucket_selector_it_cannot_resolve() {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "mode-refusals" }),
    )
    .await;

    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "no bucket selector at all",
            json!({ "defaultIndexMode": "CLASSIC" }),
            400,
            "ValidationException",
        ),
        (
            "both accepted selector forms at once",
            json!({ "vectorBucketName": "mode-refusals",
                    "vectorBucketArn": expected_bucket_arn("mode-refusals"),
                    "defaultIndexMode": "CLASSIC" }),
            400,
            "ValidationException",
        ),
        (
            "an index ARN where a bucket ARN belongs",
            json!({ "vectorBucketArn": expected_index_arn("mode-refusals", "idx"),
                    "defaultIndexMode": "CLASSIC" }),
            400,
            "ValidationException",
        ),
        (
            "a mode outside the modelled enum",
            json!({ "vectorBucketName": "mode-refusals", "defaultIndexMode": "TURBO" }),
            400,
            "ValidationException",
        ),
        (
            "a bucket this account does not hold",
            json!({ "vectorBucketName": "mode-absent", "defaultIndexMode": "CLASSIC" }),
            404,
            "NotFoundException",
        ),
    ];

    for (case, request, status, code) in cases {
        let (actual_status, body) =
            call(&router, "PutVectorBucketDefaultIndexMode", &request).await;
        assert_eq!(
            (actual_status, error_code(&body).as_str()),
            (status, code),
            "case {case:?}: the put must answer {status} {code}, got {actual_status} {body}"
        );
    }

    // No refused put changed the bucket's default.
    let reported =
        reported_default_mode(&router, &json!({ "vectorBucketName": "mode-refusals" })).await;
    assert_eq!(
        reported, SERVICE_ASSIGNED_MODE,
        "a refused put must leave the default alone, got {reported:?}"
    );
}
