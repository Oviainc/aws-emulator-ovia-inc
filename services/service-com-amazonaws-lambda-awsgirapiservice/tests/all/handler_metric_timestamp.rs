// SPDX-License-Identifier: CC0-1.0

//! The NATIVE DATE-TIME a running handler puts on a timestamp member of an SDK call: Lambda
//! and Amazon CloudWatch on ONE engine, a function whose handler publishes a datapoint with
//! a `datetime` (python) or a `Date` (node) on `MetricDatum.Timestamp`, and the series read
//! back where a client reads it.
//!
//! The rule pinned here: the AWS SDK a managed runtime ships marshals a native date-time
//! given for an input member the service's model types as a timestamp into the form that
//! service's protocol carries a timestamp in — the seconds since the epoch, for the AWS JSON
//! protocols — reading a value that carries no zone as UTC, the zone a function runs in; it
//! never refuses one as a value the call cannot carry. So the instant the handler named is
//! the instant the datapoint is stored at, and a value that really is not sendable is still
//! refused.
//!
//! The rows differ along what the rule generalises over: the FORM of the value the handler
//! passes (a naive `datetime`, one carrying microseconds, one in UTC, one at a negative and
//! one at a positive zone offset, a `Date`), whether the member is given at all, and whether
//! the value is a date-time in the first place. Every row is asserted where a client sees it
//! — the datapoint CloudWatch's own metric read reports — so a call that was accepted but
//! stored at the wrong instant, or at the clock's instant instead of the handler's, fails
//! here. A second suite drives the same rule through ANOTHER operation's own timestamp
//! members (`GetMetricStatistics`'s `StartTime`/`EndTime`), from inside the handler.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `X-Amz-Target` prefix CloudWatch's own model declares
/// (`smithy_query(query="service_info")`: service shape `GraniteServiceVersion20100801`).
const CLOUDWATCH_TARGET: &str = "GraniteServiceVersion20100801";

/// The namespace every datapoint below is published into: a custom one, so nothing here
/// touches a reserved `AWS/` namespace.
const NAMESPACE: &str = "Emulator/HandlerTimestamps";

/// One minute, in seconds — the width of the windows every read below is answered at.
const MINUTE: f64 = 60.0;

