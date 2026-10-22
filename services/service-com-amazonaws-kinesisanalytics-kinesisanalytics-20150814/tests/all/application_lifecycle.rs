// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Kinesis Analytics (v1, SQL) application lifecycle:
//! `CreateApplication`, `DescribeApplication`, `ListApplications`, `StartApplication`,
//! `StopApplication` and `DeleteApplication`.
//!
//! Every case drives the service through the same `MultiServiceRouter` the emulator
//! mounts, so each request takes the production walk (deserialize → validate → handle →
//! serialize) over the awsJson1_1 wire.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kinesisanalytics_kinesisanalytics_20150814::ops::types::{
    APPLICATION_STATUS_VALUES, INPUT_STARTING_POSITION_VALUES,
};
use service_com_amazonaws_kinesisanalytics_kinesisanalytics_20150814::state::{
    ApplicationStatus, TRANSITION_SPAN,
};
use service_com_amazonaws_kinesisanalytics_kinesisanalytics_20150814::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for. Deliberately NOT the emulator's
/// `us-east-1` fallback, so a stored ARN proves it came from the request.
const REGION: &str = "us-west-2";

/// How long to wait for a status move the clock owes to come due.
fn settle_wait() -> Duration {
    TRANSITION_SPAN + Duration::from_millis(400)
}

/// The ARN `CreateApplication` must mint for `name`.
fn expected_arn(name: &str) -> String {
    format!(
        "arn:aws:kinesisanalytics:{REGION}:{}:application/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One emulator with the Kinesis Analytics service mounted on a shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

/// One answered request.
struct Call {
    status: u16,
    body: Value,
    error_type_header: Option<String>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not under test here; the request still carries a SigV4
        // credential scope, because that is what routes it and names its region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, operation: &str, body: Value) -> Call {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("KinesisAnalytics_20150814.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTKINESISANALYTICS/20261021/{REGION}/kinesisanalytics/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("serialize the request body"),
            ))
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type_header = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let payload = response.body();
        let body = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(payload).unwrap_or(Value::Null)
        };
        Call {
            status,
            body,
            error_type_header,
        }
    }

    /// `CreateApplication` that must succeed, answering the request it sent.
    async fn create(&self, case: &str, request: Value) -> Value {
        let created = self.call("CreateApplication", request.clone()).await;
        let summary = &created.ok(case)["ApplicationSummary"];
        assert_eq!(
            summary["ApplicationName"], request["ApplicationName"],
            "case {case:?}: CreateApplication must report the stored name, got {summary}"
        );
        request
    }

    /// `DescribeApplication` that must succeed, answering the `ApplicationDetail`.
    async fn describe(&self, case: &str, name: &str) -> Value {
        let described = self
            .call("DescribeApplication", json!({"ApplicationName": name}))
            .await;
        described.ok(case)["ApplicationDetail"].clone()
    }

    /// The status `DescribeApplication` reports for `name`.
    async fn status_of(&self, case: &str, name: &str) -> String {
        self.describe(case, name).await["ApplicationStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

impl Call {
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            self.status, self.body
        );
        &self.body
    }

    /// The wire error code, from the body's `__type` or the error-type header.
    fn code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .or(self.error_type_header.as_deref())
            .map(|wire| wire.split('#').next_back().unwrap_or(wire).to_string())
            .unwrap_or_default()
    }

    fn assert_error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.code().as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got HTTP {} {:?} with body {}",
            self.status,
            self.code(),
            self.body
        );
    }
}

/// A one-input, one-output application request named `name`.
fn simple_application(name: &str) -> Value {
    json!({
        "ApplicationName": name,
        "ApplicationCode": "CREATE OR REPLACE STREAM \"OUT\" (ticker VARCHAR(4));",
        "Inputs": [{
            "NamePrefix": "SOURCE_SQL_STREAM",
            "KinesisStreamsInput": {
                "ResourceARN": "arn:aws:kinesis:us-west-2:000000000000:stream/trades",
                "RoleARN": "arn:aws:iam::000000000000:role/ka-read"
            },
            "InputSchema": {
                "RecordFormat": {"RecordFormatType": "JSON",
                                 "MappingParameters": {"JSONMappingParameters": {"RecordRowPath": "$"}}},
                "RecordColumns": [{"Name": "ticker", "SqlType": "VARCHAR(4)", "Mapping": "$.ticker"}]
            }
        }]
    })
}

