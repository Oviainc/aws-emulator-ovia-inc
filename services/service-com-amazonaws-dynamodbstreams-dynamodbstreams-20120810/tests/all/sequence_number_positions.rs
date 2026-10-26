// SPDX-License-Identifier: CC0-1.0

//! Which position a `SequenceNumber` names, and which refusal it draws when it names
//! none.
//!
//! The rule pinned here: a client-supplied sequence number addresses a position in ONE
//! shard, and it belongs to that shard only inside the range the shard itself
//! advertises (`Shard$SequenceNumberRange`: at or after `StartingSequenceNumber`, and
//! at or before `EndingSequenceNumber` where the shard is closed — an open shard
//! advertises no end). A number outside that range, like a request that leaves the
//! position out altogether, is a BAD ARGUMENT: HTTP 400 `ValidationException`.
//! `TrimmedDataAccessException` means something else entirely — "the operation
//! attempted to read past the oldest stream record in a shard", the 24 hour retention
//! trim point — and is reserved for a position inside the range whose records are gone.
//!
//! The distinction is what a consumer framework branches on: the trimmed refusal tells
//! a poller its checkpoint fell behind the retention window, so it restarts the shard
//! at `TRIM_HORIZON` and reprocesses everything, while a bad argument is non-retryable
//! and must surface.
//!
//! The rows span the class rather than one recorded number: below the shard's start,
//! above a closed shard's end, each shard's own numbers asked across the pair, a number
//! no record ever took, both sequence-number iterator types, the member omitted, a
//! value of the modelled length that is not a number at all, and — on a shard that has
//! trimmed, and on one that has trimmed everything it held — a position genuinely
//! behind the trim point.

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

/// A sequence number of the modelled minimum length (21 characters) that is below
/// every number this service hands out, so it names no position in any shard.
const ALL_ZEROS: &str = "000000000000000000000";

/// A sequence number of the modelled length above every number this service has
/// handed out: 21 nines.
const ALL_NINES: &str = "999999999999999999999";

/// The whole refusal a sequence number the addressed shard does not cover draws.
const INVALID_FOR_SHARD: &str = "Invalid SequenceNumber for the shard";

/// The whole refusal a sequence-number iterator type with no `SequenceNumber` draws.
const SEQUENCE_NUMBER_NULL: &str =
    "One or more parameter values were invalid: SequenceNumber cannot be null";

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
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs();
    UNIX_EPOCH + Duration::from_secs(now - seconds)
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

/// Publish one `INSERT` onto `stream_arn` — the seam DynamoDB's item writes call —
/// and return the sequence number it was given.
fn append(
    engine: &engine::InMemoryEngine,
    stream_arn: &str,
    id: &str,
    at: SystemTime,
) -> String {
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
        .unwrap_or_else(|rejected| panic!("the registry must take the record, got {rejected}"))
}

