// SPDX-License-Identifier: CC0-1.0

//! WHICH SERIES the backlog age of a stream event source mapping is published as, read back
//! over the wire through CloudWatch's own `GetMetricStatistics` and `ListMetrics`, with
//! Kinesis and CloudWatch mounted beside Lambda on one engine.
//!
//! The RULE asserted over the table below: a CloudWatch metric is identified by its
//! namespace, its name and its dimensions together, so the age a stream-sourced event source
//! mapping meters for each batch it reads is observable only under the name the AWS Lambda
//! metric reference gives it in the `AWS/Lambda` namespace — `IteratorAge`, "the age of the
//! last record in the event", carrying the unit `Milliseconds` and dimensioned on
//! `FunctionName` alone. A client reading stream lag names THAT series; a datapoint published
//! under any other name in that namespace is invisible to it, and the name the stream service
//! uses for the same quantity inside its OWN namespace selects nothing here.
//!
//! Every name in this suite is written as a literal, taken from the AWS Lambda metric
//! reference rather than from this crate's constants, so a series published under a name no
//! AWS reader would ask for fails here instead of agreeing with itself.
//!
//! The rows differ along what the rule generalises over — the series is the same one whatever
//! the mapping's shape: which `StartingPosition` fixes the mapping's position, whether it was
//! paused at creation and enabled later or enabled from the start, how many shards the stream
//! has, how many records are put, the `BatchSize` they are read in, and how long they are
//! held. The age each row reports is bounded from both sides against the wall clock (at least
//! the hold the row held its records for, at most how long ago it put them), which is what
//! makes the assertion machine-independent.
//!
//! The harness (the router with all three services on one engine, the Lambda, Kinesis and
//! CloudWatch calls, and the stream, function and record fixtures) is the one
//! `kinesis_event_source_delivery.rs` publishes, so both suites over this surface drive the
//! same wire.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::kinesis_event_source_delivery::{
    cloudwatch, emulator, given_function, given_stream, kinesis, lambda, put_records, BUDGET,
    LOOK_AGAIN, MAPPINGS_URI, SKEW,
};

/// The namespace Lambda's own metrics live in (the AWS Lambda metric reference).
const NAMESPACE: &str = "AWS/Lambda";

/// The name that reference gives the per-batch backlog age of a mapping that reads from a
/// stream, and the unit it is reported in.
const ITERATOR_AGE: &str = "IteratorAge";
/// See [`ITERATOR_AGE`].
const MILLISECONDS: &str = "Milliseconds";

/// The dimension that reference reports it per: the function the batch was handed to.
const FUNCTION_DIMENSION: &str = "FunctionName";

/// Names that must select NOTHING in [`NAMESPACE`], because they are not what the AWS Lambda
/// metric reference calls this series: the names the stream service uses for a stream's own
/// backlog age inside ITS namespace (`AWS/Kinesis`, whose enhanced-monitoring metric list and
/// per-consumer series spell it so). A series is its namespace, name and dimensions together,
/// so a reader of Lambda's series finds nothing under either.
const FOREIGN_NAMES: &[&str] = &["IteratorAgeMilliseconds", "GetRecords.IteratorAgeMilliseconds"];

/// The `StartingPosition` values the model's enum declares
/// (`smithy_query(query="shape", shape="EventSourcePosition")`).
const AT_TIMESTAMP: &str = "AT_TIMESTAMP";
/// See [`AT_TIMESTAMP`].
const TRIM_HORIZON: &str = "TRIM_HORIZON";
/// See [`AT_TIMESTAMP`].
const LATEST: &str = "LATEST";

/// The `Period`, in seconds, every query reads at: the one-minute granularity Lambda's own
/// series are stored at.
const PERIOD_SECS: i64 = 60;

/// How far either side of now a query's window reaches, in seconds. Wide enough that the
/// minute the batch was read in is inside it however the row's hold straddles a minute
/// boundary.
const WINDOW_SECS: f64 = 600.0;