/// Assert an `ApplicationDetail` reports exactly what the create request stored.
fn assert_detail_matches_request(case: &str, request: &Value, detail: &Value) {
    let empty: Vec<Value> = Vec::new();
    let name = request["ApplicationName"]
        .as_str()
        .expect("the case names the application");

    assert_eq!(
        detail["ApplicationName"].as_str(),
        Some(name),
        "case {case:?}: ApplicationName must come from the store, got {detail}"
    );
    assert_eq!(
        detail["ApplicationARN"].as_str(),
        Some(expected_arn(name).as_str()),
        "case {case:?}: ApplicationARN must name the signed region and the emulator account, got {}",
        detail["ApplicationARN"]
    );
    assert_eq!(
        detail["ApplicationStatus"].as_str(),
        Some("READY"),
        "case {case:?}: a created application is READY, got {}",
        detail["ApplicationStatus"]
    );
    assert_eq!(
        detail["ApplicationVersionId"].as_i64(),
        Some(1),
        "case {case:?}: ApplicationVersionId starts at 1, got {}",
        detail["ApplicationVersionId"]
    );
    assert_eq!(
        detail["ApplicationDescription"], request["ApplicationDescription"],
        "case {case:?}: ApplicationDescription must round-trip, got {}",
        detail["ApplicationDescription"]
    );
    assert_eq!(
        detail["ApplicationCode"], request["ApplicationCode"],
        "case {case:?}: ApplicationCode must round-trip, got {}",
        detail["ApplicationCode"]
    );
    let created = detail["CreateTimestamp"].as_f64().unwrap_or_default();
    assert!(
        created > 1_000_000_000.0,
        "case {case:?}: CreateTimestamp must be a real epoch-seconds value, got {}",
        detail["CreateTimestamp"]
    );
    assert!(
        detail["LastUpdateTimestamp"].as_f64().unwrap_or_default() >= created,
        "case {case:?}: LastUpdateTimestamp must not precede CreateTimestamp, got {}",
        detail["LastUpdateTimestamp"]
    );

    let sent_inputs = request["Inputs"].as_array().unwrap_or(&empty);
    let got_inputs = detail["InputDescriptions"].as_array().unwrap_or(&empty);
    assert_eq!(
        got_inputs.len(),
        sent_inputs.len(),
        "case {case:?}: one InputDescription per configured input, got {got_inputs:?}"
    );
    for (sent, got) in sent_inputs.iter().zip(got_inputs) {
        assert_eq!(
            got["NamePrefix"], sent["NamePrefix"],
            "case {case:?}: NamePrefix must round-trip, got {got}"
        );
        assert_eq!(
            got["InputSchema"], sent["InputSchema"],
            "case {case:?}: InputSchema must round-trip, got {got}"
        );
        assert_eq!(
            got["KinesisStreamsInputDescription"]["ResourceARN"],
            sent["KinesisStreamsInput"]["ResourceARN"],
            "case {case:?}: the Kinesis stream source must round-trip, got {got}"
        );
        assert_eq!(
            got["KinesisStreamsInputDescription"]["RoleARN"], sent["KinesisStreamsInput"]["RoleARN"],
            "case {case:?}: the Kinesis stream role must round-trip, got {got}"
        );
        assert_eq!(
            got["KinesisFirehoseInputDescription"]["ResourceARN"],
            sent["KinesisFirehoseInput"]["ResourceARN"],
            "case {case:?}: the Firehose source must round-trip, got {got}"
        );
        assert_eq!(
            got["InputProcessingConfigurationDescription"]["InputLambdaProcessorDescription"]
                ["ResourceARN"],
            sent["InputProcessingConfiguration"]["InputLambdaProcessor"]["ResourceARN"],
            "case {case:?}: the input processor must round-trip, got {got}"
        );
        // `InputParallelism.Count` defaults to one in-application stream.
        let count = sent["InputParallelism"]["Count"].as_i64().unwrap_or(1);
        assert_eq!(
            got["InputParallelism"]["Count"].as_i64(),
            Some(count),
            "case {case:?}: InputParallelism.Count must round-trip, got {got}"
        );
        // The `NamePrefix` doc: a prefix "MyInApplicationStream" yields
        // "MyInApplicationStream_001", "_002", ... one per parallelism count.
        let prefix = sent["NamePrefix"].as_str().unwrap_or_default();
        let expected_streams: Vec<String> =
            (1..=count).map(|n| format!("{prefix}_{n:03}")).collect();
        let got_streams: Vec<String> = got["InAppStreamNames"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            got_streams, expected_streams,
            "case {case:?}: InAppStreamNames must be derived from NamePrefix and the parallelism count"
        );
    }

    let sent_outputs = request["Outputs"].as_array().unwrap_or(&empty);
    let got_outputs = detail["OutputDescriptions"].as_array().unwrap_or(&empty);
    assert_eq!(
        got_outputs.len(),
        sent_outputs.len(),
        "case {case:?}: one OutputDescription per configured output, got {got_outputs:?}"
    );
    for (sent, got) in sent_outputs.iter().zip(got_outputs) {
        assert_eq!(
            got["Name"], sent["Name"],
            "case {case:?}: the output name must round-trip, got {got}"
        );
        assert_eq!(
            got["DestinationSchema"], sent["DestinationSchema"],
            "case {case:?}: DestinationSchema must round-trip, got {got}"
        );
        assert_eq!(
            got["KinesisStreamsOutputDescription"]["ResourceARN"],
            sent["KinesisStreamsOutput"]["ResourceARN"],
            "case {case:?}: a Kinesis stream destination must round-trip, got {got}"
        );
        assert_eq!(
            got["KinesisFirehoseOutputDescription"]["ResourceARN"],
            sent["KinesisFirehoseOutput"]["ResourceARN"],
            "case {case:?}: a Firehose destination must round-trip, got {got}"
        );
        assert_eq!(
            got["LambdaOutputDescription"]["ResourceARN"], sent["LambdaOutput"]["ResourceARN"],
            "case {case:?}: a Lambda destination must round-trip, got {got}"
        );
    }

    let sent_logs = request["CloudWatchLoggingOptions"].as_array().unwrap_or(&empty);
    let got_logs = detail["CloudWatchLoggingOptionDescriptions"]
        .as_array()
        .unwrap_or(&empty);
    assert_eq!(
        got_logs.len(),
        sent_logs.len(),
        "case {case:?}: one description per logging option, got {got_logs:?}"
    );
    for (sent, got) in sent_logs.iter().zip(got_logs) {
        assert_eq!(
            got["LogStreamARN"], sent["LogStreamARN"],
            "case {case:?}: LogStreamARN must round-trip, got {got}"
        );
        assert_eq!(
            got["RoleARN"], sent["RoleARN"],
            "case {case:?}: the logging RoleARN must round-trip, got {got}"
        );
    }

    // Every sub-resource carries a server-assigned id, and the ids are distinct.
    let mut ids: Vec<String> = Vec::new();
    for (member, items) in [
        ("InputId", got_inputs),
        ("OutputId", got_outputs),
        ("CloudWatchLoggingOptionId", got_logs),
    ] {
        for item in items {
            let id = item[member].as_str().unwrap_or_default().to_string();
            assert!(
                !id.is_empty() && id.len() <= 50,
                "case {case:?}: the service must assign a {member} within Id's 1..50 length, got {item}"
            );
            ids.push(id);
        }
    }
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "case {case:?}: sub-resource ids must be distinct, got {ids:?}"
    );
}

