// SPDX-License-Identifier: CC0-1.0

//! What an event source mapping on an Amazon DynamoDB table's STREAM does with NO client
//! action at all, end to end over the wire, with DynamoDB, DynamoDB Streams and CloudWatch
//! mounted beside Lambda on ONE engine — so the item is written where a client writes it
//! and what the mapping did is read back where a client reads it.
//!
//! The RULE asserted over the tables below: once a mapping whose `EventSourceArn` names a
//! table's stream is `Enabled`, Lambda reads that stream's shards from the position the
//! mapping carries and INVOKES the mapped function with one envelope per stream record
//! ("Lambda reads items from the event source and invokes the function"), each envelope
//! naming the stream it came from (`eventSource` `aws:dynamodb`, `eventSourceARN`) and
//! carrying the change itself under `dynamodb` — which mutation it was (`eventName`), the
//! item's `Keys`, and the images the stream's `StreamViewType` captures. The attempt at the
//! batch is reported as the mapping's own `LastProcessingResult`, "the result of the event
//! source mapping's last processing attempt", and the mapping is checkpointed past only the
//! records the attempt processed.
//!
//! Every row's verdict is computed INSIDE the handler from the envelope it was handed: a
//! handler that was never invoked, or invoked with an envelope of the wrong shape, reports
//! nothing or raises, and the row fails.
//!
//! The first suite's rows differ along what the rule generalises over: which mutation the
//! item write is (an insert, a change, a removal), which `StreamViewType` the table's
//! stream captures (each of the four the model declares), which `StartingPosition` fixes
//! the mapping's position, how many items are written, the `BatchSize` they are read in,
//! whether the mapping asks for its function's report of which records failed, and how the
//! handler ends (a return, a report, a raise).
//!
//! The second suite is the flow a client wires this up for: the handler reacts to the
//! change by writing a DERIVED item into a second table through the AWS SDK, and that item
//! is read back through `GetItem` — so the record's own values have to have reached the
//! handler for the row to pass.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call below signs for, and the region every ARN carries.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// Where `CreateFunction` is bound, and where `CreateEventSourceMapping` is.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";
/// See [`FUNCTIONS_URI`].
const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// The `X-Amz-Target` prefix of the DynamoDB service (its `OPERATIONS.md`, "Wire
/// protocol").
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// The `X-Amz-Target` prefix of the CloudWatch service.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The runtime every function below is created with, and the handler its package exposes.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// The one value `FunctionResponseType` admits
/// (`smithy_query(query="shape", shape="FunctionResponseType")`).
const REPORT_BATCH_ITEM_FAILURES: &str = "ReportBatchItemFailures";

/// The `StartingPosition` values a DynamoDB stream source may be read from: "AT_TIMESTAMP
/// is supported only for Amazon Kinesis streams, Amazon DocumentDB, Amazon MSK, and
/// self-managed Apache Kafka" (`CreateEventSourceMappingInput$StartingPosition`), so the
/// other two of the model's `EventSourcePosition` enum are what this source admits.
const TRIM_HORIZON: &str = "TRIM_HORIZON";
/// See [`TRIM_HORIZON`].
const LATEST: &str = "LATEST";

/// The `StreamViewType` values the model declares
/// (`smithy_query(query="shape", shape="StreamViewType")`, DynamoDB): what the stream
/// captures for each change.
const KEYS_ONLY: &str = "KEYS_ONLY";
/// See [`KEYS_ONLY`].
const NEW_IMAGE: &str = "NEW_IMAGE";
/// See [`KEYS_ONLY`].
const OLD_IMAGE: &str = "OLD_IMAGE";
/// See [`KEYS_ONLY`].
const NEW_AND_OLD_IMAGES: &str = "NEW_AND_OLD_IMAGES";

/// The `OperationType` a stream record reports for each kind of change
/// (`smithy_query(query="shape", shape="OperationType")`, DynamoDB Streams).
const INSERT: &str = "INSERT";
/// See [`INSERT`].
const MODIFY: &str = "MODIFY";
/// See [`INSERT`].
const REMOVE: &str = "REMOVE";

