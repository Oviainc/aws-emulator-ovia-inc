// SPDX-License-Identifier: CC0-1.0

//! The WHOLE stream-driven pipeline, end to end over the wire with no client action after the
//! record is written: a record put to an Amazon Kinesis data stream reaches the function an
//! event source mapping maps, and the AWS call that function's own handler makes is served by
//! this emulator, so its effect is readable through the called service's own API.
//!
//! The RULE asserted over the table below: once a mapping on a Kinesis data stream is
//! `Enabled`, Lambda itself reads that stream's shards from the position the mapping's
//! `StartingPosition` fixes — the oldest retained record for `TRIM_HORIZON`, "The position in
//! a stream from which to start reading", and what arrives from the mapping's own creation on
//! for `LATEST` — and invokes the mapped function with one envelope per record ("Lambda reads
//! items from the event source and invokes the function"), each carrying `eventSource`
//! `aws:kinesis`, the stream's ARN as `eventSourceARN` and that record's own base64 `data`,
//! `partitionKey` and `approximateArrivalTimestamp`; and the AWS SDK call the running handler
//! then makes is dispatched into this same emulator, so the work it reported is work the
//! called service really did.
//!
//! Nothing here invokes the function: the only data-plane call this suite makes is the one
//! that writes the record. Every item it reads back out of Amazon DynamoDB was written by the
//! handler, out of the envelope the poller handed it, so a mapping that reports healthy and
//! delivers nothing leaves the sink empty and the row fails.
//!
//! The rows differ along what the rule generalises over: which `StartingPosition` fixes the
//! mapping's position, whether the records were already in the shard when the mapping was
//! created or arrive afterwards, how many records there are, which of the stream's two write
//! calls puts them there, how many shards the stream has, and the `BatchSize` they are read
//! in. Every expected value is computed here from what the row wrote, never read back out of
//! the answer being checked.
//!
//! The Lambda, Kinesis and mapping helpers are the ones `kinesis_event_source_delivery.rs`
//! publishes, and the role fixture is `event_source_mapping_role_access.rs`'s, so every suite
//! over this surface drives the same wire.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    STARTING_POSITION_LATEST, STARTING_POSITION_TRIM_HORIZON,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{allow, given_role, logs_only, KINESIS_READS};
use crate::kinesis_event_source_delivery::{
    get_mapping, given_stream, kinesis, lambda, LOOK_AGAIN, MAPPINGS_URI,
};
use crate::synchronous_invoke_run::package;

/// The Region every call below signs for, and the Region every ARN carries.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `X-Amz-Target` prefix of the DynamoDB service (its `OPERATIONS.md`, "Wire protocol").
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// The runtime every function below is created with, and the module and attribute its package
/// exposes its handler as.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long one invocation of this handler is given, as the function's own `Timeout`: well
/// past what a run needs, because what is under test is the delivery and not how fast an
/// interpreter gets through a handler.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long a wait for work nothing asked for — a mapping settling, a batch read, the
/// handler's own write landing — is given before the row is a failure. Well above the
/// poller's interval and the state-settle span, and above one interpreted run of the handler,
/// so a row fails only when the work did not happen at all.
const BUDGET: Duration = Duration::from_secs(90);

/// How long this suite leaves between a record it puts BEFORE a mapping is created and the
/// create itself: a shard position is carried in milliseconds, so the two instants have to be
/// distinguishable at that resolution for a row to be about the position at all. It is this
/// test's own separation of two of its calls, not a wait on the emulator's behalf.
const POSITION_SEPARATION: Duration = Duration::from_millis(50);

/// The one `State` a mapping that is reading rests in
/// (`smithy_query(query="shape", shape="EventSourceMappingConfiguration", member="State")`).
const STATE_ENABLED: &str = "Enabled";

/// The attribute the sink item is keyed by, the attribute the handler stamps it with, and the
/// value it stamps there. None of the three is read by this emulator: they are the handler's
/// own, passed to it through its environment and asserted here.
const KEY_ATTRIBUTE: &str = "pk";
/// See [`KEY_ATTRIBUTE`].
const SOURCE_ATTRIBUTE: &str = "source";
/// See [`KEY_ATTRIBUTE`].
const SOURCE_MARKER: &str = "kinesis-stream";

/// What a sink key is spelled as: the handler builds it from the record's own decoded data.
fn sink_key(key: &str) -> String {
    format!("kinesis-{key}")
}