/// The create requests this suite round-trips. The rows differ along what the rule
/// generalises over: how many inputs/outputs/logging options are configured, which
/// kind of source and destination each is, the record format, the parallelism count
/// (at both published bounds of `InputParallelism.Count`'s 1..64 range), and the form
/// and length of the application name (1 and 128, the bounds of `ApplicationName`).
fn create_cases() -> Vec<(String, Value)> {
    vec![
        (
            "minimal: name only".to_string(),
            json!({"ApplicationName": "a"}),
        ),
        (
            "kinesis stream source, one stream destination, tags".to_string(),
            json!({
                "ApplicationName": "demo-app",
                "ApplicationDescription": "trades roll-up",
                "ApplicationCode": "CREATE OR REPLACE STREAM \"OUT\" (ticker VARCHAR(4));",
                "Inputs": [{
                    "NamePrefix": "SOURCE_SQL_STREAM",
                    "KinesisStreamsInput": {
                        "ResourceARN": "arn:aws:kinesis:us-west-2:000000000000:stream/trades",
                        "RoleARN": "arn:aws:iam::000000000000:role/ka-read"
                    },
                    "InputParallelism": {"Count": 2},
                    "InputSchema": {
                        "RecordFormat": {"RecordFormatType": "JSON",
                                         "MappingParameters": {"JSONMappingParameters": {"RecordRowPath": "$"}}},
                        "RecordColumns": [{"Name": "ticker", "SqlType": "VARCHAR(4)", "Mapping": "$.ticker"}]
                    }
                }],
                "Outputs": [{
                    "Name": "OUT",
                    "KinesisStreamsOutput": {
                        "ResourceARN": "arn:aws:kinesis:us-west-2:000000000000:stream/out",
                        "RoleARN": "arn:aws:iam::000000000000:role/ka-write"
                    },
                    "DestinationSchema": {"RecordFormatType": "JSON"}
                }],
                "CloudWatchLoggingOptions": [{
                    "LogStreamARN": "arn:aws:logs:us-west-2:000000000000:log-group:ka:log-stream:errors",
                    "RoleARN": "arn:aws:iam::000000000000:role/ka-logs"
                }],
                "Tags": [{"Key": "team", "Value": "analytics"}, {"Key": "env", "Value": "test"}]
            }),
        ),
        (
            "firehose source at the parallelism upper bound, three destinations, CSV".to_string(),
            json!({
                "ApplicationName": "my_app.v2-3",
                "Inputs": [{
                    "NamePrefix": "FEED",
                    "KinesisFirehoseInput": {
                        "ResourceARN": "arn:aws:firehose:us-west-2:000000000000:deliverystream/in",
                        "RoleARN": "arn:aws:iam::000000000000:role/ka-read"
                    },
                    "InputParallelism": {"Count": 64},
                    "InputSchema": {
                        "RecordFormat": {"RecordFormatType": "CSV",
                                         "MappingParameters": {"CSVMappingParameters": {"RecordRowDelimiter": "\n", "RecordColumnDelimiter": ","}}},
                        "RecordEncoding": "UTF-8",
                        "RecordColumns": [
                            {"Name": "ticker", "SqlType": "VARCHAR(4)"},
                            {"Name": "price", "SqlType": "DOUBLE"}
                        ]
                    }
                }],
                "Outputs": [
                    {
                        "Name": "OUT_STREAM",
                        "KinesisStreamsOutput": {
                            "ResourceARN": "arn:aws:kinesis:us-west-2:000000000000:stream/out",
                            "RoleARN": "arn:aws:iam::000000000000:role/ka-write"
                        },
                        "DestinationSchema": {"RecordFormatType": "CSV"}
                    },
                    {
                        "Name": "OUT_FIREHOSE",
                        "KinesisFirehoseOutput": {
                            "ResourceARN": "arn:aws:firehose:us-west-2:000000000000:deliverystream/out",
                            "RoleARN": "arn:aws:iam::000000000000:role/ka-write"
                        },
                        "DestinationSchema": {"RecordFormatType": "JSON"}
                    },
                    {
                        "Name": "OUT_LAMBDA",
                        "LambdaOutput": {
                            "ResourceARN": "arn:aws:lambda:us-west-2:000000000000:function:sink",
                            "RoleARN": "arn:aws:iam::000000000000:role/ka-write"
                        },
                        "DestinationSchema": {"RecordFormatType": "JSON"}
                    }
                ]
            }),
        ),
        (
            "name at the 128-character upper bound, input processor, parallelism lower bound"
                .to_string(),
            json!({
                "ApplicationName": "n".repeat(128),
                "ApplicationDescription": "",
                "Inputs": [{
                    "NamePrefix": "PREPROCESSED",
                    "KinesisStreamsInput": {
                        "ResourceARN": "arn:aws:kinesis:us-west-2:000000000000:stream/raw",
                        "RoleARN": "arn:aws:iam::000000000000:role/ka-read"
                    },
                    "InputProcessingConfiguration": {
                        "InputLambdaProcessor": {
                            "ResourceARN": "arn:aws:lambda:us-west-2:000000000000:function:pre",
                            "RoleARN": "arn:aws:iam::000000000000:role/ka-invoke"
                        }
                    },
                    "InputParallelism": {"Count": 1},
                    "InputSchema": {
                        "RecordFormat": {"RecordFormatType": "JSON",
                                         "MappingParameters": {"JSONMappingParameters": {"RecordRowPath": "$"}}},
                        "RecordColumns": [{"Name": "raw", "SqlType": "VARCHAR(64)", "Mapping": "$.raw"}]
                    }
                }]
            }),
        ),
    ]
}

