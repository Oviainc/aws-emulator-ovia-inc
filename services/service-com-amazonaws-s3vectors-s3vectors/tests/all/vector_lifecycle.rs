// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the S3 Vectors slice: the vector bucket -> vector index ->
//! vector lifecycle, served over the real `restJson1` wire through the router that
//! production uses.
//!
//! Every test drives HTTP requests through [`MultiServiceRouter::dispatch`], so what
//! is asserted is what a client sees: status, error code and response body. Each
//! table row states the case it covers and every assertion reports the actual value.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header is how a request names the service it is for
/// (the router's `resolve_route_key`) and the region the ARNs it mints carry. The
/// signature itself is not verified: this router runs with request authentication
/// off, which is the default for an in-process emulator.
pub(crate) const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
Credential=AKIDTESTS3VECTORS/20260101/us-east-1/s3vectors/aws4_request, \
SignedHeaders=host;x-amz-date, Signature=0000";

/// The region in [`AUTHORIZATION`]'s credential scope.
pub(crate) const REGION: &str = "us-east-1";

/// The ARN a vector bucket of this name must carry, per `VectorBucketArn`'s pattern.
pub(crate) fn expected_bucket_arn(bucket: &str) -> String {
    format!(
        "arn:aws:s3vectors:{REGION}:{}:bucket/{bucket}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The ARN a vector index of this name must carry, per `IndexArn`'s pattern.
pub(crate) fn expected_index_arn(bucket: &str, index: &str) -> String {
    format!(
        "arn:aws:s3vectors:{REGION}:{}:bucket/{bucket}/index/{index}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The production router with this one service mounted on a fresh shared engine.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_s3vectors_s3vectors::EmulatedService::new(engine),
    );
    router
}

/// Call one operation and return its HTTP status and JSON body.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_uri(router, "POST", &format!("/{operation}"), Some(body)).await
}

/// Call one operation by the HTTP method and URI its `@http` binding declares,
/// which is how an operation that does not bind `POST /<OperationName>` is
/// reached: the tagging operations all bind `/tags/{resourceArn}`, on `POST`
/// (TagResource), `GET` (ListTagsForResource) and `DELETE` (UntagResource).
pub(crate) async fn call_uri(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let (status, _, parsed) = call_reporting_errortype(router, method, path_and_query, body).await;
    (status, parsed)
}

/// As [`call_uri`], reporting the `x-amzn-errortype` header too: restJson1 resolves
/// an error SHAPE from that header rather than from the body, so a suite that pins
/// which error a client sees reads it as well as the body's `__type`.
pub(crate) async fn call_reporting_errortype(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<&Value>,
) -> (u16, Option<String>, Value) {
    let payload = match body {
        Some(body) => {
            bytes::Bytes::from(serde_json::to_vec(body).expect("the request body serializes"))
        }
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://s3vectors.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, errortype, parsed)
}

/// The wire error code (`__type`) of an error response, or `""`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// The tag surface's shared harness
//
// `TagResource`, `UntagResource` and `ListTagsForResource` all bind
// `/tags/{resourceArn}` (on POST, DELETE and GET), so ONE set of request and
// assertion helpers serves every suite that drives them — `resource_tagging.rs`,
// `resource_tags.rs` and `reserved_tag_keys.rs` all call these rather than
// keeping a harness each.
// ---------------------------------------------------------------------------

/// Percent-encode one value for use as a single URI path segment or query value,
/// as an AWS SDK does for a non-greedy `@httpLabel` or an `@httpQuery` entry:
/// every byte outside RFC 3986's unreserved set is escaped, so an ARN's own `:`
/// and `/` travel as ONE path segment instead of being read as path structure.
///
/// Spelled out here rather than pulled from a crate so the tag suites add no
/// dependency edge to `Cargo.lock`, which every landing fix touches; this is the
/// crate's single copy.
pub(crate) fn encode_label(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            out.push(ch);
        } else {
            out.push('%');
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 0x0f)] as char);
        }
    }
    out
}

