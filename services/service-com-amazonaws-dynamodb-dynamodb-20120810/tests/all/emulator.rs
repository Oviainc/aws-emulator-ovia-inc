// SPDX-License-Identifier: CC0-1.0

//! The emulator every suite in this test target drives, and the helpers they share.
//!
//! One harness rather than one per suite: a suite that needs the router or a request takes
//! it from here (`use crate::emulator::{..}`) so the way this crate is driven over the wire
//! is described once. The suites that still carry their own copy are welcome to adopt it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region a request is signed for when a suite does not care which one it is. A
/// DynamoDB table name is unique within ONE region, which is the scope the store keys by.
pub const REGION: &str = "us-east-1";

/// One emulator holding only this service, over one shared engine.
pub fn emulator() -> MultiServiceRouter {
    emulator_and_engine().0
}

/// The same emulator, together with the shared engine it was built on — for a suite that
/// must reach this service's STORED state (what a handler reaches through `self.engine`)
/// and not only its wire surface.
pub fn emulator_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suites drive unsigned requests, so signature verification is off; the
    // `Authorization` header below is still what names the region, exactly as a signed
    // client's would.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One emulator holding this service AND DynamoDB Streams, over ONE shared engine.
///
/// A stream is created by a DynamoDB call and read by a streams call, so a suite
/// about streams needs both services mounted on the same engine — which is also how
/// the router mounts them. They share the `dynamodb` signing name and are told apart
/// by `X-Amz-Target`, which is what [`call_streams`] sets.
pub fn emulator_with_streams() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Send one operation in `region` and return its HTTP status and parsed body.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch(
        router,
        region,
        &format!("DynamoDB_20120810.{operation}"),
        body,
    )
    .await
}

/// Send one DynamoDB STREAMS operation in [`REGION`]. Both services sign as
/// `dynamodb`, so the `X-Amz-Target` prefix is the whole discriminator.
pub async fn call_streams(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch(
        router,
        REGION,
        &format!("DynamoDBStreams_20120810.{operation}"),
        body,
    )
    .await
}

/// Send one request to `target` (the whole `X-Amz-Target` value) in `region`.
async fn dispatch(
    router: &MultiServiceRouter,
    region: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/dynamodb/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// [`call_in`] in [`REGION`], for a suite that is not about regions.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in(router, REGION, operation, body).await
}

/// [`call`] for an operation that must succeed, returning its parsed body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Every record a table's stream holds, read from its trim horizon through the STREAMS
/// surface — the loop a change-data-capture consumer runs, shared by every suite that
/// reads a change feed. `case` names the row the assertions below belong to.
///
/// The walk is bounded, so a handler that never advances its iterator fails rather than
/// hanging, and it asserts the stream serves a shard and a position in it: a drain that
/// silently answered nothing would surface only as a record mismatch far from its cause.
pub async fn drain_stream(
    router: &MultiServiceRouter,
    case: &str,
    stream_arn: &str,
) -> Vec<Value> {
    let (status, described) =
        call_streams(router, "DescribeStream", json!({"StreamArn": stream_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStream must resolve the table's stream, got {status} with \
         {described}"
    );
    let shards = described["StreamDescription"]["Shards"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !shards.is_empty(),
        "case {case:?}: DescribeStream must list at least one shard, got {described}"
    );
    let mut records: Vec<Value> = Vec::new();
    for shard in &shards {
        let shard_id = shard["ShardId"].as_str().unwrap_or_default();
        let (status, minted) = call_streams(
            router,
            "GetShardIterator",
            json!({
                "StreamArn": stream_arn,
                "ShardId": shard_id,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetShardIterator answered {status} with {minted}"
        );
        let mut iterator = minted["ShardIterator"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !iterator.is_empty(),
            "case {case:?}: GetShardIterator must mint a position in shard {shard_id:?}, got \
             {minted}"
        );
        for _ in 0..16 {
            let (status, page) =
                call_streams(router, "GetRecords", json!({"ShardIterator": iterator})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetRecords answered {status} with {page}"
            );
            let batch = page["Records"].as_array().cloned().unwrap_or_default();
            records.extend(batch.iter().cloned());
            match page["NextShardIterator"].as_str() {
                Some(next) if !batch.is_empty() => iterator = next.to_string(),
                _ => break,
            }
        }
    }
    records
}

/// The `eventName` of every stream record, in order.
pub fn event_names(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| record["eventName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `GlobalSecondaryIndexes` entry of a description by index name, or `Null` when
/// that description carries no index by that name.
pub fn index_named(description: &Value, name: &str) -> Value {
    description["GlobalSecondaryIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|index| index["IndexName"] == serde_json::json!(name))
        .unwrap_or(Value::Null)
}

/// The `message` a refusal carries, under either of the casings a client reads it by.
pub fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The wire error code of a refusal: the shape name the `__type` field carries, which
/// is what an SDK client matches an error on.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}
