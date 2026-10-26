// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `StartMetricStreams`: the state transition it owns, read back
//! through `GetMetricStream`.
//!
//! The rule under test is that the call puts every metric stream it names into the `running`
//! state, changes nothing else about any stream, and accepts a name no stream exists under.
//! Every table row is a different batch along what that rule generalises over — how many
//! names the batch carries, what state each named stream was in, whether a name belongs to a
//! configured stream at all, whether a name repeats, and how long a name is — so the recorded
//! happy path (one running stream started again) is one row among many.

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use crate::harness::{build_router_with_engine, call, error_code, ok};

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

/// Put the stored stream `name` into the `stopped` state.
///
/// `PutMetricStream` creates a stream running and the operation that stops one is still a
/// scaffold stub in this emulator (`cloudwatch-stopmetricstreams-stub`), so the only way a
/// test can set up a stopped stream today is through the shared store every handler reads —
/// the seam `build_router_with_engine` exists for.
fn force_stopped(engine: &engine::InMemoryEngine, name: &str) {
    let state = cloudwatch::state::service_state(engine).expect("the CloudWatch store exists");
    let mut store = state.lock();
    let stored = store
        .metric_streams
        .get_mut(name)
        .unwrap_or_else(|| panic!("the stream {name} must be configured before it is stopped"));
    stored.stream.state = Some(STOPPED.to_string());
}

/// One stream as `GetMetricStream` reports it, requiring the read to succeed.
async fn stream_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Value {
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
async fn start_metric_streams_runs_exactly_the_configured_streams_it_names() {
    // (case, the state each of STREAMS is in before the call, the names the batch carries,
    //  the state each of STREAMS must be in after it)
    let cases: Vec<StateRow> = vec![
        (
            "one stopped stream named",
            [STOPPED, STOPPED, STOPPED],
            vec!["stream-json"],
            [RUNNING, STOPPED, STOPPED],
        ),
        (
            "two of three stopped streams named",
            [STOPPED, STOPPED, STOPPED],
            vec!["stream-json", "stream-OTEL-10"],
            [RUNNING, STOPPED, RUNNING],
        ),
        (
            "every configured stream named, one of them already running",
            [STOPPED, RUNNING, STOPPED],
            vec!["stream-json", "stream_otel_07", "stream-OTEL-10"],
            [RUNNING, RUNNING, RUNNING],
        ),
        (
            "an already running stream named",
            [RUNNING, STOPPED, STOPPED],
            vec!["stream-json"],
            [RUNNING, STOPPED, STOPPED],
        ),
        (
            "no name at all",
            [STOPPED, RUNNING, STOPPED],
            vec![],
            [STOPPED, RUNNING, STOPPED],
        ),
        (
            "the same name twice",
            [STOPPED, STOPPED, STOPPED],
            vec!["stream-json", "stream-json"],
            [RUNNING, STOPPED, STOPPED],
        ),
        (
            "a name no stream exists under",
            [STOPPED, STOPPED, STOPPED],
            vec!["no-such-stream"],
            [STOPPED, STOPPED, STOPPED],
        ),
        (
            "an unknown name beside a configured one",
            [STOPPED, STOPPED, STOPPED],
            vec!["no-such-stream", "stream_otel_07"],
            [STOPPED, RUNNING, STOPPED],
        ),
        (
            "a name differing from a configured one only in case",
            [STOPPED, STOPPED, STOPPED],
            vec!["STREAM-JSON"],
            [STOPPED, STOPPED, STOPPED],
        ),
    ];

    for (case, before_states, names, after_states) in cases {
        let (router, engine) = build_router_with_engine();
        let mut before = Vec::new();
        for (index, (name, format)) in STREAMS.iter().enumerate() {
            ok(&router, "PutMetricStream", &put_request(name, format)).await;
            if before_states[index] == STOPPED {
                force_stopped(&engine, name);
            }
            let configured = stream_of(&router, name).await;
            assert_eq!(
                configured.get("State").and_then(Value::as_str),
                Some(before_states[index]),
                "case {case:?}: the row sets {name} up as {:?}, got {configured}",
                before_states[index]
            );
            before.push(configured);
        }

        let (status, body) = call(&router, "StartMetricStreams", &json!({"Names": names})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartMetricStreams, got {status} {body}"
        );

        for (index, (name, _)) in STREAMS.iter().enumerate() {
            let after = stream_of(&router, name).await;
            // Only `State` may differ: starting a stream updates no configuration member, so
            // the ARN, the destination, the format, the filters, the creation date and the
            // last configuration update date are all exactly what they were.
            let mut expected = before[index].clone();
            expected["State"] = json!(after_states[index]);
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
                "case {case:?}: StartMetricStreams must not create the stream {unknown:?}, got \
                 {get_status} {get_body}"
            );
        }
    }
}

