// SPDX-License-Identifier: CC0-1.0

//! What ONE stream retains is budgeted.
//!
//! The rule pinned here: a stream's records are a bounded window, not a log that grows
//! with however many item writes a caller makes. Records leave it OLDEST FIRST, which
//! is what the documented 24 hour retention does to records that age out, and what the
//! retained-byte ceiling this emulator adds does to records of a stream that has
//! filled it. Trimming is not a gap: the shard counts what it trimmed, so what a
//! consumer still reads from the trim horizon is a contiguous suffix of what was
//! appended, in append order.
//!
//! Driven through the publishing seam on the state (which is what DynamoDB's item
//! writes call) and read back over the wire, so the window is asserted as a consumer
//! sees it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;
use smithy_server_runtime::MultiServiceRouter;
use streams::ops::types::{AttributeValue, KeySchemaElement};
use streams::state::{
    AttributeMap, ComAmazonawsDynamodbstreamsDynamodbstreams20120810State as StreamsState,
    OperationType, RecordAppend, StreamRegistration, StreamViewType, MAX_RETAINED_STREAM_BYTES,
};

/// A SigV4 credential scope naming `dynamodb`, which is how a DynamoDB Streams request
/// is signed. Request authentication is off here, so only its routing signal matters.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/dynamodb/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

const REGION: &str = "us-east-1";

/// The payload one record in this suite carries, so a modest number of appends spans
/// the ceiling rather than needing a million of them.
const PAYLOAD_BYTES: usize = 1024 * 1024;

fn build() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(streams::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("authorization", AUTHORIZATION)
        .header(
            "x-amz-target",
            format!("DynamoDBStreams_20120810.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
    (status, parsed)
}

fn keys_of(id: usize) -> AttributeMap {
    HashMap::from([("Id".to_string(), AttributeValue::S(format!("item-{id}")))])
}

/// Publish one `INSERT` of a large item onto `stream_arn`, returning its sequence
/// number.
fn append_large(engine: &engine::InMemoryEngine, stream_arn: &str, id: usize) -> String {
    engine
        .state::<StreamsState>()
        .lock()
        .append_record(
            stream_arn,
            &RecordAppend {
                event_name: OperationType::Insert,
                keys: keys_of(id),
                new_image: Some(HashMap::from([
                    (
                        "Id".to_string(),
                        AttributeValue::S(format!("item-{id}")),
                    ),
                    (
                        "Payload".to_string(),
                        AttributeValue::S("x".repeat(PAYLOAD_BYTES)),
                    ),
                ])),
                old_image: None,
            },
            SystemTime::now(),
        )
        .unwrap_or_else(|rejected| panic!("the registry must take the record, got {rejected}"))
}

/// Every record the stream's one shard still retains, read from its trim horizon.
async fn retained(
    router: &MultiServiceRouter,
    case: &str,
    stream_arn: &str,
    appended: usize,
) -> Vec<serde_json::Value> {
    let (status, described) =
        call(router, "DescribeStream", serde_json::json!({"StreamArn": stream_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStream answered {status} with {described}"
    );
    let shard_id = described["StreamDescription"]["Shards"][0]["ShardId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no shard in {described}"))
        .to_string();
    let (status, minted) = call(
        router,
        "GetShardIterator",
        serde_json::json!({
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
    let mut records: Vec<serde_json::Value> = Vec::new();
    // Bounded by the appends: a reader that never advances fails rather than hanging.
    for _ in 0..appended + 2 {
        let (status, page) =
            call(router, "GetRecords", serde_json::json!({"ShardIterator": iterator})).await;
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
    records
}

#[tokio::test]
async fn a_stream_retains_a_bounded_window_of_its_newest_records() {
    // Two rows, either side of the ceiling: a stream holding well under it keeps
    // everything, and one driven past it keeps a suffix. The row counts are derived
    // from the ceiling itself, so neither depends on the constant's value.
    let under = usize::try_from(MAX_RETAINED_STREAM_BYTES).unwrap_or(usize::MAX) / PAYLOAD_BYTES / 8;
    let over = usize::try_from(MAX_RETAINED_STREAM_BYTES).unwrap_or(usize::MAX) / PAYLOAD_BYTES + 6;

    for (case, appends) in [("inside the ceiling", under), ("past the ceiling", over)] {
        let (engine, router) = build();
        // Registered in the PAST, so the stream's asynchronous enable is already due
        // and a consumer sees its shards: this suite is about what a stream retains,
        // not about the enable window.
        let registered_at = SystemTime::now() - Duration::from_secs(60);
        let stream_arn = engine
            .state::<StreamsState>()
            .lock()
            .register_stream(
                &StreamRegistration {
                    table_name: "ledger".to_string(),
                    stream_view_type: StreamViewType::NewImage,
                    key_schema: vec![KeySchemaElement {
                        attribute_name: "Id".to_string(),
                        key_type: "HASH".to_string(),
                    }],
                    region: REGION.to_string(),
                },
                registered_at,
            )
            .expect("the registry accepts the registration");

        let appended: Vec<String> = (0..appends)
            .map(|id| append_large(&engine, &stream_arn, id))
            .collect();

        let records = retained(&router, case, &stream_arn, appends).await;
        let kept: Vec<String> = records
            .iter()
            .map(|record| {
                record["dynamodb"]["SequenceNumber"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();

        // What is kept is a SUFFIX of what was appended, in append order: records
        // leave oldest first, and nothing in the middle is dropped.
        assert!(
            !kept.is_empty() && kept.len() <= appended.len(),
            "case {case:?}: expected between 1 and {} records retained, got {}",
            appended.len(),
            kept.len()
        );
        assert_eq!(
            kept,
            appended[appended.len() - kept.len()..],
            "case {case:?}: the retained records must be the newest ones, in append order"
        );

        // And the window is inside the ceiling.
        let retained_bytes: i64 = records
            .iter()
            .map(|record| record["dynamodb"]["SizeBytes"].as_i64().unwrap_or(0))
            .sum();
        assert!(
            retained_bytes <= MAX_RETAINED_STREAM_BYTES,
            "case {case:?}: expected at most {MAX_RETAINED_STREAM_BYTES} retained bytes, got \
             {retained_bytes}"
        );

        // A stream that never filled the ceiling has trimmed nothing; one that did has.
        let expected_all = case == "inside the ceiling";
        assert_eq!(
            kept.len() == appended.len(),
            expected_all,
            "case {case:?}: expected {} of the {} appended records retained, got {}",
            if expected_all { "all" } else { "a suffix" },
            appended.len(),
            kept.len()
        );
    }
}