#[tokio::test]
async fn create_then_describe_reads_back_every_stored_field() {
    let emulator = Emulator::new();

    for (case, request) in create_cases() {
        let name = request["ApplicationName"].as_str().unwrap().to_string();

        let created = emulator.call("CreateApplication", request.clone()).await;
        let summary = &created.ok(&case)["ApplicationSummary"];
        assert_eq!(
            summary["ApplicationName"].as_str(),
            Some(name.as_str()),
            "case {case:?}: the summary names the created application, got {summary}"
        );
        assert_eq!(
            summary["ApplicationARN"].as_str(),
            Some(expected_arn(&name).as_str()),
            "case {case:?}: the summary carries the stored ARN, got {summary}"
        );
        assert_eq!(
            summary["ApplicationStatus"].as_str(),
            Some("READY"),
            "case {case:?}: a created application is READY, got {summary}"
        );

        // The round trip: everything the response claims must be readable back.
        let detail = emulator.describe(&case, &name).await;
        assert_detail_matches_request(&case, &request, &detail);
        assert_eq!(
            detail["ApplicationARN"], summary["ApplicationARN"],
            "case {case:?}: create and describe must report one stored ARN"
        );

        // And it is on the list, with the same summary.
        let listed = emulator.call("ListApplications", json!({})).await;
        let summaries = listed.ok(&case)["ApplicationSummaries"].clone();
        let no_summaries: Vec<Value> = Vec::new();
        assert!(
            summaries
                .as_array()
                .unwrap_or(&no_summaries)
                .iter()
                .any(|entry| entry == summary),
            "case {case:?}: ListApplications must report the stored summary, got {summaries}"
        );
    }
}

#[tokio::test]
async fn creating_a_name_that_already_exists_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let first = emulator
        .create("duplicate name", simple_application("twice"))
        .await;
    let before = emulator.describe("duplicate name", "twice").await;

    // Each row re-sends the same NAME with a different body: the name is what is
    // taken, whatever else the request says.
    let rows = vec![
        ("identical body", first.clone()),
        (
            "same name, different code",
            json!({"ApplicationName": "twice", "ApplicationCode": "CREATE OR REPLACE STREAM \"X\" (a INTEGER);"}),
        ),
        (
            "same name, no configuration at all",
            json!({"ApplicationName": "twice"}),
        ),
    ];

    for (case, request) in rows {
        let again = emulator.call("CreateApplication", request).await;
        again.assert_error(case, 400, "ResourceInUseException");

        let after = emulator.describe(case, "twice").await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must leave the stored record untouched"
        );
    }

    let listed = emulator.call("ListApplications", json!({})).await;
    let summaries = listed.ok("duplicate name")["ApplicationSummaries"].clone();
    assert_eq!(
        summaries.as_array().map(Vec::len),
        Some(1),
        "a refused create must not add a second record, got {summaries}"
    );
}

/// One `ListApplications` paging case: the case name, the request's `Limit` and
/// `ExclusiveStartApplicationName`, the page it must answer and the
/// `HasMoreApplications` it must report.
type PageCase = (
    &'static str,
    Option<i64>,
    Option<&'static str>,
    Vec<&'static str>,
    bool,
);

