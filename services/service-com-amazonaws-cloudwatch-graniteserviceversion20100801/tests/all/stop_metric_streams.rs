// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `StopMetricStreams`: the state transition it owns, read back
//! through `GetMetricStream`.
//!
//! The rule under test is that the call puts every metric stream it names into the `stopped`
//! state, changes nothing else about any stream, and accepts a name no stream exists under.
//! Every table row is a different batch along what that rule generalises over — how many
//! names the batch carries, what state each named stream was in, whether a name belongs to a
//! configured stream at all, whether a name repeats, and how long a name is — so the recorded
//! happy path (one running stream stopped) is one row among many.

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use crate::harness::{build_router_with_engine, call, error_code, ok, DEFAULT_REGION};

/// The two states a metric stream can be in: "The state of the metric stream. The possible
/// values are `running` and `stopped`." (`GetMetricStreamOutput$State`).
const RUNNING: &str = "running";
/// The stopped state of that pair.
const STOPPED: &str = "stopped";

/// The streams every row of the first table is set up with: one per output format a stream
/// can carry, so the transition is asserted on streams that differ in configuration as well
/// as in state.
const STREAMS: [(&str, &str); 3] = [
    ("stream-json", "json"),
    ("stream_otel_07", "opentelemetry0.7"),
    ("stream-OTEL-10", "opentelemetry1.0"),
];

/// A `PutMetricStream` request with only the members the model makes required.
fn put_request(name: &str, output_format: &str) -> Value {
    json!({
        "Name": name,
        "FirehoseArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/metrics",
        "RoleArn": "arn:aws:iam::000000000000:role/MetricStreamRole",
        "OutputFormat": output_format,
    })
}

/// Put the stored stream `name` into `state` through the shared store every handler reads.
///
/// The row's STARTING state is set up here rather than by calling the operation under test,
/// so what this suite asserts about `StopMetricStreams` never rests on `StopMetricStreams`
/// itself — the seam `build_router_with_engine` exists for.
fn force_state(engine: &engine::InMemoryEngine, name: &str, state: &str) {
    let store = cloudwatch::state::service_state(engine).expect("the CloudWatch store exists");
    let mut store = store.lock();
    let stored = store
        .metric_streams
        .get_mut(&cloudwatch::state::metric_stream_key(DEFAULT_REGION, name))
        .unwrap_or_else(|| panic!("the stream {name} must be configured before its state is set"));
    stored.stream.state = Some(state.to_string());
}