/// That same minute as the `Period` member carries it, which the model types as an integer.
const PERIOD: i64 = 60;

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// Lambda and Amazon CloudWatch over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's SDK call is dispatched back into this same router, so the series it
/// publishes into is the series this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One CloudWatch call (`awsJson1_0`, `POST /` with `X-Amz-Target`) that must answer 200.
async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("monitoring.{REGION}.amazonaws.com"))
        .header("authorization", authorization("monitoring"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{CLOUDWATCH_TARGET}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status, headers and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// A .zip deployment package holding `source` under `entry`, written in memory the way a
/// caller uploads one.
fn package(entry: &str, source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer.start_file(entry, options).expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a function of `runtime` whose code is `source` and wait out the provisioning
/// window.
async fn create_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    runtime: &str,
    handler: &str,
    entry: &str,
    source: &str,
) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": runtime,
            "Handler": handler,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(entry, source)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` the function synchronously with `event`, answering the payload the run produced
/// and the `FunctionError` signal it carried, if any.
async fn invoke(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    event: &Value,
) -> (Option<String>, Value) {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (signal, payload)
}

/// The first instant of the minute half an hour before this suite started, in epoch seconds.
///
/// A whole minute, so the one-minute windows a read lays from it are the minutes of the clock
/// ("Start time less than 15 days ago - Round down to the nearest whole minute",
/// `GetMetricStatisticsInput$StartTime`), and well inside the two weeks back a datapoint may
/// be published into.
fn window_start() -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    (now / MINUTE).floor() * MINUTE - 30.0 * MINUTE
}

/// The `(timestamp, sum)` of every datapoint `metric` holds between `start` and `end`, read
/// at one-minute granularity through CloudWatch's own API.
async fn datapoints(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    start: f64,
    end: f64,
) -> Vec<(f64, f64)> {
    let answered = cloudwatch(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "StartTime": start,
            "EndTime": end,
            "Period": PERIOD,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    let mut held: Vec<(f64, f64)> = answered["Datapoints"]
        .as_array()
        .map(|listed| {
            listed
                .iter()
                .map(|point| {
                    (
                        point["Timestamp"].as_f64().unwrap_or(f64::NAN),
                        point["Sum"].as_f64().unwrap_or(f64::NAN),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    held.sort_unstable_by(|one, other| one.0.total_cmp(&other.0));
    held
}

/// The python handler every row runs: it publishes ONE datapoint whose `Timestamp` is built
/// in the form the event names, and reports what the SDK answered.
///
/// The form is named by the event, so one package serves every row. The handler also serves
/// the READ the second suite drives, over a window whose `StartTime` and `EndTime` are
/// `datetime`s too.
const PYTHON_HANDLER: &str = r#"
import datetime

import boto3

EPOCH = datetime.datetime(1970, 1, 1)


def at(event):
    """The instant the event names, as a naive UTC datetime."""
    return EPOCH + datetime.timedelta(
        seconds=event["seconds"], microseconds=event.get("micros", 0)
    )


def stamp(event):
    """The value the handler puts on MetricDatum.Timestamp, in the form the event names."""
    form = event["form"]
    if form == "absent":
        return None
    if form == "unsendable":
        return object()
    moment = at(event)
    if form == "naive":
        return moment
    moment = moment.replace(tzinfo=datetime.timezone.utc)
    if form == "utc":
        return moment
    return moment.astimezone(
        datetime.timezone(datetime.timedelta(minutes=event["offset_minutes"]))
    )


def lambda_handler(event, context):
    client = boto3.client("cloudwatch")
    if event.get("read"):
        try:
            answered = client.get_metric_statistics(
                Namespace=event["namespace"],
                MetricName=event["metric"],
                StartTime=at(event),
                EndTime=EPOCH + datetime.timedelta(seconds=event["end_seconds"]),
                Period=event["period"],
                Statistics=["Sum"],
            )
        except Exception as error:
            return {"ok": False, "refused": "%s: %s" % (type(error).__name__, error)}
        return {
            "ok": True,
            "sums": sorted(point["Sum"] for point in answered.get("Datapoints", [])),
        }
    datum = {"MetricName": event["metric"], "Value": event["value"], "Unit": "Count"}
    moment = stamp(event)
    if moment is not None:
        datum["Timestamp"] = moment
    try:
        answered = client.put_metric_data(
            Namespace=event["namespace"], MetricData=[datum]
        )
    except Exception as error:
        return {"ok": False, "refused": "%s: %s" % (type(error).__name__, error)}
    return {"ok": True, "status": answered["ResponseMetadata"]["HTTPStatusCode"]}
"#;

/// The node handler: the same publication, with the `Date` a JavaScript handler holds an
/// instant in.
const NODE_HANDLER: &str = r#"
import { CloudWatchClient, PutMetricDataCommand } from "@aws-sdk/client-cloudwatch";

export const handler = async (event) => {
    const client = new CloudWatchClient({ region: event.region });
    const datum = { MetricName: event.metric, Value: event.value, Unit: "Count" };
    if (event.form !== "absent") {
        datum.Timestamp = new Date(event.seconds * 1000 + event.millis);
    }
    try {
        const answered = await client.send(new PutMetricDataCommand({
            Namespace: event.namespace,
            MetricData: [datum],
        }));
        return { ok: true, status: answered.$metadata.httpStatusCode };
    } catch (error) {
        return { ok: false, refused: error.name + ": " + error.message };
    }
};
"#;

/// Where the datapoint a row publishes must land.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Landing {
    /// In the minute of `window_start() + seconds`: the instant the handler named.
    AtOffset(f64),
    /// At the service's own clock, because the handler named no instant at all.
    AtTheClock,
    /// Nowhere: the SDK refused the value, so no request was made.
    Nowhere,
}

/// What one row asks the handler to publish, and where it must land.
struct Row {
    case: &'static str,
    /// The form the handler builds the value in.
    form: &'static str,
    /// How far after the window's first minute the instant the handler names falls.
    seconds: f64,
    /// The microseconds that instant carries beyond its whole second.
    micros: i64,
    /// The zone offset, in minutes, the `offset` form expresses the instant at.
    offset_minutes: i64,
    /// The datapoint's value, distinct per row so the series pins which row wrote it.
    value: f64,
    landing: Landing,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a naive datetime, the form datetime.utcnow() answers with",
            form: "naive",
            seconds: 0.0,
            micros: 0,
            offset_minutes: 0,
            value: 1.0,
            landing: Landing::AtOffset(0.0),
        },
        Row {
            case: "a naive datetime carrying microseconds",
            form: "naive",
            seconds: 61.0,
            micros: 750_000,
            offset_minutes: 0,
            value: 2.0,
            landing: Landing::AtOffset(60.0),
        },
        Row {
            case: "a datetime carrying the UTC zone",
            form: "utc",
            seconds: 122.0,
            micros: 0,
            offset_minutes: 0,
            value: 3.0,
            landing: Landing::AtOffset(120.0),
        },
        Row {
            case: "a datetime at a negative zone offset",
            form: "offset",
            seconds: 183.0,
            micros: 0,
            offset_minutes: -450,
            value: 4.0,
            landing: Landing::AtOffset(180.0),
        },
        Row {
            case: "a datetime at a positive zone offset",
            form: "offset",
            seconds: 244.0,
            micros: 0,
            offset_minutes: 330,
            value: 5.0,
            landing: Landing::AtOffset(240.0),
        },
        Row {
            case: "no time stamp at all: the service stamps the datapoint",
            form: "absent",
            seconds: 0.0,
            micros: 0,
            offset_minutes: 0,
            value: 6.0,
            landing: Landing::AtTheClock,
        },
        Row {
            case: "a value that is no date-time at all is still refused by the SDK",
            form: "unsendable",
            seconds: 305.0,
            micros: 0,
            offset_minutes: 0,
            value: 7.0,
            landing: Landing::Nowhere,
        },
    ]
}

/// Assert the one datapoint `metric` holds is the one `row` published, where `row` says.
async fn assert_landed(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    start: f64,
    value: f64,
    landing: Landing,
) {
    // Thirty minutes back to ten minutes on: wide enough to hold both the instants the rows
    // name and whatever instant the service's own clock stamps.
    let held = datapoints(router, case, metric, start, start + 40.0 * MINUTE).await;
    match landing {
        Landing::Nowhere => assert!(
            held.is_empty(),
            "case {case:?}: expected no datapoint at all, got {held:?}"
        ),
        Landing::AtOffset(offset) => {
            let expected = start + (offset / MINUTE).floor() * MINUTE;
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: expected exactly one datapoint, got {held:?}"
            );
            let (at, sum) = held[0];
            assert!(
                (at - expected).abs() < 0.5,
                "case {case:?}: expected the datapoint in the minute at {expected}, got {at} \
                 ({held:?})"
            );
            assert!(
                (sum - value).abs() < f64::EPSILON,
                "case {case:?}: expected the datapoint to sum to {value}, got {sum} ({held:?})"
            );
        }
        Landing::AtTheClock => {
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: expected exactly one datapoint, got {held:?}"
            );
            let (at, sum) = held[0];
            // The clock's own minute is the one this suite's window ENDS in, thirty minutes
            // after its first: no instant any row names reaches it.
            assert!(
                at >= start + 30.0 * MINUTE,
                "case {case:?}: expected the service's own clock to stamp the datapoint, at \
                 {} or later, got {at} ({held:?})",
                start + 30.0 * MINUTE
            );
            assert!(
                (sum - value).abs() < f64::EPSILON,
                "case {case:?}: expected the datapoint to sum to {value}, got {sum} ({held:?})"
            );
        }
    }
}

/// One publication per row, asserted on both sides: what the SDK reported to the handler,
/// and where CloudWatch's own read finds the datapoint.
#[tokio::test]
async fn a_datetime_on_a_timestamp_member_travels_as_the_instant_it_names() {
    let router = router();
    let start = window_start();
    create_function(
        &router,
        "the publishing handler",
        "metric-stamper",
        "python3.12",
        "lambda_function.lambda_handler",
        "lambda_function.py",
        PYTHON_HANDLER,
    )
    .await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let metric = format!("Stamped{index}");
        let seconds = start + row.seconds;
        let (signal, payload) = invoke(
            &router,
            case,
            "metric-stamper",
            &json!({
                "namespace": NAMESPACE,
                "metric": metric,
                "value": row.value,
                "form": row.form,
                "seconds": seconds,
                "micros": row.micros,
                "offset_minutes": row.offset_minutes,
            }),
        )
        .await;
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        if row.landing == Landing::Nowhere {
            assert_eq!(
                payload["ok"],
                json!(false),
                "case {case:?}: expected the SDK to refuse the value, got {payload}"
            );
            let refused = payload["refused"].as_str().unwrap_or_default();
            assert!(
                refused.starts_with("TypeError")
                    && refused.contains("is not a value an AWS SDK call can carry"),
                "case {case:?}: expected the SDK's own refusal of an unsendable value, got \
                 {payload}"
            );
        } else {
            assert_eq!(
                payload["ok"],
                json!(true),
                "case {case:?}: expected the publish to be dispatched and answered, got \
                 {payload}"
            );
            assert_eq!(
                payload["status"],
                json!(200),
                "case {case:?}: expected the SDK to report HTTP 200, got {payload}"
            );
        }
        assert_landed(&router, case, &metric, start, row.value, row.landing).await;
    }
}

/// The same rule on ANOTHER operation's timestamp members: the handler reads its own series
/// back over a window whose `StartTime` and `EndTime` are `datetime`s, and sees the datapoint
/// in the window that covers its instant and nothing in the window that does not.
#[tokio::test]
async fn a_handler_reads_its_own_series_back_over_a_datetime_window() {
    let router = router();
    let start = window_start();
    let case = "a datetime window on GetMetricStatistics";
    create_function(
        &router,
        case,
        "metric-reader",
        "python3.12",
        "lambda_function.lambda_handler",
        "lambda_function.py",
        PYTHON_HANDLER,
    )
    .await;

    let metric = "ReadBack";
    let (signal, payload) = invoke(
        &router,
        case,
        "metric-reader",
        &json!({
            "namespace": NAMESPACE,
            "metric": metric,
            "value": 11.0,
            "form": "naive",
            "seconds": start + 120.0,
            "micros": 0,
            "offset_minutes": 0,
        }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the publishing invocation must run to completion, got {signal:?} with \
         {payload}"
    );
    assert_eq!(
        payload["ok"],
        json!(true),
        "case {case:?}: expected the publish to be answered, got {payload}"
    );

    // (which window the handler asks for, the sums it must read back)
    let windows: &[(&str, f64, f64, Vec<f64>)] = &[
        (
            "a window covering the datapoint's own minute",
            start,
            start + 10.0 * MINUTE,
            vec![11.0],
        ),
        (
            "a window that ends before it",
            start,
            start + MINUTE,
            vec![],
        ),
        (
            "a window that begins after it",
            start + 10.0 * MINUTE,
            start + 20.0 * MINUTE,
            vec![],
        ),
    ];
    for (window, from, to, expected) in windows {
        let (signal, payload) = invoke(
            &router,
            window,
            "metric-reader",
            &json!({
                "read": true,
                "namespace": NAMESPACE,
                "metric": metric,
                "seconds": from,
                "end_seconds": to,
                "period": PERIOD,
            }),
        )
        .await;
        assert_eq!(
            signal, None,
            "case {window:?}: the reading invocation must run to completion, got {signal:?} \
             with {payload}"
        );
        assert_eq!(
            payload["ok"],
            json!(true),
            "case {window:?}: expected the read to be dispatched and answered, got {payload}"
        );
        let sums: Vec<f64> = payload["sums"]
            .as_array()
            .map(|listed| listed.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        assert_eq!(
            sums, *expected,
            "case {window:?}: expected the sums {expected:?} inside the window, got {sums:?} \
             ({payload})"
        );
    }
}

/// The same rule in the other runtime: the `Date` a node handler puts on the member travels
/// as the instant it names, not as the ISO text a `Date` reports itself as.
#[tokio::test]
async fn a_date_a_node_handler_passes_for_a_timestamp_member_travels_as_its_instant() {
    let router = router();
    let start = window_start();
    let case = "a Date on MetricDatum.Timestamp";
    create_function(
        &router,
        case,
        "metric-stamper-node",
        "nodejs22.x",
        "index.handler",
        "index.mjs",
        NODE_HANDLER,
    )
    .await;

    let metric = "StampedByNode";
    let (signal, payload) = invoke(
        &router,
        case,
        "metric-stamper-node",
        &json!({
            "region": REGION,
            "namespace": NAMESPACE,
            "metric": metric,
            "value": 9.0,
            "form": "date",
            "seconds": start + 305.0,
            "millis": 250,
        }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion, got {signal:?} with {payload}"
    );
    assert_eq!(
        payload["ok"],
        json!(true),
        "case {case:?}: expected the publish to be dispatched and answered, got {payload}"
    );
    assert_landed(&router, case, metric, start, 9.0, Landing::AtOffset(300.0)).await;
}