#[tokio::test]
async fn list_applications_pages_in_name_order() {
    let emulator = Emulator::new();
    // Created out of order on purpose: the page order comes from the store, not from
    // the order the records arrived in.
    for name in ["delta", "alpha", "echo", "bravo", "charlie"] {
        emulator.create("paging fixture", simple_application(name)).await;
    }

    // (case, Limit, ExclusiveStartApplicationName, expected page, expected HasMoreApplications)
    let rows: Vec<PageCase> = vec![
        (
            "no limit",
            None,
            None,
            vec!["alpha", "bravo", "charlie", "delta", "echo"],
            false,
        ),
        ("limit at the lower bound", Some(1), None, vec!["alpha"], true),
        (
            "limit at the upper bound",
            Some(50),
            None,
            vec!["alpha", "bravo", "charlie", "delta", "echo"],
            false,
        ),
        (
            "limit exactly the number stored",
            Some(5),
            None,
            vec!["alpha", "bravo", "charlie", "delta", "echo"],
            false,
        ),
        (
            "limit one short",
            Some(4),
            None,
            vec!["alpha", "bravo", "charlie", "delta"],
            true,
        ),
        (
            "second page",
            Some(2),
            Some("alpha"),
            vec!["bravo", "charlie"],
            true,
        ),
        (
            "last page",
            Some(2),
            Some("charlie"),
            vec!["delta", "echo"],
            false,
        ),
        ("start after the last name", None, Some("echo"), vec![], false),
        (
            "start after a name that is not stored",
            None,
            Some("zulu"),
            vec![],
            false,
        ),
        (
            "start strictly after, mid-alphabet",
            None,
            Some("bz"),
            vec!["charlie", "delta", "echo"],
            false,
        ),
    ];

    for (case, limit, start_after, expected, expected_more) in rows {
        let mut request = serde_json::Map::new();
        if let Some(limit) = limit {
            request.insert("Limit".to_string(), json!(limit));
        }
        if let Some(start_after) = start_after {
            request.insert(
                "ExclusiveStartApplicationName".to_string(),
                json!(start_after),
            );
        }
        let listed = emulator
            .call("ListApplications", Value::Object(request))
            .await;
        let body = listed.ok(case);
        let no_summaries: Vec<Value> = Vec::new();
        let names: Vec<&str> = body["ApplicationSummaries"]
            .as_array()
            .unwrap_or(&no_summaries)
            .iter()
            .map(|entry| entry["ApplicationName"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: wrong page, got {:?}",
            body["ApplicationSummaries"]
        );
        assert_eq!(
            body["HasMoreApplications"].as_bool(),
            Some(expected_more),
            "case {case:?}: HasMoreApplications is true only when a record remains beyond the page, got {}",
            body["HasMoreApplications"]
        );
    }

    // One past each published bound of `ListApplicationsInputLimit`'s 1..50 range is
    // refused — by the boundary validator, or by the handler if it ever gets there.
    for (case, limit) in [
        ("below the lower bound", 0),
        ("above the upper bound", 51),
        ("negative", -1),
        ("far above the upper bound", 100_000),
    ] {
        let refused = emulator
            .call("ListApplications", json!({"Limit": limit}))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: Limit={limit} is outside the modelled 1..50 range, got HTTP {} with body {}",
            refused.status, refused.body
        );
    }
}

#[tokio::test]
async fn start_and_stop_move_the_stored_status_through_its_transient() {
    let emulator = Emulator::new();
    let case = "start then stop";
    emulator.create(case, simple_application("lifecycle")).await;

    let input_id = emulator.describe(case, "lifecycle").await["InputDescriptions"][0]["InputId"]
        .as_str()
        .expect("CreateApplication assigned an InputId")
        .to_string();

    let started = emulator
        .call(
            "StartApplication",
            json!({
                "ApplicationName": "lifecycle",
                "InputConfigurations": [{
                    "Id": input_id,
                    "InputStartingPositionConfiguration": {"InputStartingPosition": "NOW"}
                }]
            }),
        )
        .await;
    started.ok(case);

    // The move to RUNNING is owed, not instant: a caller reading straight back sees
    // the transient status, and starting again is refused in it.
    assert_eq!(
        emulator.status_of(case, "lifecycle").await,
        "STARTING",
        "a started application reports STARTING until the move to RUNNING comes due"
    );
    emulator
        .call(
            "StartApplication",
            json!({"ApplicationName": "lifecycle", "InputConfigurations": []}),
        )
        .await
        .assert_error("start while STARTING", 400, "ResourceInUseException");

    tokio::time::sleep(settle_wait()).await;
    assert_eq!(
        emulator.status_of(case, "lifecycle").await,
        "RUNNING",
        "the owed move to RUNNING must be observable once it has come due"
    );
    emulator
        .call(
            "StartApplication",
            json!({"ApplicationName": "lifecycle", "InputConfigurations": []}),
        )
        .await
        .assert_error("start while RUNNING", 400, "ResourceInUseException");

    let stopped = emulator
        .call("StopApplication", json!({"ApplicationName": "lifecycle"}))
        .await;
    stopped.ok(case);
    assert_eq!(
        emulator.status_of(case, "lifecycle").await,
        "STOPPING",
        "a stopped application reports STOPPING until the move to READY comes due"
    );
    emulator
        .call("StopApplication", json!({"ApplicationName": "lifecycle"}))
        .await
        .assert_error("stop while STOPPING", 400, "ResourceInUseException");

    tokio::time::sleep(settle_wait()).await;
    assert_eq!(
        emulator.status_of(case, "lifecycle").await,
        "READY",
        "the owed move back to READY must be observable once it has come due"
    );
    emulator
        .call("StopApplication", json!({"ApplicationName": "lifecycle"}))
        .await
        .assert_error("stop while READY", 400, "ResourceInUseException");

    // Starting is not a configuration change, so the version does not move.
    assert_eq!(
        emulator.describe(case, "lifecycle").await["ApplicationVersionId"].as_i64(),
        Some(1),
        "start/stop must not bump ApplicationVersionId"
    );
}