#[tokio::test]
async fn a_batch_of_any_size_starts_every_stream_it_names_and_repeats_cleanly() {
    // The rule holds whatever the batch's size, so the rows differ only in how many stopped
    // streams one call starts. Each batch also carries a name no stream exists under, which
    // the operation has no modelled error to report and therefore accepts.
    for count in [1_usize, 2, 8, 25] {
        let case = format!("{count} stopped streams started in one call");
        let (router, engine) = build_router_with_engine();
        let names: Vec<String> = (0..count).map(|index| format!("batch_{index}")).collect();
        for name in &names {
            ok(&router, "PutMetricStream", &put_request(name, "json")).await;
            force_stopped(engine.as_ref(), name);
        }
        let mut requested = names.clone();
        requested.push("no-such-stream".to_string());

        let (status, body) =
            call(&router, "StartMetricStreams", &json!({"Names": requested})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartMetricStreams, got {status} {body}"
        );

        let mut started = Vec::new();
        for name in &names {
            let got = stream_of(&router, name).await;
            assert_eq!(
                got.get("State").and_then(Value::as_str),
                Some(RUNNING),
                "case {case:?}: expected {name} to be running, got {got}"
            );
            started.push(got);
        }

        // Starting what is already running asks for the state the service is already in, so
        // the second call reports exactly the same streams as the first.
        let (again_status, again_body) =
            call(&router, "StartMetricStreams", &json!({"Names": names})).await;
        assert_eq!(
            again_status, 200,
            "case {case:?}: expected HTTP 200 from the second StartMetricStreams, got \
             {again_status} {again_body}"
        );
        for (name, first) in names.iter().zip(&started) {
            let after = stream_of(&router, name).await;
            assert_eq!(
                &after, first,
                "case {case:?}: starting {name} twice must leave the same stream, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_outside_the_modelled_length_refuses_the_whole_batch() {
    // `MetricStreamNames`'s member is a `MetricStreamName`, length 1..255 (the model's own
    // constraint), so a batch carrying a name outside that range is refused before any stream
    // is started, while a name AT the bound is a well-formed name the batch simply holds no
    // stream for. Rows: each published bound, one past each, and the empty name.
    let cases: Vec<(&str, String, bool)> = vec![
        ("a name of 1 character", "a".to_string(), true),
        ("a name of 255 characters", "n".repeat(255), true),
        ("a name of 256 characters", "n".repeat(256), false),
        ("the empty name", String::new(), false),
    ];

    for (case, name, accepted) in cases {
        let (router, engine) = build_router_with_engine();
        ok(&router, "PutMetricStream", &put_request("stream-json", "json")).await;
        force_stopped(engine.as_ref(), "stream-json");

        let (status, body) = call(
            &router,
            "StartMetricStreams",
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
                Some(RUNNING),
                "case {case:?}: the configured stream in the batch must be running, got {got}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a name outside the modelled length must be refused with HTTP \
                 400, got {status} {body}"
            );
            assert_eq!(
                got.get("State").and_then(Value::as_str),
                Some(STOPPED),
                "case {case:?}: a refused batch must start none of its streams, got {got}"
            );
        }
    }
}