/// One stream as `GetMetricStream` reports it, requiring the read to succeed.
async fn stream_of(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> Value {
    ok(router, "GetMetricStream", &json!({"Name": name})).await
}

/// One row of the state table: the case name, the state each of [`STREAMS`] is in before the
/// call, the names the batch carries, and the state each of [`STREAMS`] must be in after it.
type StateRow = (
    &'static str,
    [&'static str; 3],
    Vec<&'static str>,
    [&'static str; 3],
);

#[tokio::test]
async fn stop_metric_streams_stops_exactly_the_configured_streams_it_names() {
    // (case, the state each of STREAMS is in before the call, the names the batch carries,
    //  the state each of STREAMS must be in after it)
    let cases: Vec<StateRow> = vec![
        (
            "one running stream named",
            [RUNNING, RUNNING, RUNNING],
            vec!["stream-json"],
            [STOPPED, RUNNING, RUNNING],
        ),
        (
            "two of three running streams named",
            [RUNNING, RUNNING, RUNNING],
            vec!["stream-json", "stream-OTEL-10"],
            [STOPPED, RUNNING, STOPPED],
        ),
        (
            "every configured stream named, one of them already stopped",
            [RUNNING, STOPPED, RUNNING],
            vec!["stream-json", "stream_otel_07", "stream-OTEL-10"],
            [STOPPED, STOPPED, STOPPED],
        ),
        (
            "an already stopped stream named",
            [STOPPED, RUNNING, RUNNING],
            vec!["stream-json"],
            [STOPPED, RUNNING, RUNNING],
        ),
        (
            "no name at all",
            [RUNNING, STOPPED, RUNNING],
            vec![],
            [RUNNING, STOPPED, RUNNING],
        ),
        (
            "the same name twice",
            [RUNNING, RUNNING, RUNNING],
            vec!["stream-json", "stream-json"],
            [STOPPED, RUNNING, RUNNING],
        ),
        (
            "a name no stream exists under",
            [RUNNING, RUNNING, RUNNING],
            vec!["no-such-stream"],
            [RUNNING, RUNNING, RUNNING],
        ),
        (
            "an unknown name beside a configured one",
            [RUNNING, RUNNING, RUNNING],
            vec!["no-such-stream", "stream_otel_07"],
            [RUNNING, STOPPED, RUNNING],
        ),
        (
            "a name differing from a configured one only in case",
            [RUNNING, RUNNING, RUNNING],
            vec!["STREAM-JSON"],
            [RUNNING, RUNNING, RUNNING],
        ),
    ];

    for (case, before_states, names, after_states) in cases {
        let (router, engine) = build_router_with_engine();
        let mut before = Vec::new();
        for (index, (name, format)) in STREAMS.iter().enumerate() {
            ok(&router, "PutMetricStream", &put_request(name, format)).await;
            force_state(&engine, name, before_states[index]);
            let configured = stream_of(&router, name).await;
            assert_eq!(
                configured.get("State").and_then(Value::as_str),
                Some(before_states[index]),
                "case {case:?}: the row sets {name} up as {:?}, got {configured}",
                before_states[index]
            );
            before.push(configured);
        }

        let (status, body) = call(&router, "StopMetricStreams", &json!({"Names": names})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StopMetricStreams, got {status} {body}"
        );

        for (index, (name, _)) in STREAMS.iter().enumerate() {
            let after = stream_of(&router, name).await;
            // Only `State` may differ, beside the `LastUpdateDate` a TRANSITION stamps
            // ("The date that the configuration of this metric stream was most recently
            // updated.", `GetMetricStreamOutput$LastUpdateDate`): stopping a stream updates no
            // configuration member, so the ARN, the destination, the format, the filters and
            // the creation date are all exactly what they were, and a stream the call moved
            // nowhere keeps its update date too.
            let mut expected = before[index].clone();
            expected["State"] = json!(after_states[index]);
            if before_states[index] != after_states[index] {
                // Two calls inside one millisecond stamp the same instant, which the
                // timestamp type cannot tell apart, so the date never moves BACKWARDS.
                assert!(
                    after.get("LastUpdateDate").and_then(Value::as_f64)
                        >= before[index].get("LastUpdateDate").and_then(Value::as_f64),
                    "case {case:?}: stopping {name} transitioned it, so LastUpdateDate must \
                     not move back, got {after}"
                );
                expected["LastUpdateDate"] = after["LastUpdateDate"].clone();
            }
            assert_eq!(
                after, expected,
                "case {case:?}: expected {name} to be reported as {expected}, got {after}"
            );
        }

        // The call persists no stream of its own: a name no stream exists under is still
        // unknown afterwards, whichever row named it.
        for unknown in ["no-such-stream", "STREAM-JSON"] {
            let (get_status, get_body) =
                call(&router, "GetMetricStream", &json!({"Name": unknown})).await;
            assert_eq!(
                (get_status, error_code(&get_body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: StopMetricStreams must not create the stream {unknown:?}, got \
                 {get_status} {get_body}"
            );
        }
    }
}

#[tokio::test]
async fn a_batch_of_any_size_stops_every_stream_it_names_and_repeats_cleanly() {
    // The rule holds whatever the batch's size, so the rows differ only in how many running
    // streams one call stops. Each batch also carries a name no stream exists under, which
    // the operation has no modelled error to report and therefore accepts.
    for count in [1_usize, 2, 8, 25] {
        let case = format!("{count} running streams stopped in one call");
        let (router, _engine) = build_router_with_engine();
        let names: Vec<String> = (0..count).map(|index| format!("batch_{index}")).collect();
        for name in &names {
            // `PutMetricStream` creates a stream running, so no setup beyond the write is
            // needed for this table.
            ok(&router, "PutMetricStream", &put_request(name, "json")).await;
        }
        let mut requested = names.clone();
        requested.push("no-such-stream".to_string());

        let (status, body) = call(&router, "StopMetricStreams", &json!({"Names": requested})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StopMetricStreams, got {status} {body}"
        );

        let mut stopped = Vec::new();
        for name in &names {
            let got = stream_of(&router, name).await;
            assert_eq!(
                got.get("State").and_then(Value::as_str),
                Some(STOPPED),
                "case {case:?}: expected {name} to be stopped, got {got}"
            );
            stopped.push(got);
        }

        // Stopping what is already stopped asks for the state the service is already in, so
        // the second call reports exactly the same streams as the first.
        let (again_status, again_body) =
            call(&router, "StopMetricStreams", &json!({"Names": names})).await;
        assert_eq!(
            again_status, 200,
            "case {case:?}: expected HTTP 200 from the second StopMetricStreams, got \
             {again_status} {again_body}"
        );
        for (name, first) in names.iter().zip(&stopped) {
            let after = stream_of(&router, name).await;
            assert_eq!(
                &after, first,
                "case {case:?}: stopping {name} twice must leave the same stream, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_outside_the_modelled_length_refuses_the_whole_batch() {
    // `MetricStreamNames`'s member is a `MetricStreamName`, length 1..255 (the model's own
    // constraint), so a batch carrying a name outside that range is refused before any stream
    // is stopped, while a name AT the bound is a well-formed name the batch simply holds no
    // stream for. Rows: each published bound, one past each, and the empty name.
    let cases: Vec<(&str, String, bool)> = vec![
        ("a name of 1 character", "a".to_string(), true),
        ("a name of 255 characters", "n".repeat(255), true),
        ("a name of 256 characters", "n".repeat(256), false),
        ("the empty name", String::new(), false),
    ];

    for (case, name, accepted) in cases {
        let (router, _engine) = build_router_with_engine();
        ok(&router, "PutMetricStream", &put_request("stream-json", "json")).await;

        let (status, body) = call(
            &router,
            "StopMetricStreams",
            &json!({"Names": [name, "stream-json"]}),
        )
        .await;
        let got = stream_of(&router, "stream-json").await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a name inside the modelled length must be accepted, got \
                 {status} {body}"
            );
            assert_eq!(
                got.get("State").and_then(Value::as_str),
                Some(STOPPED),
                "case {case:?}: the configured stream in the batch must be stopped, got {got}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a name outside the modelled length must be refused with HTTP \
                 400, got {status} {body}"
            );
            assert_eq!(
                got.get("State").and_then(Value::as_str),
                Some(RUNNING),
                "case {case:?}: a refused batch must stop none of its streams, got {got}"
            );
        }
    }
}

#[tokio::test]
async fn stopping_and_starting_toggles_the_state_and_nothing_else() {
    // The two operations are inverses on `State` alone, so a stream's whole configuration —
    // including whichever of the two mutually exclusive namespace filters it carries — must
    // come back unchanged across any sequence of stops and starts. The rows differ in the
    // configuration being toggled.
    let configurations: Vec<(&str, Value)> = vec![
        ("no filter at all", put_request("toggle_plain", "json")),
        (
            "an exclude filter",
            json!({
                "Name": "toggle_exclude",
                "FirehoseArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/metrics",
                "RoleArn": "arn:aws:iam::000000000000:role/MetricStreamRole",
                "OutputFormat": "json",
                "ExcludeFilters": [
                    {"Namespace": "AWS/EC2"},
                    {"Namespace": "AWS/Lambda", "MetricNames": ["Invocations", "Errors"]},
                ],
            }),
        ),
        (
            "an include filter and a statistics configuration",
            json!({
                "Name": "toggle_include",
                "FirehoseArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/metrics",
                "RoleArn": "arn:aws:iam::000000000000:role/MetricStreamRole",
                "OutputFormat": "opentelemetry1.0",
                "IncludeFilters": [{"Namespace": "MyApp", "MetricNames": ["Latency"]}],
                "StatisticsConfigurations": [{
                    "IncludeMetrics": [{"Namespace": "MyApp", "MetricName": "Latency"}],
                    "AdditionalStatistics": ["p99"],
                }],
            }),
        ),
    ];

    // Each step is one call and the state it must leave the stream in. Stop and start are
    // both idempotent, so the sequence repeats each one.
    const SEQUENCE: [(&str, &str); 6] = [
        ("StopMetricStreams", STOPPED),
        ("StopMetricStreams", STOPPED),
        ("StartMetricStreams", RUNNING),
        ("StartMetricStreams", RUNNING),
        ("StopMetricStreams", STOPPED),
        ("StartMetricStreams", RUNNING),
    ];

    for (case, request) in configurations {
        let (router, _engine) = build_router_with_engine();
        ok(&router, "PutMetricStream", &request).await;
        let name = request["Name"].as_str().expect("the row names its stream");
        let created = stream_of(&router, name).await;
        assert_eq!(
            created.get("State").and_then(Value::as_str),
            Some(RUNNING),
            "case {case:?}: PutMetricStream creates a stream running, got {created}"
        );

        // The stream the sequence has reported so far, so each step is compared against what
        // the step before it left behind: `LastUpdateDate` moves with every transition.
        let mut before = created.clone();
        for (operation, expected_state) in SEQUENCE {
            let (status, body) = call(&router, operation, &json!({"Names": [name]})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from {operation}, got {status} {body}"
            );
            let after = stream_of(&router, name).await;
            let mut expected = before.clone();
            expected["State"] = json!(expected_state);
            // A transition stamps "the date that the configuration of this metric stream was
            // most recently updated" (`GetMetricStreamOutput$LastUpdateDate`); a call that
            // asks for the state the stream is already in moves it nowhere and stamps
            // nothing, which is what makes both operations idempotent.
            if before.get("State").and_then(Value::as_str) != Some(expected_state) {
                // Two calls inside one millisecond stamp the same instant, which the
                // timestamp type cannot tell apart, so the date never moves BACKWARDS.
                assert!(
                    after.get("LastUpdateDate").and_then(Value::as_f64)
                        >= before.get("LastUpdateDate").and_then(Value::as_f64),
                    "case {case:?}: {operation} transitioned the stream, so LastUpdateDate \
                     must not move back, got {after} after {before}"
                );
                expected["LastUpdateDate"] = after["LastUpdateDate"].clone();
            }
            assert_eq!(
                after, expected,
                "case {case:?}: after {operation} expected {expected}, got {after}"
            );
            before = after;
        }
    }
}