/// The wire error code of a refusal, as an SDK client matches it.
fn error_code(body: &serde_json::Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a refusal carries, under either of the casings a client reads it by.
fn message(body: &serde_json::Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Every shard the stream serves, as `DescribeStream` reports it (creation order).
async fn shards(router: &MultiServiceRouter, stream_arn: &str) -> Vec<serde_json::Value> {
    let (status, body) =
        call(router, "DescribeStream", serde_json::json!({"StreamArn": stream_arn})).await;
    assert_eq!(status, 200, "DescribeStream answered {status} with {body}");
    body["StreamDescription"]["Shards"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn shard_id(shard: &serde_json::Value) -> String {
    shard["ShardId"]
        .as_str()
        .unwrap_or_else(|| panic!("no ShardId in {shard}"))
        .to_string()
}

/// What one row expects: either the sequence numbers the minted iterator delivers, or
/// the error code the request is refused with — together with the whole message where
/// the wording is itself a requirement.
type Outcome = Result<Vec<String>, (&'static str, Option<&'static str>)>;

/// Ask for an iterator and, where the row expects one, read one page through it.
async fn check_row(
    router: &MultiServiceRouter,
    case: &str,
    stream_arn: &str,
    shard: &str,
    iterator_type: &str,
    sequence_number: Option<&str>,
    expected: &Outcome,
) {
    let mut body = serde_json::json!({
        "StreamArn": stream_arn,
        "ShardId": shard,
        "ShardIteratorType": iterator_type,
    });
    if let (Some(target), Some(number)) = (body.as_object_mut(), sequence_number) {
        target.insert(
            "SequenceNumber".to_string(),
            serde_json::Value::String(number.to_string()),
        );
    }
    let (status, minted) = call(router, "GetShardIterator", body).await;

    let delivered = match expected {
        Err((code, text)) => {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {minted}"
            );
            assert_eq!(
                error_code(&minted),
                *code,
                "case {case:?}: expected error code {code}, got {minted}"
            );
            if let Some(text) = text {
                assert_eq!(
                    message(&minted),
                    *text,
                    "case {case:?}: expected the message {text:?}, got {}",
                    message(&minted)
                );
            }
            return;
        }
        Ok(delivered) => delivered,
    };

    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {minted}"
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
    let read: Vec<String> = page["Records"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|record| {
            record["dynamodb"]["SequenceNumber"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        &read, delivered,
        "case {case:?}: expected the iterator to deliver {delivered:?}, got {read:?}"
    );
}

#[tokio::test]
async fn a_sequence_number_outside_the_addressed_shards_range_is_a_bad_argument() {
    let (engine, router) = build();
    let base = seconds_ago(600);
    let stream_arn = register(&engine, "ledger", base);
    // Two shards, so a number can be foreign to the shard it is asked on while still
    // being a number this stream really handed out: the parent is closed by the roll
    // and therefore advertises an end, the child stays open.
    let parent_records = vec![
        append(&engine, &stream_arn, "a-0", base),
        append(&engine, &stream_arn, "a-1", base + Duration::from_secs(1)),
    ];
    engine
        .state::<StreamsState>()
        .lock()
        .roll_shard(&stream_arn, base + Duration::from_secs(2))
        .expect("the stream has an open shard to roll");
    let child_records = vec![
        append(&engine, &stream_arn, "b-0", base + Duration::from_secs(3)),
        append(&engine, &stream_arn, "b-1", base + Duration::from_secs(4)),
    ];

    let listed = shards(&router, &stream_arn).await;
    assert_eq!(
        listed.len(),
        2,
        "the rolled stream must serve the parent and the child, got {listed:?}"
    );
    let parent = shard_id(&listed[0]);
    let child = shard_id(&listed[1]);
    // The range each row is judged against is the one the service itself advertises.
    assert_eq!(
        listed[0]["SequenceNumberRange"]["EndingSequenceNumber"].as_str(),
        Some(parent_records[1].as_str()),
        "a closed shard advertises where it ends, got {}",
        listed[0]
    );
    assert_eq!(
        listed[1]["SequenceNumberRange"].get("EndingSequenceNumber"),
        None,
        "an open shard advertises no end, got {}",
        listed[1]
    );

    let rows: Vec<(String, &str, &str, Option<&str>, Outcome)> = vec![
        (
            "all zeros on the open shard".to_string(),
            child.as_str(),
            "AT_SEQUENCE_NUMBER",
            Some(ALL_ZEROS),
            Err(("ValidationException", Some(INVALID_FOR_SHARD))),
        ),
        (
            "all zeros on the closed shard".to_string(),
            parent.as_str(),
            "AFTER_SEQUENCE_NUMBER",
            Some(ALL_ZEROS),
            Err(("ValidationException", Some(INVALID_FOR_SHARD))),
        ),
        (
            "the child's first number asked on the closed parent".to_string(),
            parent.as_str(),
            "AT_SEQUENCE_NUMBER",
            Some(child_records[0].as_str()),
            Err(("ValidationException", Some(INVALID_FOR_SHARD))),
        ),
        (
            "the parent's last number asked on the child".to_string(),
            child.as_str(),
            "AT_SEQUENCE_NUMBER",
            Some(parent_records[1].as_str()),
            Err(("ValidationException", Some(INVALID_FOR_SHARD))),
        ),
        (
            "a value of the modelled length that is not a number".to_string(),
            child.as_str(),
            "AT_SEQUENCE_NUMBER",
            Some("not-a-sequence-number"),
            Err((
                "ValidationException",
                Some("SequenceNumber must contain numeric characters only"),
            )),
        ),
        (
            "AT_SEQUENCE_NUMBER with no SequenceNumber".to_string(),
            child.as_str(),
            "AT_SEQUENCE_NUMBER",
            None,
            Err(("ValidationException", Some(SEQUENCE_NUMBER_NULL))),
        ),
        (
            "AFTER_SEQUENCE_NUMBER with no SequenceNumber".to_string(),
            parent.as_str(),
            "AFTER_SEQUENCE_NUMBER",
            None,
            Err(("ValidationException", Some(SEQUENCE_NUMBER_NULL))),
        ),
        // The controls: a number INSIDE the shard's range positions the iterator, and
        // an open shard's range has no upper end, so a number past its newest record
        // reads from where the next append will land.
        (
            "the child's own first number, AT".to_string(),
            child.as_str(),
            "AT_SEQUENCE_NUMBER",
            Some(child_records[0].as_str()),
            Ok(child_records.clone()),
        ),
        (
            "the child's own first number, AFTER".to_string(),
            child.as_str(),
            "AFTER_SEQUENCE_NUMBER",
            Some(child_records[0].as_str()),
            Ok(child_records[1..].to_vec()),
        ),
        (
            "the parent's own first number, AT".to_string(),
            parent.as_str(),
            "AT_SEQUENCE_NUMBER",
            Some(parent_records[0].as_str()),
            Ok(parent_records.clone()),
        ),
        (
            "the parent's own last number, AFTER".to_string(),
            parent.as_str(),
            "AFTER_SEQUENCE_NUMBER",
            Some(parent_records[1].as_str()),
            Ok(Vec::new()),
        ),
        (
            "all nines on the open shard".to_string(),
            child.as_str(),
            "AT_SEQUENCE_NUMBER",
            Some(ALL_NINES),
            Ok(Vec::new()),
        ),
    ];

    for (case, shard, iterator_type, sequence_number, expected) in rows {
        check_row(
            &router,
            &case,
            &stream_arn,
            shard,
            iterator_type,
            sequence_number,
            &expected,
        )
        .await;
    }
}

#[tokio::test]
async fn only_a_position_behind_the_trim_point_is_a_trimmed_data_error() {
    // One shard holding a record older than the documented 24 hour retention limit and
    // one inside it. The old record's position is INSIDE the shard's advertised range
    // and its record is gone, which is exactly what the trimmed refusal is for — and
    // the same shard still answers a number outside its range as a bad argument, so
    // the two classes are told apart on one shard rather than by which shard was asked.
    let (engine, router) = build();
    let stream_arn = register(&engine, "ledger", seconds_ago(30 * 60 * 60));
    let trimmed = append(&engine, &stream_arn, "old", seconds_ago(26 * 60 * 60));
    let retained = append(&engine, &stream_arn, "fresh", seconds_ago(60));

    let listed = shards(&router, &stream_arn).await;
    let shard = shard_id(&listed[0]);

    let rows: Vec<(&str, &str, Option<&str>, Outcome)> = vec![
        (
            "the trimmed record's own number",
            "AT_SEQUENCE_NUMBER",
            Some(trimmed.as_str()),
            // The code is the requirement here; the wording of this refusal is the
            // emulator's own, unlike the two validation messages above.
            Err(("TrimmedDataAccessException", None)),
        ),
        (
            "the retained record's own number",
            "AT_SEQUENCE_NUMBER",
            Some(retained.as_str()),
            Ok(vec![retained.clone()]),
        ),
        (
            "all zeros on the same shard",
            "AT_SEQUENCE_NUMBER",
            Some(ALL_ZEROS),
            Err(("ValidationException", Some(INVALID_FOR_SHARD))),
        ),
    ];

    for (case, iterator_type, sequence_number, expected) in rows {
        check_row(
            &router,
            case,
            &stream_arn,
            &shard,
            iterator_type,
            sequence_number,
            &expected,
        )
        .await;
    }
}

#[tokio::test]
async fn a_shard_that_retains_nothing_answers_every_number_in_its_range_as_trimmed() {
    // The limit case of the same rule: once a shard has trimmed everything it held it
    // retains no number to compare against, so it cannot tell a number it issued and
    // trimmed from one above it that it never issued — and both are positions whose
    // records are gone, which is what the trimmed refusal says. A number BELOW its
    // range is still outside the shard and still a bad argument.
    let (engine, router) = build();
    let stream_arn = register(&engine, "ledger", seconds_ago(40 * 60 * 60));
    let trimmed = append(&engine, &stream_arn, "only", seconds_ago(26 * 60 * 60));

    let listed = shards(&router, &stream_arn).await;
    let shard = shard_id(&listed[0]);
    // The trim has already happened by the time the shard is described: nothing is
    // retained, and the rows below are judged against that.
    let rows: Vec<(&str, Option<&str>, Outcome)> = vec![
        (
            "its own trimmed number",
            Some(trimmed.as_str()),
            Err(("TrimmedDataAccessException", None)),
        ),
        (
            "a higher number it never issued",
            Some(ALL_NINES),
            Err(("TrimmedDataAccessException", None)),
        ),
        (
            "a number below its range",
            Some(ALL_ZEROS),
            Err(("ValidationException", Some(INVALID_FOR_SHARD))),
        ),
    ];

    for (case, sequence_number, expected) in rows {
        check_row(
            &router,
            case,
            &stream_arn,
            &shard,
            "AT_SEQUENCE_NUMBER",
            sequence_number,
            &expected,
        )
        .await;
    }
}