/// `POST /tags/{resourceArn}` with a `tags` body — `TagResource`.
pub(crate) async fn tag(router: &MultiServiceRouter, arn: &str, tags: &Value) -> (u16, Value) {
    call_uri(
        router,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        Some(&json!({ "tags": tags })),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is a
/// required `@httpQuery` list, so it travels as one query entry per key.
pub(crate) async fn untag(
    router: &MultiServiceRouter,
    arn: &str,
    tag_keys: &[String],
) -> (u16, Value) {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect::<Vec<String>>()
        .join("&");
    call_uri(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", encode_label(arn)),
        None,
    )
    .await
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`.
pub(crate) async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call_uri(router, "GET", &format!("/tags/{}", encode_label(arn)), None).await
}

/// The tag map `ListTagsForResource` reports for `arn`, which must succeed.
pub(crate) async fn reported_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Value {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({arn}) must succeed, got {status} {body}"
    );
    body.get("tags").cloned().unwrap_or(Value::Null)
}

/// A JSON object from key/value pairs, for comparing a reported tag map whole.
pub(crate) fn tag_object(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// Owned key/value pairs from literals, so a table row can mix literal and
/// generated keys.
pub(crate) fn pairs(literal: &[(&str, &str)]) -> Vec<(String, String)> {
    literal
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Owned tag keys from literals.
pub(crate) fn keys(literal: &[&str]) -> Vec<String> {
    literal.iter().map(|key| (*key).to_string()).collect()
}

/// Call an operation that must succeed, returning its body.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, response) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} {response}"
    );
    response
}

/// The `message` member of a refusal body, whichever casing the protocol rendered.
///
/// Shared by every suite that asserts a refusal's SENTENCE, so the reading of the
/// member is written once and each suite carries only its own expectations.
pub(crate) fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `fieldList` of a refusal body as (path, message) pairs. Shared for the same
/// reason as [`message_of`].
pub(crate) fn field_list(body: &Value) -> Vec<(String, String)> {
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

/// Create a vector bucket and a vector index in it, and return the router.
///
/// Shared with the other suites in this target: one fixture builder for the
/// bucket -> index pair every data-plane and refusal case starts from.
pub(crate) async fn with_index(
    bucket: &str,
    index: &str,
    dimension: u32,
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

/// The `key` of each vector in a `vectors` list, in the order reported.
fn keys_of(body: &Value) -> Vec<String> {
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

/// Floating-point comparison for a computed distance.
fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-5
}

/// The whole create -> use -> read -> delete loop, including that the ARNs create
/// mints are the ARNs every later read reports, that an ARN is accepted wherever the
/// model allows one in place of a name, and that a delete really removes the resource.
#[tokio::test]
async fn vector_bucket_index_and_vector_lifecycle_round_trips_through_state() {
    let router = router();

    // CreateVectorBucket mints the modelled ARN.
    let created = ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "demo-vectors" }),
    )
    .await;
    let bucket_arn = expected_bucket_arn("demo-vectors");
    assert_eq!(
        created["vectorBucketArn"].as_str(),
        Some(bucket_arn.as_str()),
        "CreateVectorBucket must return the modelled bucket ARN, got {created}"
    );

    // Re-creating the same name is a conflict, not a second bucket.
    let (status, conflict) = call(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "demo-vectors" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&conflict).as_str()),
        (409, "ConflictException"),
        "re-creating a vector bucket must conflict, got {status} {conflict}"
    );
    let listed = ok(&router, "ListVectorBuckets", &json!({})).await;
    assert_eq!(
        listed["vectorBuckets"].as_array().map(Vec::len),
        Some(1),
        "the conflicting create must not have added a second bucket, got {listed}"
    );

    // GetVectorBucket reports the stored bucket, by name and by ARN.
    for (case, request) in [
        ("by name", json!({ "vectorBucketName": "demo-vectors" })),
        ("by arn", json!({ "vectorBucketArn": bucket_arn })),
    ] {
        let got = ok(&router, "GetVectorBucket", &request).await;
        assert_eq!(
            got["vectorBucket"]["vectorBucketArn"].as_str(),
            Some(bucket_arn.as_str()),
            "case {case:?}: GetVectorBucket must report the stored ARN, got {got}"
        );
        assert_eq!(
            got["vectorBucket"]["vectorBucketName"].as_str(),
            Some("demo-vectors"),
            "case {case:?}: GetVectorBucket must report the stored name, got {got}"
        );
        assert!(
            got["vectorBucket"]["creationTime"].as_f64().unwrap_or(0.0) > 0.0,
            "case {case:?}: GetVectorBucket must report a creation time, got {got}"
        );
    }

    // CreateIndex mints the modelled index ARN under that bucket.
    let created = ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": "demo-vectors",
            "indexName": "demo-index",
            "dataType": "float32",
            "dimension": 3,
            "distanceMetric": "cosine",
        }),
    )
    .await;
    let index_arn = expected_index_arn("demo-vectors", "demo-index");
    assert_eq!(
        created["indexArn"].as_str(),
        Some(index_arn.as_str()),
        "CreateIndex must return the modelled index ARN, got {created}"
    );

    // GetIndex reports the index exactly as created, resolved by ARN.
    let got = ok(&router, "GetIndex", &json!({ "indexArn": index_arn })).await;
    let index = &got["index"];
    assert_eq!(
        (
            index["vectorBucketName"].as_str(),
            index["indexName"].as_str(),
            index["indexArn"].as_str(),
            index["dataType"].as_str(),
            index["dimension"].as_u64(),
            index["distanceMetric"].as_str(),
        ),
        (
            Some("demo-vectors"),
            Some("demo-index"),
            Some(index_arn.as_str()),
            Some("float32"),
            Some(3),
            Some("cosine"),
        ),
        "GetIndex must report the index as created, got {got}"
    );

    // ListIndexes reports the same ARN create returned.
    let listed = ok(
        &router,
        "ListIndexes",
        &json!({ "vectorBucketName": "demo-vectors" }),
    )
    .await;
    assert_eq!(
        listed["indexes"][0]["indexArn"].as_str(),
        Some(index_arn.as_str()),
        "ListIndexes must report the ARN create minted, got {listed}"
    );

    // PutVectors, then read back.
    ok(
        &router,
        "PutVectors",
        &json!({
            "indexArn": index_arn,
            "vectors": [
                { "key": "v1", "data": { "float32": [1.0, 0.0, 0.0] }, "metadata": { "color": "red" } },
                { "key": "v2", "data": { "float32": [0.0, 1.0, 0.0] } },
            ],
        }),
    )
    .await;

    let got = ok(
        &router,
        "GetVectors",
        &json!({
            "vectorBucketName": "demo-vectors",
            "indexName": "demo-index",
            "keys": ["v1"],
            "returnData": true,
            "returnMetadata": true,
        }),
    )
    .await;
    assert_eq!(
        got["vectors"][0]["data"]["float32"],
        json!([1.0, 0.0, 0.0]),
        "GetVectors must return the stored data, got {got}"
    );
    assert_eq!(
        got["vectors"][0]["metadata"],
        json!({ "color": "red" }),
        "GetVectors must return the stored metadata, got {got}"
    );

    // Both flags default to false, so neither member is populated unasked.
    let got = ok(
        &router,
        "GetVectors",
        &json!({
            "vectorBucketName": "demo-vectors",
            "indexName": "demo-index",
            "keys": ["v1", "absent-key"],
        }),
    )
    .await;
    assert_eq!(
        got["vectors"],
        json!([{ "key": "v1" }]),
        "GetVectors must omit data and metadata unasked, and skip absent keys, got {got}"
    );

    // A put on an existing key replaces that vector rather than adding one.
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": "demo-vectors",
            "indexName": "demo-index",
            "vectors": [{ "key": "v1", "data": { "float32": [0.0, 0.0, 1.0] } }],
        }),
    )
    .await;
    let got = ok(
        &router,
        "GetVectors",
        &json!({
            "vectorBucketName": "demo-vectors",
            "indexName": "demo-index",
            "keys": ["v1"],
            "returnData": true,
            "returnMetadata": true,
        }),
    )
    .await;
    assert_eq!(
        got["vectors"][0]["data"]["float32"],
        json!([0.0, 0.0, 1.0]),
        "a put on an existing key must replace its data, got {got}"
    );
    assert_eq!(
        got["vectors"][0]["metadata"],
        json!({}),
        "a put with no metadata must replace the stored metadata with the default {{}}, got {got}"
    );
    let listed = ok(
        &router,
        "ListVectors",
        &json!({ "indexArn": index_arn }),
    )
    .await;
    assert_eq!(
        keys_of(&listed),
        vec!["v1".to_string(), "v2".to_string()],
        "the upsert must leave exactly the two vectors, got {listed}"
    );

    // A non-empty bucket cannot be deleted.
    let (status, conflict) = call(
        &router,
        "DeleteVectorBucket",
        &json!({ "vectorBucketName": "demo-vectors" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&conflict).as_str()),
        (409, "ConflictException"),
        "deleting a bucket that still holds an index must conflict, got {status} {conflict}"
    );

    // Delete the index, then the bucket; both resources are then gone.
    ok(&router, "DeleteIndex", &json!({ "indexArn": index_arn })).await;
    let (status, missing) = call(&router, "GetIndex", &json!({ "indexArn": index_arn })).await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (404, "NotFoundException"),
        "GetIndex after DeleteIndex must be a 404, got {status} {missing}"
    );
    let (status, missing) = call(
        &router,
        "QueryVectors",
        &json!({
            "indexArn": index_arn,
            "topK": 1,
            "queryVector": { "float32": [1.0, 0.0, 0.0] },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (404, "NotFoundException"),
        "QueryVectors on a deleted index must be a 404, got {status} {missing}"
    );

    ok(
        &router,
        "DeleteVectorBucket",
        &json!({ "vectorBucketName": "demo-vectors" }),
    )
    .await;
    let (status, missing) = call(
        &router,
        "GetVectorBucket",
        &json!({ "vectorBucketName": "demo-vectors" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (404, "NotFoundException"),
        "GetVectorBucket after DeleteVectorBucket must be a 404, got {status} {missing}"
    );
    let (status, missing) = call(
        &router,
        "DeleteVectorBucket",
        &json!({ "vectorBucketName": "demo-vectors" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (404, "NotFoundException"),
        "deleting an absent bucket must be a 404, got {status} {missing}"
    );
}

/// An index is created with the `dataType`, `dimension` and `distanceMetric` the
/// request named, and reports exactly those back — across the published range and
/// every enum value, not just the one the issue cited.
///
/// Rows span `CreateIndex$dimension`'s modelled `range 1..4096` (both bounds and
/// values between) and every value of the model's `DistanceMetric` enum
/// (`cosine`, `euclidean`); `DataType` has the single value `float32`.
#[tokio::test]
async fn created_indexes_round_trip_every_dimension_and_distance_metric() {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "metric-bucket" }),
    )
    .await;

    for dimension in [1_u32, 2, 3, 128, 4095, 4096] {
        for metric in ["cosine", "euclidean"] {
            let case = format!("dimension {dimension} metric {metric}");
            let index = format!("idx-{dimension}-{metric}");
            let created = ok(
                &router,
                "CreateIndex",
                &json!({
                    "vectorBucketName": "metric-bucket",
                    "indexName": index,
                    "dataType": "float32",
                    "dimension": dimension,
                    "distanceMetric": metric,
                }),
            )
            .await;
            assert_eq!(
                created["indexArn"].as_str(),
                Some(expected_index_arn("metric-bucket", &index).as_str()),
                "case {case:?}: CreateIndex must mint the modelled ARN, got {created}"
            );

            let got = ok(
                &router,
                "GetIndex",
                &json!({ "vectorBucketName": "metric-bucket", "indexName": index }),
            )
            .await;
            assert_eq!(
                (
                    got["index"]["dimension"].as_u64(),
                    got["index"]["distanceMetric"].as_str(),
                    got["index"]["dataType"].as_str(),
                ),
                (Some(u64::from(dimension)), Some(metric), Some("float32")),
                "case {case:?}: GetIndex must report the index as created, got {got}"
            );

            // Duplicate index name in the same bucket is a conflict.
            let (status, conflict) = call(
                &router,
                "CreateIndex",
                &json!({
                    "vectorBucketName": "metric-bucket",
                    "indexName": index,
                    "dataType": "float32",
                    "dimension": dimension,
                    "distanceMetric": metric,
                }),
            )
            .await;
            assert_eq!(
                (status, error_code(&conflict).as_str()),
                (409, "ConflictException"),
                "case {case:?}: a duplicate index name must conflict, got {status} {conflict}"
            );
        }
    }

    // A dimension outside the modelled range is refused at the boundary.
    for dimension in [0_i64, 4097] {
        let (status, refused) = call(
            &router,
            "CreateIndex",
            &json!({
                "vectorBucketName": "metric-bucket",
                "indexName": "out-of-range",
                "dataType": "float32",
                "dimension": dimension,
                "distanceMetric": "cosine",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {dimension:?}: a dimension outside 1..4096 must be refused, got {status} {refused}"
        );
    }

    // An index needs a parent bucket that exists.
    let (status, missing) = call(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": "no-such-bucket",
            "indexName": "orphan-index",
            "dataType": "float32",
            "dimension": 3,
            "distanceMetric": "cosine",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (404, "NotFoundException"),
        "CreateIndex under an absent bucket must be a 404, got {status} {missing}"
    );
}

/// QueryVectors ranks the vectors PERSISTED in the index by the distance computed
/// under that index's own `distanceMetric`, ascending, truncated to `topK`, and
/// echoes the metric from the stored index rather than from the request.
///
/// `cosine` is `1 - cosine similarity` and `euclidean` is the L2 distance, so the
/// expected distances below are arithmetic on the stored vectors, not recorded values.
#[tokio::test]
async fn query_vectors_computes_distances_from_stored_vectors() {
    // Stored vectors, shared by both metrics.
    let stored = json!([
        { "key": "ex", "data": { "float32": [1.0, 0.0, 0.0] } },
        { "key": "ey", "data": { "float32": [0.0, 1.0, 0.0] } },
        { "key": "xy", "data": { "float32": [1.0, 1.0, 0.0] } },
    ]);
    let half_sqrt2 = 1.0 - 1.0 / 2.0_f64.sqrt();

    // (metric, query vector, topK, expected (key, distance) in order)
    // One row of the distance table: the index's metric, the query vector, `topK`,
    // and the (key, distance) pairs the query must report, in order.
    type QueryCase = (&'static str, Vec<f64>, u32, Vec<(&'static str, f64)>);
    let cases: Vec<QueryCase> = vec![
        (
            "cosine",
            vec![1.0, 0.0, 0.0],
            3,
            vec![("ex", 0.0), ("xy", half_sqrt2), ("ey", 1.0)],
        ),
        (
            "cosine",
            vec![1.0, 1.0, 0.0],
            3,
            vec![("xy", 0.0), ("ex", half_sqrt2), ("ey", half_sqrt2)],
        ),
        ("cosine", vec![1.0, 0.0, 0.0], 1, vec![("ex", 0.0)]),
        (
            "euclidean",
            vec![0.0, 0.0, 0.0],
            3,
            vec![("ex", 1.0), ("ey", 1.0), ("xy", 2.0_f64.sqrt())],
        ),
        (
            "euclidean",
            vec![1.0, 1.0, 0.0],
            3,
            vec![("xy", 0.0), ("ex", 1.0), ("ey", 1.0)],
        ),
        ("euclidean", vec![1.0, 1.0, 0.0], 2, vec![("xy", 0.0), ("ex", 1.0)]),
    ];

    for (metric, query, top_k, expected) in cases {
        let case = format!("metric {metric} query {query:?} topK {top_k}");
        let router = with_index("query-bucket", "query-index", 3, metric).await;
        ok(
            &router,
            "PutVectors",
            &json!({
                "vectorBucketName": "query-bucket",
                "indexName": "query-index",
                "vectors": stored,
            }),
        )
        .await;

        let got = ok(
            &router,
            "QueryVectors",
            &json!({
                "vectorBucketName": "query-bucket",
                "indexName": "query-index",
                "topK": top_k,
                "queryVector": { "float32": query },
                "returnDistance": true,
            }),
        )
        .await;

        assert_eq!(
            got["distanceMetric"].as_str(),
            Some(metric),
            "case {case:?}: the metric must be echoed from the stored index, got {got}"
        );
        assert_eq!(
            keys_of(&got),
            expected
                .iter()
                .map(|(key, _)| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: nearest first, truncated to topK, got {got}"
        );
        for (position, (key, distance)) in expected.iter().enumerate() {
            let actual = got["vectors"][position]["distance"].as_f64();
            assert!(
                actual.is_some_and(|actual| close(actual, *distance)),
                "case {case:?}: {key} must be at distance {distance}, got {actual:?} in {got}"
            );
        }

        // `returnDistance` and `returnMetadata` default to false.
        let bare = ok(
            &router,
            "QueryVectors",
            &json!({
                "vectorBucketName": "query-bucket",
                "indexName": "query-index",
                "topK": top_k,
                "queryVector": { "float32": query },
            }),
        )
        .await;
        assert!(
            bare["vectors"]
                .as_array()
                .is_some_and(|vectors| vectors.iter().all(|vector| vector.get("distance").is_none()
                    && vector.get("metadata").is_none())),
            "case {case:?}: distance and metadata must be absent unasked, got {bare}"
        );
    }
}

/// A vector's `float32` data must have exactly the index's `dimension` values, must
/// contain only finite values, and must not be all zeros when the index's metric is
/// `cosine` — the three rules `PutInputVector$data` documents. The same rule governs
/// `QueryVectors$queryVector`, so each row is checked on both members.
#[tokio::test]
async fn vector_data_is_checked_against_the_index_it_is_written_to_or_queried_with() {
    // (case, data, accepted under cosine, accepted under euclidean)
    let cases: Vec<(&str, Value, bool, bool)> = vec![
        ("matching dimension", json!([1.0, 2.0, 3.0]), true, true),
        ("negative values", json!([-1.0, -2.0, 0.5]), true, true),
        ("one value short", json!([1.0, 2.0]), false, false),
        ("one value long", json!([1.0, 2.0, 3.0, 4.0]), false, false),
        ("empty", json!([]), false, false),
        // A zero vector has no direction, so cosine refuses it; L2 does not.
        ("all zeros", json!([0.0, 0.0, 0.0]), false, true),
        ("not a number", json!(["NaN", 1.0, 2.0]), false, false),
        ("infinity", json!(["Infinity", 1.0, 2.0]), false, false),
    ];

    for metric in ["cosine", "euclidean"] {
        for (label, data, cosine_ok, euclidean_ok) in &cases {
            let accepted = if metric == "cosine" {
                *cosine_ok
            } else {
                *euclidean_ok
            };
            let case = format!("metric {metric} data {label}");
            let router = with_index("check-bucket", "check-index", 3, metric).await;

            let (status, body) = call(
                &router,
                "PutVectors",
                &json!({
                    "vectorBucketName": "check-bucket",
                    "indexName": "check-index",
                    "vectors": [{ "key": "candidate", "data": { "float32": data } }],
                }),
            )
            .await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: PutVectors must accept it, got {status} {body}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: PutVectors must refuse it, got {status} {body}"
                );
            }

            let (status, body) = call(
                &router,
                "QueryVectors",
                &json!({
                    "vectorBucketName": "check-bucket",
                    "indexName": "check-index",
                    "topK": 1,
                    "queryVector": { "float32": data },
                }),
            )
            .await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: QueryVectors must accept it, got {status} {body}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: QueryVectors must refuse it, got {status} {body}"
                );
            }
        }
    }

    // A refused batch writes nothing: the good vector in it is not stored either.
    let router = with_index("atomic-bucket", "atomic-index", 3, "cosine").await;
    let (status, body) = call(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": "atomic-bucket",
            "indexName": "atomic-index",
            "vectors": [
                { "key": "good", "data": { "float32": [1.0, 2.0, 3.0] } },
                { "key": "bad", "data": { "float32": [1.0, 2.0] } },
            ],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a batch with one bad vector must be refused, got {status} {body}"
    );
    let listed = ok(
        &router,
        "ListVectors",
        &json!({ "vectorBucketName": "atomic-bucket", "indexName": "atomic-index" }),
    )
    .await;
    assert_eq!(
        keys_of(&listed),
        Vec::<String>::new(),
        "a refused batch must write nothing, got {listed}"
    );
}

/// A vector bucket or vector index name must be 3 to 63 characters of lowercase
/// letters, digits, hyphens and dots, beginning and ending with a letter or digit —
/// the form the model's `VectorBucketArn` and `IndexArn` patterns encode. A name
/// outside it is refused with 400, never stored under an ARN that cannot be formed.
#[tokio::test]
async fn resource_names_outside_the_modelled_arn_form_are_refused() {
    // (case, name, accepted). The length bounds are the pattern's own 3..63, with
    // one value past each bound.
    let cases: Vec<(&str, String, bool)> = vec![
        ("minimum length", "abc".to_string(), true),
        ("one below the minimum", "ab".to_string(), false),
        ("maximum length", "a".repeat(63), true),
        ("one above the maximum", "a".repeat(64), false),
        ("digits, dots and hyphens", "a1.b-c2".to_string(), true),
        ("leading hyphen", "-abc".to_string(), false),
        ("trailing hyphen", "abc-".to_string(), false),
        ("leading dot", ".abc".to_string(), false),
        ("uppercase", "Abc".to_string(), false),
        ("underscore", "a_bc".to_string(), false),
        ("space", "a bc".to_string(), false),
    ];

    for (label, name, accepted) in &cases {
        let router = router();
        let (status, body) = call(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": name }),
        )
        .await;
        if *accepted {
            assert_eq!(
                (status, body["vectorBucketArn"].as_str()),
                (200, Some(expected_bucket_arn(name).as_str())),
                "case {label:?}: {name:?} must be accepted, got {status} {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {label:?}: bucket name {name:?} must be refused, got {status} {body}"
            );
        }

        // The same rule governs an index name.
        ok(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": "name-rules" }),
        )
        .await;
        let (status, body) = call(
            &router,
            "CreateIndex",
            &json!({
                "vectorBucketName": "name-rules",
                "indexName": name,
                "dataType": "float32",
                "dimension": 2,
                "distanceMetric": "cosine",
            }),
        )
        .await;
        if *accepted {
            assert_eq!(
                (status, body["indexArn"].as_str()),
                (200, Some(expected_index_arn("name-rules", name).as_str())),
                "case {label:?}: index name {name:?} must be accepted, got {status} {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {label:?}: index name {name:?} must be refused, got {status} {body}"
            );
        }
    }
}

/// A QueryVectors `filter` selects the vectors whose metadata satisfies it, using the
/// documented operator set; an operator this emulator does not implement is refused
/// rather than ignored, so a filter never silently returns unfiltered matches.
#[tokio::test]
async fn metadata_filters_select_the_matching_vectors() {
    let router = with_index("filter-bucket", "filter-index", 2, "euclidean").await;
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": "filter-bucket",
            "indexName": "filter-index",
            "vectors": [
                { "key": "a", "data": { "float32": [0.0, 0.0] },
                  "metadata": { "color": "red", "rank": 1, "tags": ["x", "y"] } },
                { "key": "b", "data": { "float32": [0.0, 1.0] },
                  "metadata": { "color": "blue", "rank": 2 } },
                { "key": "c", "data": { "float32": [0.0, 2.0] },
                  "metadata": { "color": "green", "rank": 3, "tags": ["y"] } },
                { "key": "d", "data": { "float32": [0.0, 3.0] } },
            ],
        }),
    )
    .await;

    // (case, filter, expected keys in distance order)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filter", Value::Null, vec!["a", "b", "c", "d"]),
        ("implicit equality", json!({ "color": "red" }), vec!["a"]),
        ("$eq", json!({ "color": { "$eq": "blue" } }), vec!["b"]),
        (
            "$ne",
            json!({ "color": { "$ne": "red" } }),
            vec!["b", "c", "d"],
        ),
        ("$gt", json!({ "rank": { "$gt": 2 } }), vec!["c"]),
        ("$gte", json!({ "rank": { "$gte": 2 } }), vec!["b", "c"]),
        ("$lt", json!({ "rank": { "$lt": 2 } }), vec!["a"]),
        ("$lte", json!({ "rank": { "$lte": 2 } }), vec!["a", "b"]),
        (
            "$in",
            json!({ "color": { "$in": ["red", "green"] } }),
            vec!["a", "c"],
        ),
        (
            "$nin",
            json!({ "color": { "$nin": ["red", "green"] } }),
            vec!["b", "d"],
        ),
        (
            "$exists true",
            json!({ "color": { "$exists": true } }),
            vec!["a", "b", "c"],
        ),
        (
            "$exists false",
            json!({ "color": { "$exists": false } }),
            vec!["d"],
        ),
        (
            "$and",
            json!({ "$and": [{ "color": { "$ne": "red" } }, { "rank": { "$lt": 3 } }] }),
            vec!["b"],
        ),
        (
            "$or",
            json!({ "$or": [{ "color": "red" }, { "rank": 3 }] }),
            vec!["a", "c"],
        ),
        (
            "implicit and of two keys",
            json!({ "color": "green", "rank": 3 }),
            vec!["c"],
        ),
        (
            "implicit and with no match",
            json!({ "color": "green", "rank": 1 }),
            vec![],
        ),
        (
            "a list-valued entry matches any element",
            json!({ "tags": "y" }),
            vec!["a", "c"],
        ),
        (
            "number kinds compare by value",
            json!({ "rank": 1.0 }),
            vec!["a"],
        ),
    ];

    for (label, filter, expected) in cases {
        let mut request = json!({
            "vectorBucketName": "filter-bucket",
            "indexName": "filter-index",
            "topK": 10,
            "queryVector": { "float32": [0.0, 0.0] },
        });
        if !filter.is_null() {
            request["filter"] = filter.clone();
        }
        let got = ok(&router, "QueryVectors", &request).await;
        assert_eq!(
            keys_of(&got),
            expected
                .iter()
                .map(|key| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {label:?}: filter {filter} must select {expected:?}, got {got}"
        );
    }

    // An operator the emulator does not implement is refused, not ignored — and the
    // refusal does not depend on what the index happens to hold, so it is the same
    // against an index with no vectors at all.
    let empty = with_index("empty-bucket", "empty-index", 2, "euclidean").await;
    for operator in ["$regex", "$like", "$near"] {
        for (label, target, bucket, index) in [
            ("populated index", &router, "filter-bucket", "filter-index"),
            ("empty index", &empty, "empty-bucket", "empty-index"),
        ] {
            let (status, body) = call(
                target,
                "QueryVectors",
                &json!({
                    "vectorBucketName": bucket,
                    "indexName": index,
                    "topK": 10,
                    "queryVector": { "float32": [0.0, 0.0] },
                    "filter": { "color": { operator: "red" } },
                }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {operator:?} on an {label}: an unsupported filter operator must be \
                 refused, got {status} {body}"
            );
        }
    }

    // So are a filter that is not an object and an operand of the wrong kind.
    for (label, filter) in [
        ("a non-object filter", json!("color = red")),
        ("$in without an array", json!({ "color": { "$in": "red" } })),
        ("$exists without a boolean", json!({ "color": { "$exists": "yes" } })),
    ] {
        let (status, body) = call(
            &empty,
            "QueryVectors",
            &json!({
                "vectorBucketName": "empty-bucket",
                "indexName": "empty-index",
                "topK": 10,
                "queryVector": { "float32": [0.0, 0.0] },
                "filter": filter,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {label:?}: must be refused, got {status} {body}"
        );
    }

    // A non-filterable metadata key cannot be filtered on, but can still be read.
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": "filter-bucket",
            "indexName": "non-filterable",
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "euclidean",
            "metadataConfiguration": { "nonFilterableMetadataKeys": ["note"] },
        }),
    )
    .await;
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": "filter-bucket",
            "indexName": "non-filterable",
            "vectors": [{ "key": "a", "data": { "float32": [0.0, 0.0] },
                          "metadata": { "note": "kept", "color": "red" } }],
        }),
    )
    .await;
    let got = ok(
        &router,
        "GetVectors",
        &json!({
            "vectorBucketName": "filter-bucket",
            "indexName": "non-filterable",
            "keys": ["a"],
            "returnMetadata": true,
        }),
    )
    .await;
    assert_eq!(
        got["vectors"][0]["metadata"]["note"].as_str(),
        Some("kept"),
        "a non-filterable metadata key must still be readable, got {got}"
    );
    let (status, body) = call(
        &router,
        "QueryVectors",
        &json!({
            "vectorBucketName": "filter-bucket",
            "indexName": "non-filterable",
            "topK": 10,
            "queryVector": { "float32": [0.0, 0.0] },
            "filter": { "note": "kept" },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "filtering on a non-filterable metadata key must be refused, got {status} {body}"
    );
    let got = ok(
        &router,
        "GetIndex",
        &json!({ "vectorBucketName": "filter-bucket", "indexName": "non-filterable" }),
    )
    .await;
    assert_eq!(
        got["index"]["metadataConfiguration"]["nonFilterableMetadataKeys"],
        json!(["note"]),
        "GetIndex must report the stored metadata configuration, got {got}"
    );
}

/// A `filter` deeper or larger than the bounds the evaluator declares is REFUSED
/// with 400 rather than evaluated: the walk over a caller-supplied document is
/// recursive, and an unbounded walk would exhaust the thread stack, which aborts
/// the whole emulator process instead of failing the one request.
#[tokio::test]
async fn metadata_filters_past_the_evaluation_bounds_are_refused() {
    let router = with_index("bound-bucket", "bound-index", 2, "euclidean").await;
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": "bound-bucket",
            "indexName": "bound-index",
            "vectors": [{ "key": "a", "data": { "float32": [0.0, 0.0] },
                          "metadata": { "color": "red" } }],
        }),
    )
    .await;

    /// `{"$and": [ ... ]}` wrapped `depth` times around one equality clause.
    fn nested_and(depth: usize) -> Value {
        let mut filter = json!({ "color": "red" });
        for _ in 0..depth {
            filter = json!({ "$and": [filter] });
        }
        filter
    }

    // (case, filter, accepted)
    let cases: Vec<(String, Value, bool)> = vec![
        ("nesting 1".to_string(), nested_and(1), true),
        ("nesting 8".to_string(), nested_and(8), true),
        ("nesting far past the depth bound".to_string(), nested_and(64), false),
        (
            "an $or wider than the node bound".to_string(),
            json!({
                "$or": (0..5000)
                    .map(|n| json!({ "color": format!("shade-{n}") }))
                    .collect::<Vec<Value>>(),
            }),
            false,
        ),
        (
            "an $in list within the node bound".to_string(),
            json!({ "color": { "$in": (0..100)
                .map(|n| json!(format!("shade-{n}")))
                .chain([json!("red")])
                .collect::<Vec<Value>>() } }),
            true,
        ),
        (
            // Every element of an operand array counts toward the bound, so this
            // is refused for the SAME reason whatever the index holds.
            "an $in list wider than the node bound".to_string(),
            json!({ "color": { "$in": (0..5000)
                .map(|n| json!(format!("shade-{n}")))
                .collect::<Vec<Value>>() } }),
            false,
        ),
    ];

    // Each case is checked against an index that holds a matching vector AND
    // against one that holds none: the answer must not depend on stored data.
    let empty = with_index("bound-bucket-empty", "bound-index-empty", 2, "euclidean").await;
    for (label, filter, accepted) in cases {
        for (holds, target, bucket, index, expected) in [
            (
                "a matching vector",
                &router,
                "bound-bucket",
                "bound-index",
                vec!["a".to_string()],
            ),
            (
                "no vectors",
                &empty,
                "bound-bucket-empty",
                "bound-index-empty",
                Vec::new(),
            ),
        ] {
            let (status, body) = call(
                target,
                "QueryVectors",
                &json!({
                    "vectorBucketName": bucket,
                    "indexName": index,
                    "topK": 10,
                    "queryVector": { "float32": [0.0, 0.0] },
                    "filter": filter,
                }),
            )
            .await;
            if accepted {
                assert_eq!(
                    (status, keys_of(&body)),
                    (200, expected),
                    "case {label:?} on an index with {holds}: a filter within the bounds \
                     must be evaluated, got {status} {body}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {label:?} on an index with {holds}: a filter past the bounds must \
                     be refused, got {status} {body}"
                );
            }
        }
    }
}

/// What the emulator stores is bounded: its store is this process's memory, so a
/// create or put loop must not grow it without limit. A request that would cross a
/// ceiling is refused with the modelled `ServiceQuotaExceededException` (402), and
/// an oversized metadata document with `ValidationException` (400).
#[tokio::test]
async fn quotas_bound_what_the_emulator_stores() {
    use service_com_amazonaws_s3vectors_s3vectors::state::{
        MAX_INDEXES_PER_BUCKET, MAX_TAGS_PER_RESOURCE, MAX_VECTORS_PER_INDEX, MAX_VECTOR_BUCKETS,
    };

    // More tags than a resource carries.
    let buckets = router();
    let many_tags: Value = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|n| (format!("key-{n}"), Value::from("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let (status, body) = call(
        &buckets,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "tagged-bucket", "tags": many_tags }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "more than {MAX_TAGS_PER_RESOURCE} tags must be refused, got {status} {body}"
    );

    // One vector bucket past the ceiling.
    for n in 0..MAX_VECTOR_BUCKETS {
        ok(
            &buckets,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": format!("bucket-{n:04}") }),
        )
        .await;
    }
    let (status, body) = call(
        &buckets,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "one-too-many" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "more than {MAX_VECTOR_BUCKETS} vector buckets must be refused, got {status} {body}"
    );

    // One vector index past the ceiling, in a bucket of its own.
    let indexes = router();
    ok(
        &indexes,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": "quota-bucket" }),
    )
    .await;
    for n in 0..MAX_INDEXES_PER_BUCKET {
        ok(
            &indexes,
            "CreateIndex",
            &json!({
                "vectorBucketName": "quota-bucket",
                "indexName": format!("index-{n:04}"),
                "dataType": "float32",
                "dimension": 1,
                "distanceMetric": "euclidean",
            }),
        )
        .await;
    }
    let (status, body) = call(
        &indexes,
        "CreateIndex",
        &json!({
            "vectorBucketName": "quota-bucket",
            "indexName": "one-too-many",
            "dataType": "float32",
            "dimension": 1,
            "distanceMetric": "euclidean",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "more than {MAX_INDEXES_PER_BUCKET} indexes per bucket must be refused, got {status} {body}"
    );

    // One vector past the ceiling. `PutVectors$vectors` holds at most 500, so the
    // index is filled in batches and then pushed one over.
    let router = with_index("fill-bucket", "fill-index", 1, "euclidean").await;
    let batch = 500;
    for start in (0..MAX_VECTORS_PER_INDEX).step_by(batch) {
        ok(
            &router,
            "PutVectors",
            &json!({
                "vectorBucketName": "fill-bucket",
                "indexName": "fill-index",
                "vectors": (start..start + batch)
                    .map(|n| json!({ "key": format!("v{n:06}"), "data": { "float32": [1.0] } }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
    }
    // An upsert of a key already stored stays within the ceiling.
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": "fill-bucket",
            "indexName": "fill-index",
            "vectors": [{ "key": "v000000", "data": { "float32": [2.0] } }],
        }),
    )
    .await;
    let (status, body) = call(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": "fill-bucket",
            "indexName": "fill-index",
            "vectors": [{ "key": "one-too-many", "data": { "float32": [1.0] } }],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "more than {MAX_VECTORS_PER_INDEX} vectors per index must be refused, got {status} {body}"
    );

    // A metadata document larger than the emulator stores for one vector.
    let router = with_index("meta-bucket", "meta-index", 1, "euclidean").await;
    for (label, elements, accepted) in [("within the bound", 100, true), ("past the bound", 2000, false)] {
        let metadata = json!({ "values": (0..elements).collect::<Vec<u32>>() });
        let (status, body) = call(
            &router,
            "PutVectors",
            &json!({
                "vectorBucketName": "meta-bucket",
                "indexName": "meta-index",
                "vectors": [{ "key": "candidate", "data": { "float32": [1.0] },
                              "metadata": metadata }],
            }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: {elements} metadata elements must be stored, got {status} {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {label:?}: {elements} metadata elements must be refused, got {status} {body}"
            );
        }
    }
}

/// A list operation reports one page at a time, in stored order, and its `nextToken`
/// walks the whole collection exactly once. ListVectors additionally partitions the
/// index into `segmentCount` disjoint segments that together cover it.
#[tokio::test]
async fn list_operations_page_and_partition_over_stored_order() {
    let router = with_index("page-bucket", "page-index", 2, "euclidean").await;
    let keys: Vec<String> = (0..10).map(|n| format!("v{n:02}")).collect();
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": "page-bucket",
            "indexName": "page-index",
            "vectors": keys
                .iter()
                .map(|key| json!({ "key": key, "data": { "float32": [1.0, 0.0] } }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;

    // Paging with several page sizes walks the index once, in key order.
    for page_size in [1_u32, 3, 4, 10, 25] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=keys.len() {
            let mut request = json!({
                "vectorBucketName": "page-bucket",
                "indexName": "page-index",
                "maxResults": page_size,
            });
            if let Some(token) = token.as_ref() {
                request["nextToken"] = json!(token);
            }
            let got = ok(&router, "ListVectors", &request).await;
            let page = keys_of(&got);
            assert!(
                page.len() <= page_size as usize,
                "case {case:?}: a page must not exceed maxResults, got {got}"
            );
            seen.extend(page);
            token = got["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, keys,
            "case {case:?}: paging must walk every vector once in stored order, got {seen:?}"
        );
        assert!(
            token.is_none(),
            "case {case:?}: the last page must carry no nextToken"
        );
    }

    // The segments partition the index: disjoint, and together the whole index.
    for segment_count in [1_u32, 2, 3, 4, 16] {
        let case = format!("segmentCount {segment_count}");
        let mut seen: Vec<String> = Vec::new();
        for segment_index in 0..segment_count {
            let got = ok(
                &router,
                "ListVectors",
                &json!({
                    "vectorBucketName": "page-bucket",
                    "indexName": "page-index",
                    "segmentCount": segment_count,
                    "segmentIndex": segment_index,
                }),
            )
            .await;
            seen.extend(keys_of(&got));
        }
        seen.sort();
        assert_eq!(
            seen, keys,
            "case {case:?}: the segments must partition the index, got {seen:?}"
        );
    }

    // `segmentIndex` must be less than `segmentCount`.
    let (status, body) = call(
        &router,
        "ListVectors",
        &json!({
            "vectorBucketName": "page-bucket",
            "indexName": "page-index",
            "segmentCount": 2,
            "segmentIndex": 2,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "segmentIndex must be below segmentCount, got {status} {body}"
    );

    // ListVectorBuckets and ListIndexes page by prefix and cursor over stored order.
}

/// The bucket- and index-level list operations honour `prefix`, `maxResults` and the
/// `nextToken` cursor over the same stored order.
#[tokio::test]
async fn list_vector_buckets_and_indexes_page_by_prefix_and_cursor() {
    let router = router();
    for bucket in ["alpha-one", "alpha-two", "beta-one"] {
        ok(
            &router,
            "CreateVectorBucket",
            &json!({ "vectorBucketName": bucket }),
        )
        .await;
    }
    let got = ok(&router, "ListVectorBuckets", &json!({ "prefix": "alpha" })).await;
    assert_eq!(
        got["vectorBuckets"]
            .as_array()
            .map(|buckets| buckets
                .iter()
                .map(|bucket| bucket["vectorBucketName"].as_str().unwrap_or_default())
                .collect::<Vec<&str>>()),
        Some(vec!["alpha-one", "alpha-two"]),
        "ListVectorBuckets must honour prefix, got {got}"
    );
    let first = ok(
        &router,
        "ListVectorBuckets",
        &json!({ "maxResults": 2 }),
    )
    .await;
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        first["vectorBuckets"].as_array().map(Vec::len),
        Some(2),
        "the first page must hold maxResults buckets, got {first}"
    );
    let second = ok(
        &router,
        "ListVectorBuckets",
        &json!({ "maxResults": 2, "nextToken": token }),
    )
    .await;
    assert_eq!(
        second["vectorBuckets"]
            .as_array()
            .map(|buckets| buckets
                .iter()
                .map(|bucket| bucket["vectorBucketName"].as_str().unwrap_or_default())
                .collect::<Vec<&str>>()),
        Some(vec!["beta-one"]),
        "the cursor must continue after the first page, got {second}"
    );
    assert!(
        second["nextToken"].is_null(),
        "the last page must carry no nextToken, got {second}"
    );

    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": "alpha-one",
            "indexName": "idx-one",
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "cosine",
        }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": "alpha-one",
            "indexName": "other-index",
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "cosine",
        }),
    )
    .await;
    let got = ok(
        &router,
        "ListIndexes",
        &json!({ "vectorBucketName": "alpha-one", "prefix": "idx" }),
    )
    .await;
    assert_eq!(
        got["indexes"]
            .as_array()
            .map(|indexes| indexes
                .iter()
                .map(|index| index["indexName"].as_str().unwrap_or_default())
                .collect::<Vec<&str>>()),
        Some(vec!["idx-one"]),
        "ListIndexes must honour prefix, got {got}"
    );
    let (status, missing) = call(
        &router,
        "ListIndexes",
        &json!({ "vectorBucketName": "no-such-bucket" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&missing).as_str()),
        (404, "NotFoundException"),
        "ListIndexes on an absent bucket must be a 404, got {status} {missing}"
    );
}

/// A request that names neither accepted form of its target, or BOTH of them
/// (whether or not they agree), is refused; an ARN that is not an ARN of the right
/// resource is too.
#[tokio::test]
async fn a_target_must_be_named_by_exactly_one_accepted_form() {
    let router = with_index("target-bucket", "target-index", 2, "cosine").await;
    let other = expected_index_arn("target-bucket", "target-index");

    // (case, operation, request)
    let cases: Vec<(&str, &str, Value)> = vec![
        ("no bucket named", "GetVectorBucket", json!({})),
        (
            "bucket name and ARN disagree",
            "GetVectorBucket",
            json!({ "vectorBucketName": "target-bucket",
                    "vectorBucketArn": expected_bucket_arn("other-bucket") }),
        ),
        (
            "bucket name and ARN agree",
            "GetVectorBucket",
            json!({ "vectorBucketName": "target-bucket",
                    "vectorBucketArn": expected_bucket_arn("target-bucket") }),
        ),
        (
            "index name pair and ARN agree",
            "GetIndex",
            json!({ "vectorBucketName": "target-bucket", "indexName": "target-index",
                    "indexArn": other }),
        ),
        (
            "an index ARN where a bucket ARN belongs",
            "GetVectorBucket",
            json!({ "vectorBucketArn": other }),
        ),
        ("no index named", "GetIndex", json!({})),
        (
            "bucket without index name",
            "GetIndex",
            json!({ "vectorBucketName": "target-bucket" }),
        ),
        (
            "index name and ARN disagree",
            "GetIndex",
            json!({ "vectorBucketName": "target-bucket", "indexName": "other-index",
                    "indexArn": other }),
        ),
        (
            "a bucket ARN where an index ARN belongs",
            "GetIndex",
            json!({ "indexArn": expected_bucket_arn("target-bucket") }),
        ),
        (
            "an ARN of another service",
            "GetVectorBucket",
            json!({ "vectorBucketArn":
                    format!("arn:aws:s3:{REGION}:{}:bucket/target-bucket",
                            engine::EMULATOR_ACCOUNT_ID) }),
        ),
    ];

    for (label, operation, request) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {label:?}: {operation} {request} must be refused, got {status} {body}"
        );
    }

    // Exactly one complete form resolves the target.
    let got = ok(&router, "GetIndex", &json!({ "indexArn": other })).await;
    assert_eq!(
        got["index"]["indexArn"].as_str(),
        Some(other.as_str()),
        "an ARN-only request must resolve, got {got}"
    );
    let got = ok(
        &router,
        "GetIndex",
        &json!({ "vectorBucketName": "target-bucket", "indexName": "target-index" }),
    )
    .await;
    assert_eq!(
        got["index"]["indexArn"].as_str(),
        Some(other.as_str()),
        "a name-pair request must resolve to the same index, got {got}"
    );
}

/// The operations this slice does not implement still answer HTTP 501, never an
/// empty 200 — a client, and any check that only asserts success, must be able to
/// tell "not written yet" from a real implementation.
#[tokio::test]
async fn the_operations_outside_this_slice_still_answer_501() {
    let router = with_index("stub-bucket", "stub-index", 2, "cosine").await;

    let cases: Vec<(&str, Value)> = vec![(
        "UpdateIndexMode",
        json!({ "vectorBucketName": "stub-bucket", "indexName": "stub-index",
                "indexMode": "ENHANCED" }),
    )];

    for (operation, request) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (501, "NotImplemented"),
            "case {operation:?}: an unimplemented operation must answer 501, got {status} {body}"
        );
    }
}

/// The keys every deletion case starts from, in the order a read reports them.
const SEEDED_KEYS: [&str; 5] = ["v1", "v2", "v3", "v4", "v5"];

/// A fresh 2-dimensional cosine index holding one vector per [`SEEDED_KEYS`] entry,
/// and the router serving it.
async fn with_seeded_index(bucket: &str, index: &str) -> MultiServiceRouter {
    let router = with_index(bucket, index, 2, "cosine").await;
    let vectors: Vec<Value> = SEEDED_KEYS
        .iter()
        .enumerate()
        .map(|(slot, key)| {
            json!({
                "key": key,
                "data": { "float32": [1.0 + slot as f64, 1.0] },
            })
        })
        .collect();
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": bucket,
            "indexName": index,
            "vectors": vectors,
        }),
    )
    .await;
    router
}

/// DeleteVectors removes exactly the vectors its `keys` name from the named index and
/// nothing else, and a key the index does not hold is not a failure — the operation is
/// `@idempotent`, so the request asks for a state a missing key is already in.
///
/// Rows span what the rule generalises over: how many keys one request names (one, a
/// few, every key in the index, and a batch at `DeleteVectors$keys`'s modelled maximum
/// of 500), whether a named key is present, absent or repeated, and a mixture of the
/// two.
#[tokio::test]
async fn delete_vectors_removes_exactly_the_named_keys() {
    // A batch at the top of `DeleteVectors$keys`'s modelled `len 1..500`, of which
    // only two keys are in the index.
    let max_batch: Vec<String> = ["v1".to_string(), "v5".to_string()]
        .into_iter()
        .chain((0..498).map(|n| format!("absent-{n}")))
        .collect();
    assert_eq!(
        max_batch.len(),
        500,
        "the maximum-batch row must name 500 keys, got {}",
        max_batch.len()
    );

    // (case, the keys the request deletes, the keys that must remain)
    let cases: Vec<(&str, Vec<String>, Vec<&str>)> = vec![
        (
            "one key of several",
            vec!["v2".to_string()],
            vec!["v1", "v3", "v4", "v5"],
        ),
        (
            "two keys in one request",
            vec!["v1".to_string(), "v4".to_string()],
            vec!["v2", "v3", "v5"],
        ),
        (
            "every key in the index",
            SEEDED_KEYS.iter().map(|key| key.to_string()).collect(),
            vec![],
        ),
        (
            "a key the index never held",
            vec!["absent-key".to_string()],
            vec!["v1", "v2", "v3", "v4", "v5"],
        ),
        (
            "a present and an absent key together",
            vec!["v3".to_string(), "absent-key".to_string()],
            vec!["v1", "v2", "v4", "v5"],
        ),
        (
            "the same key twice in one batch",
            vec!["v5".to_string(), "v5".to_string()],
            vec!["v1", "v2", "v3", "v4"],
        ),
        ("a batch of 500 keys", max_batch, vec!["v2", "v3", "v4"]),
    ];

    for (case, keys, remaining) in cases {
        let router = with_seeded_index("delete-bucket", "delete-index").await;
        let expected: Vec<String> = remaining.iter().map(|key| (*key).to_string()).collect();

        let (status, body) = call(
            &router,
            "DeleteVectors",
            &json!({
                "vectorBucketName": "delete-bucket",
                "indexName": "delete-index",
                "keys": keys,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteVectors must succeed, got {status} {body}"
        );

        let listed = ok(
            &router,
            "ListVectors",
            &json!({ "vectorBucketName": "delete-bucket", "indexName": "delete-index" }),
        )
        .await;
        assert_eq!(
            keys_of(&listed),
            expected,
            "case {case:?}: ListVectors must report exactly the undeleted keys, got {listed}"
        );

        let got = ok(
            &router,
            "GetVectors",
            &json!({
                "vectorBucketName": "delete-bucket",
                "indexName": "delete-index",
                "keys": SEEDED_KEYS,
            }),
        )
        .await;
        assert_eq!(
            keys_of(&got),
            expected,
            "case {case:?}: GetVectors must not report a deleted key, got {got}"
        );
    }
}

/// A deleted vector is gone from every read of the index — including the nearest-
/// neighbour search — a repeated delete of the same key answers the same success, and
/// the key is free to be used again by a later put. The index ARN is accepted in place
/// of the bucket-and-index pair, as the model allows for this operation.
#[tokio::test]
async fn a_deleted_vector_leaves_every_read_and_its_key_is_reusable() {
    let router = with_seeded_index("reuse-bucket", "reuse-index").await;
    let index_arn = expected_index_arn("reuse-bucket", "reuse-index");
    let query = json!({
        "indexArn": index_arn,
        "topK": 5,
        "queryVector": { "float32": [1.0, 1.0] },
        "returnDistance": true,
    });

    let before = ok(&router, "QueryVectors", &query).await;
    assert_eq!(
        keys_of(&before).first().map(String::as_str),
        Some("v1"),
        "the nearest vector before the delete must be v1, got {before}"
    );

    // Deleted through the index ARN form.
    ok(
        &router,
        "DeleteVectors",
        &json!({ "indexArn": index_arn, "keys": ["v1"] }),
    )
    .await;
    let after = ok(&router, "QueryVectors", &query).await;
    assert!(
        !keys_of(&after).contains(&"v1".to_string()),
        "a deleted vector must not be a query result, got {after}"
    );
    assert_eq!(
        keys_of(&after).len(),
        4,
        "the other four vectors must still be found, got {after}"
    );

    // Repeating the delete asks for a state the index is already in.
    let (status, body) = call(
        &router,
        "DeleteVectors",
        &json!({ "indexArn": index_arn, "keys": ["v1"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "repeating a delete must answer the same success, got {status} {body}"
    );

    // The key is reusable: a put under it stores a new vector, readable end to end.
    ok(
        &router,
        "PutVectors",
        &json!({
            "indexArn": index_arn,
            "vectors": [
                { "key": "v1", "data": { "float32": [1.0, 1.0] }, "metadata": { "color": "blue" } },
            ],
        }),
    )
    .await;
    let got = ok(
        &router,
        "GetVectors",
        &json!({
            "indexArn": index_arn,
            "keys": ["v1"],
            "returnData": true,
            "returnMetadata": true,
        }),
    )
    .await;
    assert_eq!(
        got["vectors"][0]["data"]["float32"],
        json!([1.0, 1.0]),
        "a reused key must hold the data the new put stored, got {got}"
    );
    assert_eq!(
        got["vectors"][0]["metadata"],
        json!({ "color": "blue" }),
        "a reused key must hold the metadata the new put stored, got {got}"
    );
    let requeried = ok(&router, "QueryVectors", &query).await;
    let nearest = requeried["vectors"][0]["distance"].as_f64();
    assert_eq!(
        (keys_of(&requeried).first().map(String::as_str), nearest.is_some_and(|d| close(d, 0.0))),
        (Some("v1"), true),
        "the re-put vector must be the nearest again, at distance 0, got {requeried}"
    );
}

/// DeleteVectors is scoped to the one index its target names: a sibling index in the
/// same bucket, holding the same keys, is untouched.
#[tokio::test]
async fn delete_vectors_only_touches_the_named_index() {
    let router = with_seeded_index("scoped-bucket", "scoped-index").await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": "scoped-bucket",
            "indexName": "sibling-index",
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "cosine",
        }),
    )
    .await;
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": "scoped-bucket",
            "indexName": "sibling-index",
            "vectors": [{ "key": "v1", "data": { "float32": [1.0, 1.0] } }],
        }),
    )
    .await;

    ok(
        &router,
        "DeleteVectors",
        &json!({
            "vectorBucketName": "scoped-bucket",
            "indexName": "scoped-index",
            "keys": SEEDED_KEYS,
        }),
    )
    .await;

    let sibling = ok(
        &router,
        "ListVectors",
        &json!({ "vectorBucketName": "scoped-bucket", "indexName": "sibling-index" }),
    )
    .await;
    assert_eq!(
        keys_of(&sibling),
        vec!["v1".to_string()],
        "the sibling index must still hold its vector, got {sibling}"
    );
    let emptied = ok(
        &router,
        "ListVectors",
        &json!({ "vectorBucketName": "scoped-bucket", "indexName": "scoped-index" }),
    )
    .await;
    assert_eq!(
        keys_of(&emptied),
        Vec::<String>::new(),
        "the named index must be empty, got {emptied}"
    );
}

/// A delete whose target does not resolve is refused, not silently accepted: an absent
/// bucket or index (including one just deleted) is the modelled 404, and a target named
/// in no accepted form, or in two that disagree, is the 400. A deleted index also
/// refuses a later write for the same reason.
#[tokio::test]
async fn delete_vectors_refuses_a_target_it_cannot_resolve() {
    let router = with_seeded_index("refusal-bucket", "refusal-index").await;
    let index_arn = expected_index_arn("refusal-bucket", "refusal-index");
    let other_arn = expected_index_arn("refusal-bucket", "other-index");

    // (case, request, expected status and error code)
    let cases: Vec<(&str, Value, (u16, &str))> = vec![
        (
            "an absent vector bucket",
            json!({ "vectorBucketName": "absent-bucket", "indexName": "refusal-index",
                    "keys": ["v1"] }),
            (404, "NotFoundException"),
        ),
        (
            "an absent vector index",
            json!({ "vectorBucketName": "refusal-bucket", "indexName": "absent-index",
                    "keys": ["v1"] }),
            (404, "NotFoundException"),
        ),
        (
            "an absent index ARN",
            json!({ "indexArn": other_arn, "keys": ["v1"] }),
            (404, "NotFoundException"),
        ),
        (
            "neither a name pair nor an ARN",
            json!({ "keys": ["v1"] }),
            (400, "ValidationException"),
        ),
        (
            "a bucket name with no index name",
            json!({ "vectorBucketName": "refusal-bucket", "keys": ["v1"] }),
            (400, "ValidationException"),
        ),
        (
            "a name pair that disagrees with the ARN",
            json!({ "vectorBucketName": "refusal-bucket", "indexName": "other-index",
                    "indexArn": index_arn, "keys": ["v1"] }),
            (400, "ValidationException"),
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = call(&router, "DeleteVectors", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            expected,
            "case {case:?}: {request} must be refused with {expected:?}, got {status} {body}"
        );
    }

    // The refusals left the index exactly as it was.
    let listed = ok(
        &router,
        "ListVectors",
        &json!({ "indexArn": index_arn }),
    )
    .await;
    assert_eq!(
        keys_of(&listed),
        SEEDED_KEYS
            .iter()
            .map(|key| (*key).to_string())
            .collect::<Vec<String>>(),
        "a refused delete must change nothing, got {listed}"
    );

    // A deleted index holds nothing to delete from, and takes no further writes.
    ok(&router, "DeleteIndex", &json!({ "indexArn": index_arn })).await;
    for (operation, request) in [
        (
            "DeleteVectors",
            json!({ "indexArn": index_arn, "keys": ["v1"] }),
        ),
        (
            "PutVectors",
            json!({ "indexArn": index_arn,
                    "vectors": [{ "key": "v1", "data": { "float32": [1.0, 1.0] } }] }),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "NotFoundException"),
            "case {operation:?}: a deleted index must refuse it, got {status} {body}"
        );
    }
}
