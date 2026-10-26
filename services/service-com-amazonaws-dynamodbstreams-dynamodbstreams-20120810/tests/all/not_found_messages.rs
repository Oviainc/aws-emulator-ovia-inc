// SPDX-License-Identifier: CC0-1.0

//! What a `ResourceNotFoundException` SAYS, per operation.
//!
//! The rule: the record-reading operations (`GetShardIterator`, `GetRecords`) refuse a
//! stream or shard they cannot read with a FIXED sentence that names only the kind of
//! resource that is absent and reproduces nothing of the request; the stream-reporting
//! operation (`DescribeStream`), whose whole answer is about the one stream it was
//! given, names that stream in its message. All three keep HTTP 400 and the
//! `ResourceNotFoundException` code, so the message is the only thing a consumer can
//! tell the refusals apart by.
//!
//! The rows span what the rule generalises over: which component of a well-formed ARN
//! is wrong (table, region, label), how long the ARN and the shard id are, which
//! `ShardIteratorType` is asked for, and whether the stream is absent, still being
//! enabled, or serving a different shard. Instants are placed relative to the clock
//! rather than slept out, so no row races a loaded machine.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;
use smithy_server_runtime::MultiServiceRouter;
use streams::ops::types::KeySchemaElement;
use streams::state::{
    ComAmazonawsDynamodbstreamsDynamodbstreams20120810State as StreamsState, StreamRegistration,
    StreamViewType, DISABLED_STREAM_RETENTION,
};

/// A SigV4 credential scope naming `dynamodb`, which is how DynamoDB Streams requests
/// are signed. Request authentication is off in these tests, so only the scope's
/// routing signal matters.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/dynamodb/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

const REGION: &str = "us-east-1";

/// The fixed message a record-reading call answers for a stream it cannot read.
const STREAM_NOT_FOUND: &str = "Requested resource not found: Stream does not exist";

/// The fixed message a record-reading call answers for a shard the stream it is
/// reading does not have.
const SHARD_NOT_FOUND: &str = "Requested resource not found: Shard does not exist";

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

fn error_code(body: &serde_json::Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The error message as the wire carries it, under whichever casing the protocol
/// renders the modelled `message` member with.
fn error_message(body: &serde_json::Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// An instant `offset` away from now, ahead of it when `ahead` and behind it
/// otherwise.
fn clock_offset(offset: Duration, ahead: bool) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch");
    let moved = if ahead { now + offset } else { now - offset };
    UNIX_EPOCH + moved
}

fn key_schema() -> Vec<KeySchemaElement> {
    vec![KeySchemaElement {
        attribute_name: "Id".to_string(),
        key_type: "HASH".to_string(),
    }]
}

fn register(
    engine: &engine::InMemoryEngine,
    table_name: &str,
    at: SystemTime,
) -> String {
    engine
        .state::<StreamsState>()
        .lock()
        .register_stream(
            &StreamRegistration {
                table_name: table_name.to_string(),
                stream_view_type: StreamViewType::KeysOnly,
                key_schema: key_schema(),
                region: REGION.to_string(),
            },
            at,
        )
        .expect("the registry accepts the registration")
}

/// The single shard of a freshly registered stream, read back over the wire.
async fn only_shard_id(router: &MultiServiceRouter, stream_arn: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeStream",
        serde_json::json!({ "StreamArn": stream_arn }),
    )
    .await;
    assert_eq!(status, 200, "DescribeStream answered {status}: {body}");
    body["StreamDescription"]["Shards"][0]["ShardId"]
        .as_str()
        .unwrap_or_else(|| panic!("no ShardId in {body}"))
        .to_string()
}

/// A well-formed stream ARN no stream is registered under. Longer than the modelled
/// `StreamArn` minimum of 37 characters for every row below.
fn absent_arn(region: &str, table_name: &str, label: &str) -> String {
    format!("arn:aws:dynamodb:{region}:000000000000:table/{table_name}/stream/{label}")
}

/// Shard ids no stream has, inside the modelled `ShardId` bound of 28..65 characters,
/// at both ends of it.
const ABSENT_SHARD_IDS: [(&str, &str); 2] = [
    ("28-character id", "shardId-00000000000000000000"),
    (
        "57-character id",
        "shardId-11111111111111111111-00000000000000000000cafef00d",
    ),
];

