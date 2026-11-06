// SPDX-License-Identifier: CC0-1.0

//! What a Kinesis-sourced event source mapping HANDS the function it maps, asserted through
//! the function's own run: the handler of a real deployment package inspects every record of
//! the batch it was given and decides, from the record's own data, what to report back — so
//! what this suite reads back is a consequence of the records having reached the handler.
//!
//! The RULE asserted over the table below: once a mapping on a Kinesis data stream is
//! `Enabled`, Lambda reads that stream's records and INVOKES the mapped function with one
//! envelope per record ("Lambda reads items from the event source and invokes the function"),
//! each envelope naming the stream it came from and carrying that record's own base64 data,
//! partition key and sequence number — so an invocation is metered for the function and the
//! attempt is reported as the mapping's `LastProcessingResult`, "the result of the event
//! source mapping's last processing attempt": `OK` when the attempt processed the batch, and
//! the function-call problem when it did not. Whether it did is the function's own answer: an
//! exception it raises fails the batch, and for a mapping whose `FunctionResponseTypes` asks
//! for the report ("(Kinesis, DynamoDB Streams, Amazon MSK, self-managed Apache Kafka, and
//! Amazon SQS) A list of current response type enums applied to the event source mapping"),
//! a record the handler names as unprocessed holds the shard's position at that record, so
//! the mapping keeps reading the same batch.
//!
//! This is the half of the data plane that `kinesis_event_source_delivery.rs` cannot see: it
//! uploads an opaque package, so a batch it reads is handed to a function that cannot run and
//! either outcome of the attempt satisfies it. Here the package RUNS, and every row's verdict
//! is computed inside the handler from the envelope it was handed — a handler that was never
//! invoked, or invoked with an envelope of the wrong shape, reports nothing or raises, and the
//! row fails.
//!
//! The rows differ along what the rule generalises over: how many records the batch holds,
//! the `BatchSize` they are read in, which `StartingPosition` fixes the mapping's position,
//! how many shards the stream has, which of the stream's two write calls put the records
//! there, whether the mapping asks for its function's report of which records failed, where in
//! the batch the record the handler will not process sits, and how the handler ends (a return,
//! a raise).
//!
//! The harness (the router with Lambda, Kinesis and CloudWatch on one engine, and the stream
//! fixture) is the one `kinesis_event_source_delivery.rs` publishes, so every suite over this
//! surface drives the same wire.

use std::io::Write as _;
use std::time::Instant;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::kinesis_event_source_delivery::{
    cloudwatch, emulator, given_stream, kinesis, lambda, BUDGET, LOOK_AGAIN, MAPPINGS_URI,
};

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The runtime every function below is created with, and the handler its package exposes.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// The one value `FunctionResponseType` admits
/// (`smithy_query(query="shape", shape="FunctionResponseType")`).
const REPORT_BATCH_ITEM_FAILURES: &str = "ReportBatchItemFailures";

/// The `StartingPosition` values the model's enum declares
/// (`smithy_query(query="shape", shape="EventSourcePosition")`), as far as a stream whose
/// records are already in it can be read from.
const TRIM_HORIZON: &str = "TRIM_HORIZON";
/// See [`TRIM_HORIZON`].
const LATEST: &str = "LATEST";

/// What `LastProcessingResult` reports for an attempt that processed the batch, and for one
/// that did not reach the function's processing at all
/// (`EventSourceMappingConfiguration$LastProcessingResult`).
const RESULT_OK: &str = "OK";
/// See [`RESULT_OK`].
const RESULT_FUNCTION_CALL_FAILED: &str = "PROBLEM: Function call failed";

/// The namespace Lambda's own metrics live in, the series one admitted invocation is counted
/// on, the dimension it is counted per and the unit it carries (the AWS Lambda metric
/// reference).
const NAMESPACE: &str = "AWS/Lambda";
/// See [`NAMESPACE`].
const INVOCATIONS: &str = "Invocations";
/// See [`NAMESPACE`].
const FUNCTION_DIMENSION: &str = "FunctionName";