/// What `LastProcessingResult` reports for an attempt that processed the batch, and for one
/// that did not (`EventSourceMappingConfiguration$LastProcessingResult`).
const RESULT_OK: &str = "OK";
/// See [`RESULT_OK`].
const RESULT_FUNCTION_CALL_FAILED: &str = "PROBLEM: Function call failed";

/// The `State` a mapping a client has deleted reports
/// (`EventSourceMappingConfiguration$State`).
const DELETING: &str = "Deleting";

/// The namespace Lambda's own metrics live in, the series one admitted invocation is
/// counted on and the dimension it is counted per (the AWS Lambda metric reference).
const NAMESPACE: &str = "AWS/Lambda";
/// See [`NAMESPACE`].
const INVOCATIONS: &str = "Invocations";
/// See [`NAMESPACE`].
const FUNCTION_DIMENSION: &str = "FunctionName";

/// How long a wait for work the mapping has to do is given before the row is a failure.
/// Well above the table's and the stream's own settle spans and the poller's interval, so a
/// row fails only when the mapping did not do the work at all.
const BUDGET: Duration = Duration::from_secs(45);

/// How long between two looks while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// The partition key every table below is keyed on.
const KEY: &str = "pk";

/// The key value whose record the handler reports as one it did not process, and the one
/// that makes it raise instead. Nothing in the emulator reads either: they are read by the
/// handler, out of the record's own keys.
const POISON: &str = "poison";
/// See [`POISON`].
const RAISE: &str = "raise";

/// Lambda, DynamoDB, DynamoDB Streams and CloudWatch on one engine: the mapping's event
/// source is a table's stream and the series it publishes is CloudWatch's, so all of them
/// are reached in-process — the shape `core/router/src/lib.rs` mounts them in.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// A SigV4 credential scope naming `signing_name` in [`REGION`]. Nothing in it is
/// verified: the harness turns request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=not-verified"
    )
}

/// One Lambda call, over its own restJson1 routes.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One RPC call against the service whose signing name is `signing_name`.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    content_type: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", content_type)
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One DynamoDB call.
async fn dynamodb(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(
        router,
        "dynamodb",
        DYNAMODB_TARGET_PREFIX,
        "application/x-amz-json-1.0",
        operation,
        body,
    )
    .await
}

/// One CloudWatch call.
async fn cloudwatch(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(
        router,
        "monitoring",
        CLOUDWATCH_TARGET_PREFIX,
        "application/x-amz-json-1.0",
        operation,
        body,
    )
    .await
}

