// SPDX-License-Identifier: CC0-1.0

//! What an event source mapping on an Amazon Kinesis data stream does with NO client
//! action at all, end to end over the wire, with Kinesis and CloudWatch mounted beside
//! Lambda on ONE engine — so the records are put where a client puts them and the metric
//! is read back where a client reads it (`GetMetricStatistics`).
//!
//! The RULE asserted over the table below: a mapping on a Kinesis data stream reads
//! records only while it is `Enabled` ("When true, the event source mapping is active.
//! When false, Lambda pauses polling and invocation"), and the position it reads from is
//! the MAPPING's — `StartingPosition` is "The position in a stream from which to start
//! reading", fixed when the mapping is created — so pausing does not move it: records put
//! while a mapping is paused stay in the shard and are read once it is enabled again. For
//! every batch it reads, Lambda publishes `IteratorAge` to the `AWS/Lambda`
//! namespace, dimensioned on the function's name and carrying the unit `Milliseconds`,
//! equal to how long the newest record of that batch had been sitting in the shard at the
//! instant the batch was read — so a batch held back behind a paused mapping meters the
//! whole hold, while a batch read as its records arrive meters almost nothing. What the
//! attempt at the batch came to is the mapping's own `LastProcessingResult`, "the result
//! of the event source mapping's last processing attempt".
//!
//! Every row bounds the metered age from BOTH sides against the wall clock, which is what
//! makes the assertion machine-independent: the age is at least the hold the row held its
//! records for, and at most how long ago this test put them.
//!
//! The rows differ along what the rule generalises over: which `StartingPosition` fixes
//! the mapping's position (each of the three the model's enum declares), whether the
//! mapping was paused at creation and enabled later or enabled from the start, how many
//! shards the stream has, how many records are put, the `BatchSize` they are read in, and
//! how long they are held.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::metrics::{
    FUNCTION_NAME_DIMENSION, ITERATOR_AGE_METRIC, ITERATOR_AGE_UNIT, METRIC_NAMESPACE,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED, LAST_PROCESSING_RESULT_OK,
    STARTING_POSITION_AT_TIMESTAMP, STARTING_POSITION_LATEST, STARTING_POSITION_TRIM_HORIZON,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call below signs for, and the region every ARN carries.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `CreateEventSourceMapping` is bound, and under which `GetEventSourceMapping` and
/// `UpdateEventSourceMapping` address one mapping.
pub(crate) const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// The `X-Amz-Target` prefix of the Kinesis service (its `OPERATIONS.md`, "Wire
/// protocol").
const KINESIS_TARGET_PREFIX: &str = "Kinesis_20131202";

/// The `X-Amz-Target` prefix of the CloudWatch service.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// How long a wait for work the mapping has to do — a batch read, or a datapoint
/// appearing — is given before the row is a failure. Well above the poll interval and the
/// state-settle span, so a row fails only when the mapping did not do the work at all.
pub(crate) const BUDGET: Duration = Duration::from_secs(20);

/// How long between two looks while waiting.
pub(crate) const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// What the two-sided bound on a metered age allows for: the instant a record is stamped
/// with is taken inside `PutRecords`, the age is measured when the batch is read and the
/// series is read later still, so neither side of the bound is exact to the millisecond.
pub(crate) const SKEW: f64 = 1_500.0;

/// One row: how a mapping's position is fixed, and what happens to the records.
struct Row {
    case: &'static str,
    /// The `StartingPosition` the mapping is created with.
    starting_position: &'static str,
    /// Whether the mapping is `Enabled` from creation. A row that is not is enabled by
    /// `UpdateEventSourceMapping` after the hold.
    enabled_at_create: bool,
    /// How many shards the stream is created with.
    shards: i64,
    /// How many records are put to the stream.
    records: usize,
    /// The `BatchSize` the mapping reads with.
    batch_size: i64,
    /// How long the records are held before the mapping is allowed to read them.
    hold: Duration,
}

