// SPDX-License-Identifier: CC0-1.0

//! What a stream SERVES while the asynchronous enable its registration started is
//! still pending.
//!
//! The rule: a stream is `ENABLING` while "Streams is currently being enabled on the
//! DynamoDB table" (the model's `StreamStatus` doc), and a stream that is still being
//! enabled has nothing to read — it exposes NO shards, so `DescribeStream` lists none
//! and `GetShardIterator` has no shard to iterate. The consequence a consumer relies
//! on is the invariant asserted below: whenever `DescribeStream` lists a shard, the
//! same description reports `ENABLED`, so the status a consumer gates on and the
//! shards it would read can never disagree. A `DISABLING`/`DISABLED` stream is the
//! other direction: it keeps its shards, because AWS leaves a disabled stream
//! drainable for the documented 24 hours.
//!
//! The rows span the lifecycle instants the rule generalises over — a registration
//! whose enable is far from due, one that just fell due, one long past due, and a
//! stream that has since been disabled — crossed with every `StreamViewType` the
//! model declares. Registration instants are placed relative to the clock rather
//! than slept out, so no row races a loaded machine, and every assertion reads the
//! answer back over the wire.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;
use smithy_server_runtime::MultiServiceRouter;
use streams::ops::types::KeySchemaElement;
use streams::state::{
    ComAmazonawsDynamodbstreamsDynamodbstreams20120810State as StreamsState, StreamRegistration,
    StreamViewType, STREAM_ENABLE_DELAY,
};

/// A SigV4 credential scope naming `dynamodb`, which is how DynamoDB Streams
/// requests are signed. Request authentication is off in these tests, so only the
/// scope's routing signal matters.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/dynamodb/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

const REGION: &str = "us-east-1";

/// Every value the model's `StreamViewType` enum declares, as this crate's own
/// parser reads them off the wire — the view type is what a stream captures, and the
/// rule under test holds for each of them.
const VIEW_TYPES: [(&str, StreamViewType); 4] = [
    ("KEYS_ONLY", StreamViewType::KeysOnly),
    ("NEW_IMAGE", StreamViewType::NewImage),
    ("OLD_IMAGE", StreamViewType::OldImage),
    ("NEW_AND_OLD_IMAGES", StreamViewType::NewAndOldImages),
];

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

fn key_schema() -> Vec<KeySchemaElement> {
    vec![KeySchemaElement {
        attribute_name: "Id".to_string(),
        key_type: "HASH".to_string(),
    }]
}

/// An instant `offset` away from now, ahead of it when `ahead` and behind it
/// otherwise. A row whose enable must still be PENDING places the registration far
/// enough ahead that the enable cannot fall due while the test runs, whatever the
/// machine is doing, so the transient state is deterministic rather than raced.
fn clock_offset(offset: Duration, ahead: bool) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch");
    let moved = if ahead { now + offset } else { now - offset };
    UNIX_EPOCH + moved
}

fn register(
    engine: &engine::InMemoryEngine,
    table_name: &str,
    stream_view_type: StreamViewType,
    at: SystemTime,
) -> String {
    engine
        .state::<StreamsState>()
        .lock()
        .register_stream(
            &StreamRegistration {
                table_name: table_name.to_string(),
                stream_view_type,
                key_schema: key_schema(),
                region: REGION.to_string(),
            },
            at,
        )
        .expect("the registry accepts the registration")
}

/// The shard ids the registry STORES for this stream, whether or not the stream
/// serves them yet: the id a client would have to present to `GetShardIterator`.
fn stored_shard_ids(engine: &engine::InMemoryEngine, stream_arn: &str) -> Vec<String> {
    engine
        .state::<StreamsState>()
        .lock()
        .stream(stream_arn)
        .expect("the stream was just registered")
        .shards
        .iter()
        .map(|shard| shard.shard_id.clone())
        .collect()
}