/// A table keyed on [`KEY`], with a stream capturing `view_type` when one is named, waited
/// out to `ACTIVE`; answers the table's `LatestStreamArn` (empty for a table with no
/// stream).
async fn given_table(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    view_type: Option<&str>,
) -> String {
    let mut request = json!({
        "TableName": name,
        "AttributeDefinitions": [{ "AttributeName": KEY, "AttributeType": "S" }],
        "KeySchema": [{ "AttributeName": KEY, "KeyType": "HASH" }],
        "BillingMode": "PAY_PER_REQUEST",
    });
    if let Some(view_type) = view_type {
        request["StreamSpecification"] =
            json!({ "StreamEnabled": true, "StreamViewType": view_type });
    }
    let (status, created) = dynamodb(router, "CreateTable", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTable answered {status} {created}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, read) = dynamodb(router, "DescribeTable", &json!({ "TableName": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTable answered {status} {read}"
        );
        let description = &read["Table"];
        if description["TableStatus"] == json!("ACTIVE") {
            return description["LatestStreamArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory
/// the way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// A function whose package RUNS `source`, told `environment` about what it must see,
/// waited out to `Active`.
async fn given_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    source: &str,
    environment: Value,
) {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 30,
            "Environment": { "Variables": environment },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
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

/// Create the mapping between `function` and `stream_arn`, answering its UUID.
async fn given_mapping(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    stream_arn: &str,
    starting_position: &str,
    batch_size: i64,
    reads_report: bool,
) -> String {
    let mut request = json!({
        "FunctionName": function,
        "EventSourceArn": stream_arn,
        "StartingPosition": starting_position,
        "BatchSize": batch_size,
        "Enabled": true,
    });
    if reads_report {
        request["FunctionResponseTypes"] = json!([REPORT_BATCH_ITEM_FAILURES]);
    }
    let (status, created) = lambda(router, "POST", MAPPINGS_URI, &request).await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateEventSourceMapping answered {status} {created}"
    );
    assert_eq!(
        created["EventSourceArn"],
        json!(stream_arn),
        "case {case:?}: the mapping must name the stream it was created for, got {created}"
    );
    created["UUID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {created}"))
        .to_string()
}

/// `GetEventSourceMapping` on one mapping.
async fn get_mapping(router: &MultiServiceRouter, case: &str, uuid: &str) -> Value {
    let (status, read) = lambda(
        router,
        "GET",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetEventSourceMapping answered {status} {read}"
    );
    read
}

/// `DeleteEventSourceMapping` on one mapping, which is what a client tears the trigger down
/// with: "When you delete an event source mapping, it enters a `Deleting` state"
/// (`EventSourceMappingConfiguration$State`), so the mapping reports that state and reads
/// nothing more off its source.
async fn stop_mapping(router: &MultiServiceRouter, case: &str, uuid: &str) {
    let (status, deleted) = lambda(
        router,
        "DELETE",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: DeleteEventSourceMapping answered {status} {deleted}"
    );
    assert_eq!(
        deleted["State"],
        json!(DELETING),
        "case {case:?}: a deleted mapping must report that it is being deleted, got {deleted}"
    );
}

/// Wait until the mapping reports `expected` as its `LastProcessingResult`, answering every
/// result it reported on the way.
async fn results_until(
    router: &MultiServiceRouter,
    case: &str,
    uuid: &str,
    expected: &str,
) -> Vec<String> {
    let deadline = Instant::now() + BUDGET;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let read = get_mapping(router, case, uuid).await;
        if let Some(result) = read["LastProcessingResult"].as_str() {
            if seen.last().map(String::as_str) != Some(result) {
                seen.push(result.to_string());
            }
            if result == expected {
                return seen;
            }
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the mapping must come to report {expected:?} as its \
             LastProcessingResult, saw {seen:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// How many invocations CloudWatch holds for `function`, summed over a window around now.
async fn invocation_count(router: &MultiServiceRouter, case: &str, function: &str) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": INVOCATIONS,
            "Dimensions": [{ "Name": FUNCTION_DIMENSION, "Value": function }],
            "StartTime": now - 600.0,
            "EndTime": now + 600.0,
            "Period": 60,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics answered {status} {stats}"
    );
    stats["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .sum()
}

/// The handler the first suite runs, with the markers above bound to the names it reads
/// them by — so there is ONE spelling of each on both sides of the interpreter.
///
/// It asserts the shape of the envelope Lambda hands a DynamoDB stream batch over as — one
/// record per envelope, naming its source and stream, carrying the change's name and the
/// item's keys, and carrying exactly the images the stream's view type captures — and
/// decides what to report from the record's own KEY, so the report is a consequence of the
/// record having arrived.
fn inspecting_handler() -> String {
    format!(
        r#"
import os

STREAM_ARN = os.environ["STREAM_ARN"]
VIEW_TYPE = os.environ["VIEW_TYPE"]
KEY = "{KEY}"
POISON = "{POISON}"
RAISE = "{RAISE}"


def lambda_handler(event, context):
    records = event["Records"]
    if not records:
        raise ValueError("a batch was handed over holding no record")
    failures = []
    for record in records:
        if record["eventSource"] != "aws:dynamodb":
            raise ValueError("eventSource is " + repr(record["eventSource"]))
        if record["eventSourceARN"] != STREAM_ARN:
            raise ValueError("eventSourceARN is " + repr(record["eventSourceARN"]))
        if not record.get("eventID"):
            raise ValueError("a record carries no eventID")
        if not record.get("eventVersion"):
            raise ValueError("a record carries no eventVersion")
        if record["eventName"] not in ("{INSERT}", "{MODIFY}", "{REMOVE}"):
            raise ValueError("eventName is " + repr(record["eventName"]))
        change = record["dynamodb"]
        if change["StreamViewType"] != VIEW_TYPE:
            raise ValueError("StreamViewType is " + repr(change["StreamViewType"]))
        sequence_number = change["SequenceNumber"]
        if not sequence_number:
            raise ValueError("a record carries no SequenceNumber")
        key = change["Keys"][KEY]["S"]
        if not key:
            raise ValueError("a record carries no key")
        carries_new = VIEW_TYPE in ("{NEW_IMAGE}", "{NEW_AND_OLD_IMAGES}")
        carries_old = VIEW_TYPE in ("{OLD_IMAGE}", "{NEW_AND_OLD_IMAGES}")
        has_new = "NewImage" in change
        has_old = "OldImage" in change
        if has_new and not carries_new:
            raise ValueError(VIEW_TYPE + " must capture no NewImage")
        if has_old and not carries_old:
            raise ValueError(VIEW_TYPE + " must capture no OldImage")
        if carries_new and record["eventName"] != "{REMOVE}":
            if not has_new:
                raise ValueError(VIEW_TYPE + " must capture a NewImage")
            if change["NewImage"][KEY]["S"] != key:
                raise ValueError("NewImage names another item than the keys do")
        if carries_old and record["eventName"] in ("{MODIFY}", "{REMOVE}"):
            if not has_old:
                raise ValueError(VIEW_TYPE + " must capture an OldImage")
        if key.startswith(RAISE):
            raise RuntimeError("the handler cannot process " + key)
        if key.startswith(POISON):
            failures.append({{"itemIdentifier": sequence_number}})
    return {{"batchItemFailures": failures}}
"#
    )
}

/// What one item write of a row is, and what change it makes to the table.
#[derive(Debug, Clone, Copy)]
enum Write {
    /// `PutItem` of an item that was not there: an `INSERT`.
    Insert,
    /// `PutItem` over an item that was: a `MODIFY`.
    Overwrite,
    /// `UpdateItem` on an item that was there: a `MODIFY`.
    Update,
    /// `DeleteItem`: a `REMOVE`.
    Delete,
}

/// Make one change to `table` for the item `key` names.
async fn write_item(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    key: &str,
    write: Write,
) {
    let (operation, body) = match write {
        Write::Insert | Write::Overwrite => (
            "PutItem",
            json!({
                "TableName": table,
                "Item": { KEY: { "S": key }, "note": { "S": format!("{write:?}") } },
            }),
        ),
        Write::Update => (
            "UpdateItem",
            json!({
                "TableName": table,
                "Key": { KEY: { "S": key } },
                "UpdateExpression": "SET #n = :v",
                "ExpressionAttributeNames": { "#n": "note" },
                "ExpressionAttributeValues": { ":v": { "S": "updated" } },
            }),
        ),
        Write::Delete => (
            "DeleteItem",
            json!({ "TableName": table, "Key": { KEY: { "S": key } } }),
        ),
    };
    let (status, written) = dynamodb(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} answered {status} {written}"
    );
}

/// One row of the first suite: the stream's shape, the writes made to the table, and what
/// the mapping must report once the handler has run on the records they produced.
struct Row {
    case: &'static str,
    /// What the table's stream captures.
    view_type: &'static str,
    /// The `StartingPosition` the mapping is created with.
    starting_position: &'static str,
    /// The key and the change of each write, in order.
    writes: &'static [(&'static str, Write)],
    /// The `BatchSize` the mapping reads with.
    batch_size: i64,
    /// Whether the mapping asks for its function's report of which records failed.
    reads_report: bool,
    /// The `LastProcessingResult` the mapping must come to report.
    expected_result: &'static str,
}

/// RULE: an `Enabled` mapping on a DynamoDB table's stream reads that stream's records and
/// invokes the mapped function with them, and what the handler makes of each record it was
/// handed is what the mapping reports as the result of the attempt.
#[tokio::test]
async fn a_dynamodb_stream_mapping_invokes_its_function_with_the_records_it_read() {
    let rows = vec![
        // The recorded shape: an INSERT on a NEW_AND_OLD_IMAGES stream read from the
        // oldest retained record.
        Row {
            case: "an-insert-with-both-images",
            view_type: NEW_AND_OLD_IMAGES,
            starting_position: TRIM_HORIZON,
            writes: &[("trigger-insert", Write::Insert)],
            batch_size: 100,
            reads_report: true,
            expected_result: RESULT_OK,
        },
        // Every kind of change, on a stream that captures only the new image, read one
        // record at a time from the newest position.
        Row {
            case: "insert-modify-and-remove-with-the-new-image",
            view_type: NEW_IMAGE,
            starting_position: LATEST,
            writes: &[
                ("item-a", Write::Insert),
                ("item-a", Write::Overwrite),
                ("item-b", Write::Insert),
                ("item-a", Write::Delete),
            ],
            batch_size: 1,
            reads_report: true,
            expected_result: RESULT_OK,
        },
        // A stream that captures neither image: the record carries the keys alone.
        Row {
            case: "a-keys-only-stream-carries-no-image",
            view_type: KEYS_ONLY,
            starting_position: TRIM_HORIZON,
            writes: &[("item-c", Write::Insert), ("item-c", Write::Update)],
            batch_size: 10,
            reads_report: false,
            expected_result: RESULT_OK,
        },
        // A stream that captures only the old image, over an update and a delete.
        Row {
            case: "an-old-image-stream-over-a-change-and-a-removal",
            view_type: OLD_IMAGE,
            starting_position: TRIM_HORIZON,
            writes: &[
                ("item-d", Write::Insert),
                ("item-d", Write::Update),
                ("item-d", Write::Delete),
            ],
            batch_size: 100,
            reads_report: false,
            expected_result: RESULT_OK,
        },
        // The handler names the batch's FIRST record as one it did not process, so the
        // shard stays where it is and the mapping reports the function-call problem.
        Row {
            case: "the-first-record-is-not-processed",
            view_type: NEW_AND_OLD_IMAGES,
            starting_position: TRIM_HORIZON,
            writes: &[(POISON, Write::Insert), ("item-e", Write::Insert)],
            batch_size: 100,
            reads_report: true,
            expected_result: RESULT_FUNCTION_CALL_FAILED,
        },
        // A handler that RAISES on a record answers no report at all: the invocation
        // failed, so no record of the batch is processed.
        Row {
            case: "the-handler-raises-on-a-record",
            view_type: NEW_IMAGE,
            starting_position: TRIM_HORIZON,
            writes: &[("item-f", Write::Insert), (RAISE, Write::Insert)],
            batch_size: 100,
            reads_report: true,
            expected_result: RESULT_FUNCTION_CALL_FAILED,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = emulator();
        let table = format!("source-{case}");
        let function = format!("consume-{case}");
        let stream_arn = given_table(&router, case, &table, Some(row.view_type)).await;
        assert!(
            stream_arn.contains("/stream/"),
            "case {case:?}: a stream-enabled table must report its stream ARN, got \
             {stream_arn:?}"
        );
        given_function(
            &router,
            case,
            &function,
            &inspecting_handler(),
            json!({ "STREAM_ARN": stream_arn, "VIEW_TYPE": row.view_type }),
        )
        .await;
        let uuid = given_mapping(
            &router,
            case,
            &function,
            &stream_arn,
            row.starting_position,
            row.batch_size,
            row.reads_report,
        )
        .await;

        for (key, write) in row.writes {
            write_item(&router, case, &table, key, *write).await;
        }

        // The records reach the handler, and what it made of them is what the mapping
        // reports: no other component of this emulator decides this value.
        let seen = results_until(&router, case, &uuid, row.expected_result).await;
        assert_eq!(
            seen.last().map(String::as_str),
            Some(row.expected_result),
            "case {case:?}: expected the mapping to report {:?}, saw {seen:?}",
            row.expected_result
        );

        // And the invocations it made are the function's own, metered where a client reads
        // them: a function nothing invoked has none.
        let invocations = invocation_count(&router, case, &function).await;
        assert!(
            invocations >= 1.0,
            "case {case:?}: the batch read off the stream must be handed to {function} as an \
             invocation, but {NAMESPACE}/{INVOCATIONS} for it summed to {invocations}"
        );

        // The trigger is torn down as a client tears one down, which is also what stops the
        // mapping from reading its source any further.
        stop_mapping(&router, case, &uuid).await;
    }
}

/// The handler the second suite runs: for each record of the change it was handed, it
/// writes a DERIVED item into the output table through the AWS SDK — the item's key and the
/// change's name, both taken out of the record — so the item can only appear if the record
/// reached it carrying those values.
fn writing_handler() -> String {
    format!(
        r#"
import os

import boto3

OUT_TABLE = os.environ["OUT_TABLE"]
KEY = "{KEY}"


def lambda_handler(event, context):
    client = boto3.client("dynamodb")
    written = 0
    for record in event["Records"]:
        change = record["dynamodb"]
        image = change.get("NewImage") or change["Keys"]
        client.put_item(
            TableName=OUT_TABLE,
            Item={{
                KEY: {{"S": image[KEY]["S"]}},
                "event": {{"S": record["eventName"]}},
            }},
        )
        written += 1
    return {{"written": written}}
"#
    )
}

/// Wait until `table` holds the item `key` names, answering it.
async fn wait_for_item(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    key: &str,
) -> Value {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, read) = dynamodb(
            router,
            "GetItem",
            &json!({ "TableName": table, "Key": { KEY: { "S": key } } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetItem answered {status} {read}"
        );
        if read.get("Item").is_some_and(|item| !item.is_null()) {
            return read["Item"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {table} never came to hold the item {key:?} the handler had to \
             write: {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: the records a mapping delivers carry the change itself, so a handler can act on it
/// — the downstream write it makes from the record's own values is the proof the record
/// arrived.
///
/// The rows differ along which change the handler reacts to and which image the stream's
/// view type gives it the item's values from.
#[tokio::test]
async fn a_delivered_change_reaches_a_handler_that_writes_it_downstream() {
    // (case, what the stream captures, the key written, the change made, the eventName the
    // derived item must carry)
    let rows: Vec<(&str, &str, &str, Write, &str)> = vec![
        (
            "an-insert-under-both-images",
            NEW_AND_OLD_IMAGES,
            "trigger-insert",
            Write::Insert,
            INSERT,
        ),
        (
            "a-removal-under-a-keys-only-stream",
            KEYS_ONLY,
            "trigger-remove",
            Write::Delete,
            REMOVE,
        ),
    ];

    for (case, view_type, key, write, expected_event) in rows {
        let router = emulator();
        let source = format!("source-{case}");
        let output = format!("output-{case}");
        let function = format!("writer-{case}");
        let stream_arn = given_table(&router, case, &source, Some(view_type)).await;
        given_table(&router, case, &output, None).await;
        given_function(
            &router,
            case,
            &function,
            &writing_handler(),
            json!({ "OUT_TABLE": output }),
        )
        .await;

        // A removal has an item to remove only once one is there, and that write is made
        // BEFORE the mapping exists: a mapping reading from the oldest retained record
        // reads the backlog too, so the record of the removal is the one the handler must
        // act on either way.
        if matches!(write, Write::Delete) {
            write_item(&router, case, &source, key, Write::Insert).await;
        }
        let uuid = given_mapping(
            &router,
            case,
            &function,
            &stream_arn,
            TRIM_HORIZON,
            100,
            false,
        )
        .await;
        write_item(&router, case, &source, key, write).await;

        let item = wait_for_item(&router, case, &output, key).await;
        assert_eq!(
            item["event"]["S"],
            json!(expected_event),
            "case {case:?}: the derived item must carry the change the record reported, got \
             {item}"
        );
        assert_eq!(
            item[KEY]["S"],
            json!(key),
            "case {case:?}: the derived item must be keyed on the changed item's key, got \
             {item}"
        );

        // The delivery is what the mapping reports it came to — recorded by the poller
        // just after the invocation returned, so it is waited for rather than read at the
        // instant the item appeared.
        let seen = results_until(&router, case, &uuid, RESULT_OK).await;
        assert_eq!(
            seen.last().map(String::as_str),
            Some(RESULT_OK),
            "case {case:?}: a mapping whose batch was processed must report it, saw {seen:?}"
        );

        // The trigger is torn down as a client tears one down, which is also what stops the
        // mapping from reading its source any further.
        stop_mapping(&router, case, &uuid).await;
    }
}