/// `GetShardIterator` refuses every stream it cannot iterate with the one fixed
/// message, whatever the request named.
#[tokio::test]
async fn get_shard_iterator_refuses_an_unreadable_stream_with_a_fixed_message() {
    let (engine, router) = build();
    // Serving: registered long enough ago that its enable is due.
    let serving = register(&engine, "ledger", clock_offset(Duration::from_secs(3600), false));
    let serving_shard = only_shard_id(&router, &serving).await;
    // Still enabling: registered an hour from now, so the enable cannot fall due while
    // this test runs and the transient is deterministic rather than raced.
    let enabling = register(&engine, "pending", clock_offset(Duration::from_secs(3600), true));
    let enabling_shard = engine
        .state::<StreamsState>()
        .lock()
        .stream(&enabling)
        .expect("the stream was just registered")
        .shards
        .first()
        .expect("a registration stores a shard to serve")
        .shard_id
        .clone();

    // Each row: what is wrong with the request, the stream ARN, the shard id, the
    // iterator type and any member that type needs. Every row must draw the same
    // message, because it is request-independent.
    let rows: [(&str, String, String, &str, serde_json::Value); 7] = [
        (
            "no stream under this table name",
            absent_arn(REGION, "absent", "2020-01-01T00:00:00.000"),
            ABSENT_SHARD_IDS[0].1.to_string(),
            "TRIM_HORIZON",
            serde_json::json!({}),
        ),
        (
            "a registered table's name in another region",
            absent_arn("eu-west-3", "ledger", "2020-01-01T00:00:00.000"),
            serving_shard.clone(),
            "LATEST",
            serde_json::json!({}),
        ),
        (
            "a registered table with a label it never had",
            absent_arn(REGION, "ledger", "1999-12-31T23:59:59.999"),
            serving_shard.clone(),
            "AT_SEQUENCE_NUMBER",
            serde_json::json!({ "SequenceNumber": "100000000000000000001" }),
        ),
        (
            "a very long table name nothing registered",
            absent_arn(REGION, &"t".repeat(200), "2020-01-01T00:00:00.000"),
            ABSENT_SHARD_IDS[1].1.to_string(),
            "AFTER_SEQUENCE_NUMBER",
            serde_json::json!({ "SequenceNumber": "100000000000000000001" }),
        ),
        (
            "a serving stream's ARN with one extra character",
            format!("{serving}0"),
            serving_shard.clone(),
            "AT_TIMESTAMP",
            serde_json::json!({ "Timestamp": 1_600_000_000 }),
        ),
        (
            "a stream whose enable is still pending",
            enabling.clone(),
            enabling_shard,
            "TRIM_HORIZON",
            serde_json::json!({}),
        ),
        (
            "a stream whose enable is still pending, unknown shard",
            enabling,
            ABSENT_SHARD_IDS[1].1.to_string(),
            "LATEST",
            serde_json::json!({}),
        ),
    ];

    for (case, stream_arn, shard_id, iterator_type, extra) in rows {
        let mut request = serde_json::json!({
            "StreamArn": stream_arn,
            "ShardId": shard_id,
            "ShardIteratorType": iterator_type,
        });
        if let (Some(target), Some(extra)) = (request.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        let (status, body) = call(&router, "GetShardIterator", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {code:?} ({body})"
        );
        let message = error_message(&body);
        assert_eq!(
            message, STREAM_NOT_FOUND,
            "case {case:?}: expected the fixed message, got {message:?}"
        );
        // Request-independent: nothing the caller sent comes back in the message.
        assert!(
            !message.contains(&stream_arn) && !message.contains(&shard_id),
            "case {case:?}: the message must not reproduce the request, got {message:?}"
        );
    }
}

/// A shard a SERVING stream does not have is the other fixed message — distinct from
/// the stream one, so the two refusals stay tellable apart.
#[tokio::test]
async fn get_shard_iterator_refuses_an_unknown_shard_with_its_own_fixed_message() {
    let (engine, router) = build();
    let serving = register(&engine, "ledger", clock_offset(Duration::from_secs(3600), false));
    let known_shard = only_shard_id(&router, &serving).await;

    for (case, absent_shard) in ABSENT_SHARD_IDS {
        for iterator_type in ["TRIM_HORIZON", "LATEST"] {
            let case = format!("{case} ({iterator_type})");
            let (status, body) = call(
                &router,
                "GetShardIterator",
                serde_json::json!({
                    "StreamArn": serving,
                    "ShardId": absent_shard,
                    "ShardIteratorType": iterator_type,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body}"
            );
            let code = error_code(&body);
            assert!(
                code.ends_with("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {code:?} ({body})"
            );
            let message = error_message(&body);
            assert_eq!(
                message, SHARD_NOT_FOUND,
                "case {case:?}: expected the fixed shard message, got {message:?}"
            );
            assert!(
                !message.contains(&serving) && !message.contains(absent_shard),
                "case {case:?}: the message must not reproduce the request, got {message:?}"
            );
        }
    }

    // The shard the stream DOES serve is still served, so the refusals above are about
    // the shard id and not about the stream.
    let (status, body) = call(
        &router,
        "GetShardIterator",
        serde_json::json!({
            "StreamArn": serving,
            "ShardId": known_shard,
            "ShardIteratorType": "TRIM_HORIZON",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {known_shard:?}: the served shard must hand out an iterator, got {status} with \
         {body}"
    );
}

/// `DescribeStream` reports on the one stream it was given, so its message NAMES that
/// stream — the message `GetShardIterator` must not answer. Asserted over several
/// ARNs, so the text is shown to track the request rather than being another constant.
#[tokio::test]
async fn describe_stream_names_the_stream_it_was_asked_about() {
    let (_engine, router) = build();

    let rows: [(&str, String); 3] = [
        (
            "an absent table",
            absent_arn(REGION, "absent", "2020-01-01T00:00:00.000"),
        ),
        (
            "another region",
            absent_arn("ap-south-1", "ledger", "2026-02-03T04:05:06.789"),
        ),
        (
            "a very long table name",
            absent_arn(REGION, &"t".repeat(200), "2020-01-01T00:00:00.000"),
        ),
    ];

    for (case, stream_arn) in rows {
        let (status, body) = call(
            &router,
            "DescribeStream",
            serde_json::json!({ "StreamArn": stream_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {code:?} ({body})"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!("Requested resource not found: Stream: {stream_arn} not found"),
            "case {case:?}: expected the stream to be named, got {message:?}"
        );
        assert_ne!(
            message, STREAM_NOT_FOUND,
            "case {case:?}: the reporting refusal must differ from the reading one"
        );
    }
}

/// `GetRecords` with an iterator standing in a shard its stream does not have draws
/// the shard constant — the same fixed form this operation answers for a stream, and
/// distinct from it, so the two refusals stay tellable apart here too.
#[tokio::test]
async fn get_records_refuses_an_absent_shard_with_the_fixed_shard_message() {
    let (engine, router) = build();
    let stream_arn = register(
        &engine,
        "ledger",
        clock_offset(Duration::from_secs(3600), false),
    );

    for (case, absent_shard) in ABSENT_SHARD_IDS {
        // The minting seam, pointed at a shard this stream does not serve: the state a
        // shard that went away under a live iterator leaves behind.
        let token = engine.state::<StreamsState>().lock().mint_iterator(
            &stream_arn,
            absent_shard,
            0,
            SystemTime::now(),
        );

        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {code:?} ({body})"
        );
        let message = error_message(&body);
        assert_eq!(
            message, SHARD_NOT_FOUND,
            "case {case:?}: expected the fixed shard message, got {message:?}"
        );
        assert!(
            !message.contains(&stream_arn) && !message.contains(absent_shard),
            "case {case:?}: the message must disclose neither the stream nor the shard, got \
             {message:?}"
        );
    }
}

/// `GetRecords` holds an opaque token, never an ARN, so when the stream behind the
/// token is gone its refusal is the same fixed message and discloses nothing of what
/// the token resolved to.
#[tokio::test]
async fn get_records_refuses_a_vanished_stream_with_the_fixed_message() {
    let (engine, router) = build();
    let stream_arn = register(&engine, "ledger", clock_offset(Duration::from_secs(3600), false));
    let shard_id = only_shard_id(&router, &stream_arn).await;

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
    assert_eq!(status, 200, "GetShardIterator answered {status}: {body}");
    let token = body["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("no ShardIterator in {body}"))
        .to_string();

    // Disabled longer ago than the documented window a disabled stream stays
    // registered for, so the next call's clock sweep deletes it while the iterator
    // minted just now is still live.
    assert!(
        engine.state::<StreamsState>().lock().disable_stream(
            &stream_arn,
            clock_offset(DISABLED_STREAM_RETENTION + Duration::from_secs(3600), false),
        ),
        "the registered stream must be disablable"
    );

    let (status, body) = call(
        &router,
        "GetRecords",
        serde_json::json!({ "ShardIterator": token }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case vanished stream: expected HTTP 400, got {status} with {body}"
    );
    let code = error_code(&body);
    assert!(
        code.ends_with("ResourceNotFoundException"),
        "case vanished stream: expected ResourceNotFoundException, got {code:?} ({body})"
    );
    let message = error_message(&body);
    assert_eq!(
        message, STREAM_NOT_FOUND,
        "case vanished stream: expected the fixed message, got {message:?}"
    );
    assert!(
        !message.contains(&stream_arn) && !message.contains(&shard_id),
        "case vanished stream: the message must disclose neither the stream nor the shard, got \
         {message:?}"
    );
}