/// The prefix a record's data carries to be one the handler reports as unprocessed, and the
/// prefix that makes it raise instead. Nothing in the emulator reads either: they are read by
/// the handler, out of the record's own decoded data.
const POISON: &str = "poison";
/// See [`POISON`].
const RAISE: &str = "raise";

/// The handler every row runs. It asserts the shape of the envelope Lambda hands a Kinesis
/// batch over as — one record per envelope, naming its source and stream and carrying the
/// record's own partition key, sequence number and base64 data — and decides what to report
/// from that record's DATA, so the report is a consequence of the record having arrived.
const HANDLER_SOURCE: &str = r#"
import base64
import os

STREAM_ARN = os.environ["STREAM_ARN"]


def lambda_handler(event, context):
    records = event["Records"]
    if not records:
        raise ValueError("a batch was handed over holding no record")
    failures = []
    for record in records:
        if record["eventSource"] != "aws:kinesis":
            raise ValueError("eventSource is " + repr(record["eventSource"]))
        if record["eventSourceARN"] != STREAM_ARN:
            raise ValueError("eventSourceARN is " + repr(record["eventSourceARN"]))
        if not record["eventID"].startswith("shardId-"):
            raise ValueError("eventID is " + repr(record["eventID"]))
        stream_record = record["kinesis"]
        if not stream_record["partitionKey"]:
            raise ValueError("a record carries no partitionKey")
        sequence_number = stream_record["sequenceNumber"]
        if not sequence_number:
            raise ValueError("a record carries no sequenceNumber")
        payload = base64.b64decode(stream_record["data"]).decode()
        if payload.startswith("raise"):
            raise RuntimeError("the handler cannot process " + payload)
        if payload.startswith("poison"):
            failures.append({"itemIdentifier": sequence_number})
    return {"batchItemFailures": failures}
"#;

/// One row: the mapping's shape, the records put on its stream, and what the mapping must
/// report once the handler has run on them.
struct Row {
    case: &'static str,
    /// The `StartingPosition` the mapping is created with.
    starting_position: &'static str,
    /// How many shards the stream is created with.
    shards: i64,
    /// The data of each record put on the stream, in order.
    payloads: &'static [&'static str],
    /// Whether the records are put one call at a time (`PutRecord`) rather than as one batch
    /// (`PutRecords`): a record is a record of the shard however it was written.
    put_one_at_a_time: bool,
    /// The `BatchSize` the mapping reads with.
    batch_size: i64,
    /// Whether the mapping asks for its function's report of which records failed.
    reads_report: bool,
    /// The `LastProcessingResult` the mapping must come to report.
    expected_result: &'static str,
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory the
/// way a caller uploads one.
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

/// A function whose package RUNS [`HANDLER_SOURCE`], told through its environment which
/// stream the records it is handed must come from, waited out to `Active`.
async fn given_handler_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    stream_arn: &str,
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
            "Timeout": 10,
            "Environment": { "Variables": { "STREAM_ARN": stream_arn } },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
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

