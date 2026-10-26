// SPDX-License-Identifier: CC0-1.0

//! `StreamRecord.ApproximateCreationDateTime` carries whole seconds.
//!
//! The rule pinned here comes from the member's own documentation: the approximate
//! creation time of a stream record is reported "in UNIX epoch time format and rounded
//! down to the closest second", so the value a consumer reads has NO sub-second
//! component whatever sub-second phase the write that produced it landed on. Clients
//! bucket, dedupe and compare records on it and tools re-serialize it as an integer
//! epoch, so a value carrying milliseconds never equals the one the service would have
//! produced.
//!
//! The rows span the phase of the write inside its second — the boundary itself, one
//! millisecond in, the middle, and the last nanosecond before the next second — plus
//! the live wall clock, whose phase nobody chooses. The instant is stamped at WRITE
//! time, which the ordering row proves: records keep the order they were appended in
//! and each reports its own second, not the second it was read in.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;
use smithy_server_runtime::MultiServiceRouter;
use streams::ops::types::{AttributeValue, KeySchemaElement};
use streams::state::{
    AttributeMap, ComAmazonawsDynamodbstreamsDynamodbstreams20120810State as StreamsState,
    OperationType, RecordAppend, StreamRegistration, StreamViewType,
};

/// A SigV4 credential scope naming `dynamodb`, which is how a DynamoDB Streams request
/// is signed. Request authentication is off here, so only its routing signal matters.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/dynamodb/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

const REGION: &str = "us-east-1";

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

/// A whole-second instant `seconds` before now.
fn seconds_ago(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(epoch_secs(SystemTime::now()) - seconds)
}

fn epoch_secs(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the instant is after the Unix epoch")
        .as_secs()
}

fn register(engine: &engine::InMemoryEngine, table_name: &str, at: SystemTime) -> String {
    engine
        .state::<StreamsState>()
        .lock()
        .register_stream(
            &StreamRegistration {
                table_name: table_name.to_string(),
                stream_view_type: StreamViewType::NewImage,
                key_schema: vec![KeySchemaElement {
                    attribute_name: "Id".to_string(),
                    key_type: "HASH".to_string(),
                }],
                region: REGION.to_string(),
            },
            at,
        )
        .expect("the registry accepts the registration")
}

fn keys_of(id: &str) -> AttributeMap {
    HashMap::from([("Id".to_string(), AttributeValue::S(id.to_string()))])
}

/// Publish one `INSERT` onto `stream_arn` at `at` — the seam DynamoDB's item writes
/// call, which is where the record's creation instant is stamped.
fn append(engine: &engine::InMemoryEngine, stream_arn: &str, id: &str, at: SystemTime) {
    engine
        .state::<StreamsState>()
        .lock()
        .append_record(
            stream_arn,
            &RecordAppend {
                event_name: OperationType::Insert,
                keys: keys_of(id),
                new_image: Some(keys_of(id)),
                old_image: None,
            },
            at,
        )
        .unwrap_or_else(|rejected| panic!("the registry must take the record, got {rejected}"));
}

/// Every record of the stream's one shard, drained from its trim horizon.
async fn drain(
    router: &MultiServiceRouter,
    case: &str,
    stream_arn: &str,
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
    let iterator = minted["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ShardIterator in {minted}"));
    let (status, page) = call(
        router,
        "GetRecords",
        serde_json::json!({ "ShardIterator": iterator }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetRecords answered {status} with {page}"
    );
    page["Records"].as_array().cloned().unwrap_or_default()
}

/// The `ApproximateCreationDateTime` of one record, as the wire carries it: epoch
/// time, so a number.
fn creation_time(case: &str, record: &serde_json::Value) -> f64 {
    let reported = &record["dynamodb"]["ApproximateCreationDateTime"];
    reported
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: ApproximateCreationDateTime is {reported}"))
}

#[tokio::test]
async fn a_records_creation_time_is_floored_to_its_second() {
    let base = seconds_ago(600);
    // One row per sub-second phase of the write: the second's boundary, a millisecond
    // in, the middle, the recorded phase, and the last nanosecond before the next
    // second. Each must report its own second and nothing finer.
    let phases: [(&str, Duration); 5] = [
        ("on the second", Duration::ZERO),
        ("one millisecond in", Duration::from_millis(1)),
        ("mid-second", Duration::from_millis(500)),
        ("134 milliseconds in", Duration::from_millis(134)),
        ("the last nanosecond", Duration::new(0, 999_999_999)),
    ];

    for (index, (case, phase)) in phases.iter().enumerate() {
        let (engine, router) = build();
        let stream_arn = register(&engine, "clock", base);
        let written_at = base + Duration::from_secs(index as u64) + *phase;
        append(&engine, &stream_arn, "clock-item", written_at);

        let records = drain(&router, case, &stream_arn).await;
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: the one write must publish one record, got {records:?}"
        );
        let reported = creation_time(case, &records[0]);
        let expected = epoch_secs(written_at) as f64;
        assert!(
            (reported - expected).abs() < f64::EPSILON,
            "case {case:?}: expected the write's second {expected}, got {reported}"
        );
        assert!(
            reported.fract() == 0.0,
            "case {case:?}: the value carries no sub-second component, got {reported}"
        );
    }
}

#[tokio::test]
async fn the_live_clocks_own_phase_is_floored_too_and_the_stamp_is_the_writes() {
    // Nobody chooses the sub-second phase of the wall clock, which is the path a real
    // item write takes: whatever it lands on, no record may report a fraction of a
    // second. The records are also appended in order with distinct seconds, which is
    // what shows the instant is stamped per write rather than taken at read time.
    let case = "the live wall clock";
    let (engine, router) = build();
    let stream_arn = register(&engine, "clock", seconds_ago(600));
    let mut written: Vec<u64> = Vec::new();
    for index in 0..3u64 {
        let at = SystemTime::now() - Duration::from_secs(10 - index);
        written.push(epoch_secs(at));
        append(&engine, &stream_arn, &format!("item-{index}"), at);
    }

    let records = drain(&router, case, &stream_arn).await;
    assert_eq!(
        records.len(),
        written.len(),
        "case {case:?}: every write must publish one record, got {records:?}"
    );
    for (index, record) in records.iter().enumerate() {
        let reported = creation_time(case, record);
        assert!(
            reported.fract() == 0.0,
            "case {case:?} record {index}: expected a whole second, got {reported}"
        );
        assert!(
            (reported - written[index] as f64).abs() < f64::EPSILON,
            "case {case:?} record {index}: expected the write's own second {}, got {reported}",
            written[index]
        );
    }
}