/// One `DescribeStream`, as its `StreamDescription`.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    stream_arn: &str,
) -> serde_json::Value {
    let (status, body) = call(
        router,
        "DescribeStream",
        serde_json::json!({ "StreamArn": stream_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStream must answer 200, got {status} with {body}"
    );
    body["StreamDescription"].clone()
}

fn status_of(description: &serde_json::Value) -> String {
    description["StreamStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn shard_ids(description: &serde_json::Value) -> Vec<String> {
    description["Shards"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|shard| shard["ShardId"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn error_code(body: &serde_json::Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// Whether a stream serves its shards is decided by the clock alone: not while the
/// enable is pending, and from the instant it is due onwards — including after the
/// stream has been disabled, which leaves it drainable.
#[tokio::test]
async fn a_stream_serves_its_shards_only_once_its_enable_is_due() {
    // Each row: how far the registration is from now, which side of now, whether the
    // stream is then disabled, and the status plus shard visibility the rule requires.
    // `STREAM_ENABLE_DELAY` is the span the registry owes the enable, so an instant
    // exactly that far back is the first one at which the enable is due.
    let rows: [(&str, Duration, bool, bool, &str, bool); 4] = [
        ("registered an hour from now", Duration::from_secs(3600), true, false, "ENABLING", false),
        ("enable exactly due", STREAM_ENABLE_DELAY, false, false, "ENABLED", true),
        ("registered an hour ago", Duration::from_secs(3600), false, false, "ENABLED", true),
        ("disabled an hour ago", Duration::from_secs(3600), false, true, "DISABLED", true),
    ];

    for (row, offset, ahead, disabled, expected_status, serves_shards) in rows {
        for (view_name, view_type) in VIEW_TYPES {
            let case = format!("{row} ({view_name})");
            let (engine, router) = build();
            let at = clock_offset(offset, ahead);
            let stream_arn = register(&engine, "ledger", view_type, at);
            if disabled {
                assert!(
                    engine
                        .state::<StreamsState>()
                        .lock()
                        .disable_stream(&stream_arn, at),
                    "case {case:?}: the registered stream must be disablable"
                );
            }

            let description = describe(&router, &case, &stream_arn).await;
            assert_eq!(
                status_of(&description),
                expected_status,
                "case {case:?}: wrong StreamStatus, got {description}"
            );

            let stored = stored_shard_ids(&engine, &stream_arn);
            assert!(
                !stored.is_empty(),
                "case {case:?}: a registration stores a shard to serve, got {stored:?}"
            );
            let served = shard_ids(&description);
            let expected_shards: Vec<String> = if serves_shards {
                stored.clone()
            } else {
                Vec::new()
            };
            assert_eq!(
                served, expected_shards,
                "case {case:?}: wrong shards served, got {description}"
            );

            // The invariant a consumer gates on: a description that lists a shard
            // never reports the stream as still being enabled.
            assert!(
                served.is_empty() || status_of(&description) != "ENABLING",
                "case {case:?}: a stream that lists shards must not report ENABLING, got \
                 {description}"
            );

            // `GetShardIterator` answers for exactly the shards the stream serves: a
            // shard of a stream whose enable is not due yet is as absent as one of
            // another stream.
            for shard_id in &stored {
                let (status, body) = call(
                    &router,
                    "GetShardIterator",
                    serde_json::json!({
                        "StreamArn": stream_arn,
                        "ShardId": shard_id,
                        "ShardIteratorType": "TRIM_HORIZON",
                    }),
                )
                .await;
                if serves_shards {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: shard {shard_id} must hand out an iterator, got {status} \
                         with {body}"
                    );
                    assert!(
                        body["ShardIterator"].as_str().is_some_and(|token| !token.is_empty()),
                        "case {case:?}: shard {shard_id} must answer with a token, got {body}"
                    );
                } else {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: shard {shard_id} is not addressable yet, got {status} \
                         with {body}"
                    );
                    assert!(
                        error_code(&body).ends_with("ResourceNotFoundException"),
                        "case {case:?}: expected ResourceNotFoundException, got {body}"
                    );
                }
            }
        }
    }
}