/// Put one record per entry of `payloads` on the stream `name` names, each carrying that
/// entry as its data and its own partition key — with one `PutRecord` call per record when
/// `one_at_a_time`, and one `PutRecords` call for all of them otherwise.
async fn put_payloads(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    payloads: &[&str],
    one_at_a_time: bool,
) {
    if one_at_a_time {
        for (index, payload) in payloads.iter().enumerate() {
            let (status, written) = kinesis(
                router,
                "PutRecord",
                &json!({
                    "StreamName": name,
                    "Data": base64::engine::general_purpose::STANDARD
                        .encode(payload.as_bytes()),
                    "PartitionKey": format!("{index}-{payload}"),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: PutRecord answered {status} {written}"
            );
            assert!(
                written["SequenceNumber"]
                    .as_str()
                    .is_some_and(|sequence_number| !sequence_number.is_empty()),
                "case {case:?}: a placed record must answer its sequence number, got {written}"
            );
        }
        return;
    }
    let records: Vec<Value> = payloads
        .iter()
        .enumerate()
        .map(|(index, payload)| {
            json!({
                "Data": base64::engine::general_purpose::STANDARD.encode(payload.as_bytes()),
                "PartitionKey": format!("{index}-{payload}"),
            })
        })
        .collect();
    let (status, written) = kinesis(
        router,
        "PutRecords",
        &json!({ "StreamName": name, "Records": records }),
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

/// RULE: an `Enabled` mapping on a Kinesis data stream reads that stream's records and invokes
/// the mapped function with them, and what the handler makes of each record it was handed is
/// what the mapping reports as the result of the attempt.
#[tokio::test]
async fn a_kinesis_mapping_invokes_its_function_with_the_records_it_read() {
    let rows = vec![
        // The recorded shape: an enabled mapping over a one-shard stream, records put on it,
        // and a handler that processes every one of them.
        Row {
            case: "every-record-processed",
            starting_position: TRIM_HORIZON,
            shards: 1,
            payloads: &["ok-0", "ok-1", "ok-2"],
            put_one_at_a_time: true,
            batch_size: 10,
            reads_report: true,
            expected_result: RESULT_OK,
        },
        // One record at a time, from the newest position, over a stream of several shards:
        // the same handler must still see a well-formed envelope for each.
        Row {
            case: "one-record-batches-over-two-shards",
            starting_position: LATEST,
            shards: 2,
            payloads: &["ok-a", "ok-b", "ok-c", "ok-d"],
            put_one_at_a_time: false,
            batch_size: 1,
            reads_report: true,
            expected_result: RESULT_OK,
        },
        // A handler that names the batch's FIRST record as one it did not process holds the
        // shard where it is, so the mapping keeps reading that record and reports the
        // attempt as the function-call problem.
        Row {
            case: "the-first-record-is-not-processed",
            starting_position: TRIM_HORIZON,
            shards: 1,
            payloads: &[POISON, "ok-1"],
            put_one_at_a_time: false,
            batch_size: 10,
            reads_report: true,
            expected_result: RESULT_FUNCTION_CALL_FAILED,
        },
        // A record in the MIDDLE of the batch: the head of the batch is processed, and the
        // mapping reads on from the record the report named — which is then the head of a
        // batch nothing of which is processed.
        Row {
            case: "a-record-in-the-middle-is-not-processed",
            starting_position: TRIM_HORIZON,
            shards: 1,
            payloads: &["ok-0", POISON, "ok-2"],
            put_one_at_a_time: true,
            batch_size: 10,
            reads_report: true,
            expected_result: RESULT_FUNCTION_CALL_FAILED,
        },
        // The SAME records under a mapping that does not ask for the report: nothing reads
        // what the handler answered, so an invocation that succeeded processed the batch.
        Row {
            case: "a-report-no-mapping-asked-for-is-not-read",
            starting_position: TRIM_HORIZON,
            shards: 1,
            payloads: &["ok-0", POISON, "ok-2"],
            put_one_at_a_time: false,
            batch_size: 10,
            reads_report: false,
            expected_result: RESULT_OK,
        },
        // A handler that RAISES on a record never answers a report at all: the invocation
        // failed, so no record of the batch is processed.
        Row {
            case: "the-handler-raises-on-a-record",
            starting_position: TRIM_HORIZON,
            shards: 1,
            payloads: &["ok-0", RAISE],
            put_one_at_a_time: true,
            batch_size: 10,
            reads_report: true,
            expected_result: RESULT_FUNCTION_CALL_FAILED,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = emulator();
        let stream = format!("run-stream-{case}");
        let function = format!("run-function-{case}");
        let stream_arn = given_stream(&router, case, &stream, row.shards).await;
        given_handler_function(&router, case, &function, &stream_arn).await;

        let mut request = json!({
            "FunctionName": function,
            "EventSourceArn": stream_arn,
            "StartingPosition": row.starting_position,
            "BatchSize": row.batch_size,
            "Enabled": true,
        });
        if row.reads_report {
            request["FunctionResponseTypes"] = json!([REPORT_BATCH_ITEM_FAILURES]);
        }
        let (status, created) = lambda(&router, "POST", MAPPINGS_URI, &request).await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateEventSourceMapping answered {status} {created}"
        );
        let uuid = created["UUID"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {created}"))
            .to_string();

        put_payloads(&router, case, &stream, row.payloads, row.put_one_at_a_time).await;

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
    }
}