/// One row: the shape of the mapping whose metered age the series must carry.
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
    /// How many whole seconds the records are held before the mapping may read them.
    hold_secs: u32,
}

/// Now, in the epoch seconds a stored datapoint is stamped with.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// `GetMetricStatistics` over the series `metric` names in [`NAMESPACE`], dimensioned on one
/// function and selected by the unit `unit`; answers its `Datapoints`.
async fn datapoints(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    function: &str,
    unit: &str,
) -> Vec<Value> {
    let now = now_secs();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{ "Name": FUNCTION_DIMENSION, "Value": function }],
            "StartTime": now - WINDOW_SECS,
            "EndTime": now + WINDOW_SECS,
            "Period": PERIOD_SECS,
            "Statistics": ["Maximum", "Minimum"],
            "Unit": unit,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics({metric}) answered {status} {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// Wait until the [`ITERATOR_AGE`] series holds a datapoint for `function`, answering what it
/// holds when it does or when the budget runs out.
async fn wait_for_iterator_age(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
) -> Vec<Value> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let points = datapoints(router, case, ITERATOR_AGE, function, MILLISECONDS).await;
        if !points.is_empty() || Instant::now() >= deadline {
            return points;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The metric names [`NAMESPACE`] reports for the series dimensioned on `function` alone.
async fn series_names_of(router: &MultiServiceRouter, case: &str, function: &str) -> Vec<String> {
    let (status, listed) = cloudwatch(
        router,
        "ListMetrics",
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{ "Name": FUNCTION_DIMENSION, "Value": function }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListMetrics answered {status} {listed}"
    );
    listed["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|series| series["MetricName"].as_str().map(str::to_string))
        .collect()
}

/// RULE: the backlog age a stream-sourced event source mapping meters is published as the
/// `AWS/Lambda` series the AWS Lambda metric reference names — `IteratorAge`, in
/// `Milliseconds`, dimensioned on `FunctionName` — and under no other name in that namespace.
#[tokio::test]
async fn a_stream_mapping_meters_its_backlog_age_as_the_lambda_iterator_age_series() {
    let rows = vec![
        // The recorded shape: a mapping held paused over a one-shard stream, the records put
        // and held, then the mapping enabled so the backlog is read in one batch.
        Row {
            case: "trim-horizon-held-while-disabled",
            starting_position: TRIM_HORIZON,
            enabled_at_create: false,
            shards: 1,
            records: 5,
            batch_size: 100,
            hold_secs: 4,
        },
        // The same hold with the position fixed at the newest record, over several shards and
        // read in batches smaller than the backlog.
        Row {
            case: "latest-held-over-two-shards",
            starting_position: LATEST,
            enabled_at_create: false,
            shards: 2,
            records: 4,
            batch_size: 2,
            hold_secs: 3,
        },
        // The position fixed at a stated instant before the records were put.
        Row {
            case: "at-timestamp-held-while-disabled",
            starting_position: AT_TIMESTAMP,
            enabled_at_create: false,
            shards: 1,
            records: 3,
            batch_size: 10,
            hold_secs: 3,
        },
        // Nothing is held: the same series carries the age of a batch read as its records
        // arrive, which is bounded only by how long ago they were put.
        Row {
            case: "enabled-at-create-reads-fresh-records",
            starting_position: TRIM_HORIZON,
            enabled_at_create: true,
            shards: 1,
            records: 2,
            batch_size: 10,
            hold_secs: 0,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = emulator();
        let stream = format!("age-stream-{case}");
        let function = format!("age-function-{case}");
        let stream_arn = given_stream(&router, case, &stream, row.shards).await;
        given_function(&router, case, &function).await;

        let mut request = json!({
            "FunctionName": function,
            "EventSourceArn": stream_arn,
            "StartingPosition": row.starting_position,
            "BatchSize": row.batch_size,
            "Enabled": row.enabled_at_create,
        });
        if row.starting_position == AT_TIMESTAMP {
            request["StartingPositionTimestamp"] = json!(now_secs() - 60.0);
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
            tokio::time::sleep(Duration::from_secs(u64::from(row.hold_secs))).await;
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

        let points = wait_for_iterator_age(&router, case, &function).await;
        assert!(
            !points.is_empty(),
            "case {case:?}: the metered age must be readable as {NAMESPACE}/{ITERATOR_AGE} \
             dimensioned on {FUNCTION_DIMENSION}={function}, but that series reported no \
             datapoint"
        );

        let elapsed_millis = put_at.elapsed().as_secs_f64() * 1_000.0;
        let hold_millis = f64::from(row.hold_secs) * 1_000.0;
        for point in &points {
            let unit = point["Unit"].as_str().unwrap_or_default();
            assert_eq!(
                unit, MILLISECONDS,
                "case {case:?}: the age must be reported in {MILLISECONDS}, got {point:?}"
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

        // The same datapoints are reachable under no other name in this namespace: a reader
        // asking for the stream service's own spelling of a backlog age selects nothing.
        for foreign in FOREIGN_NAMES {
            let stray = datapoints(&router, case, foreign, &function, MILLISECONDS).await;
            assert!(
                stray.is_empty(),
                "case {case:?}: {NAMESPACE}/{foreign} is not what this series is called, but it \
                 reported {stray:?}"
            );
        }

        // And the catalogue of series this function has in the namespace names it the same
        // way, so a client that discovers the series before reading it asks for the name that
        // serves it.
        let names = series_names_of(&router, case, &function).await;
        assert!(
            names.iter().any(|name| name == ITERATOR_AGE),
            "case {case:?}: ListMetrics must report {ITERATOR_AGE} for \
             {FUNCTION_DIMENSION}={function}, got {names:?}"
        );
        for foreign in FOREIGN_NAMES {
            assert!(
                !names.iter().any(|name| name == foreign),
                "case {case:?}: ListMetrics must not report {foreign} in {NAMESPACE}, got \
                 {names:?}"
            );
        }
    }
}

/// RULE, the other side of the dimension: the series belongs to the function the batch was
/// handed to, so naming another function selects nothing even once the series exists.
#[tokio::test]
async fn the_iterator_age_series_belongs_to_the_function_the_batch_was_handed_to() {
    let case = "iterator-age-is-per-function";
    let router = emulator();
    let stream = "age-dimension-stream";
    let function = "age-dimension-function";
    let other = "age-dimension-other-function";
    let stream_arn = given_stream(&router, case, stream, 1).await;
    given_function(&router, case, function).await;
    given_function(&router, case, other).await;

    let (status, created) = lambda(
        &router,
        "POST",
        MAPPINGS_URI,
        &json!({
            "FunctionName": function,
            "EventSourceArn": stream_arn,
            "StartingPosition": TRIM_HORIZON,
            "BatchSize": 10,
            "Enabled": true,
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateEventSourceMapping answered {status} {created}"
    );
    put_records(&router, case, stream, 2).await;

    let points = wait_for_iterator_age(&router, case, function).await;
    assert!(
        !points.is_empty(),
        "case {case:?}: the mapped function's {ITERATOR_AGE} series must hold the metered age"
    );
    let unmapped = datapoints(&router, case, ITERATOR_AGE, other, MILLISECONDS).await;
    assert!(
        unmapped.is_empty(),
        "case {case:?}: no batch was handed to {other}, so its {ITERATOR_AGE} series must be \
         empty, got {unmapped:?}"
    );

    // The stream the batch came off is still a Kinesis stream: the namespace that holds this
    // series is Lambda's, so the stream's own `DescribeStreamSummary` is unaffected by it.
    let (status, summary) = kinesis(
        &router,
        "DescribeStreamSummary",
        &json!({ "StreamName": stream }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStreamSummary answered {status} {summary}"
    );
}