#[tokio::test]
async fn start_application_stores_the_starting_position_it_was_given() {
    let emulator = Emulator::new();

    // Every value the model declares for `InputStartingPosition`
    // (`ops::types::INPUT_STARTING_POSITION_VALUES`), one fresh application each.
    for (index, position) in INPUT_STARTING_POSITION_VALUES.iter().enumerate() {
        let case = format!("starting position {position}");
        let name = format!("position-{index}");
        emulator.create(&case, simple_application(&name)).await;
        let input_id = emulator.describe(&case, &name).await["InputDescriptions"][0]["InputId"]
            .as_str()
            .expect("CreateApplication assigned an InputId")
            .to_string();

        emulator
            .call(
                "StartApplication",
                json!({
                    "ApplicationName": name,
                    "InputConfigurations": [{
                        "Id": input_id,
                        "InputStartingPositionConfiguration": {"InputStartingPosition": position}
                    }]
                }),
            )
            .await
            .ok(&case);

        let described = emulator.describe(&case, &name).await;
        assert_eq!(
            described["InputDescriptions"][0]["InputStartingPositionConfiguration"]
                ["InputStartingPosition"]
                .as_str(),
            Some(*position),
            "case {case:?}: the starting position must be stored on the input, got {}",
            described["InputDescriptions"][0]
        );
    }

    // An id no input of this application carries is not a configuration it has, an
    // empty list names nothing to start, and a repeated id names one input twice.
    let case = "input configuration rows";
    emulator.create(case, simple_application("unknown-id")).await;
    let stored_id = emulator.describe(case, "unknown-id").await["InputDescriptions"][0]["InputId"]
        .as_str()
        .expect("CreateApplication assigned an InputId")
        .to_string();
    let rows: Vec<(&str, Value)> = vec![
        (
            "unknown input id",
            json!([{"Id": "999999", "InputStartingPositionConfiguration": {"InputStartingPosition": "NOW"}}]),
        ),
        ("no input configuration at all", json!([])),
        (
            "the same input configured twice",
            json!([
                {"Id": stored_id, "InputStartingPositionConfiguration": {"InputStartingPosition": "NOW"}},
                {"Id": stored_id, "InputStartingPositionConfiguration": {"InputStartingPosition": "TRIM_HORIZON"}}
            ]),
        ),
    ];
    for (case, configurations) in rows {
        emulator
            .call(
                "StartApplication",
                json!({"ApplicationName": "unknown-id", "InputConfigurations": configurations}),
            )
            .await
            .assert_error(case, 400, "InvalidArgumentException");
        assert_eq!(
            emulator.status_of(case, "unknown-id").await,
            "READY",
            "case {case:?}: a refused start must leave the application READY"
        );
    }
}