/// The partition key a record carrying `key` is written under.
fn partition_key(key: &str) -> String {
    format!("part-{key}")
}

/// Lambda, Kinesis, DynamoDB and IAM over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the records are put where a client puts them, the role Lambda reads the
/// stream as is the one IAM holds, and the handler's own SDK call is dispatched back into
/// this same router — so the table this suite reads is the table the handler wrote.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// One DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`) that must answer 200.
async fn dynamodb(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{DYNAMODB_TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/dynamodb/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: DynamoDB {operation} answered {status} {parsed}"
    );
    parsed
}

/// The sink table the handler writes, waited out to `ACTIVE` — "you can perform read and
/// write operations only on an ACTIVE table" — and answered as its ARN.
async fn given_table(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = dynamodb(
        router,
        case,
        "CreateTable",
        &json!({
            "TableName": name,
            "KeySchema": [{ "AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH" }],
            "AttributeDefinitions": [
                { "AttributeName": KEY_ATTRIBUTE, "AttributeType": "S" },
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let arn = created["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no TableArn in {created}"))
        .to_string();
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = dynamodb(router, case, "DescribeTable", &json!({ "TableName": name })).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The handler every row runs: for each record of the batch it was handed it decodes that
/// record's own data and writes ONE item to the sink table through the SDK the runtime
/// supplies, stamping it with the envelope members the record arrived with.
///
/// Every value it writes comes from the envelope or from its own environment, so an item in
/// the table is evidence that the record reached the handler in the shape Lambda hands a
/// Kinesis batch over in.
fn handler_source() -> String {
    format!(
        r#"
import base64
import json
import os

import boto3

TABLE = os.environ["SINK_TABLE"]
MARKER = os.environ["SOURCE_MARKER"]
CLIENT = boto3.client("dynamodb")


def lambda_handler(event, context):
    written = []
    for record in event["Records"]:
        if record["eventSource"] != "aws:kinesis":
            raise ValueError("eventSource is " + repr(record["eventSource"]))
        stream_record = record["kinesis"]
        payload = json.loads(base64.b64decode(stream_record["data"]).decode())
        CLIENT.put_item(
            TableName=TABLE,
            Item={{
                "{KEY_ATTRIBUTE}": {{"S": "kinesis-" + payload["key"]}},
                "{SOURCE_ATTRIBUTE}": {{"S": MARKER}},
                "partitionKey": {{"S": stream_record["partitionKey"]}},
                "streamArn": {{"S": record["eventSourceARN"]}},
                "arrival": {{
                    "N": "%d" % int(stream_record["approximateArrivalTimestamp"])
                }},
            }},
        )
        written.append(payload["key"])
    return {{"written": written}}
"#
    )
}

/// A function whose package RUNS [`handler_source`], running as `role_arn` and told through
/// its environment which table to write, waited out to `Active`.
async fn given_handler_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    role_arn: &str,
    table: &str,
) {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Environment": {
                "Variables": { "SINK_TABLE": table, "SOURCE_MARKER": SOURCE_MARKER },
            },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&handler_source())),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateFunction answered {status} {created}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, read) = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetFunctionConfiguration answered {status} {read}"
        );
        if read["State"] == json!("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the function never became Active: {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Put one record per entry of `keys` on the stream `stream` names, each carrying
/// `{"key": <entry>}` as its data under its own partition key — one `PutRecord` call per
/// record when `one_at_a_time`, and one `PutRecords` call for all of them otherwise. A record
/// is a record of the shard however it was written.
async fn put_keys(
    router: &MultiServiceRouter,
    case: &str,
    stream: &str,
    keys: &[&str],
    one_at_a_time: bool,
) {
    if keys.is_empty() {
        return;
    }
    let data = |key: &str| {
        base64::engine::general_purpose::STANDARD.encode(json!({ "key": key }).to_string())
    };
    if one_at_a_time {
        for key in keys {
            let (status, written) = kinesis(
                router,
                "PutRecord",
                &json!({
                    "StreamName": stream,
                    "Data": data(key),
                    "PartitionKey": partition_key(key),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: PutRecord answered {status} {written}"
            );
        }
        return;
    }
    let records: Vec<Value> = keys
        .iter()
        .map(|key| json!({ "Data": data(key), "PartitionKey": partition_key(key) }))
        .collect();
    let (status, written) = kinesis(
        router,
        "PutRecords",
        &json!({ "StreamName": stream, "Records": records }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRecords answered {status} {written}"
    );
    assert_eq!(
        written["FailedRecordCount"],
        json!(0),
        "case {case:?}: every record must be placed, got {written}"
    );
}

/// Wait until the mapping `uuid` names reports `State` `Enabled`, which is the state a
/// mapping reads in.
async fn wait_until_enabled(router: &MultiServiceRouter, case: &str, uuid: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let read = get_mapping(router, case, uuid).await;
        if read["State"] == json!(STATE_ENABLED) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the mapping never reported {STATE_ENABLED}: {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The item the sink table holds under `key`, read with `ConsistentRead` as a client that
/// polls for the effect reads it.
async fn sink_item(router: &MultiServiceRouter, case: &str, table: &str, key: &str) -> Value {
    let read = dynamodb(
        router,
        case,
        "GetItem",
        &json!({
            "TableName": table,
            "Key": { KEY_ATTRIBUTE: { "S": key } },
            "ConsistentRead": true,
        }),
    )
    .await;
    read["Item"].clone()
}

/// Wait until the sink table holds an item under `key`, answering it.
async fn wait_for_sink_item(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    key: &str,
) -> Value {
    let deadline = Instant::now() + BUDGET;
    loop {
        let item = sink_item(router, case, table, key).await;
        if !item.is_null() {
            return item;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the mapping must invoke its function, whose handler writes the sink \
             item under {key:?}, but no item ever appeared"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every key the sink table holds an item under, read through DynamoDB's own `Scan`.
async fn sink_keys(router: &MultiServiceRouter, case: &str, table: &str) -> Vec<String> {
    let scanned = dynamodb(
        router,
        case,
        "Scan",
        &json!({ "TableName": table, "ConsistentRead": true }),
    )
    .await;
    let mut keys: Vec<String> = scanned["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item[KEY_ATTRIBUTE]["S"].as_str().map(str::to_string))
        .collect();
    keys.sort();
    keys
}

/// One row: how the mapping is shaped, and which records are already in the shard when it is
/// created as against put afterwards.
struct Row {
    case: &'static str,
    /// The `StartingPosition` the mapping is created with.
    starting_position: &'static str,
    /// How many shards the stream is created with.
    shards: i64,
    /// The `BatchSize` the mapping reads with.
    batch_size: i64,
    /// The keys of the records put BEFORE the mapping is created.
    before: &'static [&'static str],
    /// The keys of the records put once the mapping reports `Enabled`.
    after: &'static [&'static str],
    /// Whether the records are put one `PutRecord` call at a time rather than as one
    /// `PutRecords` batch.
    put_one_at_a_time: bool,
}

impl Row {
    /// The keys whose records this row's mapping must deliver: everything in the shard for a
    /// mapping positioned at the oldest retained record, and only what arrived after the
    /// mapping for one positioned at the newest.
    fn delivered(&self) -> Vec<&'static str> {
        let mut keys: Vec<&'static str> = Vec::new();
        if self.starting_position == STARTING_POSITION_TRIM_HORIZON {
            keys.extend(self.before);
        }
        keys.extend(self.after);
        keys
    }
}

/// RULE: an `Enabled` mapping on a Kinesis data stream reads that stream from the position its
/// `StartingPosition` fixes and invokes the mapped function with the `aws:kinesis` envelope of
/// each record it read, and the SDK call that invocation's handler makes is served by this
/// emulator — so the item the handler wrote is in the table a client reads.
#[tokio::test]
async fn a_kinesis_mapping_drives_its_handlers_own_write_into_dynamodb() {
    let rows = [
        // The recorded shape: a single-shard stream, a TRIM_HORIZON mapping created over it
        // while it is empty, and one record put afterwards with PutRecord.
        Row {
            case: "trim-horizon-one-record-put-after-the-mapping",
            starting_position: STARTING_POSITION_TRIM_HORIZON,
            shards: 1,
            batch_size: 10,
            before: &[],
            after: &["solo"],
            put_one_at_a_time: true,
        },
        // Records ALREADY in the shard when the mapping is created: the oldest retained
        // record is where TRIM_HORIZON starts, so the backlog is delivered too.
        Row {
            case: "trim-horizon-reads-the-backlog-and-what-follows-it",
            starting_position: STARTING_POSITION_TRIM_HORIZON,
            shards: 1,
            batch_size: 2,
            before: &["older-0", "older-1", "older-2"],
            after: &["newer"],
            put_one_at_a_time: false,
        },
        // A mapping positioned at the newest record, over several shards and reading one
        // record at a time: what arrives after it is delivered, and the backlog is not.
        Row {
            case: "latest-reads-what-arrives-after-the-mapping-over-two-shards",
            starting_position: STARTING_POSITION_LATEST,
            shards: 2,
            batch_size: 1,
            before: &["skipped"],
            after: &["fresh-0", "fresh-1", "fresh-2"],
            put_one_at_a_time: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        // The names are indexed rather than spelled out of the case, which would run past
        // the length `FunctionName` and `RoleName` admit.
        let stream = format!("sink-stream-{index}");
        let table = format!("sink-table-{index}");
        let function = format!("sink-writer-{index}");

        let stream_arn = given_stream(&router, case, &stream, row.shards).await;
        let table_arn = given_table(&router, case, &table).await;
        // Lambda reads the stream acting as the function's execution role, and the handler
        // writes the table as itself: the role carries both grants, as a client that wires
        // this pipeline together gives it.
        let role = given_role(
            &router,
            case,
            &format!("sink-role-{index}"),
            Some(json!([
                logs_only(),
                allow("kinesis", &KINESIS_READS, &stream_arn),
                allow("dynamodb", &["PutItem"], &table_arn),
            ])),
        )
        .await;
        given_handler_function(&router, case, &function, &role, &table).await;

        // The records already in the shard when the mapping is created.
        put_keys(&router, case, &stream, row.before, row.put_one_at_a_time).await;
        if !row.before.is_empty() {
            tokio::time::sleep(POSITION_SEPARATION).await;
        }

        let (status, created) = lambda(
            &router,
            "POST",
            MAPPINGS_URI,
            &json!({
                "FunctionName": function,
                "EventSourceArn": stream_arn,
                "StartingPosition": row.starting_position,
                "BatchSize": row.batch_size,
                "Enabled": true,
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateEventSourceMapping answered {status} {created}"
        );
        let uuid = created["UUID"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {created}"))
            .to_string();
        wait_until_enabled(&router, case, &uuid).await;

        // The record this check writes, and the window its arrival instant has to fall in.
        let put_from = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        put_keys(&router, case, &stream, row.after, row.put_one_at_a_time).await;

        // Nothing below invokes the function: every item read back was written by the
        // handler the poller handed the record to.
        for key in row.delivered() {
            let item = wait_for_sink_item(&router, case, &table, &sink_key(key)).await;
            assert_eq!(
                item[KEY_ATTRIBUTE],
                json!({ "S": sink_key(key) }),
                "case {case:?}: expected the item to be keyed by {:?}, got {item}",
                sink_key(key)
            );
            assert_eq!(
                item[SOURCE_ATTRIBUTE],
                json!({ "S": SOURCE_MARKER }),
                "case {case:?}: expected the handler's own {SOURCE_ATTRIBUTE} attribute \
                 {SOURCE_MARKER:?}, got {item}"
            );
            // The envelope's own members, as the handler read them out of the record it was
            // handed: the stream the record came from, and the partition key it was written
            // under.
            assert_eq!(
                item["streamArn"],
                json!({ "S": stream_arn }),
                "case {case:?}: expected the envelope to name {stream_arn:?} as its \
                 eventSourceARN, got {item}"
            );
            assert_eq!(
                item["partitionKey"],
                json!({ "S": partition_key(key) }),
                "case {case:?}: expected the record's own partition key {:?}, got {item}",
                partition_key(key)
            );
            let arrival: f64 = item["arrival"]["N"]
                .as_str()
                .unwrap_or_default()
                .parse()
                .unwrap_or_default();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            assert!(
                arrival > 0.0 && arrival <= now,
                "case {case:?}: the record's approximateArrivalTimestamp must be an instant no \
                 later than now ({now}), got {arrival} in {item}"
            );
            if row.after.contains(&key) {
                assert!(
                    arrival >= put_from - 1.0,
                    "case {case:?}: a record put at {put_from} cannot have arrived at \
                     {arrival}, in {item}"
                );
            }
        }

        // And nothing else: a position that skips a record leaves no item for it, and no
        // item is written for a record nobody put.
        let mut expected: Vec<String> = row.delivered().iter().map(|key| sink_key(key)).collect();
        expected.sort();
        let held = sink_keys(&router, case, &table).await;
        assert_eq!(
            held, expected,
            "case {case:?}: expected the sink to hold exactly {expected:?}, got {held:?}"
        );
    }
}