/// Lambda, Kinesis and CloudWatch on one engine: the mapping's event source is a stream
/// and the series it publishes is CloudWatch's, so both are reached in-process — the shape
/// `core/router/src/lib.rs` mounts them in.
pub(crate) fn emulator() -> MultiServiceRouter {
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
pub(crate) async fn lambda(
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

/// One Kinesis call.
pub(crate) async fn kinesis(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(
        router,
        "kinesis",
        KINESIS_TARGET_PREFIX,
        "application/x-amz-json-1.1",
        operation,
        body,
    )
    .await
}

/// One CloudWatch call.
pub(crate) async fn cloudwatch(
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

/// A stream of `shards` shards, waited out to `ACTIVE`; answers its ARN.
pub(crate) async fn given_stream(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    shards: i64,
) -> String {
    let (status, created) = kinesis(
        router,
        "CreateStream",
        &json!({ "StreamName": name, "ShardCount": shards }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStream answered {status} {created}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, summary) = kinesis(
            router,
            "DescribeStreamSummary",
            &json!({ "StreamName": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStreamSummary answered {status} {summary}"
        );
        let description = &summary["StreamDescriptionSummary"];
        if description["StreamStatus"] == json!("ACTIVE") {
            return description["StreamARN"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no StreamARN in {summary}"))
                .to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stream never became ACTIVE: {summary}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// A function for a mapping to invoke, waited out to `Active`.
pub(crate) async fn given_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
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

/// Put `count` records to the stream `name` names, each carrying its own payload.
pub(crate) async fn put_records(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    count: usize,
) {
    let records: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "Data": base64::engine::general_purpose::STANDARD
                    .encode(format!("{case}-{index}").as_bytes()),
                "PartitionKey": format!("{case}-{index}"),
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

/// The `IteratorAge` datapoints CloudWatch holds for one function.
async fn iterator_age_datapoints(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
) -> Vec<Value> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": ITERATOR_AGE_METRIC,
            "Dimensions": [{ "Name": FUNCTION_NAME_DIMENSION, "Value": function }],
            "StartTime": now - 600.0,
            "EndTime": now + 600.0,
            "Period": 60,
            "Statistics": ["Maximum", "Minimum", "SampleCount"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics answered {status} {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// Wait until CloudWatch holds an `IteratorAge` datapoint for `function`,
/// answering them.
async fn wait_for_iterator_age(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
) -> Vec<Value> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let points = iterator_age_datapoints(router, case, function).await;
        if !points.is_empty() || Instant::now() >= deadline {
            return points;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `GetEventSourceMapping` on one mapping, which must resolve the mapping the create
/// accepted.
///
/// Published with the rest of this harness, so every suite over this surface reads a
/// mapping back the one way.
pub(crate) async fn get_mapping(router: &MultiServiceRouter, case: &str, uuid: &str) -> Value {
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

/// Wait until the mapping reports a `LastProcessingResult`, answering what it reported.
async fn wait_for_processing_result(
    router: &MultiServiceRouter,
    case: &str,
    uuid: &str,
) -> Option<String> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let read = get_mapping(router, case, uuid).await;
        let result = read["LastProcessingResult"].as_str().map(str::to_string);
        if result.is_some() || Instant::now() >= deadline {
            return result;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: a Kinesis-sourced event source mapping reads nothing while it is paused, reads
/// from the position its creation fixed once it is enabled — so a backlog built up while
/// it was paused is read rather than skipped — and meters each batch it reads as the age
/// of that batch's newest record at the instant it was read.
#[tokio::test]
async fn a_paused_mapping_accumulates_a_backlog_that_meters_its_age_when_it_resumes() {
    let rows = vec![
        // The recorded shape: a mapping created disabled with LATEST, records put to the
        // stream and held, then the mapping enabled.
        Row {
            case: "latest-held-while-disabled",
            starting_position: STARTING_POSITION_LATEST,
            enabled_at_create: false,
            shards: 1,
            records: 5,
            batch_size: 100,
            hold: Duration::from_secs(3),
        },
        // The same hold with the position fixed at the oldest retained record, over a
        // stream of several shards and read in batches smaller than the backlog.
        Row {
            case: "trim-horizon-held-over-two-shards",
            starting_position: STARTING_POSITION_TRIM_HORIZON,
            enabled_at_create: false,
            shards: 2,
            records: 4,
            batch_size: 2,
            hold: Duration::from_secs(3),
        },
        // The position fixed at a stated instant before the records were put.
        Row {
            case: "at-timestamp-held-while-disabled",
            starting_position: STARTING_POSITION_AT_TIMESTAMP,
            enabled_at_create: false,
            shards: 1,
            records: 3,
            batch_size: 10,
            hold: Duration::from_secs(3),
        },
        // Nothing is held: a mapping enabled from the start reads its records as they
        // arrive, so the age it meters is bounded by how long ago they were put.
        Row {
            case: "enabled-at-create-reads-fresh-records",
            starting_position: STARTING_POSITION_LATEST,
            enabled_at_create: true,
            shards: 1,
            records: 3,
            batch_size: 10,
            hold: Duration::ZERO,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = emulator();
        let stream = format!("stream-{case}");
        let function = format!("function-{case}");
        let stream_arn = given_stream(&router, case, &stream, row.shards).await;
        given_function(&router, case, &function).await;

        let mut request = json!({
            "FunctionName": function,
            "EventSourceArn": stream_arn,
            "StartingPosition": row.starting_position,
            "BatchSize": row.batch_size,
            "Enabled": row.enabled_at_create,
        });
        if row.starting_position == STARTING_POSITION_AT_TIMESTAMP {
            // "With StartingPosition set to AT_TIMESTAMP, the time from which to start
            // reading": an instant before the records of this row are put, so they are
            // all past it.
            let before = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                - 60.0;
            request["StartingPositionTimestamp"] = json!(before);
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

        put_records(&router, case, &stream, row.records).await;
        let put_at = Instant::now();

        if !row.enabled_at_create {
            // While the mapping is paused, nothing is read: no batch, so no age metered
            // and no processing attempt to report.
            tokio::time::sleep(row.hold).await;
            let points = iterator_age_datapoints(&router, case, &function).await;
            assert!(
                points.is_empty(),
                "case {case:?}: a paused mapping must read no batch, but {ITERATOR_AGE_METRIC} \
                 reported {points:?}"
            );
            let read = get_mapping(&router, case, &uuid).await;
            assert_eq!(
                read["LastProcessingResult"],
                Value::Null,
                "case {case:?}: a paused mapping must make no processing attempt, got {read}"
            );
            let (status, updated) = lambda(
                &router,
                "PUT",
                &format!("{MAPPINGS_URI}/{uuid}"),
                &json!({ "Enabled": true }),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: UpdateEventSourceMapping answered {status} {updated}"
            );
        }

        // Once it may read, the mapping reads the records that were waiting and meters
        // their age.
        let points = wait_for_iterator_age(&router, case, &function).await;
        assert!(
            !points.is_empty(),
            "case {case:?}: every batch read must meter {ITERATOR_AGE_METRIC}, but the series \
             reported no datapoint"
        );
        let elapsed_millis = put_at.elapsed().as_secs_f64() * 1_000.0;
        let hold_millis = row.hold.as_secs_f64() * 1_000.0;
        for point in &points {
            let unit = point["Unit"].as_str().unwrap_or_default();
            assert_eq!(
                unit, ITERATOR_AGE_UNIT,
                "case {case:?}: the age must be reported in {ITERATOR_AGE_UNIT}, got {point:?}"
            );
            let smallest = point["Minimum"].as_f64().unwrap_or_default();
            let largest = point["Maximum"].as_f64().unwrap_or_default();
            assert!(
                smallest >= hold_millis - SKEW,
                "case {case:?}: a batch held back for {hold_millis} ms must meter at least that \
                 wait, got {smallest:?} ms"
            );
            assert!(
                largest <= elapsed_millis + SKEW,
                "case {case:?}: the age cannot exceed how long ago the records were put \
                 ({elapsed_millis} ms), got {largest:?} ms"
            );
        }

        // A batch that was read was handed to the function, so the mapping reports what
        // that attempt came to: the batch was processed, or the invocation of the function
        // did not succeed and the batch stays in the shard.
        let result = wait_for_processing_result(&router, case, &uuid).await;
        assert!(
            result
                .as_deref()
                .is_some_and(|result| result == LAST_PROCESSING_RESULT_OK
                    || result == LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED),
            "case {case:?}: a mapping that read a batch must report the result of that \
             processing attempt, got {result:?}"
        );

        // The series is the function's own: `ListMetrics` reports it under the function's
        // name and nothing else.
        let (status, listed) = cloudwatch(
            &router,
            "ListMetrics",
            &json!({ "Namespace": METRIC_NAMESPACE, "MetricName": ITERATOR_AGE_METRIC }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListMetrics answered {status} {listed}"
        );
        let dimensions: Vec<Value> = listed["Metrics"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|series| series["Dimensions"].clone())
            .collect();
        assert_eq!(
            dimensions,
            vec![json!([{ "Name": FUNCTION_NAME_DIMENSION, "Value": function }])],
            "case {case:?}: the series must be dimensioned on the function alone, got {listed}"
        );
    }
}