#[tokio::test]
async fn delete_application_requires_the_stored_create_timestamp() {
    let emulator = Emulator::new();
    emulator
        .create("delete", simple_application("deletable"))
        .await;
    let detail = emulator.describe("delete", "deletable").await;
    let create_timestamp = detail["CreateTimestamp"].clone();
    let seconds = create_timestamp.as_f64().expect("a create time stamp");

    // Any time stamp but the stored one names a record this caller has not seen.
    let rows: Vec<(&str, Value)> = vec![
        ("one second earlier", json!(seconds - 1.0)),
        ("one second later", json!(seconds + 1.0)),
        ("one millisecond later", json!(seconds + 0.001)),
        ("the epoch", json!(0)),
        ("far future", json!(seconds + 86_400.0)),
    ];
    for (case, timestamp) in rows {
        let refused = emulator
            .call(
                "DeleteApplication",
                json!({"ApplicationName": "deletable", "CreateTimestamp": timestamp}),
            )
            .await;
        refused.assert_error(case, 409, "ConcurrentModificationException");
        assert_eq!(
            emulator.describe(case, "deletable").await,
            detail,
            "case {case:?}: a refused delete must leave the record in place, unchanged"
        );
    }

    // A time stamp no instant can represent is refused the same way, not with a
    // panic or a 5xx: the seconds saturate and the record cannot match.
    for (case, timestamp) in [
        ("ten quintillion seconds", 1e19),
        ("1e300 seconds", 1e300),
        ("minus 1e300 seconds", -1e300),
    ] {
        let refused = emulator
            .call(
                "DeleteApplication",
                json!({"ApplicationName": "deletable", "CreateTimestamp": timestamp}),
            )
            .await;
        assert!(
            (400..500).contains(&refused.status),
            "case {case:?}: an unrepresentable CreateTimestamp must answer a 4xx modelled error, got HTTP {} with body {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            emulator.describe(case, "deletable").await,
            detail,
            "case {case:?}: a refused delete must leave the record in place, unchanged"
        );
    }

    // The stored one deletes it.
    emulator
        .call(
            "DeleteApplication",
            json!({"ApplicationName": "deletable", "CreateTimestamp": create_timestamp}),
        )
        .await
        .ok("matching create time stamp");

    emulator
        .call(
            "DescribeApplication",
            json!({"ApplicationName": "deletable"}),
        )
        .await
        .assert_error("deleted", 400, "ResourceNotFoundException");
    let listed = emulator.call("ListApplications", json!({})).await;
    assert_eq!(
        listed.ok("deleted")["ApplicationSummaries"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "a deleted application must be gone from the list too"
    );

    // And the name is free again.
    emulator
        .create("recreate after delete", simple_application("deletable"))
        .await;
    assert_eq!(
        emulator.status_of("recreate after delete", "deletable").await,
        "READY",
        "a name must be reusable once its application is deleted"
    );
}

#[tokio::test]
async fn operations_on_an_unknown_application_report_resource_not_found() {
    let emulator = Emulator::new();
    // One row per operation that resolves an application by name.
    let rows: Vec<(&str, Value)> = vec![
        (
            "DescribeApplication",
            json!({"ApplicationName": "no-such-app"}),
        ),
        (
            "StartApplication",
            json!({"ApplicationName": "no-such-app", "InputConfigurations": []}),
        ),
        ("StopApplication", json!({"ApplicationName": "no-such-app"})),
        (
            "DeleteApplication",
            json!({"ApplicationName": "no-such-app", "CreateTimestamp": 1_761_000_000}),
        ),
    ];

    for (operation, request) in rows {
        emulator
            .call(operation, request)
            .await
            .assert_error(operation, 400, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn create_application_refuses_more_sources_and_destinations_than_an_application_has() {
    let emulator = Emulator::new();
    let one_input = simple_application("caps")["Inputs"][0].clone();
    let one_output = json!({
        "Name": "OUT",
        "KinesisStreamsOutput": {
            "ResourceARN": "arn:aws:kinesis:us-west-2:000000000000:stream/out",
            "RoleARN": "arn:aws:iam::000000000000:role/ka-write"
        },
        "DestinationSchema": {"RecordFormatType": "JSON"}
    });

    // One streaming source and up to three destinations, per this operation's doc.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "two streaming sources",
            json!({"ApplicationName": "two-inputs", "Inputs": [one_input.clone(), one_input.clone()]}),
            "InvalidArgumentException",
        ),
        (
            "four destinations",
            json!({
                "ApplicationName": "four-outputs",
                "Outputs": [one_output.clone(), one_output.clone(), one_output.clone(), one_output.clone()]
            }),
            "InvalidArgumentException",
        ),
        (
            "two CloudWatch log streams",
            json!({
                "ApplicationName": "two-logs",
                "CloudWatchLoggingOptions": [
                    {
                        "LogStreamARN": "arn:aws:logs:us-west-2:000000000000:log-group:ka:log-stream:a",
                        "RoleARN": "arn:aws:iam::000000000000:role/ka-logs"
                    },
                    {
                        "LogStreamARN": "arn:aws:logs:us-west-2:000000000000:log-group:ka:log-stream:b",
                        "RoleARN": "arn:aws:iam::000000000000:role/ka-logs"
                    }
                ]
            }),
            "InvalidArgumentException",
        ),
        (
            "duplicate tag key",
            json!({
                "ApplicationName": "dup-tag",
                "Tags": [{"Key": "team", "Value": "a"}, {"Key": "team", "Value": "b"}]
            }),
            "InvalidArgumentException",
        ),
        (
            "fifty-one user tags",
            json!({
                "ApplicationName": "many-tags",
                "Tags": (0..51).map(|n| json!({"Key": format!("k{n}"), "Value": "v"})).collect::<Vec<_>>()
            }),
            "TooManyTagsException",
        ),
    ];

    for (case, request, expected) in rows {
        let refused = emulator.call("CreateApplication", request.clone()).await;
        refused.assert_error(case, 400, expected);
        // A refused create stores nothing.
        emulator
            .call(
                "DescribeApplication",
                json!({"ApplicationName": request["ApplicationName"].clone()}),
            )
            .await
            .assert_error(case, 400, "ResourceNotFoundException");
    }

    // The boundary cases the doc allows are accepted: one source, three destinations,
    // fifty tags.
    let accepted = json!({
        "ApplicationName": "at-the-caps",
        "Inputs": [one_input],
        "Outputs": [one_output.clone(), one_output.clone(), one_output],
        "Tags": (0..50).map(|n| json!({"Key": format!("k{n}"), "Value": "v"})).collect::<Vec<_>>()
    });
    emulator.create("at the caps", accepted).await;
    let detail = emulator.describe("at the caps", "at-the-caps").await;
    assert_eq!(
        detail["OutputDescriptions"].as_array().map(Vec::len),
        Some(3),
        "three destinations are within the documented cap, got {}",
        detail["OutputDescriptions"]
    );

    // `InputParallelism.Count` is bounded at both ends by the modelled 1..64 range:
    // one past either bound is refused and nothing is stored, so no stored record
    // can expand into an unbounded set of in-application stream names.
    let input_at = |count: i64| {
        let mut configured = simple_application("probe")["Inputs"][0].clone();
        configured["InputParallelism"] = json!({"Count": count});
        configured
    };
    for (case, count) in [
        ("below the parallelism lower bound", 0),
        ("above the parallelism upper bound", 65),
        ("far above the parallelism upper bound", 2_000_000),
    ] {
        let name = format!("parallelism-{count}");
        let refused = emulator
            .call(
                "CreateApplication",
                json!({"ApplicationName": name, "Inputs": [input_at(count)]}),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: Count={count} is outside the modelled 1..64 range, got HTTP {} with body {}",
            refused.status, refused.body
        );
        emulator
            .call("DescribeApplication", json!({"ApplicationName": name}))
            .await
            .assert_error(case, 400, "ResourceNotFoundException");
    }

    // And the published bounds themselves are accepted, each yielding that many
    // in-application stream names.
    for (case, count) in [
        ("parallelism lower bound", 1),
        ("parallelism upper bound", 64),
    ] {
        let name = format!("parallelism-ok-{count}");
        emulator
            .create(
                case,
                json!({"ApplicationName": name, "Inputs": [input_at(count)]}),
            )
            .await;
        let described = emulator.describe(case, &name).await;
        assert_eq!(
            described["InputDescriptions"][0]["InAppStreamNames"]
                .as_array()
                .map(Vec::len),
            Some(usize::try_from(count).unwrap()),
            "case {case:?}: one in-application stream name per parallelism count, got {}",
            described["InputDescriptions"][0]["InAppStreamNames"]
        );
    }
}

#[tokio::test]
async fn creating_past_the_application_quota_reports_the_limit() {
    let emulator = Emulator::new();
    // The documented default quota is 50 applications; the 51st is refused with the
    // modelled LimitExceededException and nothing is stored.
    for n in 0..50 {
        emulator
            .create("within the quota", simple_application(&format!("app-{n:03}")))
            .await;
    }
    let refused = emulator
        .call("CreateApplication", simple_application("app-050"))
        .await;
    refused.assert_error("past the quota", 400, "LimitExceededException");
    emulator
        .call("DescribeApplication", json!({"ApplicationName": "app-050"}))
        .await
        .assert_error("past the quota", 400, "ResourceNotFoundException");

    // Deleting one frees room for another.
    let detail = emulator.describe("free a slot", "app-000").await;
    emulator
        .call(
            "DeleteApplication",
            json!({"ApplicationName": "app-000", "CreateTimestamp": detail["CreateTimestamp"].clone()}),
        )
        .await
        .ok("free a slot");
    emulator
        .create("after freeing a slot", simple_application("app-050"))
        .await;
}

#[tokio::test]
async fn application_status_values_are_exactly_the_ones_the_model_declares() {
    // `APPLICATION_STATUS_VALUES` is generated from the model's `ApplicationStatus`
    // enum; the state's statuses must render as those values and no others.
    let statuses = [
        ApplicationStatus::Deleting,
        ApplicationStatus::Starting,
        ApplicationStatus::Stopping,
        ApplicationStatus::Ready,
        ApplicationStatus::Running,
        ApplicationStatus::Updating,
    ];
    let mut rendered: Vec<&str> = statuses.iter().map(|status| status.as_str()).collect();
    rendered.sort_unstable();
    let mut modelled: Vec<&str> = APPLICATION_STATUS_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        rendered, modelled,
        "every stored status must be a modelled ApplicationStatus, and every modelled value reachable"
    );
}

#[tokio::test]
async fn operations_this_slice_does_not_cover_still_answer_501() {
    let emulator = Emulator::new();
    let created = emulator
        .create("stub census", simple_application("still-stubbed"))
        .await;
    let name = created["ApplicationName"].as_str().unwrap().to_string();
    let arn = expected_arn(&name);
    let role = "arn:aws:iam::000000000000:role/ka";
    let schema = simple_application("probe")["Inputs"][0]["InputSchema"].clone();

    // One minimal, MODEL-VALID request per operation this slice leaves alone: each
    // must answer 501 `NotImplemented`, never an empty 200 and never a refusal that
    // would read as implemented behaviour.
    let rows: Vec<(&str, Value)> = vec![
        (
            "AddApplicationCloudWatchLoggingOption",
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 1,
                   "CloudWatchLoggingOption": {"LogStreamARN": "arn:aws:logs:us-west-2:000000000000:log-group:ka:log-stream:e", "RoleARN": role}}),
        ),
        (
            "AddApplicationInput",
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 1,
                   "Input": {"NamePrefix": "MORE", "InputSchema": schema}}),
        ),
        (
            "AddApplicationInputProcessingConfiguration",
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 1, "InputId": "1",
                   "InputProcessingConfiguration": {"InputLambdaProcessor": {"ResourceARN": "arn:aws:lambda:us-west-2:000000000000:function:pre", "RoleARN": role}}}),
        ),
        (
            "AddApplicationOutput",
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 1,
                   "Output": {"Name": "OUT", "DestinationSchema": {"RecordFormatType": "JSON"}}}),
        ),
        (
            "AddApplicationReferenceDataSource",
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 1,
                   "ReferenceDataSource": {"TableName": "LOOKUP", "ReferenceSchema": schema}}),
        ),
        (
            "DeleteApplicationCloudWatchLoggingOption",
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 1, "CloudWatchLoggingOptionId": "1"}),
        ),
        (
            "DeleteApplicationInputProcessingConfiguration",
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 1, "InputId": "1"}),
        ),
        (
            "DeleteApplicationOutput",
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 1, "OutputId": "1"}),
        ),
        (
            "DeleteApplicationReferenceDataSource",
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 1, "ReferenceId": "1"}),
        ),
        (
            "DiscoverInputSchema",
            json!({"ResourceARN": "arn:aws:kinesis:us-west-2:000000000000:stream/trades", "RoleARN": role,
                   "InputStartingPositionConfiguration": {"InputStartingPosition": "NOW"}}),
        ),
        ("ListTagsForResource", json!({"ResourceARN": arn})),
        (
            "TagResource",
            json!({"ResourceARN": arn, "Tags": [{"Key": "team", "Value": "analytics"}]}),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": arn, "TagKeys": ["team"]}),
        ),
        (
            "UpdateApplication",
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 1,
                   "ApplicationUpdate": {"ApplicationCodeUpdate": "CREATE OR REPLACE STREAM \"OUT\" (a INTEGER);"}}),
        ),
    ];
    assert_eq!(
        rows.len(),
        14,
        "this slice implements 6 of the service's 20 operations, so 14 stubs remain"
    );

    for (operation, request) in rows {
        let answered = emulator.call(operation, request).await;
        assert_eq!(
            answered.status, 501,
            "case {operation:?}: an operation nobody has written yet must answer 501, got HTTP {} with body {}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.code(),
            "NotImplemented",
            "case {operation:?}: the scaffold signal must stay NotImplemented, got {}",
            answered.body
        );
    }
}
